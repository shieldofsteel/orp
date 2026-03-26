# ORP — Fix All Audit Findings (Parser Safety + Correctness)

Read .factory/FIX_UNWRAPS.md and specs/PARSER_AUDIT_V2.md for details.

## Fixes Required

### HIGH (panics on real data)
1. `duckdb_engine.rs` — Replace 4 `entity_type.unwrap()` with `.unwrap_or_default()` or proper Option handling
2. `analytics.rs:659` — Replace `partial_cmp().unwrap()` with `partial_cmp().unwrap_or(Ordering::Equal)` (NaN-safe sort)
3. `threat.rs:478` — Same NaN-safe sort fix
4. `ais.rs` — Either delete the file or redirect to nmea.rs AIS decoder. Currently a stub returning None.
5. `nmea.rs` — Filter AIS sentinel positions: reject lat==91.0 or lon==181.0 (means "not available" per ITU-R M.1371)

### MEDIUM
6. `abac.rs` — Replace 4 `.expect()` with `.unwrap_or_else(|e| { tracing::error!(...); })` 
7. `generic_api.rs:813` — Replace `.expect()` with `?` or `.ok()`
8. `pcap.rs` — Convert 10 array index unwrap() to checked access with .get()
9. `database.rs` — Replace `.lock().unwrap()` with `.lock().unwrap_or_else(|e| e.into_inner())`
10. `stix.rs` — Add `spec_version: String` as required field, make `created`/`modified` required not optional

### CLEANUP
11. Remove `ais.rs` if it's just a stub (real AIS decoder is in nmea.rs)

cargo test + cargo clippy after all fixes. Commit + push.
