//! Structured error taxonomy for the VacuumDrop engine.
//!
//! Charter 39 requires errors that distinguish their origin. Every fallible
//! operation in the engine funnels into [`Error`], and each variant carries
//! structured detail rather than a formatted string, so callers can branch on
//! *what went wrong* without parsing prose.
//!
//! This module is the single place errors are defined. Protocol and transport
//! crates add their own variants; they never reuse a variant that means
//! something else.
//!
//! # Why `Error` and not `Box<dyn Error>`
//!
//! A benchmark crate using `Box<dyn Error>` is a documented Phase-0 exception
//! ([`docs/phase1-benchmark.md`](../docs/phase1-benchmark.md) section 3.3). It
//! does not extend past Phase 0: a typed taxonomy is what makes error handling
//! matchable and testable, which is the whole point of the requirement.

use std::fmt;

/// Result alias for engine operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type for the engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying byte transport failed, or a transport invariant broke.
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// The peer sent something that violates the protocol.
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Peer authentication failed or was never attempted where required.
    #[error("authentication error: {0}")]
    Authentication(#[from] AuthenticationError),

    /// The peer is authenticated but not permitted to perform the operation.
    #[error("authorization error: {0}")]
    Authorization(#[from] AuthorizationError),

    /// Data failed its integrity check: bad length, bad hash, bad chunk index.
    #[error("integrity error: {0}")]
    Integrity(#[from] IntegrityError),

    /// Filesystem or persistence failed.
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    /// An operation exceeded its deadline.
    #[error("timeout: {0}")]
    Timeout(#[from] TimeoutError),

    /// The peer went away mid-operation.
    #[error("peer disconnected: {0}")]
    PeerDisconnected(#[from] PeerDisconnectedError),

    /// No mutually supported protocol version exists.
    #[error("unsupported protocol version: {0}")]
    UnsupportedVersion(#[from] UnsupportedVersionError),

    /// A message was structurally invalid: bad framing, bad length, bad field.
    #[error("invalid message: {0}")]
    InvalidMessage(#[from] InvalidMessageError),
}

/// Failures originating in the transport layer.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// Socket-level failure (connect, accept, read, write).
    #[error("i/o failed: {0}")]
    Io(#[source] std::io::Error),

    /// The transport was shut down while the operation was in flight.
    #[error("transport is closed")]
    Closed,

    /// A configured deadline elapsed before the operation completed.
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(std::time::Duration),

    /// The local address space or interface is unavailable.
    #[error("no usable network interface: {0}")]
    NoInterface(String),

    /// The adapter exists but the platform cannot currently provide the link.
    #[error("transport unavailable: {0}")]
    Unavailable(String),
}

/// Failures where a message violates the protocol's grammar.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// A message arrived in a state that does not allow it.
    ///
    /// This is a protocol violation by the peer, not a local bug: the local
    /// state machine refused a message the peer should not have sent.
    #[error("message {message} is not valid in state {state}")]
    UnexpectedMessage {
        /// The message that was rejected.
        message: &'static str,
        /// The state that rejected it.
        state: &'static str,
    },

    /// A handshake or lifecycle step was violated.
    #[error("protocol state violation: {0}")]
    StateViolation(&'static str),

    /// The peer sent a frame whose declared length is inconsistent with its
    /// contents, or exceeds the configured limit.
    ///
    /// Always detected **before** any allocation proportional to the declared
    /// length. See `docs/threat-model.md` T-01.
    #[error("declared payload length {declared} is invalid (limit {limit})")]
    PayloadTooLarge {
        /// Length the peer declared.
        declared: u64,
        /// Configured maximum.
        limit: u64,
    },

    /// A required field was absent, empty, or structurally wrong.
    #[error("invalid field `{field}`: {reason}")]
    InvalidField {
        /// Field name.
        field: &'static str,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// The peer sent a frame whose magic bytes are wrong.
    ///
    /// Reported separately from other framing errors so a port-scan or a
    /// non-VacuumDrop client is trivially distinguishable from an attack.
    #[error("bad frame magic")]
    BadMagic,

    /// Reserved header flags were set, implying a newer protocol we cannot
    /// correctly interpret.
    #[error("unknown header flags set: {0:#06x}")]
    UnknownFlags(u16),

    /// The frame declared a message type this build does not know.
    ///
    /// Refusing is deliberate. Decoding the payload regardless would mean
    /// guessing at what a framing convention we do not implement intended.
    #[error("unknown message type {0:#06x}")]
    UnknownMessageType(u16),

    /// A capability the peer requires is not supported here.
    #[error("required capability not supported: {0}")]
    MissingCapability(&'static str),

    /// The connection ended cleanly at the peer's request.
    #[error("peer sent goodbye: {0}")]
    Goodbye(&'static str),
}

/// Failures establishing or verifying peer identity.
#[derive(Debug, thiserror::Error)]
pub enum AuthenticationError {
    /// The peer did not authenticate when the operation required it.
    #[error("peer is not authenticated")]
    NotAuthenticated,

    /// Credentials were presented and rejected.
    #[error("authentication rejected: {0}")]
    Rejected(&'static str),

    /// The handshake itself failed or was malformed.
    #[error("handshake failed: {0}")]
    Handshake(&'static str),

    /// The peer exhausted its authentication attempts.
    ///
    /// Cooldown is time-based and bounded (see `docs/security.md` section 7); it
    /// is never a permanent lockout.
    #[error("rate limited after {attempts} failed attempts")]
    RateLimited {
        /// Failures observed before the cooldown began.
        attempts: u32,
    },

    /// No trusted identity exists for this peer and pairing was required.
    #[error("peer is untrusted; explicit pairing required")]
    Untrusted,
}

/// Failures where an authenticated peer lacks permission.
#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    /// The peer holds a valid identity but lacks the required permission.
    #[error("peer lacks permission `{permission}`")]
    MissingPermission {
        /// The permission that was required.
        permission: &'static str,
    },

    /// The operation is not permitted at all in the current session state.
    #[error("operation `{operation}` is not permitted for this peer")]
    NotPermitted {
        /// The refused operation.
        operation: &'static str,
    },
}

/// Failures where received data did not match what was declared or expected.
#[derive(Debug, thiserror::Error)]
pub enum IntegrityError {
    /// A chunk arrived with an index outside the declared range.
    #[error("chunk index {index} out of range (total {total})")]
    ChunkOutOfRange {
        /// Offending index.
        index: u64,
        /// Declared total chunk count.
        total: u64,
    },

    /// A chunk's payload length disagreed with the negotiated chunk size.
    #[error("chunk {index} length {actual} != negotiated {expected}")]
    ChunkLengthMismatch {
        /// Offending chunk index.
        index: u64,
        /// Bytes received.
        actual: u64,
        /// Bytes expected.
        expected: u64,
    },

    /// A hash did not match the value that was expected.
    #[error("integrity check failed at {scope}")]
    HashMismatch {
        /// What was being verified, e.g. `"chunk 42"` or `"whole file"`.
        scope: &'static str,
    },

    /// Declared metadata is internally inconsistent, e.g. `total_chunks` does
    /// not agree with `file_size / chunk_size`.
    #[error("inconsistent metadata: {0}")]
    InconsistentMetadata(&'static str),

    /// The received byte count did not match the declared size.
    #[error("length mismatch: expected {expected} bytes, got {actual}")]
    LengthMismatch {
        /// Declared size.
        expected: u64,
        /// Bytes actually handled.
        actual: u64,
    },
}

/// Failures of the local filesystem or persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Filesystem call failed.
    #[error("filesystem i/o failed: {0}")]
    Io(#[source] std::io::Error),

    /// The destination has less free space than the transfer requires.
    #[error("insufficient space: need {needed} bytes, have {available}")]
    InsufficientSpace {
        /// Bytes required.
        needed: u64,
        /// Bytes available.
        available: u64,
    },

    /// The destination path was rejected as unsafe.
    ///
    /// See `docs/threat-model.md` T-03: filenames arrive from an untrusted peer
    /// and are never used as a path without sanitisation.
    #[error("unsafe destination path: {0}")]
    UnsafePath(String),

    /// A file that must not already exist does.
    #[error("refusing to overwrite existing file: {0}")]
    AlreadyExists(String),

    /// Transfer state could not be persisted or recovered.
    #[error("persistence failure: {0}")]
    Persistence(&'static str),
}

/// An operation exceeded its deadline.
#[derive(Debug, thiserror::Error)]
#[error("operation `{operation}` exceeded {limit:?}")]
pub struct TimeoutError {
    /// What was being waited on.
    pub operation: &'static str,
    /// The configured limit that was exceeded.
    pub limit: std::time::Duration,
}

impl TimeoutError {
    /// Builds a timeout error.
    pub fn new(operation: &'static str, limit: std::time::Duration) -> Self {
        Self { operation, limit }
    }
}

/// The peer closed the connection or became unreachable.
#[derive(Debug, thiserror::Error)]
pub enum PeerDisconnectedError {
    /// The transport reported end-of-stream.
    #[error("connection closed by peer")]
    Closed,

    /// The peer vanished without a clean shutdown.
    #[error("connection lost: {0}")]
    Lost(String),

    /// The session exceeded its idle window.
    #[error("session idle for {0:?}")]
    Idle(std::time::Duration),
}

/// No protocol version is acceptable to both peers.
#[derive(Debug, thiserror::Error)]
#[error("no common protocol version (local: {local:?}, remote: {remote:?})")]
pub struct UnsupportedVersionError {
    /// Versions this build accepts.
    pub local: Vec<u16>,
    /// Versions the peer advertised.
    pub remote: Vec<u16>,
}

/// A frame or field failed structural validation before interpretation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidMessageError {
    /// The frame was too short to contain its own header.
    #[error("truncated frame: need {need} bytes, have {have}")]
    Truncated {
        /// Bytes required.
        need: usize,
        /// Bytes available.
        have: usize,
    },

    /// A string field exceeded its configured bound.
    #[error("field `{field}` exceeds {actual} bytes (limit {limit})")]
    FieldTooLong {
        /// Field name.
        field: &'static str,
        /// Length received.
        actual: usize,
        /// Configured bound.
        limit: usize,
    },

    /// A string field was not valid UTF-8.
    #[error("field `{field}` is not valid UTF-8")]
    NotUtf8 {
        /// Field name.
        field: &'static str,
    },

    /// A numeric field fell outside its permitted range.
    #[error("field `{field}` value {value} outside {min}..={max}")]
    OutOfRange {
        /// Field name.
        field: &'static str,
        /// Value received.
        value: u64,
        /// Inclusive lower bound.
        min: u64,
        /// Inclusive upper bound.
        max: u64,
    },

    /// The codec could not decode the payload.
    #[error("payload could not be decoded: {0}")]
    Decode(String),

    /// The message type discriminant is not known to this build.
    #[error("unknown message type {0:#06x}")]
    UnknownType(u16),
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<std::io::Error> for Error {
    /// A bare I/O error is a transport failure unless it came from a filesystem
    /// call, in which case the caller should have used [`StorageError`]
    /// directly. Mapping here keeps `?` ergonomic in socket-facing code, which
    /// is where this conversion will actually be used.
    fn from(e: std::io::Error) -> Self {
        Error::Transport(TransportError::Io(e))
    }
}

/// A validation failure with enough context to be logged usefully.
///
/// Used by the protocol crate's validators so rejections carry the field and
/// reason rather than a bare message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Field or check that failed.
    pub field: &'static str,
    /// Why it failed.
    pub reason: &'static str,
}

impl Violation {
    /// Builds a violation.
    pub const fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for Violation {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_errors_carry_their_context() {
        let e = Error::Protocol(ProtocolError::PayloadTooLarge {
            declared: 4_000_000_000,
            limit: 33_554_432,
        });
        // The declared length must appear: it is the evidence needed to decide
        // whether this was a bug, a probe, or an attack.
        assert!(e.to_string().contains("4000000000"));
        assert!(e.to_string().contains("33554432"));
    }

    #[test]
    fn timeout_error_names_the_operation() {
        let e = Error::Timeout(TimeoutError::new(
            "connect",
            std::time::Duration::from_secs(5),
        ));
        assert!(e.to_string().contains("connect"));
        assert!(e.to_string().contains("5s"));
    }

    #[test]
    fn unsupported_version_reports_both_sides() {
        let e = Error::UnsupportedVersion(UnsupportedVersionError {
            local: vec![1, 2],
            remote: vec![7],
        });
        let s = e.to_string();
        assert!(s.contains("1"));
        assert!(s.contains("7"));
    }

    #[test]
    fn io_errors_convert_into_transport_and_storage() {
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone");
        let t: Error = TransportError::from(io).into();
        assert!(matches!(t, Error::Transport(TransportError::Io(_))));

        let io2 = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let s: Error = StorageError::from(io2).into();
        assert!(matches!(s, Error::Storage(StorageError::Io(_))));
    }

    #[test]
    fn top_level_discriminants_are_distinct() {
        // Guards against a copy-paste error collapsing two variants.
        let variants = [
            Error::Protocol(ProtocolError::BadMagic),
            Error::Authentication(AuthenticationError::Untrusted),
            Error::Authorization(AuthorizationError::MissingPermission { permission: "SEND" }),
            Error::Integrity(IntegrityError::HashMismatch { scope: "chunk 1" }),
            Error::Storage(StorageError::Persistence("x")),
            Error::PeerDisconnected(PeerDisconnectedError::Closed),
            Error::Transport(TransportError::Closed),
        ];
        for (i, a) in variants.iter().enumerate() {
            for b in variants.iter().skip(i + 1) {
                assert_ne!(
                    std::mem::discriminant(a),
                    std::mem::discriminant(b),
                    "two error variants share a discriminant"
                );
            }
        }
    }

    #[test]
    fn violation_renders_field_and_reason() {
        let v = Violation::new("file_name", "contains a path separator");
        assert_eq!(v.to_string(), "file_name: contains a path separator");
    }
}
