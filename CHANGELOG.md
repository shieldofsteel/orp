# Changelog

All notable changes to ORP are documented here.

This project follows [Semantic Versioning](https://semver.org/) and [Conventional Commits](https://www.conventionalcommits.org/).

---

## [Unreleased] — 2026-05-01 audit/fix wave

### New crate

- **`orp-ml`** — first ML seam in ORP. Exposes an `AnomalyScorer` trait so any model (rule-based, statistical, deep) plugs into the same hot path. Ships with `NullScorer`, `OnlineQuantileScorer` (rolling-p99.5 baseline), and a small in-house `IsolationForestScorer` (~275 LoC, `bincode` model load) — no heavyweight ML dep. `crates/orp-stream/src/processor.rs` calls the scorer in `upsert_entity`; non-`NullScorer` results land on entities as `ml_anomaly_score: f32` + `ml_model_id: String` and **augments** (does not replace) the rule-based score. The default `NullScorer` is a true no-op — no properties are written — so storage isn't bloated by zero scores from a disabled model.

### New connector

- **MAVLink v2** — drone telemetry adapter at `crates/orp-connector/src/adapters/mavlink.rs`. UDP listener, `mavlink://0.0.0.0:14550` URI scheme, decodes HEARTBEAT, GLOBAL_POSITION_INT, VFR_HUD, ATTITUDE, GPS_RAW_INT, SYS_STATUS. Per-vehicle entity dedup via `(system_id, component_id)`. The single biggest "be a real Lattice/Maven peer" win in the connector subsystem — every PX4/ArduPilot/Skydio/Auterion ground station now interoperates.

### Connector capability expansions

- **GRIB Section 7 (data unpacking)** — `crates/orp-connector/src/adapters/grib.rs` now unpacks Data Representation Template 5.0 (simple packing) per the WMO formula `Y = (R + X * 2^E) / 10^D` (the binary-scale `E` may be negative, so it's a real exponentiation, not a left-shift). GRIB messages now carry actual weather values, not just metadata. Templates other than 5.0 still return metadata-only with a warning rather than failing.
- **Universal-ingest CSV** — `csv_watcher.rs` switched from naive `line.split(',')` to the `csv` crate. Quoted fields containing commas (`"Doe, John",51.5,-0.1`) parse correctly instead of being silently dropped.
- **NFFI track-id collision fix** — `nffi.rs` no longer falls back to index-based IDs (`track-0`, `track-1`) for unnamed tracks. Synthesises a stable hash of `(name, lat, lon, affiliation)` so two distinct unnamed tracks don't merge during entity resolution.

### Storage / Ops

- **Persistent storage by default.** `commands.rs::run_start` now reads `config.storage.duckdb.path` and calls `DuckDbStorage::new_with_path`; entities, events, audit log all survive process restarts. New `--in-memory` flag opts back into the old behaviour for tests/demos.
- **Graph engine cached** in `DuckDbStorage` via `OnceLock<Arc<GraphEngine>>` — DROP/CREATE VIEW × 19 now runs once per storage handle, not once per `graph_query` call (was the worst single perf bug identified by the perf audit).
- **Forgiving config schema.** `#[serde(default)]` on `ServerConfig`, `StorageConfig`, `DuckDbConfig`, `KuzuConfig`, `RocksDbConfig`, `SqliteConfig`. A 4-line `config.yaml` now boots cleanly instead of demanding all 30 fields.
- **`/health` now returns `graph_engine`** component (was missing from the response despite being declared in `openapi.yaml`).

### Security hardening

- **CSPRNG everywhere.** `crates/orp-audit/src/crypto.rs` (Ed25519 audit signer) and `crates/orp-security/src/api_keys.rs` (API key generation) swapped from `rand::thread_rng()` to `rand::rngs::OsRng`. Reproducible/predictable keys would have been a real attack on tamper-evidence and key enumeration.
- **SSRF guard** on `crates/orp-connector/src/adapters/http_poller.rs`. Loopback / RFC1918 / link-local / 100.64/10 (CGNAT) / cloud-metadata hosts are blocked unless the connector opts in via `allow_private_targets = true`. 6 new unit tests.
- **JWT hardening.** Claims now carry a required `nbf`; `validate_token` enables `validate_nbf`, requires `["exp", "iss", "aud", "sub"]`, and honours a configurable `leeway_seconds` (default 60s). Algorithm pinning was already in.
- **OIDC discovery TTL cache.** `OidcClient` now caches the discovery document with a configurable TTL (default 1 h, env `ORP_OIDC_DISCOVERY_TTL_SECS`). On refresh failure the still-cached doc is returned with a warning rather than failing closed.
- **Dev-mode safety belt.** `ORP_DEV_MODE=true` is honoured only when `ORP_ENV` is unset / `development` / `dev` / `test` / `ci`. In any other environment, permissive auth is **refused** with a loud error log so leaking dev env into prod doesn't open the front door.
- **Database connector SQL safety contract.** `QueryExecutor` trait carries an explicit safety contract; new `validate_query_template` rejects `${watermark}` / `{watermark}` / `%(watermark)s` / `<watermark>` placeholders at connector start so accidental string-interpolation can't slip through.

### Federation

- **Adaptive backoff + per-peer scheduling.** `spawn_federation_sync` no longer sleeps a global 30 s. Each peer has its own next-fire instant; on success we reset to `ORP_FED_BASE_INTERVAL_SECS` (default 30 s), on failure we double up to `ORP_FED_MAX_INTERVAL_SECS` (default 600 s). Flapping satellite/4G uplinks no longer burn bandwidth and CPU.

### Performance / edge

- **`ORP_TRACK_LEN` env var** + default 50 (down from 500) for `AnalyticsEngine`. At 100K entities the in-memory track buffer drops from ~2.4 GB to ~240 MB — Pi-class deployment is real, not aspirational.

### Frontend

- **Canvas mock** in `frontend/src/test-setup.ts` so Leaflet stops throwing in jsdom.
- **`useWebSocket` tests un-skipped** — `describe.skip` removed from all four blocks; the existing `MockWebSocket` was already correct. **All 19 tests now pass.**
- **`vite.config.ts` `manualChunks`** for `react-vendor` / `leaflet-vendor` / `data-vendor`; `chunkSizeWarningLimit: 250`.

### Docs

- **Brand unified** to "Open Reality Protocol" across README, openapi.yaml, both SDKs (was three different names: "Open Reality Protocol", "Object Relationship Platform", "Open Relationship Protocol").
- **License unified** to Apache-2.0 across `sdk/python/setup.py`, both SDK READMEs, and JS package.json (three places previously claimed MIT).
- **Install URL fixed.** README and `docs/GETTING_STARTED.md` no longer point at the 404 `https://orp.dev/install` for first-time users.
- **`protoc` listed as a prereq** with brew/apt/dnf/pacman commands.
- **Kuzu sweep.** README, ARCHITECTURE, CHANGELOG, REQUIREMENTS, openapi.yaml, docs/* all rewritten to describe the actual implementation: a DuckDB-backed graph projection with an in-memory BFS executor. ADR-001 in ARCHITECTURE.md now documents this and reserves a `--features kuzu-graph` Cargo flag for the day a real customer hits a billion-edge / depth-5+ workload.
- **OpenAPI** rate-limit corrected (1000/sec doc → 100/sec implementation), `/query/natural` documented as 501 Not Implemented (Phase 2 roadmap), `/graph` description updated.
- **CI workflow branch filter** `[main, develop]` → `[master, main, develop]` so PRs against the actual default branch are gated.

### Project Stats (verified 2026-05-01)

- 13 crates (added `orp-ml`)
- 34 protocol adapters (added MAVLink)
- 1,122 backend tests passing across the workspace (orp-connector 547 / orp-core 154 / orp-stream 93 / orp-security 80 / orp-storage 53 / orp-ml 17 / cross-crate 178 + small crates), 0 failing.
- 45 MB stripped release binary (Mach-O arm64) — verified, persistence test passed end-to-end.

---

## [0.2.0-alpha] — 2026-03-27

### New Protocol Adapters (15 new parsers)

- **ACARS** — Aircraft Communications Addressing and Reporting System data link. Decodes ACARS messages from VHF ground stations or satellite feeds; maps flight ID, registration, message label, and payload to ORP entities.
- **BACnet** — Building Automation and Control Networks (ASHRAE 135). Reads device objects, analog/binary values, and trend logs from BACnet/IP gateways. Enables facility sensors in the ORP knowledge graph.
- **GRIB** — WMO Gridded Binary weather model data (GRIB 1 and GRIB 2). Ingests NWP forecast grids (wind, pressure, temperature) and creates geospatial `WeatherGrid` entities with valid-time metadata.
- **CEF** — ArcSight Common Event Format. Parses CEF syslog frames into structured security events; maps severity, device vendor, and extension fields to ORP `ThreatEvent` entities.
- **LoRaWAN** — Long-range IoT network frames via ChirpStack/TTN REST API. Decodes device EUI, payload bytes, RSSI, SNR, and GPS coordinates from LoRa sensor uplinks.
- **NMEA 2000 / N2K** — Modern CAN-bus marine protocol via YDWG-02 or similar gateway (serial/UDP). Parses PGNs for vessel position, speed, heading, depth, wind, and engine data.
- **NFFI** — NATO Friendly Force Information (APP-6 symbology). Decodes NFFI XML track messages including unit identity, SIDC symbol code, speed, heading, and operational status.
- **SparkplugB** — Industrial MQTT payload specification (Eclipse Tahu). Parses NBIRTH, DBIRTH, NDATA, DDATA payloads into structured metric entities.
- **DNP3** — Distributed Network Protocol 3 for utility SCADA and substations. Reads analog inputs, binary inputs, counters, and control outputs from DNP3 outstations.
- **CAN / CANbus** — CAN 2.0A/B frame capture via SocketCAN (Linux) and peak/kvaser interfaces. J1939 PGN decoding for vehicle telemetry.
- **PCAP** — Packet capture (.pcap / .pcapng) replay and live capture (libpcap). Extracts IP flows, DNS queries, and HTTP metadata into network entity objects.
- **Zeek** — Zeek (formerly Bro) network security monitor log ingestion. Parses conn.log, dns.log, http.log, ssl.log, and notice.log into ORP threat and host entities.
- **NetFlow / IPFIX** — Cisco NetFlow v5/v9 and IPFIX flow telemetry via UDP collector. Maps flow src/dst, bytes, packets, and AS numbers to network entities.
- **METAR** — Aviation Routine Weather Report. Parses METAR and SPECI strings from NOAA/AVIMET feeds; creates `WeatherStation` entities with decoded present weather, visibility, and altimeter.
- **GTFS-RT** — General Transit Feed Specification Realtime (Protocol Buffers). Ingests VehiclePositions, TripUpdates, and ServiceAlerts from any GTFS-RT feed URL.

### Security Audit Fixes

- **Rate limiter**: Moved from 1,000 req/sec (openapi.yaml description) to the actual implementation of 100 tokens/sec per IP with token-bucket refill. Documentation updated to reflect true limits.
- **CORS**: Replaced wildcard `Any` origin with explicit allowlist from `ORP_CORS_ORIGINS` environment variable. Fallback is `http://localhost:3000` only — not `*`.
- **Ed25519 signing**: Audit signer is now always initialized (fresh keypair generated if none provided in `ServerConfig`). Events from unsigned connectors receive `low_confidence` flag rather than being silently accepted.

### Project Stats (as of this release)

- **Crates:** 12
- **Rust source files:** 85
- **Lines of Rust:** 51,641
- **Tests:** 764 passing
- **Clippy warnings:** 0
- **Protocol adapters:** 32
- **Git commits:** 50
- **Binary (core):** 43 MB
- **License:** Apache 2.0

---

## [0.1.0-alpha] — 2026-03-26

Initial alpha release of ORP — Open Reality Protocol.

This release delivers a complete, working single-binary data fusion platform: real-time maritime and aircraft tracking, live knowledge graph, ORP-QL query language, OIDC authentication, ABAC authorization, Ed25519 event signing, hash-chained audit logging, and an embedded React console.

### New Features

#### Core Architecture

- **Single-binary deployment** — all components (connectors, stream processor, storage engines, API server, frontend) compile into one self-contained binary. No Docker, no external databases, no configuration required for a basic start.
- **Tokio async runtime** — full async/await throughout; handles 100K+ events/sec on commodity hardware.
- **12-crate workspace** — `orp-core`, `orp-proto`, `orp-config`, `orp-connector`, `orp-stream`, `orp-entity`, `orp-storage`, `orp-query`, `orp-security`, `orp-audit`, `orp-geospatial`, `orp-testbed`.
- **Axum HTTP server** — Tower middleware stack with structured logging, compression, timeout, CORS, and rate limiting.

#### Connectors (`orp-connector`)

- **AIS maritime connector** — NMEA 0183 over TCP. Sustained throughput: 30K events/sec. Supports multiple simultaneous AIS feeds (AISHub, personal receiver). Parses position reports (Type 1/2/3), voyage data (Type 5), and base station reports.
- **ADS-B aircraft connector** — SBS-1 (BaseStation) format over TCP. 1K events/sec. Decodes ICAO24, callsign, altitude, position, ground speed, track.
- **HTTP polling connector** — generic REST → ORP entities. Configurable polling interval, JSON path extraction, custom entity type mapping. Supports API key and Bearer token auth.
- **MQTT sensor connector** — subscribes to any MQTT broker topic. Maps message payloads to ORP entity properties.
- **NOAA weather connector** — polls NOAA weather alerts API every 10 minutes. Creates `WeatherSystem` entities with severity zones and expiry times.
- **OpenStreetMap connector** — bootstraps port, harbor, and anchorage geometry from OSM Overpass API on startup.
- **Connector trait** (`Connector`) — public API for implementing custom connectors. Includes health reporting, metrics, and graceful stop.
- **Connector supervisor** — each connector runs in its own Tokio task. Panics are caught; connector restarts with exponential backoff without taking down the binary.
- **Ed25519 signing** — every connector signs each event with an Ed25519 key at the point of ingestion. Signature is verified by the stream processor and stored in the audit log.

#### Stream Processing (`orp-stream`)

- **Deduplication** — RocksDB-backed 24-hour dedup window. SHA-256 event hash; duplicate events are dropped and logged.
- **Change detection** — compares incoming events against cached entity state. Only genuine state changes propagate to storage and WebSocket fanout.
- **Batch insert pipeline** — accumulates events into batches of 1,000 (or 1-second flush) before DuckDB write. Achieves > 100K events/sec with < 1% CPU overhead per core.
- **Connector checkpointing** — RocksDB persists byte offsets and sequence numbers. Binary restarts resume from last checkpoint without reprocessing.
- **Entity resolution** (`orp-entity`) — structural matching merges events from multiple connectors that describe the same real-world entity (e.g., same vessel from two AIS feeds).

#### Storage Layer (`orp-storage`)

- **DuckDB integration** — embedded columnar OLAP engine. Handles geospatial queries (RTREE index), temporal scans, and aggregate analytics.
- **Core tables** — `entities`, `entity_geometry`, `entity_properties`, `events`, `relationships`, `data_sources`, `audit_log`.
- **DuckDB graph projection** — `graph_nodes` and `graph_edges` tables held inside DuckDB plus an in-memory adjacency list (`crates/orp-storage/src/graph_engine.rs`) serve as the property graph. Ships, ports, aircraft, weather systems, and organizations are nodes; relationships (HEADING_TO, OWNS, THREATENS, NEAR, etc.) are edges. (See ADR-001 — a `kuzu-graph` Cargo feature is reserved for billion-edge workloads.)
- **Graph projection refresh** — background task rebuilds the projection tables and in-memory adjacency from the canonical entity/relationship tables every 30 seconds.
- **RocksDB stream state** — dedup window, entity state cache, connector checkpoints. Survives binary restarts.
- **Storage trait** — unified abstraction over the DuckDB engine and the graph projection. Query engine routes through this trait; storage backends are swappable in tests.

#### Query Engine (`orp-query`)

- **ORP-QL v0.1** — purpose-built query language. LALRPOP-generated parser. SQL-style filtering combined with Cypher-style MATCH patterns.
- **Supported syntax** — `MATCH`, `WHERE`, `RETURN`, `ORDER BY`, `LIMIT`, `GROUP BY`, `AT TIME` (temporal), `near()`, `within()`, `bbox()`, `point()`, `distance()`, `interval()`.
- **Query planner** — routes queries to DuckDB (geospatial, analytics, temporal) or the graph projection's BFS executor (graph traversal) based on query shape. Hybrid queries use both with a Rust-level result merge.
- **Query cache** — identical queries within a 30-second window return cached results without hitting storage.
- **Cypher-style passthrough** — `POST /api/v1/graph` accepts Cypher-style traversal queries that execute against the DuckDB graph projection.
- **P50 latency** — simple queries < 200 ms; 3-hop graph queries < 1 s.

#### API Layer (`orp-core`)

- **REST API** — `/api/v1/entities`, `/api/v1/query`, `/api/v1/graph`, `/api/v1/connectors`, `/api/v1/monitors`, `/api/v1/alerts`, `/api/v1/health`, `/api/v1/metrics`.
- **WebSocket** — `/ws/updates`. Clients subscribe with bbox + entity type filters. Server fans out only matching entity updates with ABAC filtering per client. Ping/pong keepalive.
- **Prometheus metrics** — `/api/v1/metrics`. Connector throughput, query latency histograms, storage health, stream processor lag.
- **Embedded frontend** — React SPA bundled into the binary via `include_dir!`. No separate web server needed.

#### Security (`orp-security`, `orp-audit`)

- **OIDC authentication** — full authorization code flow with PKCE. Compatible with Keycloak, Auth0, Dex, Okta, Google, Microsoft Entra ID. JWT validation with cached JWKS (refreshed every hour).
- **ABAC authorization** — per-request policy evaluation. Subject attributes (user permissions, clearance, org), resource attributes (entity sensitivity, org), and environment attributes. < 10 ms overhead per request (cached after first evaluation per token).
- **API key authentication** — scoped API keys for non-interactive clients. Per-key permission set and optional expiry.
- **Ed25519 event signing** — connectors sign events at the source with Ed25519 keypairs. Stream processor verifies; low-confidence flag on invalid signatures.
- **Hash-chained audit log** — every API action and significant system event written to `audit_log` with SHA-256 chain. Tamper detection via `orp verify --audit-log`.
- **Cryptographic erasure** — GDPR Article 17 support. Per-entity DEK encrypted with master key. Erasure destroys the DEK; ciphertext remains but is permanently unrecoverable.
- **Rate limiting** — token bucket per client IP / API key. Configurable limits with standard `X-RateLimit-*` response headers.
- **No telemetry** — zero unsolicited outbound network connections.

#### Frontend

- **Deck.gl 2D live map** — ScatterplotLayer, IconLayer, HeatmapLayer, PathLayer. LOD rendering: heatmap at low zoom, dots at medium zoom, ship silhouettes at high zoom. 50K entities at 60 fps.
- **CesiumJS 3D globe** (beta) — 3D terrain rendering for situational awareness use cases.
- **Entity inspector** — click any entity for full property list (with confidence/freshness), Cytoscape.js relationship mini-graph, and event history timeline.
- **Query bar** — ORP-QL input with syntax highlighting and autocomplete. Query history (last 50 queries). Results highlighted on map.
- **Alert feed** — real-time rule-triggered anomaly notifications. Speed threshold, geofence deviation, weather proximity.
- **Timeline scrubber** — drag to replay any past 24-hour state. Temporal queries executed against historical events table.
- **Connector status sidebar** — live health indicators and throughput metrics per connector.

#### Configuration & Templates

- **YAML configuration** — single `~/.orp/config.yaml`. Environment variable substitution (`${env.KEY}`). Secrets never stored in config files.
- **Templates** — pre-built configurations for common domains:
  - `maritime` — AIS ships + NOAA weather + OSM ports
  - `adsb` — ADS-B aircraft tracking
  - `supply-chain` — cargo tracking template
  - `climate` — weather + shipping correlation
  - `custom` — blank slate

#### Tooling

- **`orp start`** — launch with config or template; browser opens automatically
- **`orp query`** — run ORP-QL queries from the CLI and print results
- **`orp connector list`** — show all registered connectors and health
- **`orp verify`** — verify audit log chain integrity or event signatures
- **`orp keygen`** — generate Ed25519 signing keypairs for connectors

### Performance

| Metric | Achieved |
|--------|----------|
| Binary size (core Rust) | 43 MB |
| Cold start → HTTP ready | < 4 s |
| Simple query P50 | ~150 ms |
| Stream throughput | 100K+ events/sec |
| Map rendering (50K entities) | 60 fps |
| Tests passing | 203 (764 as of 0.2.0-alpha) |
| Clippy warnings | 0 |

### Known Limitations (Alpha)

- Natural language queries are not yet implemented (Phase 2).
- WASM plugin system for custom connectors is not yet implemented (Phase 2).
- Horizontal clustering / multi-node deployment is not yet implemented (Phase 3).
- CesiumJS 3D globe is in beta and may have rendering inconsistencies on some GPUs.
- The graph projection refresh runs every 30 seconds. Graph queries on very recently updated entities may reflect slightly stale state during that window.
- The `supply-chain` and `climate` templates are functional but use placeholder connector configs that require manual customization.

### Project Stats

- **Crates:** 12
- **Rust source files:** 53 (85 as of 0.2.0-alpha)
- **Lines of Rust:** ~17,000 (51,641 as of 0.2.0-alpha)
- **Tests:** 203 passing (764 as of 0.2.0-alpha)
- **Binary (core):** 43 MB
- **License:** Apache 2.0

---

_All notable future changes will be documented here. Follow [GitHub Releases](https://github.com/orproject/orp/releases) for announcements._
