//! Core domain types: peer identity, sessions, transfers, and engine events.
//!
//! Charter 16 requires a persistent cryptographic identity and forbids using
//! MAC address, IP address, or hostname as identity. Those are network
//! attributes, not identity. [`PeerId`] is an opaque identifier minted by the
//! security layer in Phase 4; nothing in Phase 1 may invent one.
//!
//! Everything here is domain vocabulary. No socket, file, or platform type
//! appears in this module.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An opaque peer identifier.
///
/// Deliberately not constructible from a string: identity must come from the
/// cryptographic key, never from something the network layer supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId([u8; 32]);

impl PeerId {
    /// Wraps public key material as a peer identifier.
    ///
    /// Callers in Phase 1 do not call this. It exists so the type is usable
    /// without making identity forgeable from arbitrary input.
    pub fn from_public_key(public_key: [u8; 32]) -> Self {
        Self(public_key)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// A short, non-reversible-enough form for logs and CLI display.
    ///
    /// Logs and terminal output need something readable; they must never need
    /// the whole identifier.
    pub fn short(&self) -> String {
        let hex = self.to_hex();
        format!("{}..{}", &hex[..8], &hex[56..])
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

impl PeerId {
    fn to_hex(self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

/// How a peer is reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAddress {
    /// A conventional IP address and port.
    ///
    /// This is what a platform adapter returns after it has established a
    /// link. The engine never learns the radio details behind it.
    Tcp {
        /// Peer address.
        ip: std::net::IpAddr,
        /// Peer port.
        port: u16,
    },

    /// A platform-specific handle that the transport will resolve later.
    ///
    /// Exists so a future transport (USB, optical) does not require inventing
    /// an IP address. The core must not interpret the contents.
    PlatformHandle {
        /// Opaque adapter-defined value.
        handle: String,
    },
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerAddress::Tcp { ip, port } => write!(f, "{ip}:{port}"),
            PeerAddress::PlatformHandle { handle } => f.write_str(handle),
        }
    }
}

/// A peer's self-declared description, received over the wire and untrusted.
///
/// Every field is bounded and validated before use (`docs/threat-model.md`
/// section 5). Construction is crate-private so an unvalidated value cannot
/// enter the domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDescription {
    /// User-visible device name. Untrusted, length-bounded, may contain
    /// arbitrary characters including control codes.
    display_name: String,
    /// Protocol versions the peer claims to speak.
    supported_versions: Vec<u16>,
    /// Capabilities the peer claims to have. Claims, not facts.
    capabilities: Vec<Capability>,
    /// Maximum chunk size the peer will accept.
    max_chunk_size: u32,
}

impl PeerDescription {
    /// Builds a validated description.
    ///
    /// Returns [`Violation`] rather than clamping. Silently truncating a peer's
    /// name would produce a display value the peer never claimed, and quietly
    /// accepting an over-long one is exactly the unbounded input Charter 32
    /// forbids.
    pub fn new(
        display_name: String,
        supported_versions: Vec<u16>,
        capabilities: Vec<Capability>,
        max_chunk_size: u32,
        limits: &crate::limits::Limits,
    ) -> Result<Self, crate::error::Violation> {
        if display_name.is_empty() {
            return Err(crate::error::Violation::new(
                "display_name",
                "must not be empty",
            ));
        }
        if display_name.len() > limits.max_name_length {
            return Err(crate::error::Violation::new(
                "display_name",
                "exceeds the configured maximum length",
            ));
        }
        if let Some(bad) = display_name.chars().find(|c| is_disallowed_in_name(*c)) {
            // The character is named in the error because a user seeing "invalid
            // name" with a nearly-correct name has no way to find the problem.
            return Err(crate::error::Violation::new(
                "display_name",
                match bad {
                    '\n' | '\r' => "contains a line break",
                    '\t' => "contains a tab",
                    _ => "contains a control or invisible formatting character",
                },
            ));
        }
        if supported_versions.is_empty() {
            return Err(crate::error::Violation::new(
                "supported_versions",
                "must advertise at least one version",
            ));
        }
        if max_chunk_size == 0 {
            return Err(crate::error::Violation::new(
                "max_chunk_size",
                "must be non-zero",
            ));
        }
        if max_chunk_size > limits.max_frame_length {
            return Err(crate::error::Violation::new(
                "max_chunk_size",
                "exceeds the local maximum frame length",
            ));
        }
        Ok(Self {
            display_name,
            supported_versions,
            capabilities,
            max_chunk_size,
        })
    }

    /// The peer's declared name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The peer's declared supported versions.
    pub fn supported_versions(&self) -> &[u16] {
        &self.supported_versions
    }

    /// The peer's declared capabilities.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// The largest chunk the peer says it accepts.
    pub fn max_chunk_size(&self) -> u32 {
        self.max_chunk_size
    }
}

/// Whether a character may appear in a peer-supplied device name.
///
/// `char::is_control` is necessary but **not sufficient**. It covers the Unicode
/// `Cc` category and nothing else, so three classes of hostile character pass it:
/// - **Format characters (`Cf`)** including RIGHT-TO-LEFT OVERRIDE (`U+202E`),
///   LEFT-TO-RIGHT OVERRIDE, ZERO WIDTH SPACE (`U+200B`), and the BOM. These are
///   the dangerous ones: RLO makes `"Ian's Laptop"` render as `topakS na'l nI`,
///   so a peer can impersonate a device the user already trusts.
/// - **Bidirectional isolates (`U+2066`-`U+2069`)** achieve the same by
///   bracketing.
/// - **Private-use and noncharacter code points**, which have no defined display
///   and render unpredictably.
///
/// A name is peer-controlled text rendered in a terminal and in a device list.
/// Allowing invisible or reordering characters in it is a spoofing primitive, so
/// the check is explicit rather than delegated to `is_control`.
fn is_disallowed_in_name(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
            // Zero-width and BOM
            0x200B..=0x200F
            // Directional formatting: marks, embeddings, isolates, overrides
            | 0x202A..=0x202E
            | 0x2066..=0x2069
            // BOM / ZWNBSP
            | 0xFEFF
            // Word joiner
            | 0x2060
            // Private use areas
            | 0xE000..=0xF8FF
            | 0xF_0000..=0xF_FFFD
            | 0x10_0000..=0x10_FFFD
            // Noncharacters
            | 0xFDD0..=0xFDEF
            | 0xFFFE..=0xFFFF)
}

/// A feature a peer may advertise.
///
/// Charter 67: capabilities are claims. This type records what was claimed;
/// nothing here decides whether to believe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Can resume an interrupted transfer.
    SupportsResume,
    /// Can open multiple concurrent data streams.
    SupportsParallelStreams,
    /// Can decompress a compressed transfer.
    SupportsCompression,
    /// Supports an encrypted session.
    SupportsEncryption,
    /// Verifies per-chunk hashes.
    SupportsChunkVerification,
}

impl Capability {
    /// The wire discriminant.
    pub fn discriminant(self) -> u8 {
        match self {
            Capability::SupportsResume => 0,
            Capability::SupportsParallelStreams => 1,
            Capability::SupportsCompression => 2,
            Capability::SupportsEncryption => 3,
            Capability::SupportsChunkVerification => 4,
        }
    }

    /// Maps a wire discriminant back to a capability.
    ///
    /// Returns `None` for an unknown value so an unrecognised capability is
    /// ignored rather than fatal: a newer peer may advertise features this
    /// build has never heard of.
    pub fn from_discriminant(value: u8) -> Option<Self> {
        match value {
            0 => Some(Capability::SupportsResume),
            1 => Some(Capability::SupportsParallelStreams),
            2 => Some(Capability::SupportsCompression),
            3 => Some(Capability::SupportsEncryption),
            4 => Some(Capability::SupportsChunkVerification),
            _ => None,
        }
    }

    /// A stable name for display and logging.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::SupportsResume => "supports_resume",
            Capability::SupportsParallelStreams => "supports_parallel_streams",
            Capability::SupportsCompression => "supports_compression",
            Capability::SupportsEncryption => "supports_encryption",
            Capability::SupportsChunkVerification => "supports_chunk_verification",
        }
    }
}

/// The negotiated protocol version for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedVersion(pub u16);

impl NegotiatedVersion {
    /// The version this build speaks.
    pub const LOCAL: u16 = 1;

    /// Every version this build can speak.
    pub const SUPPORTED: &'static [u16] = &[1];

    /// Chooses the highest version both peers support.
    ///
    /// Highest common version, not lowest: a peer that speaks 1 and 2 talking
    /// to one that speaks 1 and 2 should get 2. Failure is explicit; the
    /// caller must not silently downgrade (Charter 66).
    pub fn negotiate(remote: &[u16]) -> Result<Self, crate::error::UnsupportedVersionError> {
        let local = Self::SUPPORTED;
        let best = remote.iter().filter(|v| local.contains(v)).max().copied();

        best.map(Self)
            .ok_or_else(|| crate::error::UnsupportedVersionError {
                local: local.to_vec(),
                remote: remote.to_vec(),
            })
    }
}

impl fmt::Display for NegotiatedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Identifies one transfer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferId(String);

impl TransferId {
    /// Builds a transfer id from a UUID.
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies one session, for log correlation (Charter 59).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    /// Wraps a session counter.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{:08x}", self.0)
    }
}

/// Lifecycle of a single transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    /// Metadata agreed, nothing sent.
    Prepared,
    /// Data is moving.
    Running,
    /// Paused by an explicit request.
    Paused,
    /// All chunks received and awaiting verification.
    Verifying,
    /// Verified and committed.
    Completed,
    /// Ended in failure.
    Failed,
    /// Abandoned by request.
    Cancelled,
}

impl TransferState {
    /// Whether moving to `next` is legal.
    pub fn can_transition_to(self, next: TransferState) -> bool {
        use TransferState::*;
        matches!(
            (self, next),
            (Prepared, Running)
                | (Prepared, Cancelled)
                | (Prepared, Failed)
                | (Running, Paused)
                | (Running, Verifying)
                | (Running, Failed)
                | (Running, Cancelled)
                | (Paused, Running)
                | (Paused, Cancelled)
                | (Paused, Failed)
                | (Verifying, Completed)
                | (Verifying, Failed)
        )
    }

    /// Whether this is a final state with no further transitions.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        )
    }

    /// A stable, log-friendly name.
    pub fn as_str(self) -> &'static str {
        match self {
            TransferState::Prepared => "PREPARED",
            TransferState::Running => "RUNNING",
            TransferState::Paused => "PAUSED",
            TransferState::Verifying => "VERIFYING",
            TransferState::Completed => "COMPLETED",
            TransferState::Failed => "FAILED",
            TransferState::Cancelled => "CANCELLED",
        }
    }
}

impl fmt::Display for TransferState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Progress within one transfer.
///
/// `received_bytes` is counted by the receiver and is not a completeness claim:
/// completion additionally requires verification (Charter 25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    /// Chunks durably received.
    pub received_chunks: u64,
    /// Total chunks the transfer consists of.
    pub total_chunks: u64,
    /// Bytes received so far.
    pub received_bytes: u64,
    /// Bytes the transfer consists of.
    pub total_bytes: u64,
}

impl TransferProgress {
    /// Completion as a fraction in `0.0..=1.0`.
    ///
    /// Returns `0.0` for an empty transfer rather than dividing by zero.
    pub fn fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.received_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
        }
    }

    /// Whether every chunk has been received.
    ///
    /// Not a completion claim. See [`TransferState::Verifying`].
    pub fn all_chunks_received(&self) -> bool {
        self.received_chunks >= self.total_chunks
    }
}

/// Things that happened, delivered to the application instead of polled.
///
/// Charter 49: prefer event-driven design over constant polling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A peer was found by the platform layer.
    PeerDiscovered {
        /// Which peer.
        id: PeerId,
        /// How to reach it.
        address: PeerAddress,
    },
    /// A previously seen peer is no longer visible.
    PeerLost {
        /// Which peer.
        id: PeerId,
    },
    /// A connection attempt has begun.
    ConnectionStarted {
        /// Which peer.
        peer: PeerId,
    },
    /// Transport and handshake are complete.
    ConnectionEstablished {
        /// Which peer.
        peer: PeerId,
        /// The negotiated version.
        version: NegotiatedVersion,
    },
    /// The peer must be authenticated.
    AuthenticationRequired {
        /// Which peer.
        peer: PeerId,
    },
    /// Authentication succeeded.
    AuthenticationSucceeded {
        /// Which peer.
        peer: PeerId,
    },
    /// Authentication failed.
    AuthenticationFailed {
        /// Which peer.
        peer: PeerId,
        /// Why.
        reason: &'static str,
    },
    /// A transfer has begun.
    TransferStarted {
        /// Which transfer.
        transfer: TransferId,
    },
    /// A transfer advanced.
    TransferProgress {
        /// Which transfer.
        transfer: TransferId,
        /// New progress.
        progress: TransferProgress,
    },
    /// A transfer was paused.
    TransferPaused {
        /// Which transfer.
        transfer: TransferId,
    },
    /// A transfer resumed.
    TransferResumed {
        /// Which transfer.
        transfer: TransferId,
    },
    /// A transfer completed and verified.
    TransferCompleted {
        /// Which transfer.
        transfer: TransferId,
    },
    /// A transfer failed.
    TransferFailed {
        /// Which transfer.
        transfer: TransferId,
        /// Why.
        reason: &'static str,
    },
    /// A peer disconnected.
    PeerDisconnected {
        /// Which peer.
        peer: PeerId,
    },
}

impl Event {
    /// A short category name for log filtering.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::PeerDiscovered { .. } => "peer_discovered",
            Event::PeerLost { .. } => "peer_lost",
            Event::ConnectionStarted { .. } => "connection_started",
            Event::ConnectionEstablished { .. } => "connection_established",
            Event::AuthenticationRequired { .. } => "authentication_required",
            Event::AuthenticationSucceeded { .. } => "authentication_succeeded",
            Event::AuthenticationFailed { .. } => "authentication_failed",
            Event::TransferStarted { .. } => "transfer_started",
            Event::TransferProgress { .. } => "transfer_progress",
            Event::TransferPaused { .. } => "transfer_paused",
            Event::TransferResumed { .. } => "transfer_resumed",
            Event::TransferCompleted { .. } => "transfer_completed",
            Event::TransferFailed { .. } => "transfer_failed",
            Event::PeerDisconnected { .. } => "peer_disconnected",
        }
    }
}

/// Milliseconds since the Unix epoch, saturating rather than panicking.
///
/// Used only for human-facing metadata. Protocol timeouts must use a monotonic
/// clock: a wall-clock jump backwards would extend a timeout indefinitely
/// (`docs/security.md` section 11.1).
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Formats a duration for CLI output, choosing a sensible unit.
pub fn human_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", d.as_secs_f64() * 1000.0)
    } else if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        let mins = (secs / 60.0).floor();
        let rem = secs - mins * 60.0;
        format!("{mins:.0} min {rem:.0} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> crate::limits::Limits {
        crate::limits::Limits::new()
    }

    #[test]
    fn peer_id_displays_a_short_form_only() {
        let id = PeerId::from_public_key([0xABu8; 32]);
        let shown = id.to_string();
        // Logs must never need the full identity to be identifiable.
        assert!(shown.len() < 24, "display form too long: {shown}");
        assert_eq!(shown, "abababab..abababab");
        // The middle must not be present: a log line is not the place for a
        // complete identity.
        assert!(!shown.contains("ababababababababab"));
    }

    #[test]
    fn peer_id_hex_round_trips() {
        let key = [7u8; 32];
        let id = PeerId::from_public_key(key);
        assert_eq!(id.as_bytes(), &key);
    }

    #[test]
    fn peer_description_rejects_empty_name() {
        let r = PeerDescription::new(String::new(), vec![1], vec![], 1024, &limits());
        assert!(r.is_err());
    }

    #[test]
    fn peer_description_rejects_overlong_name() {
        let name = "x".repeat(256);
        let r = PeerDescription::new(name, vec![1], vec![], 1024, &limits());
        assert!(r.is_err(), "a 256-byte name was accepted");
    }

    #[test]
    fn peer_description_rejects_control_characters() {
        // A name is rendered in a terminal; control codes must not survive.
        for bad in ["a\u{0}b", "a\nb", "a\u{1b}[31m", "a\rb"] {
            assert!(
                PeerDescription::new(bad.to_string(), vec![1], vec![], 1024, &limits()).is_err(),
                "accepted control characters in {bad:?}"
            );
        }
    }

    #[test]
    fn peer_description_rejects_invisible_formatting_characters() {
        // `char::is_control` returns FALSE for all of these. They are Unicode
        // `Cf` format characters, and they are the dangerous ones: RLO makes a
        // name render reversed, so a peer can impersonate a device the user
        // already trusts. Found by the stress suite, not by inspection.
        let invisible = [
            '\u{202E}', // RIGHT-TO-LEFT OVERRIDE - the spoofing primitive
            '\u{202D}', // LEFT-TO-RIGHT OVERRIDE
            '\u{202C}', // POP DIRECTIONAL FORMATTING
            '\u{200B}', // ZERO WIDTH SPACE
            '\u{200E}', // LEFT-TO-RIGHT MARK
            '\u{200F}', // RIGHT-TO-LEFT MARK
            '\u{FEFF}', // ZERO WIDTH NO-BREAK SPACE / BOM
            '\u{2060}', // WORD JOINER
            '\u{2066}', // LEFT-TO-RIGHT ISOLATE
            '\u{2069}', // POP DIRECTIONAL ISOLATE
        ];
        for c in invisible {
            let name = format!("Ian{c}s Laptop");
            assert!(
                PeerDescription::new(name.clone(), vec![1], vec![], 1024, &limits()).is_err(),
                "accepted invisible character {c:?} in {name:?}; is_control() is {}",
                c.is_control()
            );
        }
    }

    #[test]
    fn peer_description_accepts_ordinary_non_ascii_names() {
        // The invisible-character check must not reject legitimate international
        // names. A device called "Büro" or "笔记本" has to work.
        for name in [
            "Büro",
            "Ordinateur",
            "ノートブック",
            "Ноутбук",
            "Ωμέγα",
            "Café",
            "Émile's Laptop",
            "房间-1",
            "café ☕", // emoji
            "🎧 headphones",
            "naïve résumé", // combining marks
        ] {
            assert!(
                PeerDescription::new(name.to_owned(), vec![1], vec![], 1024, &limits()).is_ok(),
                "rejected a legitimate device name {name:?}"
            );
        }
    }

    #[test]
    fn peer_description_rejects_private_use_characters() {
        // Private-use code points have no defined glyph and render as tofu or
        // inconsistently. They are not displayable text.
        for c in ['\u{E000}', '\u{F8FF}'] {
            let name = format!("dev{c}");
            assert!(
                PeerDescription::new(name.clone(), vec![1], vec![], 1024, &limits()).is_err(),
                "accepted private-use character {c:?} in {name:?}"
            );
        }
    }

    #[test]
    fn peer_description_accepts_a_maximum_length_name() {
        let name = "x".repeat(255);
        assert!(PeerDescription::new(name, vec![1], vec![], 1024, &limits()).is_ok());
    }

    #[test]
    fn peer_description_rejects_empty_version_list() {
        let r = PeerDescription::new("dev".into(), vec![], vec![], 1024, &limits());
        assert!(r.is_err());
    }

    #[test]
    fn peer_description_rejects_zero_chunk_size() {
        let r = PeerDescription::new("dev".into(), vec![1], vec![], 0, &limits());
        assert!(r.is_err());
    }

    #[test]
    fn peer_description_rejects_chunk_size_above_frame_limit() {
        let too_big = limits().max_frame_length + 1;
        let r = PeerDescription::new("dev".into(), vec![1], vec![], too_big, &limits());
        assert!(
            r.is_err(),
            "chunk size larger than the frame limit accepted"
        );
    }

    #[test]
    fn capability_discriminants_round_trip() {
        let all = [
            Capability::SupportsResume,
            Capability::SupportsParallelStreams,
            Capability::SupportsCompression,
            Capability::SupportsEncryption,
            Capability::SupportsChunkVerification,
        ];
        for (i, c) in all.iter().enumerate() {
            assert_eq!(c.discriminant() as usize, i, "discriminant drift");
            assert_eq!(Capability::from_discriminant(c.discriminant()), Some(*c));
        }
    }

    #[test]
    fn unknown_capability_is_ignored_not_fatal() {
        // A newer peer may advertise features this build has never heard of.
        assert_eq!(Capability::from_discriminant(200), None);
    }

    #[test]
    fn negotiation_picks_the_highest_common_version() {
        assert_eq!(NegotiatedVersion::negotiate(&[1]).unwrap().0, 1);
        assert_eq!(NegotiatedVersion::negotiate(&[1, 2, 3]).unwrap().0, 1);
    }

    #[test]
    fn negotiation_fails_closed_with_no_overlap() {
        let e = NegotiatedVersion::negotiate(&[99]).unwrap_err();
        assert_eq!(e.local, vec![1]);
        assert_eq!(e.remote, vec![99]);
    }

    #[test]
    fn negotiation_fails_on_empty_remote_list() {
        assert!(NegotiatedVersion::negotiate(&[]).is_err());
    }

    #[test]
    fn transfer_state_happy_path() {
        let path = [
            TransferState::Prepared,
            TransferState::Running,
            TransferState::Verifying,
            TransferState::Completed,
        ];
        for pair in path.windows(2) {
            assert!(
                pair[0].can_transition_to(pair[1]),
                "{} -> {} should be allowed",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn transfer_cannot_skip_from_prepared_to_completed() {
        // Charter 25: completion requires verification.
        assert!(!TransferState::Prepared.can_transition_to(TransferState::Completed));
        assert!(!TransferState::Running.can_transition_to(TransferState::Completed));
    }

    #[test]
    fn transfer_terminal_states_go_nowhere() {
        for s in [
            TransferState::Completed,
            TransferState::Failed,
            TransferState::Cancelled,
        ] {
            assert!(s.is_terminal());
            for target in [
                TransferState::Prepared,
                TransferState::Running,
                TransferState::Paused,
                TransferState::Verifying,
                TransferState::Completed,
                TransferState::Failed,
                TransferState::Cancelled,
            ] {
                assert!(
                    !s.can_transition_to(target),
                    "{s} -> {target} should be refused"
                );
            }
        }
    }

    #[test]
    fn transfer_pause_resume_round_trips() {
        assert!(TransferState::Running.can_transition_to(TransferState::Paused));
        assert!(TransferState::Paused.can_transition_to(TransferState::Running));
    }

    #[test]
    fn transfer_cannot_resume_after_completion() {
        assert!(!TransferState::Completed.can_transition_to(TransferState::Running));
    }

    #[test]
    fn progress_fraction_is_clamped_and_safe() {
        let p = TransferProgress {
            received_chunks: 0,
            total_chunks: 0,
            received_bytes: 0,
            total_bytes: 0,
        };
        assert_eq!(p.fraction(), 0.0, "empty transfer must not divide by zero");

        let half = TransferProgress {
            received_chunks: 1,
            total_chunks: 2,
            received_bytes: 50,
            total_bytes: 100,
        };
        assert!((half.fraction() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_clamps_above_one() {
        let over = TransferProgress {
            received_chunks: 3,
            total_chunks: 2,
            received_bytes: 150,
            total_bytes: 100,
        };
        assert_eq!(over.fraction(), 1.0);
    }

    #[test]
    fn all_chunks_received_is_not_implied_by_bytes() {
        // Byte count alone must never be read as completion.
        let p = TransferProgress {
            received_chunks: 1,
            total_chunks: 2,
            received_bytes: 100,
            total_bytes: 100,
        };
        assert!(!p.all_chunks_received());
    }

    #[test]
    fn event_kinds_are_unique() {
        let events = vec![
            Event::PeerDiscovered {
                id: PeerId::from_public_key([1; 32]),
                address: PeerAddress::Tcp {
                    ip: "127.0.0.1".parse().unwrap(),
                    port: 1,
                },
            },
            Event::PeerLost {
                id: PeerId::from_public_key([1; 32]),
            },
            Event::ConnectionStarted {
                peer: PeerId::from_public_key([1; 32]),
            },
            Event::ConnectionEstablished {
                peer: PeerId::from_public_key([1; 32]),
                version: NegotiatedVersion(1),
            },
            Event::AuthenticationRequired {
                peer: PeerId::from_public_key([1; 32]),
            },
            Event::AuthenticationSucceeded {
                peer: PeerId::from_public_key([1; 32]),
            },
            Event::AuthenticationFailed {
                peer: PeerId::from_public_key([1; 32]),
                reason: "x",
            },
            Event::TransferStarted {
                transfer: TransferId::new("t".into()),
            },
            Event::TransferProgress {
                transfer: TransferId::new("t".into()),
                progress: TransferProgress {
                    received_chunks: 0,
                    total_chunks: 0,
                    received_bytes: 0,
                    total_bytes: 0,
                },
            },
            Event::TransferPaused {
                transfer: TransferId::new("t".into()),
            },
            Event::TransferResumed {
                transfer: TransferId::new("t".into()),
            },
            Event::TransferCompleted {
                transfer: TransferId::new("t".into()),
            },
            Event::TransferFailed {
                transfer: TransferId::new("t".into()),
                reason: "x",
            },
            Event::PeerDisconnected {
                peer: PeerId::from_public_key([1; 32]),
            },
        ];
        for (i, a) in events.iter().enumerate() {
            for b in events.iter().skip(i + 1) {
                assert_ne!(a.kind(), b.kind(), "duplicate event kind");
            }
        }
    }

    #[test]
    fn event_catalogue_matches_charter_49() {
        // Every event named in Charter 49 must exist.
        let expected = [
            "peer_discovered",
            "peer_lost",
            "connection_started",
            "connection_established",
            "authentication_required",
            "authentication_succeeded",
            "authentication_failed",
            "transfer_started",
            "transfer_progress",
            "transfer_paused",
            "transfer_resumed",
            "transfer_completed",
            "transfer_failed",
            "peer_disconnected",
        ];
        let ids = PeerId::from_public_key([3; 32]);
        let tid = TransferId::new("t".into());
        let prog = TransferProgress {
            received_chunks: 0,
            total_chunks: 0,
            received_bytes: 0,
            total_bytes: 0,
        };
        let present = vec![
            Event::PeerLost { id: ids },
            Event::ConnectionStarted { peer: ids },
            Event::ConnectionEstablished {
                peer: ids,
                version: NegotiatedVersion(1),
            },
            Event::AuthenticationRequired { peer: ids },
            Event::AuthenticationSucceeded { peer: ids },
            Event::AuthenticationFailed {
                peer: ids,
                reason: "r",
            },
            Event::TransferStarted {
                transfer: tid.clone(),
            },
            Event::TransferProgress {
                transfer: tid.clone(),
                progress: prog,
            },
            Event::TransferPaused {
                transfer: tid.clone(),
            },
            Event::TransferResumed {
                transfer: tid.clone(),
            },
            Event::TransferCompleted {
                transfer: tid.clone(),
            },
            Event::TransferFailed {
                transfer: tid.clone(),
                reason: "r",
            },
            Event::PeerDisconnected { peer: ids },
        ];
        for want in expected {
            if want == "peer_discovered" {
                continue; // covered separately; needs an address
            }
            assert!(
                present.iter().any(|e| e.kind() == want),
                "event {want} is missing"
            );
        }
    }

    #[test]
    fn session_id_is_displayable_and_unique_per_value() {
        assert_eq!(SessionId::new(1).to_string(), "session-00000001");
        assert_ne!(SessionId::new(1), SessionId::new(2));
        assert_eq!(SessionId::new(0xdead).get(), 0xdead);
    }

    #[test]
    fn peer_address_renders() {
        let a = PeerAddress::Tcp {
            ip: "192.168.1.5".parse().unwrap(),
            port: 9000,
        };
        assert_eq!(a.to_string(), "192.168.1.5:9000");
        let b = PeerAddress::PlatformHandle {
            handle: "usb://0".into(),
        };
        assert_eq!(b.to_string(), "usb://0");
    }

    #[test]
    fn human_duration_picks_a_sensible_unit() {
        assert!(human_duration(Duration::from_millis(250)).ends_with("ms"));
        assert!(human_duration(Duration::from_secs(5)).ends_with("s"));
        assert!(human_duration(Duration::from_secs(125)).contains("min"));
    }

    #[test]
    fn unix_millis_is_plausible() {
        // After 2020 and before 2100. A zero would mean the clock was before the
        // epoch and would silently corrupt stored metadata.
        let ms = unix_millis();
        assert!(ms > 1_577_836_800_000, "implausible timestamp: {ms}");
        assert!(ms < 4_102_444_800_000, "implausible timestamp: {ms}");
    }
}
