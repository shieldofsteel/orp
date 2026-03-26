# ORP — 500+ Tests + World-Class CLI

## 1. TEST EXPANSION (target 500+)
Add tests to EVERY crate:
- orp-core handlers: axum::test integration tests for EVERY endpoint (200/401/403/404/409/422/429), pagination, sorting. 50+ tests.
- orp-core websocket: subscribe, unsubscribe, auth, broadcast, heartbeat. 15+ tests.
- orp-query: every ORP-QL feature — AND, OR, NEAR, WITHIN, DISTANCE, aggregations, graph, errors. 30+ tests.
- orp-connector: each adapter edge cases — malformed input, retry, reconnect. 25+ tests.
- orp-storage/graph_engine: path queries, neighbors, sync, cypher. 15+ tests.
- orp-entity: matching, merge, canonical IDs. 10+ tests.
- orp-config: YAML parsing, env var substitution, validation errors. 10+ tests.
Target: `cargo test` shows 400+ Rust tests.

## 2. WORLD-CLASS CLI (clap v4 derive)
Rewrite `cli/args.rs` + `commands.rs`:
- `orp start` (--port --config --template --dev)
- `orp query "ORP-QL"` — output as table/json/csv
- `orp query --file queries.oql`
- `orp status` — health, entities, connectors, uptime
- `orp connectors list/add/remove`
- `orp entities search --near 51.9,4.2 --radius 50`
- `orp entities get <id>`
- `orp events --entity <id> --since 1h`
- `orp monitors list/add/remove`
- `orp config validate <file>`
- `orp version` — version + build info
- `orp completions <shell>` — bash/zsh/fish
Colored table output (add `tabled` + `colored` deps). --json/--csv modes. Respect NO_COLOR.

## RULES
- cargo test 400+. cargo clippy zero. Commit+push.
