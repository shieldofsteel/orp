# ORP — Integration Build

6 parallel teams wrote ~16,000 lines of new code. Your job: make it all compile, pass tests, and work together.

## Priority 1: Fix Compilation
Run `cargo check` and fix ALL errors. Teams wrote files independently — expect import mismatches, type conflicts, missing re-exports. Fix them.

## Priority 2: API & WebSocket Gaps (team that failed)
These files need rewriting — current versions are stale:
- `crates/orp-core/src/server/handlers.rs` — add missing endpoints: GET /events (global), PUT /connectors/{id}, DELETE /connectors/{id}, PUT /monitors/{id}. Fix pagination to include `links` object. Fix error responses to include `details` field.
- `crates/orp-core/src/server/websocket.rs` — rewrite for full protocol: multiple subscriptions per client, all message types (entity_created, entity_deleted, relationship_changed, alert_triggered), 30s heartbeat, geo region subscriptions.
- `crates/orp-core/src/server/http.rs` — wire auth middleware from orp-security, serve frontend/dist/ via ServeDir, add CORS config.

## Priority 3: Wire Everything Together
- `crates/orp-core/src/main.rs` + `cli/commands.rs` — use new error.rs, retry.rs. Wire auth, storage, stream with new traits.
- Ensure orp-proto OrpEvent is used consistently across all crates
- Ensure Cargo.toml deps are correct (rocksdb in orp-stream, jsonwebtoken in orp-security, etc.)

## Priority 4: Tests + Commit
- `cargo test` — all must pass
- `cargo clippy` — zero warnings
- Add LICENSE file (Apache 2.0)
- Git commit everything, push to origin

## Rules
- Use superpowers: verification-before-completion
- Do NOT rewrite files that are already good — only fix what's broken
- Run cargo check frequently to catch errors early
