use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// Default ceiling on simultaneously-active media relay sessions across the
/// whole process. Tunable via [`MediaRegistry::with_max_concurrent_relays`].
/// Each active session holds one upstream TCP connection and one downstream
/// hyper Body; 256 leaves headroom on a 1024-fd default ulimit.
pub const DEFAULT_MAX_CONCURRENT_RELAYS: usize = 256;

/// Per-session hard byte ceiling. A legitimate camera tops out at a few
/// hundred kbps; 4 GiB is well past any sane single-viewer session.
pub const RELAY_BYTE_CAP: u64 = 4 * 1024 * 1024 * 1024;

/// If no upstream chunk arrives within this window the relay is torn down
/// and the downstream client gets a `TimedOut` error frame. This is what
/// closes the slow-loris hole the audit flagged as C2.
pub const RELAY_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// HLS playlist text bodies are bounded — 1 MiB is far above any real master
/// or media playlist and stops a hostile origin from OOMing the process via
/// `application/vnd.apple.mpegurl` content-type bait.
pub const HLS_PLAYLIST_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaProtocol {
    Rtsp,
    Rtmp,
    Hls,
    HttpFlv,
    Mjpeg,
    Jpeg,
    WebRtc,
    Whep,
    Srt,
    Onvif,
    V4l2,
    Usb,
    File,
    Klv,
    KlvTs,
}

impl MediaProtocol {
    pub fn from_source_url(url: &str) -> Result<Self, MediaRegistryError> {
        let scheme = url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .ok_or_else(|| {
                MediaRegistryError::Validation(
                    "media source_url must include a URL scheme".to_string(),
                )
            })?;

        match scheme.as_str() {
            "rtsp" | "rtsps" => Ok(Self::Rtsp),
            "rtmp" | "rtmps" => Ok(Self::Rtmp),
            "http" | "https" => Ok(classify_http_media(url)),
            "hls" => Ok(Self::Hls),
            "mjpeg" | "mjpg" => Ok(Self::Mjpeg),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "webrtc" => Ok(Self::WebRtc),
            "whep" => Ok(Self::Whep),
            "srt" => Ok(Self::Srt),
            "onvif" => Ok(Self::Onvif),
            "v4l2" => Ok(Self::V4l2),
            "usb" => Ok(Self::Usb),
            "file" => Ok(Self::File),
            "klv" => Ok(Self::Klv),
            "klv-ts" => Ok(Self::KlvTs),
            "exec" | "shell" | "pipe" | "stdin" | "echo" => Err(
                MediaRegistryError::Validation(format!(
                    "media source scheme '{scheme}' is intentionally disabled; use an explicit camera/media URL instead"
                )),
            ),
            other => Err(MediaRegistryError::Validation(format!(
                "unsupported media source scheme '{other}'"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rtsp => "rtsp",
            Self::Rtmp => "rtmp",
            Self::Hls => "hls",
            Self::HttpFlv => "http_flv",
            Self::Mjpeg => "mjpeg",
            Self::Jpeg => "jpeg",
            Self::WebRtc => "webrtc",
            Self::Whep => "whep",
            Self::Srt => "srt",
            Self::Onvif => "onvif",
            Self::V4l2 => "v4l2",
            Self::Usb => "usb",
            Self::File => "file",
            Self::Klv => "klv",
            Self::KlvTs => "klv_ts",
        }
    }
}

fn classify_http_media(url: &str) -> MediaProtocol {
    let lower = url.to_ascii_lowercase();
    if lower.contains(".m3u8") || lower.contains("format=hls") {
        MediaProtocol::Hls
    } else if lower.contains(".mjpeg")
        || lower.contains(".mjpg")
        || lower.contains("mjpeg")
        || lower.contains("multipart")
    {
        MediaProtocol::Mjpeg
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.contains("snapshot") {
        MediaProtocol::Jpeg
    } else {
        MediaProtocol::HttpFlv
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStreamStatus {
    Registered,
    RelayReady,
    NeedsRelay,
}

impl MediaStreamStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::RelayReady => "relay_ready",
            Self::NeedsRelay => "needs_relay",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaCapabilities {
    pub low_latency_view: bool,
    pub browser_fallback: bool,
    pub in_binary_relay: bool,
    pub metadata_fusion: bool,
    pub two_way_audio_candidate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStream {
    pub id: String,
    pub name: String,
    pub protocol: MediaProtocol,
    #[serde(skip_serializing)]
    pub source_url: String,
    pub source_url_redacted: String,
    pub allow_private_network: bool,
    pub expected_codec: Option<String>,
    pub klv_metadata: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    pub status: MediaStreamStatus,
    pub capabilities: MediaCapabilities,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MediaStream {
    pub fn entity_id(&self) -> String {
        format!("media:{}", self.id)
    }

    pub fn entity_properties(&self) -> HashMap<String, JsonValue> {
        let mut props = HashMap::new();
        props.insert(
            "media_protocol".to_string(),
            JsonValue::String(self.protocol.as_str().to_string()),
        );
        props.insert(
            "source_url".to_string(),
            JsonValue::String(self.source_url_redacted.clone()),
        );
        props.insert(
            "status".to_string(),
            JsonValue::String(self.status.as_str().to_string()),
        );
        props.insert(
            "allow_private_network".to_string(),
            JsonValue::Bool(self.allow_private_network),
        );
        props.insert(
            "klv_metadata".to_string(),
            JsonValue::Bool(self.klv_metadata),
        );
        props.insert(
            "labels".to_string(),
            JsonValue::Array(
                self.labels
                    .iter()
                    .map(|label| JsonValue::String(label.clone()))
                    .collect(),
            ),
        );
        if let Some(codec) = &self.expected_codec {
            props.insert(
                "expected_codec".to_string(),
                JsonValue::String(codec.clone()),
            );
        }
        props
    }

    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    pub fn supports_in_binary_relay(&self) -> bool {
        self.capabilities.in_binary_relay
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMediaStreamInput {
    pub id: Option<String>,
    pub name: String,
    pub source_url: String,
    #[serde(default)]
    pub allow_private_network: bool,
    pub expected_codec: Option<String>,
    #[serde(default)]
    pub klv_metadata: bool,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaRegistryError {
    Validation(String),
    Conflict(String),
    NotFound(String),
}

impl std::fmt::Display for MediaRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(msg) | Self::Conflict(msg) | Self::NotFound(msg) => {
                write!(f, "{msg}")
            }
        }
    }
}

impl std::error::Error for MediaRegistryError {}

/// Per-stream observability counters. All Atomic so the relay hot path
/// never grabs a lock; reads via `snapshot()` for the `/api/v1/media/stats`
/// endpoint. `last_activity` lives behind an `RwLock<Option<…>>` because
/// `DateTime<Utc>` isn't `Atomic`-friendly and we read it once per scrape.
#[derive(Default, Debug)]
pub struct MediaStreamStats {
    pub active_sessions: AtomicU64,
    pub total_sessions: AtomicU64,
    pub bytes_relayed: AtomicU64,
    pub errors: AtomicU64,
    pub last_activity: RwLock<Option<DateTime<Utc>>>,
}

/// Read-only snapshot of [`MediaStreamStats`] suitable for JSON emission.
#[derive(Debug, Clone, Serialize)]
pub struct MediaStreamStatsSnapshot {
    pub active_sessions: u64,
    pub total_sessions: u64,
    pub bytes_relayed: u64,
    pub errors: u64,
    pub last_activity: Option<DateTime<Utc>>,
}

impl MediaStreamStats {
    pub async fn snapshot(&self) -> MediaStreamStatsSnapshot {
        MediaStreamStatsSnapshot {
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
            total_sessions: self.total_sessions.load(Ordering::Relaxed),
            bytes_relayed: self.bytes_relayed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            last_activity: *self.last_activity.read().await,
        }
    }
}

/// Owning handle for an in-flight media stream — couples the descriptor, a
/// process-lifetime cancellation token, and the live counter set. Relay
/// tasks hold an `Arc<MediaStreamHandle>` so DELETE can cancel them by
/// firing the token; the registry drops its reference, the relay task drops
/// its reference, and Rust naturally collects the handle.
pub struct MediaStreamHandle {
    pub stream: MediaStream,
    pub cancel: CancellationToken,
    /// Arc-wrapped so a spawned relay task (RTSP, future RTMP/SRT/WebRTC)
    /// can hold an independent strong reference and keep bumping atomic
    /// counters even after the registry has dropped its handle on DELETE.
    /// All MediaStreamStats fields are atomic / RwLock so concurrent
    /// readers (the /api/v1/media/stats endpoint) and writers (the relay
    /// task) compose without coarse locking.
    pub stats: Arc<MediaStreamStats>,
}

/// Snapshot pairing an emitted stream descriptor with its current stats —
/// the exact JSON shape returned by `/api/v1/media/stats`.
#[derive(Debug, Clone, Serialize)]
pub struct MediaStreamWithStats {
    pub stream: MediaStream,
    pub stats: MediaStreamStatsSnapshot,
}

pub struct MediaRegistry {
    streams: RwLock<HashMap<String, Arc<MediaStreamHandle>>>,
    relay_semaphore: Arc<Semaphore>,
}

impl Default for MediaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaRegistry {
    pub fn new() -> Self {
        Self::with_max_concurrent_relays(DEFAULT_MAX_CONCURRENT_RELAYS)
    }

    pub fn with_max_concurrent_relays(max: usize) -> Self {
        Self {
            streams: RwLock::new(HashMap::new()),
            // Semaphore::new clamps to MAX permits internally; passing 0
            // would lock the relay path entirely, so guard against it.
            relay_semaphore: Arc::new(Semaphore::new(max.max(1))),
        }
    }

    /// Shared semaphore for outbound relay sessions. The handler acquires a
    /// permit before connecting upstream and holds it for the relay task's
    /// lifetime; an exhausted pool returns 503 instead of stacking sockets.
    pub fn relay_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.relay_semaphore)
    }

    pub async fn create(
        &self,
        input: CreateMediaStreamInput,
    ) -> Result<MediaStream, MediaRegistryError> {
        let stream = build_stream(input)?;
        let mut guard = self.streams.write().await;
        if guard.contains_key(&stream.id) {
            return Err(MediaRegistryError::Conflict(format!(
                "media stream '{}' already exists",
                stream.id
            )));
        }
        let handle = Arc::new(MediaStreamHandle {
            stream: stream.clone(),
            cancel: CancellationToken::new(),
            stats: Arc::new(MediaStreamStats::default()),
        });
        guard.insert(stream.id.clone(), handle);
        Ok(stream)
    }

    pub async fn list(&self) -> Vec<MediaStream> {
        let mut streams: Vec<_> = self
            .streams
            .read()
            .await
            .values()
            .map(|handle| handle.stream.clone())
            .collect();
        streams.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        streams
    }

    pub async fn list_with_stats(&self) -> Vec<MediaStreamWithStats> {
        let handles: Vec<Arc<MediaStreamHandle>> =
            self.streams.read().await.values().cloned().collect();
        let mut rows: Vec<MediaStreamWithStats> = Vec::with_capacity(handles.len());
        for handle in handles {
            rows.push(MediaStreamWithStats {
                stream: handle.stream.clone(),
                stats: handle.stats.snapshot().await,
            });
        }
        rows.sort_by(|a, b| {
            a.stream
                .name
                .cmp(&b.stream.name)
                .then(a.stream.id.cmp(&b.stream.id))
        });
        rows
    }

    pub async fn get(&self, id: &str) -> Option<MediaStream> {
        self.streams
            .read()
            .await
            .get(id)
            .map(|handle| handle.stream.clone())
    }

    pub async fn get_handle(&self, id: &str) -> Option<Arc<MediaStreamHandle>> {
        self.streams.read().await.get(id).cloned()
    }

    pub async fn delete(&self, id: &str) -> Result<MediaStream, MediaRegistryError> {
        let removed = self.streams.write().await.remove(id).ok_or_else(|| {
            MediaRegistryError::NotFound(format!("media stream '{id}' not found"))
        })?;
        // Fire the cancel token so any in-flight relay task tears down its
        // upstream socket and stops pushing chunks into its mpsc channel.
        // Without this, DELETE leaks the relay until the upstream EOFs or
        // the downstream client disconnects (the C1 finding from audit).
        removed.cancel.cancel();
        Ok(removed.stream.clone())
    }
}

fn build_stream(input: CreateMediaStreamInput) -> Result<MediaStream, MediaRegistryError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(MediaRegistryError::Validation(
            "media stream name cannot be empty".to_string(),
        ));
    }

    let source_url = input.source_url.trim();
    if source_url.is_empty() {
        return Err(MediaRegistryError::Validation(
            "media source_url cannot be empty".to_string(),
        ));
    }

    let protocol = MediaProtocol::from_source_url(source_url)?;
    validate_private_host_opt_in(source_url, input.allow_private_network)?;

    let id = input
        .id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| format!("stream-{}", uuid::Uuid::new_v4()));
    validate_id(&id)?;

    let now = Utc::now();
    Ok(MediaStream {
        id,
        name: name.to_string(),
        protocol,
        source_url: source_url.to_string(),
        source_url_redacted: redact_url_credentials(source_url),
        allow_private_network: input.allow_private_network,
        expected_codec: input.expected_codec.map(|codec| codec.trim().to_string()),
        klv_metadata: input.klv_metadata,
        labels: input
            .labels
            .into_iter()
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .collect(),
        status: if supports_in_binary_relay(protocol) {
            MediaStreamStatus::RelayReady
        } else {
            MediaStreamStatus::NeedsRelay
        },
        capabilities: capabilities_for(protocol, input.klv_metadata),
        created_at: now,
        updated_at: now,
    })
}

fn validate_id(id: &str) -> Result<(), MediaRegistryError> {
    if id.len() > 96 {
        return Err(MediaRegistryError::Validation(
            "media stream id must be 96 characters or fewer".to_string(),
        ));
    }
    if id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(MediaRegistryError::Validation(
            "media stream id may contain only ASCII letters, digits, '-' and '_'".to_string(),
        ))
    }
}

fn capabilities_for(protocol: MediaProtocol, klv_metadata: bool) -> MediaCapabilities {
    let low_latency_view = matches!(
        protocol,
        MediaProtocol::Rtsp
            | MediaProtocol::Rtmp
            | MediaProtocol::WebRtc
            | MediaProtocol::Whep
            | MediaProtocol::Srt
            | MediaProtocol::Mjpeg
    );
    let browser_fallback = matches!(
        protocol,
        MediaProtocol::Hls
            | MediaProtocol::Mjpeg
            | MediaProtocol::Jpeg
            | MediaProtocol::WebRtc
            | MediaProtocol::Whep
            | MediaProtocol::Rtsp
            | MediaProtocol::Rtmp
    );
    let two_way_audio_candidate = matches!(
        protocol,
        MediaProtocol::Rtsp | MediaProtocol::Onvif | MediaProtocol::WebRtc | MediaProtocol::Whep
    );

    MediaCapabilities {
        low_latency_view,
        browser_fallback,
        in_binary_relay: supports_in_binary_relay(protocol),
        metadata_fusion: klv_metadata
            || matches!(protocol, MediaProtocol::Klv | MediaProtocol::KlvTs),
        two_way_audio_candidate,
    }
}

fn supports_in_binary_relay(protocol: MediaProtocol) -> bool {
    // HTTP-family is relayed by re-streaming the upstream body. RTSP is
    // relayed by a dedicated client (retina) that depacketizes RTP into
    // Annex-B H.264. Both paths feed the same MediaStreamStats and are
    // governed by the same cancellation + semaphore invariants.
    matches!(
        protocol,
        MediaProtocol::Hls
            | MediaProtocol::HttpFlv
            | MediaProtocol::Mjpeg
            | MediaProtocol::Jpeg
            | MediaProtocol::Rtsp
    )
}

fn validate_private_host_opt_in(
    source_url: &str,
    allow_private_network: bool,
) -> Result<(), MediaRegistryError> {
    let Some(host) = extract_host(source_url) else {
        return Ok(());
    };
    let Ok(ip) = host.parse::<IpAddr>() else {
        return Ok(());
    };
    if is_private_or_local_ip(ip) && !allow_private_network {
        return Err(MediaRegistryError::Validation(format!(
            "media source host '{host}' is private/local; set allow_private_network=true to register LAN cameras explicitly"
        )));
    }
    Ok(())
}

fn extract_host(source_url: &str) -> Option<String> {
    let rest = source_url.split_once("://")?.1;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(rest.split(['/', '?', '#']).next().unwrap_or_default());
    if authority.is_empty() {
        return None;
    }
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped.split_once(']').map(|(host, _)| host.to_string());
    }
    Some(
        authority
            .split_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
            .to_string(),
    )
}

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 (::ffff:a.b.c.d) recurses on the v4 form so
            // a literal `[::ffff:127.0.0.1]` source URL is gated correctly.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_or_local_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

pub fn redact_url_credentials(source_url: &str) -> String {
    let Some((scheme, rest)) = source_url.split_once("://") else {
        return source_url.to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let suffix = &rest[authority_end..];
    if let Some((_, host)) = authority.rsplit_once('@') {
        format!("{scheme}://<redacted>@{host}{suffix}")
    } else {
        source_url.to_string()
    }
}

pub fn resolve_same_origin_media_url(
    base_url: &str,
    reference: &str,
) -> Result<String, MediaRegistryError> {
    // Reject reference inputs that are pathologically large before they
    // reach the URL parser — covers the "megabyte ?url= query" class.
    if reference.len() > 2048 {
        return Err(MediaRegistryError::Validation(
            "media playlist reference exceeds 2 KiB".to_string(),
        ));
    }

    let base = reqwest::Url::parse(base_url).map_err(|e| {
        MediaRegistryError::Validation(format!("media source_url is not a valid URL: {e}"))
    })?;
    let resolved = base.join(reference).map_err(|e| {
        MediaRegistryError::Validation(format!("media playlist reference is not valid: {e}"))
    })?;

    if !matches!(resolved.scheme(), "http" | "https") {
        return Err(MediaRegistryError::Validation(
            "media relay only supports http/https HLS segment URLs".to_string(),
        ));
    }

    // The base URL legitimately holds camera credentials (e.g. an HLS
    // origin behind basic auth). The *reference* is attacker-controllable
    // playlist content — a tampered upstream playlist could smuggle
    // userinfo to coerce ORP into mailing the credentials downstream.
    // Reject any userinfo on the resolved reference outright.
    if !resolved.username().is_empty() || resolved.password().is_some() {
        return Err(MediaRegistryError::Validation(
            "media playlist reference may not contain credentials".to_string(),
        ));
    }

    if base.scheme() != resolved.scheme()
        || base.host_str() != resolved.host_str()
        || base.port_or_known_default() != resolved.port_or_known_default()
    {
        return Err(MediaRegistryError::Validation(
            "media playlist reference points outside the registered source origin".to_string(),
        ));
    }

    Ok(resolved.to_string())
}

pub fn rewrite_hls_playlist(
    stream_id: &str,
    source_url: &str,
    playlist: &str,
) -> Result<String, MediaRegistryError> {
    if playlist.len() > HLS_PLAYLIST_MAX_BYTES {
        return Err(MediaRegistryError::Validation(format!(
            "HLS playlist exceeds {HLS_PLAYLIST_MAX_BYTES} byte ceiling"
        )));
    }
    let mut out = String::with_capacity(playlist.len() + 256);
    for line in playlist.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        if trimmed.starts_with('#') {
            // Any HLS tag may carry one or more `URI="..."` attributes —
            // EXT-X-KEY, EXT-X-MAP, EXT-X-MEDIA, EXT-X-I-FRAME-STREAM-INF,
            // EXT-X-SESSION-KEY, EXT-X-PART, EXT-X-PRELOAD-HINT,
            // EXT-X-RENDITION-REPORT, etc. (RFC 8216 §4.4 and HLS LL-HLS
            // additions). Rewriting every URI= attribute generically means
            // we don't have to chase the spec for new tag names.
            out.push_str(&rewrite_hls_uri_attributes(stream_id, source_url, line)?);
            out.push('\n');
            continue;
        }

        let resolved = resolve_same_origin_media_url(source_url, trimmed)?;
        out.push_str(&format!(
            "/api/v1/media/streams/{stream_id}/hls/fetch?url={}",
            percent_encode_component(&resolved)
        ));
        out.push('\n');
    }
    Ok(out)
}

fn rewrite_hls_uri_attributes(
    stream_id: &str,
    source_url: &str,
    line: &str,
) -> Result<String, MediaRegistryError> {
    let mut out = String::with_capacity(line.len() + 64);
    let mut cursor = 0;
    // Iterate `URI="..."` occurrences left-to-right so we cover any tag
    // that legally contains more than one (rare but spec-allowed). After
    // each rewrite we advance past the closing quote — never re-scanning
    // the rewritten relay URI itself, so we can't recurse into our own
    // output.
    while let Some(rel) = line[cursor..].find("URI=\"") {
        let attr_start = cursor + rel;
        let value_start = attr_start + 5;
        let Some(rel_end) = line[value_start..].find('"') else {
            // Malformed attribute — bail out and leave the line untouched.
            return Ok(line.to_string());
        };
        let value_end = value_start + rel_end;
        let resolved = resolve_same_origin_media_url(source_url, &line[value_start..value_end])?;
        let relay_uri = format!(
            "/api/v1/media/streams/{stream_id}/hls/fetch?url={}",
            percent_encode_component(&resolved)
        );
        out.push_str(&line[cursor..value_start]);
        out.push_str(&relay_uri);
        cursor = value_end;
    }
    out.push_str(&line[cursor..]);
    Ok(out)
}

pub fn validate_relay_target(
    stream: &MediaStream,
    target_url: &str,
) -> Result<(), MediaRegistryError> {
    if !stream.supports_in_binary_relay() {
        return Err(MediaRegistryError::Validation(format!(
            "in-binary relay for '{}' sources is not implemented yet",
            stream.protocol.as_str()
        )));
    }
    let _ = resolve_same_origin_media_url(stream.source_url(), target_url)?;
    Ok(())
}

fn percent_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(url: &str) -> CreateMediaStreamInput {
        CreateMediaStreamInput {
            id: Some("cam-1".to_string()),
            name: "Gate Camera".to_string(),
            source_url: url.to_string(),
            allow_private_network: true,
            expected_codec: Some("h264".to_string()),
            klv_metadata: false,
            labels: vec!["perimeter".to_string()],
        }
    }

    #[test]
    fn classifies_rtsp_and_redacts_credentials() {
        let stream = build_stream(input("rtsp://admin:secret@10.0.0.5:554/main")).unwrap();
        assert_eq!(stream.protocol, MediaProtocol::Rtsp);
        assert_eq!(
            stream.source_url_redacted,
            "rtsp://<redacted>@10.0.0.5:554/main"
        );
        assert_eq!(stream.entity_id(), "media:cam-1");
        assert!(stream.capabilities.low_latency_view);
        // Post-v0.4: RTSP IS relay-ready (retina pulls H.264 Annex-B
        // through the same MediaStreamHandle pipeline as HTTP/MJPEG/HLS).
        // Pre-v0.4 baseline asserted `!in_binary_relay`; this tracks the
        // shipped capability instead of the historical limitation.
        assert!(stream.capabilities.in_binary_relay);
        assert_eq!(stream.status, MediaStreamStatus::RelayReady);
    }

    #[test]
    fn private_ip_requires_explicit_opt_in() {
        let mut req = input("rtsp://192.168.1.20/stream");
        req.allow_private_network = false;
        let err = build_stream(req).unwrap_err();
        assert!(err.to_string().contains("allow_private_network=true"));
    }

    #[test]
    fn rejects_exec_sources() {
        let err = build_stream(input("exec://ffmpeg -i rtsp://camera")).unwrap_err();
        assert!(err.to_string().contains("intentionally disabled"));
    }

    #[test]
    fn classifies_http_hls_mjpeg_and_snapshot() {
        assert_eq!(
            MediaProtocol::from_source_url("https://example.com/live/index.m3u8").unwrap(),
            MediaProtocol::Hls
        );
        assert_eq!(
            MediaProtocol::from_source_url("https://example.com/cam.mjpeg").unwrap(),
            MediaProtocol::Mjpeg
        );
        assert_eq!(
            MediaProtocol::from_source_url("https://example.com/snapshot.jpg").unwrap(),
            MediaProtocol::Jpeg
        );
    }

    #[test]
    fn http_snapshot_is_relay_ready() {
        let stream = build_stream(input("https://example.com/snapshot.jpg")).unwrap();
        assert!(stream.supports_in_binary_relay());
        assert_eq!(stream.status, MediaStreamStatus::RelayReady);
    }

    #[test]
    fn hls_playlist_rewrites_relative_segments_and_keys() {
        let playlist =
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:2.0,\nseg-1.ts\n";
        let rewritten = rewrite_hls_playlist(
            "cam-1",
            "https://media.example.test/live/index.m3u8",
            playlist,
        )
        .unwrap();
        assert!(rewritten.contains("/api/v1/media/streams/cam-1/hls/fetch?url="));
        assert!(rewritten.contains("https%3A%2F%2Fmedia.example.test%2Flive%2Fseg-1.ts"));
        assert!(rewritten.contains("https%3A%2F%2Fmedia.example.test%2Flive%2Fkey.bin"));
    }

    #[test]
    fn hls_playlist_rejects_cross_origin_segments() {
        let err = rewrite_hls_playlist(
            "cam-1",
            "https://media.example.test/live/index.m3u8",
            "#EXTM3U\nhttps://evil.example.test/seg.ts\n",
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("outside the registered source origin"));
    }

    #[test]
    fn ipv4_mapped_ipv6_loopback_requires_opt_in() {
        // Regression: ::ffff:127.0.0.1 was previously accepted at register-time
        // because the v6 branch of is_private_or_local_ip didn't recurse on
        // to_ipv4_mapped(). Now the LAN gate fires.
        let mut req = input("http://[::ffff:127.0.0.1]/snapshot.jpg");
        req.allow_private_network = false;
        let err = build_stream(req).unwrap_err();
        assert!(err.to_string().contains("allow_private_network=true"));
    }

    #[test]
    fn ipv4_mapped_ipv6_rfc1918_requires_opt_in() {
        let mut req = input("http://[::ffff:10.0.0.5]/snapshot.jpg");
        req.allow_private_network = false;
        let err = build_stream(req).unwrap_err();
        assert!(err.to_string().contains("allow_private_network=true"));
    }

    #[test]
    fn hls_rewriter_handles_session_key_and_preload_hint() {
        // Audit finding H1: the previous rewriter only handled EXT-X-KEY and
        // EXT-X-MAP. EXT-X-SESSION-KEY (whole-presentation DRM key),
        // EXT-X-PRELOAD-HINT (LL-HLS), and EXT-X-RENDITION-REPORT all carry
        // URI=, and the generic rewriter must rewrite all of them.
        let playlist = "\
#EXTM3U
#EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"sk.bin\"
#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-0.ts\"
#EXT-X-RENDITION-REPORT:URI=\"rendition-1.m3u8\"
#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio.m3u8\",GROUP-ID=\"a1\",NAME=\"en\"
seg-0.ts
";
        let rewritten = rewrite_hls_playlist(
            "cam-1",
            "https://media.example.test/live/index.m3u8",
            playlist,
        )
        .unwrap();
        for ref_name in ["sk.bin", "part-0.ts", "rendition-1.m3u8", "audio.m3u8"] {
            let pct =
                percent_encode_component(&format!("https://media.example.test/live/{ref_name}"));
            assert!(
                rewritten.contains(&format!("?url={pct}")),
                "missing rewrite for {ref_name} in:\n{rewritten}"
            );
        }
        // The segment-line URL also gets rewritten through the same
        // /hls/fetch route — the percent-encoded form contains the seg name
        // exactly once (no recursion through the relay rewrite).
        let seg_pct = percent_encode_component("https://media.example.test/live/seg-0.ts");
        assert_eq!(
            rewritten.matches(&seg_pct).count(),
            1,
            "expected exactly one rewritten segment URL"
        );
        assert!(rewritten.contains("/api/v1/media/streams/cam-1/hls/fetch?url="));
    }

    #[test]
    fn hls_rewriter_rejects_credential_smuggling_on_segment() {
        // H2: a hostile playlist could ship a same-host URL with userinfo
        // to coerce ORP into mailing the creds downstream. The resolver
        // refuses any reference that carries username/password.
        let err = rewrite_hls_playlist(
            "cam-1",
            "https://media.example.test/live/index.m3u8",
            "#EXTM3U\nhttps://attacker:pw@media.example.test/seg.ts\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("credentials"),
            "expected credential rejection, got: {err}"
        );
    }

    #[test]
    fn hls_rewriter_rejects_2kib_reference() {
        let big = format!("seg-{}.ts", "x".repeat(3000));
        let err = rewrite_hls_playlist(
            "cam-1",
            "https://media.example.test/live/index.m3u8",
            &format!("#EXTM3U\n{big}\n"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("2 KiB"));
    }

    #[tokio::test]
    async fn registry_default_uses_default_max_relays() {
        let registry = MediaRegistry::new();
        assert_eq!(
            registry.relay_semaphore().available_permits(),
            DEFAULT_MAX_CONCURRENT_RELAYS
        );
    }

    #[tokio::test]
    async fn delete_fires_cancel_token_for_in_flight_relay() {
        // Audit finding C1: DELETE used to leak in-flight relay tasks.
        // Now the registry holds a CancellationToken on each handle, and
        // delete() fires it. A relay-side task observing the token via
        // `cancel.cancelled().await` will tear down its upstream socket.
        let registry = MediaRegistry::new();
        registry
            .create(input("https://example.com/snapshot.jpg"))
            .await
            .unwrap();
        let handle = registry.get_handle("cam-1").await.unwrap();
        assert!(!handle.cancel.is_cancelled());
        registry.delete("cam-1").await.unwrap();
        // Token clone held by the relay task observes cancellation
        // immediately — the registry only released its own copy.
        assert!(handle.cancel.is_cancelled());
    }

    #[tokio::test]
    async fn list_with_stats_emits_zero_counters_on_fresh_registry() {
        let registry = MediaRegistry::new();
        registry
            .create(input("https://example.com/snapshot.jpg"))
            .await
            .unwrap();
        let rows = registry.list_with_stats().await;
        assert_eq!(rows.len(), 1);
        let stats = &rows[0].stats;
        assert_eq!(stats.active_sessions, 0);
        assert_eq!(stats.total_sessions, 0);
        assert_eq!(stats.bytes_relayed, 0);
        assert_eq!(stats.errors, 0);
        assert!(stats.last_activity.is_none());
    }

    #[tokio::test]
    async fn registry_lifecycle() {
        let registry = MediaRegistry::new();
        let stream = registry
            .create(input("rtsp://203.0.113.10/main"))
            .await
            .unwrap();
        assert_eq!(stream.id, "cam-1");
        assert!(registry.get("cam-1").await.is_some());
        assert_eq!(registry.list().await.len(), 1);
        assert!(registry
            .create(input("rtsp://203.0.113.11/main"))
            .await
            .unwrap_err()
            .to_string()
            .contains("already exists"));
        registry.delete("cam-1").await.unwrap();
        assert!(registry.get("cam-1").await.is_none());
    }
}
