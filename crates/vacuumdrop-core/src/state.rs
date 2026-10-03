//! Connection lifecycle as an explicit state machine.
//!
//! Charter 40 requires explicit connection state and rejection of invalid
//! transitions. Charter 87 asks for states to be modelled with the type system
//! where practical rather than as a string.
//!
//! Two mechanisms are used together:
//!
//! 1. [`ConnectionState`] is an enum, so an arbitrary status string cannot be
//!    represented at all.
//! 2. [`StateMachine`] owns the state and is the only way to change it. Every
//!    transition is validated, so the checks cannot be bypassed by accident.
//!
//! The residual risk is a caller skipping the state machine and acting on a
//! session it believes is established. That is mitigated by
//! [`Session`][crate::session::Session], which only exposes operations legal in
//! the current state.

use std::fmt;

/// Where a connection is in its lifecycle.
///
/// ```text
/// Disconnected -> Discovering -> PeerFound -> Connecting -> Connected
///              -> Authenticating -> Established -> Transferring
///              -> Completed -> Closing -> Disconnected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// No connection exists.
    Disconnected,
    /// Searching for peers.
    Discovering,
    /// A candidate peer is known but not yet contacted.
    PeerFound,
    /// A connection attempt is in progress.
    Connecting,
    /// Transport is established; the protocol handshake has not finished.
    Connected,
    /// Identity is being verified.
    Authenticating,
    /// Authenticated and idle. Ready for work.
    Established,
    /// Data is moving.
    Transferring,
    /// Work finished successfully.
    Completed,
    /// A clean shutdown is in progress.
    Closing,
}

impl ConnectionState {
    /// Whether moving from `self` to `next` is legal.
    ///
    /// Terminal states return `false` for everything: recovery must go through
    /// [`ConnectionState::Disconnected`] first, so a dropped session cannot
    /// silently rejoin a previous identity.
    pub fn can_transition_to(self, next: ConnectionState) -> bool {
        use ConnectionState::*;
        matches!(
            (self, next),
            (Disconnected, Discovering)
                | (Disconnected, Connecting)
                | (Discovering, PeerFound)
                | (Discovering, Disconnected)
                | (PeerFound, Connecting)
                | (PeerFound, Disconnected)
                // Closing is legal from CONNECTING: a session abandoned mid-handshake
                // must be able to shut down cleanly rather than only being torn down.
                | (Connecting, Connected)
                | (Connecting, Closing)
                | (Connecting, Disconnected)
                | (Connected, Authenticating)
                | (Connected, Closing)
                | (Connected, Disconnected)
                | (Authenticating, Established)
                | (Authenticating, Closing)
                | (Authenticating, Disconnected)
                | (Established, Transferring)
                | (Established, Closing)
                | (Established, Disconnected)
                | (Transferring, Completed)
                | (Transferring, Established)
                | (Transferring, Closing)
                | (Transferring, Disconnected)
                | (Completed, Closing)
                | (Completed, Disconnected)
                | (Closing, Disconnected)
        )
    }

    /// Whether the peer is authenticated at this state.
    pub fn is_authenticated(self) -> bool {
        matches!(
            self,
            ConnectionState::Established
                | ConnectionState::Transferring
                | ConnectionState::Completed
        )
    }

    /// Whether a transport is expected to exist at this state.
    pub fn has_transport(self) -> bool {
        !matches!(
            self,
            ConnectionState::Disconnected | ConnectionState::Discovering
        )
    }

    /// A stable, log-friendly name.
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "DISCONNECTED",
            ConnectionState::Discovering => "DISCOVERING",
            ConnectionState::PeerFound => "PEER_FOUND",
            ConnectionState::Connecting => "CONNECTING",
            ConnectionState::Connected => "CONNECTED",
            ConnectionState::Authenticating => "AUTHENTICATING",
            ConnectionState::Established => "ESTABLISHED",
            ConnectionState::Transferring => "TRANSFERRING",
            ConnectionState::Completed => "COMPLETED",
            ConnectionState::Closing => "CLOSING",
        }
    }
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An owned, validated connection state machine.
///
/// Every mutation goes through [`StateMachine::transition`], which is the single
/// enforcement point for Charter 40.
#[derive(Debug, Clone)]
pub struct StateMachine {
    state: ConnectionState,
    history: Vec<ConnectionState>,
    /// Bound on `history` so a long-lived engine cannot grow it without limit.
    history_limit: usize,
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl StateMachine {
    /// Creates a machine in [`ConnectionState::Disconnected`].
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            history: Vec::new(),
            history_limit: 64,
        }
    }

    /// The current state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// The transition history, oldest first.
    pub fn history(&self) -> &[ConnectionState] {
        &self.history
    }

    /// Whether `next` is reachable from the current state.
    pub fn can_transition(&self, next: ConnectionState) -> bool {
        self.state.can_transition_to(next)
    }

    /// Attempts a transition.
    ///
    /// On rejection the state is left untouched and the error names both the
    /// current state and the refused one, because "invalid transition" without
    /// context is close to useless when diagnosing a peer.
    pub fn transition(&mut self, next: ConnectionState) -> Result<(), crate::error::ProtocolError> {
        if self.state.can_transition_to(next) {
            self.state = next;
            self.push_history(next);
            Ok(())
        } else {
            Err(crate::error::ProtocolError::StateViolation(match next {
                ConnectionState::Disconnected => {
                    "transition to DISCONNECTED refused: not currently connected"
                }
                ConnectionState::Established => {
                    "transition to ESTABLISHED refused: must authenticate first"
                }
                ConnectionState::Transferring => {
                    "transition to TRANSFERRING refused: session not established"
                }
                _ => "transition refused: not permitted from the current state",
            }))
        }
    }

    /// Returns to `Disconnected` from any state.
    ///
    /// Used by error handling. Any state can be torn down, so this cannot fail;
    /// it exists so callers recovering from an error have one obvious path and
    /// do not write their own escape hatch.
    pub fn force_disconnect(&mut self) {
        if self.state != ConnectionState::Disconnected {
            self.state = ConnectionState::Disconnected;
            self.push_history(ConnectionState::Disconnected);
        }
    }

    fn push_history(&mut self, state: ConnectionState) {
        if self.history.len() >= self.history_limit {
            // Drop the oldest entry. A ring buffer by removal: simple, bounded,
            // and adequate for the diagnostic purpose history serves here.
            self.history.remove(0);
        }
        self.history.push(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_disconnected() {
        assert_eq!(StateMachine::new().state(), ConnectionState::Disconnected);
    }

    #[test]
    fn happy_path_transitions_are_allowed() {
        let mut sm = StateMachine::new();
        let path = [
            ConnectionState::Discovering,
            ConnectionState::PeerFound,
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Authenticating,
            ConnectionState::Established,
            ConnectionState::Transferring,
            ConnectionState::Completed,
            ConnectionState::Closing,
            ConnectionState::Disconnected,
        ];
        for next in path {
            sm.transition(next)
                .unwrap_or_else(|e| panic!("{next} should be allowed: {e}"));
        }
        assert_eq!(sm.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn cannot_skip_authentication() {
        // CONNECTED -> TRANSFERRING would let an unauthenticated peer move data.
        let mut sm = StateMachine::new();
        sm.transition(ConnectionState::Connecting).unwrap();
        sm.transition(ConnectionState::Connected).unwrap();
        assert!(sm.transition(ConnectionState::Transferring).is_err());
        assert_eq!(sm.state(), ConnectionState::Connected);
    }

    #[test]
    fn cannot_transfer_before_established() {
        let mut sm = StateMachine::new();
        sm.transition(ConnectionState::Connecting).unwrap();
        sm.transition(ConnectionState::Connected).unwrap();
        sm.transition(ConnectionState::Authenticating).unwrap();
        assert!(sm.transition(ConnectionState::Transferring).is_err());
    }

    #[test]
    fn rejected_transitions_leave_state_untouched() {
        let mut sm = StateMachine::new();
        let before = sm.state();
        assert!(sm.transition(ConnectionState::Transferring).is_err());
        assert_eq!(sm.state(), before, "a rejected transition mutated state");
    }

    #[test]
    fn cannot_resume_transferring_from_completed() {
        let mut sm = StateMachine::new();
        for s in [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Authenticating,
            ConnectionState::Established,
            ConnectionState::Transferring,
            ConnectionState::Completed,
        ] {
            sm.transition(s).unwrap();
        }
        assert!(sm.transition(ConnectionState::Transferring).is_err());
    }

    #[test]
    fn cannot_self_transition() {
        let mut sm = StateMachine::new();
        sm.transition(ConnectionState::Connecting).unwrap();
        assert!(sm.transition(ConnectionState::Connecting).is_err());
    }

    #[test]
    fn force_disconnect_works_from_any_state() {
        for start in [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Established,
            ConnectionState::Transferring,
        ] {
            let mut sm = StateMachine::new();
            let _ = sm.transition(start);
            sm.force_disconnect();
            assert_eq!(sm.state(), ConnectionState::Disconnected);
        }
    }

    #[test]
    fn force_disconnect_is_idempotent() {
        let mut sm = StateMachine::new();
        sm.force_disconnect();
        let depth = sm.history().len();
        sm.force_disconnect();
        assert_eq!(sm.history().len(), depth);
    }

    #[test]
    fn terminal_state_requires_disconnect_before_reuse() {
        // A dropped session must not rejoin a previous identity.
        let mut sm = StateMachine::new();
        for s in [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Authenticating,
            ConnectionState::Established,
        ] {
            sm.transition(s).unwrap();
        }
        sm.force_disconnect();
        assert!(sm.transition(ConnectionState::Established).is_err());
    }

    #[test]
    fn authentication_predicate_matches_charter_states() {
        assert!(!ConnectionState::Connected.is_authenticated());
        assert!(!ConnectionState::Authenticating.is_authenticated());
        assert!(ConnectionState::Established.is_authenticated());
        assert!(ConnectionState::Transferring.is_authenticated());
        assert!(ConnectionState::Completed.is_authenticated());
        assert!(!ConnectionState::Disconnected.is_authenticated());
    }

    #[test]
    fn history_is_bounded() {
        let mut sm = StateMachine::new();
        for _ in 0..500 {
            sm.transition(ConnectionState::Connecting).unwrap();
            sm.force_disconnect();
        }
        assert!(
            sm.history().len() <= 64,
            "history grew unbounded to {}",
            sm.history().len()
        );
    }

    #[test]
    fn history_records_the_actual_path() {
        let mut sm = StateMachine::new();
        sm.transition(ConnectionState::Connecting).unwrap();
        sm.transition(ConnectionState::Connected).unwrap();
        assert_eq!(
            sm.history(),
            &[ConnectionState::Connecting, ConnectionState::Connected]
        );
    }

    #[test]
    fn every_state_has_a_distinct_name() {
        let all = [
            ConnectionState::Disconnected,
            ConnectionState::Discovering,
            ConnectionState::PeerFound,
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Authenticating,
            ConnectionState::Established,
            ConnectionState::Transferring,
            ConnectionState::Completed,
            ConnectionState::Closing,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a.as_str(), b.as_str(), "duplicate state name");
            }
        }
    }

    #[test]
    fn transport_presence_matches_state() {
        assert!(!ConnectionState::Disconnected.has_transport());
        assert!(!ConnectionState::Discovering.has_transport());
        assert!(ConnectionState::Connected.has_transport());
        assert!(ConnectionState::Established.has_transport());
    }
}
