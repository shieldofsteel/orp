# ORP — Operational Readiness Platform

**Real-time data fusion engine for maritime domain awareness.**

ORP ingests heterogeneous data streams (AIS, ADS-B, HTTP feeds, MQTT, CSV), fuses them into a canonical entity graph, and exposes the result through a REST API, WebSocket push, and an interactive React/Deck.gl map console.

---

## Quick Start

### Prerequisites

- **Rust** 1.75+ (install via [rustup](https://rustup.rs))
- **Node.js** 18+ with npm (for frontend build)

### Build & Run

```bash
# Build the Rust binary
cargo build --release

# Build the frontend
cd frontend && npm run build && cd ..

# Start ORP
./target/release/orp start --port 9090
```

Open [http://localhost:9090](http://localhost:9090) in your browser.

### CLI Commands

```bash
# Start the server
orp start                          # defaults: port 9090, in-memory DB
orp start --port 8080              # custom port
orp start --config config.yaml     # load config file
orp start --template maritime      # load synthetic maritime data

# Execute ORP-QL queries
orp query "MATCH (s:Ship) WHERE s.speed > 15 RETURN s.id, s.name, s.speed"

# Check system health
orp status

# List configured connectors
orp connectors list
```

---

## Architecture Overview

ORP is a Cargo workspace with 12 crates:

| Crate | Purpose |
|-------|---------|
| `orp-core` | Binary entry-point, Axum HTTP/WS server, CLI |
| `orp-proto` | Canonical data types (Entity, Event, Relationship) |
| `orp-storage` | DuckDB storage engine with geospatial indexes |
| `orp-stream` | Stream processor — dedup, batching, monitor engine |
| `orp-query` | ORP-QL parser (nom) and query executor |
| `orp-connector` | Data source connectors (AIS, ADS-B, HTTP, MQTT, CSV) |
| `orp-entity` | Entity resolution and canonical ID mapping |
| `orp-config` | Configuration loading with env-var substitution |
| `orp-audit` | Hash-chained, Ed25519-signed audit log |
| `orp-security` | ABAC policy engine, OIDC auth stubs |
| `orp-geospatial` | Haversine, bounding-box, geofence utilities |
| `orp-testbed` | Synthetic data generators, benchmark harness |

The React frontend lives in `frontend/` and is served as static files from `frontend/dist/` by the Axum server.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full system design.

---

## API

**Base URL:** `http://localhost:9090/api/v1`

### Core Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/health` | System health check |
| GET | `/api/v1/metrics` | Prometheus metrics |
| GET | `/api/v1/entities` | List entities (paginated) |
| POST | `/api/v1/entities` | Create entity |
| GET | `/api/v1/entities/:id` | Get single entity |
| PUT | `/api/v1/entities/:id` | Update entity |
| DELETE | `/api/v1/entities/:id` | Delete entity |
| GET | `/api/v1/entities/search` | Geospatial + text search |
| GET | `/api/v1/entities/:id/relationships` | Entity relationships |
| GET | `/api/v1/entities/:id/events` | Entity event history |
| POST | `/api/v1/relationships` | Create relationship |
| POST | `/api/v1/query` | Execute ORP-QL query |
| POST | `/api/v1/graph` | Execute graph query |
| GET | `/api/v1/connectors` | List data connectors |
| POST | `/api/v1/connectors` | Create connector |
| GET | `/api/v1/monitors` | List alert monitors |
| POST | `/api/v1/monitors` | Create alert monitor |
| GET | `/api/v1/alerts` | List triggered alerts |

### WebSocket

Connect to `ws://localhost:9090/ws/updates` for real-time entity updates, alerts, and heartbeat messages.

### ORP-QL

A Cypher-inspired query language for entity filtering and graph traversal:

```sql
MATCH (s:Ship) WHERE s.speed > 15 RETURN s.id, s.name, s.speed LIMIT 100
MATCH (s:Ship) WHERE NEAR(s, lat=51.92, lon=4.27, radius_km=50) RETURN s.id, s.name
MATCH (s:Ship)-[:HEADING_TO]->(p:Port) RETURN s.name, p.name
```

---

## Frontend

Built with Vite + React 18 + TypeScript + Tailwind CSS.

| Technology | Purpose |
|-----------|---------|
| Deck.gl 9 | GPU-accelerated map layers (ships, ports, weather, tracks, heatmap) |
| MapLibre GL | Base map tiles (dark-matter style) |
| Zustand | Lightweight global state management |
| TanStack Query | Server-state fetching, caching, real-time sync |

### Build Frontend

```bash
cd frontend
npm install
npm run build    # outputs to frontend/dist/
```

---

## Testing

```bash
# Run all unit tests
cargo test

# Run tests for a specific crate
cargo test -p orp-query

# Run with output
cargo test -- --nocapture

# Lint
cargo clippy -- -D warnings
```

### Benchmarks (Criterion)

```bash
# Query latency benchmarks
cargo bench -p orp-testbed --bench query_latency

# Stream throughput benchmarks
cargo bench -p orp-testbed --bench stream_throughput
```

---

## Project Structure

```
orp/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── orp-core/           # Server binary + CLI
│   ├── orp-proto/          # Shared types
│   ├── orp-storage/        # DuckDB engine
│   ├── orp-stream/         # Event processing
│   ├── orp-query/          # ORP-QL parser/executor
│   ├── orp-connector/      # Data connectors
│   ├── orp-entity/         # Entity resolution
│   ├── orp-config/         # Configuration
│   ├── orp-audit/          # Audit log
│   ├── orp-security/       # Auth + ABAC
│   ├── orp-geospatial/     # Geo utilities
│   └── orp-testbed/        # Test data + benchmarks
├── frontend/               # React/Vite frontend
│   ├── src/
│   │   ├── components/     # MapView, EntityInspector, QueryBar, etc.
│   │   ├── hooks/          # useEntities, useWebSocket
│   │   ├── store/          # Zustand store
│   │   └── types/          # TypeScript interfaces
│   └── dist/               # Built assets (served by Axum)
└── specs/                  # Design specifications
```

---

## License

Apache 2.0
