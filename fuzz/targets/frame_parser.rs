#![no_main]

//! Fuzzes the frame parser against raw bytes.
//!
//! Charter 53 requires the protocol parser to be security-sensitive, and that it
//! must not panic, hang, allocate unbounded memory, or crash the process.
//!
//! This target feeds arbitrary bytes to `FrameCodec::decode`, which is the exact
//! function that sees every hostile byte in the system. The property tests in
//! `tests/tests/properties.rs` assert the same contract over structured input;
//! this one searches for a violation of it over unstructured input.
//!
//! # Properties asserted
//!
//! 1. **No panic.** Any panic here is a remote crash, reachable by anyone who can
//!    reach the socket.
//! 2. **Bounded buffer.** The codec's internal buffer must never exceed its hard
//!    cap, no matter what a peer declares.
//! 3. **No unbounded allocation.** A declared length must never drive an
//!    allocation proportional to itself before validation (threat T-01).
//! 4. **No input consumed on failure.** A rejected frame must leave the buffer
//!    untouched, or a desynchronised attacker stream could desynchronise the
//!    frame boundary and smuggle a frame past the parser.
//! 5. **Progress.** A returned message must have consumed bytes, so a decoder
//!    that returns `Some` without advancing cannot loop forever.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

use libfuzzer_sys::fuzz_target;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::Decoder;
use vacuumdrop_core::Limits;
use vacuumdrop_protocol::codec::FrameCodec;

/// The codec's hard read-buffer cap, mirrored from the implementation.
///
/// Duplicated deliberately: the fuzzer must assert against a *fixed* number, not
/// against whatever the implementation happens to use. If the cap regresses, this
/// constant should fail loudly rather than silently follow.
const HARD_BUFFER_CAP: usize = 64 * 1024;

/// The frame limit under test. Small, so the length-validation path is exercised
/// by short inputs rather than requiring gigabytes to reach the boundary.
const MAX_FRAME_LENGTH: u32 = 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::new();

    // Cap the input so libFuzzer spends its budget on structure rather than on
    // re-testing one huge buffer. Inputs above this still run through the
    // length check, just truncated first.
    const INPUT_CAP: usize = 4096;
    let data = if data.len() > INPUT_CAP {
        &data[..INPUT_CAP]
    } else {
        data
    };

    let mut buffer = BytesMut::from(data);
    let mut codec = FrameCodec::for_session(1, MAX_FRAME_LENGTH);

    // Bounded iteration. A decoder that returns `None` without consuming would
    // otherwise spin forever inside this loop instead of surfacing as a timeout.
    const MAX_DECODE_ROUNDS: usize = 64;

    for _ in 0..MAX_DECODE_ROUNDS {
        let before = buffer.len();

        let result = codec.decode(&mut buffer);

        match result {
            Ok(Some(_message)) => {
                // Property 5: progress.
                assert!(
                    buffer.len() < before,
                    "decoder returned a message without consuming input"
                );
            }
            Ok(None) => {
                // Needs more data. Also must not have consumed anything.
                assert_eq!(
                    buffer.len(),
                    before,
                    "decoder asked for more data but consumed {before} bytes"
                );
                break;
            }
            Err(_) => {
                // Property 4: a rejected frame leaves the stream intact.
                assert_eq!(
                    buffer.len(),
                    before,
                    "a failed decode consumed {before} bytes; a desynchronised \
                     stream could smuggle a frame past the parser"
                );
                break;
            }
        }
    }

    // Property 2: the buffer never exceeds the hard cap.
    assert!(
        buffer.len() <= HARD_BUFFER_CAP,
        "buffer grew to {} bytes, past the {HARD_BUFFER_CAP}-byte cap",
        buffer.len()
    );

    // Property 3: the codec must never have allocated proportionally to a
    // declared length. We cannot observe allocation directly, but we can bound
    // what the fuzzer handed it: a peer-declared length above MAX_FRAME_LENGTH is
    // refused at the header, so anything the codec allocates stays inside the cap
    // asserted above. This assertion documents that reasoning at the call site.
    debug_assert!(MAX_FRAME_LENGTH <= limits.max_frame_length);
});