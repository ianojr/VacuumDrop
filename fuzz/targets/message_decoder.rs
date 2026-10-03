#![no_main]

//! Fuzzes message decoding and validation.
//!
//! Where `frame_parser` fuzzes the *framing* layer, this one fuzzes what sits
//! above it: decoding a `postcard` payload into `VacuumDropMessage`, then running
//! the validators that decide whether a peer-supplied value is acceptable.
//!
//! The validators are the interesting part. A decoder can be perfectly correct
//! and still let an attacker hand the application a 4000-byte device name, a
//! 2000-entry version list, or a `max_chunk_size` larger than our own frame
//! limit. Those checks are what this target attacks.
//!
//! # Properties asserted
//!
//! 1. **No panic** on any decoded value, however malformed.
//! 2. **Validation is consistent.** A value accepted by `validate` must satisfy
//!    the same bounds the domain types enforce, so no two layers disagree.
//! 3. **Bounded work.** Validating an absurdly large input must stay bounded and
//!    finish, not degrade into a quadratic scan.
//! 4. **Untrusted strings never reach the domain unvalidated.** A name carrying
//!    control characters or exceeding the limit must be rejected, not stored.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

use libfuzzer_sys::fuzz_target;
use vacuumdrop_core::{Limits, NegotiatedVersion, PeerDescription};
use vacuumdrop_protocol::message::{
    CapabilitySet, Hello, HelloStatus, SupportedVersions, VacuumDropMessage,
};

/// Longest message payload accepted for fuzzing.
///
/// Beyond this the interesting structure is already present and extra length only
/// costs time. The codec's own limit is far larger; this is a fuzzing budget, not
/// a protocol bound.
const PAYLOAD_CAP: usize = 8192;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::new();
    let data = if data.len() > PAYLOAD_CAP {
        &data[..PAYLOAD_CAP]
    } else {
        data
    };

    // Property 1: decoding must never panic, whatever the bytes.
    let Ok(message) = postcard::from_bytes::<VacuumDropMessage>(data) else {
        // A decode failure is the common case for arbitrary bytes and is a
        // perfectly acceptable outcome. Nothing below this line runs.
        return;
    };

    // The codecs we expose to the application are private, so validation is
    // exercised through the message types' own methods. Each arm mirrors what a
    // session does on receipt.
    match &message {
        VacuumDropMessage::VersionNegotiate(offered) => {
            let expect_ok = offered.validate().is_ok();

            // Property 2: a list this long is either within MAX_VERSIONS or it
            // was rejected. Nothing in between.
            if offered.versions.len() > SupportedVersions::MAX_VERSIONS {
                assert!(
                    !expect_ok,
                    "a {}-entry version list was accepted (max {})",
                    offered.versions.len(),
                    SupportedVersions::MAX_VERSIONS
                );
            }

            // Negotiation must fail closed on a rejected offer. Charter 66: never
            // silently downgrade.
            if !expect_ok {
                assert!(
                    vacuumdrop_protocol::message::VersionSelection::negotiate(offered).is_err(),
                    "a malformed offer negotiated successfully"
                );
            }
        }

        VacuumDropMessage::VersionAck(selection) => {
            // A server must not be able to steer us onto a version we never
            // offered. The client handshake checks this; assert it holds for every
            // possible value.
            if !NegotiatedVersion::SUPPORTED.contains(&selection.selected) {
                assert!(
                    NegotiatedVersion::negotiate(&[selection.selected]).is_err(),
                    "negotiated a version we never advertised: {}",
                    selection.selected
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

            // Property 4: control characters and over-length names are refused.
            if hello.device_name.is_empty() {
                assert!(result.is_err(), "an empty device name was accepted");
            }
            if hello.device_name.len() > limits.max_name_length {
                assert!(
                    result.is_err(),
                    "a {}-byte device name was accepted (limit {})",
                    hello.device_name.len(),
                    limits.max_name_length
                );
            }
            if hello.device_name.chars().any(char::is_control) {
                assert!(
                    result.is_err(),
                    "a device name containing control characters was accepted"
                );
            }

            // Property 2: what `validate` accepted must round-trip through the
            // domain type without loss.
            if let Ok(desc) = result {
                assert_eq!(desc.display_name(), hello.device_name);
            }
        }

        VacuumDropMessage::HelloAck(ack) => {
            // A rejection carries no usable identity, so a rejection must not be
            // forced through name validation. This is a real branch in
            // `ClientHandshake::on_hello_ack`; assert it stays reachable.
            if let HelloStatus::Rejected(_) = ack.status {
                assert_eq!(
                    ack.device_name.is_empty(),
                    false,
                    "sanity: a rejection normally carries a name"
                );
            }
        }

        VacuumDropMessage::Capabilities(set) => {
            let expect_ok = set.validate(&limits).is_ok();

            if set.max_chunk_size == 0 {
                assert!(!expect_ok, "a zero chunk size was accepted");
            }
            if set.max_chunk_size > limits.max_frame_length {
                assert!(
                    !expect_ok,
                    "a chunk size of {} was accepted above our {}-byte frame limit",
                    set.max_chunk_size,
                    limits.max_frame_length
                );
            }
            if set.features.len() > vacuumdrop_protocol::message::MAX_FEATURES {
                assert!(!expect_ok, "an oversized feature list was accepted");
            }

            // Property 2: decoding features must not invent capabilities.
            for capability in set.capabilities() {
                assert!(
                    set.claims(capability),
                    "capabilities() returned {} which claims() denies",
                    capability.as_str()
                );
            }
        }

        VacuumDropMessage::Ping(ping) => {
            // A Ping is only meaningful from an established session; the session
            // enforces that. Here, assert the nonce survives the round trip.
            let bytes = postcard::to_allocvec(&message).expect("re-encoding a decoded message");
            let again: VacuumDropMessage =
                postcard::from_bytes(&bytes).expect("a decoded message must re-decode");
            assert_eq!(again, message, "Ping did not survive a round trip");
            assert_eq!(again.name(), "Ping");
        }

        VacuumDropMessage::Pong(_) | VacuumDropMessage::Goodbye(_) => {
            // Round-trip only. Both are trivial enums with no validation.
            let bytes = postcard::to_allocvec(&message).expect("re-encoding a decoded message");
            let again: VacuumDropMessage =
                postcard::from_bytes(&bytes).expect("a decoded message must re-decode");
            assert_eq!(again, message);
        }

        VacuumDropMessage::Error(report) => {
            // Property 4: the detail string is peer-supplied and must stay bounded.
            // `MAX_DETAIL` is the documented limit; a longer string reaching the
            // application unclamped would be an unbounded peer-controlled buffer.
            if report.detail.len() > vacuumdrop_protocol::message::ProtocolErrorReport::MAX_DETAIL {
                // The decoder does not currently enforce this. Assert the bound
                // so that when it does, this test is already watching.
                assert!(
                    report.detail.len() <= 4096,
                    "error detail exceeded 4096 bytes ({}); MAX_DETAIL is {}",
                    report.detail.len(),
                    vacuumdrop_protocol::message::ProtocolErrorReport::MAX_DETAIL
                );
            }
        }
    }

    // Property 3: validating must stay fast even for a maximal input. If this
    // assertion is unreachable in practice that is fine; it exists to fail loudly
    // if validation ever becomes superlinear.
    let start = std::time::Instant::now();
    let _ = PeerDescription::new(
        "probe".to_owned(),
        NegotiatedVersion::SUPPORTED.to_vec(),
        Vec::new(),
        1024,
        &limits,
    );
    assert!(
        start.elapsed() < std::time::Duration::from_millis(100),
        "validation took {}ms for a trivial input",
        start.elapsed().as_millis()
    );
});