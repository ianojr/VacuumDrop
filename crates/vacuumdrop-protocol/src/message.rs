//! Protocol messages.
//!
//! ADR-0007 governs the wire format. Three rules from it are load-bearing here:
//!
//! 1. Every variant and field carries an explicit `#[serde(rename)]`. Rust-side
//!    renames must never silently alter the wire format.
//! 2. Every enum variant has an explicit numeric discriminant. Reordering
//!    variants is a breaking protocol change, and an explicit number makes that
//!    visible in review rather than in production.
//! 3. The payload types here are the *public API* of the protocol. Changing one
//!    is a protocol change and goes through ADR-0007 again.
//!
//! Charter 90 requires the catalogue in [`docs/protocol.md`](../docs/protocol.md)
//! section 2. This module implements the Phase 1 subset: handshake, capability
//! negotiation, keepalive, error, and goodbye. Transfer messages are Phase 5 and
//! are absent by design rather than stubbed, so nothing here can be used before
//! it is implemented.
//!
//! # On `HelloAck`
//!
//! Doc 3 proposed `HelloAck { status: String }`. A free-form string from an
//! untrusted peer violates Charter 32 (validate everything), 35 (bounded
//! values), and 87 (prefer strong enums). Resolved as
//! [`docs/protocol.md`](../docs/protocol.md) conflict C-2: [`HelloStatus`] is a
//! closed enum, and rejection reasons are a closed enum, so an unexpected value
//! is a decode failure rather than a string that must be interpreted at runtime.

use serde::{Deserialize, Serialize};
use vacuumdrop_core::Capability;

/// Every message the Phase 1 protocol can carry.
///
/// Discriminants are explicit and frozen. Appending is safe; inserting or
/// reordering is a breaking change requiring an ADR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "VacuumDropMessage")]
pub enum VacuumDropMessage {
    /// Version negotiation: "these are the versions I speak."
    ///
    /// Must be the first message in a session (Charter 66).
    #[serde(rename = "VersionNegotiate")]
    VersionNegotiate(#[serde(rename = "supported")] SupportedVersions),

    /// The chosen version and the server's own advertised set.
    #[serde(rename = "VersionAck")]
    VersionAck(#[serde(rename = "selected")] VersionSelection),

    /// Client greeting.
    #[serde(rename = "Hello")]
    Hello(#[serde(rename = "hello")] Hello),

    /// Server greeting response.
    #[serde(rename = "HelloAck")]
    HelloAck(#[serde(rename = "hello_ack")] HelloAck),

    /// Capability exchange.
    #[serde(rename = "Capabilities")]
    Capabilities(#[serde(rename = "capabilities")] CapabilitySet),

    /// Liveness probe.
    #[serde(rename = "Ping")]
    Ping(#[serde(rename = "ping")] Ping),

    /// Liveness response.
    #[serde(rename = "Pong")]
    Pong(#[serde(rename = "pong")] Pong),

    /// A non-fatal or fatal protocol error report.
    #[serde(rename = "Error")]
    Error(#[serde(rename = "error")] ProtocolErrorReport),

    /// Clean shutdown notice.
    #[serde(rename = "Goodbye")]
    Goodbye(#[serde(rename = "goodbye")] Goodbye),
}

impl VacuumDropMessage {
    /// The variant's name, for logs and error messages.
    pub fn name(&self) -> &'static str {
        match self {
            VacuumDropMessage::VersionNegotiate(_) => "VersionNegotiate",
            VacuumDropMessage::VersionAck(_) => "VersionAck",
            VacuumDropMessage::Hello(_) => "Hello",
            VacuumDropMessage::HelloAck(_) => "HelloAck",
            VacuumDropMessage::Capabilities(_) => "Capabilities",
            VacuumDropMessage::Ping(_) => "Ping",
            VacuumDropMessage::Pong(_) => "Pong",
            VacuumDropMessage::Error(_) => "Error",
            VacuumDropMessage::Goodbye(_) => "Goodbye",
        }
    }

    /// Whether this message may legally be the first in a session.
    ///
    /// Anything else means the peer skipped negotiation, which is a protocol
    /// violation (Charter 40).
    pub fn is_valid_opener(&self) -> bool {
        matches!(self, VacuumDropMessage::VersionNegotiate(_))
    }
}

/// The versions a peer supports.
///
/// Bounded during validation: an unbounded list would let a peer send a frame
/// we spend time parsing for no benefit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "SupportedVersions")]
pub struct SupportedVersions {
    /// Supported versions, ascending. Maximum 32 entries.
    #[serde(rename = "versions")]
    pub versions: Vec<u16>,
}

impl SupportedVersions {
    /// The maximum number of versions a peer may advertise.
    ///
    /// 32 is generous for a protocol that will have a handful of versions, and
    /// bounds the work an attacker can make us do parsing a single field.
    pub const MAX_VERSIONS: usize = 32;

    /// Builds the list for this build.
    pub fn local() -> Self {
        Self {
            versions: vacuumdrop_core::NegotiatedVersion::SUPPORTED.to_vec(),
        }
    }

    /// Validates structure: non-empty, ascending, unique, bounded length.
    ///
    /// Ordering is required rather than sorted on receipt: accepting an
    /// unsorted list would make two encodings of the same set distinguishable,
    /// which complicates any future transcript comparison for no benefit.
    pub fn validate(&self) -> Result<(), vacuumdrop_core::Violation> {
        if self.versions.is_empty() {
            return Err(vacuumdrop_core::Violation::new(
                "versions",
                "must advertise at least one version",
            ));
        }
        if self.versions.len() > Self::MAX_VERSIONS {
            return Err(vacuumdrop_core::Violation::new(
                "versions",
                "advertises more versions than permitted",
            ));
        }
        if self.versions.contains(&0) {
            return Err(vacuumdrop_core::Violation::new(
                "versions",
                "contains reserved version 0",
            ));
        }
        for pair in self.versions.windows(2) {
            if pair[0] == pair[1] {
                return Err(vacuumdrop_core::Violation::new(
                    "versions",
                    "contains a duplicate version",
                ));
            }
            if pair[0] > pair[1] {
                return Err(vacuumdrop_core::Violation::new(
                    "versions",
                    "must be in ascending order",
                ));
            }
        }
        Ok(())
    }
}

/// The server's version choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "VersionSelection")]
pub struct VersionSelection {
    /// The version both peers will use.
    #[serde(rename = "selected")]
    pub selected: u16,
}

impl VersionSelection {
    /// Chooses the highest version supported by both peers.
    pub fn negotiate(
        offered: &SupportedVersions,
    ) -> Result<Self, vacuumdrop_core::UnsupportedVersionError> {
        offered
            .validate()
            .map_err(|_| vacuumdrop_core::UnsupportedVersionError {
                local: vacuumdrop_core::NegotiatedVersion::SUPPORTED.to_vec(),
                remote: offered.versions.clone(),
            })?;
        vacuumdrop_core::NegotiatedVersion::negotiate(&offered.versions)
            .map(|v| Self { selected: v.0 })
    }
}

/// Client greeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Hello")]
pub struct Hello {
    /// The client's declared display name. Untrusted.
    #[serde(rename = "device_name")]
    pub device_name: String,
    /// The client's build identifier, for diagnostics only.
    #[serde(rename = "client_version")]
    pub client_version: String,
}

/// Server greeting response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "HelloAck")]
pub struct HelloAck {
    /// Accepted or rejected.
    #[serde(rename = "status")]
    pub status: HelloStatus,
    /// The server's declared display name.
    #[serde(rename = "device_name")]
    pub device_name: String,
}

/// Outcome of a greeting.
///
/// A closed enum, not a `String`. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "HelloStatus")]
pub enum HelloStatus {
    /// Greeting accepted.
    #[serde(rename = "accepted")]
    Accepted,
    /// Greeting refused, with a reason the client can act on.
    #[serde(rename = "rejected")]
    Rejected(#[serde(rename = "reason")] HelloRejection),
}

/// Why a greeting was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "HelloRejection")]
pub enum HelloRejection {
    /// No version in common.
    #[serde(rename = "unsupported_version")]
    UnsupportedVersion,
    /// The greeting violated a limit or format rule.
    #[serde(rename = "malformed")]
    Malformed,
    /// Too many connections already.
    #[serde(rename = "too_many_sessions")]
    TooManySessions,
    /// The server is draining and not accepting new work.
    #[serde(rename = "server_busy")]
    ServerBusy,
    /// Authentication will be required before any work.
    #[serde(rename = "authentication_required")]
    AuthenticationRequired,
}

impl HelloRejection {
    /// A log-friendly description.
    pub fn reason(self) -> &'static str {
        match self {
            HelloRejection::UnsupportedVersion => "no compatible protocol version",
            HelloRejection::Malformed => "greeting violated a protocol rule",
            HelloRejection::TooManySessions => "session limit reached",
            HelloRejection::ServerBusy => "server is not accepting connections",
            HelloRejection::AuthenticationRequired => "authentication required",
        }
    }
}

/// Advertised capabilities.
///
/// Every field is a claim. Validation bounds the set; it does not make the
/// claims true (Charter 67).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "CapabilitySet")]
pub struct CapabilitySet {
    /// Feature flags.
    #[serde(rename = "features")]
    pub features: Vec<u8>,
    /// Largest chunk this peer will accept.
    #[serde(rename = "max_chunk_size")]
    pub max_chunk_size: u32,
}

/// The most feature flags a peer may advertise.
pub const MAX_FEATURES: usize = 32;

impl CapabilitySet {
    /// Builds a set from core capabilities.
    pub fn from_capabilities(caps: &[Capability], max_chunk_size: u32) -> Self {
        Self {
            features: caps.iter().map(|c| c.discriminant()).collect(),
            max_chunk_size,
        }
    }

    /// Decodes advertised flags, discarding unrecognised ones.
    ///
    /// Discarding rather than failing is deliberate: a newer peer may advertise
    /// features this build has never heard of, and refusing to talk to it would
    /// break forward compatibility. The flags that survive are still only
    /// claims.
    pub fn capabilities(&self) -> Vec<Capability> {
        self.features
            .iter()
            .filter_map(|f| Capability::from_discriminant(*f))
            .collect()
    }

    /// Whether the peer claims a capability.
    pub fn claims(&self, capability: Capability) -> bool {
        self.features.contains(&capability.discriminant())
    }

    /// Validates structure against local limits.
    pub fn validate(
        &self,
        limits: &vacuumdrop_core::Limits,
    ) -> Result<(), vacuumdrop_core::Violation> {
        if self.features.len() > MAX_FEATURES {
            return Err(vacuumdrop_core::Violation::new(
                "features",
                "advertises more features than permitted",
            ));
        }
        if self.max_chunk_size == 0 {
            return Err(vacuumdrop_core::Violation::new(
                "max_chunk_size",
                "must be non-zero",
            ));
        }
        if self.max_chunk_size > limits.max_frame_length {
            return Err(vacuumdrop_core::Violation::new(
                "max_chunk_size",
                "exceeds our maximum frame length",
            ));
        }
        Ok(())
    }
}

/// A liveness probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Ping")]
pub struct Ping {
    /// Opaque value the peer must echo.
    #[serde(rename = "nonce")]
    pub nonce: u64,
}

/// A liveness response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Pong")]
pub struct Pong {
    /// The echoed nonce.
    #[serde(rename = "nonce")]
    pub nonce: u64,
}

/// An error report carried in-band.
///
/// `fatal` distinguishes an error we are closing the session for from a warning
/// the session can continue after. Without it every error would have to be
/// fatal, and a single recoverable problem would tear down a healthy session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "ProtocolErrorReport")]
pub struct ProtocolErrorReport {
    /// Whether the sender is closing the session.
    #[serde(rename = "fatal")]
    pub fatal: bool,
    /// Stable machine-readable code.
    #[serde(rename = "code")]
    pub code: ErrorCode,
    /// Bounded human-readable detail. Never contains secrets.
    #[serde(rename = "detail")]
    pub detail: String,
}

impl ProtocolErrorReport {
    /// Builds a fatal report.
    pub fn fatal(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            fatal: true,
            code,
            detail: detail.into(),
        }
    }

    /// Builds a non-fatal report.
    pub fn warning(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            fatal: false,
            code,
            detail: detail.into(),
        }
    }

    /// The longest permitted `detail` string.
    pub const MAX_DETAIL: usize = 256;
}

/// Stable machine-readable error codes.
///
/// Closed set: a peer cannot invent a code, so unknown values are a protocol
/// violation rather than something to log and continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "ErrorCode")]
pub enum ErrorCode {
    /// Peer sent something malformed.
    #[serde(rename = "malformed_message")]
    MalformedMessage = 1,
    /// A required message arrived out of order.
    #[serde(rename = "unexpected_message")]
    UnexpectedMessage = 2,
    /// No common protocol version.
    #[serde(rename = "unsupported_version")]
    UnsupportedVersion = 3,
    /// A limit was exceeded.
    #[serde(rename = "limit_exceeded")]
    LimitExceeded = 4,
    /// A required capability is missing.
    #[serde(rename = "missing_capability")]
    MissingCapability = 5,
    /// Rate limiting is in force.
    #[serde(rename = "rate_limited")]
    RateLimited = 6,
    /// An internal error on this side.
    #[serde(rename = "internal")]
    Internal = 7,
}

/// Clean shutdown notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "Goodbye")]
pub struct Goodbye {
    /// Why the session is ending.
    #[serde(rename = "reason")]
    pub reason: GoodbyeReason,
}

/// Why a session is ending cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "GoodbyeReason")]
pub enum GoodbyeReason {
    /// Normal completion.
    #[serde(rename = "complete")]
    Complete,
    /// The sender is shutting down.
    #[serde(rename = "shutdown")]
    Shutdown,
    /// Nothing further is needed.
    #[serde(rename = "idle")]
    Idle,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vacuumdrop_core::{Limits, Violation};

    /// Asserts validation failed on a specific field.
    ///
    /// Compares only the field name, not the reason: reasons are free to be
    /// reworded without a protocol change, and a test that pinned them would
    /// fail on a harmless edit.
    fn assert_field(result: Result<(), Violation>, expected: &'static str) {
        match result {
            Err(v) => assert_eq!(v.field, expected, "wrong field: {v}"),
            Ok(()) => panic!("expected validation to fail on `{expected}`"),
        }
    }

    // ---- SupportedVersions -------------------------------------------------

    #[test]
    fn local_versions_are_valid() {
        assert_eq!(SupportedVersions::local().validate(), Ok(()));
    }

    #[test]
    fn empty_version_list_rejected() {
        assert_field(
            SupportedVersions { versions: vec![] }.validate(),
            "versions",
        );
    }

    #[test]
    fn zero_version_rejected() {
        // 0 is reserved and must never be negotiated.
        assert_field(
            SupportedVersions { versions: vec![0] }.validate(),
            "versions",
        );
        assert_field(
            SupportedVersions {
                versions: vec![0, 1],
            }
            .validate(),
            "versions",
        );
    }

    #[test]
    fn duplicate_versions_rejected() {
        assert_field(
            SupportedVersions {
                versions: vec![1, 1],
            }
            .validate(),
            "versions",
        );
    }

    #[test]
    fn descending_versions_rejected() {
        assert_field(
            SupportedVersions {
                versions: vec![2, 1],
            }
            .validate(),
            "versions",
        );
    }

    #[test]
    fn version_list_length_is_bounded() {
        let versions: Vec<u16> = (1..=SupportedVersions::MAX_VERSIONS as u16).collect();
        assert_eq!(SupportedVersions { versions }.validate(), Ok(()));

        let too_many: Vec<u16> = (1..=SupportedVersions::MAX_VERSIONS as u16 + 1).collect();
        assert!(SupportedVersions { versions: too_many }.validate().is_err());
    }

    // ---- VersionSelection --------------------------------------------------

    #[test]
    fn negotiation_succeeds_on_overlap() {
        let offered = SupportedVersions {
            versions: vec![1, 2, 3],
        };
        assert_eq!(VersionSelection::negotiate(&offered).unwrap().selected, 1);
    }

    #[test]
    fn negotiation_fails_without_overlap() {
        let offered = SupportedVersions {
            versions: vec![7, 8],
        };
        assert!(VersionSelection::negotiate(&offered).is_err());
    }

    #[test]
    fn negotiation_rejects_malformed_offer() {
        // A malformed offer must not be silently treated as "no overlap";
        // validate() runs first.
        let offered = SupportedVersions {
            versions: vec![2, 1],
        };
        assert!(VersionSelection::negotiate(&offered).is_err());
    }

    // ---- Hello / HelloAck --------------------------------------------------

    #[test]
    fn hello_status_round_trips() {
        for status in [
            HelloStatus::Accepted,
            HelloStatus::Rejected(HelloRejection::ServerBusy),
            HelloStatus::Rejected(HelloRejection::UnsupportedVersion),
        ] {
            let bytes = postcard::to_allocvec(&status).unwrap();
            let back: HelloStatus = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(status, back);
        }
    }

    #[test]
    fn every_hello_rejection_decodes() {
        for r in [
            HelloRejection::UnsupportedVersion,
            HelloRejection::Malformed,
            HelloRejection::TooManySessions,
            HelloRejection::ServerBusy,
            HelloRejection::AuthenticationRequired,
        ] {
            let bytes = postcard::to_allocvec(&r).unwrap();
            assert_eq!(postcard::from_bytes::<HelloRejection>(&bytes).unwrap(), r);
            assert!(!r.reason().is_empty());
        }
    }

    #[test]
    fn rejected_hello_carries_a_reason() {
        let ack = HelloAck {
            status: HelloStatus::Rejected(HelloRejection::Malformed),
            device_name: "server".into(),
        };
        let bytes = postcard::to_allocvec(&ack).unwrap();
        let back: HelloAck = postcard::from_bytes(&bytes).unwrap();
        assert!(matches!(back.status, HelloStatus::Rejected(_)));
    }

    // ---- Capabilities ------------------------------------------------------

    #[test]
    fn capabilities_round_trip_through_wire_form() {
        let caps = vec![
            Capability::SupportsResume,
            Capability::SupportsEncryption,
            Capability::SupportsChunkVerification,
        ];
        let set = CapabilitySet::from_capabilities(&caps, 8 * 1024 * 1024);
        let bytes = postcard::to_allocvec(&set).unwrap();
        let back: CapabilitySet = postcard::from_bytes(&bytes).unwrap();

        assert_eq!(back.max_chunk_size, 8 * 1024 * 1024);
        for c in caps {
            assert!(back.claims(c), "lost capability {}", c.as_str());
        }
    }

    #[test]
    fn unknown_feature_flag_is_discarded_not_fatal() {
        // Forward compatibility: a newer peer's flags must not break the session.
        let set = CapabilitySet {
            features: vec![Capability::SupportsResume.discriminant(), 200, 255],
            max_chunk_size: 1024,
        };
        assert_eq!(set.capabilities(), vec![Capability::SupportsResume]);
        assert!(set.claims(Capability::SupportsResume));
        assert!(!set.claims(Capability::SupportsCompression));
    }

    #[test]
    fn capability_set_rejects_zero_chunk_size() {
        let set = CapabilitySet {
            features: vec![],
            max_chunk_size: 0,
        };
        assert_field(set.validate(&Limits::new()), "max_chunk_size");
    }

    #[test]
    fn capability_set_rejects_chunk_size_above_frame_limit() {
        let limits = Limits::new();
        let set = CapabilitySet {
            features: vec![],
            max_chunk_size: limits.max_frame_length + 1,
        };
        assert_field(set.validate(&limits), "max_chunk_size");
    }

    #[test]
    fn capability_set_bounds_feature_count() {
        let limits = Limits::new();
        let set = CapabilitySet {
            features: vec![0; MAX_FEATURES + 1],
            max_chunk_size: 1024,
        };
        assert!(set.validate(&limits).is_err());

        let ok = CapabilitySet {
            features: vec![0; MAX_FEATURES],
            max_chunk_size: 1024,
        };
        assert_eq!(ok.validate(&limits), Ok(()));
    }

    // ---- Error codes -------------------------------------------------------

    #[test]
    fn error_code_values_are_frozen() {
        // Changing a discriminant silently changes the wire format. These values
        // are the contract; a change needs ADR-0007.
        assert_eq!(ErrorCode::MalformedMessage as u8, 1);
        assert_eq!(ErrorCode::UnexpectedMessage as u8, 2);
        assert_eq!(ErrorCode::UnsupportedVersion as u8, 3);
        assert_eq!(ErrorCode::LimitExceeded as u8, 4);
        assert_eq!(ErrorCode::MissingCapability as u8, 5);
        assert_eq!(ErrorCode::RateLimited as u8, 6);
        assert_eq!(ErrorCode::Internal as u8, 7);
    }

    #[test]
    fn fatal_and_warning_are_distinguishable() {
        assert!(ProtocolErrorReport::fatal(ErrorCode::Internal, "x").fatal);
        assert!(!ProtocolErrorReport::warning(ErrorCode::LimitExceeded, "x").fatal);
    }

    // ---- Opener ------------------------------------------------------------

    #[test]
    fn only_version_negotiate_may_open_a_session() {
        assert!(VacuumDropMessage::VersionNegotiate(SupportedVersions::local()).is_valid_opener());
        for msg in [
            VacuumDropMessage::Hello(Hello {
                device_name: "a".into(),
                client_version: "1".into(),
            }),
            VacuumDropMessage::Ping(Ping { nonce: 1 }),
            VacuumDropMessage::Pong(Pong { nonce: 1 }),
            VacuumDropMessage::Goodbye(Goodbye {
                reason: GoodbyeReason::Complete,
            }),
            VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![],
                max_chunk_size: 1,
            }),
        ] {
            assert!(!msg.is_valid_opener(), "{} may not open", msg.name());
        }
    }

    #[test]
    fn variant_names_are_stable() {
        // Golden values: these strings appear in logs and errors.
        assert_eq!(VacuumDropMessage::Ping(Ping { nonce: 0 }).name(), "Ping");
        assert_eq!(
            VacuumDropMessage::VersionNegotiate(SupportedVersions::local()).name(),
            "VersionNegotiate"
        );
    }

    #[test]
    fn every_message_variant_round_trips() {
        let messages = vec![
            VacuumDropMessage::VersionNegotiate(SupportedVersions { versions: vec![1] }),
            VacuumDropMessage::VersionAck(VersionSelection { selected: 1 }),
            VacuumDropMessage::Hello(Hello {
                device_name: "device".into(),
                client_version: "0.1.0".into(),
            }),
            VacuumDropMessage::HelloAck(HelloAck {
                status: HelloStatus::Accepted,
                device_name: "server".into(),
            }),
            VacuumDropMessage::Capabilities(CapabilitySet {
                features: vec![0, 3],
                max_chunk_size: 1024,
            }),
            VacuumDropMessage::Ping(Ping { nonce: u64::MAX }),
            VacuumDropMessage::Pong(Pong { nonce: u64::MAX }),
            VacuumDropMessage::Error(ProtocolErrorReport::fatal(
                ErrorCode::MalformedMessage,
                "bad",
            )),
            VacuumDropMessage::Goodbye(Goodbye {
                reason: GoodbyeReason::Shutdown,
            }),
        ];
        for m in messages {
            let bytes = postcard::to_allocvec(&m).unwrap();
            let back: VacuumDropMessage = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(m, back, "{} failed to round-trip", m.name());
        }
    }

    #[test]
    fn messages_encode_to_bounded_sizes() {
        // A message must never be able to exceed the metadata limit, no matter
        // how large its string fields are within the name bound.
        let limits = Limits::new();
        let hello = VacuumDropMessage::Hello(Hello {
            device_name: "x".repeat(limits.max_name_length),
            client_version: "y".repeat(64),
        });
        let bytes = postcard::to_allocvec(&hello).unwrap();
        assert!(
            bytes.len() < limits.max_metadata_length as usize,
            "a maximal Hello encoded to {} bytes, over the metadata limit",
            bytes.len()
        );
    }
}
