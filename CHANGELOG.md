# Changelog

All notable changes to ORP are documented here.

This project follows [Semantic Versioning](https://semver.org/) and [Conventional Commits](https://www.conventionalcommits.org/).

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
- **Kuzu graph store** — embedded property graph database. Ships, ports, aircraft, weather systems, and organizations stored as nodes; relationships (HEADING_TO, OWNS, THREATENS, NEAR, etc.) as edges.
- **DuckDB → Kuzu sync** — background task syncs entity and relationship changes to Kuzu every 30 seconds.
- **RocksDB stream state** — dedup window, entity state cache, connector checkpoints. Survives binary restarts.
- **Storage trait** — unified abstraction over DuckDB + Kuzu. Query engine routes through this trait; storage backends are swappable in tests.

#### Query Engine (`orp-query`)

- **ORP-QL v0.1** — purpose-built query language. LALRPOP-generated parser. SQL-style filtering combined with Cypher-style MATCH patterns.
- **Supported syntax** — `MATCH`, `WHERE`, `RETURN`, `ORDER BY`, `LIMIT`, `GROUP BY`, `AT TIME` (temporal), `near()`, `within()`, `bbox()`, `point()`, `distance()`, `interval()`.
- **Query planner** — routes queries to DuckDB (geospatial, analytics, temporal) or Kuzu (graph traversal) based on query shape. Hybrid queries use both engines with Rust-level result merge.
- **Query cache** — identical queries within a 30-second window return cached results without hitting storage.
- **Cypher passthrough** — `POST /api/v1/graph` allows raw Kuzu Cypher queries for power users.
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
- DuckDB → Kuzu sync has an eventual consistency window of up to 30 seconds. Graph queries on very recently updated entities may reflect slightly stale state.
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
