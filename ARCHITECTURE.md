# ORP Architecture — Deep Dive

**Version:** 1.0 · **Date:** 2026-03-26 · **Audience:** Core contributors and integration engineers

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Component Diagram](#2-component-diagram)
3. [Data Flow](#3-data-flow)
4. [Storage Layer](#4-storage-layer)
5. [Stream Processing](#5-stream-processing)
6. [Query Engine](#6-query-engine)
7. [API Layer](#7-api-layer)
8. [Security Architecture](#8-security-architecture)
9. [Frontend Architecture](#9-frontend-architecture)
10. [Performance Architecture](#10-performance-architecture)
11. [Crate Dependency Graph](#11-crate-dependency-graph)
12. [Design Decisions](#12-design-decisions)

---

## 1. System Overview

ORP is a single Rust binary (~250–350 MB) that bundles everything required for real-time multi-source data fusion:

- **Connectors** — protocol-specific adapters that pull from AIS feeds, ADS-B, REST APIs, MQTT brokers, CSV files
- **Stream Processor** — deduplication, change detection, entity resolution, and batch insertion pipeline
- **Storage** — DuckDB for OLAP analytics + Kuzu for graph queries, synchronized every 30 seconds
- **Query Engine** — ORP-QL parser and planner that routes to DuckDB or Kuzu based on query shape
- **API** — Axum-based HTTP + WebSocket server exposing REST endpoints and real-time subscriptions
- **Frontend** — React SPA (Deck.gl map, entity inspector, query bar, alert feed) served from embedded static assets

No external process dependencies. No Docker required. No databases to configure.

---

## 2. Component Diagram

```
╔══════════════════════════════════════════════════════════════════════════╗
║                         ORP BINARY (~300 MB)                            ║
║                                                                          ║
║  ┌────────────────────────────────────────────────────────────────────┐ ║
║  │  CLI  (clap v4)                                                    │ ║
║  │  orp start  ·  orp query  ·  orp connector  ·  orp verify         │ ║
║  └──────────────────────┬─────────────────────────────────────────────┘ ║
║                         │                                                ║
║  ┌──────────────────────▼─────────────────────────────────────────────┐ ║
║  │  ORCHESTRATOR  (orp-core)                                          │ ║
║  │  Config loading · Component wiring · Tokio runtime bootstrap      │ ║
║  └──┬──────────┬──────────┬──────────┬──────────┬────────────────────┘ ║
║     │          │          │          │          │                        ║
║  ┌──▼──┐  ┌───▼───┐  ┌───▼───┐  ┌───▼───┐  ┌───▼──────────────────┐  ║
║  │CONN │  │STREAM │  │STORE  │  │QUERY  │  │ HTTP / WebSocket API  │  ║
║  │ECTORS│  │  PROC │  │LAYER  │  │ENGINE │  │ (Axum · Tower)        │  ║
║  └──┬──┘  └───┬───┘  └───┬───┘  └───┬───┘  └───────────────────────┘  ║
║     │          │          │          │                                   ║
║  ┌──▼──────────▼──┐  ┌────▼──────┐  └──► DuckDB executor               ║
║  │  OrpEvent bus  │  │  DuckDB   │       Kuzu executor                  ║
║  │  (tokio::mpsc) │  │  + Kuzu   │       Hybrid router                  ║
║  └────────────────┘  │  + Rocks  │                                      ║
║                       └───────────┘                                     ║
║                                                                          ║
║  ┌────────────────────────────────────────────────────────────────────┐ ║
║  │  SECURITY LAYER  (cross-cutting)                                   │ ║
║  │  OIDC token validation · ABAC enforcement · Ed25519 signing        │ ║
║  │  Hash-chained audit log · Cryptographic erasure                    │ ║
║  └────────────────────────────────────────────────────────────────────┘ ║
║                                                                          ║
╚══════════════════════════════════════════════════════════════════════════╝
           │ HTTP :9090        │ WebSocket :9090/ws
           ▼                   ▼
   ┌───────────────────────────────────────────┐
   │  BROWSER  (React + Deck.gl + CesiumJS)     │
   │  Served from embedded static at :9090/     │
   └───────────────────────────────────────────┘
```

---

## 3. Data Flow

### 3.1 Ingestion Path

```
Data Source
    │
    │ TCP/HTTP/MQTT/file
    ▼
┌─────────────────────────────┐
│  Connector Task             │
│  (one Tokio task per conn.) │
│  · Parse protocol           │
│  · Map to OrpEvent          │
│  · Ed25519 sign             │
│  · Send to event_tx channel │
└──────────────┬──────────────┘
               │ mpsc::Sender<OrpEvent>
               ▼
┌─────────────────────────────┐
│  Stream Processor           │
│  · Dedup (RocksDB window)   │
│  · Change detection         │
│  · Entity resolution        │
│  · Accumulate in batch vec  │
└──────────────┬──────────────┘
               │ every 1000 events or 1s
               ▼
┌─────────────────────────────┐
│  Batch Inserter             │
│  · BEGIN TRANSACTION        │
│  · Upsert entities          │
│  · Upsert entity_geometry   │
│  · Upsert entity_properties │
│  · INSERT events            │
│  · COMMIT                   │
└──────────────┬──────────────┘
               │ DuckDB write
               ▼
         ┌───────────┐
         │  DuckDB   │ ◄── Primary source of truth
         └─────┬─────┘
               │ every 30 seconds
               ▼
         ┌───────────┐
         │   Kuzu    │ ◄── Graph projection of DuckDB state
         └───────────┘
```

### 3.2 Query Path

```
HTTP POST /api/v1/query
{ "query": "MATCH (s:Ship)-[:HEADING_TO]->(p:Port) ..." }
    │
    ▼
┌─────────────────────────┐
│  Auth Middleware        │
│  · Validate JWT         │
│  · Load ABAC policy     │
└────────────┬────────────┘
             │
             ▼
┌─────────────────────────┐
│  ORP-QL Parser          │   (orp-query crate)
│  · Tokenize             │
│  · Build AST            │
│  · Validate syntax      │
└────────────┬────────────┘
             │ QueryAst
             ▼
┌─────────────────────────┐
│  Query Planner          │
│  · Cost estimation      │
│  · Predicate pushdown   │
│  · Route decision:      │
│    → graph query? Kuzu  │
│    → analytics? DuckDB  │
│    → hybrid? both       │
└────────────┬────────────┘
             │ PhysicalPlan
             ▼
┌─────────────────────────┐
│  Executor               │
│  · Run DuckDB SQL       │
│  · Run Kuzu Cypher      │
│  · Merge result sets    │
│  · Apply ABAC filters   │
│  · Serialize to JSON    │
└────────────┬────────────┘
             │ JSON response
             ▼
     HTTP 200 + results

(Latency budget: parse 5ms · plan 10ms · execute <185ms · serialize 5ms = <205ms P50)
```

### 3.3 WebSocket Push Path

```
DuckDB write (new entity update)
    │
    ▼
Notification channel (tokio::broadcast)
    │
    ▼
WebSocket fanout task
    │ for each connected client:
    ├── check subscription filter (bbox, type, id)
    ├── check ABAC (can this client see this entity?)
    └── if match: serialize → send via WS frame

(Target: event arrival → client frame < 100 ms)
```

---

## 4. Storage Layer

### 4.1 DuckDB — Primary Store

DuckDB serves as the primary analytical store. It is embedded (no separate process) and handles columnar OLAP queries, geospatial predicates, and temporal filtering.

**Core Tables:**

| Table | Purpose | Key Columns |
|-------|---------|-------------|
| `entities` | Canonical record of every real-world thing | `entity_id (PK)`, `entity_type`, `name`, `confidence` |
| `entity_geometry` | Latest known position per entity | `entity_id (FK)`, `position (GEOMETRY)`, `course`, `speed` |
| `entity_properties` | Dynamic typed key-value per entity | `entity_id`, `key`, `value (JSON)`, `confidence` |
| `events` | Every state change / observation | `event_id (UUIDv7)`, `entity_id`, `timestamp`, `payload (JSON)` |
| `relationships` | Edges between entities | `source_id`, `target_id`, `rel_type`, `properties (JSON)` |
| `data_sources` | Registered connectors | `source_id`, `connector_type`, `trust_score`, `public_key` |
| `audit_log` | Immutable hash-chained log | `seq_id`, `actor`, `action`, `prev_hash`, `hash` |

**Spatial Indexing:**

```sql
-- RTREE index on geometry for fast bbox / nearest-neighbor queries
CREATE INDEX idx_entity_geo ON entity_geometry USING RTREE (position);

-- Time-ordered index for temporal queries
CREATE INDEX idx_events_time ON events (entity_id, timestamp DESC);
```

**Write Pattern:** Batch inserts every 1–5 seconds, ~1,000 events per batch, wrapped in a transaction. This amortizes DuckDB's per-transaction overhead and achieves > 100K events/sec throughput on commodity hardware.

### 4.2 Kuzu — Graph Store

Kuzu is a purpose-built embeddable property graph database with a columnar storage backend. ORP uses it exclusively for relationship-heavy queries: path traversal, reachability, and multi-hop graph walks that would be expensive as self-joins in DuckDB.

**Node Types (Maritime Template):**

```cypher
Ship(entity_id, mmsi, name, ship_type, flag, lat, lon, speed, course, last_update)
Port(entity_id, name, country, lat, lon, capacity, congestion)
Aircraft(entity_id, icao, callsign, lat, lon, altitude, speed)
WeatherSystem(entity_id, name, system_type, severity, lat, lon, radius_km)
Organization(entity_id, name, org_type, country)
```

**Relationship Types:**

```cypher
DOCKED_AT     (Ship → Port)       : since, berth
HEADING_TO    (Ship → Port)       : eta, distance_km
OWNS          (Organization → Ship) : since
OPERATES      (Organization → Port) : role
THREATENS     (WeatherSystem → Port) : severity, eta
NEAR          (Ship → Ship)       : distance_km, duration_min
FOLLOWS_ROUTE (Ship → Port)       : seq, planned_arrival
```

**Sync Design:** The DuckDB → Kuzu sync runs as a background Tokio task every 30 seconds:

```
1. Query DuckDB for entities updated since last_sync_ts
2. For each changed entity: MERGE node in Kuzu (upsert by entity_id)
3. Query DuckDB for relationships updated since last_sync_ts
4. For each changed relationship: MERGE edge in Kuzu
5. Update last_sync_ts = now()
6. Emit sync metrics (entities_synced, edges_synced, duration_ms)
```

The sync is eventually consistent (up to 30 seconds behind DuckDB). Graph queries that need freshness < 30s are served by DuckDB with manual joins.

### 4.3 RocksDB — Stream State

RocksDB stores the ephemeral state needed by the stream processor:

- **Dedup window** — `event_hash → timestamp` (TTL: configurable, default 24 hours)
- **Entity state cache** — latest known snapshot of each entity (used for change detection)
- **Connector checkpoints** — byte offsets / sequence numbers for resume after restart

RocksDB state survives binary restarts. On startup, the stream processor resumes from the last checkpoint without reprocessing old events.

---

## 5. Stream Processing

### 5.1 Connector Trait

Every data source implements the `Connector` trait:

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn id(&self) -> &str;
    fn connector_type(&self) -> &str;
    async fn start(&self, tx: mpsc::Sender<OrpEvent>) -> Result<(), ConnectorError>;
    async fn stop(&self) -> Result<(), ConnectorError>;
    fn health(&self) -> ConnectorHealth;
    fn metrics(&self) -> ConnectorMetrics;
}
```

Each connector runs in its own Tokio task, isolated from others. A panicking connector logs the error and restarts with exponential backoff without taking down the binary.

### 5.2 Deduplication

The dedup stage prevents the same physical observation from being written twice. This matters because:

- AIS receivers often re-broadcast the same NMEA sentence (UDP multicast)
- Multiple connectors may ingest overlapping feeds

**Algorithm:**

```
1. Compute event_hash = SHA-256(entity_id + timestamp + payload_fields)
2. Check RocksDB: does event_hash exist with TTL > 0?
   a. YES → drop event (duplicate)
   b. NO  → write event_hash to RocksDB with TTL, forward event
```

The TTL window is configurable (default: 24 hours). Dedup state is stored on disk and survives restarts.

### 5.3 Change Detection

Change detection compares each incoming event against the entity's current state in the RocksDB cache:

```
Incoming event for entity E:
1. Load E's current state from RocksDB cache
2. Compute delta: which fields changed?
3. If delta is empty → no-op (position identical to last known)
4. If delta is non-empty → emit ChangeEvent with old + new values
5. Update RocksDB cache with new state
```

This drives the WebSocket fanout — clients only receive push notifications when something actually changes, not on every event arrival.

---

## 6. Query Engine

### 6.1 ORP-QL Grammar (v0.1, EBNF)

```ebnf
query         ::= match_clause where_clause? return_clause order_clause? limit_clause?
match_clause  ::= "MATCH" pattern ("," pattern)*
pattern       ::= node_pattern (rel_pattern node_pattern)*
node_pattern  ::= "(" alias ":" type properties? ")"
rel_pattern   ::= "-[" ":" rel_type "]" "->"
where_clause  ::= "WHERE" condition ("AND" condition)*
condition     ::= property_filter | geo_filter | temporal_filter
property_filter ::= alias "." prop op value
geo_filter    ::= ("near" | "within") "(" alias "." "position" "," geo_expr "," distance ")"
temporal_filter ::= "AT" "TIME" time_expr
return_clause ::= "RETURN" return_item ("," return_item)*
return_item   ::= alias "." prop | aggregate_fn | alias
order_clause  ::= "ORDER BY" alias "." prop ("ASC" | "DESC")?
limit_clause  ::= "LIMIT" integer
```

### 6.2 Query Planner — Routing Logic

```
QueryAst
    │
    ├── Has MATCH with relationship patterns?
    │   ├── YES, depth ≤ 3 hops → route to Kuzu
    │   └── YES, depth > 3 hops → route to DuckDB (recursive CTE)
    │
    ├── Has geospatial functions (near, within, bbox)?
    │   └── Route to DuckDB (RTREE index)
    │
    ├── Has GROUP BY / aggregate?
    │   └── Route to DuckDB
    │
    └── Has AT TIME clause?
        └── Route to DuckDB (events table temporal scan)
```

**Hybrid queries** (geospatial filter + graph traversal): DuckDB resolves the geospatial filter first (returns entity IDs), then Kuzu does the graph walk on the filtered set. Results are merged in Rust and returned as a single JSON array.

### 6.3 Performance Budget

| Stage | P50 Budget | Optimization |
|-------|-----------|--------------|
| Parse | 5 ms | LALRPOP-generated parser, zero-copy tokens |
| Plan | 10 ms | Cached query plan for identical queries |
| Execute (DuckDB) | 130 ms | RTREE index, predicate pushdown |
| Execute (Kuzu) | 700 ms | 3-hop budget |
| Merge + serialize | 15 ms | SIMD JSON serialization |
| **Total P50** | **< 200 ms** | _(simple queries)_ |

---

## 7. API Layer

### 7.1 HTTP Stack

```
Request
  │
  ▼
Tower middleware stack:
  ├── TraceLayer (structured logging, request IDs)
  ├── CompressionLayer (gzip/br for responses > 1 KB)
  ├── CorsLayer (configured origins)
  ├── TimeoutLayer (30 s hard timeout)
  ├── RateLimitLayer (per-client, token bucket)
  └── AuthLayer (JWT validation, ABAC policy load)
  │
  ▼
Axum Router
  ├── /api/v1/** → REST handlers
  ├── /ws/updates → WebSocket upgrade
  ├── /auth/** → OIDC flow
  ├── /api/v1/metrics → Prometheus metrics
  └── /** → Static frontend assets (embedded via include_dir!)
```

### 7.2 WebSocket Protocol

Connection lifecycle:

```
1. Client: GET /ws/updates + Upgrade: websocket
2. Server: 101 Switching Protocols
3. Client: { "type": "auth", "token": "Bearer ..." }
4. Server: { "type": "auth_ok", "user_id": "..." }
5. Client: { "type": "subscribe", "subscription_id": "sub1", "filter": {...} }
6. Server: { "type": "subscribed", "subscription_id": "sub1" }
7. Server: { "type": "entity_update", "entity": {...} }  (streamed continuously)
8. Client: { "type": "unsubscribe", "subscription_id": "sub1" }
9. Client: { "type": "ping" } / Server: { "type": "pong" }
```

The server maintains a fanout registry: a `tokio::broadcast` channel per active subscription. When a DuckDB write occurs, the notification system checks all active subscriptions and pushes matching updates.

---

## 8. Security Architecture

### 8.1 Authentication — OIDC Flow

```
Browser                    ORP Server               OIDC Provider
   │                           │                         │
   │  GET /auth/login           │                         │
   ├──────────────────────────► │                         │
   │                           │  redirect to /authorize │
   │ ◄────────────────────────────────────────────────── │
   │  user authenticates                                  │
   ├─────────────────────────────────────────────────────►
   │  code                                                │
   │ ◄─────────────────────────────────────────────────── │
   │  GET /auth/callback?code=...                         │
   ├──────────────────────────► │                         │
   │                           │  POST /token            │
   │                           ├────────────────────────► │
   │                           │  { access_token, ... }  │
   │                           │ ◄───────────────────────  │
   │  set httpOnly cookie       │                         │
   │ ◄────────────────────────── │                         │
   │  all subsequent API calls: Authorization: Bearer ... │
```

### 8.2 ABAC Policy Evaluation

ABAC rules are evaluated per request with < 10 ms overhead (cached after first evaluation per token):

```
Subject attributes (from JWT):
  - user.id, user.email, user.org_id
  - user.permissions: ["entities:read", "graph:read"]
  - user.clearance_level: "confidential"

Resource attributes (from storage):
  - resource.type: "Ship"
  - resource.sensitivity: "public"
  - resource.tags: ["maritime"]
  - resource.org_id: "org-456"

Environment attributes:
  - time.hour: 14
  - request.ip: "10.0.0.1"

Policy (example):
  ALLOW IF:
    user.permissions CONTAINS "entities:read"
    AND resource.sensitivity IN ["public", "internal"]
    AND (resource.org_id == user.org_id OR user.permissions CONTAINS "admin")
```

### 8.3 Ed25519 Event Signing

Every event is signed by the connector that produced it:

```
Connector startup:
  1. Load signing key from ${env.ORP_SIGNING_KEY_PATH}
  2. Register public key in data_sources table

For each event:
  1. Serialize event fields (deterministic JSON, sorted keys)
  2. Sign with Ed25519 private key
  3. Attach signature to OrpEvent.signature field

Verification:
  1. Load public key from data_sources WHERE source_id = event.source_id
  2. Verify signature against serialized event fields
  3. If invalid: log warning, mark event confidence = 0.0, still store
```

### 8.4 Audit Log Design

The audit log is append-only and hash-chained:

```sql
-- Each entry contains the SHA-256 of the previous entry
-- Tampering with any entry invalidates all subsequent hashes
INSERT INTO audit_log (seq_id, actor, action, target_type, target_id, details, prev_hash, hash)
VALUES (
  next_seq_id,
  'user:alice',
  'query_executed',
  'entities',
  NULL,
  '{"query": "MATCH (s:Ship) ...", "result_count": 42}',
  sha256_of_previous_row,
  sha256(concat(seq_id, actor, action, details, prev_hash))
);
```

**Verification command:**

```bash
orp verify --audit-log ~/.orp/data/audit.db
# Output: ✓ Audit log integrity verified (42,891 entries, chain unbroken)
```

### 8.5 Cryptographic Erasure (GDPR)

Instead of deleting encrypted data, ORP destroys the encryption key. Approach:

```
1. On entity creation: encrypt sensitive fields with AES-256-GCM using entity-specific DEK
2. Store DEK encrypted with master key (stored in OS keychain or HashiCorp Vault)
3. On erasure request: delete DEK from key store
4. Result: all ciphertext for that entity remains in DuckDB but is permanently unreadable
5. Log erasure in audit log (action: "cryptographic_erasure", target_id: entity_id)
```

---

## 9. Frontend Architecture

### 9.1 Component Tree

```
<App>
 ├── <AuthProvider>         (OIDC token management)
 ├── <DataProvider>         (TanStack Query + WebSocket integration)
 │    ├── useEntities()     (paginated entity list)
 │    ├── useWebSocket()    (real-time push updates)
 │    └── useQuery()        (ORP-QL execution)
 │
 ├── <Sidebar>
 │    ├── <ConnectorStatus> (health indicators per connector)
 │    ├── <SavedQueries>    (bookmarked ORP-QL queries)
 │    └── <AlertFeed>       (real-time alert notifications)
 │
 ├── <MapView>              (Deck.gl ScatterplotLayer + IconLayer)
 │    ├── ScatterplotLayer  (entity positions, color by type)
 │    ├── IconLayer         (ship heading arrows, aircraft icons)
 │    ├── HeatmapLayer      (density view at small zoom)
 │    └── PathLayer         (predicted routes, historical trails)
 │
 ├── <QueryBar>
 │    ├── <AutoComplete>    (ORP-QL syntax hints)
 │    ├── <QueryHistory>    (last 50 queries)
 │    └── <ResultsPanel>    (table + map highlight)
 │
 ├── <EntityInspector>      (slide-in panel on entity click)
 │    ├── <PropertyList>    (all properties with confidence/freshness)
 │    ├── <RelationshipGraph> (Cytoscape.js mini-graph)
 │    └── <EventTimeline>   (event history list)
 │
 └── <TimelineScrubber>     (bottom bar, drag to replay past state)
```

### 9.2 State Management

```
Zustand store (useAppStore):
  - selectedEntityId: string | null
  - mapViewState: { longitude, latitude, zoom, bearing, pitch }
  - activeQuery: string
  - queryResults: QueryResult | null
  - alerts: Alert[]
  - connectorStatus: Record<connectorId, ConnectorHealth>
  - timelineTs: Date | null   (null = live mode)

TanStack Query cache:
  - ['entities', filters] → PaginatedResponse<Entity>
  - ['entity', id] → EntityDetail
  - ['query', hash] → QueryResult   (cached 30s)
  - ['connectors'] → ConnectorStatus[]
```

---

## 10. Performance Architecture

### 10.1 Write Path Batching

Raw throughput bottleneck is DuckDB's transaction overhead (~5 ms/transaction). Solution: accumulate events in a `Vec<OrpEvent>` and flush every 1,000 events or 1 second (whichever comes first).

```
At 100K events/sec:
  - 100 batches/sec × 1000 events = 100K/sec
  - 100 transactions/sec × 5ms = 500ms of DuckDB I/O
  - Parallelism: 4 DuckDB writer threads = 125ms effective overhead
  - Net: > 100K/sec sustained ✓
```

### 10.2 Deduplication Window

RocksDB compaction keeps the dedup hash store small:

```
Default window: 24 hours
AIS at 30K events/sec × 86400 sec = 2.6B potential entries/day
With 32-byte hash keys + 8-byte TTL = ~120 GB? NO:
  → AIS generates ~50K unique MMSI × 1 position/10s = 5M unique events/day
  → 5M × 40 bytes = 200 MB in RocksDB ✓
  → Compacted with LZ4: ~60 MB ✓
```

### 10.3 Map Rendering

Deck.gl renders entities on the GPU using WebGL. Performance strategy:

- **LOD (Level of Detail):** At zoom < 6, show HeatmapLayer (aggregated density); at zoom 6–10, show ScatterplotLayer (dots); at zoom > 10, show IconLayer (ship silhouettes with heading arrows)
- **Data transfer:** Only push delta updates via WebSocket (changed entities, not full list)
- **Viewport culling:** Subscribe only to entities within the current viewport bbox + 20% buffer
- **Instanced rendering:** All ships share one GPU buffer; position updates are buffer sub-updates

### 10.4 Index Strategy

```sql
-- Geospatial: RTREE on position (DuckDB spatial extension)
CREATE INDEX idx_entity_geo ON entity_geometry USING RTREE (position);

-- Time queries: compound index on entity + time
CREATE INDEX idx_events_time ON events (entity_id, timestamp DESC);

-- Property lookups: partial index on common query patterns
CREATE INDEX idx_entity_type ON entities (entity_type);
CREATE INDEX idx_entities_active ON entities (entity_type) WHERE is_active = true;

-- Relationship traversal (DuckDB fallback for shallow graphs)
CREATE INDEX idx_rel_source ON relationships (source_id, rel_type);
CREATE INDEX idx_rel_target ON relationships (target_id, rel_type);
```

---

## 11. Crate Dependency Graph

```
orp (binary, orp-core)
├── orp-config          (YAML parsing, env substitution, template loading)
├── orp-proto           (OrpEvent, EventPayload, GeoPoint — shared types)
│
├── orp-connector       (connector trait + built-in connectors)
│   └── orp-proto
│
├── orp-stream          (stream processor: dedup, change detection, batching)
│   ├── orp-proto
│   └── orp-entity      (entity resolution: structural matching)
│       └── orp-proto
│
├── orp-storage         (DuckDB + Kuzu + RocksDB integration)
│   └── orp-proto
│
├── orp-query           (ORP-QL parser, planner, executor)
│   ├── orp-proto
│   └── orp-storage
│
├── orp-security        (OIDC, ABAC, Ed25519, audit enforcement)
│   ├── orp-proto
│   └── orp-audit
│       └── orp-proto
│
└── orp-geospatial      (PROJ / GEOS wrappers, coordinate transforms)
    └── orp-proto
```

External workspace dependencies:

```toml
tokio          = { version = "1", features = ["full"] }
axum           = { version = "0.7", features = ["ws"] }
tower          = { version = "0.4" }
tower-http     = { version = "0.5", features = ["cors", "compression-gzip", "trace"] }
serde          = { version = "1", features = ["derive"] }
serde_json     = { version = "1" }
chrono         = { version = "0.4", features = ["serde"] }
uuid           = { version = "1", features = ["v7", "serde"] }
tracing        = { version = "0.1" }
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
thiserror      = { version = "1" }
anyhow         = { version = "1" }
clap           = { version = "4", features = ["derive"] }
duckdb         = { version = "0.10", features = ["bundled"] }
rocksdb        = { version = "0.21", features = ["multi-threaded-cf"] }
```

---

## 12. Design Decisions

### ADR-001: DuckDB + Kuzu + RocksDB

**Context:** Need OLAP analytics, graph traversal, and high-speed stream state. Three separate concerns.

**Decision:** Use three embedded databases, each specializing:
- DuckDB: columnar OLAP, geospatial, temporal queries
- Kuzu: graph traversal, path queries
- RocksDB: mutable K/V stream state, dedup window

**Consequence:** ~110 MB of storage engines in the binary. But: no external processes, single-binary deploy, and each engine is best-in-class for its workload.

**Rejected:** PostgreSQL + PostGIS (requires separate process), SurrealDB (immature), Apache Flink (distributed complexity).

### ADR-002: Single Binary, No Microservices

**Context:** Target users are researchers, disaster response agencies, and developers who want _one command_ to get started.

**Decision:** Single static binary. All components are crates linked at compile time. No Docker required for basic usage.

**Consequence:** Binary is ~300 MB (large for a binary, tiny for what it does). Cross-compilation is required for all targets.

### ADR-003: Async Runtime — Tokio, Not Flink

**Context:** Stream processing could use Apache Flink (distributed, JVM) or a Tokio-based async pipeline.

**Decision:** Tokio. Single-process, single-binary constraint rules out JVM. Tokio achieves > 100K events/sec on modern hardware with the batch insert pattern.

**Consequence:** No built-in distributed mode. Horizontal scaling is a Phase 3 concern.

### ADR-004: ORP-QL Instead of SQL or Pure Cypher

**Context:** Users query both analytical data (DuckDB) and graph data (Kuzu). Neither pure SQL nor pure Cypher covers both.

**Decision:** ORP-QL: SQL-style filtering + Cypher-style MATCH patterns, compiled to either DuckDB SQL or Kuzu Cypher by the query planner.

**Consequence:** New language to learn. Mitigated by: (a) natural language queries in Phase 2, (b) familiar syntax for anyone who knows SQL or Cypher.

### ADR-005: Apache 2.0 License

**Context:** Need a permissive license that allows commercial use without requiring users to open-source their own code.

**Decision:** Apache 2.0. Includes patent grant (important for enterprise adopters). Allows commercial use, modification, and distribution.

**Rejected:** GPL (too restrictive for enterprise), MIT (no patent grant), BSL (not truly open source).

---

_Last updated: 2026-03-26 · Questions? Open an issue or ask in `#architecture` on Discord._
