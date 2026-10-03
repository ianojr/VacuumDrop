//! Session: a protocol conversation over a transport.
//!
//! This module joins the pieces: a [`Transport`] carrying bytes, a [`FrameCodec`]
//! framing them, and a [`StateMachine`] tracking where the conversation is. It
//! is the only place in the engine where those three meet.
//!
//! # Layering
//!
//! ```text
//! session.rs   state machine + timeouts + correlation
//!   |
//!   +-- protocol (FrameCodec, VacuumDropMessage, handshake)
//!   +-- transport (Transport: bytes)
//!   +-- core (StateMachine, Limits, errors)
//! ```
//!
//! The session knows about all three and still contains no Wi-Fi Direct concept.
//! That is the property ADR-0004 exists to protect, and it is what lets the same
//! session run over TCP, a future USB transport, or a test harness.
//!
//! # Phase 1 scope
//!
//! Connect, handshake, keepalive, close. Transfer messages are Phase 5 and are
//! absent rather than stubbed.

use std::sync::Arc;
use std::time::Duration;

use vacuumdrop_core::{
    Capability, ConnectionState, Error, Event, Limits, NegotiatedVersion, PeerDescription,
    ProtocolError, SessionId, StateMachine, TransportError, Violation,
};
use vacuumdrop_transport::Transport;

use crate::codec::FrameCodec;
use crate::handshake::{ClientHandshake, HandshakeError, ServerHandshake, SessionParameters};
use crate::message::{CapabilitySet, GoodbyeReason, HelloStatus, Ping, Pong, VacuumDropMessage};

/// Failures a session can report.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The transport failed.
    #[error("transport failed: {0}")]
    Transport(#[from] TransportError),

    /// The codec failed while framing or parsing.
    #[error("framing failed: {0}")]
    Frame(#[from] crate::codec::FrameError),

    /// The protocol failed.
    #[error("protocol failed: {0}")]
    Protocol(#[from] ProtocolError),

    /// The handshake failed or was refused.
    #[error("handshake failed: {0}")]
    Handshake(#[from] HandshakeError),

    /// No protocol version is acceptable to both peers.
    #[error("version negotiation failed: {0}")]
    UnsupportedVersion(#[from] vacuumdrop_core::UnsupportedVersionError),

    /// A state transition was refused.
    #[error("invalid session state: {0}")]
    InvalidState(&'static str),

    /// The peer declined to proceed.
    #[error("peer refused the session: {0}")]
    Refused(&'static str),

    /// No message arrived within the deadline.
    #[error("timed out waiting for {operation}")]
    Timeout {
        /// What was being waited on.
        operation: &'static str,
        /// The deadline that elapsed.
        limit: Duration,
    },

    /// The peer closed cleanly before the expected exchange completed.
    ///
    /// Distinct from [`SessionError::Timeout`] on purpose. The distinction is
    /// only useful in one direction: a *close* mid-handshake is not a timeout,
    /// and reporting it as one would send a user looking at the wrong layer.
    #[error("peer closed before sending {operation} (waited {limit:?})")]
    DeadlineExceeded {
        /// What was being waited on.
        operation: &'static str,
        /// How long we waited before concluding the peer would not send it.
        limit: Duration,
    },

    /// A caller closed the session.
    #[error("session closed")]
    Closed,

    /// Validation failed.
    #[error("invalid session parameter: {0}")]
    InvalidParameter(#[from] Violation),
}

impl From<SessionError> for Error {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::Transport(t) => Error::Transport(t),
            // A framing failure is already an engine error, so it is preserved
            // rather than being flattened into a protocol error: a dead socket
            // must stay distinguishable from a protocol violation.
            SessionError::Frame(f) => f.into(),
            SessionError::Protocol(p) => Error::Protocol(p),
            SessionError::Handshake(h) => Error::Protocol(h.into()),
            SessionError::UnsupportedVersion(v) => Error::UnsupportedVersion(v),
            SessionError::InvalidState(s) => Error::Protocol(ProtocolError::StateViolation(s)),
            SessionError::Refused(r) => {
                Error::Authentication(vacuumdrop_core::AuthenticationError::Rejected(r))
            }
            SessionError::Timeout { operation, limit }
            | SessionError::DeadlineExceeded { operation, limit } => {
                Error::Timeout(vacuumdrop_core::TimeoutError::new(operation, limit))
            }
            SessionError::Closed => {
                Error::PeerDisconnected(vacuumdrop_core::PeerDisconnectedError::Closed)
            }
            SessionError::InvalidParameter(v) => Error::Protocol(ProtocolError::InvalidField {
                field: v.field,
                reason: v.reason,
            }),
        }
    }
}

/// Configuration for one end of a session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// This device's declared name.
    pub device_name: String,
    /// This device's advertised capabilities.
    pub capabilities: Vec<Capability>,
    /// The largest chunk this device will accept.
    pub max_chunk_size: u32,
    /// Resource limits.
    pub limits: Limits,
}

impl SessionConfig {
    /// A configuration with the given name and default limits.
    pub fn new(device_name: impl Into<String>) -> Self {
        Self {
            device_name: device_name.into(),
            capabilities: vec![Capability::SupportsResume],
            // 8 MiB: the ADR-0006 starting experiment. Not a permanent value.
            max_chunk_size: 8 * 1024 * 1024,
            limits: Limits::new(),
        }
    }

    fn capability_flags(&self) -> Vec<u8> {
        self.capabilities.iter().map(|c| c.discriminant()).collect()
    }

    /// Rejects a configuration that could not be enforced safely.
    ///
    /// Public because the CLI must validate before opening a socket: a bad
    /// device name discovered after `listen` has already bound is a worse
    /// failure than one caught before.
    pub fn validate(&self) -> Result<(), Violation> {
        if self.device_name.is_empty() {
            return Err(Violation::new("device_name", "must not be empty"));
        }
        if self.device_name.len() > self.limits.max_name_length {
            return Err(Violation::new(
                "device_name",
                "exceeds the configured maximum length",
            ));
        }
        if self.device_name.chars().any(|c| c.is_control()) {
            return Err(Violation::new("device_name", "contains control characters"));
        }
        if self.max_chunk_size == 0 {
            return Err(Violation::new("max_chunk_size", "must be non-zero"));
        }
        if self.max_chunk_size > self.limits.max_frame_length {
            return Err(Violation::new(
                "max_chunk_size",
                "exceeds our maximum frame length",
            ));
        }
        self.limits.validate()
    }
}

/// A protocol session over a transport.
pub struct Session {
    id: SessionId,
    transport: Arc<dyn Transport>,
    codec: FrameCodec,
    machine: StateMachine,
    params: Option<SessionParameters>,
    pending_ping: Option<u64>,
    next_request_id: u64,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("state", &self.machine.state())
            .field("transport", &self.transport.describe())
            .field("negotiated", &self.params.as_ref().map(|p| p.version))
            .finish()
    }
}

impl Session {
    /// Wraps a transport and a codec into a session.
    ///
    /// The session starts `Disconnected` and moves to `Connecting`. Callers
    /// drive the state machine through the typed operations below rather than by
    /// setting states, so the Charter 40 checks cannot be bypassed.
    pub fn new(
        id: SessionId,
        transport: Arc<dyn Transport>,
        limits: Limits,
    ) -> Result<Self, SessionError> {
        let codec = FrameCodec::for_session(id.get(), limits.max_frame_length);
        let mut machine = StateMachine::new();
        machine
            .transition(ConnectionState::Connecting)
            .map_err(SessionError::Protocol)?;
        Ok(Self {
            id,
            transport,
            codec,
            machine,
            params: None,
            pending_ping: None,
            next_request_id: 1,
        })
    }

    /// The session id, for log correlation.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// The current state.
    pub fn state(&self) -> ConnectionState {
        self.machine.state()
    }

    /// The negotiated parameters, once the handshake has completed.
    pub fn parameters(&self) -> Option<&SessionParameters> {
        self.params.as_ref()
    }

    /// The peer's validated description, once known.
    pub fn peer(&self) -> Option<&PeerDescription> {
        self.params.as_ref().map(|p| &p.peer)
    }

    /// Moves to `Connected`, after the transport is up.
    pub fn mark_connected(&mut self) -> Result<(), SessionError> {
        self.machine
            .transition(ConnectionState::Connected)
            .map_err(SessionError::Protocol)
    }

    /// Moves to `Authenticating`.
    pub fn mark_authenticating(&mut self) -> Result<(), SessionError> {
        self.machine
            .transition(ConnectionState::Authenticating)
            .map_err(SessionError::Protocol)
    }

    /// Moves to `Established`, recording the negotiated parameters.
    ///
    /// Phase 1 has no cryptographic authentication, so this records the
    /// *handshake* result. A Phase 4 session must not reach this state until
    /// [`Session::mark_authenticated`] has run; the CLI states the difference in
    /// plain language so nobody mistakes a version handshake for trust.
    pub fn mark_established(&mut self, params: SessionParameters) -> Result<(), SessionError> {
        self.machine
            .transition(ConnectionState::Established)
            .map_err(SessionError::Protocol)?;
        self.params = Some(params);
        Ok(())
    }

    /// Records that cryptographic authentication succeeded.
    ///
    /// A no-op in Phase 1 because nothing authenticates yet. It exists so
    /// Phase 4 has an obvious place to record the real result rather than
    /// reordering the state machine later.
    pub fn mark_authenticated(&mut self) -> Result<(), SessionError> {
        if self.machine.state() != ConnectionState::Authenticating {
            return Err(SessionError::InvalidState(
                "authentication can only complete from AUTHENTICATING",
            ));
        }
        Ok(())
    }

    /// Allocates the next correlation id.
    ///
    /// Monotonic rather than random: it is a request counter, not a secret, and
    /// a predictable counter is easier to debug.
    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Sends one message, advancing the state machine where required.
    pub async fn send(&mut self, msg: VacuumDropMessage) -> Result<(), SessionError> {
        if self.machine.state() == ConnectionState::Closing
            || self.machine.state() == ConnectionState::Disconnected
        {
            return Err(SessionError::Closed);
        }

        // A responder to a Ping must not itself be answered with a Ping, or two
        // peers can livelock. Requests are only permitted from an established
        // or connected session, and a Ping may only be sent from `Established`.
        if matches!(msg, VacuumDropMessage::Ping(_))
            && self.machine.state() != ConnectionState::Established
        {
            return Err(SessionError::InvalidState(
                "ping may only be sent from an established session",
            ));
        }

        self.send_raw(msg).await
    }

    async fn send_raw(&mut self, msg: VacuumDropMessage) -> Result<(), SessionError> {
        // Encode into a standalone buffer, then write it. Encoding separately
        // keeps a codec failure from writing a partial frame.
        let mut buf = tokio_util::bytes::BytesMut::new();
        use tokio_util::codec::Encoder;
        self.codec
            .encode(msg, &mut buf)
            .map_err(SessionError::Frame)?;

        let bytes = buf.freeze();
        let mut written = 0usize;
        while written < bytes.len() {
            let n = self.transport.send(&bytes[written..]).await?;
            if n == 0 {
                return Err(SessionError::Closed);
            }
            written += n;
        }
        Ok(())
    }

    /// Receives the next message, bounded by `timeout`.
    ///
    /// Returns `Ok(None)` when the peer closed cleanly.
    pub async fn receive(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<VacuumDropMessage>, SessionError> {
        use tokio_util::codec::Decoder;

        let mut scratch = tokio_util::bytes::BytesMut::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            // Try to parse whatever is already buffered before blocking on I/O.
            // A frame may be complete even though the read returned fewer bytes.
            if let Some(msg) = self
                .codec
                .decode(&mut scratch)
                .map_err(SessionError::Frame)?
            {
                return Ok(Some(self.observe(msg)));
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(SessionError::Timeout {
                    operation: "inbound message",
                    limit: timeout,
                });
            }

            let mut chunk = [0u8; 8192];
            let n = match tokio::time::timeout(remaining, self.transport.receive(&mut chunk)).await
            {
                Ok(Ok(n)) => n,
                Ok(Err(TransportError::ConnectTimeout(_))) => {
                    return Err(SessionError::Timeout {
                        operation: "inbound message",
                        limit: timeout,
                    });
                }
                Ok(Err(TransportError::Closed)) => return Ok(None),
                Ok(Err(e)) => return Err(SessionError::Transport(e)),
                Err(_) => {
                    return Err(SessionError::Timeout {
                        operation: "inbound message",
                        limit: timeout,
                    });
                }
            };

            if n == 0 {
                // Clean end of stream.
                return Ok(None);
            }
            scratch.extend_from_slice(&chunk[..n]);
        }
    }

    /// Applies a side effect of an inbound message to the session state.
    ///
    /// Keeps state transitions in one place: a Ping received in
    /// `Authenticating` moves the session to `Established`, because liveness
    /// from a peer that has completed version negotiation is what "connected"
    /// means in practice.
    fn observe(&mut self, msg: VacuumDropMessage) -> VacuumDropMessage {
        match &msg {
            VacuumDropMessage::Ping(p) => {
                self.pending_ping = Some(p.nonce);
            }
            VacuumDropMessage::Pong(p) => {
                // Only clear if it answers a ping we actually sent; a stray Pong
                // must not look like a completed keepalive.
                if self.pending_ping == Some(p.nonce) {
                    self.pending_ping = None;
                }
            }
            VacuumDropMessage::HelloAck(ack) => {
                if matches!(ack.status, HelloStatus::Accepted)
                    && self.machine.state() == ConnectionState::Authenticating
                {
                    let _ = self.machine.transition(ConnectionState::Established);
                }
            }
            _ => {}
        }
        msg
    }

    /// Whether a keepalive is outstanding.
    pub fn has_pending_ping(&self) -> bool {
        self.pending_ping.is_some()
    }

    /// Sends a keepalive probe.
    ///
    /// Routed through [`Session::send`] so the state guard applies. A probe
    /// permitted outside `ESTABLISHED` would let two peers ping each other during
    /// a handshake and livelock answering.
    pub async fn ping(&mut self, nonce: u64) -> Result<(), SessionError> {
        self.send(VacuumDropMessage::Ping(Ping { nonce })).await?;
        self.pending_ping = Some(nonce);
        Ok(())
    }

    /// Builds the reply to a received Ping.
    pub fn pong_for(nonce: u64) -> VacuumDropMessage {
        VacuumDropMessage::Pong(Pong { nonce })
    }

    /// Ends the session cleanly.
    ///
    /// Sends `Goodbye` when it can, then closes the transport. A failure to send
    /// the courtesy message does not prevent the close: a peer that has already
    /// gone away should not leave us hanging.
    pub async fn close(&mut self, reason: GoodbyeReason) -> Result<(), SessionError> {
        if self.machine.state() == ConnectionState::Disconnected {
            return Err(SessionError::Closed);
        }
        if self.machine.state() != ConnectionState::Closing {
            let _ = self.send_raw(crate::handshake::goodbye(reason)).await;
            self.machine
                .transition(ConnectionState::Closing)
                .map_err(SessionError::Protocol)?;
        }
        let result = self.transport.shutdown().await;
        self.machine.force_disconnect();
        result.map_err(SessionError::Transport)
    }

    /// Events describing the session's current state.
    ///
    /// Phase 1 exposes the session-level subset. Transfer events arrive in
    /// Phase 5 with the transfer engine.
    pub fn events(&self) -> Vec<Event> {
        let peer = match self.peer() {
            Some(p) => p,
            None => return vec![],
        };
        // Identity is minted from the session id until Phase 4 provides real
        // cryptographic identity. Phase 1 sessions are unauthenticated and the
        // CLI says so, so a placeholder that is visibly a placeholder is safer
        // than one that looks like a real key.
        let id = vacuumdrop_core::PeerId::from_public_key({
            let mut k = [0u8; 32];
            k[..8].copy_from_slice(&self.id.get().to_le_bytes());
            k
        });

        match self.state() {
            ConnectionState::Established | ConnectionState::Transferring => vec![
                Event::ConnectionEstablished {
                    peer: id,
                    version: self
                        .params
                        .as_ref()
                        .map(|p| p.version)
                        .unwrap_or(NegotiatedVersion(1)),
                },
                Event::PeerDiscovered {
                    id,
                    address: vacuumdrop_core::PeerAddress::PlatformHandle {
                        handle: self.transport.describe(),
                    },
                },
            ],
            _ => {
                let _ = peer;
                vec![]
            }
        }
    }
}

impl Session {
    /// Runs the client side of a handshake to completion.
    ///
    /// Drives the real session, so framing, timeouts, and state transitions all
    /// apply. On success the session is `ESTABLISHED` and its parameters are
    /// recorded; the caller only has to print them.
    ///
    /// ```text
    /// -> VersionNegotiate
    /// <- VersionAck
    /// -> Hello
    /// <- HelloAck            (a rejection ends the exchange here)
    /// -> Capabilities
    /// <- Capabilities
    /// ```
    ///
    /// Both sides send capabilities. A client that never learned the server's
    /// capabilities would be guessing at them for the rest of the session, which
    /// is exactly what Charter 67 warns against.
    pub async fn run_client_handshake(
        &mut self,
        config: &SessionConfig,
    ) -> Result<SessionParameters, SessionError> {
        config.validate()?;
        self.mark_connected()?;

        let mut client = ClientHandshake::new(
            config.device_name.clone(),
            crate::CRATE_VERSION,
            config.limits.clone(),
            config.capability_flags(),
            config.max_chunk_size,
        );

        let timeout = config.limits.handshake_timeout;

        let opener = client.opener();
        self.send(opener).await?;

        let ack = next(self, "VersionAck", timeout).await?;
        let hello = client.on_version_ack(ack)?;
        self.send(hello).await?;

        let hello_ack = next(self, "HelloAck", timeout).await?;
        let ack = client.on_hello_ack(hello_ack)?;
        if let HelloStatus::Rejected(reason) = ack.status {
            self.machine.force_disconnect();
            return Err(SessionError::Refused(reason.reason()));
        }

        // Phase 1 performs no cryptographic authentication, so the session moves
        // AUTHENTICATING -> ESTABLISHED once the exchange completes. Phase 4
        // replaces this with a real proof of identity; the transition points do
        // not change, only what justifies them.
        self.mark_authenticating()?;

        self.send(client.capabilities()).await?;

        // The server's capabilities arrive last, so a client cannot decide what
        // the session supports until it has them.
        let server_caps = next(self, "Capabilities", timeout).await?;
        let CapabilitySet {
            features,
            max_chunk_size,
        } = match server_caps {
            VacuumDropMessage::Capabilities(set) => set,
            other => {
                return Err(SessionError::Protocol(ProtocolError::UnexpectedMessage {
                    message: other.name(),
                    state: "Capabilities",
                }));
            }
        };
        // Validate against our own limits before believing any of it.
        CapabilitySet {
            features: features.clone(),
            max_chunk_size,
        }
        .validate(&config.limits)?;

        let params = SessionParameters {
            version: NegotiatedVersion::negotiate(&[NegotiatedVersion::LOCAL])?,
            peer: PeerDescription::new(
                ack.device_name,
                vec![NegotiatedVersion::LOCAL],
                CapabilitySet {
                    features,
                    max_chunk_size,
                }
                .capabilities(),
                max_chunk_size,
                &config.limits,
            )?,
            local_capabilities: config.capability_flags(),
            max_chunk_size: config.max_chunk_size,
        };

        self.mark_established(params.clone())?;
        Ok(params)
    }

    /// Runs the server side of a handshake to completion.
    ///
    /// Mirrors [`Session::run_client_handshake`]: on success the session is
    /// `ESTABLISHED` with its parameters recorded.
    pub async fn run_server_handshake(
        &mut self,
        config: &SessionConfig,
    ) -> Result<SessionParameters, SessionError> {
        config.validate()?;
        self.mark_connected()?;

        let mut server = ServerHandshake::new(
            config.device_name.clone(),
            config.limits.clone(),
            config.capability_flags(),
            config.max_chunk_size,
        );

        let timeout = config.limits.handshake_timeout;

        let offer = next(self, "VersionNegotiate", timeout).await?;
        let ack = server.on_version_negotiate(offer)?;
        self.send(ack).await?;

        let hello = next(self, "Hello", timeout).await?;
        let hello_ack = server.on_hello(hello)?;
        self.send(hello_ack).await?;

        self.mark_authenticating()?;

        let caps = next(self, "Capabilities", timeout).await?;
        let params = server.on_capabilities(caps)?;

        // Send our own capabilities last. A client that has not received them
        // cannot know what this session supports.
        self.send(VacuumDropMessage::Capabilities(CapabilitySet {
            features: config.capability_flags(),
            max_chunk_size: config.max_chunk_size,
        }))
        .await?;

        self.mark_established(params.clone())?;
        Ok(params)
    }
}

/// Awaits the next message, requiring it to be the expected one.
///
/// A clean close mid-handshake is reported as a timeout on the message we were
/// waiting for, because from the caller's point of view the important fact is
/// that the expected exchange never completed. Reporting `Closed` instead would
/// bury the cause under a generic "peer disconnected".
async fn next(
    session: &mut Session,
    expected: &'static str,
    timeout: Duration,
) -> Result<VacuumDropMessage, SessionError> {
    match session.receive(timeout).await? {
        Some(msg) => Ok(msg),
        // A clean close mid-handshake is reported as a timeout on the message we
        // were waiting for, because the operationally important fact is that the
        // expected exchange never completed. Reporting `Closed` instead would
        // bury the cause under a generic "peer disconnected".
        None => Err(SessionError::DeadlineExceeded {
            operation: expected,
            limit: timeout,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: impl Into<String>) -> SessionConfig {
        SessionConfig::new(name)
    }

    fn stub_transport() -> Arc<dyn Transport> {
        struct Stub;
        impl Transport for Stub {
            fn peer_addr(&self) -> Option<std::net::SocketAddr> {
                None
            }
            fn send<'a>(
                &'a self,
                _: &'a [u8],
            ) -> vacuumdrop_transport::BoxFuture<'a, Result<usize, TransportError>> {
                Box::pin(async { Ok(0) })
            }
            fn receive<'a>(
                &'a self,
                _: &'a mut [u8],
            ) -> vacuumdrop_transport::BoxFuture<'a, Result<usize, TransportError>> {
                Box::pin(async { Ok(0) })
            }
            fn shutdown(&self) -> vacuumdrop_transport::BoxFuture<'_, Result<(), TransportError>> {
                Box::pin(async { Ok(()) })
            }
        }
        Arc::new(Stub)
    }

    fn session() -> Session {
        Session::new(SessionId::new(1), stub_transport(), Limits::new()).unwrap()
    }

    #[test]
    fn config_rejects_empty_name() {
        let mut c = config("");
        assert!(c.validate().is_err());
        c.device_name = "ok".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn config_rejects_control_characters_in_name() {
        let c = config("evil\u{1b}[31m");
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_rejects_overlong_name() {
        let c = config("x".repeat(1000));
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_rejects_oversized_chunk() {
        let mut c = config("dev");
        c.max_chunk_size = c.limits.max_frame_length + 1;
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_rejects_zero_chunk_size() {
        let mut c = config("dev");
        c.max_chunk_size = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn session_starts_connecting() {
        assert_eq!(session().state(), ConnectionState::Connecting);
    }

    #[test]
    fn state_advances_through_the_lifecycle() {
        let mut s = session();
        s.mark_connected().unwrap();
        assert_eq!(s.state(), ConnectionState::Connected);
        s.mark_authenticating().unwrap();
        assert_eq!(s.state(), ConnectionState::Authenticating);
    }

    #[test]
    fn cannot_establish_before_authenticating() {
        let mut s = session();
        s.mark_connected().unwrap();
        let params = SessionParameters {
            version: NegotiatedVersion(1),
            peer: PeerDescription::new("peer".into(), vec![1], vec![], 1024, &Limits::new())
                .unwrap(),
            local_capabilities: vec![],
            max_chunk_size: 1024,
        };
        assert!(s.mark_established(params).is_err());
    }

    #[test]
    fn request_ids_are_monotonic() {
        let mut s = session();
        let a = s.next_request_id();
        let b = s.next_request_id();
        let c = s.next_request_id();
        assert!(a < b && b < c, "request ids must increase");
    }

    #[test]
    fn establishing_records_parameters() {
        let mut s = session();
        s.mark_connected().unwrap();
        s.mark_authenticating().unwrap();
        let params = SessionParameters {
            version: NegotiatedVersion(1),
            peer: PeerDescription::new("peer".into(), vec![1], vec![], 1024, &Limits::new())
                .unwrap(),
            local_capabilities: vec![],
            max_chunk_size: 4096,
        };
        s.mark_established(params).unwrap();
        assert_eq!(s.peer().unwrap().display_name(), "peer");
        assert_eq!(s.parameters().unwrap().chunk_size(), 1024);
    }

    #[test]
    fn authentication_requires_the_authenticating_state() {
        let mut s = session();
        assert!(s.mark_authenticated().is_err());
        s.mark_connected().unwrap();
        s.mark_authenticating().unwrap();
        assert!(s.mark_authenticated().is_ok());
    }

    #[tokio::test]
    async fn ping_is_refused_before_established() {
        // A Ping is a liveness probe for a working session; sending one during
        // a handshake would let two peers livelock answering each other.
        let mut s = session();
        assert!(matches!(
            s.ping(1).await,
            Err(SessionError::InvalidState(_))
        ));
    }

    #[test]
    fn pong_echoes_the_nonce() {
        match Session::pong_for(42) {
            VacuumDropMessage::Pong(Pong { nonce }) => assert_eq!(nonce, 42),
            other => panic!("expected Pong, got {}", other.name()),
        }
    }

    #[tokio::test]
    async fn sending_after_close_is_refused() {
        let mut s = session();
        s.close(GoodbyeReason::Shutdown).await.unwrap();
        assert_eq!(s.state(), ConnectionState::Disconnected);
        assert!(
            s.send(VacuumDropMessage::Ping(Ping { nonce: 1 }))
                .await
                .is_err(),
            "a closed session accepted a send"
        );
    }

    #[tokio::test]
    async fn closing_twice_is_refused_not_panicking() {
        let mut s = session();
        s.close(GoodbyeReason::Shutdown).await.unwrap();
        assert!(matches!(
            s.close(GoodbyeReason::Shutdown).await,
            Err(SessionError::Closed)
        ));
    }

    #[test]
    fn session_errors_convert_to_core_errors() {
        let e: Error = SessionError::Closed.into();
        assert!(matches!(
            e,
            Error::PeerDisconnected(vacuumdrop_core::PeerDisconnectedError::Closed)
        ));

        let e2: Error = SessionError::Refused("server busy").into();
        assert!(matches!(e2, Error::Authentication(_)));
    }

    #[test]
    fn events_are_empty_until_a_peer_is_known() {
        let s = session();
        assert!(s.events().is_empty());
    }
}
