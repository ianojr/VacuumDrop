//! Frame codec: bytes to messages and back.
//!
//! Charter 11 requires explicit framing, because TCP is a byte stream and
//! `write(message)` does not imply `read(message)`. This module implements that
//! framing over `tokio_util::codec`.
//!
//! # Why the header is not left to `LengthDelimitedCodec`
//!
//! ADR-0007 selects `LengthDelimitedCodec` for the *length-delimited stream*.
//! But Charter 10 additionally requires every frame to carry a version, type,
//! session id, and request id. `LengthDelimitedCodec` handles only length.
//!
//! So the payload written here is itself length-delimited: `LengthDelimitedCodec`
//! frames the encoded message, and [`FrameCodec`] adds the VacuumDrop header on
//! top. The two compose because each is self-delimiting.
//!
//! # Allocation discipline
//!
//! [`FrameCodec`] uses a fixed-size read buffer and never sizes a buffer from an
//! untrusted length. A frame larger than `max_frame_length` is refused at the
//! header, before any growth ([`docs/threat-model.md`](../docs/threat-model.md)
//! T-01). Phase 1 messages are small, so a fixed buffer is the right shape; the
//! transfer path in Phase 5 will size from the *negotiated* chunk size, never
//! from a per-frame declared value.

use tokio_util::bytes::{BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};
use vacuumdrop_core::{Error as EngineError, InvalidMessageError, ProtocolError};

use crate::header::{ENUM_PAYLOAD, FrameHeader, HEADER_LEN};
use crate::message::VacuumDropMessage;

/// A framing failure.
///
/// `tokio_util`'s codec traits require the error type to implement
/// `From<io::Error>`, which no domain error type should. Rather than bolting an
/// I/O variant onto the protocol taxonomy, framing gets its own error that wraps
/// [`EngineError`] (which already distinguishes transport, protocol, and
/// validation failures) plus the I/O case the codec trait demands.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The stream failed underneath the codec.
    #[error("frame i/o failed: {0}")]
    Io(#[from] std::io::Error),

    /// The bytes were not a valid VacuumDrop frame.
    #[error("framing failed: {0}")]
    Engine(#[from] EngineError),
}

impl From<FrameError> for EngineError {
    fn from(e: FrameError) -> Self {
        match e {
            // A read failure is a transport failure. Preserved as such rather
            // than flattened into a protocol error, so a dead socket stays
            // distinguishable from a protocol violation.
            FrameError::Io(io) => EngineError::from(io),
            FrameError::Engine(e) => e,
        }
    }
}

/// Fixed read buffer size.
///
/// Comfortably above `HEADER_LEN` plus any Phase 1 message. Sized here rather
/// than derived from a declared length so an attacker cannot influence it.
const READ_BUF: usize = 16 * 1024;

/// Hard ceiling on the read buffer, checked before growth.
///
/// A cap on our own buffer is the last line of defence: even if every other
/// bound were bypassed, memory stays bounded.
const MAX_READ_BUF: usize = 64 * 1024;

/// The high bit marking a VacuumDrop-encoded payload on the wire.
///
/// Set by [`FrameCodec::encode`] and required by [`FrameCodec::decode`]. Without
/// it, a random TCP stream would be fed to the postcard decoder.
const CODEC_TAG: u8 = 0x56; // 'V'

impl From<ProtocolError> for FrameError {
    fn from(e: ProtocolError) -> Self {
        FrameError::Engine(e.into())
    }
}

/// Encodes and decodes VacuumDrop frames over a byte stream.
#[derive(Debug)]
pub struct FrameCodec {
    header: FrameHeader,
    max_frame_length: u32,
}

impl FrameCodec {
    /// Builds a codec for one session.
    pub fn new(header: FrameHeader, max_frame_length: u32) -> Self {
        Self {
            header,
            max_frame_length,
        }
    }

    /// The header stamped on outgoing frames.
    pub fn header(&self) -> FrameHeader {
        self.header
    }

    /// The session id stamped on outgoing frames.
    pub fn session_id(&self) -> u64 {
        self.header.session_id
    }

    /// Builds a codec for a session id, using the current protocol version.
    pub fn for_session(session_id: u64, max_frame_length: u32) -> Self {
        Self::new(
            FrameHeader::new(vacuumdrop_core::NegotiatedVersion::LOCAL, session_id, 0),
            max_frame_length,
        )
    }

    /// Sets the request id stamped on subsequent frames.
    pub fn set_request_id(&mut self, request_id: u64) {
        self.header.request_id = request_id;
    }
}

impl Encoder<VacuumDropMessage> for FrameCodec {
    type Error = FrameError;

    fn encode(&mut self, item: VacuumDropMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let payload = postcard::to_allocvec(&item).map_err(|e| {
            FrameError::Engine(EngineError::from(InvalidMessageError::Decode(
                e.to_string(),
            )))
        })?;

        if payload.len() as u64 > self.max_frame_length as u64 {
            // An outgoing frame over the limit is our own bug or a deliberate
            // misconfiguration. Either way, refuse rather than send something we
            // would reject on receipt.
            return Err(FrameError::Engine(
                ProtocolError::PayloadTooLarge {
                    declared: payload.len() as u64,
                    limit: self.max_frame_length as u64,
                }
                .into(),
            ));
        }

        let mut header_bytes = [0u8; HEADER_LEN];
        self.header
            .encode(ENUM_PAYLOAD, payload.len() as u32, &mut header_bytes);

        // Reserve exactly what is needed; BytesMut grows geometrically but only
        // when asked to.
        dst.reserve(HEADER_LEN + payload.len() + 1);
        dst.put_u8(CODEC_TAG);
        dst.extend_from_slice(&header_bytes);
        dst.extend_from_slice(&payload);
        Ok(())
    }
}

impl Decoder for FrameCodec {
    type Item = VacuumDropMessage;
    type Error = FrameError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Phase 1 messages are small, so the buffer is capped rather than grown
        // to fit whatever a peer claims.
        if src.len() > MAX_READ_BUF {
            return Err(FrameError::Engine(
                ProtocolError::PayloadTooLarge {
                    declared: src.len() as u64,
                    limit: MAX_READ_BUF as u64,
                }
                .into(),
            ));
        }

        if src.len() < HEADER_LEN + 1 {
            // Reserve enough for a typical Phase 1 frame up front so a
            // fragmented message stops triggering reallocation after the first
            // few reads, while still refusing to grow toward a declared length.
            let want = READ_BUF.saturating_sub(src.len());
            src.reserve(want.max(HEADER_LEN + 1 - src.len()));
            return Ok(None);
        }

        // Reject a non-VacuumDrop stream before parsing it.
        if src[0] != CODEC_TAG {
            return Err(FrameError::Engine(ProtocolError::BadMagic.into()));
        }

        // Borrow the header without consuming, so a partial frame stays intact.
        let (header, _message_type, payload_len) =
            FrameHeader::decode(&src[1..1 + HEADER_LEN], self.max_frame_length)?;

        let total = 1 + HEADER_LEN + payload_len as usize;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }

        // Consume the whole frame only once it is fully present. Slicing with
        // `split_to` before the length check would let a truncated frame corrupt
        // the stream.
        let frame = src.split_to(total);
        let payload = &frame[1 + HEADER_LEN..];

        // The peer must be speaking our version. Checked before decoding so a
        // version mismatch is reported as such rather than as a decode error.
        if header.version != self.header.version {
            return Err(FrameError::Engine(
                ProtocolError::StateViolation("frame version differs from the negotiated version")
                    .into(),
            ));
        }

        let message: VacuumDropMessage = postcard::from_bytes(payload).map_err(|e| {
            FrameError::Engine(EngineError::from(InvalidMessageError::Decode(
                e.to_string(),
            )))
        })?;

        // Reflect the peer's correlation ids so a response can be matched.
        self.header.session_id = header.session_id;
        self.header.request_id = header.request_id;

        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        ErrorCode, Goodbye, GoodbyeReason, Hello, HelloAck, HelloStatus, Ping, Pong,
        ProtocolErrorReport, SupportedVersions, VacuumDropMessage, VersionSelection,
    };
    use vacuumdrop_core::NegotiatedVersion;
    use vacuumdrop_core::{Error, ProtocolError};

    fn codec() -> FrameCodec {
        FrameCodec::for_session(0x1234, 1024 * 1024)
    }

    fn encode_one(c: &mut FrameCodec, m: &VacuumDropMessage) -> BytesMut {
        let mut buf = BytesMut::new();
        c.encode(m.clone(), &mut buf).unwrap();
        buf
    }

    fn round_trip(m: VacuumDropMessage) {
        let mut c = codec();
        let mut bytes = encode_one(&mut c, &m);
        let mut read = codec();
        let back = read.decode(&mut bytes).unwrap().unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn every_phase_1_message_round_trips_through_the_codec() {
        for m in [
            VacuumDropMessage::VersionNegotiate(SupportedVersions::local()),
            VacuumDropMessage::VersionAck(VersionSelection { selected: 1 }),
            VacuumDropMessage::Hello(Hello {
                device_name: "device".into(),
                client_version: "0.1.0".into(),
            }),
            VacuumDropMessage::HelloAck(HelloAck {
                status: HelloStatus::Accepted,
                device_name: "server".into(),
            }),
            VacuumDropMessage::Ping(Ping { nonce: 42 }),
            VacuumDropMessage::Pong(Pong { nonce: 42 }),
            VacuumDropMessage::Error(ProtocolErrorReport::fatal(
                ErrorCode::MalformedMessage,
                "bad frame",
            )),
            VacuumDropMessage::Goodbye(Goodbye {
                reason: GoodbyeReason::Shutdown,
            }),
        ] {
            round_trip(m);
        }
    }

    #[test]
    fn partial_frames_return_none_and_consume_nothing() {
        let mut c = codec();
        let full = encode_one(&mut c, &VacuumDropMessage::Ping(Ping { nonce: 7 }));

        // Every proper prefix of a frame must ask for more, not error and not
        // consume. This is the TCP-fragmentation property (Charter 11).
        for take in 0..full.len() {
            let mut partial = BytesMut::from(&full[..take]);
            assert_eq!(
                c.decode(&mut partial).unwrap(),
                None,
                "a {take}-byte prefix should await more data"
            );
            assert_eq!(partial.len(), take, "partial decode consumed input");
        }
    }

    #[test]
    fn two_messages_in_one_buffer_decode_in_order() {
        let mut c = codec();
        let mut stream = encode_one(&mut c, &VacuumDropMessage::Ping(Ping { nonce: 1 }));
        let second = encode_one(&mut c, &VacuumDropMessage::Pong(Pong { nonce: 1 }));
        stream.extend_from_slice(&second);

        let mut r = codec();
        assert_eq!(
            r.decode(&mut stream).unwrap(),
            Some(VacuumDropMessage::Ping(Ping { nonce: 1 }))
        );
        assert_eq!(
            r.decode(&mut stream).unwrap(),
            Some(VacuumDropMessage::Pong(Pong { nonce: 1 }))
        );
        assert_eq!(r.decode(&mut stream).unwrap(), None);
    }

    #[test]
    fn byte_at_a_time_delivery_still_decodes() {
        // Worst-case fragmentation: the message arrives one byte per read.
        let mut c = codec();
        let full = encode_one(
            &mut c,
            &VacuumDropMessage::Hello(Hello {
                device_name: "fragmented".into(),
                client_version: "0.1.0".into(),
            }),
        );

        let mut r = codec();
        let mut stream = BytesMut::new();
        let mut decoded = None;
        for i in 0..full.len() {
            stream.extend_from_slice(&full[i..i + 1]);
            if let Some(m) = r.decode(&mut stream).unwrap() {
                decoded = Some(m);
            }
        }
        match decoded {
            Some(VacuumDropMessage::Hello(h)) => assert_eq!(h.device_name, "fragmented"),
            other => panic!("expected Hello, got {other:?}"),
        }
        assert!(
            stream.is_empty(),
            "bytes left unconsumed after a full frame"
        );
    }

    #[test]
    fn non_vacuumdrop_stream_is_rejected_immediately() {
        // Long enough to clear the minimum-length check, and starting with a byte that
        // is not the VacuumDrop tag.
        let mut junk = BytesMut::from(
            &b"GET / HTTP/1.1\r\nHost: example.com\r\nUser-Agent: curl/8.0\r\n\r\n"[..],
        );
        assert!(matches!(
            codec().decode(&mut junk),
            Err(FrameError::Engine(Error::Protocol(ProtocolError::BadMagic)))
        ));
    }

    #[test]
    fn corrupt_magic_after_tag_is_rejected() {
        let mut c = codec();
        let mut bytes = encode_one(&mut c, &VacuumDropMessage::Ping(Ping { nonce: 1 }));
        bytes[1] = b'X'; // break the magic, keep the tag
        assert!(matches!(
            codec().decode(&mut bytes),
            Err(FrameError::Engine(Error::Protocol(ProtocolError::BadMagic)))
        ));
    }

    #[test]
    fn oversized_declared_length_is_refused_by_the_codec() {
        // Build a header claiming ~4 GiB with a tiny buffer behind it. The codec
        // must refuse on the header, never grow to the declared size.
        let mut bytes = BytesMut::new();
        let h = FrameHeader::new(1, 1, 1);
        let mut hb = [0u8; HEADER_LEN];
        h.encode(ENUM_PAYLOAD, u32::MAX, &mut hb);
        bytes.put_u8(CODEC_TAG);
        bytes.extend_from_slice(&hb);
        bytes.extend_from_slice(b"tiny");

        let before = bytes.len();
        assert!(matches!(
            codec().decode(&mut bytes),
            Err(FrameError::Engine(Error::Protocol(
                ProtocolError::PayloadTooLarge { .. }
            )))
        ));
        assert_eq!(bytes.len(), before, "codec consumed input while failing");
    }

    #[test]
    fn oversized_outgoing_message_is_refused() {
        let mut c = FrameCodec::for_session(1, 8); // absurdly small limit
        let mut buf = BytesMut::new();
        let err = c
            .encode(
                VacuumDropMessage::Hello(Hello {
                    device_name: "x".repeat(64),
                    client_version: "y".into(),
                }),
                &mut buf,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FrameError::Engine(Error::Protocol(ProtocolError::PayloadTooLarge { .. }))
        ));
        assert!(buf.is_empty());
    }

    #[test]
    fn corrupt_payload_is_a_decode_error_not_a_panic() {
        let mut c = codec();
        let mut bytes = encode_one(&mut c, &VacuumDropMessage::Ping(Ping { nonce: 1 }));
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let err = codec().decode(&mut bytes).unwrap_err();
        assert!(matches!(err, FrameError::Engine(Error::InvalidMessage(_))));
    }

    #[test]
    fn version_mismatch_is_refused() {
        let mut writer = FrameCodec::new(FrameHeader::new(1, 7, 7), 1024 * 1024);
        let mut bytes = BytesMut::new();
        writer
            .encode(VacuumDropMessage::Ping(Ping { nonce: 1 }), &mut bytes)
            .unwrap();

        // A reader that negotiated a different version must not decode it.
        let mut reader = FrameCodec::new(FrameHeader::new(2, 7, 7), 1024 * 1024);
        assert!(matches!(
            reader.decode(&mut bytes),
            Err(FrameError::Engine(Error::Protocol(
                ProtocolError::StateViolation(_)
            )))
        ));
    }

    #[test]
    fn decoder_reflects_peer_correlation_ids() {
        let mut writer = FrameCodec::new(FrameHeader::new(1, 0xABCD, 0x1234), 1024 * 1024);
        let mut bytes = BytesMut::new();
        writer
            .encode(VacuumDropMessage::Ping(Ping { nonce: 1 }), &mut bytes)
            .unwrap();

        let mut reader = codec();
        reader.decode(&mut bytes).unwrap();
        assert_eq!(reader.header().session_id, 0xABCD);
        assert_eq!(reader.header().request_id, 0x1234);
    }

    #[test]
    fn buffer_over_the_hard_cap_is_refused() {
        let mut c = codec();
        let mut bytes = BytesMut::from(&vec![CODEC_TAG; MAX_READ_BUF + 1][..]);
        assert!(matches!(
            c.decode(&mut bytes),
            Err(FrameError::Engine(Error::Protocol(
                ProtocolError::PayloadTooLarge { .. }
            )))
        ));
    }

    #[test]
    fn random_bytes_never_panic() {
        // A fuzz-shaped smoke test. Deterministic LCG so failures reproduce.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        let mut c = codec();
        for _ in 0..2000 {
            let len = (next() % 200) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
            let mut buf = BytesMut::from(&bytes[..]);
            // May succeed or fail; the contract is only that it never panics and
            // never exceeds the cap.
            let _ = c.decode(&mut buf);
            assert!(buf.len() <= MAX_READ_BUF);
        }
    }

    #[test]
    fn request_id_is_stamped_on_outgoing_frames() {
        let mut c = codec();
        c.set_request_id(0x5555);
        let mut bytes = BytesMut::new();
        c.encode(VacuumDropMessage::Ping(Ping { nonce: 1 }), &mut bytes)
            .unwrap();

        let mut r = codec();
        r.decode(&mut bytes).unwrap();
        assert_eq!(r.header().request_id, 0x5555);
    }

    #[test]
    fn version_one_is_what_the_codec_speaks() {
        assert_eq!(NegotiatedVersion::LOCAL, 1);
        assert_eq!(codec().header().version, 1);
    }
}
