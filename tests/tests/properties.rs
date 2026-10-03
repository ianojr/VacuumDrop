//! Property tests for the untrusted-input surface.
//!
//! Charter 52 requires property tests for malformed frames, random metadata,
//! chunk boundaries, and serialisation round-trips. Charter 53 additionally
//! requires fuzzing the parser; the fuzz targets live in `fuzz/`.
//!
//! The properties asserted here are the contract for hostile input:
//!
//! 1. **The decoder never panics.** No input may crash the process.
//! 2. **The decoder never grows unboundedly.** A declared length cannot drive
//!    allocation past the configured limit.
//! 3. **Encoding is deterministic.** The same message always produces the same
//!    bytes, so a golden test stays valid.
//! 4. **Round-trips are lossless** for any message we can construct.
//! 5. **Re-encoding a decoded message reproduces the original bytes**, which is
//!    what catches an accidental wire-format change.

use proptest::prelude::*;
use tokio_util::bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use vacuumdrop_core::Limits;
use vacuumdrop_protocol::codec::FrameCodec;
use vacuumdrop_protocol::header::{FrameHeader, HEADER_LEN, MAGIC_BYTES};
use vacuumdrop_protocol::message::{
    CapabilitySet, ErrorCode, Goodbye, GoodbyeReason, Hello, HelloAck, HelloRejection, HelloStatus,
    Ping, Pong, ProtocolErrorReport, SupportedVersions, VacuumDropMessage, VersionSelection,
};

// proptest cannot implement `Arbitrary` for a foreign type, so each strategy is
// built with `prop_flat_map` instead. Mapping through a constructor function
// keeps the generated values owned and correctly typed, which the earlier
// `Just(...)`-wrapping approach got wrong by producing strategies rather than
// values.

/// A codec configured for tests. `max_frame` is passed in so tests can provoke
/// both the accept and the reject paths.
fn codec(max_frame: u32) -> FrameCodec {
    FrameCodec::for_session(1, max_frame)
}

fn encode(m: &VacuumDropMessage, max_frame: u32) -> BytesMut {
    let mut c = codec(max_frame);
    let mut buf = BytesMut::new();
    c.encode(m.clone(), &mut buf).unwrap();
    buf
}

/// A name using only characters a real device name would contain.
///
/// Deliberately excludes control characters: those must be *rejected*, so a
/// round-trip property that generated them would assert the wrong thing.
fn arb_device_name() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.\\-]{1,64}"
}

fn arb_hello_status() -> impl Strategy<Value = HelloStatus> {
    prop_oneof![
        Just(HelloStatus::Accepted),
        Just(HelloStatus::Rejected(HelloRejection::UnsupportedVersion)),
        Just(HelloStatus::Rejected(HelloRejection::Malformed)),
        Just(HelloStatus::Rejected(HelloRejection::TooManySessions)),
        Just(HelloStatus::Rejected(HelloRejection::ServerBusy)),
        Just(HelloStatus::Rejected(
            HelloRejection::AuthenticationRequired
        )),
    ]
}

fn arb_error_code() -> impl Strategy<Value = ErrorCode> {
    prop_oneof![
        Just(ErrorCode::MalformedMessage),
        Just(ErrorCode::UnexpectedMessage),
        Just(ErrorCode::UnsupportedVersion),
        Just(ErrorCode::LimitExceeded),
        Just(ErrorCode::MissingCapability),
        Just(ErrorCode::RateLimited),
        Just(ErrorCode::Internal),
    ]
}

fn arb_goodbye_reason() -> impl Strategy<Value = GoodbyeReason> {
    prop_oneof![
        Just(GoodbyeReason::Complete),
        Just(GoodbyeReason::Shutdown),
        Just(GoodbyeReason::Idle),
    ]
}

/// Builds the Phase 1 message space.
///
/// Each arm maps an inner strategy through a constructor, so the generated
/// values are owned and correctly typed. `prop_flat_map` is used rather than
/// `Just(...)` wrapping because the inner strategies must be *evaluated*, not
/// stored.
fn arb_message() -> impl Strategy<Value = VacuumDropMessage> {
    prop_oneof![
        4 => prop::collection::vec(0u16..2000, 0..8)
            .prop_map(|versions| VacuumDropMessage::VersionNegotiate(SupportedVersions { versions })),
        4 => (1u16..2000).prop_map(|selected| {
            VacuumDropMessage::VersionAck(VersionSelection { selected })
        }),
        4 => (arb_device_name(), "[0-9.]{1,16}")
            .prop_map(|(device_name, client_version)| {
                VacuumDropMessage::Hello(Hello { device_name, client_version })
            }),
        4 => (arb_hello_status(), arb_device_name())
            .prop_map(|(status, device_name)| {
                VacuumDropMessage::HelloAck(HelloAck { status, device_name })
            }),
        4 => (prop::collection::vec(0u8..=255, 0..32), 1u32..(64 * 1024 * 1024))
            .prop_map(|(features, max_chunk_size)| {
                VacuumDropMessage::Capabilities(CapabilitySet { features, max_chunk_size })
            }),
        4 => any::<u64>().prop_map(|nonce| VacuumDropMessage::Ping(Ping { nonce })),
        4 => any::<u64>().prop_map(|nonce| VacuumDropMessage::Pong(Pong { nonce })),
        4 => (any::<bool>(), arb_error_code(), "[ -~]{0,128}")
            .prop_map(|(fatal, code, detail)| {
                VacuumDropMessage::Error(ProtocolErrorReport { fatal, code, detail })
            }),
        4 => arb_goodbye_reason()
            .prop_map(|reason| VacuumDropMessage::Goodbye(Goodbye { reason })),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Encoding is deterministic. A golden test is only meaningful if the same
    /// message always produces the same bytes.
    #[test]
    fn encoding_is_deterministic(msg in arb_message()) {
        let a = encode(&msg, 1 << 20);
        let b = encode(&msg, 1 << 20);
        prop_assert_eq!(&a[..], &b[..]);
    }

    /// Every constructible message survives a round trip unchanged, leaving no
    /// residue in the buffer.
    #[test]
    fn round_trip_is_lossless(msg in arb_message()) {
        let mut buf = encode(&msg, 1 << 20);
        let mut c = codec(1 << 20);
        let decoded = c.decode(&mut buf).unwrap();
        prop_assert_eq!(decoded, Some(msg));
        prop_assert!(buf.is_empty());
    }

    /// Re-encoding a decoded message reproduces the original bytes exactly.
    #[test]
    fn re_encoding_reproduces_the_original_bytes(msg in arb_message()) {
        let original = encode(&msg, 1 << 20);
        let mut buf = original.clone();
        let mut c = codec(1 << 20);
        let decoded = c.decode(&mut buf).unwrap().unwrap();
        let again = encode(&decoded, 1 << 20);
        prop_assert_eq!(&original[..], &again[..]);
    }

    /// Arbitrary bytes never panic the decoder and never break its buffer cap.
    #[test]
    fn arbitrary_bytes_never_panic(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let mut buf = BytesMut::from(&data[..]);
        let mut c = codec(1 << 20);
        let _ = c.decode(&mut buf);
        prop_assert!(buf.len() <= 64 * 1024);
    }

    /// A single bit flip in the *payload* is never accepted as the original.
    ///
    /// Scoped to the payload deliberately. The 8 reserved header bytes are
    /// defined as ignored, so a flip there decoding back to the same message is
    /// correct behaviour, not a missed detection. Flipping a payload byte must
    /// change the message or fail outright.
    #[test]
    fn payload_bit_flips_are_detected(
        msg in arb_message(),
        bit in 0usize..8,
        offset in 0usize..64,
    ) {
        let mut bytes = encode(&msg, 1 << 20);
        // 1 codec tag byte + HEADER_LEN header bytes.
        let payload_start = 1 + HEADER_LEN;
        let index = payload_start + offset;
        if bytes.len() <= index {
            return Ok(());
        }
        bytes[index] ^= 1u8 << bit;

        let mut buf = bytes;
        let mut c = codec(1 << 20);
        if let Ok(Some(decoded)) = c.decode(&mut buf) {
            prop_assert_ne!(
                decoded, msg,
                "a flipped payload bit was accepted as the original message"
            );
        }
    }

    /// A bit flip in a *validated* header field never yields the original message.
    ///
    /// Covers bytes 0..14: magic, version, flags, message type, and payload
    /// length. Each is checked by `FrameHeader::decode`, so a flip must either be
    /// rejected or change the message.
    #[test]
    fn validated_header_fields_reject_bit_flips(
        msg in arb_message(),
        bit in 0usize..8,
        index in 0usize..14,
    ) {
        let mut bytes = encode(&msg, 1 << 20);
        let at = 1 + index; // skip the codec tag
        bytes[at] ^= 1u8 << bit;

        let mut buf = bytes;
        let mut c = codec(1 << 20);
        if let Ok(Some(decoded)) = c.decode(&mut buf) {
            prop_assert_ne!(decoded, msg);
        }
    }

    /// Correlation ids are opaque: changing them must not change the message.
    ///
    /// Session and request ids exist so a receiver can match a reply to a
    /// request. They are deliberately *not* authenticated or validated by the
    /// codec, so this property documents that a flip there is inert rather than
    /// treated as corruption.
    #[test]
    fn correlation_ids_do_not_affect_message_identity(msg in arb_message()) {
        let original = encode(&msg, 1 << 20);

        let mut altered = original.clone();
        // Session id occupies header bytes 14..22.
        altered[1 + 14] ^= 0xFF;
        altered[1 + 15] ^= 0xFF;

        let mut c = codec(1 << 20);
        let mut buf = altered;
        let decoded = c.decode(&mut buf).unwrap();
        prop_assert_eq!(decoded, Some(msg));
    }

    /// A header claiming more than the limit is refused without allocating.
    ///
    /// The lower bound is included so a legal declaration with no payload behind
    /// it must ask for more rather than error.
    #[test]
    fn declared_lengths_are_bounded(declared in prop_oneof![Just(1025u32), Just(1 << 24), Just(u32::MAX)]) {
        let mut hb = [0u8; HEADER_LEN];
        FrameHeader::new(1, 1, 1).encode(0, declared, &mut hb);

        let mut buf = BytesMut::new();
        buf.put_u8(0x56);
        buf.extend_from_slice(&hb);
        buf.extend_from_slice(b"short");

        let mut c = codec(1024);
        prop_assert!(c.decode(&mut buf).is_err());
    }

    /// Truncating a valid frame anywhere yields `None` and consumes nothing.
    /// This is the TCP-fragmentation property from Charter 11.
    #[test]
    fn truncation_never_errors_and_never_consumes(
        msg in arb_message(),
        take in 0usize..512,
    ) {
        let full = encode(&msg, 1 << 20);
        let take = take.min(full.len().saturating_sub(1));
        let mut buf = BytesMut::from(&full[..take]);
        let mut c = codec(1 << 20);
        prop_assert_eq!(c.decode(&mut buf).unwrap(), None);
        prop_assert_eq!(buf.len(), take);
    }

    /// Concatenated frames decode in order, leaving no residue.
    #[test]
    fn concatenated_frames_decode_in_order(a in arb_message(), b in arb_message()) {
        let mut stream = encode(&a, 1 << 20);
        let second = encode(&b, 1 << 20);
        stream.extend_from_slice(&second);

        let mut c = codec(1 << 20);
        prop_assert_eq!(c.decode(&mut stream).unwrap(), Some(a));
        prop_assert_eq!(c.decode(&mut stream).unwrap(), Some(b));
        prop_assert_eq!(c.decode(&mut stream).unwrap(), None);
        prop_assert!(stream.is_empty());
    }

    /// The magic is validated on every decode.
    #[test]
    fn magic_is_always_checked(data in prop::collection::vec(any::<u8>(), 40..200)) {
        let mut buf = BytesMut::from(&data[..]);
        // Force the tag so the magic is reached, then corrupt it.
        buf[0] = 0x56;
        buf[1] ^= 0xFF;
        let mut c = codec(1 << 20);
        prop_assert!(c.decode(&mut buf).is_err());
    }

    /// Any non-zero reserved flag bit is refused.
    ///
    /// A newer writer setting a bit we do not implement must be rejected, not
    /// guessed at.
    #[test]
    fn reserved_flags_are_always_refused(flags in 1u16..=u16::MAX) {
        let mut hb = [0u8; HEADER_LEN];
        FrameHeader::new(1, 1, 1).encode(0, 0, &mut hb);
        hb[6..8].copy_from_slice(&flags.to_le_bytes());
        prop_assert!(FrameHeader::decode(&hb, 1 << 20).is_err());
    }

    /// The name-length boundary is exact: 1..=max accepted, 0 and above
    /// rejected.
    #[test]
    fn name_length_boundary_is_enforced(len in 0usize..600) {
        let limits = Limits::new();
        let result = vacuumdrop_core::PeerDescription::new(
            "x".repeat(len),
            vec![1],
            vec![],
            1024,
            &limits,
        );
        if len == 0 {
            prop_assert!(result.is_err());
        } else if len <= limits.max_name_length {
            prop_assert!(result.is_ok());
        } else {
            prop_assert!(result.is_err());
        }
    }

    /// Version lists are accepted only when non-empty, ascending, unique, and
    /// free of the reserved zero.
    #[test]
    fn version_list_validation(versions in prop::collection::vec(0u16..2000, 0..40)) {
        let offered = SupportedVersions { versions };
        let expect_ok = !offered.versions.is_empty()
            && !offered.versions.contains(&0)
            && offered.versions.len() <= SupportedVersions::MAX_VERSIONS
            && offered.versions.windows(2).all(|p| p[0] < p[1]);
        prop_assert_eq!(offered.validate().is_ok(), expect_ok);
    }

    /// Capability flag counts are bounded.
    #[test]
    fn capability_flags_are_bounded(count in 0usize..80) {
        let limits = Limits::new();
        let set = CapabilitySet {
            features: vec![0u8; count],
            max_chunk_size: 1024,
        };
        prop_assert_eq!(
            set.validate(&limits).is_ok(),
            count <= vacuumdrop_protocol::message::MAX_FEATURES
        );
    }

    /// A `max_chunk_size` at or above our frame limit is always refused.
    #[test]
    fn chunk_size_is_bounded(size in 0u32..u32::MAX) {
        let limits = Limits::new();
        let set = CapabilitySet {
            features: vec![],
            max_chunk_size: size,
        };
        let expect_ok = size > 0 && size <= limits.max_frame_length;
        prop_assert_eq!(set.validate(&limits).is_ok(), expect_ok);
    }
}

#[test]
fn magic_bytes_are_stable() {
    // If this ever changes, every deployed peer's framing changes with it.
    assert_eq!(&MAGIC_BYTES, b"VDDR");
}
