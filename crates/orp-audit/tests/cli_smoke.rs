//! End-to-end smoke: write audit rows via PersistentAuditLog, then read them
//! back via the same `read_all` path the `orp audit verify` CLI subcommand
//! uses, and recompute every signature with the recorded public key.
//!
//! Catches integration regressions that would slip past unit-tests-in-place
//! (for example, schema drift between `orp-storage` and `orp-audit`).

use orp_audit::traits::VerifyKey;
use orp_audit::{AuditLogger, EventSigner, PersistentAuditLog};
use std::sync::Arc;

#[tokio::test]
async fn write_then_read_back_via_cli_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.duckdb");

    let signer = Arc::new(EventSigner::new());
    let pubkey_hex = hex::encode(signer.public_key_bytes());

    // Write 3 entries.
    {
        let log = PersistentAuditLog::open(&path, signer.clone()).unwrap();
        for i in 0..3 {
            log.record(
                "smoke_op",
                Some("e"),
                Some(&format!("id:{}", i)),
                Some("user"),
                serde_json::json!({"i": i}),
            )
            .await
            .unwrap();
        }
    }

    // Re-open with a *different* logger handle but the same DB and verify.
    let signer2 = Arc::new(EventSigner::new()); // NB: different signer is fine for read-back
    let log2 = PersistentAuditLog::open(&path, signer2).unwrap();
    let entries = log2.replay(None).await.unwrap();
    assert_eq!(entries.len(), 3);
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.sequence_number, (i as u64) + 1);
        assert_eq!(e.operation, "smoke_op");
    }

    // Verify chain using the *original* signer's public key — proves the
    // signature column is what the writer put there, not what the reader
    // generates.
    let vk = VerifyKey::from_hex(&pubkey_hex).unwrap();
    let conn_arc = {
        // Use the helper that the CLI walks: read_all + manual chain check.
        let conn = duckdb::Connection::open(&path).unwrap();
        let entries = PersistentAuditLog::read_all(&conn).unwrap();
        for e in entries {
            assert!(
                vk.verify_signature(&e.content_hash, &e.signature),
                "signature must verify for seq {}",
                e.sequence_number
            );
        }
    };
    let _ = conn_arc;
}
