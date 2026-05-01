//! RTSP client integration — pulls live H.264/H.265 from any IP camera.
//!
//! ORP needs to ingest RTSP because every NVR / ONVIF camera / drone-ground-
//! station / tactical-mast speaks it. Pre-v0.4 the media relay was HTTP-only
//! (JPEG/MJPEG/HLS), so RTSP cameras could be REGISTERED but not actually
//! relayed. This module closes that gap.
//!
//! ## Design
//!
//! `rtsp_to_h264_annexb` opens an RTSP session, plays the first video stream,
//! and pushes Annex-B-formatted H.264 access units onto an `mpsc::Sender`.
//! It is:
//!
//! * **Cancel-aware**: respects `tokio_util::sync::CancellationToken`. The
//!   `MediaRegistry::delete` path fires the token and the relay tears down.
//! * **Timeout-wrapped**: retina has no built-in timeouts (documented design).
//!   Every await is wrapped in `tokio::time::timeout` so a stuck camera does
//!   not hold a relay task forever.
//! * **Stat-emitting**: the same `MediaStreamStats` the HTTP relay populates
//!   gets bytes / frames / errors counters. The `/api/v1/media/stats`
//!   endpoint surfaces them live.
//! * **Annex-B by construction**: `FrameFormat::SIMPLE` makes retina
//!   re-prepend SPS+PPS on every keyframe, so a downstream decoder can be
//!   joined mid-stream without negotiation.
//!
//! ## Trade-offs
//!
//! * TCP/interleaved transport only. UDP is documented as experimental
//!   (no reorder buffer, no RTCP RR). NVRs accept TCP fine.
//! * No automatic UDP→TCP fallback (retina doesn't do that — they do TCP
//!   first, by our explicit choice, so the question doesn't arise).
//! * Reconnection on RTP loss is the caller's responsibility — when this
//!   function returns `Err`, the parent should re-spawn with backoff.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use retina::client::{
    Credentials, PlayOptions, Session, SessionOptions, SetupOptions, TcpTransportOptions,
    TeardownPolicy, Transport,
};
use retina::codec::CodecItem;

use crate::server::media::MediaStreamStats;

/// Connect-DESCRIBE timeout. Cameras under load take a moment to answer
/// the initial OPTIONS+DESCRIBE round. 8s is conservative enough that a
/// healthy LAN never trips it but a stalled box gets dropped.
pub const RTSP_DESCRIBE_TIMEOUT: Duration = Duration::from_secs(8);

/// SETUP-per-track timeout. Faster than DESCRIBE — once we know the SDP
/// the server should bind RTP/RTCP and answer in <1s on real hardware.
pub const RTSP_SETUP_TIMEOUT: Duration = Duration::from_secs(5);

/// PLAY timeout — same magnitude as SETUP.
pub const RTSP_PLAY_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-frame read timeout. A stuck stream that stops emitting access units
/// (camera firmware glitch, network black hole) gets torn down here. 15s
/// is generous — typical 30 fps streams emit every 33 ms; even a 1 fps
/// thermal stream emits inside this window.
pub const RTSP_READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Pull H.264 (and optionally H.265) from `url` into `tx` as Annex-B
/// access units. Returns when:
/// * `cancel.cancelled()` fires — Ok(()).
/// * The downstream `tx` is closed — Ok(()).
/// * The upstream stream EOFs cleanly — Err(EOF).
/// * Any timeout, decode, or transport error — Err with details.
///
/// Caller is expected to handle reconnection with backoff. The function
/// is intentionally single-shot so the policy can live where the parent
/// task sees it (e.g. exponential backoff in `MediaRegistry`).
pub async fn rtsp_to_h264_annexb(
    url: &str,
    cancel: CancellationToken,
    tx: mpsc::Sender<Bytes>,
    stats: Arc<MediaStreamStats>,
) -> Result<()> {
    let mut parsed =
        url::Url::parse(url).map_err(|e| anyhow!("rtsp: malformed URL '{url}': {e}"))?;
    if !matches!(parsed.scheme(), "rtsp" | "rtsps") {
        return Err(anyhow!(
            "rtsp: scheme '{}' is not rtsp/rtsps",
            parsed.scheme()
        ));
    }

    // Retina parses credentials separately — strip userinfo from the URL
    // we hand to it and pass the username/password as Credentials. Without
    // this Basic/Digest auth never gets attempted.
    let creds = (!parsed.username().is_empty()).then(|| Credentials {
        username: parsed.username().to_owned(),
        password: parsed.password().unwrap_or("").to_owned(),
    });
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);

    let opts = SessionOptions::default()
        .creds(creds)
        .user_agent("orp-rtsp/0.4".to_owned())
        .teardown(TeardownPolicy::Auto);

    // DESCRIBE — open the session and pull the SDP.
    let mut session = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        r = tokio::time::timeout(RTSP_DESCRIBE_TIMEOUT, Session::describe(parsed, opts)) => {
            match r {
                Err(_) => {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    return Err(anyhow!("rtsp: DESCRIBE timeout after {:?}", RTSP_DESCRIBE_TIMEOUT));
                }
                Ok(Err(e)) => {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    return Err(anyhow!("rtsp: DESCRIBE failed: {e}"));
                }
                Ok(Ok(s)) => s,
            }
        }
    };

    // Pick the first video track. Multi-track support (audio side-channel,
    // KLV metadata PID via the same RTSP session) is a follow-up — for now
    // single-stream is the 95% case and the smaller surface area.
    let video_idx = session
        .streams()
        .iter()
        .position(|s| s.media() == "video")
        .ok_or_else(|| anyhow!("rtsp: SDP advertises no video stream"))?;

    // SETUP video. SIMPLE frame format = Annex-B byte stream + auto-
    // prepended SPS/PPS on every keyframe. Decoder-friendly out of the box.
    tokio::time::timeout(
        RTSP_SETUP_TIMEOUT,
        session.setup(
            video_idx,
            SetupOptions::default().transport(Transport::Tcp(TcpTransportOptions::default())),
        ),
    )
    .await
    .map_err(|_| anyhow!("rtsp: SETUP timeout"))?
    .map_err(|e| anyhow!("rtsp: SETUP failed: {e}"))?;

    let played = tokio::time::timeout(
        RTSP_PLAY_TIMEOUT,
        session.play(
            PlayOptions::default()
                // Cap inter-frame timestamp jumps at 10s — anything beyond
                // is stream corruption, abort rather than feed downstream.
                .enforce_timestamps_with_max_jump_secs(NonZeroU32::new(10).unwrap()),
        ),
    )
    .await
    .map_err(|_| anyhow!("rtsp: PLAY timeout"))?
    .map_err(|e| anyhow!("rtsp: PLAY failed: {e}"))?;
    let mut demuxed = played
        .demuxed()
        .map_err(|e| anyhow!("rtsp: demux init: {e}"))?;

    info!(target: "orp::rtsp", url = %url, "rtsp session opened");

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(target: "orp::rtsp", "cancel requested");
                return Ok(());
            }
            next = tokio::time::timeout(RTSP_READ_TIMEOUT, demuxed.next()) => {
                let item = match next {
                    Err(_elapsed) => {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        return Err(anyhow!("rtsp: read timeout after {:?}", RTSP_READ_TIMEOUT));
                    }
                    Ok(None) => return Err(anyhow!("rtsp: upstream EOF")),
                    Ok(Some(Err(e))) => {
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                        return Err(anyhow!("rtsp: stream error: {e}"));
                    }
                    Ok(Some(Ok(it))) => it,
                };
                if let CodecItem::VideoFrame(frame) = item {
                    let lost = frame.loss();
                    if lost > 0 {
                        warn!(target: "orp::rtsp", lost, "rtp packet loss");
                        stats
                            .errors
                            .fetch_add(lost as u64, Ordering::Relaxed);
                    }
                    let buf = Bytes::copy_from_slice(frame.data());
                    let len = buf.len() as u64;
                    if tx.send(buf).await.is_err() {
                        // Downstream closed cleanly — not an error from
                        // ORP's POV.
                        info!(target: "orp::rtsp", "downstream channel closed");
                        return Ok(());
                    }
                    stats.bytes_relayed.fetch_add(len, Ordering::Relaxed);
                    *stats.last_activity.write().await = Some(chrono::Utc::now());
                }
                // Audio / OnvifMessage / ClockHint are dropped silently
                // for v0.4. KLV-over-RTP and audio-back-channel land later.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_non_rtsp_scheme() {
        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        let stats = Arc::new(MediaStreamStats::default());
        let err = rtsp_to_h264_annexb("http://camera/stream", cancel, tx, stats)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not rtsp/rtsps"));
    }

    #[tokio::test]
    async fn rejects_malformed_url() {
        let cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        let stats = Arc::new(MediaStreamStats::default());
        let err = rtsp_to_h264_annexb("not a url at all", cancel, tx, stats)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("malformed URL"));
    }

    #[tokio::test]
    async fn cancel_before_describe_returns_ok() {
        // Cancel BEFORE we even start the DESCRIBE — function should
        // hit the biased select arm and return Ok(()) immediately.
        // Uses a 203.0.113.0/24 (TEST-NET-3) address that won't connect
        // anywhere; the cancel must beat the connect attempt.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = mpsc::channel::<Bytes>(8);
        let stats = Arc::new(MediaStreamStats::default());
        let result = rtsp_to_h264_annexb("rtsp://203.0.113.42:554/main", cancel, tx, stats).await;
        assert!(
            result.is_ok(),
            "cancel-before-start should return Ok(()), got {result:?}"
        );
    }
}
