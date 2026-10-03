//! Resource limits applied to every protocol endpoint.
//!
//! Charter 35 requires limits on every endpoint; Charter 36 requires that a
//! remote peer cannot force unbounded consumption of memory, disk, CPU, tasks,
//! or connections. This module is the single source of those limits.
//!
//! Every limit is configurable, and every limit is checked **before** the
//! allocation or work it bounds. A limit that is only checked afterwards is not
//! a limit.
//!
//! # Why these numbers
//!
//! They are starting points chosen to be safe on a constrained device, not
//! optimised values. Raise them deliberately, with a reason; never disable one
//! to make a transfer work (`docs/engineering-standards.md` rule 10).

use crate::error::Violation;

/// Default resource limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Maximum bytes in one frame's payload.
    ///
    /// Enforced by the codec against the length header *before* the buffer is
    /// sized. This is the primary defence against a length-prefix memory
    /// exhaustion attack (`docs/threat-model.md` T-01).
    pub max_frame_length: u32,

    /// Maximum bytes in a metadata payload such as `FileMetadata`.
    pub max_metadata_length: u32,

    /// Maximum bytes in a device or display name.
    pub max_name_length: usize,

    /// Maximum concurrently established sessions.
    pub max_sessions: u32,

    /// Maximum concurrently active transfers.
    pub max_transfers: u32,

    /// Maximum chunks in flight before the sender must wait for an ack.
    ///
    /// Bounded by design: this is the send window, and an unbounded one would
    /// reintroduce unbounded buffering under the guise of throughput
    /// (`docs/file-transfer.md` section 10).
    pub max_in_flight_chunks: u32,

    /// Maximum chunks queued between pipeline stages.
    pub max_queued_chunks: u32,

    /// Failed authentication attempts before a cooldown begins.
    pub max_auth_attempts: u32,

    /// How long a cooldown lasts after too many failed attempts.
    pub auth_cooldown: std::time::Duration,

    /// Maximum new connections accepted per minute.
    pub max_connections_per_minute: u32,

    /// Maximum inbound protocol messages processed per second per session.
    pub max_messages_per_second: u32,

    /// Deadline for establishing a connection.
    pub connect_timeout: std::time::Duration,

    /// Deadline for the handshake and version negotiation.
    pub handshake_timeout: std::time::Duration,

    /// Deadline for authentication.
    pub auth_timeout: std::time::Duration,

    /// Deadline for a metadata exchange.
    pub metadata_timeout: std::time::Duration,

    /// How long a session may sit idle before it is torn down.
    pub idle_timeout: std::time::Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 32 MiB. Comfortably above the 16 MiB maximum chunk size in
            // ADR-0006 so a full chunk plus its header always fits.
            max_frame_length: 32 * 1024 * 1024,

            // 64 KiB. Metadata is a filename and a handful of numbers; anything
            // larger is either a bug or an attempt to waste our memory.
            max_metadata_length: 64 * 1024,

            // 255 bytes, matching the common filesystem name limit.
            max_name_length: 255,

            max_sessions: 8,
            max_transfers: 4,
            max_in_flight_chunks: 8,
            max_queued_chunks: 64,
            max_auth_attempts: 5,
            auth_cooldown: std::time::Duration::from_secs(30),
            max_connections_per_minute: 30,
            max_messages_per_second: 512,

            connect_timeout: std::time::Duration::from_secs(10),
            handshake_timeout: std::time::Duration::from_secs(10),
            auth_timeout: std::time::Duration::from_secs(30),
            metadata_timeout: std::time::Duration::from_secs(30),
            idle_timeout: std::time::Duration::from_secs(120),
        }
    }
}

impl Limits {
    /// Returns the default limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rejects a configuration that could not be enforced safely.
    ///
    /// Called once at startup so an impossible configuration fails immediately
    /// rather than at the moment it first matters, mid-transfer.
    pub fn validate(&self) -> Result<(), Violation> {
        if self.max_frame_length == 0 {
            return Err(Violation::new("max_frame_length", "must be non-zero"));
        }
        if self.max_metadata_length == 0 {
            return Err(Violation::new("max_metadata_length", "must be non-zero"));
        }
        if self.max_name_length == 0 {
            return Err(Violation::new("max_name_length", "must be non-zero"));
        }
        if self.max_sessions == 0 {
            return Err(Violation::new("max_sessions", "must be non-zero"));
        }
        if self.max_transfers == 0 {
            return Err(Violation::new("max_transfers", "must be non-zero"));
        }
        if self.max_in_flight_chunks == 0 {
            return Err(Violation::new(
                "max_in_flight_chunks",
                "must be non-zero; zero would stall every transfer",
            ));
        }
        if self.max_metadata_length > self.max_frame_length {
            return Err(Violation::new(
                "max_metadata_length",
                "must not exceed max_frame_length",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(Limits::new().validate(), Ok(()));
    }

    #[test]
    fn metadata_limit_cannot_exceed_frame_limit() {
        let mut l = Limits::new();
        l.max_metadata_length = l.max_frame_length + 1;
        assert!(l.validate().is_err());
    }

    #[test]
    fn zero_limits_are_rejected() {
        // Each of these would silently disable a defence if accepted.
        for f in [
            |l: &mut Limits| l.max_frame_length = 0,
            |l: &mut Limits| l.max_metadata_length = 0,
            |l: &mut Limits| l.max_name_length = 0,
            |l: &mut Limits| l.max_sessions = 0,
            |l: &mut Limits| l.max_transfers = 0,
            |l: &mut Limits| l.max_in_flight_chunks = 0,
        ] {
            let mut l = Limits::new();
            f(&mut l);
            assert!(l.validate().is_err(), "a zero limit was accepted");
        }
    }

    #[test]
    fn frame_limit_admits_the_largest_chunk_from_adr_0006() {
        // ADR-0006 lists 32 MiB as the largest candidate chunk size. If the frame
        // limit ever dropped below that, a maximum-size chunk could not be
        // framed at all.
        let l = Limits::new();
        assert!(l.max_frame_length >= 32 * 1024 * 1024);
    }

    #[test]
    fn every_timeout_is_non_zero() {
        let l = Limits::new();
        assert!(l.connect_timeout > std::time::Duration::ZERO);
        assert!(l.handshake_timeout > std::time::Duration::ZERO);
        assert!(l.auth_timeout > std::time::Duration::ZERO);
        assert!(l.metadata_timeout > std::time::Duration::ZERO);
        assert!(l.idle_timeout > std::time::Duration::ZERO);
    }
}
