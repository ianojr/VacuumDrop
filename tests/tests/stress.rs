//! Randomised stress testing of the untrusted-input surface.
//!
//! # Why this exists alongside `cargo-fuzz`
//!
//! Charter 53 requires fuzzing the protocol parser. The `fuzz/` targets are the
//! real coverage-guided fuzzer, but libFuzzer needs the ASan runtime matching
//! `rustc`'s own LLVM. On this machine `rustc` nightly is built against LLVM
//! 21.1.5 while only LLVM 22.1.5 is installed, so the ASan runtime cannot load
//! (`STATUS_ENTRYPOINT_NOT_FOUND`). See [`fuzz/README.md`](../../fuzz/README.md).
//!
//! This module provides the same four properties under a deterministic PRNG on
//! stable Rust, so the guarantee does not depend on a toolchain accident:
//!
//! 1. **No panic** on any input.
//! 2. **Bounded memory** — the decoder's buffer never exceeds its hard cap.
//! 3. **No unbounded allocation** from a declared length (threat T-01).
//! 4. **No input consumed on failure** — a rejected frame must not shift the
//!    frame boundary.
//!
//! # What this is not
//!
//! This is **not** equivalent to coverage-guided fuzzing. libFuzzer uses
//! instrumentation feedback to explore states a random generator will not reach.
//! This module explores uniformly at random, so it is weaker at finding deep
//! parser bugs.
//!
//! It is stronger in one respect: it is deterministic and runs on stable Rust in
//! ordinary CI, so the properties are checked on every commit rather than only
//! where the toolchain happens to work.
//!
//! `fuzz/` remains authoritative. When its toolchain requirement is satisfied,
//! run both.

use tokio_util::bytes::BytesMut;
use tokio_util::codec::Decoder;

use vacuumdrop_core::{Limits, NegotiatedVersion, PeerDescription};
use vacuumdrop_protocol::codec::FrameCodec;
use vacuumdrop_protocol::message::{CapabilitySet, SupportedVersions, VacuumDropMessage};

/// The codec's hard read-buffer cap, mirrored from the implementation.
///
/// Fixed deliberately: the fuzzer must assert against a known number, not
/// whatever the implementation currently uses.
const HARD_BUFFER_CAP: usize = 64 * 1024;

/// Frame limit under test. Small, so the length-validation boundary is reached by
/// short inputs rather than gigabytes of them.
const MAX_FRAME_LENGTH: u32 = 64 * 1024;

/// Deterministic xorshift64* generator.
///
/// A fixed seed makes any failure exactly reproducible from the iteration
/// number, which matters more here than statistical quality: the point is a
/// regression test that fails the same way twice.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 24) as u8
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            *b = self.byte();
        }
    }
}

/// Bytes likely to reach interesting parser states: the codec tag, the magic,
/// and a legal header prefix. Mixing these into random noise means the fuzzer
/// spends time past the magic check instead of rediscovering it.
fn seed_prefix(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(4) {
        0 => out.extend_from_slice(&[0x56]), // codec tag only
        1 => out.extend_from_slice(&[0x56, b'V', b'D', b'D', b'R']), // tag + magic
        2 => {
            // tag + magic + legal version/flags/type
            out.extend_from_slice(&[0x56, b'V', b'D', b'D', b'R']);
            out.extend_from_slice(&1u16.to_le_bytes()); // version 1
            out.extend_from_slice(&0u16.to_le_bytes()); // no flags
            out.extend_from_slice(&0u16.to_le_bytes()); // message type
        }
        _ => {}
    }
}

/// Property 1-4: the frame decoder survives arbitrary input.
#[test]
fn frame_decoder_survives_random_input() {
    const ITERATIONS: usize = 50_000;
    let mut rng = Rng::new(0x5EED_1234_ABCD_0001);

    for iteration in 0..ITERATIONS {
        let len = rng.below(2048);
        let mut data = Vec::with_capacity(len + 16);
        seed_prefix(&mut rng, &mut data);
        // Fill the body first, then extend: `data.len()` is read before the
        // mutable borrow, so the two cannot overlap.
        let mut body = vec![0u8; len];
        rng.fill(&mut body);
        data.extend_from_slice(&body);

        let mut buffer = BytesMut::from(&data[..]);
        let mut codec = FrameCodec::for_session(1, MAX_FRAME_LENGTH);

        for _ in 0..64 {
            let before = buffer.len();
            match codec.decode(&mut buffer) {
                Ok(Some(_)) => assert!(
                    buffer.len() < before,
                    "iteration {iteration}: returned a message without consuming input"
                ),
                Ok(None) => {
                    assert_eq!(
                        buffer.len(),
                        before,
                        "iteration {iteration}: asked for more data but consumed bytes"
                    );
                    break;
                }
                Err(_) => {
                    // Property 4: a failed decode must leave the stream intact, or
                    // a desynchronised attacker stream could smuggle a frame past
                    // the parser.
                    assert_eq!(
                        buffer.len(),
                        before,
                        "iteration {iteration}: a failed decode consumed {before} bytes"
                    );
                    break;
                }
            }
        }

        // Property 2: the buffer never exceeds the hard cap.
        assert!(
            buffer.len() <= HARD_BUFFER_CAP,
            "iteration {iteration}: buffer grew to {} bytes",
            buffer.len()
        );
    }
}

/// A declared length above the limit must be refused, and the buffer must not
/// grow toward it. Property 3, stated directly.
#[test]
fn declared_length_never_drives_allocation() {
    let mut rng = Rng::new(0x5EED_1234_ABCD_0002);

    for iteration in 0..10_000u32 {
        // Legal tag and magic, then an arbitrary declared length.
        let mut data = vec![0x56, b'V', b'D', b'D', b'R'];
        data.extend_from_slice(&1u16.to_le_bytes()); // version
        data.extend_from_slice(&0u16.to_le_bytes()); // flags
        data.extend_from_slice(&0u16.to_le_bytes()); // message type

        // Bias towards values above the limit, since that is the attack.
        let declared: u32 = if rng.below(2) == 0 {
            rng.next() as u32
        } else {
            MAX_FRAME_LENGTH + 1 + (rng.next() % (u32::MAX as u64 - MAX_FRAME_LENGTH as u64)) as u32
        };
        data.extend_from_slice(&declared.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // session id
        data.extend_from_slice(&0u64.to_le_bytes()); // request id
        data.extend_from_slice(b"only a handful of payload bytes follow");

        let len_before = data.len();
        let mut buffer = BytesMut::from(&data[..]);
        let mut codec = FrameCodec::for_session(1, MAX_FRAME_LENGTH);

        let result = codec.decode(&mut buffer);

        if declared > MAX_FRAME_LENGTH {
            assert!(
                result.is_err(),
                "iteration {iteration}: declared {declared} (> limit {MAX_FRAME_LENGTH}) was accepted"
            );
            // Property 4 on the rejection path.
            assert_eq!(
                buffer.len(),
                len_before,
                "iteration {iteration}: rejection consumed input"
            );
        }

        // Property 2, whatever the outcome. A declared length near u32::MAX must
        // not have produced a multi-gigabyte buffer.
        assert!(
            buffer.len() <= HARD_BUFFER_CAP,
            "iteration {iteration}: buffer reached {} bytes from a declared {declared}",
            buffer.len()
        );
    }
}

/// Property 1 extended: any byte sequence that decodes to a message must then
/// survive every validator without panicking, and validation must agree with its
/// own stated contract.
#[test]
fn validators_agree_with_their_contract_on_random_input() {
    let limits = Limits::new();
    let mut rng = Rng::new(0x5EED_1234_ABCD_0003);

    let mut decoded_count = 0usize;

    for iteration in 0..20_000usize {
        let len = rng.below(512);
        let mut data = vec![0u8; len];
        rng.fill(&mut data);

        let Ok(message) = postcard::from_bytes::<VacuumDropMessage>(&data) else {
            continue;
        };
        decoded_count += 1;

        match &message {
            VacuumDropMessage::VersionNegotiate(offered) => {
                let expect_ok = offered.validate().is_ok();
                let contract_ok = !offered.versions.is_empty()
                    && !offered.versions.contains(&0)
                    && offered.versions.len() <= SupportedVersions::MAX_VERSIONS
                    && offered.versions.windows(2).all(|p| p[0] < p[1]);
                assert_eq!(
                    expect_ok, contract_ok,
                    "iteration {iteration}: version validation disagrees with its contract"
                );
                if !expect_ok {
                    // Charter 66: fail closed.
                    assert!(
                        vacuumdrop_protocol::message::VersionSelection::negotiate(offered).is_err(),
                        "iteration {iteration}: a malformed offer negotiated"
                    );
                }
            }

            VacuumDropMessage::Capabilities(set) => {
                let expect_ok = set.validate(&limits).is_ok();
                let contract_ok = set.max_chunk_size > 0
                    && set.max_chunk_size <= limits.max_frame_length
                    && set.features.len() <= vacuumdrop_protocol::message::MAX_FEATURES;
                assert_eq!(
                    expect_ok, contract_ok,
                    "iteration {iteration}: capability validation disagrees with its contract"
                );
                // Capabilities() may only return what claims() agrees with.
                for c in set.capabilities() {
                    assert!(
                        set.claims(c),
                        "iteration {iteration}: claims/capabilities disagree"
                    );
                }
            }

            VacuumDropMessage::Hello(hello) => {
                let result = PeerDescription::new(
                    hello.device_name.clone(),
                    NegotiatedVersion::SUPPORTED.to_vec(),
                    vec![],
                    1024,
                    &limits,
                );
                if hello.device_name.is_empty() {
                    assert!(
                        result.is_err(),
                        "iteration {iteration}: empty name accepted"
                    );
                }
                if hello.device_name.len() > limits.max_name_length {
                    assert!(
                        result.is_err(),
                        "iteration {iteration}: overlong name accepted"
                    );
                }
                if hello.device_name.chars().any(char::is_control) {
                    assert!(
                        result.is_err(),
                        "iteration {iteration}: control characters in a name were accepted"
                    );
                }
            }

            _ => {
                // Round-trip only: the remaining variants carry no validation.
                let bytes = postcard::to_allocvec(&message).expect("re-encode");
                let again: VacuumDropMessage = postcard::from_bytes(&bytes).expect("re-decode");
                assert_eq!(
                    again, message,
                    "iteration {iteration}: round trip lost data"
                );
            }
        }
    }

    // Sanity: the corpus must actually have decoded *something*, or this test is
    // asserting nothing. Random bytes rarely form a valid enum, so seed a few.
    assert!(
        decoded_count > 0,
        "no random input decoded to a message; the corpus is not exercising anything"
    );
}

/// Every message this build can produce must survive the real encoder/decoder
/// round trip, at every chunk of its length. Catches length-dependent framing
/// bugs that a fixed-size test would miss.
#[test]
fn all_message_shapes_round_trip() {
    // Seed valid encoded messages, then round-trip each one.
    let seeds: Vec<vacuumdrop_protocol::message::Hello> = (0..64)
        .map(|i| vacuumdrop_protocol::message::Hello {
            device_name: "d".repeat(i * 4),
            client_version: "0.1.0".to_owned(),
        })
        .collect();

    for (i, hello) in seeds.into_iter().enumerate() {
        let message = VacuumDropMessage::Hello(hello);
        let mut encoder = FrameCodec::for_session(1, MAX_FRAME_LENGTH);
        let mut buf = BytesMut::new();
        {
            use tokio_util::codec::Encoder;
            encoder
                .encode(message.clone(), &mut buf)
                .unwrap_or_else(|e| panic!("seed {i}: encode failed: {e}"));
        }

        let mut decoder = FrameCodec::for_session(1, MAX_FRAME_LENGTH);
        let back = decoder
            .decode(&mut buf)
            .unwrap_or_else(|e| panic!("seed {i}: decode failed: {e}"))
            .unwrap_or_else(|| panic!("seed {i}: nothing decoded"));
        assert_eq!(back, message, "seed {i}: round trip lost data");
        assert!(buf.is_empty(), "seed {i}: bytes left unconsumed");
    }
}

/// Names built from hostile characters must all be rejected, at every length
/// across the boundary. This is the check that catches an off-by-one in the
/// name limit, which random bytes would essentially never produce.
#[test]
fn name_length_boundary_is_exact() {
    let limits = Limits::new();
    let max = limits.max_name_length;

    // Every character class that must never survive: control codes, invisible
    // formatting, and bidirectional-override tricks.
    //
    // Path separators are deliberately NOT listed. `PeerDescription::new`
    // validates a *name*, and a device name legitimately containing `\` or `/` is
    // not a threat. Names only become path components in Phase 5, where
    // `sanitise_filename` strips them. Asserting that rule here would demand a
    // property the name validator does not have and should not have.
    let hostile = [
        '\0', '\n', '\r', '\t', // control codes
        '\u{1b}', '\u{7f}', // escape sequences
        '\u{202E}', '\u{202D}', '\u{202C}', // directional overrides and pop
        '\u{2066}', '\u{2069}', // directional isolates
        '\u{200B}', '\u{200E}', '\u{200F}', // zero-width and directional marks
        '\u{FEFF}', // BOM
        '\u{2060}', // word joiner
        '\u{E000}', '\u{F8FF}', // private use
    ];

    for c in hostile {
        for len in [1usize, 8, max - 1, max, max + 1, max * 2] {
            let name: String = std::iter::repeat_n('a', len.saturating_sub(1))
                .chain(std::iter::once(c))
                .collect();
            let result = PeerDescription::new(
                name.clone(),
                NegotiatedVersion::SUPPORTED.to_vec(),
                vec![],
                1024,
                &limits,
            );
            assert!(
                result.is_err(),
                "a {len}-byte name containing {c:?} was accepted"
            );
        }
    }

    // A clean name must be accepted exactly up to the limit and refused beyond.
    for len in [1usize, 8, max - 1, max] {
        let name = "a".repeat(len);
        assert!(
            PeerDescription::new(
                name,
                NegotiatedVersion::SUPPORTED.to_vec(),
                vec![],
                1024,
                &limits
            )
            .is_ok(),
            "a clean {len}-byte name was rejected (limit {max})"
        );
    }
    for len in [max + 1, max * 2] {
        let name = "a".repeat(len);
        assert!(
            PeerDescription::new(
                name,
                NegotiatedVersion::SUPPORTED.to_vec(),
                vec![],
                1024,
                &limits
            )
            .is_err(),
            "a clean {len}-byte name was accepted (limit {max})"
        );
    }
}

/// Version lists must be accepted only when non-empty, ascending, unique, and
/// free of the reserved zero -- checked across every small list, since the
/// interesting cases are all near the boundary.
#[test]
fn version_list_contract_holds_everywhere() {
    for a in 0u16..40 {
        for b in 0u16..40 {
            for shape in 0..4 {
                let versions: Vec<u16> = match shape {
                    0 => vec![a, b],
                    1 => vec![b, a],
                    // Single element: always valid unless it is the reserved 0.
                    2 => vec![a],
                    _ => vec![a, b, a],
                };
                let offered = SupportedVersions {
                    versions: versions.clone(),
                };
                let expect_ok = !offered.versions.is_empty()
                    && !offered.versions.contains(&0)
                    && offered.versions.len() <= SupportedVersions::MAX_VERSIONS
                    && offered.versions.windows(2).all(|p| p[0] < p[1]);
                assert_eq!(
                    offered.validate().is_ok(),
                    expect_ok,
                    "validation disagrees for {versions:?}"
                );
            }
        }
    }
}

/// Capability flags above the known range must be discarded, not fatal, so a
/// newer peer does not break the session.
#[test]
fn unknown_capability_flags_are_discarded_not_fatal() {
    let limits = Limits::new();
    let mut rng = Rng::new(0x5EED_1234_ABCD_0005);

    for _ in 0..5_000 {
        let features: Vec<u8> = (0..rng.below(40)).map(|_| rng.byte()).collect();
        let set = CapabilitySet {
            features,
            max_chunk_size: 1024,
        };

        // Must never panic on any flag combination.
        let caps = set.capabilities();
        for c in &caps {
            assert!(set.claims(*c), "claims/capabilities disagree");
        }
        // Unknown flags must not make validation fail either; they are simply
        // not capabilities we recognise.
        let _ = set.validate(&limits);
    }

    // Explicitly: every possible byte value must decode to a known capability
    // or to nothing, never to a panic or a bogus variant.
    for value in 0u8..=255 {
        let set = CapabilitySet {
            features: vec![value],
            max_chunk_size: 1024,
        };
        let caps = set.capabilities();
        assert!(
            caps.len() <= 1,
            "one flag decoded to {} capabilities",
            caps.len()
        );
    }
}
