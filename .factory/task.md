# ORP — Fix ALL Audit Findings

Security + quality hardening. Fix every issue below.

## CRITICAL (fix all 5)
1. `http.rs` — Wire AuthState into AppState, add AuthContext extractor to EVERY handler
2. `middleware.rs` — anonymous() must have ZERO permissions. permissive_mode only when ORP_DEV_MODE=true
3. `jwt.rs` — Remove hardcoded secret "change-me-in-production". Require JWT_SECRET env var
4. `jwt.rs` — Delete validate_any_audience or remove insecure_disable_signature_validation()
5. `duckdb_engine.rs` — Replace ALL format!() SQL with parameterized queries (params![])

## HIGH (fix all 6)
6. `http.rs` — CORS: configurable origins, not allow_origin(Any)
7. `websocket.rs` — Require auth token in query param, validate before upgrade
8. Wire AbacEngine into AppState, call evaluate() before data access in handlers
9. Wire AuditLog into AppState, log every state-mutating operation
10. Rewrite websocket.rs — tokio::broadcast for real push, not polling. Add entity_created/deleted/relationship_changed/alert_triggered. Multiple subscriptions per client
11. `http.rs` — Protect /metrics endpoint with auth

## Frontend Fix
12. `frontend/src/hooks/useWebSocket.ts` — merge properties { ...existing, ...update } not replace

## Missing Endpoints
13. Add: GET /events (global), PUT /connectors/{id}, DELETE /connectors/{id}, PUT /monitors/{id}

## Rules
- cargo test — all must pass. cargo clippy — zero warnings
- Commit and push to origin when done
