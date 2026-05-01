//! `orp-tak` — TAK Protocol v1 wire codec for Rust.
//!
//! As of 2026 there is no first-class Rust crate implementing the TAK Server
//! data-plane protocol used by ATAK, WinTAK, and iTAK clients. The Python
//! [`takproto`] and Go [`gotak`] are the reference implementations; this crate
//! ports their framing layer to Rust.
//!
//! TAK Protocol v1 wraps Cursor-on-Target (CoT) XML payloads in a length-
//! prefixed binary envelope. Two flavours are defined by the TAK spec:
//!
//! * **Mesh** — UDP multicast: `0xBF 0x01 0xBF <CoT XML>`. Marker 0xBF
//!   acts as both magic byte and version (`0x01` = TAK Protocol v1).
//!   Used over `239.2.3.1:6969` for local-mesh discovery.
//! * **Stream** — TCP/TLS unicast: `0xBF <varint length> <CoT XML>`. Length
//!   is a protobuf-style base-128 varint (1–5 bytes covering up to ~2^35
//!   payload bytes; in practice ≤ 64 KiB for sane CoT messages).
//!
//! ORP already has a CoT XML parser in `orp-connector::adapters::cot`. This
//! crate is **only** the framing — it hands raw CoT XML bytes to / from the
//! caller, who is expected to plug into that parser (or any other CoT
//! handler).
//!
//! ## Examples
//!
//! ### Decoding a TAK mesh frame
//! ```rust
//! use orp_tak::{decode_mesh_frame, TakDecodeError};
//!
//! let frame: &[u8] = &[
//!     0xbf, 0x01, 0xbf,
//!     b'<', b'e', b'v', b'e', b'n', b't', b'/', b'>',
//! ];
//! let payload = decode_mesh_frame(frame).unwrap();
//! assert_eq!(payload, b"<event/>");
//! ```
//!
//! ### Encoding a TAK stream frame
//! ```rust
//! use orp_tak::encode_stream_frame;
//!
//! let payload = b"<event uid='ABC' type='a-f-G-U-C'/>";
//! let mut out = Vec::new();
//! encode_stream_frame(payload, &mut out);
//! assert_eq!(out[0], 0xbf);
//! // out[1..] contains varint(payload.len()) followed by payload bytes.
//! ```
//!
//! [`takproto`]: https://github.com/snstac/takproto
//! [`gotak`]: https://github.com/atc-net/gotak

mod wire;

pub use wire::{
    decode_mesh_frame, decode_stream_frame, decode_varint, encode_mesh_frame, encode_stream_frame,
    encode_varint, MeshFrame, StreamFrame, TakDecodeError, TakFrameKind, MAGIC, MAX_PAYLOAD_BYTES,
    MAX_VARINT_BYTES, MESH_FRAME_LEN, MESH_VERSION,
};
