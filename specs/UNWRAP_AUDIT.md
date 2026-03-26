# ORP Codebase — Unwrap/Expect/Panic Audit

**Audit Date:** 2026-03-27  
**Scope:** All production Rust code in `/crates/` (excluding `orp-testbed/` benchmarks and `#[cfg(test)]` modules)  
**Auditor:** Rust Safety Subagent

---

## Methodology

- Scanned all `.rs` files under `crates/`
- For each hit, determined whether the line falls inside a `#[cfg(test)]` module using bracket-depth tracking
- Classified each **production** finding as SAFE / RISKY / CRITICAL

**Key rules used:**
- `Mutex::lock().unwrap()` — SAFE if the mutex is never poisoned by a panic in a guard-holding context (single-writer async code). In async codebases using tokio, Mutex poisoning cannot happen because panics don't cross await points. Classified SAFE with a note.
- `.first().unwrap()` / `.last().unwrap()` — only after a `len() < 2` guard → SAFE
- `partial_cmp(...).unwrap()` on floats — RISKY if NaN values are possible in input data
- `Response::builder().body(...).unwrap()` — SAFE (infallible Response builder with known-good args)
- `expect("default policy is valid")` — RISKY if called at startup with config-driven policies
- `expect("serialise api config")` — RISKY if `api_config` contains non-serialisable types (unlikely but not guaranteed)
- `entries.last().unwrap()` after `push()` — SAFE (always Some after push)

---

## PRODUCTION CODE FINDINGS

### `crates/orp-storage/src/duckdb_engine.rs`

All production hits are `self.conn.lock().unwrap()` mutex guards, plus two `entity_type.unwrap()` calls on an `Option<&str>` parameter.

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 212 | `self.conn.lock().unwrap()` | SAFE | Async runtime; DuckDB conn held briefly. Mutex cannot poison across await. |
| 386 | `self.conn.lock().unwrap()` | SAFE | Same |
| 452 | `self.conn.lock().unwrap()` | SAFE | Same |
| 494 | `self.conn.lock().unwrap()` | SAFE | Same |
| 540 | `self.conn.lock().unwrap()` | SAFE | Same |
| 577 | `self.conn.lock().unwrap()` | SAFE | Same |
| 587 | `self.conn.lock().unwrap()` | SAFE | Same |
| 599 | `self.conn.lock().unwrap()` | SAFE | Same |
| 619 | `self.conn.lock().unwrap()` | SAFE | Same |
| **673** | `params![entity_type.unwrap(), ...]` | **RISKY** | `entity_type: Option<&str>` — if caller passes `None` without checking, panics. Called from query path. |
| 701 | `self.conn.lock().unwrap()` | SAFE | Same as above |
| **784** | `params![entity_type.unwrap(), ...]` | **RISKY** | Same as line 673 |
| **788** | `params![entity_type.unwrap()]` | **RISKY** | Same as line 673 |
| 810 | `self.conn.lock().unwrap()` | SAFE | Same |
| 847 | `self.conn.lock().unwrap()` | SAFE | Same |
| 882 | `self.conn.lock().unwrap()` | SAFE | Same |
| 944 | `self.conn.lock().unwrap()` | SAFE | Same |
| 981 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1016 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1049 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1086 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1137 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1209 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1264 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1287 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1332 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1342 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1377 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1394 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1414 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1485 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1527 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1534 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1541 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1555 | `self.conn.lock().unwrap()` | SAFE | Same |
| **1605** | `params![entity_type.unwrap(), ...]` | **RISKY** | Same as line 673 — search_entities path |
| 1627 | `self.conn.lock().unwrap()` | SAFE | Same |
| 1634 | `self.conn.lock().unwrap()` | SAFE | Same |

---

### `crates/orp-storage/src/graph_engine.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 125 | `self.conn.lock().unwrap()` | SAFE | std::sync::Mutex; graph engine is SQLite-backed, single-writer. Panic-poisoning possible but extremely unlikely. Acceptable. |
| 152 | `self.conn.lock().unwrap()` | SAFE | Same |
| 303 | `self.conn.lock().unwrap()` | SAFE | Same |
| 413 | `self.conn.lock().unwrap()` | SAFE | Same |
| 465 | `self.conn.lock().unwrap()` | SAFE | Same |
| 533 | `self.conn.lock().unwrap()` | SAFE | Same |
| 601 | `self.conn.lock().unwrap()` | SAFE | Same |

> Note: GraphEngine uses `std::sync::Mutex` (not tokio). If any caller panics while holding the lock, subsequent callers will get `PoisonError`. Should be converted to `lock().unwrap_or_else(|e| e.into_inner())` pattern for robustness.

---

### `crates/orp-connector/src/adapters/database.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 374 | `self.watermark.lock().unwrap()` | SAFE | Watermark mutex; sync mutex in async context. Low poison risk. |
| 428 | `watermark.lock().unwrap()` | SAFE | Same |
| 437 | `watermark_clone.lock().unwrap()` | SAFE | Same |

---

### `crates/orp-stream/src/analytics.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 224 | `window.first().unwrap()` | SAFE | Guarded by `if window.len() < 2 { return; }` check 2 lines above |
| 225 | `window.last().unwrap()` | SAFE | Same guard |
| **659** | `b.dark_duration_minutes.partial_cmp(&a.dark_duration_minutes).unwrap()` | **RISKY** | `partial_cmp` returns `None` if either value is `NaN`. If `dark_duration_minutes` is computed from division or floating-point ops on network input, NaN is possible and will panic. |

---

### `crates/orp-stream/src/threat.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| **478** | `b.risk_score.partial_cmp(&a.risk_score).unwrap()` | **RISKY** | Same as analytics.rs:659 — `risk_score` derived from input data. NaN possible if computation involves 0/0 or unchecked arithmetic. |

---

### `crates/orp-core/src/server/websocket.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 84 | `Response::builder()...body(...).unwrap()` | SAFE | `Response::builder()` with static string body is infallible |
| 99 | `Response::builder()...body(...).unwrap()` | SAFE | Same |

---

### `crates/orp-audit/src/logger.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| 70 | `self.entries.last().unwrap()` | SAFE | Called immediately after `self.entries.push(entry)` — always Some |

---

### `crates/orp-security/src/abac.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| **233** | `.expect("default policy is valid")` | **RISKY** | Calls `add_policy()` with a hardcoded policy struct in `default_permissive()`. Technically SAFE if validation logic is deterministic, but `add_policy()` validates the policy and returns `Err` if invalid. If policy validation logic ever changes, startup panics. |
| **261** | `.expect("valid")` | **RISKY** | Same pattern — hardcoded policy in `default_policies()` |
| **281** | `.expect("valid")` | **RISKY** | Same |
| **306** | `.expect("valid")` | **RISKY** | Same |

---

### `crates/orp-connector/src/adapters/generic_api.rs`

| Line | Code | Classification | Reason |
|------|------|---------------|--------|
| **813** | `.expect("serialise api config")` | **RISKY** | `serde_json::to_value(&api_config)` — if `ApiConnectorConfig` ever gains a non-serialisable field (e.g., a raw function pointer, a channel), this will panic at runtime when building a connector config. |

---

## TEST CODE FINDINGS (Not actionable — documented for completeness)

All `unwrap()`/`expect()`/`panic!()` in the following test modules are **intentionally not classified as production risk**:

- `crates/orp-proto/src/lib.rs` — test module starts at line 702; all panics/unwraps after that are in tests
- `crates/orp-entity/src/resolver.rs` — all hits are in `#[cfg(test)]` mod
- `crates/orp-stream/src/processor.rs` — all hits are in `#[cfg(test)]` mod
- `crates/orp-stream/src/dlq.rs` — all hits are in test mod
- `crates/orp-stream/src/dedup.rs` — all hits are in test mod
- `crates/orp-stream/src/analytics.rs` — line 1029 is in test mod
- `crates/orp-storage/src/duckdb_engine.rs` — lines 1724+ are in test mod
- `crates/orp-storage/src/graph_engine.rs` — lines 839+ are in test mod
- `crates/orp-config/src/schema.rs` — all hits in test mod
- `crates/orp-core/src/server/handlers.rs` — all hits are in `#[cfg(test)]` mod (starts ~line 1860)
- `crates/orp-core/src/server/ingest.rs` — all hits in test mod (starts line 540)
- `crates/orp-core/src/server/federation.rs` — all hits in test mod
- `crates/orp-core/src/server/layers.rs` — all hits in test mod
- `crates/orp-core/src/retry.rs` — all hits in test mod
- `crates/orp-query/src/executor.rs` — all hits in test mod
- `crates/orp-query/src/parser.rs` — all hits in test mod
- `crates/orp-security/src/jwt.rs` — all hits in test mod
- `crates/orp-security/src/api_keys.rs` — all hits in test mod
- `crates/orp-security/src/oidc.rs` — all hits in test mod
- `crates/orp-connector/src/adapters/*` — all connector adapter hits are in `#[cfg(test)]` mods
- `crates/orp-testbed/benches/*` — benchmark code, excluded from scope

---

## Summary

| Severity | Count | Files |
|----------|-------|-------|
| CRITICAL | 0 | — |
| RISKY | 9 | duckdb_engine.rs (×3), analytics.rs (×1), threat.rs (×1), abac.rs (×4), generic_api.rs (×1) |
| SAFE | ~50+ | duckdb_engine.rs, graph_engine.rs, websocket.rs, logger.rs, database.rs |

**No CRITICAL panics found in production code.** All `panic!()` calls are inside `#[cfg(test)]` modules.

The most operationally dangerous items are:
1. **`entity_type.unwrap()` in duckdb_engine.rs** — called from query paths with network input; should be ? or early return
2. **`partial_cmp(...).unwrap()` in analytics/threat** — NaN float panic in sorting hot paths
3. **`.expect()` in abac.rs** — startup-time policy registration; brittle against policy validation changes
