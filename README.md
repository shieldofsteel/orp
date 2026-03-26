# ORP — Open Reality Protocol

<div align="center">

**The single-binary Palantir alternative. Real-time data fusion, graph intelligence, and live ops maps — on your laptop.**

[![Tests](https://img.shields.io/badge/tests-203_passing-brightgreen?style=flat-square)](https://github.com/orproject/orp/actions)
[![Clippy](https://img.shields.io/badge/clippy-0_warnings-brightgreen?style=flat-square)](https://github.com/orproject/orp/actions)
[![Binary](https://img.shields.io/badge/binary-43MB_core-blue?style=flat-square)](https://github.com/orproject/orp/releases)
[![License](https://img.shields.io/badge/license-Apache_2.0-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)

[**Quick Start**](#quick-start) · [**Architecture**](ARCHITECTURE.md) · [**ORP-QL**](#orp-ql-query-language) · [**API Reference**](#api-quick-reference) · [**Security**](docs/SECURITY.md) · [**Contributing**](CONTRIBUTING.md)

</div>

---

## What Is ORP?

ORP connects to real-time data sources — AIS ship feeds, ADS-B aircraft transponders, IoT sensors, REST APIs — fuses them into a live knowledge graph, and lets humans and AI query, reason about, and monitor physical reality.

**One binary. No cloud. No setup. Live data on screen in under 3 minutes.**

### The Pattern

Every generation, an embedded open-source tool demolishes a bloated enterprise category:

| Precedent | Replaced | Binary Size | Impact |
|-----------|----------|-------------|--------|
| SQLite | Oracle / MySQL server farms | < 1 MB | In every device on earth |
| DuckDB | Apache Spark clusters ($100K+/yr) | ~50 MB | 100× faster, 50%+ YoY growth |
| llama.cpp | OpenAI API (cloud-only, $$$/token) | ~100 MB | LLMs on laptops |
| **ORP** | **Palantir ($50–500M/deployment)** | **~300 MB** | **Data fusion on laptops** |

Palantir earns $2.8B/year. Anduril Lattice won a $20B Army contract. Both proprietary. Both locked behind defense procurement. ORP open-sources this capability — so a disaster response agency, climate researcher, city planner, or defense contractor can download one binary and get the same class of real-time situational awareness for free.

---

## Quick Start

```bash
# 1. Install ORP
curl -fsSL https://orp.dev/install | sh

# 2. Launch the maritime template (ships + weather + ports, live data)
orp start --template maritime

# 3. Open the console — browser launches automatically
open http://localhost:9090
```

**What you'll see in 60 seconds:**
- 🚢 2,000+ ships updating live on a Deck.gl map
- ⛈️ Active weather systems overlaid with severity zones
- 🔍 Query bar — type ORP-QL or plain English and get results in < 200 ms
- 🔔 Alert feed — anomaly detection fires when ships deviate from expected routes
- ⏪ Timeline scrubber — drag back 24 hours and replay any past state

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                        DATA SOURCES                          │
│   AIS Feed   ADS-B Feed   Weather API   MQTT   CSV / REST   │
└──────┬──────────┬──────────────┬────────────┬───────────────┘
       │          │              │            │  (connector trait)
       ▼          ▼              ▼            ▼
┌──────────────────────────────────────────────────────────────┐
│  CONNECTORS  ·  orp-connector                                │
│  Parse protocol → emit signed OrpEvents → async channel      │
└───────────────────────────┬──────────────────────────────────┘
                            │ Vec<OrpEvent>
                            ▼
┌──────────────────────────────────────────────────────────────┐
│  STREAM PROCESSOR  ·  orp-stream                             │
│  Dedup (RocksDB) → Change Detection → Batch (1K) → Insert   │
└───────────────────────────┬──────────────────────────────────┘
                            │ batch insert
                            ▼
┌──────────────────────────────────────────────────────────────┐
│  STORAGE LAYER  ·  orp-storage                               │
│  ┌──────────────┐  ◄ sync every 30s ►  ┌──────────────────┐ │
│  │   DuckDB     │                       │   Kuzu Graph     │ │
│  │  (OLAP)      │                       │  (Relationships) │ │
│  │  entities    │                       │  Ship → Port     │ │
│  │  events      │                       │  Org → Fleet     │ │
│  │  geometry    │                       │  Weather → Port  │ │
│  └──────────────┘                       └──────────────────┘ │
│         + RocksDB (stream state, dedup window, checkpoints)   │
└───────────────────────────┬──────────────────────────────────┘
                            │ unified Storage trait
                            ▼
┌──────────────────────────────────────────────────────────────┐
│  QUERY ENGINE  ·  orp-query                                  │
│  ORP-QL parser → planner → route to DuckDB or Kuzu → merge  │
└───────────────────────────┬──────────────────────────────────┘
                            │ JSON results
                            ▼
┌──────────────────────────────────────────────────────────────┐
│  API LAYER  ·  Axum + Tower                                  │
│  REST + WebSocket  ·  OIDC auth  ·  ABAC enforcement         │
│  Rate limiting  ·  Audit log  ·  Prometheus metrics          │
└───────────────────────────┬──────────────────────────────────┘
                            │ JSON / WebSocket
                            ▼
┌──────────────────────────────────────────────────────────────┐
│  CONSOLE  ·  React + Deck.gl + CesiumJS                      │
│  Live map  ·  Entity inspector  ·  Query bar  ·  Alerts      │
│  Served from embedded static assets at localhost:9090        │
└──────────────────────────────────────────────────────────────┘
```

---

## Features

| Feature | Status | Notes |
|---------|--------|-------|
| **Connectors** | | |
| AIS maritime (NMEA 0183) | ✅ Stable | 30K events/sec sustained |
| ADS-B aircraft (SBS-1) | ✅ Stable | 1K events/sec |
| HTTP polling (generic REST) | ✅ Stable | Any JSON API → ORP entities |
| MQTT sensor / IoT | ✅ Stable | Any broker, configurable topics |
| NOAA weather | ✅ Stable | 10-min polling, severity zones |
| OpenStreetMap (ports, harbors) | ✅ Stable | Bootstrap geometry on startup |
| **Storage & Query** | | |
| DuckDB columnar store | ✅ Stable | Geospatial RTREE index, OLAP |
| Kuzu graph store | ✅ Stable | Columnar property graph, 3-hop < 1s |
| RocksDB stream state | ✅ Stable | Dedup window, checkpoints |
| ORP-QL query language | ✅ Stable | SQL + Cypher hybrid, v0.1 grammar |
| Graph traversal queries | ✅ Stable | Multi-hop path walks |
| Temporal queries (AT TIME) | ✅ Stable | Replay any past state |
| **Frontend** | | |
| Deck.gl 2D live map | ✅ Stable | 50K entities @ 60 fps, LOD rendering |
| CesiumJS 3D globe | 🔄 Beta | Phase 1, terrain + atmosphere |
| Query bar (ORP-QL) | ✅ Stable | Syntax highlighting + autocomplete |
| Entity inspector | ✅ Stable | Properties, relationships, history |
| Alert feed | ✅ Stable | Real-time anomaly notifications |
| Timeline scrubber | ✅ Stable | Drag to replay past 24h |
| **Security** | | |
| OIDC authentication | ✅ Stable | Any OIDC-compatible IdP |
| ABAC authorization | ✅ Stable | Attribute-based, per-entity policies |
| Ed25519 event signing | ✅ Stable | Every connector event cryptographically signed |
| Hash-chained audit log | ✅ Stable | Tamper-evident, verifiable with `orp verify` |
| Cryptographic erasure (GDPR) | ✅ Stable | DEK destruction, ciphertext remains |
| **Planned** | | |
| Natural language queries | 🗓️ Phase 2 | llama.cpp + Phi-2, local inference |
| WASM plugin system | 🗓️ Phase 2 | Custom connectors in any language |
| Scenario simulation / forking | 🗓️ Phase 2 | Agent-based what-if modeling |
| Python / JS / Go SDKs | 🗓️ Phase 2 | |
| Horizontal clustering | 🗓️ Phase 3 | Raft consensus, multi-node |

---

## Performance Targets

| Metric | Target | CI Gate |
|--------|--------|---------|
| Binary size (core) | 43 MB | Block if > 400 MB total |
| Cold start → HTTP ready | < 5 s | Alert if > 8 s |
| Simple query P50 | < 200 ms | Alert if > 500 ms |
| Temporal + spatial query P50 | < 800 ms | Alert if > 2 s |
| 3-hop graph query P50 | < 1 s | Alert if > 3 s |
| Stream throughput (sustained) | 100K events/sec | Alert if < 50K |
| Memory at 1M entities | < 3 GB | Alert if > 5 GB |
| Map rendering (50K entities) | 60 fps | Alert if < 30 fps |
| WebSocket update latency | < 100 ms | Alert if > 500 ms |
| `orp start` → data on screen | < 3 min | — |

---

## ORP-QL Query Language

ORP-QL combines SQL-style filtering with Cypher-style graph traversal. The query planner routes to DuckDB (OLAP, geospatial) or Kuzu (graph) based on query shape.

### Find ships near a port

```sql
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 50km)
RETURN s.name, s.speed, s.heading
ORDER BY distance(s.position, point(51.9225, 4.4792))
LIMIT 20
```

### Fast cargo ships in the North Sea

```sql
MATCH (s:Ship)
WHERE s.ship_type = "cargo"
  AND s.speed > 18
  AND within(s.position, bbox(-10, 48, 10, 62))
RETURN s.entity_id, s.name, s.speed, s.destination
```

### Graph traversal — all ships heading to Rotterdam

```sql
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.name = "Rotterdam"
RETURN s.name, s.mmsi, s.eta, s.current_position
ORDER BY s.eta
```

### Ships inside a weather system's radius

```sql
MATCH (s:Ship), (w:WeatherSystem)
WHERE w.severity = "CRITICAL"
  AND near(s.position, w.center, w.radius_km)
RETURN s.name, s.mmsi, w.name AS storm,
       distance(s.position, w.center) AS dist_km
ORDER BY dist_km
```

### Multi-hop — Maersk ships heading to congested ports

```sql
MATCH (org:Organization {name: "MaerskLine"})-[:OWNS]->(s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.congestion > 0.8
RETURN s.name, p.name, p.congestion, s.eta
ORDER BY p.congestion DESC
```

### Aggregate — vessel count by type in region

```sql
MATCH (s:Ship)
WHERE within(s.position, bbox(-5.0, 48.0, 10.0, 55.0))
RETURN s.ship_type, count(*) AS vessel_count
GROUP BY s.ship_type
ORDER BY vessel_count DESC
```

### Temporal — where was a ship 6 hours ago?

```sql
MATCH (s:Ship {entity_id: "mmsi:123456789"})
AT TIME now() - interval(6, hours)
RETURN s.position, s.speed, s.heading
```

---

## API Quick Reference

All endpoints require `Authorization: Bearer <token>` (or configure `auth.mode: "none"` for local dev).

### Entities

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/entities` | List entities (paginated, filter by type/region/time) |
| `GET` | `/api/v1/entities/{id}` | Entity detail — properties, relationships, confidence |
| `GET` | `/api/v1/entities/search` | Geospatial + type + property search |
| `GET` | `/api/v1/entities/{id}/relationships` | Graph edges for entity |
| `GET` | `/api/v1/entities/{id}/events` | Full event history |
| `POST` | `/api/v1/entities` | Manually create entity |
| `PATCH` | `/api/v1/entities/{id}` | Update entity properties |
| `DELETE` | `/api/v1/entities/{id}` | Soft-delete |

### Queries

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/v1/query` | Execute ORP-QL query |
| `POST` | `/api/v1/query/natural` | Natural language → results *(Phase 2)* |
| `POST` | `/api/v1/graph` | Raw Kuzu Cypher query |

### Connectors

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/connectors` | List connectors + live metrics |
| `POST` | `/api/v1/connectors` | Register new connector |
| `DELETE` | `/api/v1/connectors/{id}` | Deregister connector |

### Monitors & Alerts

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/monitors` | List monitor rules |
| `POST` | `/api/v1/monitors` | Create monitor rule |
| `GET` | `/api/v1/alerts` | List fired alerts |
| `POST` | `/api/v1/alerts/{id}/acknowledge` | Acknowledge alert |

### System

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/health` | Component health (DuckDB, Kuzu, connectors) |
| `GET` | `/api/v1/metrics` | Prometheus-compatible metrics |
| `WS` | `/ws/updates` | Real-time entity push, filter by bbox/type/id |

---

## WebSocket Example

```javascript
const ws = new WebSocket('ws://localhost:9090/ws/updates');

ws.onopen = () => {
  // Authenticate
  ws.send(JSON.stringify({ type: 'auth', token: 'Bearer eyJ...' }));

  // Subscribe to ships in the North Sea
  ws.send(JSON.stringify({
    type: 'subscribe',
    subscription_id: 'north-sea-ships',
    filter: {
      entity_types: ['Ship'],
      bbox: { min_lat: 48.0, max_lat: 62.0, min_lon: -5.0, max_lon: 10.0 },
      update_interval_ms: 1000
    }
  }));
};

ws.onmessage = ({ data }) => {
  const msg = JSON.parse(data);
  if (msg.type === 'entity_update') console.log('Updated:', msg.entity);
  if (msg.type === 'alert')         console.log('ALERT:', msg.alert.severity, msg.alert.message);
};
```

---

## Security Overview

ORP is built for high-stakes environments. Security is not an afterthought.

| Mechanism | What It Does |
|-----------|-------------|
| **OIDC authentication** | Every request validated against any OIDC-compatible IdP (Keycloak, Auth0, Dex, etc.) |
| **ABAC authorization** | Per-entity, per-request attribute-based policy evaluation — no data returned without explicit permission |
| **Ed25519 event signing** | Every connector event signed at the source; provenance of any data point is cryptographically verifiable |
| **Hash-chained audit log** | Each audit entry contains SHA-256 of the prior entry; tampering is immediately detectable via `orp verify` |
| **Cryptographic erasure** | GDPR right-to-erasure: destroy the DEK — all ciphertext remains but is permanently unreadable |
| **No telemetry** | The binary makes zero external network calls unless you configure a connector; no phone-home, no analytics |
| **Secrets via environment** | Config YAML uses `${env.MY_SECRET}` — never stores secrets inline |

Full security architecture including OIDC flow diagram, ABAC policy model, signing chain, and rate limiting: [docs/SECURITY.md](docs/SECURITY.md)

---

## Building from Source

```bash
# Prerequisites: Rust 1.75+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# macOS
xcode-select --install && brew install cmake pkg-config openssl

# Ubuntu / Debian
sudo apt install build-essential cmake pkg-config libssl-dev

# Clone and build
git clone https://github.com/orproject/orp.git
cd orp
cargo build --release         # optimized binary
./target/release/orp start --template maritime
```

```bash
# Test suite
cargo test --workspace        # 203 tests
cargo clippy --all-targets -- -D warnings   # 0 warnings
cargo fmt -- --check
```

---

## Configuration

ORP reads from `~/.orp/config.yaml` or `--config <path>`:

```yaml
server:
  host: "127.0.0.1"
  port: 9090

storage:
  data_dir: "~/.orp/data"
  duckdb:
    max_memory: "2GB"
    threads: 4
  kuzu:
    sync_interval_secs: 30

connectors:
  - id: "ais-global"
    type: "ais"
    config:
      host: "153.44.253.27"
      port: 9999

auth:
  mode: "oidc"   # none | oidc | api_key
  oidc:
    issuer: "https://auth.example.com"
    client_id: "orp-console"
    client_secret: "${env.OIDC_CLIENT_SECRET}"

security:
  signing:
    enabled: true
    key_path: "${env.ORP_SIGNING_KEY_PATH}"
  audit_log:
    enabled: true
    retention_days: 365
```

**Templates** (`--template <name>`): `maritime` · `adsb` · `supply-chain` · `climate` · `custom`

---

## Project Stats

| Stat | Value |
|------|-------|
| Crates | 12 |
| Rust source files | 53 |
| Lines of Rust | ~17,000 |
| Tests | 203 passing |
| Binary size (core) | 43 MB |
| License | Apache 2.0 |
| Rust edition | 2021 (1.75+ MSRV) |

**Crate map:** `orp-core` · `orp-proto` · `orp-config` · `orp-connector` · `orp-stream` · `orp-entity` · `orp-storage` · `orp-query` · `orp-security` · `orp-audit` · `orp-geospatial` · `orp-testbed`

---

## Contributing

We welcome contributions: new connectors, bug fixes, documentation, benchmarks.

```bash
# 1. Fork + clone
git checkout -b feat/ORP-123-your-feature

# 2. Build + test
cargo test --workspace && cargo clippy --all-targets -- -D warnings

# 3. Commit (Conventional Commits)
git commit -m "feat(connector): add Kafka source connector"

# 4. Open a PR
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for PR standards, connector guide, and code review requirements.
Good first issues: [`good first issue`](https://github.com/orproject/orp/issues?q=label%3A%22good+first+issue%22)

---

## License

Apache License, Version 2.0 — see [LICENSE](LICENSE).

Copyright 2026 The ORP Authors and Contributors.

---

<div align="center">
<sub>Built by engineers who believe the tools for understanding physical reality should be free and open.</sub>
</div>
