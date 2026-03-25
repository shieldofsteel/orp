# ORP — Build from Spec to Working Code

## Mission
Build ORP (Open Reality Protocol) — a single Rust binary for Palantir-grade data fusion. Read all 4 spec files in `specs/` then build the entire project.

## Specs (read ALL before coding)
- `specs/Powerful.md` — Architecture, event schema, DuckDB/Kuzu schemas
- `specs/BUILD_CORE_ENGINE.md` — Cargo workspace, Rust traits, schemas, config
- `specs/BUILD_API_FRONTEND.md` — REST API, WebSocket, React frontend, ORP-QL
- `specs/BUILD_TEAMS_SPRINTS.md` — Sprint plan, PR standards

## Build Order
1. Cargo workspace — full crate structure per BUILD_CORE_ENGINE §1
2. Core types — OrpEvent, Entity, Relationship + all traits
3. DuckDB integration — schema init, CRUD, geospatial (orp-storage)
4. Kuzu graph — schema, sync from DuckDB (orp-storage/kuzu_sync)
5. Stream processor — RocksDB dedup, windowing, batching (orp-stream)
6. AIS connector — NMEA TCP parser (orp-connector)
7. Query engine — ORP-QL parser (nom), planner, executor (orp-query)
8. HTTP API — Axum REST endpoints per OpenAPI spec
9. WebSocket — real-time entity subscriptions
10. React frontend — Vite + React + Deck.gl map + inspector + query bar
11. Security — Ed25519, audit log, OIDC stubs, ABAC
12. Config — YAML parsing + maritime template
13. CLI — `orp start`, `orp query`, `orp status`
14. Tests — unit + integration + benchmarks

## Rules
- Use superpowers: verification-before-completion, writing-plans, executing-plans
- Spawn sub-agents for parallel work. One builds, another audits
- Every crate must compile. Run `cargo check` frequently
- Follow exact schemas/traits from specs — no improvisation on contracts
- Init git repo, commit after each major milestone (conventional commits)
- Work non-stop until `orp start` launches the full binary
