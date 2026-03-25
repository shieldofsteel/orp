# ORP — Build Specification for Engineering Teams

**Classification:** Engineering Build Spec — Distribute to all engineering teams
**Version:** 1.0 FINAL
**Date:** March 26, 2026
**Audience:** 490 senior software engineers
**Total Spec Size:** ~7,500 lines across 4 files

---

## READ THIS FIRST (5 Minutes)

### What We're Building

ORP (Open Reality Protocol) is a single Rust binary (~250-350MB) that does what Palantir charges $50-500 million for: it connects to real-time data sources, fuses them into a live knowledge graph, and lets humans and AI query, reason about, and simulate that reality. Everything runs locally. No cloud. No external dependencies. No setup.

**Install → Run → Live data on screen in 5 minutes.**

### Why It Matters

Palantir: $2.8B/year revenue, deployed across every US military command and NATO. Anduril Lattice: $20B Army contract. Both proprietary. Both locked behind defense contracts.

ORP open-sources this capability. A disaster response agency, a city planner, a climate researcher — anyone — downloads one binary and gets Palantir-grade data fusion for free.

### The Pattern

| Precedent | What It Replaced | Binary Size | Impact |
|---|---|---|---|
| SQLite | Oracle/MySQL servers | <1MB | In every device on earth |
| DuckDB | Apache Spark clusters ($100K+/yr) | ~50MB | 100x faster, 50% YoY growth |
| llama.cpp | OpenAI API (cloud, $$$) | ~100MB | LLMs on laptops |
| **ORP** | **Palantir ($50-500M/deployment)** | **~300MB** | **Data fusion on laptops** |

### The Binary

```
ORP Binary (~250-350MB)
├── Rust Core (40MB) — Connectors, stream processor, HTTP/WS server
├── DuckDB (50MB) — OLAP queries, geospatial, columnar storage
├── Kuzu (40MB) — Graph queries, path traversal, relationship walks
├── RocksDB (20MB) — Stream state, deduplication, windowing
├── Built-in Connectors (30MB) — AIS, ADS-B, HTTP, MQTT, CSV, WebSocket
├── React Frontend (20MB) — Map (Deck.gl), entity inspector, query bar
└── Runtime Libs (60MB) — Tokio, Proj, GEOS

Separate (Phase 2):
├── AI Model: Phi-2 2.7B quantized (~1.6GB) — Downloaded on first NL query
└── WASM Plugin SDK — Custom connector runtime
```

### How Data Flows

```
┌──────────────────────────────────────────────────────────────────────┐
│                        DATA SOURCES                                  │
│  AIS Feed   ADS-B Feed   Weather API   MQTT Sensor   CSV File      │
└──────┬──────────┬───────────┬────────────┬─────────────┬────────────┘
       │          │           │            │             │
       ▼          ▼           ▼            ▼             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  CONNECTORS (orp-connector crate)                                    │
│  Each connector: parse protocol → emit OrpEvent structs              │
└────────────────────────────┬─────────────────────────────────────────┘
                             │ Vec<OrpEvent> via async channel
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  STREAM PROCESSOR (orp-stream crate)                                 │
│  Dedup (RocksDB) → Window → Batch (1K events) → Entity Resolution   │
└────────────────────────────┬─────────────────────────────────────────┘
                             │ Batch insert
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  STORAGE LAYER (orp-storage crate)                                   │
│                                                                      │
│  ┌─────────────┐    Sync (30s)    ┌─────────────┐                   │
│  │   DuckDB    │ ───────────────► │    Kuzu     │                   │
│  │   (OLAP)    │                  │   (Graph)   │                   │
│  │  entities   │                  │  Ship nodes │                   │
│  │  events     │                  │  Port nodes │                   │
│  │  properties │                  │  Edges      │                   │
│  └──────┬──────┘                  └──────┬──────┘                   │
│         │                                │                           │
│         └──────────┬─────────────────────┘                           │
│                    │ Unified Storage trait                            │
└────────────────────┼─────────────────────────────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────────────────────────────┐
│  QUERY ENGINE (orp-query crate)                                      │
│  ORP-QL parser → Query planner → Route to DuckDB or Kuzu → Merge    │
└────────────────────────────┬─────────────────────────────────────────┘
                             │ JSON results
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  API LAYER (orp-core/server)                                         │
│  Axum HTTP server (REST + WebSocket)                                 │
│  GET /api/v1/entities/search  POST /api/v1/query  /ws/updates       │
└────────────────────────────┬─────────────────────────────────────────┘
                             │ JSON/WebSocket
                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  FRONTEND (React + Deck.gl + CesiumJS)                               │
│  Map ← Entity Inspector ← Query Bar ← Timeline ← Alert Feed        │
│  Served from embedded static assets at localhost:9090                 │
└──────────────────────────────────────────────────────────────────────┘
```

---

## DOCUMENT MAP

This build spec is split across 4 files. Every engineer should read **this file** (Powerful.md) first, then their team's relevant spec files.

| File | Lines | Contents | Who Reads It |
|---|---|---|---|
| **Powerful.md** (this file) | ~800 | Vision, architecture overview, team assignments, coding standards, Definition of Done | Everyone |
| **BUILD_CORE_ENGINE.md** | 2,466 | Cargo workspace, DuckDB/Kuzu schemas, Rust traits, event schema, config schema, data flow, error handling, testing, build pipeline | Core Engine, Storage, Stream, Query, Entity Resolution, DevOps teams |
| **BUILD_API_FRONTEND.md** | 2,848 | REST API (OpenAPI 3.1), WebSocket protocol, React component tree, Deck.gl map layers, ORP-QL grammar, auth flows, error contracts | API, Frontend, Security, SDK teams |
| **BUILD_TEAMS_SPRINTS.md** | 1,998 | 20 team definitions (490 engineers), week-by-week sprint plan (42 weeks), critical path, PR standards, communication plan, hiring ramp | Leads, PMs, all teams |
| **ORP_MASTER.md** | 600 | Product vision, market context, revenue model, competitive analysis, personas, risks | Leadership, Product, DevRel |

---

## TEAM STRUCTURE (Summary)

490 engineers organized into 20 teams:

### Tier 1: Core (Must Ship Phase 1)

| Team | Size | Lead(s) | Mission | Key Deliverable |
|---|---|---|---|---|
| **Core Engine** | 8 | 1 Principal | Binary orchestration, startup, CLI, build system | `orp start` works end-to-end |
| **Storage** | 15 | 1 Staff | DuckDB + Kuzu integration, sync service, storage traits | 1M entities, <200ms simple queries |
| **Stream Processing** | 35 | 2 Staff | Ingest pipeline, dedup, windowing, batching, backpressure | 100K events/sec sustained |
| **Connectors** | 60 | 3 Staff | AIS, ADS-B, HTTP, MQTT, CSV, WebSocket adapters | 6 connectors, all passing integration tests |
| **Entity Resolution** | 40 | 2 Staff | MMSI/ICAO matching, merge/split, conflict resolution | 99%+ accuracy on structural matching |
| **Query Engine** | 45 | 2 Staff | ORP-QL parser, planner, executor, optimizer | MATCH/WHERE/RETURN, geospatial, <500ms |
| **Graph Engine** | 30 | 1 Staff | Kuzu integration, sync from DuckDB, graph queries | 3-hop traversal <1s on 1M entities |
| **Frontend** | 50 | 2 Staff | Console UI, map, entity inspector, query bar, timeline | 60fps with 50K visible entities |
| **API** | 30 | 1 Staff | REST endpoints, WebSocket, rate limiting, versioning | All endpoints per OpenAPI spec |
| **Security** | 25 | 1 Staff | OIDC, ABAC, Ed25519 signing, audit log, pen testing | Auth works, ABAC enforced, audit log immutable |

### Tier 2: Quality & Infrastructure

| Team | Size | Mission |
|---|---|---|
| **DevOps** | 40 | CI/CD, cross-compilation (4 targets), binary distribution, Helm charts, benchmarks |
| **Testing/QA** | 25 | Integration tests, load tests, chaos testing, benchmark suite, property tests |
| **Documentation** | 20 | Architecture docs (200+ pages), API reference, tutorials, getting started, contributor guide |

### Tier 3: Phase 2 Preparation (Start Month 4)

| Team | Size | Mission |
|---|---|---|
| **AI/ML** | 35 | llama.cpp integration, NL→query, anomaly detection, model fine-tuning |
| **Enterprise** | 20 | SSO/SAML, compliance reporting, clustering, audit exports |
| **Cloud** | 30 | Managed ORP service — multi-tenant, scaling, billing, monitoring |
| **SDK** | 22 | Python/JS/Go SDKs, WASM plugin SDK, developer portal |
| **Simulation** | 18 | Agent-based models, scenario forking, Ray integration |
| **Mobile** | 15 | React Native companion app, push notifications |
| **Edge** | 12 | Lightweight binary, ARM optimization, resource-constrained deployment |
| **Community/DevRel** | 18 | Tap registry, contributor tooling, community calls, content |

---

## TECHNOLOGY DECISIONS (Final, Do Not Revisit Without RFC)

| Decision | Choice | Rationale | ADR |
|---|---|---|---|
| Language | **Rust** | Performance, safety, single binary, no GC | ADR-001 |
| OLAP Engine | **DuckDB** (embedded) | Fastest embedded OLAP, spatial extensions, MIT license | ADR-002 |
| Graph Engine | **Kuzu** (embedded) | Purpose-built embeddable graph, columnar backend, MIT license | ADR-003 |
| Stream State | **RocksDB** (embedded) | Battle-tested, fast writes, compaction, Apache 2.0 | ADR-004 |
| Config Store | **SQLite** (embedded) | Universal, zero-config, public domain | ADR-005 |
| HTTP Framework | **Axum** (Rust) | Tokio-native, tower middleware, type-safe extractors | ADR-006 |
| Async Runtime | **Tokio** | Industry standard, mature, well-documented | ADR-006 |
| Frontend Framework | **React 18+** | Largest ecosystem, team familiarity, Deck.gl integration | ADR-007 |
| Map Library | **Deck.gl** (2D) + **CesiumJS** (3D) | WebGL performance, geospatial-native, open source | ADR-007 |
| State Management | **Zustand** | Minimal boilerplate, works with React Query, simpler than Redux | ADR-007 |
| Data Fetching | **TanStack Query** (React Query) | Caching, refetching, optimistic updates, industry standard | ADR-007 |
| Schema Language | **JSON-LD + SHACL** | W3C standard, federated, composable | ADR-008 |
| Crypto Signing | **Ed25519** | Fast, secure, small keys/signatures | ADR-009 |
| Auth | **OIDC** (OpenID Connect) | Industry standard, works with any IdP | ADR-010 |
| LLM Inference (Phase 2) | **llama.cpp** | Embedded, fast CPU inference, MIT license | ADR-011 |
| LLM Model (Phase 2) | **Phi-2** (2.7B, Q4_K quantized) | Best accuracy at embeddable size, MIT license | ADR-011 |
| Plugin System (Phase 2) | **WASM** (wasmtime) | Sandboxed, language-agnostic, fast enough for I/O | ADR-012 |

**Rejected:**
- Custom graph layer on DuckDB → 100-1000x slower for path queries than Kuzu
- SurrealDB → too immature
- Apache Flink/Kafka → distributed complexity, not needed for single-binary
- PostgreSQL + PostGIS → not embeddable without separate process
- Embedding LLM in binary → model too large (1.6GB min)
- Blockchain → slow, complex, zero benefit

---

## CODING STANDARDS

### Rust

```
Edition: 2021
MSRV: 1.75+
Lints: #![warn(clippy::all, clippy::pedantic)]
Formatting: rustfmt (default config, enforced in CI)
Error handling: thiserror for library crates, anyhow for binary crate
Async: tokio (multi-threaded runtime)
Logging: tracing crate (structured, spans)
Serialization: serde + serde_json
CLI: clap v4 (derive)
Testing: cargo test + cargo bench (criterion)
```

### TypeScript (Frontend)

```
Strict mode: true
Framework: React 18+ with hooks (no class components)
Styling: Tailwind CSS (utility classes only)
State: Zustand
Fetching: TanStack Query
Types: All props typed, no `any`
Linting: ESLint + Prettier (enforced in CI)
Testing: Vitest + React Testing Library
Build: Vite
```

### Git

```
Branch naming: {type}/{ticket}-{short-description}
  Types: feature/, bugfix/, refactor/, docs/, perf/, test/
  Example: feature/ORP-142-ais-connector

Commit format (Conventional Commits):
  {type}({scope}): {description}
  Example: feat(connector): add AIS NMEA sentence parser

PR requirements:
  - 2 approvals (1 must be from owning team's Staff+ engineer)
  - All CI gates green (tests, clippy, fmt, binary size check, security audit)
  - No unresolved threads
  - Squash merge to main

Binary size gate:
  - CI measures binary size on every PR
  - Alert if delta > +5MB
  - Block if total > 400MB

Performance gate:
  - CI runs benchmark suite on every PR to main
  - Alert if any benchmark regresses > 10%
  - Block if any P50 latency doubles
```

---

## PERFORMANCE TARGETS (Phase 1)

Every team must hit these. CI enforces them.

| Metric | Target | Measured By | Gate |
|---|---|---|---|
| Binary size | <350MB (without AI model) | CI on every PR | Block if >400MB |
| Startup time | <5 seconds (cold start to HTTP ready) | Benchmark suite | Alert if >8s |
| Simple query (ships near location) P50 | <200ms | Benchmark suite | Alert if >500ms |
| Temporal + spatial query P50 | <800ms | Benchmark suite | Alert if >2s |
| Graph query (3-hop) P50 | <1s | Benchmark suite | Alert if >3s |
| Stream throughput | 100K events/sec sustained | Load test | Alert if <50K |
| Memory (1M entities under load) | <3GB | Load test | Alert if >5GB |
| Map rendering (50K entities) | 60fps | Frontend benchmark | Alert if <30fps |
| WebSocket update latency | <100ms (event → client) | E2E test | Alert if >500ms |
| Time from `orp start` to data on screen | <3 minutes | E2E test | Alert if >5 min |

---

## PHASE 1 MILESTONES (10 Months)

| Month | Milestone | What Gets Demoed | Go/No-Go Gate |
|---|---|---|---|
| **0** (Wks 1-6) | Design Sprint Complete | Architecture prototypes, benchmarks | DuckDB+Kuzu 1M entities, <500ms query |
| **2** | Core + AIS Tap | Ship positions on a map, live | Stream throughput >50K events/sec |
| **3** | Multi-source + Graph | AIS + ADS-B + weather fused, graph queries work | Entity resolution >99%, Kuzu 3-hop <1s |
| **4** | Query Engine | ORP-QL queries via API, complex geospatial | ORP-QL parser covers full v0.1 grammar |
| **5** | **REALITY CHECK** | Full demo: map + queries + alerts + timeline | All P50 targets met. If not: STOP and fix. |
| **6** | Console UI v1 | Complete web dashboard, polished | 60fps map, entity inspector, query bar |
| **7** | Monitors + Alerts | Automated anomaly detection, alert feed | False positive rate <10% |
| **8** | Security Hardening | Pen test results, ABAC demo | No critical/high vulnerabilities |
| **9** | Documentation | 200+ page docs site, tutorials, API ref | External reviewers can follow getting started |
| **10** | **PUBLIC ALPHA** | Live demo, binary download, HackerNews | 500+ downloads in first week |

---

## THE ORP EVENT (Canonical Data Format)

Every piece of data flowing through ORP is an OrpEvent. This is the universal contract between all components.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrpEvent {
    /// Globally unique event ID (UUIDv7 — time-ordered)
    pub id: Uuid,

    /// Entity type (e.g., "ship", "port", "weather_system", "aircraft")
    pub entity_type: String,

    /// Entity identifier (e.g., "mmsi:123456789", "icao:A1B2C3")
    pub entity_id: String,

    /// When this observation was made (source timestamp, not ingestion time)
    pub timestamp: DateTime<Utc>,

    /// Geospatial location (optional — not all events have position)
    pub geo: Option<GeoPoint>,

    /// Event-specific payload
    pub payload: EventPayload,

    /// Which connector produced this event
    pub source_id: String,

    /// Source reliability (0.0 = unknown, 1.0 = verified)
    pub confidence: f64,

    /// Ed25519 signature of the event (set by connector)
    pub signature: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,  // meters above sea level
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    PositionUpdate {
        course: Option<f64>,    // degrees (0-360)
        speed: Option<f64>,     // knots
        heading: Option<f64>,   // degrees
    },
    PropertyChange {
        key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },
    StateTransition {
        from_state: String,
        to_state: String,
    },
    RelationshipChange {
        relationship_type: String,
        target_entity_id: String,
        action: RelationshipAction, // Created, Updated, Deleted
    },
    AlertTriggered {
        monitor_id: String,
        severity: AlertSeverity,  // Info, Warning, Critical
        message: String,
        evidence: serde_json::Value,
    },
    Custom {
        data: serde_json::Value,
    },
}
```

**JSON Example (AIS position update):**
```json
{
  "id": "01965a3b-7c4d-7def-8a12-3456789abcde",
  "entity_type": "ship",
  "entity_id": "mmsi:123456789",
  "timestamp": "2026-03-26T14:30:00Z",
  "geo": { "lat": 51.9225, "lon": 4.4792, "alt": null },
  "payload": {
    "type": "PositionUpdate",
    "course": 245.0,
    "speed": 12.3,
    "heading": 243.0
  },
  "source_id": "ais-tap-01",
  "confidence": 0.95,
  "signature": "base64..."
}
```

---

## DUCKDB SCHEMA (Core Tables)

```sql
-- Entities: the canonical record of every real-world thing ORP knows about
CREATE TABLE entities (
    entity_id       VARCHAR PRIMARY KEY,  -- "mmsi:123456789"
    entity_type     VARCHAR NOT NULL,     -- "ship", "port", "aircraft"
    name            VARCHAR,
    created_at      TIMESTAMP NOT NULL DEFAULT current_timestamp,
    updated_at      TIMESTAMP NOT NULL DEFAULT current_timestamp,
    source_id       VARCHAR NOT NULL,     -- which connector created this
    confidence      DOUBLE DEFAULT 0.5,
    is_active       BOOLEAN DEFAULT true
);

-- Geospatial positions (latest known position per entity)
CREATE TABLE entity_geometry (
    entity_id       VARCHAR PRIMARY KEY REFERENCES entities(entity_id),
    position        GEOMETRY NOT NULL,    -- POINT, LINESTRING, or POLYGON
    course          DOUBLE,
    speed           DOUBLE,
    heading         DOUBLE,
    altitude        DOUBLE,
    geo_updated_at  TIMESTAMP NOT NULL
);
CREATE INDEX idx_entity_geo ON entity_geometry USING RTREE (position);

-- Dynamic properties (typed key-value per entity)
CREATE TABLE entity_properties (
    entity_id       VARCHAR NOT NULL REFERENCES entities(entity_id),
    key             VARCHAR NOT NULL,
    value           VARCHAR NOT NULL,     -- JSON-encoded
    value_type      VARCHAR NOT NULL,     -- "string", "number", "boolean", "json"
    updated_at      TIMESTAMP NOT NULL,
    source_id       VARCHAR NOT NULL,
    confidence      DOUBLE DEFAULT 0.5,
    PRIMARY KEY (entity_id, key)
);

-- Events: every state change, position update, or observation
CREATE TABLE events (
    event_id        VARCHAR PRIMARY KEY,  -- UUIDv7
    entity_id       VARCHAR NOT NULL,
    entity_type     VARCHAR NOT NULL,
    event_type      VARCHAR NOT NULL,     -- "position_update", "property_change", etc.
    timestamp       TIMESTAMP NOT NULL,
    geo             GEOMETRY,
    payload         JSON NOT NULL,
    source_id       VARCHAR NOT NULL,
    confidence      DOUBLE DEFAULT 0.5,
    signature       BLOB
);

-- Relationships between entities
CREATE TABLE relationships (
    relationship_id VARCHAR PRIMARY KEY,
    source_id       VARCHAR NOT NULL REFERENCES entities(entity_id),
    target_id       VARCHAR NOT NULL REFERENCES entities(entity_id),
    rel_type        VARCHAR NOT NULL,     -- "docked_at", "heading_to", "owns", etc.
    properties      JSON,
    created_at      TIMESTAMP NOT NULL,
    updated_at      TIMESTAMP NOT NULL,
    confidence      DOUBLE DEFAULT 0.5,
    data_source_id  VARCHAR NOT NULL
);
CREATE INDEX idx_rel_source ON relationships(source_id);
CREATE INDEX idx_rel_target ON relationships(target_id);
CREATE INDEX idx_rel_type ON relationships(rel_type);

-- Registered data sources / connectors
CREATE TABLE data_sources (
    source_id       VARCHAR PRIMARY KEY,
    name            VARCHAR NOT NULL,
    connector_type  VARCHAR NOT NULL,     -- "ais", "adsb", "http", "mqtt", etc.
    config          JSON NOT NULL,
    status          VARCHAR DEFAULT 'active',
    trust_score     DOUBLE DEFAULT 0.5,
    events_ingested BIGINT DEFAULT 0,
    last_event_at   TIMESTAMP,
    created_at      TIMESTAMP NOT NULL DEFAULT current_timestamp,
    public_key      BLOB                  -- Ed25519 public key
);

-- Immutable audit log (hash-chained)
CREATE TABLE audit_log (
    seq_id          BIGINT PRIMARY KEY,   -- monotonically increasing
    timestamp       TIMESTAMP NOT NULL DEFAULT current_timestamp,
    actor           VARCHAR NOT NULL,     -- user_id, system, connector_id
    action          VARCHAR NOT NULL,     -- "entity_created", "query_executed", etc.
    target_type     VARCHAR,
    target_id       VARCHAR,
    details         JSON,
    prev_hash       VARCHAR(64) NOT NULL, -- SHA-256 of previous entry
    hash            VARCHAR(64) NOT NULL  -- SHA-256 of this entry
);
```

---

## KUZU GRAPH SCHEMA (Maritime Template)

```cypher
-- Node types
CREATE NODE TABLE Ship (
    entity_id STRING PRIMARY KEY,
    mmsi INT64,
    name STRING,
    ship_type STRING,
    flag STRING,
    lat DOUBLE,
    lon DOUBLE,
    speed DOUBLE,
    course DOUBLE,
    last_update TIMESTAMP
);

CREATE NODE TABLE Port (
    entity_id STRING PRIMARY KEY,
    name STRING,
    country STRING,
    lat DOUBLE,
    lon DOUBLE,
    capacity INT64,
    congestion DOUBLE
);

CREATE NODE TABLE Aircraft (
    entity_id STRING PRIMARY KEY,
    icao STRING,
    callsign STRING,
    lat DOUBLE,
    lon DOUBLE,
    altitude DOUBLE,
    speed DOUBLE
);

CREATE NODE TABLE WeatherSystem (
    entity_id STRING PRIMARY KEY,
    name STRING,
    system_type STRING,
    severity STRING,
    lat DOUBLE,
    lon DOUBLE,
    radius_km DOUBLE
);

CREATE NODE TABLE Organization (
    entity_id STRING PRIMARY KEY,
    name STRING,
    org_type STRING,
    country STRING
);

-- Relationship types
CREATE REL TABLE DOCKED_AT (FROM Ship TO Port, since TIMESTAMP, berth STRING);
CREATE REL TABLE HEADING_TO (FROM Ship TO Port, eta TIMESTAMP, distance_km DOUBLE);
CREATE REL TABLE OWNS (FROM Organization TO Ship, since TIMESTAMP);
CREATE REL TABLE OPERATES (FROM Organization TO Port, role STRING);
CREATE REL TABLE THREATENS (FROM WeatherSystem TO Port, severity STRING, eta TIMESTAMP);
CREATE REL TABLE NEAR (FROM Ship TO Ship, distance_km DOUBLE, duration_min DOUBLE);
CREATE REL TABLE FOLLOWS_ROUTE (FROM Ship TO Port, seq INT64, planned_arrival TIMESTAMP);
```

---

## API OVERVIEW (Key Endpoints)

Full spec in BUILD_API_FRONTEND.md. Summary:

| Method | Endpoint | Description |
|---|---|---|
| GET | /api/v1/entities | List entities (paginated, filtered) |
| GET | /api/v1/entities/{id} | Get entity details + properties + relationships |
| GET | /api/v1/entities/search | Geospatial + type + property search |
| GET | /api/v1/entities/{id}/relationships | Get entity's graph relationships |
| GET | /api/v1/entities/{id}/events | Get entity's event history |
| POST | /api/v1/query | Execute ORP-QL query |
| POST | /api/v1/query/natural | Natural language → ORP-QL → results (Phase 2) |
| POST | /api/v1/graph | Execute Kuzu Cypher query |
| GET | /api/v1/connectors | List active connectors |
| POST | /api/v1/connectors | Register new connector |
| GET | /api/v1/monitors | List monitor rules |
| POST | /api/v1/monitors | Create monitor rule |
| GET | /api/v1/health | System health + component status |
| GET | /api/v1/metrics | Prometheus-compatible metrics |
| WS | /ws/updates | Real-time entity updates (subscribe by type/region/id) |

**Standard Error Response (all endpoints):**
```json
{
  "error": {
    "code": "ENTITY_NOT_FOUND",
    "message": "Entity with id 'mmsi:999999999' not found",
    "status": 404,
    "request_id": "req_abc123",
    "timestamp": "2026-03-26T14:30:00Z"
  }
}
```

---

## DEFINITION OF DONE

### Feature
- [ ] Code written, compiles, passes `cargo clippy` with zero warnings
- [ ] Unit tests covering happy path + 2 error cases minimum
- [ ] Integration test if feature crosses crate boundaries
- [ ] Binary size delta measured and within budget
- [ ] Performance benchmark added (if latency-sensitive)
- [ ] API documentation updated (if public API changed)
- [ ] User-facing documentation updated (if behavior changed)
- [ ] 2 PR approvals (1 from Staff+ on owning team)
- [ ] All CI gates green

### Connector
- [ ] Implements `Connector` trait fully
- [ ] Integration test with real (or recorded) data
- [ ] YAML config example in documentation
- [ ] Error handling: graceful degradation on source failure
- [ ] Metrics: events_ingested, errors, latency exported
- [ ] Sample data fixture (30 days recorded) for testing
- [ ] Getting Started section in docs

### Bug Fix
- [ ] Regression test that fails without fix, passes with fix
- [ ] Root cause documented in PR description
- [ ] No performance regression (benchmark suite passes)

### Documentation Page
- [ ] Technically accurate (reviewed by owning team)
- [ ] Includes working code examples (tested)
- [ ] Follows style guide (consistent formatting, no jargon without definition)
- [ ] Cross-linked to related pages

---

## SECURITY NON-NEGOTIABLES

These apply from Day 1. No exceptions. No "we'll add it later."

1. **Every event is Ed25519 signed** at the connector level. The audit log can verify provenance of any data point.

2. **ABAC is enforced on every API call.** No endpoint returns data without checking the caller's attributes against the entity's access policy.

3. **The audit log is immutable and hash-chained.** Each entry's hash includes the previous entry's hash. Tampering is detectable.

4. **Connectors are sandboxed.** Each runs in its own async task with resource limits. Phase 2: WASM sandboxes with no host filesystem access.

5. **No telemetry, no phone-home, no analytics.** The binary does not contact any external server unless explicitly configured by the user.

6. **Cryptographic erasure for deletion.** When data must be removed (GDPR), destroy the encryption key. Ciphertext remains but is unrecoverable.

7. **All secrets in config are environment-variable-referenced.** Never store API keys, passwords, or signing keys in YAML config files. Use `${env.MY_SECRET}` syntax.

---

## WHAT SUCCESS LOOKS LIKE

### Month 5 (Reality Check)

```bash
$ orp start --template maritime

[Browser opens to localhost:9090]

- 2,000+ ships visible on live map, updating in real-time
- Click any ship: see MMSI, name, position, speed, heading, destination port
- Type "ships near Rotterdam" → 150 results in <200ms
- Type "ships where speed > 20" → filtered results
- Drag timeline slider → see positions from 24 hours ago
- Alert: "Ship MMSI:123456789 deviated from route by 52km"
- Graph query: "MATCH (s:Ship)-[:HEADING_TO]->(p:Port {name:'Rotterdam'}) RETURN s" → works
```

### Month 10 (Public Alpha)

Everything above, plus:
- Documentation site with 200+ pages
- `curl -fsSL https://orp.dev/install | sh` works on Linux + macOS
- First 500 external users
- Zero critical security vulnerabilities
- Performance targets all met
- HackerNews front page

---

## COMPANION FILES

For detailed specifications, read:

- **BUILD_CORE_ENGINE.md** — Cargo workspace, all schemas, all traits, event format, config format, error handling, testing strategy, build pipeline
- **BUILD_API_FRONTEND.md** — Full OpenAPI spec, WebSocket protocol, React components, Deck.gl layers, ORP-QL grammar, auth flows
- **BUILD_TEAMS_SPRINTS.md** — 20 team definitions, week-by-week sprint plan, critical path, communication plan, hiring ramp
- **ORP_MASTER.md** — Product vision, market analysis, revenue model, competitive landscape, user personas, risks

---

## MISSION

*"To make the tools for understanding and reasoning about physical reality universally accessible, free, and open — so that the power to see systemic risks, plan for the future, and coordinate collective action is not limited to those who can afford a defense contractor."*

---

**Start building.**
