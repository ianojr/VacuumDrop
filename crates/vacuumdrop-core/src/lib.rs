//! VacuumDrop core domain.
//!
//! Charter 1: 90% of the logic lives in a portable Rust core, and platform
//! code stays thin. This crate is the bottom of that stack. It depends on no
//! async runtime, no wire format, no filesystem, and no platform API, which is
//! what makes that guarantee structural rather than aspirational.
//!
//! What lives here:
//!
//! | Module | Charter |
//! | --- | --- |
//! | [`error`] | 39, structured errors with distinct variants |
//! | [`limits`] | 35, 36, resource limits enforced before allocation |
//! | [`state`] | 40, 87, explicit connection state machine |
//! | [`domain`] | 16, 23, 25, 40, 49, 66, 67, 69, identity and events |
//!
//! What must never appear here: a socket, a file handle, a platform type, or a
//! message definition. If any of those are needed, the logic belongs elsewhere.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

pub mod domain;
pub mod error;
pub mod limits;
pub mod state;

pub use domain::{
    Capability, Event, NegotiatedVersion, PeerAddress, PeerDescription, PeerId, SessionId,
    TransferId, TransferProgress, TransferState,
};
pub use error::{
    AuthenticationError, AuthorizationError, Error, IntegrityError, InvalidMessageError,
    PeerDisconnectedError, ProtocolError, Result, StorageError, TimeoutError, TransportError,
    UnsupportedVersionError, Violation,
};
pub use limits::Limits;
pub use state::{ConnectionState, StateMachine};

/// The crate version, reported in `Hello` so mismatched builds are visible.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The crate name, sent during handshake for diagnostics.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_reported() {
        assert!(!super::VERSION.is_empty());
        assert_eq!(super::NAME, "vacuumdrop-core");
    }
}
