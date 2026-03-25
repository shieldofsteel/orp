# ORP Architecture

## System Overview

ORP (Operational Readiness Platform) is a real-time data fusion engine designed for maritime domain awareness. It ingests heterogeneous data streams, resolves entities, maintains a property graph, and serves a web-based operational console.

```
┌──────────────────────────────────────────────────────────────┐
│                        ORP Server                            │
│                                                              │
│  ┌─────────┐   ┌──────────────┐   ┌──────────────────────┐  │
│  │Connectors│──▶│Stream Processor│──▶│  DuckDB Storage      │  │
│  │ AIS      │   │ • Dedup       │   │  • Entities          │  │
│  │ ADS-B    │   │ • Batching    │   │  • Events            │  │
│  │ HTTP     │   │ • Validation  │   │  • Relationships     │  │
│  │ MQTT     │   │ • Signing     │   │  • Geospatial Index  │  │
│  │ CSV      │   └──────┬───────┘   └──────────┬───────────┘  │
│  └─────────┘          │                       │              │
│                       ▼                       ▼              │
│               ┌───────────────┐    ┌──────────────────┐      │
│               │Monitor Engine │    │  Query Executor   │      │
│               │ • Thresholds  │    │  • ORP-QL Parser  │      │
│               │ • Geofences   │    │  • Plan + Execute │      │
│               │ • Anomalies   │    │  • Graph Queries  │      │
│               └───────┬───────┘    └──────────┬───────┘      │
│                       │                       │              │
│                       ▼                       ▼              │
│              ┌────────────────────────────────────────┐      │
│              │          Axum HTTP + WS Server         │      │
│              │  • REST API v1 (JSON)                  │      │
│              │  • WebSocket real-time push             │      │
│              │  • Static file serving (frontend/dist/) │      │
│              └────────────────────────────────────────┘      │
│                              │                               │
└──────────────────────────────┼───────────────────────────────┘
                               │
                               ▼
                  ┌─────────────────────────┐
                  │    React Frontend        │
                  │  • Deck.gl Map Layers   │
                  │  • Entity Inspector     │
                  │  • Query Bar (ORP-QL)   │
                  │  • Timeline Scrubber    │
                  │  • Alert Feed           │
                  └─────────────────────────┘
```

---

## Crate Dependency Graph

```
orp-core (binary)
├── orp-proto          (shared types)
├── orp-storage        (DuckDB engine)
│   └── orp-proto
├── orp-stream         (event processing)
│   ├── orp-proto
│   └── orp-storage
├── orp-query          (ORP-QL parser + executor)
│   ├── orp-proto
│   └── orp-storage
├── orp-connector      (data source adapters)
│   └── orp-proto
├── orp-entity         (entity resolution)
│   └── orp-proto
├── orp-config         (configuration)
├── orp-audit          (audit log)
│   └── orp-proto
├── orp-security       (auth + ABAC)
├── orp-geospatial     (geo utilities)
└── orp-testbed        (synthetic data)
    ├── orp-proto
    └── orp-storage
```

---

## Data Flow

### Ingestion Pipeline

1. **Connector** receives raw data (e.g., NMEA sentence from AIS TCP feed).
2. Connector parses into `OrpEvent` (canonical event format with position, properties, source trust).
3. **Stream Processor** receives the event:
   - **Deduplication**: Checks if entity+timestamp was seen within the dedup window (in-memory HashMap).
   - **Batching**: Buffers events and flushes when batch size is reached.
   - **Storage**: Inserts/updates entity in DuckDB, appends to event log.
4. **Monitor Engine** evaluates the new entity state against registered rules (thresholds, geofences, anomaly detection) and triggers alerts.
5. **WebSocket** broadcasts entity updates and alerts to subscribed clients.

### Query Pipeline

1. Client sends ORP-QL query via `POST /api/v1/query`.
2. **Parser** (nom-based) produces an AST: `Match → Pattern → Where → Return → OrderBy → Limit`.
3. **Planner** converts AST into a query plan (DuckDB SQL for property/geospatial queries).
4. **Executor** runs the plan against DuckDB and returns results.
5. Graph traversal queries (`-[:REL]->`) are executed against the relationship table with multi-hop support.

---

## Storage Layer

### DuckDB Schema

DuckDB is the primary analytical store. Tables:

- **entities** — Entity ID, type, name, properties (JSON), geometry (lat/lon), confidence, timestamps
- **events** — Append-only event log with entity references, event type, payload, source attribution
- **relationships** — Source/target entity pairs with typed edges and properties
- **data_sources** — Registered connectors with trust scores and ingestion stats

### Geospatial

Geospatial queries use Haversine distance calculations. The `NEAR()` and `WITHIN()` predicates in ORP-QL translate to bounding-box pre-filters followed by exact distance checks.

---

## Frontend Architecture

### Component Tree

```
App (QueryClientProvider, layout)
├── Header (logo, QueryBar, connection status)
├── Sidebar
│   ├── System Status (health, uptime, WebSocket)
│   ├── Data Sources (connector list)
│   └── AlertFeed (real-time alert notifications)
├── MainContent
│   ├── MapView (Deck.gl + MapLibre)
│   │   ├── ScatterplotLayer (ships — color by speed)
│   │   ├── ScatterplotLayer (ports — color by congestion)
│   │   ├── PolygonLayer (weather systems)
│   │   ├── PathLayer (ship tracks)
│   │   └── HeatmapLayer (vessel density)
│   └── EntityInspector (properties, relationships, history)
├── QueryResultsPanel (tabular query output)
└── TimelineScrubber (temporal navigation)
```

### State Management

**Zustand** provides a single global store:

- **UI state**: selected entity, sidebar/inspector open, map mode
- **Map state**: center, zoom, layer toggles
- **Query state**: last query, results, loading, error
- **WebSocket state**: connection status, subscriptions
- **Data**: entity map, relationship map, alerts

### Data Fetching

**TanStack Query** (React Query v5) manages server state:

- `useEntities(filters)` — paginated entity list
- `useEntity(id)` — single entity with relationships
- `useEntitySearch(params)` — geospatial + text search
- `useORPQuery()` — ORP-QL mutation
- `useHealth()` / `useConnectors()` / `useAlerts()` — system monitoring

### Real-time Updates

The `useWebSocket` hook maintains a persistent connection to `ws://host/ws/updates`:

- Subscribes by entity type
- Applies entity updates to the Zustand store
- Handles heartbeat/ack protocol
- Exponential backoff reconnection (1s → 60s max)

---

## Security Model

### Authentication

- OIDC-based auth flow (Keycloak or external provider)
- JWT tokens (HS256/RS256) with scoped permissions
- httpOnly cookies (no XSS vector)
- API key support for service integrations

### Authorization (ABAC)

Every API request is evaluated against attribute-based policies:

- **Principal attributes**: user ID, permissions, role, org
- **Resource attributes**: entity type, sensitivity, owner
- **Action**: read, write, delete
- Default-deny policy evaluation

### Audit

- Ed25519-signed events at ingestion
- Hash-chained audit log (each entry links to previous via SHA-256)
- Tamper detection via chain verification

---

## Performance Targets

| Metric | Target |
|--------|--------|
| API response (entity CRUD) | < 50ms p99 |
| ORP-QL query (simple) | < 500ms |
| Geospatial search (50km radius) | < 200ms |
| Stream throughput | 10,000+ events/sec |
| Map rendering (50K entities) | 60 FPS |
| Memory (1M entities) | < 3 GB |
| Binary size | < 350 MB |

---

## Benchmarks

Criterion benchmarks live in `crates/orp-testbed/benches/`:

- **query_latency** — Simple match, geospatial near, full scan at 100/500/1K entities
- **stream_throughput** — Event ingestion rate, dedup overhead, full pipeline throughput

Run with:

```bash
cargo bench -p orp-testbed
```
