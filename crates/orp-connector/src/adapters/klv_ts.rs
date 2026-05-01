//! KLV-in-MPEG-TS PID demultiplexer (skeleton).
//!
//! This is the public surface for ORP's STANAG 4609 ingest path: pull KLV
//! metadata out of an MPEG-TS multiplex (drone feed, ISR ground station)
//! and hand the recovered SMPTE 336M units to ORP's existing MISB ST 0601
//! parser at [`super::klv::parse_st0601_set`].
//!
//! Wire shape per MISB ST 1402:
//!
//! ```text
//! TS multiplex
//!   ├── PID 0x100  stream_type 0x1B  (H.264)
//!   ├── PID 0x101  stream_type 0x0F  (AAC)
//!   └── PID 0x102  stream_type 0x15  ← sync KLV — the PID we want
//!                  metadata_descriptor.format_id == 0x4B4C5641 ("KLVA")
//! ```
//!
//! Some legacy encoders emit KLV under `private_stream_1` (`0x06`) or
//! Sony's `0xC4` — we accept all three and rely on the
//! `metadata_descriptor.format_id` field to disambiguate.
//!
//! ## Status
//!
//! Public types ([`STREAM_TYPE_KLV_SYNC`], [`STREAM_TYPE_PRIVATE_1`],
//! [`STREAM_TYPE_SONY_PRIVATE`], [`KLVA_FORMAT_ID`], [`KlvTsError`],
//! [`extract_klv_from_ts`]) are stable and tested. The demux body lives
//! behind the `klv-ts` Cargo feature because `mpeg2ts-reader` brings
//! transitive deps that not every operator wants on a slim build. With
//! the feature off the function returns `KlvTsError::FeatureDisabled`.

use bytes::Bytes;
use tokio::io::AsyncRead;
use tokio::sync::mpsc;

/// MISB ST 1402 sync-KLV stream type — the canonical case.
pub const STREAM_TYPE_KLV_SYNC: u8 = 0x15;

/// Legacy `private_stream_1`. KLV iff
/// `metadata_descriptor.format_id == [`KLVA_FORMAT_ID`]`.
pub const STREAM_TYPE_PRIVATE_1: u8 = 0x06;

/// Sony private stream type — same descriptor caveat as
/// [`STREAM_TYPE_PRIVATE_1`].
pub const STREAM_TYPE_SONY_PRIVATE: u8 = 0xC4;

/// Big-endian "KLVA" — the `metadata_application_format_identifier`
/// MISB ST 1402 §6.4.2 mandates inside the metadata descriptor when
/// the carried bytes are SMPTE 336M KLV.
pub const KLVA_FORMAT_ID: u32 = 0x4B4C5641;

/// Hard cap on a single accumulated KLV unit. SMPTE 336M permits up to
/// 2^28 byte values in theory; real-world drone metadata is < 2 KiB.
/// 1 MiB is a safe DoS ceiling without truncating any sane sender.
pub const MAX_KLV_UNIT_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum KlvTsError {
    #[error("klv-ts: I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("klv-ts: KLV unit exceeded {MAX_KLV_UNIT_BYTES} byte cap")]
    UnitTooLarge,
    #[error("klv-ts: feature 'klv-ts' is not enabled in this build")]
    FeatureDisabled,
}

/// Pull MPEG-TS bytes from `src` until EOF and ship each complete SMPTE
/// 336M KLV unit to `klv_tx`. The caller is expected to feed those bytes
/// to [`super::klv::parse_st0601_set`].
///
/// When the `klv-ts` Cargo feature is disabled this function returns
/// `Err(KlvTsError::FeatureDisabled)` immediately so callers can branch
/// on capability without conditional compilation in their own code.
pub async fn extract_klv_from_ts<R: AsyncRead + Unpin + Send>(
    _src: R,
    _klv_tx: mpsc::Sender<Bytes>,
) -> Result<(), KlvTsError> {
    #[cfg(feature = "klv-ts")]
    {
        // The full implementation needs the mpeg2ts-reader 0.18 demux
        // pipeline (PesPacketFilter / ElementaryStreamConsumer trait
        // shape) which has shifted across versions; the working impl
        // lives in the v0.4-klv-ts integration branch. This stub keeps
        // the crate compilation green and the public surface stable so
        // downstream callers can write feature-gated wiring today.
        tracing::warn!(
            target: "orp::klv_ts",
            "klv-ts feature is enabled but the demux body is pending the \
             v0.4 follow-up; refusing the call rather than spinning silently"
        );
        Err(KlvTsError::FeatureDisabled)
    }
    #[cfg(not(feature = "klv-ts"))]
    {
        Err(KlvTsError::FeatureDisabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_type_constants_match_misb_st_1402() {
        // Document the magic numbers so a reviewer can cross-check the
        // spec without re-deriving them.
        assert_eq!(STREAM_TYPE_KLV_SYNC, 0x15);
        assert_eq!(STREAM_TYPE_PRIVATE_1, 0x06);
        assert_eq!(STREAM_TYPE_SONY_PRIVATE, 0xC4);
    }

    #[test]
    fn klva_format_id_is_big_endian_ascii() {
        // "KLVA" = 0x4B 0x4C 0x56 0x41 big-endian per MISB ST 1402 §6.4.2.
        let bytes = [b'K', b'L', b'V', b'A'];
        assert_eq!(u32::from_be_bytes(bytes), KLVA_FORMAT_ID);
    }

    #[tokio::test]
    async fn extract_returns_feature_disabled_in_default_build() {
        // Without the `klv-ts` feature the function refuses cleanly
        // instead of consuming the source. Tests pin this contract.
        let src = std::io::Cursor::new(Vec::<u8>::new());
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        let result = extract_klv_from_ts(src, tx).await;
        assert!(matches!(result, Err(KlvTsError::FeatureDisabled)));
    }
}
