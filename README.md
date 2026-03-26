# ORP — Open Reality Protocol

<div align="center">

**The single-binary Palantir alternative. Real-time data fusion. Runs on your laptop.**

[![Build Status](https://img.shields.io/github/actions/workflow/status/orproject/orp/ci.yml?branch=main&style=flat-square&logo=github)](https://github.com/orproject/orp/actions)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue?style=flat-square)](LICENSE)
[![Rust Version](https://img.shields.io/badge/rust-1.75%2B-orange?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Binary Size](https://img.shields.io/badge/binary-%3C350MB-green?style=flat-square)](https://github.com/orproject/orp/releases)
[![Discord](https://img.shields.io/discord/placeholder?style=flat-square&logo=discord)](https://discord.gg/orp)

[**Quick Start**](#quick-start) · [**Documentation**](https://docs.orp.dev) · [**Architecture**](ARCHITECTURE.md) · [**Contributing**](CONTRIBUTING.md)

</div>

---

## What Is ORP?

ORP connects to real-time data sources, fuses them into a live knowledge graph, and lets humans and AI query, reason about, and simulate physical reality. Everything runs locally. No cloud. No external dependencies. No setup.

**Install → Run → Live data on screen in under 5 minutes.**

### The Pattern

| Precedent | What It Replaced | Binary Size | Impact |
|-----------|-----------------|-------------|--------|
| SQLite | Oracle / MySQL servers | < 1 MB | In every device on earth |
| DuckDB | Apache Spark clusters ($100K+/yr) | ~50 MB | 100× faster, 50% YoY growth |
| llama.cpp | OpenAI API (cloud, $$$) | ~100 MB | LLMs on laptops |
| **ORP** | **Palantir ($50–500M/deployment)** | **~300 MB** | **Data fusion on laptops** |

Palantir earns $2.8B/year. Anduril Lattice: $20B Army contract. Both proprietary. Both locked behind defense contracts. ORP open-sources this capability — so a disaster response agency, city planner, or climate researcher can download one binary and get Palantir-grade data fusion for free.

---

## Quick Start

```bash
# Install ORP (Linux + macOS)
curl -fsSL https://orp.dev/install | sh

# Start with the maritime template (ships + weather + ports)
orp start --template maritime

# Browser opens automatically at http://localhost:9090
```

> **Screenshot:** _2,000+ ships updating live on a Deck.gl map. Click any ship for full details. Query bar ready._

![ORP Console — Maritime Template](docs/assets/screenshot-placeholder.png)

### What You'll See

- **Live map** — ships, aircraft, weather systems updating in real-time
- **Entity inspector** — click any entity to see its properties, relationships, and history
- **Query bar** — type natural language or ORP-QL queries; results in < 200 ms
- **Alert feed** — rule-based anomaly detection fires when ships deviate, speeds spike, or weather threatens ports
- **Timeline scrubber** — drag backward to replay any past state

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          DATA SOURCES                                │
│   AIS Feed   ADS-B Feed   Weather API   MQTT Sensor   CSV File      │
└──────┬──────────┬──────────────┬────────────┬─────────────┬─────────┘
       │          │              │            │             │
       ▼          ▼              ▼            ▼             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  CONNECTORS  (orp-connector)                                         │
│  Each connector: parse protocol → emit OrpEvent structs              │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ Vec<OrpEvent> via async channel
                              ▼
┌──────────────────────────────────────────────────────────────────────┐
│  STREAM PROCESSOR  (orp-stream)                                      │
│  Dedup (RocksDB) → Window → Batch (1K events) → Entity Resolution   │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ Batch insert
                              ▼
┌──────────────────────────────────────────────────────────────────────┐
│  STORAGE LAYER  (orp-storage)                                        │
│  ┌─────────────┐   Sync (30s)   ┌─────────────┐                     │
│  │   DuckDB    │ ─────────────► │    Kuzu     │                     │
│  │   (OLAP)    │                │   (Graph)   │                     │
│  │  entities   │                │  Ship nodes │                     │
│  │  events     │                │  Port nodes │                     │
│  │  properties │                │  Edges      │                     │
│  └──────┬──────┘                └──────┬──────┘                     │
│         └──────────────┬───────────────┘                             │
│                        │ Unified Storage trait                        │
└────────────────────────┼─────────────────────────────────────────────┘
                         │
                         ▼
┌──────────────────────────────────────────────────────────────────────┐
│  QUERY ENGINE  (orp-query)                                           │
│  ORP-QL parser → Query planner → Route to DuckDB or Kuzu → Merge    │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ JSON results
                              ▼
┌──────────────────────────────────────────────────────────────────────┐
│  API LAYER  (orp-core / Axum)                                        │
│  REST + WebSocket · GET /api/v1/entities · POST /api/v1/query       │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ JSON / WebSocket
                              ▼
┌──────────────────────────────────────────────────────────────────────┐
│  FRONTEND  (React + Deck.gl + CesiumJS)                              │
│  Map · Entity Inspector · Query Bar · Timeline · Alert Feed          │
│  Served from embedded static assets at localhost:9090                │
└──────────────────────────────────────────────────────────────────────┘
```

### Binary Composition (~300 MB)

| Component | Size | Role |
|-----------|------|------|
| Rust Core | ~40 MB | Connectors, stream processor, HTTP/WS server |
| DuckDB | ~50 MB | OLAP queries, geospatial, columnar storage |
| Kuzu | ~40 MB | Graph queries, path traversal, relationship walks |
| RocksDB | ~20 MB | Stream state, deduplication, windowing |
| Built-in Connectors | ~30 MB | AIS, ADS-B, HTTP, MQTT, CSV, WebSocket |
| React Frontend | ~20 MB | Map (Deck.gl), entity inspector, query bar |
| Runtime Libs | ~60 MB | Tokio, Proj, GEOS |

---

## Features

| Feature | Status | Notes |
|---------|--------|-------|
| AIS maritime connector | ✅ Stable | 30K events/sec sustained |
| ADS-B aircraft connector | ✅ Stable | 1K events/sec |
| HTTP polling connector | ✅ Stable | Generic REST → ORP entities |
| MQTT sensor connector | ✅ Stable | IoT/sensor data |
| NOAA weather connector | ✅ Stable | 10-min polling |
| OpenStreetMap connector | ✅ Stable | Ports, harbors, geometry |
| Deck.gl 2D map | ✅ Stable | 50K entities @ 60 fps |
| CesiumJS 3D globe | 🔄 Beta | Phase 1 |
| ORP-QL query language | ✅ Stable | v0.1 grammar |
| Graph queries (Kuzu) | ✅ Stable | 3-hop traversal < 1s |
| Rule-based alerting | ✅ Stable | Speed, deviation, zone rules |
| Timeline scrubber | ✅ Stable | Replay past states |
| OIDC authentication | ✅ Stable | Any compatible IdP |
| ABAC authorization | ✅ Stable | Attribute-based policies |
| Ed25519 event signing | ✅ Stable | All connector data signed |
| Immutable audit log | ✅ Stable | Hash-chained, tamper-evident |
| Natural language queries | 🗓️ Phase 2 | llama.cpp + Phi-2 (1.6 GB model) |
| WASM plugin system | 🗓️ Phase 2 | Custom connectors in any language |
| Simulation / scenario forking | 🗓️ Phase 2 | Agent-based models |
| Python / JS / Go SDKs | 🗓️ Phase 2 | |
| Horizontal clustering | 🗓️ Phase 3 | Raft consensus |

---

## Performance Targets

| Metric | Target | Gate (CI blocks if exceeded) |
|--------|--------|------------------------------|
| Binary size | < 350 MB | Block if > 400 MB |
| Cold start → HTTP ready | < 5 s | Alert if > 8 s |
| Simple query P50 (ships near location) | < 200 ms | Alert if > 500 ms |
| Temporal + spatial query P50 | < 800 ms | Alert if > 2 s |
| 3-hop graph query P50 | < 1 s | Alert if > 3 s |
| Stream throughput (sustained) | 100 K events/sec | Alert if < 50 K |
| Memory (1 M entities under load) | < 3 GB | Alert if > 5 GB |
| Map rendering (50 K entities) | 60 fps | Alert if < 30 fps |
| WebSocket update latency | < 100 ms | Alert if > 500 ms |
| `orp start` → data on screen | < 3 min | Alert if > 5 min |

---

## Building from Source

### Prerequisites

```bash
# Rust (1.75+)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable

# System dependencies (Ubuntu/Debian)
sudo apt install build-essential cmake pkg-config libssl-dev

# System dependencies (macOS)
xcode-select --install
brew install cmake pkg-config openssl
```

### Build

```bash
git clone https://github.com/orproject/orp.git
cd orp

# Development build
cargo build

# Optimized release build (~300 MB binary)
cargo build --release

# Run
./target/release/orp start --template maritime
```

### Cross-Compilation

```bash
# Add targets
rustup target add x86_64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-gnu
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin

# Build for Linux (from macOS, requires cross)
cargo install cross
cross build --release --target x86_64-unknown-linux-gnu
```

### Running Tests

```bash
# Unit tests
cargo test

# All tests including integration
cargo test --workspace

# Benchmarks
cargo bench --package orp-testbed

# Clippy (required to pass before PRs)
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt -- --check
```

---

## Configuration

ORP is configured via a single YAML file. By default, it reads from `~/.orp/config.yaml` or a path specified with `--config`.

```yaml
# ~/.orp/config.yaml

server:
  host: "127.0.0.1"
  port: 9090
  cors_origins:
    - "http://localhost:3000"

storage:
  data_dir: "~/.orp/data"
  duckdb:
    max_memory: "2GB"
    threads: 4
  kuzu:
    buffer_pool_size: "512MB"
    sync_interval_secs: 30
  rocksdb:
    cache_size: "256MB"

connectors:
  - id: "ais-global"
    type: "ais"
    enabled: true
    config:
      host: "153.44.253.27"      # AISHub or your feed
      port: 9999
      filter_mmsi_ranges: []      # empty = all ships
      max_events_per_sec: 30000

  - id: "weather-noaa"
    type: "http_poll"
    enabled: true
    config:
      url: "https://api.weather.gov/alerts/active"
      poll_interval_secs: 600
      entity_type: "WeatherAlert"
      api_key: "${env.NOAA_API_KEY}"   # never hardcode secrets

  - id: "adsb-local"
    type: "adsb"
    enabled: false
    config:
      host: "localhost"
      port: 30003

auth:
  mode: "oidc"                   # options: none, oidc, api_key
  oidc:
    issuer: "https://auth.yourcompany.com"
    client_id: "orp-console"
    client_secret: "${env.OIDC_CLIENT_SECRET}"
    redirect_uri: "http://localhost:9090/auth/callback"

security:
  signing:
    enabled: true
    key_path: "${env.ORP_SIGNING_KEY_PATH}"
  audit_log:
    enabled: true
    retention_days: 365
  tls:
    enabled: false
    cert_path: ""
    key_path: ""

stream:
  dedup_window_hours: 24
  batch_size: 1000
  batch_flush_ms: 1000
  backpressure_buffer: 100000

logging:
  level: "info"                  # trace, debug, info, warn, error
  format: "json"                 # json, pretty
  file: "~/.orp/logs/orp.log"
```

### Templates

Templates are pre-built configurations for common domains:

```bash
orp start --template maritime     # Ships, ports, weather
orp start --template adsb         # Aircraft tracking only
orp start --template supply-chain # Cargo, routes, warehouses
orp start --template climate      # Weather + shipping correlations
orp start --template custom       # Blank slate
```

---

## ORP-QL Query Examples

ORP-QL is a purpose-built query language for reasoning about real-world entities. It combines SQL-style filtering with Cypher-style graph traversal.

### Example 1: Find ships near a port

```sql
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 50km)
RETURN s.name, s.speed, s.heading
ORDER BY distance(s.position, point(51.9225, 4.4792))
LIMIT 20
```

### Example 2: Fast cargo ships in a region

```sql
MATCH (s:Ship)
WHERE s.ship_type = "cargo"
  AND s.speed > 18
  AND within(s.position, bbox(-10, 35, 30, 60))
RETURN s.entity_id, s.name, s.speed, s.destination
```

### Example 3: Graph traversal — ships heading to Rotterdam

```sql
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.name = "Rotterdam"
RETURN s.name, s.mmsi, s.eta, s.current_position
ORDER BY s.eta
```

### Example 4: Ships potentially affected by a weather system

```sql
MATCH (s:Ship), (w:WeatherSystem)
WHERE w.severity = "CRITICAL"
  AND near(s.position, w.center, w.radius_km)
RETURN s.name, s.mmsi, w.name AS storm, distance(s.position, w.center) AS dist_km
ORDER BY dist_km
```

### Example 5: Multi-hop — find ships owned by an organization heading to congested ports

```sql
MATCH (org:Organization {name: "MaerskLine"})-[:OWNS]->(s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.congestion > 0.8
RETURN s.name, p.name, p.congestion, s.eta
ORDER BY p.congestion DESC
```

### Example 6: Aggregate — vessel count by type in region

```sql
MATCH (s:Ship)
WHERE within(s.position, bbox(-5.0, 48.0, 10.0, 55.0))
RETURN s.ship_type, count(*) AS vessel_count
GROUP BY s.ship_type
ORDER BY vessel_count DESC
```

### Example 7: Temporal — where was a ship 6 hours ago?

```sql
MATCH (s:Ship {entity_id: "mmsi:123456789"})
AT TIME now() - interval(6, hours)
RETURN s.position, s.speed, s.heading
```

---

## API Quick Reference

All endpoints require `Authorization: Bearer <token>` unless `auth.mode = "none"`.

### Entities

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/entities` | List entities (paginated, filterable by type/tag/time) |
| `GET` | `/api/v1/entities/{id}` | Get entity details, properties, relationships |
| `GET` | `/api/v1/entities/search` | Geospatial + type + property search |
| `GET` | `/api/v1/entities/{id}/relationships` | Get graph relationships |
| `GET` | `/api/v1/entities/{id}/events` | Get event history |
| `POST` | `/api/v1/entities` | Create entity manually |
| `PATCH` | `/api/v1/entities/{id}` | Update entity properties |
| `DELETE` | `/api/v1/entities/{id}` | Soft-delete entity |

### Queries

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/api/v1/query` | Execute ORP-QL query |
| `POST` | `/api/v1/query/natural` | Natural language → ORP-QL → results _(Phase 2)_ |
| `POST` | `/api/v1/graph` | Execute raw Kuzu Cypher query |

### Connectors

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/connectors` | List active connectors + metrics |
| `POST` | `/api/v1/connectors` | Register new connector |
| `GET` | `/api/v1/connectors/{id}` | Connector status and config |
| `DELETE` | `/api/v1/connectors/{id}` | Deregister connector |

### Monitors & Alerts

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/monitors` | List monitor rules |
| `POST` | `/api/v1/monitors` | Create monitor rule |
| `GET` | `/api/v1/monitors/{id}` | Get monitor detail |
| `DELETE` | `/api/v1/monitors/{id}` | Delete monitor |
| `GET` | `/api/v1/alerts` | List fired alerts |
| `POST` | `/api/v1/alerts/{id}/acknowledge` | Acknowledge an alert |

### System

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/v1/health` | System health + component status |
| `GET` | `/api/v1/metrics` | Prometheus-compatible metrics |
| `GET` | `/api/v1/version` | Binary version, build info |

### WebSocket

| Endpoint | Description |
|----------|-------------|
| `WS /ws/updates` | Real-time entity updates (subscribe by type/region/id) |

---

## WebSocket Usage

Connect to `/ws/updates` for real-time entity push updates:

```javascript
const ws = new WebSocket('ws://localhost:9090/ws/updates');

// Authenticate
ws.onopen = () => {
  ws.send(JSON.stringify({
    type: 'auth',
    token: 'Bearer eyJ...'
  }));
};

// Subscribe to all ship updates within a bounding box
ws.send(JSON.stringify({
  type: 'subscribe',
  subscription_id: 'my-ships',
  filter: {
    entity_types: ['Ship'],
    bbox: {
      min_lat: 48.0,
      max_lat: 55.0,
      min_lon: -5.0,
      max_lon: 10.0
    },
    update_interval_ms: 1000   // batch updates to client
  }
}));

// Receive updates
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  if (msg.type === 'entity_update') {
    console.log('Updated:', msg.entity.entity_id, msg.entity.properties);
  }

  if (msg.type === 'alert') {
    console.log('ALERT:', msg.alert.severity, msg.alert.message);
  }
};
```

### WebSocket Message Types

| Type | Direction | Description |
|------|-----------|-------------|
| `auth` | Client → Server | Authenticate the connection |
| `subscribe` | Client → Server | Subscribe to entity updates |
| `unsubscribe` | Client → Server | Cancel a subscription |
| `entity_update` | Server → Client | Entity position/property changed |
| `entity_created` | Server → Client | New entity discovered |
| `entity_deleted` | Server → Client | Entity removed |
| `alert` | Server → Client | Monitor rule triggered |
| `ping` / `pong` | Both | Keep-alive |

---

## Security Overview

ORP is built for high-stakes environments. Security is not an add-on.

- **Ed25519 event signing** — every event signed at the connector level; audit log verifies provenance of any data point
- **ABAC authorization** — every API call checks the caller's attributes against the entity's access policy; no data returned without explicit permission
- **Immutable audit log** — hash-chained (each entry includes SHA-256 of the previous entry); tampering is detectable
- **No telemetry** — the binary does not contact any external server unless explicitly configured; no phone-home, no analytics
- **Cryptographic erasure** — GDPR right-to-erasure: destroy the encryption key; ciphertext remains but is unrecoverable
- **Secrets in environment** — config files use `${env.MY_SECRET}` syntax; secrets are never stored in YAML
- **Connector sandboxing** — each connector runs in its own async task with resource limits; Phase 2 adds WASM sandboxes

See [docs/SECURITY.md](docs/SECURITY.md) for the full security architecture.

---

## Contributing

We welcome contributions of all kinds: bug fixes, new connectors, documentation improvements, performance optimizations.

**Before contributing:**

1. Read [CONTRIBUTING.md](CONTRIBUTING.md) — PR standards, commit format, code review
2. Read [ARCHITECTURE.md](ARCHITECTURE.md) — understand the codebase
3. Read [GOVERNANCE.md](GOVERNANCE.md) — how decisions are made
4. Find or create an issue first — discuss before building large features

**Quick contribution path:**

```bash
# Fork, clone, and create a branch
git checkout -b feature/ORP-123-your-feature

# Make changes, ensure CI passes
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt

# Commit using Conventional Commits format
git commit -m "feat(connector): add MQTT broker support"

# Push and open a PR
```

**Good first issues** are tagged [`good first issue`](https://github.com/orproject/orp/issues?q=label%3A%22good+first+issue%22) on GitHub.

---

## License

ORP is released under the **Apache License, Version 2.0**.

See [LICENSE](LICENSE) for the full text.

Copyright 2026 The ORP Authors and Contributors.

---

<div align="center">
<sub>Built by a community of engineers who believe the tools for understanding physical reality should be free and open.</sub>
</div>
