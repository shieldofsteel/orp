# ORP Handoff — 2026-05-01 Media Relay Continuation

## Current Git State

- Repo: `shieldofsteel/orp`
- Default GitHub branch: `master`
- Current pushed commit: `1921e5e feat(media): add in-binary HTTP media relay`
- Local branch at handoff: `master`
- Working tree after push: clean

The older handoff at:

```text
~/.claude/projects/-Users-grey-orp/memory/handoff_2026_05_01_v0.3.0_for_next_agent.md
```

is now stale. It described `feat/v0.3.0-rc2`, but this checkout is already on
`master` past `v0.3.0-rc3`.

## What Was Added

ORP now has an actual in-binary media relay foundation, not only a registry.

Main files:

- `crates/orp-core/src/server/media.rs`
- `crates/orp-core/src/server/handlers.rs`
- `crates/orp-core/src/server/http.rs`
- `docs/MEDIA.md`
- `README.md`

Capabilities now present:

- Register media streams as first-class ORP objects.
- Redact credentials from media source URLs in API responses and graph properties.
- Require explicit `allow_private_network=true` for literal private/local IP camera URLs.
- Reject dangerous executable source schemes such as `exec://`, `shell://`, `pipe://`.
- Project streams into the entity graph as `media_stream` entities.
- Relay HTTP-family media directly through the ORP binary:
  - JPEG snapshots
  - MJPEG / long-lived HTTP byte streams
  - HTTP/FLV-like byte streams
  - HLS playlists
  - HLS segment/key fetches
- Rewrite HLS playlist segment/key URLs back through ORP.
- Enforce same-origin HLS asset fetches so a playlist cannot turn ORP into an open proxy.

New API routes:

```text
GET    /api/v1/media/streams
POST   /api/v1/media/streams
GET    /api/v1/media/streams/{id}
DELETE /api/v1/media/streams/{id}
GET    /api/v1/media/streams/{id}/relay
GET    /api/v1/media/streams/{id}/playlist.m3u8
GET    /api/v1/media/streams/{id}/hls/fetch?url=<encoded-segment-url>
```

## Important Caveat

ORP still does not fully compete with `go2rtc` as a media router.

`go2rtc` has mature RTSP/WebRTC/HLS/MJPEG routing, two-way audio, codec
negotiation, ingest, publish, FFmpeg handoff, and stream stats. ORP now has the
secure control plane plus real HTTP-family relay, but not RTSP demux/remux or
WebRTC/WHEP data-plane output yet.

## Extra Fix Included

`crates/orp-stream/src/sanctions.rs` had a real performance bug: exact-name
queries still fell into broad trigram/Levenshtein scanning. This caused
`sanctions::tests::test_load_under_concurrent_load` to hang during the inherited
full-suite run. The exact-name index is now used as a fast path before fuzzy
matching.

## Validation Run Before Push

All passed:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=/Volumes/Sony/orp-target/integrator \
  CARGO_BUILD_INCREMENTAL=false \
  cargo clippy --all --all-features --tests -- -D warnings
CARGO_TARGET_DIR=/Volumes/Sony/orp-target/integrator \
  CARGO_BUILD_INCREMENTAL=false \
  cargo test --workspace --all-features --no-fail-fast
cargo audit
```

`cargo audit` still reports the existing allowed warnings:

- `RUSTSEC-2025-0141` — `bincode` unmaintained
- `RUSTSEC-2025-0134` — `rustls-pemfile` unmaintained

Both are allowed by the current audit configuration / existing dependency state.

## Next Best Work

The next agent should continue with actual data-plane capability, in this order:

1. RTSP ingest/demux for H.264/H.265/AAC/PCMA/PCMU.
2. RTSP remux/proxy output from ORP.
3. WebRTC/WHEP output for browser-low-latency viewing.
4. ONVIF discovery/profile import.
5. Per-stream byte/session/error stats in `/api/v1/health`.
6. Optional FFmpeg handoff behind a strict binary/path allowlist.
7. KLV sidecar extraction from MPEG-TS into the existing MISB ST 0601 parser.

Do not claim ORP is a full `go2rtc` replacement until at least RTSP input and
WebRTC/WHEP output are implemented and tested end to end.
