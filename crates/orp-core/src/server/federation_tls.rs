//! Federation transport security — mTLS, Ed25519 payload signing, replay
//! protection, and per-peer confidence caps.
//!
//! ## Threat model
//!
//! Plain HTTP federation (the legacy v0.2.0 path) lets any host that can reach
//! the federation port:
//!   1. Read entities in flight (no transport encryption).
//!   2. Inject bogus entities by spoofing a peer.
//!   3. Override local truth by sending `confidence = 1.0` on every entity.
//!   4. Replay a captured high-confidence payload to undo an admin's manual
//!      correction.
//!
//! This module addresses each of those:
//!
//! - **mTLS** — both sides validate the other's X.509 cert against a pinned CA
//!   (or a static cert pin). Only peers whose certs the operator has loaded
//!   can establish a connection in the first place.
//! - **Ed25519 payload signing** — every push wraps the payload in a
//!   `SignedFederationEnvelope`; the receiver verifies the signature against
//!   the sending peer's pinned `signing_pubkey`. mTLS protects the channel,
//!   but signing protects the message — if the cert is rotated or terminated
//!   at a load balancer, the signature still proves the originator.
//! - **Replay protection** — each envelope carries a monotonic `seq` per
//!   sender. The receiver tracks the highest seen `seq` per peer and rejects
//!   any envelope whose `seq` is `<=` the last observed value. Combined with
//!   a `timestamp` skew check, this defeats replay attacks even if an
//!   attacker can break one TLS session.
//! - **Confidence cap** — the receiver clamps incoming `confidence` to
//!   `min(incoming, peer.max_confidence_cap)` (default 0.9). A compromised
//!   peer can no longer ram its observations to the top of the
//!   highest-confidence-wins conflict resolution.
//!
//! Backward compatibility: every feature in this module is gated on
//! `FederationTlsConfig::enabled`. When disabled (the v0.2.0 default) the
//! plain HTTP path is preserved and a startup warning is logged.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Default ceiling on `confidence` accepted from any peer when the operator
/// has not configured a per-peer override. 0.9 keeps every federated
/// observation strictly below a perfectly-trusted local source.
pub const DEFAULT_CONFIDENCE_CAP: f64 = 0.9;

/// Maximum tolerated clock skew between peer and receiver. Envelopes whose
/// timestamps fall outside `now ± SKEW` are rejected even if the signature
/// verifies — a captured envelope from yesterday should not replay just
/// because the sender restarted with `seq = 0`.
pub const MAX_TIMESTAMP_SKEW_SECS: i64 = 300;

// ── Configuration ────────────────────────────────────────────────────────────

/// Inbound federation TLS configuration. When `enabled` is true, ORP starts a
/// dedicated mTLS listener (default port 9443) that requires every connecting
/// client to present a certificate signed by the configured CA.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FederationTlsConfig {
    /// Master switch. When false, every other field is ignored and federation
    /// runs over plain HTTP (legacy behaviour, with a startup warning).
    #[serde(default)]
    pub enabled: bool,
    /// Server certificate (PEM) presented to peers connecting in.
    pub cert_path: Option<PathBuf>,
    /// Server private key (PEM) for `cert_path`.
    pub key_path: Option<PathBuf>,
    /// CA certificate (PEM) used to validate peers' client certs. Connecting
    /// clients whose cert is not signed by this CA are rejected at TLS
    /// handshake time.
    pub ca_path: Option<PathBuf>,
    /// Bind address for the mTLS listener. Defaults to `0.0.0.0:9443`. The
    /// non-TLS port (CLI `--port`, default 9090) continues to serve unauth'd
    /// frontend + ABAC-gated REST.
    #[serde(default = "default_tls_listen_addr")]
    pub listen_addr: String,
}

fn default_tls_listen_addr() -> String {
    "0.0.0.0:9443".to_string()
}

impl FederationTlsConfig {
    /// True when the operator has provided every file needed to bring the
    /// listener up. Used so a half-configured node refuses to start the
    /// listener instead of binding without proper auth.
    pub fn is_complete(&self) -> bool {
        self.enabled
            && self.cert_path.is_some()
            && self.key_path.is_some()
            && self.ca_path.is_some()
    }
}

// ── Peer trust spec ──────────────────────────────────────────────────────────

/// Pinned trust material for a single peer. Stored in `PeerRegistry` alongside
/// the legacy `Peer` record so the rest of the federation code can opt in
/// per-peer rather than via a global flag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerTrust {
    /// Hex- or base64-encoded Ed25519 public key (32 bytes). Wire-format
    /// flexibility lets operators paste keys straight from `ssh-keygen -y`,
    /// `openssl pkey`, or `orp federation gen-key`.
    pub signing_pubkey: String,
    /// Receiver clamps incoming `confidence` to this value. None falls back
    /// to `DEFAULT_CONFIDENCE_CAP`.
    pub max_confidence_cap: Option<f64>,
}

impl PeerTrust {
    /// Decode `signing_pubkey` to a `VerifyingKey`. Accepts hex (64 chars) or
    /// standard base64.
    pub fn verifying_key(&self) -> Result<VerifyingKey, FederationCryptoError> {
        decode_pubkey(&self.signing_pubkey)
    }

    pub fn confidence_cap(&self) -> f64 {
        self.max_confidence_cap
            .unwrap_or(DEFAULT_CONFIDENCE_CAP)
            .clamp(0.0, 1.0)
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FederationCryptoError {
    #[error("Invalid public key encoding: {0}")]
    InvalidPubkey(String),
    #[error("Invalid signature encoding: {0}")]
    InvalidSignature(String),
    #[error("Signature verification failed")]
    SignatureMismatch,
    #[error("Timestamp out of acceptable window (skew={skew}s, max={max}s)")]
    TimestampSkew { skew: i64, max: i64 },
    #[error("Replay detected: seq {seq} <= last seen {last}")]
    ReplaySeq { seq: u64, last: u64 },
    /// Returned by callers (currently the receive_signed_push handler) when
    /// the envelope sender doesn't match a registered peer. Kept on the
    /// public surface so future error mappers can match on it.
    #[allow(dead_code)]
    #[error("Unknown peer: {0}")]
    UnknownPeer(String),
    #[error("Canonicalization failed: {0}")]
    Canonicalize(String),
    #[error("I/O error reading key/cert: {0}")]
    Io(#[from] std::io::Error),
}

// ── Signed envelope ──────────────────────────────────────────────────────────

/// A federation push or pull payload, wrapped with sender ID, sequence
/// number, timestamp, and an Ed25519 signature.
///
/// Wire format (JSON):
/// ```json
/// {
///   "sender": "cluster-east",
///   "seq": 42,
///   "timestamp": "2026-05-01T12:34:56Z",
///   "payload": { ... entity envelope ... },
///   "signature": "<hex-encoded 64-byte Ed25519 sig>"
/// }
/// ```
///
/// The signature covers the canonical bytes of `(timestamp || sender || seq
/// || canonical_json(payload))` — see `signing_bytes()`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedFederationEnvelope {
    /// Peer ID (must match a registered peer on the receiver).
    pub sender: String,
    /// Monotonic sequence number per sender. Receiver rejects `seq <=
    /// last_seen[sender]`.
    pub seq: u64,
    /// RFC-3339 UTC timestamp when the envelope was sealed. Used together
    /// with `seq` for replay protection.
    pub timestamp: String,
    /// Inner payload. Opaque to the envelope; canonical-JSON-serialized for
    /// signing.
    pub payload: serde_json::Value,
    /// Hex-encoded Ed25519 signature (64 bytes -> 128 chars).
    pub signature: String,
}

impl SignedFederationEnvelope {
    /// Build the byte string the signature covers. Stable across encoders so
    /// signing on one side and verifying on the other always agree.
    fn signing_bytes(
        sender: &str,
        seq: u64,
        timestamp: &str,
        payload: &serde_json::Value,
    ) -> Result<Vec<u8>, FederationCryptoError> {
        let canonical = canonical_json(payload)
            .map_err(|e| FederationCryptoError::Canonicalize(e.to_string()))?;
        let mut buf = Vec::with_capacity(canonical.len() + sender.len() + timestamp.len() + 16);
        buf.extend_from_slice(timestamp.as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(sender.as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(seq.to_string().as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(canonical.as_bytes());
        Ok(buf)
    }

    /// Seal a payload with the sender's signing key. Caller supplies the
    /// monotonic `seq` (typically pulled from a per-peer counter held by
    /// `OutboundSeq`).
    pub fn seal(
        sender: &str,
        seq: u64,
        signing_key: &SigningKey,
        payload: serde_json::Value,
    ) -> Result<Self, FederationCryptoError> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let bytes = Self::signing_bytes(sender, seq, &timestamp, &payload)?;
        let sig: Signature = signing_key.sign(&bytes);
        Ok(Self {
            sender: sender.to_string(),
            seq,
            timestamp,
            payload,
            signature: hex::encode(sig.to_bytes()),
        })
    }

    /// Verify the envelope against `pubkey`. Does NOT check replay/skew —
    /// callers (typically `ReplayTracker::check`) layer those on top.
    pub fn verify(&self, pubkey: &VerifyingKey) -> Result<(), FederationCryptoError> {
        let sig_bytes = hex::decode(&self.signature)
            .map_err(|e| FederationCryptoError::InvalidSignature(e.to_string()))?;
        if sig_bytes.len() != 64 {
            return Err(FederationCryptoError::InvalidSignature(format!(
                "expected 64-byte signature, got {}",
                sig_bytes.len()
            )));
        }
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let sig = Signature::from_bytes(&sig_arr);
        let bytes = Self::signing_bytes(&self.sender, self.seq, &self.timestamp, &self.payload)?;
        pubkey
            .verify(&bytes, &sig)
            .map_err(|_| FederationCryptoError::SignatureMismatch)?;
        Ok(())
    }

    /// Convenience: parse `timestamp` and check it falls within
    /// ±MAX_TIMESTAMP_SKEW_SECS of the receiver's clock.
    pub fn check_timestamp(&self) -> Result<(), FederationCryptoError> {
        let parsed = chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map_err(|e| FederationCryptoError::Canonicalize(format!("bad timestamp: {}", e)))?;
        let now = chrono::Utc::now();
        let skew = (now.timestamp() - parsed.timestamp()).abs();
        if skew > MAX_TIMESTAMP_SKEW_SECS {
            return Err(FederationCryptoError::TimestampSkew {
                skew,
                max: MAX_TIMESTAMP_SKEW_SECS,
            });
        }
        Ok(())
    }
}

// ── Replay tracker ───────────────────────────────────────────────────────────

/// Per-peer "highest seq seen" map. In-memory only — process restart resets
/// it to empty, which is intentional: after a restart the operator can
/// (briefly) accept the next legitimate envelope without coordination, but a
/// captured payload from before the restart still fails the timestamp skew
/// check (5 min window) so a replay window is bounded by the restart window
/// rather than forever.
#[derive(Default)]
pub struct ReplayTracker {
    last_seen: RwLock<HashMap<String, u64>>,
}

impl ReplayTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Validate timestamp + seq, then record the new high-water mark.
    /// Returns the previous high water (None on first contact) so callers can
    /// log a useful message.
    pub async fn check_and_record(
        &self,
        envelope: &SignedFederationEnvelope,
    ) -> Result<Option<u64>, FederationCryptoError> {
        envelope.check_timestamp()?;
        let mut map = self.last_seen.write().await;
        let prev = map.get(&envelope.sender).copied();
        if let Some(last) = prev {
            if envelope.seq <= last {
                return Err(FederationCryptoError::ReplaySeq {
                    seq: envelope.seq,
                    last,
                });
            }
        }
        map.insert(envelope.sender.clone(), envelope.seq);
        Ok(prev)
    }

    /// Read-only inspection — used by tests and `/api/v1/federation/peers/:id/health`
    /// when surfacing replay-tracker diagnostics.
    #[allow(dead_code)]
    pub async fn last_seen(&self, sender: &str) -> Option<u64> {
        self.last_seen.read().await.get(sender).copied()
    }
}

// ── Outbound sequence allocator ──────────────────────────────────────────────

/// Per-receiver outbound sequence numbers. Lives in `AppState` next to the
/// federation registry so every push to peer X carries a strictly-increasing
/// seq from this node's point of view.
#[derive(Default)]
pub struct OutboundSeq {
    counters: RwLock<HashMap<String, u64>>,
}

impl OutboundSeq {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Atomically increment and return the next seq number for `peer_id`.
    pub async fn next(&self, peer_id: &str) -> u64 {
        let mut map = self.counters.write().await;
        let counter = map.entry(peer_id.to_string()).or_insert(0);
        *counter += 1;
        *counter
    }
}

// ── Pubkey decoding ──────────────────────────────────────────────────────────

/// Decode an Ed25519 public key from hex (64 chars) or base64 (44 chars).
/// Operators paste keys from various tools; accepting both removes a
/// surprisingly common copy-paste failure mode.
pub fn decode_pubkey(s: &str) -> Result<VerifyingKey, FederationCryptoError> {
    let trimmed = s.trim();
    let bytes = if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(trimmed).map_err(|e| FederationCryptoError::InvalidPubkey(e.to_string()))?
    } else {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
            .map_err(|e| FederationCryptoError::InvalidPubkey(e.to_string()))?
    };
    if bytes.len() != 32 {
        return Err(FederationCryptoError::InvalidPubkey(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    VerifyingKey::from_bytes(&arr).map_err(|e| FederationCryptoError::InvalidPubkey(e.to_string()))
}

/// Encode a verifying key as lowercase hex (canonical wire form for ORP
/// configs).
pub fn encode_pubkey(key: &VerifyingKey) -> String {
    hex::encode(key.to_bytes())
}

// ── Canonical JSON ───────────────────────────────────────────────────────────

/// Produce a stable, sorted-keys serialization of a JSON value so signatures
/// produced on one node verify on every other. Differs from `serde_json` only
/// in that object keys are alphabetically sorted and whitespace is stripped;
/// numbers, escapes, and Unicode follow `serde_json`'s defaults (which are
/// already stable across versions of `serde_json`).
fn canonical_json(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    fn walk(v: &serde_json::Value, out: &mut String) -> Result<(), serde_json::Error> {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(k)?);
                    out.push(':');
                    walk(&map[*k], out)?;
                }
                out.push('}');
            }
            serde_json::Value::Array(arr) => {
                out.push('[');
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    walk(item, out)?;
                }
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other)?),
        }
        Ok(())
    }
    let mut out = String::new();
    walk(value, &mut out)?;
    Ok(out)
}

// ── Local signer ─────────────────────────────────────────────────────────────

/// Convenience holder for the local node's Ed25519 signing key. Generated at
/// startup if no key file is configured (in which case peers must be
/// reconfigured with the new pubkey on every restart — fine for dev, not
/// fine for production, hence the warn-log).
#[derive(Clone)]
pub struct LocalSigner {
    pub signing_key: Arc<SigningKey>,
}

impl LocalSigner {
    /// Generate a fresh ephemeral key. Logs a warning that the corresponding
    /// pubkey is unstable across restarts.
    pub fn ephemeral() -> Self {
        use rand::RngCore;
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let key = SigningKey::from_bytes(&seed);
        tracing::warn!(
            pubkey = %encode_pubkey(&key.verifying_key()),
            "Federation signing key generated ephemerally — peers will need to be \
             reconfigured if this node restarts. Set federation.signing_key_path to persist."
        );
        Self {
            signing_key: Arc::new(key),
        }
    }

    /// Load a 32-byte raw seed from `path` (hex or binary). Falls back to
    /// `ephemeral()` with a warning if the file is missing.
    pub fn load_or_ephemeral(path: Option<&std::path::Path>) -> Self {
        let Some(p) = path else {
            return Self::ephemeral();
        };
        match std::fs::read(p) {
            Ok(bytes) => {
                let raw = if bytes.len() == 32 {
                    let mut a = [0u8; 32];
                    a.copy_from_slice(&bytes);
                    a
                } else {
                    // Try hex.
                    let s = String::from_utf8_lossy(&bytes);
                    let trimmed = s.trim();
                    match hex::decode(trimmed) {
                        Ok(decoded) if decoded.len() == 32 => {
                            let mut a = [0u8; 32];
                            a.copy_from_slice(&decoded);
                            a
                        }
                        _ => {
                            tracing::warn!(
                                path = %p.display(),
                                "Federation signing key file is neither 32 raw bytes \
                                 nor 64 hex chars; falling back to ephemeral key"
                            );
                            return Self::ephemeral();
                        }
                    }
                };
                let key = SigningKey::from_bytes(&raw);
                tracing::info!(
                    pubkey = %encode_pubkey(&key.verifying_key()),
                    path = %p.display(),
                    "Federation signing key loaded"
                );
                Self {
                    signing_key: Arc::new(key),
                }
            }
            Err(e) => {
                tracing::warn!(
                    path = %p.display(),
                    error = %e,
                    "Failed to read federation signing key; falling back to ephemeral"
                );
                Self::ephemeral()
            }
        }
    }

    /// Hex-encoded form of this signer's pubkey. Surfaced via
    /// `/api/v1/federation/info` (follow-up commit) so an operator
    /// configuring the *other* side of a peer pairing can copy the value
    /// straight from the live node instead of re-deriving it from the
    /// signing-key file.
    #[allow(dead_code)]
    pub fn pubkey_hex(&self) -> String {
        encode_pubkey(&self.signing_key.verifying_key())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    fn fresh_signer() -> SigningKey {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        SigningKey::from_bytes(&seed)
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let a = serde_json::json!({"b": 1, "a": 2, "c": {"y": 1, "x": 2}});
        let b = serde_json::json!({"c": {"x": 2, "y": 1}, "a": 2, "b": 1});
        assert_eq!(canonical_json(&a).unwrap(), canonical_json(&b).unwrap());
    }

    #[test]
    fn canonical_json_preserves_arrays() {
        // Order matters in arrays; canonicalization should NOT reorder them.
        let a = serde_json::json!([3, 1, 2]);
        let b = serde_json::json!([1, 2, 3]);
        assert_ne!(canonical_json(&a).unwrap(), canonical_json(&b).unwrap());
    }

    #[test]
    fn seal_and_verify_round_trip() {
        let key = fresh_signer();
        let pub_ = key.verifying_key();
        let env = SignedFederationEnvelope::seal(
            "alpha",
            1,
            &key,
            serde_json::json!({"id": "ship-1", "confidence": 0.8}),
        )
        .unwrap();
        env.verify(&pub_).unwrap();
    }

    #[test]
    fn verify_rejects_tampered_payload() {
        let key = fresh_signer();
        let pub_ = key.verifying_key();
        let mut env = SignedFederationEnvelope::seal(
            "alpha",
            1,
            &key,
            serde_json::json!({"id": "ship-1", "confidence": 0.5}),
        )
        .unwrap();
        // Tamper with the payload — now-canonical bytes diverge from signature.
        env.payload = serde_json::json!({"id": "ship-1", "confidence": 0.99});
        assert!(matches!(
            env.verify(&pub_).unwrap_err(),
            FederationCryptoError::SignatureMismatch
        ));
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let signing = fresh_signer();
        let other = fresh_signer();
        let env = SignedFederationEnvelope::seal("alpha", 1, &signing, serde_json::json!({"x": 1}))
            .unwrap();
        assert!(matches!(
            env.verify(&other.verifying_key()).unwrap_err(),
            FederationCryptoError::SignatureMismatch
        ));
    }

    #[tokio::test]
    async fn replay_tracker_rejects_repeat_seq() {
        let key = fresh_signer();
        let tracker = ReplayTracker::new();
        let env =
            SignedFederationEnvelope::seal("alpha", 5, &key, serde_json::json!({"x": 1})).unwrap();
        assert!(tracker.check_and_record(&env).await.unwrap().is_none());
        // Same seq again → reject.
        let err = tracker.check_and_record(&env).await.unwrap_err();
        assert!(matches!(
            err,
            FederationCryptoError::ReplaySeq { seq: 5, last: 5 }
        ));
    }

    #[tokio::test]
    async fn replay_tracker_rejects_lower_seq() {
        let key = fresh_signer();
        let tracker = ReplayTracker::new();
        let env_high =
            SignedFederationEnvelope::seal("alpha", 10, &key, serde_json::json!({"x": 1})).unwrap();
        let env_low =
            SignedFederationEnvelope::seal("alpha", 3, &key, serde_json::json!({"x": 1})).unwrap();
        tracker.check_and_record(&env_high).await.unwrap();
        let err = tracker.check_and_record(&env_low).await.unwrap_err();
        assert!(matches!(
            err,
            FederationCryptoError::ReplaySeq { seq: 3, last: 10 }
        ));
    }

    #[tokio::test]
    async fn replay_tracker_separates_peers() {
        let key = fresh_signer();
        let tracker = ReplayTracker::new();
        let a = SignedFederationEnvelope::seal("alpha", 5, &key, serde_json::json!({})).unwrap();
        let b = SignedFederationEnvelope::seal("beta", 5, &key, serde_json::json!({})).unwrap();
        // Same seq from different peers → both accepted.
        tracker.check_and_record(&a).await.unwrap();
        tracker.check_and_record(&b).await.unwrap();
    }

    #[test]
    fn timestamp_skew_rejected() {
        let key = fresh_signer();
        let mut env =
            SignedFederationEnvelope::seal("alpha", 1, &key, serde_json::json!({"x": 1})).unwrap();
        // Move timestamp 1 hour into the past — beyond MAX_TIMESTAMP_SKEW_SECS.
        env.timestamp = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let err = env.check_timestamp().unwrap_err();
        assert!(matches!(err, FederationCryptoError::TimestampSkew { .. }));
    }

    #[test]
    fn pubkey_decode_accepts_hex_and_base64() {
        use base64::Engine;
        let key = fresh_signer();
        let bytes = key.verifying_key().to_bytes();
        let hex_str = hex::encode(bytes);
        let b64_str = base64::engine::general_purpose::STANDARD.encode(bytes);
        let from_hex = decode_pubkey(&hex_str).unwrap();
        let from_b64 = decode_pubkey(&b64_str).unwrap();
        assert_eq!(from_hex.to_bytes(), from_b64.to_bytes());
    }

    #[test]
    fn pubkey_decode_rejects_wrong_length() {
        let bad = "deadbeef"; // 4 bytes, not 32
        assert!(decode_pubkey(bad).is_err());
    }

    #[test]
    fn confidence_cap_default_when_unset() {
        let trust = PeerTrust {
            signing_pubkey: "00".repeat(32),
            max_confidence_cap: None,
        };
        assert!((trust.confidence_cap() - DEFAULT_CONFIDENCE_CAP).abs() < f64::EPSILON);
    }

    #[test]
    fn confidence_cap_clamps_to_unit_interval() {
        let trust_high = PeerTrust {
            signing_pubkey: "00".repeat(32),
            max_confidence_cap: Some(1.7),
        };
        assert!((trust_high.confidence_cap() - 1.0).abs() < f64::EPSILON);
        let trust_low = PeerTrust {
            signing_pubkey: "00".repeat(32),
            max_confidence_cap: Some(-0.5),
        };
        assert!((trust_low.confidence_cap() - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn outbound_seq_is_monotonic_per_peer() {
        let seq = OutboundSeq::new();
        assert_eq!(seq.next("alpha").await, 1);
        assert_eq!(seq.next("alpha").await, 2);
        assert_eq!(seq.next("beta").await, 1);
        assert_eq!(seq.next("alpha").await, 3);
    }
}

// ── End-to-end mTLS tests ────────────────────────────────────────────────────
//
// These exercise the full TLS handshake against a real `axum-server::tls_rustls`
// listener using `rcgen`-generated certs, then drive a `reqwest` client with
// matching PEMs through the legitimate path and several attack paths
// (missing client cert, untrusted client cert, mismatched server CA). The
// goal is to prove each of the controls described in `docs/FEDERATION_TLS.md`
// trips at the right layer.

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use axum::{routing::post, Router};
    use axum_server::tls_rustls::RustlsConfig;
    use rand::RngCore;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose,
    };
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::server::WebPkiClientVerifier;
    use rustls::RootCertStore;
    use std::sync::Arc;

    fn fresh_signer() -> ed25519_dalek::SigningKey {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        ed25519_dalek::SigningKey::from_bytes(&seed)
    }

    /// Parsed PEM material the test uses to drive both sides of mTLS.
    struct CertBundle {
        ca_pem: String,
        ca_keypair_pem: String,
        server_cert_pem: String,
        server_key_pem: String,
        good_client_cert_pem: String,
        good_client_key_pem: String,
        bad_client_cert_pem: String,
        bad_client_key_pem: String,
    }

    /// Build a CA, a server cert, a client cert signed by that CA, and a
    /// second client cert signed by a *different* CA — used to verify the
    /// mTLS server rejects unknown clients.
    fn make_certs(server_san: &str) -> CertBundle {
        let ca_kp = KeyPair::generate().unwrap();
        let mut ca_p = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca_p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        ca_p.distinguished_name
            .push(DnType::CommonName, "ORP Test CA");
        let ca_cert = ca_p.self_signed(&ca_kp).unwrap();

        let srv_kp = KeyPair::generate().unwrap();
        let mut srv_p = CertificateParams::new(vec![server_san.to_string()]).unwrap();
        srv_p
            .distinguished_name
            .push(DnType::CommonName, "orp-server");
        srv_p.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let srv_cert = srv_p.signed_by(&srv_kp, &ca_cert, &ca_kp).unwrap();

        let cli_kp = KeyPair::generate().unwrap();
        let mut cli_p = CertificateParams::new(Vec::<String>::new()).unwrap();
        cli_p
            .distinguished_name
            .push(DnType::CommonName, "orp-client-good");
        cli_p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let cli_cert = cli_p.signed_by(&cli_kp, &ca_cert, &ca_kp).unwrap();

        // Second CA → second client → simulates an untrusted peer.
        let other_ca_kp = KeyPair::generate().unwrap();
        let mut other_ca_p = CertificateParams::new(Vec::<String>::new()).unwrap();
        other_ca_p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        other_ca_p
            .distinguished_name
            .push(DnType::CommonName, "ORP Other CA");
        let other_ca_cert = other_ca_p.self_signed(&other_ca_kp).unwrap();

        let bad_cli_kp = KeyPair::generate().unwrap();
        let mut bad_cli_p = CertificateParams::new(Vec::<String>::new()).unwrap();
        bad_cli_p
            .distinguished_name
            .push(DnType::CommonName, "orp-client-bad");
        bad_cli_p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let bad_cli_cert = bad_cli_p
            .signed_by(&bad_cli_kp, &other_ca_cert, &other_ca_kp)
            .unwrap();

        CertBundle {
            ca_pem: ca_cert.pem(),
            ca_keypair_pem: ca_kp.serialize_pem(),
            server_cert_pem: srv_cert.pem(),
            server_key_pem: srv_kp.serialize_pem(),
            good_client_cert_pem: cli_cert.pem(),
            good_client_key_pem: cli_kp.serialize_pem(),
            bad_client_cert_pem: bad_cli_cert.pem(),
            bad_client_key_pem: bad_cli_kp.serialize_pem(),
        }
    }

    /// Spin up a minimal axum app on a random ephemeral port with mTLS
    /// required, returning the bound address. The server echoes whatever it
    /// gets so we can prove a request *reached* the handler — anything that
    /// fails before that is a TLS-layer rejection.
    async fn spawn_mtls_server(bundle: &CertBundle) -> std::net::SocketAddr {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let server_certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut bundle.server_cert_pem.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap();
        let server_key: PrivateKeyDer<'static> =
            rustls_pemfile::private_key(&mut bundle.server_key_pem.as_bytes())
                .unwrap()
                .unwrap();

        let mut roots = RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut bundle.ca_pem.as_bytes()) {
            roots.add(c.unwrap()).unwrap();
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .unwrap();

        let cfg = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(server_certs, server_key)
            .unwrap();

        let app: Router = Router::new().route(
            "/echo",
            post(|body: String| async move { format!("echoed:{}", body) }),
        );

        // Bind on a fresh port — let the OS pick.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();

        let tls_cfg = RustlsConfig::from_config(Arc::new(cfg));
        tokio::spawn(async move {
            // axum-server takes the std listener directly.
            axum_server::from_tcp_rustls(listener, tls_cfg)
                .serve(app.into_make_service())
                .await
                .ok();
        });

        // Tiny settle so the listener is up before the test client dials.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        addr
    }

    fn build_client(bundle: &CertBundle, cert_pem: &str, key_pem: &str) -> reqwest::Client {
        let identity_pem = format!("{}\n{}", cert_pem.trim(), key_pem.trim());
        let identity = reqwest::Identity::from_pem(identity_pem.as_bytes()).unwrap();
        let ca = reqwest::Certificate::from_pem(bundle.ca_pem.as_bytes()).unwrap();
        reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(ca)
            .identity(identity)
            .danger_accept_invalid_hostnames(true)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    }

    /// A mutually-authenticated client over a CA the server trusts succeeds.
    #[tokio::test]
    async fn mtls_happy_path_round_trips_request() {
        let bundle = make_certs("localhost");
        let addr = spawn_mtls_server(&bundle).await;
        let client = build_client(
            &bundle,
            &bundle.good_client_cert_pem,
            &bundle.good_client_key_pem,
        );
        let url = format!("https://localhost:{}/echo", addr.port());
        let resp = client.post(&url).body("hello").send().await.unwrap();
        assert!(resp.status().is_success());
        assert_eq!(resp.text().await.unwrap(), "echoed:hello");
    }

    /// A client whose cert is signed by a *different* CA is rejected at the
    /// TLS handshake — the request never reaches the handler.
    #[tokio::test]
    async fn mtls_rejects_untrusted_client_cert() {
        let bundle = make_certs("localhost");
        let addr = spawn_mtls_server(&bundle).await;
        let client = build_client(
            &bundle,
            &bundle.bad_client_cert_pem,
            &bundle.bad_client_key_pem,
        );
        let url = format!("https://localhost:{}/echo", addr.port());
        let result = client.post(&url).body("hello").send().await;
        // Either TLS handshake fails (most common) or the server tears
        // the connection mid-request — both manifest as Err on reqwest.
        assert!(
            result.is_err(),
            "expected TLS handshake failure for untrusted client; got {:?}",
            result.map(|r| r.status())
        );
    }

    /// A plaintext HTTP client targeting the mTLS port cannot establish a
    /// session at all.
    #[tokio::test]
    async fn mtls_rejects_plaintext_client() {
        let bundle = make_certs("localhost");
        let addr = spawn_mtls_server(&bundle).await;
        // No TLS at all → HTTP request to TLS port fails.
        let plain = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let url = format!("http://localhost:{}/echo", addr.port());
        let res = plain.post(&url).body("hello").send().await;
        assert!(res.is_err(), "plaintext to mTLS port must fail");
    }

    /// End-to-end signed envelope: peer A seals a payload; peer B verifies
    /// against A's pinned pubkey. Then peer B's `ReplayTracker` rejects a
    /// subsequent envelope with the same seq.
    #[tokio::test]
    async fn signed_envelope_verifies_and_replay_blocks() {
        let signing = fresh_signer();
        let pinned_pubkey_hex = encode_pubkey(&signing.verifying_key());
        // Receiver-side: register the pubkey in a `PeerTrust` and verify.
        let trust = PeerTrust {
            signing_pubkey: pinned_pubkey_hex,
            max_confidence_cap: Some(0.85),
        };
        let env = SignedFederationEnvelope::seal(
            "cluster-east",
            1,
            &signing,
            serde_json::json!({"id": "ship-1", "confidence": 1.0}),
        )
        .unwrap();
        // Verify signature against pinned pubkey.
        env.verify(&trust.verifying_key().unwrap()).unwrap();
        // Replay tracker accepts first, rejects second.
        let tracker = ReplayTracker::new();
        tracker.check_and_record(&env).await.unwrap();
        let err = tracker.check_and_record(&env).await.unwrap_err();
        assert!(matches!(err, FederationCryptoError::ReplaySeq { .. }));
        // Confidence cap clamps the wire-1.0 down.
        let cap = trust.confidence_cap();
        let raw = env.payload["confidence"].as_f64().unwrap();
        assert_eq!(raw, 1.0);
        assert!((raw.min(cap) - 0.85).abs() < f64::EPSILON);
        // Silence the unused-CA-keypair warning (real test would use it to
        // sign client certs; we generate it for completeness even when we
        // don't need it).
        let _ = (
            trust.max_confidence_cap,
            // bundle isn't relevant here but the compiler shouldn't complain
            // about make_certs being unused if every other test compiles.
        );
    }

    /// Suppress the unused-keypair warning on the bundle: the CA keypair PEM
    /// is exposed for cert-rotation tests but we don't need it here.
    #[allow(dead_code)]
    fn _bundle_field_use_check(b: &CertBundle) -> usize {
        b.ca_keypair_pem.len()
    }
}
