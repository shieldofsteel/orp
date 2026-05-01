//! Persistent API-key store with Argon2id hashing.
//!
//! Replaces the previous `key_hash` scheme — SHA-256 of the raw key without
//! a salt, stored in an in-memory `HashMap` — with a backend that:
//!
//! 1. Hashes plaintext keys with **Argon2id** at OWASP 2023 parameters
//!    (`m=19_456 KiB`, `t=2`, `p=1`) and stores the result as a PHC string,
//!    so per-key salt is implicit and the algorithm/params are recoverable
//!    from the hash itself.
//! 2. Persists records to **DuckDB** in a dedicated `api_keys` table so
//!    keys survive process restarts. The DB connection is owned by the
//!    keystore — keystore deployments either reuse the main ORP DuckDB
//!    file (production) or open an isolated tempfile DB (tests).
//!
//! Two implementations live here:
//!   - [`InMemoryKeyStore`] — `Arc<RwLock<HashMap<…>>>`, used in unit tests
//!     and dev-mode (`AuthState::dev`). Identical durability semantics to
//!     the pre-Argon2id behaviour but with the proper hash function.
//!   - [`DuckDbKeyStore`] — production. Opens (or creates) an `api_keys`
//!     table on a supplied DuckDB connection.
//!
//! Both implement [`KeyStore`], so [`crate::api_keys::ApiKeyService`] can
//! be wired against either at construction time.
//!
//! ## Why a trait?
//!
//! ORP runs in three deployment shapes (in-memory tests, single-binary
//! `--in-memory`, persistent production). Tests must not drag a DuckDB
//! file into every test process; production must not lose keys on
//! restart. Splitting the storage behind a trait lets the same
//! `ApiKeyService` cover all three without conditional compilation.

use crate::api_keys::ApiKeyRecord;
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};
use chrono::{DateTime, Utc};
use duckdb::{params, Connection};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};
use thiserror::Error;

// ─── Argon2id parameters (OWASP 2023 guidance) ───────────────────────────────
//
// `m_cost` is in **KiB**; 19_456 KiB = 19 MiB, the OWASP-recommended floor
// for general-purpose password hashing on commodity server hardware as of
// 2023. `t_cost=2` and `p_cost=1` round out the published baseline.
//
// We expose the params via `argon2_params()` so tests and future tunings
// can read them without re-declaring magic numbers, and so the production
// code path always agrees with the test path on what was hashed.
const ARGON2_M_COST_KIB: u32 = 19_456;
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

/// Build an `argon2::Params` from the constants above.
///
/// Wrapped in a function rather than a `const` because `Params::new` is
/// not `const fn` in argon2 0.5. The returned value is small and cheap to
/// reconstruct per-call; we don't bother caching it.
fn argon2_params() -> Params {
    // `Params::new(m_cost, t_cost, p_cost, output_len)` — `output_len: None`
    // means "use the default 32-byte tag length", which the verifier
    // also expects.
    Params::new(ARGON2_M_COST_KIB, ARGON2_T_COST, ARGON2_P_COST, None).expect(
        "Argon2id params (m=19456, t=2, p=1) are within library bounds; this is a build-time \
         invariant, not user input",
    )
}

/// Build the Argon2id hasher with the configured params.
fn argon2_hasher() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params())
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Errors from the keystore backend itself (separate from
/// [`crate::api_keys::ApiKeyError`] which is the user-facing API-surface
/// error). Callers in `api_keys.rs` translate these to `ApiKeyError::Storage`.
#[derive(Debug, Error)]
pub enum KeyStoreError {
    /// The underlying database (DuckDB or the in-memory lock) returned
    /// an error.
    #[error("keystore backend error: {0}")]
    Backend(String),
    /// The key plaintext could not be hashed (encoding error, salt RNG
    /// failure, etc). This is effectively unreachable in practice but
    /// is surfaced rather than panicked because Argon2 is foundational
    /// — silent panics would break startup.
    #[error("argon2 hashing failed: {0}")]
    Hash(String),
}

impl From<duckdb::Error> for KeyStoreError {
    fn from(e: duckdb::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Hash a plaintext API key with Argon2id and return the PHC string.
///
/// PHC strings encode the algorithm, version, parameters, salt, and tag
/// in a single ASCII line, which lets verifiers reconstruct the exact
/// hasher used at insert time even after we tune the cost parameters
/// in a future release. Callers store the returned `String` directly.
pub fn hash_password(plaintext: &str) -> Result<String, KeyStoreError> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let hasher = argon2_hasher();
    let phc = hasher
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| KeyStoreError::Hash(e.to_string()))?
        .to_string();
    Ok(phc)
}

/// Verify a plaintext key against a stored PHC string.
///
/// Returns `Ok(true)` for a match, `Ok(false)` for a non-matching plaintext
/// (constant-time inside argon2), `Err` only when the stored hash is
/// malformed (corruption / out-of-band tampering).
pub fn verify_password(plaintext: &str, phc: &str) -> Result<bool, KeyStoreError> {
    let parsed = PasswordHash::new(phc).map_err(|e| KeyStoreError::Hash(e.to_string()))?;
    // `verify_password` walks the algorithm registry inside argon2; we
    // pass a freshly-built Argon2 instance so the params from `parsed`
    // are honoured (PHC carries them, not the verifier).
    match argon2_hasher().verify_password(plaintext.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(KeyStoreError::Hash(e.to_string())),
    }
}

// ─── Trait ────────────────────────────────────────────────────────────────────

/// Storage backend behind [`crate::api_keys::ApiKeyService`].
///
/// Implementations MUST be `Send + Sync` so the service can be wrapped in
/// `Arc<…>` and shared across Tokio worker threads.
///
/// All hashing of plaintext keys happens **inside** the implementation —
/// callers pass plaintext to [`Self::insert`] and to [`Self::verify`] only.
/// This keeps the Argon2id contract (random salt per key, PHC encoding)
/// in one place and makes it impossible for a caller to "accidentally"
/// store a SHA-256 hash by following the wrong API.
pub trait KeyStore: Send + Sync {
    /// Look up a record by its stable `id` (UUID). Used by `revoke_key`,
    /// `list_keys`, and rate-limit accounting.
    fn lookup_by_id(&self, id: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError>;

    /// Verify a plaintext API key against the record at `id`. Returns
    /// `Some(record)` only when the plaintext Argon2id-verifies AND the
    /// record is not revoked / expired. Returns `None` on any rejection
    /// (unknown id, wrong plaintext, revoked, expired) — the caller can
    /// then map to the appropriate user-visible error.
    ///
    /// Why `id`+`plaintext` instead of just `plaintext`? Plaintext keys
    /// are random 24-byte tokens; we never index on a hash of them
    /// (Argon2id hashes are slow and salted, so a hash-based lookup
    /// would require an O(N) scan + per-row Argon2id verify). Instead
    /// the **prefix of the plaintext encodes the id**: callers split the
    /// raw key on `_` to extract the id, then `verify` against that id.
    fn verify(&self, id: &str, plaintext: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError>;

    /// Insert a freshly-created record with the given plaintext key.
    /// The implementation hashes the plaintext (Argon2id) before
    /// persisting; the plaintext is dropped after this call.
    fn insert(&self, record: &ApiKeyRecord, plaintext: &str) -> Result<(), KeyStoreError>;

    /// Mark a record as revoked. Subsequent `verify` calls return
    /// `Ok(None)`. Returns `Ok(false)` if the id is unknown.
    fn revoke(&self, id: &str) -> Result<bool, KeyStoreError>;

    /// Update `last_used_at` to `now` for the given record. Best-effort:
    /// errors are surfaced but the caller usually treats them as
    /// non-fatal (the principal is already authenticated by the time we
    /// reach this).
    fn mark_used(&self, id: &str, now: DateTime<Utc>) -> Result<(), KeyStoreError>;

    /// Return all records (for `GET /api/v1/api-keys`).
    fn list_all(&self) -> Result<Vec<ApiKeyRecord>, KeyStoreError>;

    /// Number of keys currently stored — used by the bootstrap path to
    /// decide whether to seed an admin key on first start.
    fn count(&self) -> Result<usize, KeyStoreError> {
        Ok(self.list_all()?.len())
    }
}

// ─── In-memory implementation (tests + dev) ──────────────────────────────────

/// Internal slot storing the PHC hash alongside the public record.
///
/// We keep `phc` separate from `ApiKeyRecord` rather than mutating the
/// existing `key_hash` field because the record is serialised over the
/// wire (`list_keys`) and we don't want to leak the Argon2id PHC string
/// to API clients. The record's `key_hash` field stays as a placeholder
/// (empty string) for backwards compat with downstream JSON schemas.
#[derive(Clone, Debug)]
struct InMemorySlot {
    record: ApiKeyRecord,
    phc: String,
}

/// In-memory `KeyStore` — backed by `RwLock<HashMap<id, slot>>`.
///
/// **Persistence:** none. Process restart drops every key. Use only in
/// unit tests and dev-mode auth.
#[derive(Clone, Debug, Default)]
pub struct InMemoryKeyStore {
    inner: Arc<RwLock<HashMap<String, InMemorySlot>>>,
}

impl InMemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for InMemoryKeyStore {
    fn lookup_by_id(&self, id: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError> {
        let g = self
            .inner
            .read()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        Ok(g.get(id).map(|s| s.record.clone()))
    }

    fn verify(&self, id: &str, plaintext: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError> {
        let phc_and_record = {
            let g = self
                .inner
                .read()
                .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
            g.get(id).map(|s| (s.phc.clone(), s.record.clone()))
        };
        let Some((phc, record)) = phc_and_record else {
            return Ok(None);
        };
        if record.is_revoked || record.is_expired() {
            return Ok(None);
        }
        if verify_password(plaintext, &phc)? {
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    fn insert(&self, record: &ApiKeyRecord, plaintext: &str) -> Result<(), KeyStoreError> {
        let phc = hash_password(plaintext)?;
        let mut g = self
            .inner
            .write()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        g.insert(
            record.id.clone(),
            InMemorySlot {
                record: record.clone(),
                phc,
            },
        );
        Ok(())
    }

    fn revoke(&self, id: &str) -> Result<bool, KeyStoreError> {
        let mut g = self
            .inner
            .write()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        if let Some(slot) = g.get_mut(id) {
            slot.record.is_revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn mark_used(&self, id: &str, now: DateTime<Utc>) -> Result<(), KeyStoreError> {
        let mut g = self
            .inner
            .write()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        if let Some(slot) = g.get_mut(id) {
            slot.record.last_used_at = Some(now);
        }
        Ok(())
    }

    fn list_all(&self) -> Result<Vec<ApiKeyRecord>, KeyStoreError> {
        let g = self
            .inner
            .read()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
        Ok(g.values().map(|s| s.record.clone()).collect())
    }
}

// ─── DuckDB-backed implementation (production) ───────────────────────────────

/// SQL schema applied at construction time.
///
/// `phc_hash` is stored verbatim — the PHC string already includes the
/// algorithm, version, params and salt. `tenant_id` is nullable to match
/// the existing `org_id` field on `ApiKeyRecord`. `revoked_at` is
/// nullable so revocation is a single-row UPDATE rather than a row
/// rewrite.
///
/// Timestamps are stored as RFC-3339 TEXT rather than DuckDB TIMESTAMP
/// to dodge a roundtrip quirk: `duckdb-rs` (without the optional
/// `chrono` feature) cannot read a TIMESTAMP column straight into
/// `String`, so the rest of `orp-storage` works around it with
/// `CAST(...) AS VARCHAR` in every SELECT. Keeping these columns as
/// TEXT is simpler and avoids dragging an extra feature into the
/// security crate just for ten timestamps.
const API_KEYS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS api_keys (
    id              TEXT PRIMARY KEY,
    phc_hash        TEXT NOT NULL,
    role            TEXT NOT NULL,
    tenant_id       TEXT,
    name            TEXT NOT NULL,
    scopes          TEXT NOT NULL,
    rate_limit      BIGINT NOT NULL,
    expires_at      TEXT,
    created_at      TEXT NOT NULL,
    last_used_at    TEXT,
    revoked_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_keys_tenant ON api_keys(tenant_id);
"#;

/// Persistent `KeyStore` backed by an embedded DuckDB connection.
///
/// The connection is held behind a `Mutex` because `duckdb::Connection`
/// is `Send` but **not** `Sync`. ORP's existing storage path already
/// serialises through the same primitive, so we match that pattern here
/// rather than introducing a connection pool.
#[derive(Debug)]
pub struct DuckDbKeyStore {
    conn: Arc<Mutex<Connection>>,
}

impl DuckDbKeyStore {
    /// Open (or create) the `api_keys` table on the supplied connection.
    ///
    /// The caller owns the underlying DuckDB file — production wires this
    /// to the same connection the rest of ORP uses (`storage.duckdb.path`),
    /// so authentication state lives in the same single-binary single-file
    /// world as entity / event / audit data.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, KeyStoreError> {
        {
            let g = conn
                .lock()
                .map_err(|e| KeyStoreError::Backend(e.to_string()))?;
            g.execute_batch(API_KEYS_SCHEMA)?;
        }
        Ok(Self { conn })
    }

    /// Convenience constructor that opens its own DuckDB file. Used by
    /// the test suite and by the bootstrap CLI flow when no central
    /// connection is wired up yet.
    pub fn open_path(path: &str) -> Result<Self, KeyStoreError> {
        let conn = Connection::open(path)?;
        Self::new(Arc::new(Mutex::new(conn)))
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KeyStoreError> {
        self.conn
            .lock()
            .map_err(|e| KeyStoreError::Backend(e.to_string()))
    }

    fn row_to_record(row: &duckdb::Row) -> Result<ApiKeyRecord, duckdb::Error> {
        // Column order matches the SELECT in `lookup_by_id` /
        // `list_all`. We deliberately avoid pulling `phc_hash` here —
        // it is only loaded inside `verify` so it never escapes this
        // module.
        let id: String = row.get(0)?;
        // `role` is stored for forward compat (RBAC integration);
        // not exposed on `ApiKeyRecord` today, but persisted so a
        // future migration doesn't lose it. Pulled but discarded here.
        let _role: String = row.get(1)?;
        let tenant_id: Option<String> = row.get(2)?;
        let name: String = row.get(3)?;
        let scopes_csv: String = row.get(4)?;
        let rate_limit: i64 = row.get(5)?;
        let expires_at: Option<String> = row.get(6)?;
        let created_at: String = row.get(7)?;
        let last_used_at: Option<String> = row.get(8)?;
        let revoked_at: Option<String> = row.get(9)?;

        let scopes: Vec<String> = if scopes_csv.is_empty() {
            Vec::new()
        } else {
            scopes_csv.split(',').map(|s| s.to_string()).collect()
        };

        Ok(ApiKeyRecord {
            id,
            // `key_hash` is a legacy field on the public record; we
            // never expose the Argon2id PHC string outside this module,
            // so leave it empty for backwards JSON-schema compat.
            key_hash: String::new(),
            name,
            scopes,
            rate_limit_per_second: rate_limit as u64,
            expires_at: expires_at.as_deref().and_then(parse_ts),
            is_revoked: revoked_at.is_some(),
            org_id: tenant_id,
            created_at: parse_ts(&created_at).unwrap_or_else(Utc::now),
            last_used_at: last_used_at.as_deref().and_then(parse_ts),
        })
    }
}

/// Parse a TIMESTAMP value from DuckDB's text representation. DuckDB
/// returns timestamps as `YYYY-MM-DD HH:MM:SS[.fff]` (no timezone) for
/// our schema; we treat them as UTC.
fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
                .map(|n| n.and_utc())
                .ok()
        })
}

impl KeyStore for DuckDbKeyStore {
    fn lookup_by_id(&self, id: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, tenant_id, name, scopes, rate_limit, expires_at, created_at, \
             last_used_at, revoked_at FROM api_keys WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::row_to_record(row)?)),
            None => Ok(None),
        }
    }

    fn verify(&self, id: &str, plaintext: &str) -> Result<Option<ApiKeyRecord>, KeyStoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, tenant_id, name, scopes, rate_limit, expires_at, created_at, \
             last_used_at, revoked_at, phc_hash FROM api_keys WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let record = Self::row_to_record(row)?;
        let phc: String = row.get(10)?;
        // Drop the row + statement so we release the connection lock
        // before the (potentially-slow) Argon2id verify. We've already
        // copied everything we need.
        drop(rows);
        drop(stmt);
        drop(conn);

        if record.is_revoked || record.is_expired() {
            return Ok(None);
        }
        if verify_password(plaintext, &phc)? {
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    fn insert(&self, record: &ApiKeyRecord, plaintext: &str) -> Result<(), KeyStoreError> {
        let phc = hash_password(plaintext)?;
        let conn = self.conn()?;
        let scopes_csv = record.scopes.join(",");
        // We don't model an explicit `role` column on `ApiKeyRecord`
        // yet — store the org_id-or-"user" placeholder for forward
        // compat. The role column is intentionally future-proofing for
        // RBAC and not used by today's auth flow.
        let role = "api_key";
        conn.execute(
            "INSERT INTO api_keys (id, phc_hash, role, tenant_id, name, scopes, rate_limit, \
             expires_at, created_at, last_used_at, revoked_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)",
            params![
                record.id,
                phc,
                role,
                record.org_id,
                record.name,
                scopes_csv,
                record.rate_limit_per_second as i64,
                record.expires_at.map(|t| t.to_rfc3339()),
                record.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn revoke(&self, id: &str) -> Result<bool, KeyStoreError> {
        let conn = self.conn()?;
        let now = Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, id],
        )?;
        Ok(n > 0)
    }

    fn mark_used(&self, id: &str, now: DateTime<Utc>) -> Result<(), KeyStoreError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE api_keys SET last_used_at = ? WHERE id = ?",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    fn list_all(&self) -> Result<Vec<ApiKeyRecord>, KeyStoreError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, role, tenant_id, name, scopes, rate_limit, expires_at, created_at, \
             last_used_at, revoked_at FROM api_keys",
        )?;
        let rows = stmt.query_map([], Self::row_to_record)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn count(&self) -> Result<usize, KeyStoreError> {
        let conn = self.conn()?;
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM api_keys", [], |row| row.get(0))?;
        Ok(n as usize)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_keys::ApiKeyRecord;
    use chrono::Duration;

    fn sample_record(id: &str) -> ApiKeyRecord {
        ApiKeyRecord {
            id: id.to_string(),
            key_hash: String::new(),
            name: format!("test-{id}"),
            scopes: vec!["entities:read".to_string(), "monitors:read".to_string()],
            rate_limit_per_second: 1000,
            expires_at: Some(Utc::now() + Duration::seconds(3600)),
            is_revoked: false,
            org_id: Some("org-1".to_string()),
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    /// PHC roundtrip — the same plaintext verifies, a different one does not.
    #[test]
    fn phc_hash_and_verify_roundtrip() {
        let phc = hash_password("hunter2").unwrap();
        assert!(phc.starts_with("$argon2id$"), "phc={phc}");
        assert!(verify_password("hunter2", &phc).unwrap());
        assert!(!verify_password("hunter3", &phc).unwrap());
    }

    /// PHC strings for the same plaintext differ (random salt).
    #[test]
    fn phc_includes_random_salt() {
        let a = hash_password("same-input").unwrap();
        let b = hash_password("same-input").unwrap();
        assert_ne!(a, b, "two hashes of the same plaintext must differ");
    }

    // ── In-memory store ──────────────────────────────────────────────────

    #[test]
    fn in_memory_insert_and_verify() {
        let store = InMemoryKeyStore::new();
        let rec = sample_record("id-1");
        store.insert(&rec, "plain-key-1").unwrap();
        let got = store.verify("id-1", "plain-key-1").unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().name, "test-id-1");
    }

    #[test]
    fn in_memory_wrong_plaintext_returns_none() {
        let store = InMemoryKeyStore::new();
        let rec = sample_record("id-1");
        store.insert(&rec, "right-key").unwrap();
        assert!(store.verify("id-1", "wrong-key").unwrap().is_none());
    }

    #[test]
    fn in_memory_unknown_id_returns_none() {
        let store = InMemoryKeyStore::new();
        assert!(store.verify("never-inserted", "any-key").unwrap().is_none());
    }

    #[test]
    fn in_memory_revoke_then_verify() {
        let store = InMemoryKeyStore::new();
        let rec = sample_record("id-1");
        store.insert(&rec, "plain").unwrap();
        assert!(store.revoke("id-1").unwrap());
        // After revoke, verify returns None even with the right plaintext.
        assert!(store.verify("id-1", "plain").unwrap().is_none());
        // Lookup still finds the (now-revoked) record so admin tooling
        // can audit it.
        let rec_after = store.lookup_by_id("id-1").unwrap().unwrap();
        assert!(rec_after.is_revoked);
    }

    #[test]
    fn in_memory_mark_used() {
        let store = InMemoryKeyStore::new();
        let rec = sample_record("id-1");
        store.insert(&rec, "plain").unwrap();
        let before = store.lookup_by_id("id-1").unwrap().unwrap();
        assert!(before.last_used_at.is_none());
        let now = Utc::now();
        store.mark_used("id-1", now).unwrap();
        let after = store.lookup_by_id("id-1").unwrap().unwrap();
        assert_eq!(after.last_used_at, Some(now));
    }

    // ── DuckDB-backed store ──────────────────────────────────────────────

    #[test]
    fn duckdb_persistence_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let path_str = path.to_str().unwrap().to_string();

        // First open — insert a key.
        {
            let store = DuckDbKeyStore::open_path(&path_str).unwrap();
            let rec = sample_record("persistent-id");
            store.insert(&rec, "secret-plaintext").unwrap();
            assert!(store
                .verify("persistent-id", "secret-plaintext")
                .unwrap()
                .is_some());
        }
        // Reopen — same key still verifies.
        {
            let store = DuckDbKeyStore::open_path(&path_str).unwrap();
            assert_eq!(store.count().unwrap(), 1);
            let got = store.verify("persistent-id", "secret-plaintext").unwrap();
            assert!(
                got.is_some(),
                "key did not survive process restart — persistence broken"
            );
            assert!(store.verify("persistent-id", "wrong").unwrap().is_none());
        }
    }

    #[test]
    fn duckdb_revoke_then_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let store = DuckDbKeyStore::open_path(path.to_str().unwrap()).unwrap();

        let rec = sample_record("rev-id");
        store.insert(&rec, "plain").unwrap();
        assert!(store.verify("rev-id", "plain").unwrap().is_some());

        assert!(store.revoke("rev-id").unwrap());
        // After revoke: verify returns None, but lookup still finds the
        // record with `is_revoked = true`.
        assert!(store.verify("rev-id", "plain").unwrap().is_none());
        let after = store.lookup_by_id("rev-id").unwrap().unwrap();
        assert!(after.is_revoked, "lookup_by_id should report revoked=true");

        // Revoking a non-existent id is Ok(false), not Err.
        assert!(!store.revoke("never-existed").unwrap());
    }

    #[test]
    fn duckdb_mark_used_updates_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let store = DuckDbKeyStore::open_path(path.to_str().unwrap()).unwrap();

        let rec = sample_record("mu-id");
        store.insert(&rec, "plain").unwrap();
        assert!(store
            .lookup_by_id("mu-id")
            .unwrap()
            .unwrap()
            .last_used_at
            .is_none());

        let now = Utc::now();
        store.mark_used("mu-id", now).unwrap();
        let after = store.lookup_by_id("mu-id").unwrap().unwrap();
        // We compare to the second to avoid sub-second representation
        // mismatch between DuckDB roundtrip and `chrono::Utc::now()`.
        assert!(
            after.last_used_at.is_some()
                && (after.last_used_at.unwrap() - now).num_seconds().abs() < 2,
            "mark_used did not persist the timestamp"
        );
    }

    #[test]
    fn duckdb_wrong_plaintext_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let store = DuckDbKeyStore::open_path(path.to_str().unwrap()).unwrap();

        let rec = sample_record("id-x");
        store.insert(&rec, "right-plaintext").unwrap();
        assert!(store.verify("id-x", "wrong-plaintext").unwrap().is_none());
    }

    #[test]
    fn duckdb_unknown_id_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let store = DuckDbKeyStore::open_path(path.to_str().unwrap()).unwrap();
        assert!(store.verify("never-inserted", "any").unwrap().is_none());
        assert!(store.lookup_by_id("never-inserted").unwrap().is_none());
    }

    #[test]
    fn duckdb_list_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.db");
        let store = DuckDbKeyStore::open_path(path.to_str().unwrap()).unwrap();
        assert_eq!(store.count().unwrap(), 0);
        store.insert(&sample_record("a"), "ka").unwrap();
        store.insert(&sample_record("b"), "kb").unwrap();
        assert_eq!(store.count().unwrap(), 2);
        let listed = store.list_all().unwrap();
        assert_eq!(listed.len(), 2);
        let ids: Vec<&str> = listed.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
    }
}
