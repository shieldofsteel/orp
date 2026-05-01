use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Compute SHA-256 hash of data, returned as hex string
pub fn compute_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Ed25519 event signer for data provenance
pub struct EventSigner {
    signing_key: SigningKey,
}

impl EventSigner {
    /// Generate a new random keypair using the OS CSPRNG.
    ///
    /// Uses `OsRng` (not `thread_rng`) because the secret material backs
    /// audit-log signatures — a non-cryptographic RNG would let an attacker
    /// who can observe one signature predict future ones.
    pub fn new() -> Self {
        let mut csprng = OsRng;
        let mut secret_key_bytes = [0u8; 32];
        csprng.fill_bytes(&mut secret_key_bytes);
        let signing_key = SigningKey::from_bytes(&secret_key_bytes);
        Self { signing_key }
    }

    /// Sign data bytes
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        let signature = self.signing_key.sign(data);
        signature.to_bytes().to_vec()
    }

    /// Verify a signature against data
    pub fn verify(&self, data: &[u8], signature: &[u8]) -> bool {
        if signature.len() != 64 {
            return false;
        }
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(signature);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        let verifying_key: VerifyingKey = self.signing_key.verifying_key();
        verifying_key.verify(data, &sig).is_ok()
    }

    /// Get the public key bytes
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.signing_key.verifying_key().to_bytes().to_vec()
    }

    /// Load the Ed25519 secret key from `secret_path`, or generate a fresh
    /// keypair and persist it on first run. The file is created with mode
    /// 0600 on Unix and a world-readable sibling `<name>.pub` is written
    /// alongside so external verifiers can pin the trust root without ever
    /// reading the private key.
    ///
    /// Closes P-audit F2: the previous `EventSigner::new()` regenerated a
    /// fresh keypair on every process start, which meant signatures emitted
    /// before a restart could not be verified by the running instance.
    pub fn load_or_generate(secret_path: &Path) -> std::io::Result<Self> {
        if secret_path.exists() {
            let bytes = std::fs::read(secret_path)?;
            if bytes.len() != 32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "audit signing key '{}' has length {}, expected 32",
                        secret_path.display(),
                        bytes.len()
                    ),
                ));
            }
            let mut sk = [0u8; 32];
            sk.copy_from_slice(&bytes);
            let signing_key = SigningKey::from_bytes(&sk);
            // Self-heal the public-key sidecar. If the previous run was
            // killed between the secret-rename and the .pub-write, the
            // sidecar is missing — recreate it now so external verifiers
            // pinning the .pub file work without manual recovery.
            let pub_path = pub_path_for(secret_path);
            if !pub_path.exists() {
                let pub_bytes = signing_key.verifying_key().to_bytes();
                if let Err(e) = std::fs::write(&pub_path, pub_bytes.as_ref()) {
                    tracing::warn!(
                        path = %pub_path.display(),
                        error = %e,
                        "could not heal missing audit-key public sidecar"
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&pub_path) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o644);
                        let _ = std::fs::set_permissions(&pub_path, perms);
                    }
                }
            }
            return Ok(Self { signing_key });
        }
        let mut csprng = OsRng;
        let mut sk = [0u8; 32];
        csprng.fill_bytes(&mut sk);
        let signing_key = SigningKey::from_bytes(&sk);
        if let Some(parent) = secret_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Two-step atomic publish: write to `.tmp` with the locked-down
        // mode set BEFORE the rename, so there is never a moment where the
        // 32 bytes of secret material exist on disk with default 0644.
        let tmp = secret_path.with_extension("ed25519.tmp");
        std::fs::write(&tmp, sk.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&tmp, perms)?;
        }
        std::fs::rename(&tmp, secret_path)?;
        // Public-key sidecar: any operator (or a remote attestation peer)
        // can read this file without needing the secret key.
        let pub_path = pub_path_for(secret_path);
        let pub_bytes = signing_key.verifying_key().to_bytes();
        std::fs::write(&pub_path, pub_bytes.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&pub_path)?.permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&pub_path, perms)?;
        }
        Ok(Self { signing_key })
    }
}

/// Public-key sidecar path for a given secret-key path:
/// `audit-key.ed25519` → `audit-key.pub.ed25519`.
fn pub_path_for(secret_path: &Path) -> PathBuf {
    let mut name = secret_path
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("audit-key"));
    name.push(".pub.ed25519");
    secret_path.with_file_name(name)
}

/// Default audit-key location, honoring `XDG_DATA_HOME` then `$HOME`.
pub fn default_audit_key_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("orp").join("audit-key.ed25519");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("orp")
                .join("audit-key.ed25519");
        }
    }
    PathBuf::from(".orp-audit-key.ed25519")
}

impl Default for EventSigner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash() {
        let hash = compute_hash("hello world");
        assert_eq!(hash.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_sign_verify() {
        let signer = EventSigner::new();
        let data = b"test event data";
        let signature = signer.sign(data);
        assert!(signer.verify(data, &signature));
    }

    #[test]
    fn test_invalid_signature() {
        let signer = EventSigner::new();
        let data = b"test data";
        let mut bad_sig = signer.sign(data);
        bad_sig[0] ^= 0xFF; // corrupt signature
        assert!(!signer.verify(data, &bad_sig));
    }

    #[test]
    fn test_hash_deterministic() {
        let h1 = compute_hash("same input");
        let h2 = compute_hash("same input");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_different_inputs() {
        let h1 = compute_hash("hello");
        let h2 = compute_hash("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_empty_input() {
        let h = compute_hash("");
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn test_public_key_bytes() {
        let signer = EventSigner::new();
        let pk = signer.public_key_bytes();
        assert_eq!(pk.len(), 32); // Ed25519 public key = 32 bytes
    }

    #[test]
    fn test_different_signers_produce_different_sigs() {
        let s1 = EventSigner::new();
        let s2 = EventSigner::new();
        let data = b"test data";
        let sig1 = s1.sign(data);
        let sig2 = s2.sign(data);
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_verify_wrong_signer_fails() {
        let s1 = EventSigner::new();
        let s2 = EventSigner::new();
        let data = b"test data";
        let sig = s1.sign(data);
        assert!(!s2.verify(data, &sig));
    }

    #[test]
    fn test_verify_wrong_data_fails() {
        let signer = EventSigner::new();
        let sig = signer.sign(b"original data");
        assert!(!signer.verify(b"different data", &sig));
    }

    #[test]
    fn test_verify_short_signature_fails() {
        let signer = EventSigner::new();
        assert!(!signer.verify(b"data", &[0u8; 32]));
    }

    #[test]
    fn test_verify_empty_signature_fails() {
        let signer = EventSigner::new();
        assert!(!signer.verify(b"data", &[]));
    }

    #[test]
    fn test_default_signer() {
        let signer = EventSigner::default();
        let sig = signer.sign(b"test");
        assert!(signer.verify(b"test", &sig));
    }

    #[test]
    fn test_sign_large_data() {
        let signer = EventSigner::new();
        let data = vec![0xABu8; 10_000];
        let sig = signer.sign(&data);
        assert!(signer.verify(&data, &sig));
    }

    #[test]
    fn load_or_generate_creates_then_reuses_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subdir").join("audit-key.ed25519");
        let s1 = EventSigner::load_or_generate(&path).unwrap();
        assert!(path.exists(), "secret key file must exist after first call");
        let pub_path = path.with_file_name("audit-key.pub.ed25519");
        assert!(pub_path.exists(), "public-key sidecar must exist");
        let s2 = EventSigner::load_or_generate(&path).unwrap();
        // Same signer ⇒ same public key (regression: F2 was that this
        // returned a fresh key each call).
        assert_eq!(s1.public_key_bytes(), s2.public_key_bytes());
        // And signatures verify across instances.
        let sig = s1.sign(b"audit row");
        assert!(s2.verify(b"audit row", &sig));
    }

    #[cfg(unix)]
    #[test]
    fn load_or_generate_writes_secret_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-key.ed25519");
        let _ = EventSigner::load_or_generate(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "secret key must be 0600, got {:o}", mode);
    }

    #[test]
    fn load_or_generate_self_heals_missing_pub_sidecar() {
        // Crypto-audit concern: if a previous run was killed between
        // secret-rename and pubkey-write, the sidecar is missing on the
        // next start. load_or_generate now self-heals — without that, an
        // external verifier pinning .pub.ed25519 stays broken until manual
        // recovery.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-key.ed25519");
        let pub_path = path.with_file_name("audit-key.pub.ed25519");

        let s1 = EventSigner::load_or_generate(&path).unwrap();
        assert!(pub_path.exists());
        // Simulate the crash: delete the sidecar.
        std::fs::remove_file(&pub_path).unwrap();
        assert!(!pub_path.exists());

        let s2 = EventSigner::load_or_generate(&path).unwrap();
        // Sidecar is back.
        assert!(pub_path.exists(), "load_or_generate must self-heal sidecar");
        // And it matches the secret-derived public key.
        let expected = s2.public_key_bytes();
        let actual = std::fs::read(&pub_path).unwrap();
        assert_eq!(actual, expected);
        // Same signer key.
        assert_eq!(s1.public_key_bytes(), s2.public_key_bytes());
    }

    #[test]
    fn load_or_generate_rejects_truncated_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit-key.ed25519");
        std::fs::write(&path, b"too-short").unwrap();
        // EventSigner deliberately omits Debug so a misplaced format!() can't
        // leak secret material — match on Result instead of unwrap_err().
        match EventSigner::load_or_generate(&path) {
            Ok(_) => panic!("expected truncated key file to be rejected"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
        }
    }
}
