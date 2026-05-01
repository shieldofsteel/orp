# ORP Real-Time Media Control Plane

ORP v0.3.x now has a Rust-native media stream registry and an in-binary relay
for HTTP-family media. It can register camera/video sources, validate risky
source URLs, redact embedded credentials, expose stream inventory over the REST
API, project every stream into the ORP entity graph as a `media_stream`, and
relay practical browser-consumable media without a sidecar service.

This is not yet a full `go2rtc` replacement. ORP does not currently packetize
RTSP to WebRTC or negotiate codecs in the way a dedicated media router does.
The useful capability that exists now is direct binary relay for HTTP/JPEG,
MJPEG, HTTP/FLV-like byte streams, and HLS playlists/segments.

## Supported Source Schemes

The registry accepts these source families:

- `rtsp://`, `rtsps://`
- `rtmp://`, `rtmps://`
- `http://`, `https://` for HLS, MJPEG, JPEG snapshots, and HTTP/FLV-like feeds
- `hls://`, `mjpeg://`, `jpeg://`
- `webrtc://`, `whep://`
- `srt://`
- `onvif://`
- `v4l2://`, `usb://`
- `file://`
- `klv://`, `klv-ts://`

Dangerous executable-style schemes such as `exec://`, `shell://`, `pipe://`,
`stdin://`, and `echo://` are rejected by design.

## API

Create a stream:

```bash
curl -X POST http://localhost:9090/api/v1/media/streams \
  -H 'content-type: application/json' \
  -H "X-API-Key: $ORP_API_KEY" \
  -d '{
    "id": "gate-cam",
    "name": "Gate Camera",
    "source_url": "rtsp://admin:secret@10.0.0.5:554/main",
    "allow_private_network": true,
    "expected_codec": "h264",
    "klv_metadata": true,
    "labels": ["perimeter", "north-gate"]
  }'
```

The response redacts credentials:

```json
{
  "data": {
    "id": "gate-cam",
    "name": "Gate Camera",
    "protocol": "rtsp",
    "source_url_redacted": "rtsp://<redacted>@10.0.0.5:554/main",
    "allow_private_network": true,
    "expected_codec": "h264",
    "klv_metadata": true,
    "labels": ["perimeter", "north-gate"],
    "status": "relay_ready",
    "capabilities": {
      "low_latency_view": true,
      "browser_fallback": true,
      "in_binary_relay": true,
      "metadata_fusion": true,
      "two_way_audio_candidate": true
    }
  }
}
```

Relay a JPEG, MJPEG, or HTTP/FLV-like stream:

```bash
curl http://localhost:9090/api/v1/media/streams/gate-cam/relay \
  -H "X-API-Key: $ORP_API_KEY" \
  --output frame-or-stream.bin
```

Relay an HLS playlist:

```bash
curl http://localhost:9090/api/v1/media/streams/gate-cam/playlist.m3u8 \
  -H "X-API-Key: $ORP_API_KEY"
```

ORP rewrites relative HLS segment/key URIs so clients fetch media chunks back
through ORP:

```text
/api/v1/media/streams/gate-cam/hls/fetch?url=https%3A%2F%2Fcamera.example%2Flive%2Fseg-1.ts
```

List streams:

```bash
curl http://localhost:9090/api/v1/media/streams -H "X-API-Key: $ORP_API_KEY"
```

Get one stream:

```bash
curl http://localhost:9090/api/v1/media/streams/gate-cam -H "X-API-Key: $ORP_API_KEY"
```

Delete a stream:

```bash
curl -X DELETE http://localhost:9090/api/v1/media/streams/gate-cam \
  -H "X-API-Key: $ORP_API_KEY"
```

## Graph Projection

Registering a stream creates a `media_stream` entity with properties such as:

- `media_protocol`
- `source_url` (credential-redacted)
- `status`
- `allow_private_network`
- `expected_codec`
- `klv_metadata`
- `labels`

That means streams can be searched, audited, federated later, and related to
ships, aircraft, sensors, sites, missions, or KLV metadata tracks using the
same ORP entity and relationship model as every other source.

## Security Defaults

LAN cameras usually live on private IP ranges, but registering private/local
addresses through a remote API is a real SSRF footgun. ORP therefore rejects
literal private/local IP source URLs unless the caller sets
`allow_private_network=true`.

The registry also rejects executable source schemes. A future relay may support
operator-approved FFmpeg/GStreamer pipelines, but those should live behind an
explicit allowlist and never be accepted as arbitrary API input.

## Data-Plane Roadmap

The next media tranche should add the lower-level camera transports:

- RTSP demux/remux for H.264/H.265/AAC/PCMA/PCMU.
- WebRTC/WHEP output for low-latency browser viewing.
- ONVIF discovery and profile import.
- KLV sidecar extraction into existing MISB ST 0601 parser paths.
- Per-stream packet/byte/session stats in `/api/v1/health`.
- Optional FFmpeg handoff for transcoding, behind a strict binary/path allowlist.
