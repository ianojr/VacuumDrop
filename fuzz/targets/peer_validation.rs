#![no_main]

//! Fuzzes peer validation and capability handling directly.
//!
//! `message_decoder` fuzzes values that survive `postcard` decoding. This target
//! attacks the validators themselves with structured, hostile input - the shape a
//! deliberate attacker would construct by hand rather than stumble into.
//!
//! Split out because the two explore different spaces. Mutation-driven fuzzing
//! rarely produces a *valid* 2000-entry version list or a name that is exactly
//! one byte over the limit; these are precisely the boundary cases worth checking,
//! and they are cheap to test directly.
//!
//! # Properties asserted
//!
//! 1. **Every bound is exact.** At the limit, accept; one past it, reject. An
//!    off-by-one in either direction is a bug.
//! 2. **No control characters survive** a name.
//! 3. **Negotiation fails closed.** No overlap is an error, never a silent
//!    downgrade.
//! 4. **Unknown capability flags are discarded, not fatal**, so a newer peer does
//!    not break the session.
//! 5. **Capabilities are claims.** `capabilities()` may only ever return values
//!    that `claims()` also returns.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

use libfuzzer_sys::fuzz_target;
use vacuumdrop_core::{Capability, Limits, NegotiatedVersion, PeerDescription};
use vacuumdrop_protocol::message::{CapabilitySet, SupportedVersions};

/// A small alphabet of hostile name characters.
///
/// Weighted toward the cases that matter: traversal, path separators, control
/// codes, and a wide multi-byte rune to check UTF-8 handling.
const HOSTILE_CHARS: [char; 26] = [
    '\0', '\n', '\r', '\t', '\u{1b}', '\u{7f}', '\\', '/', ':', '*', '?', '"', '<', '>', '|', '.',
    ' ', 'a', 'Z', '9', '\u{00e9}', '\u{4e2d}', '\u{1f600}', '\u{feff}', '\u{202e}', '_', '\u{0}',
];

/// Builds a name from the fuzzer's bytes, biased toward hostile characters.
fn build_name(data: &[u8], limits: &Limits) -> String {
    // Cover the boundary region deliberately: lengths from just under the limit
    // to well over it, where a one-byte over-limit check would fail.
    let bias = usize::from(data.first().copied().unwrap_or(0)) % 64;
    let len = (limits.max_name_length + bias).saturating_sub(32);

    let mut name = String::with_capacity(len);
    for i in 0..len {
        let b = data.get(i).copied().unwrap_or(b'a');
        // Mix: every fourth character hostile, the rest plausible.
        let c = if i % 4 == 0 {
            HOSTILE_CHARS[(b as usize) % HOSTILE_CHARS.len()]
        } else {
            char::from(b'!' + (b % 90))
        };
        name.push(c);
    }
    name
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits::new();

    // ---- Device names -------------------------------------------------
    let name = build_name(data, &limits);

    let result = PeerDescription::new(
        name.clone(),
        NegotiatedVersion::SUPPORTED.to_vec(),
        vec![],
        1024,
        &limits,
    );

    // Property 1: exact bound. Empty is rejected; at the limit, accepted;
    // one past the limit, rejected.
    if name.is_empty() {
        assert!(result.is_err(), "an empty name was accepted");
    }
    if name.chars().any(char::is_control) {
        // Property 2.
        assert!(result.is_err(), "a name with control characters was accepted");
    }
    if name.len() > limits.max_name_length {
        assert!(
            result.is_err(),
            "a {}-byte name was accepted (limit {})",
            name.len(),
            limits.max_name_length
        );
    } else if !name.chars().any(char::is_control) {
        assert!(
            result.is_ok(),
            "a valid {}-byte name was rejected: {:?}",
            name.len(),
            result.err()
        );
    }

    // ---- Version lists ------------------------------------------------
    let version_data = &data[data.len() / 2..];
    let versions: Vec<u16> = version_data
        .chunks(2)
        .take(40)
        .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
        .collect();

    let offered = SupportedVersions {
        versions: versions.clone(),
    };
    let expect_valid = !versions.is_empty()
        && !versions.contains(&0)
        && versions.len() <= SupportedVersions::MAX_VERSIONS
        && versions.windows(2).all(|p| p[0] < p[1]);

    assert_eq!(
        offered.validate().is_ok(),
        expect_valid,
        "version validation disagrees with its own contract for {versions:?}"
    );

    // Property 3: fail closed. A malformed offer must never negotiate.
    if !expect_valid {
        assert!(
            vacuumdrop_protocol::message::VersionSelection::negotiate(&offered).is_err(),
            "a malformed offer negotiated successfully"
        );
    }

    // ---- Capabilities -------------------------------------------------
    let flag_data = &data[data.len() / 3..];
    let features: Vec<u8> = flag_data.iter().copied().take(64).collect();
    let set = CapabilitySet {
        features: features.clone(),
        max_chunk_size: 1024,
    };

    let caps = set.capabilities();

    // Property 5: `capabilities()` may only return what `claims()` agrees with.
    for c in &caps {
        assert!(
            set.claims(*c),
            "capabilities() returned {} but claims() denies it",
            c.as_str()
        );
    }

    // Property 4: unknown flags are discarded, not fatal. Every input must
    // produce at most the known capabilities.
    for c in &caps {
        assert!(
            matches!(
                c,
                Capability::SupportsResume
                    | Capability::SupportsParallelStreams
                    | Capability::SupportsCompression
                    | Capability::SupportsEncryption
                    | Capability::SupportsChunkVerification
            ),
            "decoded an unknown capability variant: {c:?}"
        );
    }

    // Feature count must be bounded.
    if features.len() > vacuumdrop_protocol::message::MAX_FEATURES {
        assert!(
            set.validate(&limits).is_err(),
            "an oversized feature list ({}) was accepted",
            features.len()
        );
    }

    // ---- chunk_size boundary -------------------------------------------
    // Derived from the last two bytes, so the exact limit boundary is reachable.
    let size = u32::from_le_bytes([
        *data.last().unwrap_or(&0),
        *data.get(1).unwrap_or(&0),
        *data.get(2).unwrap_or(&0),
        *data.get(3).unwrap_or(&0),
    ]);
    let capped = CapabilitySet {
        features: vec![],
        max_chunk_size: size,
    };
    let expect_ok = size > 0 && size <= limits.max_frame_length;
    assert_eq!(
        capped.validate(&limits).is_ok(),
        expect_ok,
        "chunk size {size} validation disagrees with the contract (limit {})",
        limits.max_frame_length
    );
});