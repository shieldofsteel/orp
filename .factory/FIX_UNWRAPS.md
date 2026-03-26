# FIX_UNWRAPS.md — RISKY/CRITICAL Production Unwraps

**Generated from:** `specs/UNWRAP_AUDIT.md`  
**Date:** 2026-03-27

This file lists every RISKY or CRITICAL unwrap in production code that must be converted to proper error handling. Zero CRITICAL items found. Nine RISKY items follow.

---

## 1. `entity_type.unwrap()` — DuckDB query paths

**File:** `crates/orp-storage/src/duckdb_engine.rs`  
**Lines:** 673, 784, 788, 1605  
**Risk:** If any caller passes `entity_type: Option<&str>` as `None` without pre-validation, these lines panic in production query handlers.

**Current code (example at line 673):**
```rust
stmt.query(params![entity_type.unwrap(), lat_min, lat_max, lon_min, lon_max])
```

**Fix — add an early-return guard before the SQL block:**
```rust
let et = entity_type.ok_or_else(|| StorageError::InvalidInput("entity_type is required".into()))?;
stmt.query(params![et, lat_min, lat_max, lon_min, lon_max])
```

Apply the same pattern at lines 784, 788, and 1605. The function signature for these methods already returns `Result<_, StorageError>`, so `?` propagation is clean.

---

## 2. `partial_cmp(...).unwrap()` in alert sorting — analytics hot path

**File:** `crates/orp-stream/src/analytics.rs`  
**Line:** 659  
**Risk:** `dark_duration_minutes` is a `f64` computed from timestamps/network data. If the field ever receives a NaN (e.g., division by zero in upstream calc, or malformed sensor data), `partial_cmp` returns `None` and `unwrap()` panics inside `sort_by`.

**Current code:**
```rust
alerts.sort_by(|a, b| b.dark_duration_minutes.partial_cmp(&a.dark_duration_minutes).unwrap());
```

**Fix — use `total_cmp` (Rust 1.62+, treats NaN consistently) or a NaN-safe fallback:**
```rust
alerts.sort_by(|a, b| b.dark_duration_minutes.total_cmp(&a.dark_duration_minutes));
```
Or if `total_cmp` is unavailable on the type:
```rust
alerts.sort_by(|a, b| {
    b.dark_duration_minutes
        .partial_cmp(&a.dark_duration_minutes)
        .unwrap_or(std::cmp::Ordering::Equal)
});
```

---

## 3. `partial_cmp(...).unwrap()` in threat sorting — threat engine

**File:** `crates/orp-stream/src/threat.rs`  
**Line:** 478  
**Risk:** Same as above. `risk_score` is derived from multi-factor computation on network input. NaN propagation causes panic in sort.

**Current code:**
```rust
result.sort_by(|a, b| b.risk_score.partial_cmp(&a.risk_score).unwrap());
```

**Fix:**
```rust
result.sort_by(|a, b| {
    b.risk_score
        .partial_cmp(&a.risk_score)
        .unwrap_or(std::cmp::Ordering::Equal)
});
```

---

## 4. `.expect()` in ABAC policy registration — startup code

**File:** `crates/orp-security/src/abac.rs`  
**Lines:** 233, 261, 281, 306  
**Risk:** These `.expect()` calls are in `AbacEngine::default_permissive()` and `default_policies()` — functions called at server startup. If policy validation logic changes and rejects a previously-valid hardcoded policy, the server will **panic at startup** rather than fail gracefully.

**Current code (example):**
```rust
engine.add_policy(AbacPolicy { ... }).expect("default policy is valid");
```

**Fix — return `Result` from these constructors and propagate:**
```rust
// Option A: return Result from the factory function
pub fn default_permissive() -> Result<Self, AbacError> {
    let engine = Self::new();
    engine.add_policy(AbacPolicy { ... })?;
    engine.add_policy(AbacPolicy { ... })?;
    Ok(engine)
}

// Option B: if callers can't handle Result, log + proceed gracefully
engine.add_policy(AbacPolicy { ... })
    .unwrap_or_else(|e| {
        tracing::error!("Failed to register default policy: {e}");
    });
```

Prefer Option A — callers should handle startup errors explicitly and shut down cleanly.

---

## 5. `.expect("serialise api config")` — connector config builder

**File:** `crates/orp-connector/src/adapters/generic_api.rs`  
**Line:** 813  
**Risk:** `serde_json::to_value(&api_config)` panics if `ApiConnectorConfig` contains a non-serialisable value. Low probability today, but adding any non-serialisable field in future would silently break all template-based connector creation.

**Current code:**
```rust
m.insert(
    "generic_api".to_string(),
    serde_json::to_value(&api_config).expect("serialise api config"),
);
```

**Fix:**
```rust
m.insert(
    "generic_api".to_string(),
    serde_json::to_value(&api_config)
        .map_err(|e| ConnectorError::ConfigError(format!("Failed to serialise api_config: {e}")))?,
);
```

The enclosing function already returns `Result<Self, ConnectorError>`, so `?` propagates cleanly.

---

## Fix Priority Order

| Priority | File | Line(s) | Why |
|----------|------|---------|-----|
| 🔴 HIGH | `duckdb_engine.rs` | 673, 784, 788, 1605 | Network-input query path; panics affect all live queries |
| 🔴 HIGH | `analytics.rs` | 659 | Sort panic in stream analytics hot path; drops the whole stream task |
| 🔴 HIGH | `threat.rs` | 478 | Sort panic in threat scoring; silently kills threat detection |
| 🟡 MEDIUM | `abac.rs` | 233, 261, 281, 306 | Startup panic; caught in dev but could block prod deployment |
| 🟡 MEDIUM | `generic_api.rs` | 813 | Config-build panic; affects connector creation only |

**Total: 9 items, 0 CRITICAL (will-always-panic), all RISKY (conditional on data/future changes).**
