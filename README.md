<div align="center">

# ORP — Open Reality Protocol

### A single binary that does what Palantir charges $50M for.

[![Tests](https://img.shields.io/badge/tests-538%20passing-brightgreen?style=flat-square)](https://github.com/shieldofsteel/orp/actions)
[![Binary Size](https://img.shields.io/badge/binary-43MB-blue?style=flat-square)](https://github.com/shieldofsteel/orp/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-orange?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?style=flat-square)](https://www.rust-lang.org)
[![Crates](https://img.shields.io/badge/crates-69%20Rust%20files-red?style=flat-square)](crates/)
[![Lines](https://img.shields.io/badge/lines-38K%2B-lightgrey?style=flat-square)](crates/)

</div>

---

## The $50M Question

The tools that fuse live sensor data, render a real-time operational picture, and alert you when something is wrong cost a fortune. Palantir AIP: **$50–500M**. Esri GIS: **$500K/year**. Custom C2 systems: **$10M+**.

What if the same thing ran on a laptop? Or a Raspberry Pi? For free?

| What | Replaces | Size | Cost |
|------|----------|------|------|
| SQLite | Oracle | `<1MB` | Free |
| DuckDB | Apache Spark | `~50MB` | Free |
| llama.cpp | OpenAI API | `~100MB` | Free |
| **ORP** | **Palantir ($50–500M)** | **43MB** | **Free** |

**ORP** is an open-source data fusion engine and Common Operating Picture (COP) platform. It ingests live data from any sensor, protocol, or API — fuses it into a unified knowledge graph — and renders it on a military-grade map. One binary. Zero dependencies. Apache 2.0.

---

## 30 Seconds to Live Data

```bash
# Install
curl -fsSL https://orp.dev/install | sh

# Launch with the maritime template
orp start --template maritime

# Ships appear on your screen in 30 seconds
# Open http://localhost:9090
```

That's it. No YAML sprawl. No microservices. No Kubernetes. One process, one port, everything included.

---

## What You'll See

```
╔══════════════════════════════════════════════════════════════════════╗
║  ORP — MARITIME COP                               [LIVE] 14:32:07   ║
╠══════════════╦═══════════════════════════════════════════════════════╣
║ ENTITIES     ║                                                       ║
║              ║        🚢 EVER GIVEN           ►──────────           ║
║ Ships   247  ║                                                       ║
║ Aircraft  12 ║    🚢 MSC OSCAR ►────                                 ║
║ Vehicles   3 ║                        ⚠ ANOMALY                     ║
║ Threats    1 ║                      🚢 UNKNOWN        ►             ║
║              ║                                                       ║
╠══════════════╣           🏭 PORT OF ROTTERDAM                       ║
║ ALERTS       ║                                                       ║
║              ║                                                       ║
║ ⚠ Speed      ║                                                       ║
║   anomaly    ╠═══════════════════════════════════════════════════════╣
║   mmsi:      ║ > MATCH (s:Ship) WHERE s.speed > 20 RETURN s.name    ║
║   244820000  ║   s.speed, s.course LIMIT 10                         ║
║              ║                                                       ║
║ ℹ 3 ships    ║   → EVER GIVEN    22.4 kn  082°                      ║
║   in zone    ║   → MSC OSCAR     21.8 kn  264°                      ║
╚══════════════╩═══════════════════════════════════════════════════════╝
```

---

## What ORP Does

**Fuses data from any source** — 17 protocol adapters, a universal JSON ingest endpoint, and a connector SDK. If it outputs data, ORP can consume it.

**Builds a live knowledge graph** — every entity (ship, aircraft, vehicle, sensor, threat) becomes a node. Relationships auto-form. The graph updates in real time.

**Renders a military-grade COP** — a full-featured map with 4 tile layers, directional arrows, course vectors, lasso select, and a timeline scrubber. Not a dashboard — an operational picture.

**Lets you query anything** — ORP-QL is a purpose-built query language combining SQL analytics with Cypher-style graph traversal. Query across sensors, entities, and time.

**Alerts you before it matters** — anomaly detection and threat scoring run continuously. When a ship deviates from its pattern-of-life, you know.

**Runs anywhere** — a laptop, a Raspberry Pi, a warship, a data center. `--headless` for embedded deployments. Docker for cloud. ARM binaries for edge.

---

## Protocol Support

ORP speaks the languages your sensors already use.

### 🚢 Maritime
| Protocol | Description | Status |
|----------|-------------|--------|
| **NMEA 0183** | GPS, depth, wind, heading, all sentence types | ✅ Implemented |
| **AIS** | Vessel tracking, 27 message types, Class A/B | ✅ Implemented |
| **NMEA 2000 / N2K** | Modern vessel CAN bus (via gateway) | 🔜 Planned |

### ✈️ Aviation
| Protocol | Description | Status |
|----------|-------------|--------|
| **ADS-B / Mode S** | Aircraft position, velocity, identity at 1090 MHz | ✅ Implemented |
| **ASTERIX** | Eurocontrol ATC radar data exchange | ✅ Implemented |

### 🪖 Military / Tactical
| Protocol | Description | Status |
|----------|-------------|--------|
| **CoT (Cursor on Target)** | TAK/ATAK compatible track sharing | ✅ Implemented |
| **STIX/TAXII** | Threat intelligence exchange | ✅ Implemented |

### 🏭 Industrial / IoT
| Protocol | Description | Status |
|----------|-------------|--------|
| **OPC-UA** | Industrial automation, SCADA | ✅ Implemented |
| **Modbus TCP/RTU** | PLCs, sensors, energy meters | ✅ Implemented |
| **MQTT** | IoT sensor telemetry | ✅ Implemented |
| **SparkplugB** | Industrial MQTT structured payload | 🔜 Planned |
| **DNP3** | Utility SCADA, substations | 🔜 Planned |
| **CAN / J1939** | Vehicles, trucks, heavy equipment | 🔜 Planned |

### 🔐 Cyber / Network
| Protocol | Description | Status |
|----------|-------------|--------|
| **Syslog** | System and network device logs | ✅ Implemented |
| **PCAP** | Packet capture analysis | 🔜 Planned |
| **Zeek** | Network security monitoring logs | 🔜 Planned |
| **NetFlow / IPFIX** | Network flow telemetry | 🔜 Planned |

### 🌦 Weather / Environment
| Protocol | Description | Status |
|----------|-------------|--------|
| **METAR** | Aviation weather reports | 🔜 Planned |
| **CAP** | Common Alerting Protocol (emergency) | ✅ Implemented |

### 🚌 Transport
| Protocol | Description | Status |
|----------|-------------|--------|
| **GTFS-RT** | Real-time transit feeds | 🔜 Planned |

### 🌐 Universal Ingest
| Source | Description | Status |
|--------|-------------|--------|
| **HTTP Poller** | Pull any REST API on a schedule | ✅ Implemented |
| **WebSocket Client** | Subscribe to any WebSocket stream | ✅ Implemented |
| **CSV Watcher** | Watch a file/directory for CSV changes | ✅ Implemented |
| **Database** | Query any SQL database as a source | ✅ Implemented |
| **Generic API** | POST any JSON → instant entities | ✅ Implemented |
| **GeoJSON** | Static and streaming geospatial data | ✅ Implemented |

> **Don't see your protocol?** The connector SDK is 50 lines of Rust. [Build one →](docs/CONNECTOR_GUIDE.md)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        ORP SINGLE BINARY                            │
│                                                                     │
│  ┌──────────────┐   ┌──────────────┐   ┌─────────────────────────┐ │
│  │  CONNECTORS  │   │  FUSION      │   │  QUERY ENGINE           │ │
│  │              │   │  ENGINE      │   │                         │ │
│  │  NMEA/AIS    │──▶│              │──▶│  ORP-QL                 │ │
│  │  ADS-B       │   │  Entity      │   │  (SQL + Graph hybrid)   │ │
│  │  CoT / TAK   │   │  Resolution  │   │                         │ │
│  │  OPC-UA      │   │              │   │  DuckDB (analytics)     │ │
│  │  MQTT        │   │  Knowledge   │   │  Kuzu (graph traversal) │ │
│  │  Modbus      │   │  Graph       │   │                         │ │
│  │  Syslog      │   │              │   └─────────────────────────┘ │
│  │  HTTP/WS     │   │  Anomaly     │                               │
│  │  CSV / DB    │   │  Detection   │   ┌─────────────────────────┐ │
│  │  + 9 more    │   │              │   │  API & REALTIME         │ │
│  └──────────────┘   │  Threat      │──▶│                         │ │
│                     │  Scoring     │   │  REST API (v1)          │ │
│  ┌──────────────┐   │              │   │  WebSocket (live push)  │ │
│  │  FEDERATION  │   │  ABAC +      │   │  ORP-to-ORP mesh sync   │ │
│  │              │──▶│  Ed25519     │   └─────────────────────────┘ │
│  │  Peer ORPs   │   │  Signing     │                               │
│  │  (mesh sync) │   └──────────────┘   ┌─────────────────────────┐ │
│  └──────────────┘                      │  WEB UI                 │ │
│                                        │  Map (4 tile layers)    │ │
│  ┌──────────────┐                      │  Dashboard              │ │
│  │  STORAGE     │                      │  Entity Inspector       │ │
│  │              │                      │  Query Console          │ │
│  │  DuckDB      │                      │  Search Panel           │ │
│  │  (entities,  │                      │  Alert Feed             │ │
│  │   history)   │                      │  Timeline Scrubber      │ │
│  │  Kuzu        │                      └─────────────────────────┘ │
│  │  (graph)     │                                                   │
│  └──────────────┘                                                   │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Feature Checklist

**Data Ingestion**
- ✅ 17 protocol adapters (NMEA, AIS, ADS-B, CoT, ASTERIX, STIX, OPC-UA, CAP, Modbus, MQTT, Syslog, HTTP, WebSocket, CSV, Database, GeoJSON, Generic API)
- ✅ Universal JSON ingest — `POST /api/v1/ingest` accepts anything
- ✅ Serial port NMEA direct read (`serial:///dev/ttyUSB0`)
- ✅ Connector hot-reload — add/remove sources without restart

**Fusion & Intelligence**
- ✅ Live knowledge graph — entities, relationships, history
- ✅ Entity resolution — same ship from 3 sources = 1 entity
- ✅ Anomaly detection — speed, course, zone, pattern-of-life
- ✅ Threat scoring — configurable scoring rules per entity type
- ✅ Confidence scoring — multi-source data gets weighted

**Query**
- ✅ ORP-QL — SQL meets Cypher, built for real-world entities
- ✅ Graph traversal — `MATCH (s:Ship)-[:NEAR]->(p:Port)`
- ✅ Geospatial queries — `WHERE geo_within(s.position, zone)`
- ✅ Temporal queries — `WHERE s.updated_at > NOW() - 5m`
- ✅ REST API query endpoint — run ORP-QL over HTTP

**Visualization**
- ✅ Real-time map — OpenStreetMap, satellite, nautical chart, dark tile layers
- ✅ Directional arrows and course vectors on all moving entities
- ✅ Lasso select — drag to select a region and inspect all entities
- ✅ Entity Inspector — click anything for full data, history, graph
- ✅ Dashboard — live counts, alert feed, top entities
- ✅ Query Console — run ORP-QL in the browser, see results instantly
- ✅ Search Panel — full-text search across all entities
- ✅ Timeline Scrubber — replay history at any speed
- ✅ Dynamic UI — adapts to any entity type automatically

**Security**
- ✅ ABAC (Attribute-Based Access Control) — fine-grained permissions
- ✅ Ed25519 message signing — tamper-evident entity provenance
- ✅ JWT / OIDC authentication
- ✅ API key support for programmatic access
- ✅ Multi-tenant isolation

**Operations**
- ✅ Federation — ORP-to-ORP peer mesh sync
- ✅ Headless mode — runs without UI for edge/embedded deployments
- ✅ Docker support — single container, compose-ready
- ✅ ARM cross-compilation — Raspberry Pi, Apple Silicon, AWS Graviton
- ✅ 538+ tests, zero clippy warnings

---

## ORP-QL

ORP-QL is SQL and Cypher combined into a single language for real-world entities. The query planner compiles it to DuckDB (for analytics) or Kuzu (for graph traversal) automatically.

### Find vessels moving too fast
```sql
MATCH (s:Ship)
WHERE s.speed > 25
RETURN s.name, s.mmsi, s.speed, s.course, s.position
ORDER BY s.speed DESC
LIMIT 20
```

### Find ships near a port that haven't identified themselves
```sql
MATCH (s:Ship)-[:NEAR]->(p:Port {name: "Rotterdam"})
WHERE s.name IS NULL OR s.mmsi IS NULL
RETURN s.entity_id, s.position, s.speed, p.name
```

### Geospatial — all entities in a bounding box
```sql
MATCH (e:Entity)
WHERE geo_within(e.position, bbox(51.9, 4.0, 52.1, 4.2))
RETURN e.entity_type, e.name, e.position
ORDER BY e.updated_at DESC
```

### Temporal — what happened in the last 10 minutes?
```sql
MATCH (e:Entity)
WHERE e.updated_at > NOW() - INTERVAL '10 minutes'
  AND e.entity_type IN ('Ship', 'Aircraft')
RETURN e.name, e.entity_type, e.speed, e.updated_at
ORDER BY e.updated_at DESC
```

### Graph — find all aircraft within relay distance of a ground station
```sql
MATCH (a:Aircraft)-[:WITHIN_RANGE]->(g:GroundStation)
WHERE a.altitude < 10000
RETURN a.callsign, a.altitude, a.speed, g.name
```

### Aggregation — track volume by source in the last hour
```sql
MATCH (e:Entity)
WHERE e.updated_at > NOW() - INTERVAL '1 hour'
RETURN e.source_connector, COUNT(*) as updates, AVG(e.confidence) as avg_conf
ORDER BY updates DESC
```

Full language reference → [docs/ORP_QL_GUIDE.md](docs/ORP_QL_GUIDE.md)

---

## Edge Deployment

ORP was designed from day one to run on constrained hardware. A 43MB binary with no runtime dependencies means it runs anywhere Linux runs.

### Raspberry Pi as a Vessel Intelligence Node

```
Ship's NMEA Bus (serial RS-422)
         │
         ▼ /dev/ttyUSB0
   ┌─────────────┐
   │ Raspberry   │  ← $50 hardware
   │ Pi 4 / CM4  │
   │             │
   │  ORP        │  ← 43MB binary, --headless
   │  (headless) │
   │             │
   └──────┬──────┘
          │
          ├── Local API on ship LAN (crew browser → map)
          │
          └── When internet available:
              └── Syncs to shore ORP (or other vessels)
```

```bash
# On the Raspberry Pi
orp start \
  --headless \
  --connector nmea://serial:///dev/ttyUSB0:38400 \
  --peer https://shore.example.com/orp \
  --sync-interval 30s
```

Parses every NMEA sentence. Builds a live picture of the vessel and surrounding traffic. Serves a local map on the ship LAN. Syncs deltas to shore when connected. Uses ~80MB RAM at steady state.

### Multi-Node Mesh

```
Vessel A ──────────────────▶ Shore COP
         ◀──── sync ───────
Vessel B ──────────────────▶ Shore COP
         ◀──── sync ───────
Vessel C ──────────────────▶ Shore COP
```

Each node shares only what its ABAC policy permits. Conflict resolution is automatic: highest-confidence source wins. Bandwidth is minimal: only deltas are transmitted.

---

## CLI Reference

```bash
orp <command> [options]
```

| Command | Description |
|---------|-------------|
| `orp start` | Start the ORP server |
| `orp start --template maritime` | Start with pre-configured maritime connectors |
| `orp start --headless` | Start without web UI (edge/embedded mode) |
| `orp start --port 9090` | Override default port |
| `orp query "<ORP-QL>"` | Run an ORP-QL query from the CLI |
| `orp query -f query.ql` | Run a query from a file |
| `orp status` | Show server health and entity counts |
| `orp connectors list` | List all configured connectors |
| `orp connectors add <spec>` | Add a connector at runtime |
| `orp connectors remove <id>` | Remove a connector |
| `orp entities list` | List recent entities |
| `orp entities get <id>` | Get a specific entity |
| `orp events tail` | Stream live entity events to stdout |
| `orp monitors list` | List configured alert monitors |
| `orp config show` | Print current configuration |
| `orp config set <key> <value>` | Update a config value |
| `orp version` | Show version information |
| `orp completions <shell>` | Generate shell completions |

Full CLI reference → [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)

---

## API Reference

**Base URL:** `http://localhost:9090/api/v1`  
**Auth:** `Authorization: Bearer <token>` or `X-API-Key: <key>`

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/entities` | List all entities (paginated) |
| `GET` | `/entities/:id` | Get a specific entity |
| `GET` | `/entities/:id/history` | Get entity history |
| `GET` | `/entities/:id/graph` | Get entity's graph neighborhood |
| `POST` | `/ingest` | Universal ingest — POST any JSON |
| `POST` | `/query` | Execute an ORP-QL query |
| `GET` | `/connectors` | List connectors |
| `POST` | `/connectors` | Add a connector |
| `DELETE` | `/connectors/:id` | Remove a connector |
| `GET` | `/monitors` | List alert monitors |
| `POST` | `/monitors` | Create a monitor |
| `GET` | `/alerts` | List recent alerts |
| `GET` | `/health` | Health check |
| `GET` | `/metrics` | Prometheus metrics |
| `WS` | `/ws` | WebSocket — live entity stream |

**Universal Ingest** (the simplest possible integration):
```bash
curl -X POST http://localhost:9090/api/v1/ingest \
  -H "Content-Type: application/json" \
  -d '{
    "entity_type": "Vehicle",
    "name": "Patrol Unit 7",
    "position": { "lat": 51.505, "lon": -0.09 },
    "speed": 45.2,
    "source": "fleet_tracker"
  }'
```

ORP normalizes it, resolves it against existing entities, and it appears on the map within 100ms.

Full API reference → [docs/API_REFERENCE.md](docs/API_REFERENCE.md)

---

## Security

ORP is built for environments where data integrity is non-negotiable.

| Feature | Details |
|---------|---------|
| **ABAC** | Attribute-Based Access Control — permissions tied to entity type, source, classification, and tenant |
| **Ed25519 Signing** | Every entity carries a cryptographic signature from its source. Tampered data is rejected. |
| **JWT / OIDC** | Standard bearer token auth. Integrates with Keycloak, Auth0, Okta. |
| **API Keys** | For programmatic access and service accounts |
| **TLS** | HTTPS/WSS in production. Certificate auto-management via ACME. |
| **Multi-tenant** | Hard data isolation between organizations. No cross-tenant leakage. |
| **Audit Log** | Every read and write is logged with actor, timestamp, and trace ID. |

Full security docs → [docs/SECURITY.md](docs/SECURITY.md)

---

## Performance

ORP is written in Rust. These are design targets, not marketing numbers.

| Metric | Target |
|--------|--------|
| Entity ingest throughput | 50,000 updates/sec (single node) |
| Query latency (simple) | < 5ms p99 |
| Query latency (graph, depth 3) | < 50ms p99 |
| WebSocket push latency | < 100ms end-to-end |
| Memory at 100K entities | ~512MB |
| Binary size | 43MB (static, no dependencies) |
| ARM build (Raspberry Pi 4) | ✅ Supported |
| Cold start time | < 2 seconds |

---

## Building from Source

```bash
# Prerequisites: Rust 1.75+
git clone https://github.com/shieldofsteel/orp
cd orp

# Run all tests
cargo test

# Build release binary
cargo build --release
# Binary: target/release/orp (43MB, statically linked)

# Cross-compile for Raspberry Pi (ARM64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# Docker
docker build -t orp .
docker run -p 9090:9090 orp start --template maritime
```

---

## Why Not TAK / FreeTAKServer / Palantir?

| | TAK Server | FreeTAKServer | Palantir | **ORP** |
|--|-----------|---------------|----------|---------|
| Open source | Restricted GOSS | EPL | ❌ | **Apache 2.0** |
| Modern web UI | ❌ Android-first | ❌ | ✅ | **✅** |
| Maritime domain | Minimal | Minimal | Limited | **First-class** |
| Aviation domain | ❌ | ❌ | Limited | **✅ ADS-B, ASTERIX** |
| Protocol parsers | CoT only | CoT only | Proprietary | **17 open adapters** |
| Multi-tenant SaaS | ❌ | ❌ | ✅ | **✅** |
| AI/anomaly detection | ❌ | ❌ | ✅ | **✅** |
| Edge / Raspberry Pi | ❌ | ⚠️ | ❌ | **✅ 43MB, headless** |
| Query language | ❌ | ❌ | Proprietary | **ORP-QL (open)** |
| Federation mesh | Limited | Limited | ✅ | **✅** |
| Cost | Free | Free | **$50–500M** | **Free** |

---

## Contributing

ORP is early. The protocol universe is large. Help is welcome.

**Highest-impact contributions:**
1. **New protocol adapters** — NMEA 2000, DNP3, Zeek, NetFlow, GTFS-RT. See [docs/CONNECTOR_GUIDE.md](docs/CONNECTOR_GUIDE.md) — a basic adapter is ~50 lines of Rust.
2. **Test coverage** — 538 tests is a start. More protocol parsing tests, more edge cases.
3. **Frontend features** — React/TypeScript. See [frontend/src/components/](frontend/src/components/).
4. **Documentation** — real-world deployment guides, integration recipes.
5. **Performance** — benchmarks, profiling, optimization.

```bash
# Run tests
cargo test

# Check lints (must be zero warnings)
cargo clippy -- -D warnings

# Format
cargo fmt

# Run a specific adapter test
cargo test -p orp-connector nmea
```

Issues are tracked on GitHub. PRs welcome. No CLA required.

---

## License

Apache 2.0 — see [LICENSE](LICENSE).

Use it commercially. Fork it. Embed it in products. Build a business on it. The only thing you can't do is sue us for patent infringement using patents you contributed.

---

<div align="center">

**ORP** — because operational awareness shouldn't cost $50M.

[⭐ Star on GitHub](https://github.com/shieldofsteel/orp) · [📖 Docs](docs/) · [🐛 Issues](https://github.com/shieldofsteel/orp/issues) · [💬 Discussions](https://github.com/shieldofsteel/orp/discussions)

</div>
