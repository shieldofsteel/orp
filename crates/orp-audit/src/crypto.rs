use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

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
    /// Generate a new random keypair
    pub fn new() -> Self {
        let mut csprng = rand::thread_rng();
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
}
