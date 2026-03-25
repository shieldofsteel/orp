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
}
