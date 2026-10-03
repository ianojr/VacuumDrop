//! VacuumDrop wire protocol.
//!
//! Charter 11: TCP is a byte stream, so framing is mandatory. This crate owns
//! framing, encoding, validation, and negotiation, and nothing else. It has no
//! sockets, no files, and no platform types, which is what lets the same
//! protocol run over TCP, a Wi-Fi Direct link, or a test harness unchanged
//! (ADR-0004).
//!
//! | Module | Charter |
//! | --- | --- |
//! | [`header`] | 10, fixed frame prefix with version, type, length, ids |
//! | [`codec`] | 11, byte-stream framing |
//! | [`message`] | 9.2, the message catalogue |
//! | [`handshake`] | 40, 66, version negotiation that fails closed |
//!
//! # Phase 1 scope
//!
//! Handshake, capability negotiation, keepalive, error, goodbye. Transfer
//! messages are deliberately absent rather than stubbed: a stub invites use
//! before it works. See [`docs/roadmap.md`](../docs/roadmap.md) section 9.
//!
//! # Security posture
//!
//! Every value arriving from the wire is hostile. Two properties are load-bearing:
//!
//! 1. **Declared lengths are validated before allocation**
//!    ([`docs/threat-model.md`](../docs/threat-model.md) T-01).
//! 2. **No unbounded input reaches the domain.** Strings are length-bounded and
//!    control-character-checked; lists are length-bounded; every enum is closed.
//!
//! What is *not* here yet: encryption and authentication are Phase 4
//! ([ADR-0010](../docs/adr/0010-handshake-protocol.md)). Phase 1 sessions are
//! plaintext and unauthenticated, and the CLI says so.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

pub mod codec;
pub mod handshake;
pub mod header;
pub mod message;
pub mod session;

pub use codec::FrameCodec;
pub use handshake::{ClientHandshake, HandshakeError, ServerHandshake, SessionParameters};
pub use header::{FrameHeader, HEADER_LEN, MAGIC};
pub use message::{
    CapabilitySet, ErrorCode, Goodbye, GoodbyeReason, Hello, HelloAck, HelloRejection, HelloStatus,
    Ping, Pong, ProtocolErrorReport, SupportedVersions, VacuumDropMessage, VersionSelection,
};

/// The protocol version this build speaks.
pub const VERSION: u16 = vacuumdrop_core::NegotiatedVersion::LOCAL;

/// The crate version, reported in `Hello` for diagnostics.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one() {
        assert_eq!(VERSION, 1);
    }

    #[test]
    fn crate_version_is_reported() {
        assert!(!CRATE_VERSION.is_empty());
    }
}
