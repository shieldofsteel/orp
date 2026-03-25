# ORP — Complete ALL Phases to Production Quality

## Context
Phase 1 foundation is built: 12 crates, 4,570 lines, 25 tests, binary compiles. Now finish EVERYTHING from the specs. Read `specs/` files for full requirements.

## What's Done
- Cargo workspace with 12 crates, all compiling
- DuckDB storage with entity CRUD + geospatial
- Stream processor with dedup + batching
- AIS connector (CSV parsing)
- ORP-QL parser (nom) with MATCH/WHERE/RETURN/NEAR
- Audit log (hash-chained, Ed25519)
- ABAC + OIDC stubs
- Axum HTTP server (13 endpoints) + WebSocket
- Embedded HTML dashboard
- 25 passing tests

## What Must Be Built (ALL of this)

### Core Engine Hardening
- Full DuckDB schema from spec (all tables, indexes, partitioning)
- Kuzu graph engine integration (schema DDL, sync from DuckDB every 30s)
- RocksDB-backed dedup window (persistent across restarts)
- Entity resolution (structural MMSI/ICAO matching + canonical IDs)
- Config system with env var substitution (`${env.SECRET}`)

### All Connectors
- ADS-B TCP receiver
- HTTP REST polling connector
- MQTT subscriber
- CSV file watcher
- WebSocket client connector
- Connector trait with full lifecycle

### Query Engine Complete
- Full ORP-QL v0.1 grammar (EBNF from spec)
- Query planner with cost optimization
- Hybrid executor (DuckDB for OLAP, Kuzu for graph)
- All aggregation functions (COUNT/SUM/AVG/MIN/MAX)
- ORDER BY, LIMIT, graph traversal patterns

### Full REST API (OpenAPI 3.1)
- All entity CRUD endpoints with proper pagination
- Entity search (geospatial + property + text)
- Events endpoint (query by entity/type/time)
- Relationships endpoint
- Graph query endpoint (Cypher passthrough)
- Connector CRUD endpoints
- Monitor/alert rule CRUD
- Health + Prometheus metrics
- Standard error responses per spec
- Rate limiting, CORS

### WebSocket Protocol
- Subscribe by entity type/region/entity ID
- Entity update/create/delete broadcasts
- Relationship change events
- Alert triggered events
- Heartbeat + reconnection support

### Full React Frontend (NOT embedded HTML)
- Vite + React 18 + TypeScript
- Zustand state management
- TanStack Query for data fetching
- Deck.gl map (IconLayer ships, ScatterplotLayer ports, PolygonLayer weather, PathLayer tracks, HeatmapLayer)
- Entity inspector panel with properties + relationships
- Query bar with autocomplete
- Timeline scrubber
- Alert feed with real-time notifications
- Dark mode, WCAG 2.1 AA accessible

### Security
- Full OIDC auth flow (login → token → cookie)
- ABAC policy evaluation on every endpoint
- Ed25519 signing on all ingested data
- Immutable hash-chained audit log with verification
- API key scoping + rate limiting

### Testing
- 100+ unit tests across all crates
- Integration tests (end-to-end pipeline)
- Benchmark suite (criterion) for query latency + throughput
- Synthetic data generators (ships, ports, weather)

### CLI
- `orp start` with all flags
- `orp query "ORP-QL"` 
- `orp status` (health check)
- `orp connectors list/add/remove`

### Documentation
- README.md with getting started
- Architecture docs
- API reference

## Rules
- Use superpowers: verification-before-completion, executing-plans
- Spawn sub-agents for parallel work
- Run `cargo test` after each milestone, all must pass
- Run `cargo clippy` — zero warnings
- Commit with conventional commits after each major piece
- Build the React frontend as a proper Vite project in `frontend/`
- Embed built frontend assets in the binary
- Work non-stop until complete
