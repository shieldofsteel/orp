# ORP — Final A+ Pass

## CRITICAL BUGS
1. `orp-query/src/executor.rs` — AND/OR conditions ALWAYS return true. Fix filter to evaluate both sides recursively
2. `executor.rs` — Type-less MATCH defaults to "ship". Scan all types when none specified
3. `orp-storage/src/duckdb_engine.rs` — graph_query() returns empty. Implement via recursive CTE on relationships
4. `duckdb_engine.rs` — begin/commit/rollback are no-ops. Implement real transactions
5. `orp-security/src/abac.rs` — Box::leak memory leak in principal_matches. Use owned comparison

## SECURITY
6. `http.rs` — Add rate limiting middleware (100 req/sec per IP), return 429 + Retry-After
7. `handlers.rs` — create_entity: return 409 CONFLICT if entity already exists
8. `handlers.rs` — Validate lat [-90,90] lon [-180,180] on create/update
9. `handlers.rs` — Return 400 on malformed `near` search param
10. `oidc.rs` — Store CSRF state in signed cookie, verify in callback
11. Add POST/GET/DELETE /api/v1/api-keys endpoints

## WEBSOCKET PUSH
12. Add tokio::broadcast to AppState. Handlers emit events on mutations. WS subscribes to broadcast, remove polling loop
13. ABAC check per entity before sending to WS client

## QUERY + FRONTEND
14. Push ORDER BY + LIMIT into DuckDB SQL, not Rust-side sort
15. `useWebSocket.ts` — Pass ?token= from stored JWT
16. `MapView.tsx` — Filter entities with no geometry (don't cluster at [0,0])
17. `handlers.rs` — Return separate created_at and updated_at

## RULES
- cargo test — all pass. cargo clippy — zero warnings. Commit + push
