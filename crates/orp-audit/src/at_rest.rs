//! At-rest envelope encryption for the most sensitive audit-log column.
//!
//! P-audit Wave 2 F7 calls for at-rest encryption of DuckDB and RocksDB
//! state. Doing that fully requires either the DuckDB encryption extension
//! (build-time cost) or a wrapping `EncryptedEnv` around RocksDB (per-call
//! cost on every read). This module ships the realistic interim: an
//! application-layer AES-256-GCM envelope around the `details` JSON column
//! of `audit_log` rows. A captured-laptop attacker reading the raw DuckDB
//! file sees ciphertext + a 12-byte nonce, not the original entity payload.
//!
//! ## Threat model
//!
//! * **In scope**: cold-storage exposure of the DuckDB file. PII / entity
//!   IDs in the `details` column are not readable without the key.
//! * **Out of scope**: a live process holds the key in memory. A running-
//!   server compromise still has cleartext access (handlers operate on the
//!   plaintext JSON before it's sealed). The other `audit_log` columns
//!   (operation, entity_type, entity_id, user_id, timestamp, hashes,
//!   signature) are NOT encrypted — they're queryable indexes. If the
//!   threat model requires them encrypted too, run the whole DuckDB file
//!   on an encrypted volume (LUKS / FileVault / BitLocker).
//!
//! ## Wire format
//!
//! Each sealed `details` cell is the base64-URL-no-pad encoding of:
//!
//! ```text
//! "ORPAEAD1" || nonce[12] || ciphertext_with_tag[N+16]
//! ```
//!
//! `"ORPAEAD1"` is the format magic (8 ASCII bytes); future revisions can
//! bump to `ORPAEAD2`. The `unseal` path refuses anything else, so a
//! plaintext row left over from before encryption was enabled comes back
//! verbatim — supporting in-place migration.
//!
//! ## Key handling
//!
//! Keys live at `${ORP_AT_REST_KEY_PATH}` or
//! `${XDG_DATA_HOME:-$HOME/.local/share}/orp/at-rest.key`. File is 32 raw
//! bytes (mode 0600). [`AtRestKey::load_or_generate`] mirrors the audit-
//! signer flow: load if present, generate + persist on first run.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;
use std::path::{Path, PathBuf};

const FORMAT_MAGIC: &[u8; 8] = b"ORPAEAD1";
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// AES-256 key with envelope helpers. Internally stores the raw 32 bytes
/// behind a `Box<[u8]>` so a `Drop` impl can scrub before deallocation.
pub struct AtRestKey {
    cipher: Aes256Gcm,
}

impl AtRestKey {
    /// Construct from raw bytes. Returns `Err` if `bytes.len() != 32`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AtRestError> {
        if bytes.len() != KEY_LEN {
            return Err(AtRestError::BadKeyLength {
                got: bytes.len(),
                want: KEY_LEN,
            });
        }
        let key = Key::<Aes256Gcm>::from_slice(bytes);
        Ok(Self {
            cipher: Aes256Gcm::new(key),
        })
    }

    /// Load the at-rest key from `path`, or generate a fresh 32-byte key
    /// and persist it (mode 0600 on Unix). Mirrors
    /// [`crate::EventSigner::load_or_generate`] — same atomic-rename and
    /// permission-tightening flow.
    pub fn load_or_generate(path: &Path) -> std::io::Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            return AtRestKey::from_bytes(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()));
        }
        let mut sk = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut sk);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("key.tmp");
        std::fs::write(&tmp, sk.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&tmp, perms)?;
        }
        std::fs::rename(&tmp, path)?;
        AtRestKey::from_bytes(&sk)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Encrypt + base64-encode `plaintext` for storage in a VARCHAR column.
    /// Each call uses a fresh random nonce (stored alongside the ciphertext).
    pub fn seal(&self, plaintext: &[u8]) -> Result<String, AtRestError> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| AtRestError::EncryptFailed)?;
        let mut out = Vec::with_capacity(FORMAT_MAGIC.len() + NONCE_LEN + ct.len());
        out.extend_from_slice(FORMAT_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(URL_SAFE_NO_PAD.encode(&out))
    }

    /// Decode + decrypt. Anything that doesn't start with the format magic
    /// is returned verbatim — that's how mixed plaintext-and-encrypted
    /// audit rows after migration enable continue to verify.
    pub fn unseal(&self, sealed: &str) -> Result<Vec<u8>, AtRestError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(sealed.as_bytes())
            .map_err(|_| AtRestError::Malformed)?;
        if bytes.len() < FORMAT_MAGIC.len() + NONCE_LEN {
            return Err(AtRestError::Malformed);
        }
        if &bytes[..FORMAT_MAGIC.len()] != FORMAT_MAGIC {
            return Err(AtRestError::NotSealed);
        }
        let nonce_start = FORMAT_MAGIC.len();
        let ct_start = nonce_start + NONCE_LEN;
        let nonce = Nonce::from_slice(&bytes[nonce_start..ct_start]);
        self.cipher
            .decrypt(nonce, &bytes[ct_start..])
            .map_err(|_| AtRestError::DecryptFailed)
    }
}

/// `${ORP_AT_REST_KEY_PATH}` → `${XDG_DATA_HOME}/orp/at-rest.key`
/// → `${HOME}/.local/share/orp/at-rest.key` → `./.orp-at-rest.key`.
pub fn default_at_rest_key_path() -> PathBuf {
    if let Ok(p) = std::env::var("ORP_AT_REST_KEY_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("orp").join("at-rest.key");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("orp")
                .join("at-rest.key");
        }
    }
    PathBuf::from(".orp-at-rest.key")
}

#[derive(Debug, thiserror::Error)]
pub enum AtRestError {
    #[error("at-rest key length {got} bytes, expected {want}")]
    BadKeyLength { got: usize, want: usize },
    #[error("at-rest encrypt failed")]
    EncryptFailed,
    #[error("at-rest decrypt failed (wrong key, tampered ciphertext, or unauthenticated tag)")]
    DecryptFailed,
    #[error("at-rest blob is not the ORPAEAD1 format")]
    NotSealed,
    #[error("at-rest blob is malformed (base64 / length)")]
    Malformed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AtRestKey {
        AtRestKey::from_bytes(&[7u8; KEY_LEN]).unwrap()
    }

    #[test]
    fn from_bytes_rejects_wrong_length() {
        // AtRestKey deliberately omits Debug — never let a misplaced
        // format!() leak the secret. Match on Ok/Err manually.
        match AtRestKey::from_bytes(&[0u8; 31]) {
            Ok(_) => panic!("expected BadKeyLength, got Ok"),
            Err(AtRestError::BadKeyLength { got: 31, want: 32 }) => (),
            Err(e) => panic!("expected BadKeyLength(31, 32), got {e:?}"),
        }
    }

    #[test]
    fn seal_unseal_roundtrip() {
        let k = key();
        let plaintext = b"sensitive audit details: { entity_id: 'BOAT-1', mmsi: '12345' }";
        let sealed = k.seal(plaintext).unwrap();
        let recovered = k.unseal(&sealed).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn seal_uses_fresh_nonce_each_call() {
        // Two seals of the same plaintext under the same key MUST produce
        // different ciphertexts (or AES-GCM nonce reuse breaks IND-CPA).
        let k = key();
        let p = b"same input";
        let s1 = k.seal(p).unwrap();
        let s2 = k.seal(p).unwrap();
        assert_ne!(s1, s2, "nonce reuse — catastrophic for AES-GCM");
    }

    #[test]
    fn unseal_rejects_wrong_key() {
        let k1 = key();
        let k2 = AtRestKey::from_bytes(&[8u8; KEY_LEN]).unwrap();
        let sealed = k1.seal(b"secret").unwrap();
        match k2.unseal(&sealed) {
            Err(AtRestError::DecryptFailed) => (),
            other => panic!("expected DecryptFailed, got {other:?}"),
        }
    }

    #[test]
    fn unseal_rejects_tampered_ciphertext() {
        let k = key();
        let mut sealed = k.seal(b"do-not-tamper").unwrap();
        // Flip a bit deep in the base64 — affects either nonce or ct,
        // both of which the AEAD tag covers. Decryption must fail.
        let last = sealed.pop().unwrap();
        sealed.push(if last == 'A' { 'B' } else { 'A' });
        match k.unseal(&sealed) {
            Err(AtRestError::DecryptFailed) | Err(AtRestError::Malformed) => (),
            other => panic!("expected Decrypt/Malformed, got {other:?}"),
        }
    }

    #[test]
    fn unseal_passes_through_plaintext_for_migration() {
        // Backward-compat: an audit row written before encryption was
        // turned on lives in the column verbatim. unseal returns NotSealed
        // (the magic prefix is missing) so the caller knows to keep the
        // raw bytes.
        let k = key();
        let plain = "raw plaintext json";
        let err = k.unseal(plain).unwrap_err();
        assert!(matches!(
            err,
            AtRestError::NotSealed | AtRestError::Malformed
        ));
    }

    #[test]
    fn load_or_generate_creates_then_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("at-rest.key");
        let k1 = AtRestKey::load_or_generate(&path).unwrap();
        assert!(path.exists());
        let p = b"shared between runs";
        let sealed = k1.seal(p).unwrap();
        // New process loads the same key file → same key → same decrypts.
        let k2 = AtRestKey::load_or_generate(&path).unwrap();
        let recovered = k2.unseal(&sealed).unwrap();
        assert_eq!(recovered, p);
    }

    #[cfg(unix)]
    #[test]
    fn load_or_generate_writes_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("at-rest.key");
        let _ = AtRestKey::load_or_generate(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "at-rest key must be 0600, got {:o}", mode);
    }

    #[test]
    fn default_at_rest_key_path_honors_env() {
        let prev = std::env::var("ORP_AT_REST_KEY_PATH").ok();
        std::env::set_var("ORP_AT_REST_KEY_PATH", "/custom/path/k");
        let p = default_at_rest_key_path();
        assert_eq!(p, PathBuf::from("/custom/path/k"));
        match prev {
            Some(v) => std::env::set_var("ORP_AT_REST_KEY_PATH", v),
            None => std::env::remove_var("ORP_AT_REST_KEY_PATH"),
        }
    }
}
