# ORP — Complete ALL Phases to Production

Read `REQUIREMENTS.md` for the full checklist of what's built and what's missing. Read `specs/` for the complete technical specifications (Powerful.md, BUILD_CORE_ENGINE.md, BUILD_API_FRONTEND.md, BUILD_TEAMS_SPRINTS.md).

## Mission
The foundation (12 crates, 4,570 lines) is done. Now finish EVERYTHING to production quality. Every feature from every phase in the specs — even months 5-10 features — must be built now.

## Priority Order
1. Kuzu graph engine + DuckDB→Kuzu sync (30s loop)
2. All 6 connectors (ADS-B, HTTP, MQTT, CSV, WebSocket + improve AIS)
3. Full ORP-QL parser + query planner + hybrid executor
4. Complete REST API (all endpoints from OpenAPI spec)
5. WebSocket protocol (subscriptions, broadcasts, heartbeat)
6. Proper React frontend (Vite + Deck.gl + Zustand + TanStack Query)
7. Entity resolution (structural + canonical IDs)
8. Security hardening (OIDC flow, ABAC enforcement, signing)
9. Monitor/alerting system (rules engine, anomaly detection)
10. 100+ tests, benchmarks, clippy clean
11. CLI commands, config system, documentation

## Rules
- Use superpowers: verification-before-completion, executing-plans
- Spawn sub-agents for parallel work — one builds, one audits
- `cargo test` + `cargo clippy` after each milestone
- Conventional commits. Work non-stop until fully complete.
