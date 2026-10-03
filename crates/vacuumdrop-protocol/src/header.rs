//! Frame header: the fixed-layout prefix on every message.
//!
//! Charter 10 requires each message to carry a protocol version, type, length,
//! session id, and request id, and forbids depending on implicit message sizes.
//! This module is that prefix.
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! |                    Magic (u32)                                |
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! |  Version (u16)  |    Flags (u16)   |   Message Type (u16)   |
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! |                     Payload Length (u32)                      |
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! |                     Session ID (u64)                          |
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! |                     Request ID (u64)                          |
//! +-------+-------+-------+-------+-------+-------+-------+-------+
//! ```
//!
//! Header size: 38 bytes, fixed, little-endian.
//!
//! # Security note
//!
//! [`FrameHeader::decode`] validates `payload_len` against the caller's limit
//! **before** the caller allocates anything proportional to it. This is the
//! primary defence against a length-prefix memory exhaustion attack
//! ([`docs/threat-model.md`](../docs/threat-model.md) T-01). The order of the
//! checks in `decode` is load-bearing; do not reorder it.
//!
//! Endianness is fixed to little-endian so a deployment cannot produce a
//! different byte order on a big-endian target, which would make captured
//! transcripts unreproducible.

use vacuumdrop_core::{Error, InvalidMessageError, ProtocolError};

/// Bytes in a frame header.
pub const HEADER_LEN: usize = 38;

/// The magic as it appears on the wire: ASCII `VDDR`.
///
/// Written as bytes rather than derived from a `u32` so the encoding is
/// unambiguous. `0x5644_4452u32.to_le_bytes()` would put `RDDV` on the wire,
/// which is confusing to read in a packet capture.
pub const MAGIC_BYTES: [u8; 4] = *b"VDDR";

/// Marks a frame as VacuumDrop, read as a little-endian `u32`.
///
/// Equal to `u32::from_le_bytes(MAGIC_BYTES)`. A distinctive magic is
/// deliberate: it lets a non-VacuumDrop client, a port scan, or a misrouted
/// connection be identified instantly instead of being misdiagnosed as
/// corruption.
pub const MAGIC: u32 = u32::from_le_bytes(MAGIC_BYTES);

/// Flags valid in protocol version 1.
pub const KNOWN_FLAGS: u16 = 0;

/// The only message type protocol version 1 emits.
///
/// Version 1 sends every message as a self-describing `serde` enum, so the type
/// field carries no dispatch information. It is still **validated on receipt**:
/// an unrecognised value means the peer is using a framing convention we do not
/// implement, and decoding the payload regardless would be a guess.
///
/// This is the value [`FrameHeader::encode`] writes, replacing a constant that
/// lived in the codec. Keeping it beside the field it populates means the writer
/// and the validator cannot drift apart.
pub const ENUM_PAYLOAD: u16 = 0;

/// A decoded frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Protocol version in use.
    pub version: u16,
    /// Header flags. Reserved bits must be zero in version 1.
    pub flags: u16,
    /// Session this frame belongs to.
    pub session_id: u64,
    /// Request this frame belongs to, for request/response pairing.
    pub request_id: u64,
}

impl FrameHeader {
    /// Builds a header for encoding.
    pub const fn new(version: u16, session_id: u64, request_id: u64) -> Self {
        Self {
            version,
            flags: 0,
            session_id,
            request_id,
        }
    }

    /// Serialises the header into a fixed 38-byte buffer.
    ///
    /// Writes into `out` rather than allocating: this runs once per frame on
    /// the hot path.
    pub fn encode(&self, message_type: u16, payload_len: u32, out: &mut [u8; HEADER_LEN]) {
        out[0..4].copy_from_slice(&MAGIC_BYTES);
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..10].copy_from_slice(&message_type.to_le_bytes());
        out[10..14].copy_from_slice(&payload_len.to_le_bytes());
        out[14..22].copy_from_slice(&self.session_id.to_le_bytes());
        out[22..30].copy_from_slice(&self.request_id.to_le_bytes());
        // Bytes 30..38 are reserved and always zero, which keeps the header a
        // fixed size now and leaves room to add a field later without
        // breaking existing readers' offset arithmetic.
        out[30..HEADER_LEN].fill(0);
    }

    /// Decodes and validates a header.
    ///
    /// `bytes` must be exactly [`HEADER_LEN`] long.
    ///
    /// # Validation order
    ///
    /// 1. Length, so a short buffer is rejected before any field access.
    /// 2. Magic, so a non-VacuumDrop peer is identified as such.
    /// 3. Flags, so an unknown-flag frame is refused before its length is used.
    /// 4. `payload_len` against `max_frame_length` -- **before** the caller
    ///    allocates anything sized from it.
    ///
    /// Steps 1-3 deliberately precede step 4. Reading the length first is
    /// harmless, but *trusting* it before validating magic and flags would let
    /// arbitrary bytes from a non-peer drive our allocation size.
    pub fn decode(bytes: &[u8], max_frame_length: u32) -> Result<(Self, u16, u32), Error> {
        if bytes.len() != HEADER_LEN {
            return Err(InvalidMessageError::Truncated {
                need: HEADER_LEN,
                have: bytes.len(),
            }
            .into());
        }

        if bytes[0..4] != MAGIC_BYTES {
            return Err(ProtocolError::BadMagic.into());
        }

        let version = u16::from_le_bytes(bytes[4..6].try_into().expect("slice is 2 bytes"));
        let flags = u16::from_le_bytes(bytes[6..8].try_into().expect("slice is 2 bytes"));
        if flags & !KNOWN_FLAGS != 0 {
            // Reserved bits set means a newer writer expects semantics we do
            // not implement. Continuing would mean interpreting a payload
            // according to rules the sender did not use.
            return Err(ProtocolError::UnknownFlags(flags).into());
        }

        let message_type = u16::from_le_bytes(bytes[8..10].try_into().expect("slice is 2 bytes"));
        if message_type != ENUM_PAYLOAD {
            // Charter 32: validate every value arriving from the wire. An
            // unrecognised type means the sender used a framing convention we do
            // not implement, and interpreting the payload anyway would be a
            // guess about its meaning. Refusing is the only safe response.
            return Err(ProtocolError::UnknownMessageType(message_type).into());
        }

        let payload_len = u32::from_le_bytes(bytes[10..14].try_into().expect("slice is 4 bytes"));

        // The critical check. Must happen before the caller resizes a buffer.
        if payload_len > max_frame_length {
            return Err(ProtocolError::PayloadTooLarge {
                declared: payload_len as u64,
                limit: max_frame_length as u64,
            }
            .into());
        }

        let session_id = u64::from_le_bytes(bytes[14..22].try_into().expect("slice is 8 bytes"));
        let request_id = u64::from_le_bytes(bytes[22..30].try_into().expect("slice is 8 bytes"));

        Ok((
            Self {
                version,
                flags,
                session_id,
                request_id,
            },
            message_type,
            payload_len,
        ))
    }

    /// Whether reserved bytes were left non-zero by a writer.
    ///
    /// Used by tests to confirm the encoder keeps them clear. Not checked on
    /// decode: reserved bytes are defined as ignored, so tolerating garbage
    /// there is what allows a future writer to use them.
    pub fn reserved_bytes_are_zero(bytes: &[u8]) -> bool {
        bytes[30..HEADER_LEN].iter().all(|b| *b == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(h: &FrameHeader, msg_type: u16, len: u32) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        h.encode(msg_type, len, &mut buf);
        buf
    }

    fn default_header() -> FrameHeader {
        FrameHeader::new(1, 0xABCD_1234_5678_9ABC, 0x1111_2222_3333_4444)
    }

    #[test]
    fn header_is_38_bytes() {
        assert_eq!(HEADER_LEN, 38);
    }

    #[test]
    fn magic_is_ascii_vddr_on_the_wire() {
        assert_eq!(&MAGIC_BYTES, b"VDDR");
        assert_eq!(MAGIC, u32::from_le_bytes(*b"VDDR"));
    }

    #[test]
    fn round_trips_all_fields() {
        let h = default_header();
        let bytes = encode(&h, ENUM_PAYLOAD, 1234);
        let (back, msg_type, len) = FrameHeader::decode(&bytes, u32::MAX).unwrap();

        assert_eq!(back, h);
        assert_eq!(msg_type, ENUM_PAYLOAD);
        assert_eq!(len, 1234);
    }

    #[test]
    fn encoding_is_little_endian_and_stable() {
        // Golden bytes. A change here is a wire-format change needing ADR-0007.
        let bytes = encode(&FrameHeader::new(1, 1, 2), ENUM_PAYLOAD, 4);
        assert_eq!(&bytes[0..4], b"VDDR");
        assert_eq!(bytes[4], 1, "version low byte");
        assert_eq!(bytes[8], ENUM_PAYLOAD as u8, "message type low byte");
        assert_eq!(bytes[10], 4, "payload length low byte");
        assert_eq!(bytes[14], 1, "session id low byte");
        assert_eq!(bytes[22], 2, "request id low byte");
    }

    #[test]
    fn encoder_zeroes_reserved_bytes() {
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        assert!(FrameHeader::reserved_bytes_are_zero(&bytes));
    }

    #[test]
    fn decoder_rejects_short_buffer() {
        for len in [0, 1, 10, HEADER_LEN - 1] {
            let bytes = vec![0u8; len];
            assert!(
                FrameHeader::decode(&bytes, u32::MAX).is_err(),
                "accepted a {len}-byte header"
            );
        }
    }

    #[test]
    fn decoder_rejects_bad_magic() {
        let mut bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        bytes[0] = b'X';
        assert!(matches!(
            FrameHeader::decode(&bytes, u32::MAX),
            Err(Error::Protocol(ProtocolError::BadMagic))
        ));
    }

    #[test]
    fn decoder_rejects_unknown_flags() {
        // A newer writer setting a reserved bit must be refused, not guessed at.
        let mut bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        bytes[6] = 0x01;
        assert!(matches!(
            FrameHeader::decode(&bytes, u32::MAX),
            Err(Error::Protocol(ProtocolError::UnknownFlags(f))) if f == 1
        ));
    }

    #[test]
    fn decoder_rejects_all_flags_bit() {
        let mut bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        bytes[6] = 0xFF;
        bytes[7] = 0xFF;
        assert!(FrameHeader::decode(&bytes, u32::MAX).is_err());
    }

    #[test]
    fn oversized_payload_is_rejected_before_allocation() {
        // The core security property: a declared length above the limit must be
        // refused without the caller ever sizing a buffer from it.
        let limit = 1024u32;
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 4_294_967_295);
        match FrameHeader::decode(&bytes, limit) {
            Err(Error::Protocol(ProtocolError::PayloadTooLarge { declared, limit: l })) => {
                assert_eq!(declared, 4_294_967_295);
                assert_eq!(l, 1024);
            }
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn payload_exactly_at_limit_is_accepted() {
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 1024);
        assert!(FrameHeader::decode(&bytes, 1024).is_ok());
    }

    #[test]
    fn payload_one_over_limit_is_rejected() {
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 1025);
        assert!(matches!(
            FrameHeader::decode(&bytes, 1024),
            Err(Error::Protocol(ProtocolError::PayloadTooLarge { .. }))
        ));
    }

    #[test]
    fn bad_magic_takes_priority_over_oversized_length() {
        // Arbitrary bytes from a non-peer must never drive our allocation size,
        // so the magic check must come first.
        let mut bytes = encode(&default_header(), ENUM_PAYLOAD, 4_294_967_295);
        bytes[0] = 0;
        assert!(matches!(
            FrameHeader::decode(&bytes, 1024),
            Err(Error::Protocol(ProtocolError::BadMagic))
        ));
    }

    #[test]
    fn unknown_flags_take_priority_over_oversized_length() {
        let mut bytes = encode(&default_header(), ENUM_PAYLOAD, 4_294_967_295);
        bytes[6] = 0x80;
        assert!(matches!(
            FrameHeader::decode(&bytes, 1024),
            Err(Error::Protocol(ProtocolError::UnknownFlags(_)))
        ));
    }

    #[test]
    fn arbitrary_garbage_never_panics() {
        // The parser is security-sensitive and fuzzed. Every byte pattern in a
        // small window must return an error, never panic.
        for a in 0u8..=255 {
            for b in [0u8, 1, 0xFF] {
                let mut bytes = [0u8; HEADER_LEN];
                bytes[0] = a;
                bytes[6] = b;
                bytes[10] = 0xFF;
                bytes[11] = 0xFF;
                bytes[12] = 0xFF;
                bytes[13] = 0xFF;
                let _ = FrameHeader::decode(&bytes, 32 * 1024 * 1024);
            }
        }
    }

    #[test]
    fn unknown_message_type_is_refused() {
        // An unrecognised type means the peer is using a framing convention we
        // do not implement. Decoding the payload anyway would be a guess.
        for bad in [1u16, 2, 0x8000, u16::MAX] {
            let mut bytes = encode(&default_header(), bad, 0);
            bytes[8..10].copy_from_slice(&bad.to_le_bytes());
            assert!(
                matches!(
                    FrameHeader::decode(&bytes, u32::MAX),
                    Err(Error::Protocol(ProtocolError::UnknownMessageType(t))) if t == bad
                ),
                "accepted message type {bad}"
            );
        }
    }

    #[test]
    fn the_written_message_type_is_the_one_accepted() {
        // Guards against the writer and the validator drifting apart.
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        assert_eq!(&bytes[8..10], &ENUM_PAYLOAD.to_le_bytes());
        assert!(FrameHeader::decode(&bytes, u32::MAX).is_ok());
    }

    #[test]
    fn zero_length_payload_is_legal_at_the_header_level() {
        // Whether a zero-length payload is meaningful is a message-layer
        // question; the header must not second-guess it.
        let bytes = encode(&default_header(), ENUM_PAYLOAD, 0);
        let (_, _, len) = FrameHeader::decode(&bytes, 1024).unwrap();
        assert_eq!(len, 0);
    }
}
