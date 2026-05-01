//! TAK Protocol v1 wire framing.
//!
//! Two transport flavours share the same magic byte `0xBF`. This module
//! has the encode + decode for both, plus the protobuf-style varint helpers
//! used by the stream variant.

use thiserror::Error;

/// Magic byte that opens every TAK Protocol v1 frame.
pub const MAGIC: u8 = 0xbf;

/// Version field of a TAK mesh frame: byte 1 of `[0xBF, 0x01, 0xBF, …]`.
pub const MESH_VERSION: u8 = 0x01;

/// Mesh-frame fixed header length (3 bytes: magic, version, magic).
pub const MESH_FRAME_LEN: usize = 3;

/// Hard ceiling on a varint length prefix in bytes. The protobuf wire format
/// uses up to 10 bytes for u64; TAK practical payloads fit in 5.
pub const MAX_VARINT_BYTES: usize = 5;

/// Hard ceiling on payload length we will accept in a single TAK frame —
/// a defence against runaway varint values pointing at huge buffers. CoT
/// XML rarely exceeds 16 KiB; we set the floor at 1 MiB to leave room for
/// data packages metadata without inviting OOMs.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// Errors from decoding TAK frames.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TakDecodeError {
    #[error("buffer too short for TAK frame (need {need} bytes, got {got})")]
    TooShort { need: usize, got: usize },
    #[error("missing TAK magic byte 0xBF at position {position}")]
    BadMagic { position: usize },
    #[error("unsupported TAK protocol version {version} (only v1 is implemented)")]
    BadVersion { version: u8 },
    #[error("varint length prefix exceeded {MAX_VARINT_BYTES} bytes — likely garbage")]
    VarintTooLong,
    #[error("declared payload length {len} exceeds {MAX_PAYLOAD_BYTES} byte ceiling")]
    PayloadTooLarge { len: u64 },
    #[error("declared payload length {declared} but {available} bytes available")]
    PayloadTruncated { declared: usize, available: usize },
}

/// Result of a successful mesh-frame decode — borrowed slice of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshFrame<'a> {
    pub payload: &'a [u8],
}

/// Result of a successful stream-frame decode — borrowed payload + the
/// number of bytes consumed (so callers iterating a TCP read buffer can
/// advance past this frame and decode the next).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFrame<'a> {
    pub payload: &'a [u8],
    pub consumed: usize,
}

/// Sentinel describing which TAK framing a buffer's first byte indicates.
/// Useful for adapters that accept both flavours on a single port and need
/// to dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakFrameKind {
    Mesh,
    Stream,
    /// First byte didn't match magic — caller should drop the byte and
    /// re-sync at the next 0xBF.
    Unknown,
}

impl TakFrameKind {
    pub fn classify(buf: &[u8]) -> Self {
        if buf.len() < 2 || buf[0] != MAGIC {
            return Self::Unknown;
        }
        // Mesh: 0xBF 0x01 0xBF — version byte sits at offset 1.
        if buf[1] == MESH_VERSION && buf.get(2) == Some(&MAGIC) {
            return Self::Mesh;
        }
        // Otherwise treat as stream (the second byte starts a varint).
        Self::Stream
    }
}

/// Decode a TAK mesh frame. The buffer must begin with `[0xBF, 0x01, 0xBF, …]`;
/// the entire body after the third byte is treated as payload.
///
/// Mesh frames don't carry their own length — the UDP datagram boundary IS
/// the length, which is why TAK chose this format for multicast: one frame
/// per packet.
pub fn decode_mesh_frame(buf: &[u8]) -> Result<&[u8], TakDecodeError> {
    if buf.len() < MESH_FRAME_LEN {
        return Err(TakDecodeError::TooShort {
            need: MESH_FRAME_LEN,
            got: buf.len(),
        });
    }
    if buf[0] != MAGIC {
        return Err(TakDecodeError::BadMagic { position: 0 });
    }
    if buf[1] != MESH_VERSION {
        return Err(TakDecodeError::BadVersion { version: buf[1] });
    }
    if buf[2] != MAGIC {
        return Err(TakDecodeError::BadMagic { position: 2 });
    }
    Ok(&buf[MESH_FRAME_LEN..])
}

/// Encode a TAK mesh frame: `[0xBF, 0x01, 0xBF, payload…]`.
pub fn encode_mesh_frame(payload: &[u8], out: &mut Vec<u8>) {
    out.reserve(MESH_FRAME_LEN + payload.len());
    out.extend_from_slice(&[MAGIC, MESH_VERSION, MAGIC]);
    out.extend_from_slice(payload);
}

/// Decode a TAK stream frame: `[0xBF, varint(N), payload[N]]`. Returns the
/// payload slice plus the number of bytes consumed (header + payload) so
/// the caller can advance through a contiguous read buffer.
///
/// A short read returns `Err(PayloadTruncated)`; the caller should accumulate
/// more bytes from the socket and retry.
pub fn decode_stream_frame(buf: &[u8]) -> Result<StreamFrame<'_>, TakDecodeError> {
    if buf.is_empty() {
        return Err(TakDecodeError::TooShort { need: 1, got: 0 });
    }
    if buf[0] != MAGIC {
        return Err(TakDecodeError::BadMagic { position: 0 });
    }
    let (len, varint_bytes) = decode_varint(&buf[1..])?;
    if len as usize > MAX_PAYLOAD_BYTES {
        return Err(TakDecodeError::PayloadTooLarge { len });
    }
    let len = len as usize;
    let header_total = 1 + varint_bytes;
    let frame_end = header_total + len;
    if buf.len() < frame_end {
        return Err(TakDecodeError::PayloadTruncated {
            declared: len,
            available: buf.len().saturating_sub(header_total),
        });
    }
    Ok(StreamFrame {
        payload: &buf[header_total..frame_end],
        consumed: frame_end,
    })
}

/// Encode a TAK stream frame.
pub fn encode_stream_frame(payload: &[u8], out: &mut Vec<u8>) {
    out.push(MAGIC);
    encode_varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

/// Decode a base-128 varint (protobuf-compatible). Returns the value and
/// the number of bytes consumed.
pub fn decode_varint(buf: &[u8]) -> Result<(u64, usize), TakDecodeError> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &byte) in buf.iter().enumerate() {
        if i >= MAX_VARINT_BYTES {
            return Err(TakDecodeError::VarintTooLong);
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err(TakDecodeError::TooShort { need: 1, got: 0 })
}

/// Encode a u64 as a base-128 varint, appending to `out`.
pub fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Varint round-trips ────────────────────────────────────────────────

    #[test]
    fn varint_zero_is_one_byte() {
        let mut out = Vec::new();
        encode_varint(0, &mut out);
        assert_eq!(out, vec![0x00]);
    }

    #[test]
    fn varint_127_is_one_byte() {
        let mut out = Vec::new();
        encode_varint(127, &mut out);
        assert_eq!(out, vec![0x7f]);
    }

    #[test]
    fn varint_128_is_two_bytes() {
        let mut out = Vec::new();
        encode_varint(128, &mut out);
        assert_eq!(out, vec![0x80, 0x01]);
    }

    #[test]
    fn varint_roundtrip_random_values() {
        for n in [0u64, 1, 50, 127, 128, 16_384, 1_000_000, 4_294_967_296] {
            let mut buf = Vec::new();
            encode_varint(n, &mut buf);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, n, "round-trip failed for {n}");
        }
    }

    #[test]
    fn varint_decode_rejects_runaway() {
        // 6 continuation-bit-set bytes — exceeds MAX_VARINT_BYTES.
        let buf = vec![0xff; 6];
        assert!(matches!(
            decode_varint(&buf),
            Err(TakDecodeError::VarintTooLong)
        ));
    }

    // ── Mesh framing ──────────────────────────────────────────────────────

    #[test]
    fn mesh_decode_happy_path() {
        let frame = [&[0xbf, 0x01, 0xbf][..], b"<event/>"].concat();
        let payload = decode_mesh_frame(&frame).unwrap();
        assert_eq!(payload, b"<event/>");
    }

    #[test]
    fn mesh_decode_rejects_truncated() {
        assert!(matches!(
            decode_mesh_frame(&[0xbf, 0x01]),
            Err(TakDecodeError::TooShort { .. })
        ));
    }

    #[test]
    fn mesh_decode_rejects_wrong_version() {
        let err = decode_mesh_frame(&[0xbf, 0x02, 0xbf]).unwrap_err();
        assert_eq!(err, TakDecodeError::BadVersion { version: 0x02 });
    }

    #[test]
    fn mesh_decode_rejects_bad_magic() {
        let err = decode_mesh_frame(&[0xff, 0x01, 0xbf]).unwrap_err();
        assert_eq!(err, TakDecodeError::BadMagic { position: 0 });
        let err = decode_mesh_frame(&[0xbf, 0x01, 0xff]).unwrap_err();
        assert_eq!(err, TakDecodeError::BadMagic { position: 2 });
    }

    #[test]
    fn mesh_encode_roundtrip() {
        let payload = b"<event uid='X' type='a-f-G-U-C'/>";
        let mut out = Vec::new();
        encode_mesh_frame(payload, &mut out);
        let decoded = decode_mesh_frame(&out).unwrap();
        assert_eq!(decoded, payload.as_slice());
    }

    // ── Stream framing ────────────────────────────────────────────────────

    #[test]
    fn stream_encode_roundtrip_small() {
        let payload = b"<event/>";
        let mut out = Vec::new();
        encode_stream_frame(payload, &mut out);
        let frame = decode_stream_frame(&out).unwrap();
        assert_eq!(frame.payload, payload.as_slice());
        assert_eq!(frame.consumed, out.len());
    }

    #[test]
    fn stream_encode_roundtrip_large() {
        // Payload > 127 bytes → varint takes 2 bytes.
        let payload = vec![b'a'; 5_000];
        let mut out = Vec::new();
        encode_stream_frame(&payload, &mut out);
        let frame = decode_stream_frame(&out).unwrap();
        assert_eq!(frame.payload.len(), 5_000);
        assert_eq!(frame.consumed, out.len());
    }

    #[test]
    fn stream_decode_two_frames_in_one_buffer() {
        // Real TCP path: two TAK frames concatenated in a single read buffer.
        let mut buf = Vec::new();
        encode_stream_frame(b"first", &mut buf);
        encode_stream_frame(b"second message", &mut buf);

        let f1 = decode_stream_frame(&buf).unwrap();
        assert_eq!(f1.payload, b"first");
        let f2 = decode_stream_frame(&buf[f1.consumed..]).unwrap();
        assert_eq!(f2.payload, b"second message");
        assert_eq!(f1.consumed + f2.consumed, buf.len());
    }

    #[test]
    fn stream_decode_partial_returns_truncated() {
        let mut full = Vec::new();
        encode_stream_frame(b"hello world", &mut full);
        // Drop the last 5 bytes — payload starts but doesn't finish.
        let partial = &full[..full.len() - 5];
        match decode_stream_frame(partial) {
            Err(TakDecodeError::PayloadTruncated {
                declared,
                available,
            }) => {
                assert_eq!(declared, 11);
                assert!(available < 11);
            }
            other => panic!("expected PayloadTruncated, got {other:?}"),
        }
    }

    #[test]
    fn stream_decode_rejects_oversize_declared_length() {
        // Manually craft a header that claims 100 MiB of payload —
        // protective ceiling must reject it before any allocation.
        let mut buf = vec![MAGIC];
        encode_varint((MAX_PAYLOAD_BYTES as u64) + 1, &mut buf);
        match decode_stream_frame(&buf) {
            Err(TakDecodeError::PayloadTooLarge { .. }) => (),
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn stream_decode_rejects_bad_magic() {
        let err = decode_stream_frame(&[0x00, 0x00]).unwrap_err();
        assert_eq!(err, TakDecodeError::BadMagic { position: 0 });
    }

    // ── Frame-kind classifier ─────────────────────────────────────────────

    #[test]
    fn classify_mesh_three_byte_header() {
        assert_eq!(
            TakFrameKind::classify(&[0xbf, 0x01, 0xbf]),
            TakFrameKind::Mesh
        );
        assert_eq!(
            TakFrameKind::classify(&[0xbf, 0x01, 0xbf, b'<']),
            TakFrameKind::Mesh
        );
    }

    #[test]
    fn classify_stream_when_second_byte_is_varint() {
        // Stream frame for an empty payload: [0xbf, 0x00].
        assert_eq!(TakFrameKind::classify(&[0xbf, 0x00]), TakFrameKind::Stream);
        // Common case: short payload — varint is one byte < 128 but != 0x01.
        assert_eq!(TakFrameKind::classify(&[0xbf, 0x10]), TakFrameKind::Stream);
    }

    #[test]
    fn classify_unknown_for_no_magic() {
        assert_eq!(TakFrameKind::classify(&[]), TakFrameKind::Unknown);
        assert_eq!(TakFrameKind::classify(&[0x00]), TakFrameKind::Unknown);
        assert_eq!(TakFrameKind::classify(&[0xff, 0x01]), TakFrameKind::Unknown);
    }

    // ── 0x01 disambiguation: 1-byte varint is also 0x01. ─────────────────
    //
    // This is the trickiest case in the wire format: a stream frame for a
    // 1-byte payload starts `[0xBF, 0x01, <byte>]`, which collides byte-for-
    // byte with a mesh frame whose third byte happens to be `0xBF`. Only the
    // third byte differs:
    //   mesh:   [0xBF, 0x01, 0xBF, ...]    third byte = MAGIC, payload follows
    //   stream: [0xBF, 0x01, 0xBF]         third byte IS the 1-byte payload
    // ORP follows `takproto`'s convention: classify as Mesh when len ≥ 3 and
    // the third byte is MAGIC. The probability of a stream frame's only
    // payload byte being 0xBF is 1/256 and that's acceptable; a stream
    // sender should prefer payloads ≠ 0xBF for 1-byte messages or use a
    // 2-byte varint explicitly.

    #[test]
    fn classify_mesh_wins_disambiguation() {
        // [0xBF, 0x01, 0xBF, <CoT>] — Mesh.
        assert_eq!(
            TakFrameKind::classify(&[0xbf, 0x01, 0xbf, b'X']),
            TakFrameKind::Mesh
        );
    }
}
