//! ORP audit log — append-only, hash-chained, Ed25519-signed event ledger.
//!
//! The audit log records every privileged action ORP performs (entity create,
//! relationship insert, monitor rule change, …) so operators can prove
//! tamper-evidence after the fact. Two backends share the [`AuditLog`] trait:
//!
//! * [`InMemoryAuditLog`] — RAM-only ring; used in tests and `--in-memory` mode.
//! * [`PersistentAuditLog`] — DuckDB-backed; survives restarts. The chain head
//!   is recovered on startup by reading the last persisted row, so the next
//!   appended entry's `prev_hash` correctly extends the existing chain.
//!
//! All entries are SHA-256-hashed over the canonical JSON pre-image
//! (`prev_hash || canonical_entry_minus_hash_and_signature`) and then signed
//! with Ed25519. The signature is over the *hash bytes*, so verifiers only
//! need the per-row pre-image to re-derive everything.

pub mod at_rest;
pub mod crypto;
pub mod entry;
pub mod logger;
pub mod persistent;
pub mod traits;

pub use at_rest::{default_at_rest_key_path, AtRestError, AtRestKey};
pub use crypto::{compute_hash, default_audit_key_path, EventSigner};
pub use entry::{canonical_preimage, compute_content_hash, AuditEntry, GENESIS_PREV_HASH};
pub use logger::{AuditLog, InMemoryAuditLog};
pub use persistent::{export_jsonl, ExportLine, PersistentAuditLog};
pub use traits::{AuditError, AuditLogger, AuditResult, VerifyKey};
