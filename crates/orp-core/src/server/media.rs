use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::net::IpAddr;
use tokio::sync::RwLock;

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

#[derive(Default)]
pub struct MediaRegistry {
    streams: RwLock<HashMap<String, MediaStream>>,
}

impl MediaRegistry {
    pub fn new() -> Self {
        Self::default()
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
        guard.insert(stream.id.clone(), stream.clone());
        Ok(stream)
    }

    pub async fn list(&self) -> Vec<MediaStream> {
        let mut streams: Vec<_> = self.streams.read().await.values().cloned().collect();
        streams.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        streams
    }

    pub async fn get(&self, id: &str) -> Option<MediaStream> {
        self.streams.read().await.get(id).cloned()
    }

    pub async fn delete(&self, id: &str) -> Result<MediaStream, MediaRegistryError> {
        self.streams
            .write()
            .await
            .remove(id)
            .ok_or_else(|| MediaRegistryError::NotFound(format!("media stream '{id}' not found")))
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
    matches!(
        protocol,
        MediaProtocol::Hls | MediaProtocol::HttpFlv | MediaProtocol::Mjpeg | MediaProtocol::Jpeg
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
    let mut out = String::with_capacity(playlist.len() + 256);
    for line in playlist.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }

        if trimmed.starts_with("#EXT-X-KEY") || trimmed.starts_with("#EXT-X-MAP") {
            out.push_str(&rewrite_hls_uri_attribute(stream_id, source_url, line)?);
            out.push('\n');
            continue;
        }

        if trimmed.starts_with('#') {
            out.push_str(line);
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

fn rewrite_hls_uri_attribute(
    stream_id: &str,
    source_url: &str,
    line: &str,
) -> Result<String, MediaRegistryError> {
    let Some(uri_start) = line.find("URI=\"") else {
        return Ok(line.to_string());
    };
    let value_start = uri_start + 5;
    let Some(value_end_rel) = line[value_start..].find('"') else {
        return Ok(line.to_string());
    };
    let value_end = value_start + value_end_rel;
    let resolved = resolve_same_origin_media_url(source_url, &line[value_start..value_end])?;
    let relay_uri = format!(
        "/api/v1/media/streams/{stream_id}/hls/fetch?url={}",
        percent_encode_component(&resolved)
    );
    Ok(format!(
        "{}{}{}",
        &line[..value_start],
        relay_uri,
        &line[value_end..]
    ))
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
        assert!(!stream.capabilities.in_binary_relay);
        assert_eq!(stream.status, MediaStreamStatus::NeedsRelay);
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
