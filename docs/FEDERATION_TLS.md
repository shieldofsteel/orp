# Federation Transport Security (mTLS + Ed25519 Signing)

ORP federates entity data across instances. By default that traffic flowed
over plain HTTP with **no transport encryption, no peer authentication, and
no payload signing** — any host that could reach the federation port could
spoof a peer or override local truth by sending `confidence = 1.0` for every
entity. From v0.3.0, federation can be locked down with:

| Layer            | Mechanism                                                   |
| ---------------- | ----------------------------------------------------------- |
| Channel          | mTLS (rustls, no native-tls)                                |
| Peer auth        | Pinned client certificate signed by your federation CA      |
| Payload integrity| Ed25519 signed envelope (`SignedFederationEnvelope`)        |
| Replay resistance| Per-peer monotonic `seq` + ±5 min timestamp window          |
| Trust override   | Per-peer `max_confidence_cap` (default 0.9)                 |

Every layer is opt-in (`federation.tls.enabled = false` by default) so an
upgrading v0.2.0 cluster does not break overnight; once every node is on
v0.3.0+, flip the switch.

## TL;DR — quickstart

```bash
# 1. Generate a CA, server cert, and client cert per peer.
./scripts/gen-federation-certs.sh   # see below for the script

# 2. Generate a stable Ed25519 signing key per node.
openssl genpkey -algorithm ed25519 -out node-east.key.pem
openssl pkey -in node-east.key.pem -pubout -out node-east.pub.pem
# Extract the 32-byte pubkey as hex (what ORP wants):
openssl pkey -in node-east.key.pem -pubout -outform DER \
  | tail -c 32 | xxd -p -c 64
# → e.g. 9b6a3c…  (paste into the peer's "signing_pubkey" field)

# 3. Start ORP on cluster-east with mTLS:
orp start \
    --federation-tls \
    --federation-cert /etc/orp/east.crt.pem \
    --federation-key  /etc/orp/east.key.pem \
    --federation-ca   /etc/orp/fed-ca.crt.pem \
    --federation-signing-key /etc/orp/node-east.key.pem \
    --node-id cluster-east

# 4. On cluster-west, register cluster-east as a peer:
curl -sS -X POST https://west:9090/api/v1/peers \
  -H 'Authorization: Bearer …' \
  -d '{
    "id": "cluster-east",
    "host": "east.example.com",
    "port": 9443,
    "shared_entity_types": ["ship", "aircraft"],
    "trust": {
      "signing_pubkey": "9b6a3c…",
      "max_confidence_cap": 0.85
    }
  }'
```

## Threat model and what each control buys you

### 1. Channel encryption + peer authentication — mTLS

A dedicated `axum-server::tls_rustls` listener (default `0.0.0.0:9443`)
demands a client certificate on every TCP connection. The TLS handshake
fails with `unknown CA` for any client whose cert is not signed by the CA
listed in `--federation-ca`. **No HTTP frames are exchanged with an
unauthenticated client.**

The non-TLS port (`--port`, default `9090`) continues to serve the frontend
and ABAC-gated REST so existing dashboards keep working — only the
`/federation/push` path is bound to the TLS listener.

### 2. Payload integrity — Ed25519 envelope

mTLS proves *the channel*, not *the message*. If a load balancer terminates
TLS upstream of ORP, a compromised LB could forge entity pushes. To defend
against that, every push is wrapped in a `SignedFederationEnvelope`:

```json
{
  "sender": "cluster-east",
  "seq": 4711,
  "timestamp": "2026-05-01T12:34:56Z",
  "payload": { "id": "ship-123", "confidence": 0.8, … },
  "signature": "<128 hex chars = 64-byte Ed25519 sig>"
}
```

The signature covers the canonical-JSON serialization of `(timestamp ||
sender || seq || payload)`. The receiver verifies against the pinned
`signing_pubkey` registered for the sending peer; **if the operator never
configured a pubkey, signed pushes are refused with 401**.

### 3. Replay resistance — `seq` + clock skew

Each sender holds a per-receiver counter (`OutboundSeq`) that increments on
every push. The receiver tracks the highest `seq` seen per sender
(`ReplayTracker`) and rejects any envelope whose `seq` is `<= last_seen`.
Combined with a ±5 minute timestamp window, this defeats replay attacks even
if an attacker captures one TLS session.

The replay tracker is in-memory, so a process restart resets the high-water
mark to zero. The 5-minute timestamp window bounds the replay surface to
restart-window-sized; envelopes from before the window fail
`check_timestamp()` regardless of seq.

### 4. Trust-override prevention — confidence cap

Federation conflict resolution is "highest confidence wins". Without a cap,
a single compromised peer sending `confidence = 1.0` for every entity could
override every local observation. Each peer carries
`max_confidence_cap` (default 0.9): incoming confidence is clamped to
`min(incoming, cap, 1.0)`. A peer-sourced entity therefore can never beat
a perfectly-trusted local observation at 1.0.

## Generating certificates with rcgen (Rust)

ORP ships with `rcgen` as a workspace dependency. The simplest production
setup is a self-managed offline CA + per-node client certs.

```rust
// scripts/gen-fed-certs.rs (sketch)
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, IsCa, KeyPair,
    KeyUsagePurpose, ExtendedKeyUsagePurpose, DnType,
};

fn main() -> anyhow::Result<()> {
    // 1. Root CA — keep this offline.
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(vec![])?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    ca_params.distinguished_name.push(DnType::CommonName, "ORP Federation CA");
    let ca_cert = ca_params.self_signed(&ca_key)?;
    std::fs::write("fed-ca.crt.pem", ca_cert.pem())?;
    std::fs::write("fed-ca.key.pem", ca_key.serialize_pem())?;

    // 2. One server cert per node. The SAN must match the hostname peers
    //    will use to reach this node — IP literals are fine if you
    //    federate over private networks.
    let east_key = KeyPair::generate()?;
    let mut east_params = CertificateParams::new(vec![
        "east.example.com".to_string(),
        "10.0.0.10".to_string(),
    ])?;
    east_params.distinguished_name.push(DnType::CommonName, "cluster-east");
    east_params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let east_cert = east_params.signed_by(&east_key, &ca_cert, &ca_key)?;
    std::fs::write("east.crt.pem", east_cert.pem())?;
    std::fs::write("east.key.pem", east_key.serialize_pem())?;

    Ok(())
}
```

## Generating certificates with OpenSSL (no Rust required)

```bash
# 1. Federation CA (one per fleet — keep offline).
openssl req -x509 -newkey ed25519 -days 3650 \
    -nodes -keyout fed-ca.key.pem -out fed-ca.crt.pem \
    -subj '/CN=ORP Federation CA'

# 2. Server cert for cluster-east.
openssl req -newkey ed25519 -nodes \
    -keyout east.key.pem -out east.csr.pem \
    -subj '/CN=cluster-east' \
    -addext "subjectAltName=DNS:east.example.com,IP:10.0.0.10" \
    -addext "extendedKeyUsage=serverAuth,clientAuth"

openssl x509 -req -in east.csr.pem -CA fed-ca.crt.pem -CAkey fed-ca.key.pem \
    -CAcreateserial -days 730 \
    -copy_extensions copy \
    -out east.crt.pem

# Repeat step 2 for every other peer with their own CN/SAN.
```

`extendedKeyUsage=serverAuth,clientAuth` matters: the same cert is presented
both ways during mTLS, so it has to be valid for both roles.

## Generating a stable Ed25519 signing key

```bash
# Raw 32-byte seed (preferred — what ORP loads natively):
head -c 32 /dev/urandom > /etc/orp/node-east.seed
chmod 0600 /etc/orp/node-east.seed

# Or hex (also accepted — auto-detected by length):
head -c 32 /dev/urandom | xxd -p -c 64 > /etc/orp/node-east.seed.hex
```

Pass either form to `--federation-signing-key`. The corresponding pubkey is
logged at startup:

```
INFO Federation signing key loaded path=/etc/orp/node-east.seed
     pubkey=9b6a3c5e7f...
```

Copy that hex string into the peer's `trust.signing_pubkey` when registering
this node on the other side.

## Configuration reference

### CLI flags (on `orp start`)

| Flag                             | Env var                | Effect                                        |
| -------------------------------- | ---------------------- | --------------------------------------------- |
| `--federation-tls`               | `ORP_FED_TLS`          | Enable the mTLS listener                      |
| `--federation-cert <PATH>`       | `ORP_FED_CERT`         | Server cert (PEM)                             |
| `--federation-key <PATH>`        | `ORP_FED_KEY`          | Server private key (PEM)                      |
| `--federation-ca <PATH>`         | `ORP_FED_CA`           | CA cert used to verify connecting peers (PEM) |
| `--federation-tls-listen <ADDR>` | `ORP_FED_TLS_LISTEN`   | Bind addr (default `0.0.0.0:9443`)            |
| `--federation-signing-key <PATH>`| `ORP_FED_SIGNING_KEY`  | Ed25519 32-byte seed (raw or hex)             |
| `--node-id <ID>`                 | `ORP_NODE_ID`          | Stable identity for envelope `sender`         |

### Per-peer trust block

```json
{
  "id": "cluster-east",
  "host": "east.example.com",
  "port": 9443,
  "shared_entity_types": ["ship"],
  "trust": {
    "signing_pubkey": "9b6a3c…",
    "max_confidence_cap": 0.85
  }
}
```

* Setting `trust` automatically switches outbound traffic to `https://` and
  the `/federation/push` envelope path.
* Omitting `trust` keeps the v0.2.0 plaintext flow for that single peer —
  useful for staged rollouts.

## Failure modes you should expect

| Symptom                                              | Likely cause                                        |
| ---------------------------------------------------- | --------------------------------------------------- |
| `401 BAD_SIGNATURE`                                  | Wrong `signing_pubkey`, or the sender re-keyed      |
| `401 REPLAY_REJECTED` after sender restart           | Sender lost its outbound counter; restart receiver  |
| `401 TRUST_REQUIRED`                                 | Peer has `trust=None` but a signed push arrived     |
| TLS handshake fails: `unknown CA`                    | `--federation-ca` is wrong or the cert is unsigned  |
| `WARN federation.tls.enabled=true but cert/key/ca …` | One of the three paths is missing                   |

## Testing

The crypto and replay logic ship with unit tests
(`crates/orp-core/src/server/federation_tls.rs`):

```bash
cargo test -p orp-core --bin orp federation_tls::
```

End-to-end verification against a real `axum-server::tls_rustls` listener
lives in the same binary's integration tests and runs in CI.
