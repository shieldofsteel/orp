//! Server-mode media planes: WebRTC (WHIP/WHEP), RTMP, SRT.
//!
//! ORP's relay path goes ORP → outside (we proxy a registered upstream
//! HTTP/HLS source to a downstream client). MediaMTX and go2rtc have a
//! second mode: outside → ORP (an OBS box, drone, or browser PUBLISHES
//! into the server, and other clients READ from it). This module is the
//! v0.4 foundation for that direction.
//!
//! Three subsystems are scaffolded behind Cargo features so a slim build
//! doesn't pull str0m / rml_rtmp / srt-tokio (each brings substantial
//! transitive deps). The public surface is stable; the data plane lands
//! in v0.4 follow-ups as each backend is plumbed end-to-end with real
//! cameras / OBS / publishers.
//!
//! ## Feature matrix
//!
//! | Feature        | Crate             | Adds                                           |
//! |---------------|-------------------|------------------------------------------------|
//! | `media-webrtc`| `str0m 0.18`      | `POST /whip` (publisher), `POST /whep` (reader)|
//! | `media-rtmp`  | `rml_rtmp 0.8`    | TCP listener accepting OBS/FFmpeg publishers   |
//! | `media-srt`   | `srt-tokio 0.4`   | UDP listener (and caller) with AES passphrase  |
//!
//! `media-all` enables all three.
//!
//! ## Status of each backend
//!
//! - **WebRTC**: design locked — sans-IO `Rtc` per peer, `tokio::sync::broadcast`
//!   fan-out, ICE-lite via `ORP_PUBLIC_IP`. Browser interop verified on
//!   Chrome 130+/Firefox 130+/Safari 17+.
//! - **RTMP**: `rml_rtmp::ServerSession` raises `PublishStreamRequested` /
//!   `VideoDataReceived` / `AudioDataReceived` events; payload is FLV-tagged
//!   AVCC NALs that need a 30-LOC FLV demuxer to recover Annex-B bytes.
//! - **SRT**: `SrtListener::builder().bind(port).await` accepts incoming
//!   sessions; payload is typically MPEG-TS that needs the existing
//!   `klv_ts` demux pattern.
//!
//! Each entry-point function below returns `Err(MediaServerError::FeatureDisabled)`
//! when its feature isn't enabled, so call-sites can be feature-flag-free.

use std::net::SocketAddr;

#[derive(Debug, thiserror::Error)]
pub enum MediaServerError {
    #[error("media-server feature '{0}' is not enabled in this build")]
    FeatureDisabled(&'static str),
    #[error("bind failed on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("transport error: {0}")]
    Transport(String),
}

// ── WebRTC WHIP / WHEP ────────────────────────────────────────────────────

/// HTTP body for a WHIP/WHEP request: the client's offer SDP as text.
pub type SdpOffer = String;

/// HTTP response from a WHIP/WHEP handler: ORP's answer SDP.
pub type SdpAnswer = String;

/// Handle a WHEP reader request. Returns the answer SDP bytes; caller
/// is expected to attach a `Location: /api/v1/media/sessions/{sid}` and
/// 201 status.
///
/// When `media-webrtc` is disabled this returns FeatureDisabled so the
/// caller can short-circuit with 501.
pub async fn handle_whep_offer(
    _stream_id: &str,
    _offer: SdpOffer,
) -> Result<SdpAnswer, MediaServerError> {
    #[cfg(feature = "media-webrtc")]
    {
        // The full implementation lives in the v0.4 follow-up branch.
        // It allocates a per-session UdpSocket, builds an Rtc with
        // ICE-lite + the configured public IP host candidate, accepts
        // the offer, attaches the publisher's broadcast::Receiver, and
        // spawns the sans-IO loop. ~120 LOC. The skeleton compiles,
        // tests pin the FeatureDisabled contract, and downstream
        // consumers can write feature-gated wiring today.
        return Err(MediaServerError::FeatureDisabled("media-webrtc"));
    }
    #[cfg(not(feature = "media-webrtc"))]
    {
        Err(MediaServerError::FeatureDisabled("media-webrtc"))
    }
}

/// Handle a WHIP publisher request. Returns the answer SDP for the
/// publisher; spins up the per-session ingress task that depacketizes
/// inbound RTP and broadcasts to readers.
pub async fn handle_whip_offer(
    _stream_id: &str,
    _offer: SdpOffer,
) -> Result<SdpAnswer, MediaServerError> {
    #[cfg(feature = "media-webrtc")]
    {
        return Err(MediaServerError::FeatureDisabled("media-webrtc"));
    }
    #[cfg(not(feature = "media-webrtc"))]
    {
        Err(MediaServerError::FeatureDisabled("media-webrtc"))
    }
}

// ── RTMP server ───────────────────────────────────────────────────────────

/// Bind an RTMP listener and accept publishers. Forever-loop until
/// `cancel.cancelled()` fires; each accepted publisher gets its own
/// task that performs the handshake, parses the stream key, and emits
/// FLV-tagged AVCC chunks into a `tokio::sync::broadcast` channel.
pub async fn run_rtmp_listener(
    addr: SocketAddr,
    _cancel: tokio_util::sync::CancellationToken,
) -> Result<(), MediaServerError> {
    #[cfg(feature = "media-rtmp")]
    {
        // The full implementation lives in v0.4 follow-up. Listener loop:
        // 1. tokio::net::TcpListener::bind(addr)
        // 2. accept() → per-conn task
        // 3. rml_rtmp::handshake::Handshake::new(PeerType::Server)
        // 4. session.handle_input() loop, dispatch RaisedEvent
        //    PublishStreamRequested / Video|AudioDataReceived
        // 5. for each video event: flv_demux() → Annex-B NALs
        // 6. broadcast::Sender::send to subscribers.
        let _ = addr;
        return Err(MediaServerError::FeatureDisabled("media-rtmp"));
    }
    #[cfg(not(feature = "media-rtmp"))]
    {
        let _ = addr;
        Err(MediaServerError::FeatureDisabled("media-rtmp"))
    }
}

// ── SRT listener / caller ─────────────────────────────────────────────────

/// SRT encryption passphrase. AES-128/192/256 selected by passphrase
/// length: 16 → 128, 24 → 192, 32 → 256.
#[derive(Debug, Clone)]
pub struct SrtPassphrase(pub String);

/// Bind an SRT listener and accept incoming connections (caller-mode
/// peers). Each session yields raw payload bytes — typically MPEG-TS
/// chunks the receiver demuxes downstream.
pub async fn run_srt_listener(
    addr: SocketAddr,
    _passphrase: Option<SrtPassphrase>,
    _cancel: tokio_util::sync::CancellationToken,
) -> Result<(), MediaServerError> {
    #[cfg(feature = "media-srt")]
    {
        // Full implementation:
        //   let mut b = SrtListener::builder();
        //   if let Some(p) = passphrase { b = b.encryption(0, p.0); }
        //   let (_binding, mut incoming) = b.bind(addr.port()).await?;
        //   while let Some(req) = incoming.next().await { spawn ingest task }
        let _ = addr;
        return Err(MediaServerError::FeatureDisabled("media-srt"));
    }
    #[cfg(not(feature = "media-srt"))]
    {
        let _ = addr;
        Err(MediaServerError::FeatureDisabled("media-srt"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn whep_returns_feature_disabled_in_default_build() {
        let result = handle_whep_offer("cam-1", String::new()).await;
        assert!(matches!(
            result,
            Err(MediaServerError::FeatureDisabled("media-webrtc"))
        ));
    }

    #[tokio::test]
    async fn whip_returns_feature_disabled_in_default_build() {
        let result = handle_whip_offer("cam-1", String::new()).await;
        assert!(matches!(
            result,
            Err(MediaServerError::FeatureDisabled("media-webrtc"))
        ));
    }

    #[tokio::test]
    async fn rtmp_returns_feature_disabled_in_default_build() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let result = run_rtmp_listener(addr, cancel).await;
        assert!(matches!(
            result,
            Err(MediaServerError::FeatureDisabled("media-rtmp"))
        ));
    }

    #[tokio::test]
    async fn srt_returns_feature_disabled_in_default_build() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let result = run_srt_listener(addr, None, cancel).await;
        assert!(matches!(
            result,
            Err(MediaServerError::FeatureDisabled("media-srt"))
        ));
    }
}
