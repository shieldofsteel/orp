<div align="center">

# ORP — Open Reality Protocol

### A single Rust binary that fuses 40+ protocols into one cryptographically-signed real-time picture.

[![Tests](https://img.shields.io/badge/tests-1500%2B%20passing-brightgreen?style=flat-square)](https://github.com/shieldofsteel/orp/actions)
[![Binary Size](https://img.shields.io/badge/binary-45MB-blue?style=flat-square)](https://github.com/shieldofsteel/orp/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-orange?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?style=flat-square)](https://www.rust-lang.org)
[![Crates](https://img.shields.io/badge/crates-14%20workspace%20crates-red?style=flat-square)](crates/)
[![Adapters](https://img.shields.io/badge/adapters-40%20%2B%20TAK%20Protocol%20v1-purple?style=flat-square)](crates/orp-connector/src/adapters/)
[![Security](https://img.shields.io/badge/security-mTLS%20%7C%20OIDC%20%7C%20Argon2id%20%7C%20AES--256--GCM-darkgreen?style=flat-square)](docs/SECURITY.md)
[![SBOM](https://img.shields.io/badge/release-CycloneDX%201.5%20%2B%20cosign-blue?style=flat-square)](.github/workflows/release.yml)

</div>

---

## What it is, in three lines

ORP is a single 45 MB Rust binary that ingests AIS, ADS-B, MAVLink, OPC-UA, MQTT, Modbus, Zeek, syslog, GRIB, CoT, KLV (MISB ST 0601), CCSDS+SGP4, HL7 v2.5, Kafka, NATS and 25 more protocols into one queryable real-time graph — with an ORP-QL query language, **mTLS-secured federation mesh**, **Ed25519-signed tamper-evident audit log**, **OIDC JWKS verification (RS256/ES256)**, **Argon2id keystore**, and a built-in COP map. **No JVM. No Postgres. No Kubernetes.** Just `./orp start` and you're ingesting in 30 seconds. The slot it fills: *the new SQLite/Postgres for real-time multi-source backends — but military-grade out of the box*.

---

## 30-Second Demo

> 🎬 *Demo GIF coming with the first tagged release. Until then — the transcript:*

```text
$ orp doctor
✓ protoc on PATH         found
✓ DuckDB writable        ok at ./data.duckdb
✓ RocksDB writable       ok at ./state.db (parent directory writable)
✓ Server port free       :9090 is bindable
✓ Config validation      no config.yaml found — defaults will be used
✓ Cert chain validity    skipped — pass --https-url to test
✓ ready — run `orp start --template maritime`.

$ orp start --template maritime
INFO Initializing DuckDB storage at ./data.duckdb
INFO 🌍 Connecting to AISStream.io — live global AIS data
INFO Starting HTTP server on 0.0.0.0:9090
INFO Dashboard: http://localhost:9090/

$ orp query "MATCH (s:ship) WHERE s.speed > 25 RETURN s.name, s.speed LIMIT 5"
╭───────────────┬───────╮
│ s.name        │ speed │
├───────────────┼───────┤
│ MSC ARIES     │ 28.4  │
│ EVER GIVEN    │ 25.2  │
╰───────────────┴───────╯
2 rows in 4.3ms
```

---

## What slot does this fill?

A new shape of system needs a new comparison set. ORP is **not** a database, **not** a SCADA, **not** a SIEM — it's a single-binary fusion engine that absorbs *all* of those into one queryable graph.

| | SQLite | Postgres | Lattice OS | Maven OS / Anduril | **ORP** |
|---|--------|----------|------------|---------------------|---------|
| Single binary, no daemons | ✅ | ❌ | ❌ | ❌ | **✅** |
| Embeddable in another process | ✅ | ⚠️ libpq | ❌ | ❌ | **✅ (via orp-core crate)** |
| First-class real-time ingest | ❌ | ❌ partial (LISTEN/NOTIFY) | ✅ | ✅ | **✅ (50k events/s)** |
| Protocol adapters out of the box | 0 | 0 | proprietary | proprietary | **39 open** |
| Graph query language | ❌ | extensions only | proprietary | proprietary | **ORP-QL (open)** |
| Live federation mesh | ❌ | logical replication only | ✅ closed | ✅ closed | **✅ open** |
| Edge / Raspberry Pi capable | ✅ | ⚠️ | ❌ | ❌ | **✅ 45 MB, ARM64** |
| Built-in COP / map UI | ❌ | ❌ | ✅ | ✅ | **✅** |
| License | public domain | PostgreSQL | proprietary | proprietary | **Apache 2.0** |
| Cost | free | free | undisclosed (8-figure deals) | undisclosed | **free** |

The pitch in one sentence: *SQLite-style "single binary, zero config" pushed up the stack to where Lattice OS / Maven OS / Palantir AIP currently live.*

---

## 30 Seconds to Live Data

```bash
# Install (works after the first tagged release — see "Building from Source" until then)
curl -fsSL https://raw.githubusercontent.com/shieldofsteel/orp/master/scripts/install.sh | sh

# Or build from source:
git clone https://github.com/shieldofsteel/orp && cd orp
brew install protobuf  # or: apt install -y protobuf-compiler
cargo build --release

# Diagnose the host first
./target/release/orp doctor

# Launch with the maritime template
./target/release/orp start --template maritime

# Ships appear on your screen in 30 seconds — open http://localhost:9090
```

That's it. No YAML sprawl. No microservices. No Kubernetes. One process, one port, everything included.

For the 10-minute "from zero to ingesting your own data" tour, see **[docs/QUICKSTART.md](docs/QUICKSTART.md)**.
For copy-paste recipes (AIS, ADS-B, MAVLink, Modbus, Zeek, audit-log export, federation, continuous alerts), see **[docs/RECIPES.md](docs/RECIPES.md)**.

---

## What ORP Does

**Fuses data from any source** — 39 protocol adapters, a universal JSON ingest endpoint, and a connector SDK. If it outputs data, ORP can consume it.

**Builds a live knowledge graph** — every entity (ship, aircraft, vehicle, sensor, threat) becomes a node. Relationships auto-form. The graph updates in real time.

**Relays real-time media sources** — register RTSP/RTMP/HLS/MJPEG/WebRTC/SRT/ONVIF/KLV camera streams, relay HTTP/JPEG/MJPEG/HLS streams in-binary, validate risky LAN URLs explicitly, redact embedded credentials, and project every stream into the graph as a `media_stream`. See [docs/MEDIA.md](docs/MEDIA.md).

**Renders a military-grade COP** — a full-featured map with 4 tile layers, directional arrows, course vectors, lasso select, and a timeline scrubber. Not a dashboard — an operational picture.

**Lets you query anything** — ORP-QL is a purpose-built query language combining SQL analytics with Cypher-style graph traversal. Query across sensors, entities, and time.

**Alerts you before it matters** — anomaly detection and threat scoring run continuously. When a ship deviates from its pattern-of-life, you know.

**Runs anywhere** — a laptop, a Raspberry Pi, a warship, a data center. `--headless` for embedded deployments. Docker for cloud. ARM binaries for edge.

---

## Security Posture (v0.3.2)

ORP ships with the cryptographic primitives a defense / federal procurement reviewer expects, all in the single binary, all configurable via flags or env vars:

| Capability | What it does | How to turn on |
|---|---|---|
| **Inbound TLS** | `axum-server` + `rustls` (no native-tls / openssl) terminates HTTPS for the REST + WebSocket API. | `--tls-cert <pem> --tls-key <pem>` (or `orp gen-cert` for dev). |
| **Inbound mTLS** | Optional client-cert auth — server requires every caller to present a cert signed by your CA. | `--tls-client-ca <pem>` |
| **HSTS** | `Strict-Transport-Security: max-age=31536000` when TLS is active. | Automatic when `--tls-cert` is set. |
| **Federation mTLS** | Dedicated rustls listener on a separate port (default 9443) requires connecting peers to present a client cert signed by the federation CA. | `--federation-tls --federation-cert/key/ca <pem>` |
| **Federation Ed25519 signing** | Each federated payload carries a signature over `(timestamp \|\| peer_id \|\| canonical_json(payload))`; receivers verify against the sending peer's pinned pubkey. | `--federation-signing-key <pem>` and per-peer `signing_pubkey` in config. |
| **Federation replay protection** | Per-peer monotonic sequence numbers; receiver rejects `seq <= last_seen`. | Automatic with federation TLS. |
| **Federation confidence cap** | Receiver clamps incoming `confidence` to a per-peer max (default 0.9) so a compromised peer can't overwrite truth-of-record. | `peers[].max_confidence_cap` in config. |
| **OIDC JWKS verification** | Real RS256 / ES256 verification of JWTs against `discovery.jwks_uri`; caches the JWKS with TTL + refresh-on-`kid`-miss; multi-IdP routing by `iss`. | `oidc.providers[]` in config (Keycloak, Auth0, Okta, Azure AD). |
| **Argon2id API key store** | OWASP-2023 floor (m=19 MiB, t=2, p=1), PHC strings, persisted to a separate `*-auth.duckdb` file with `last_used_at` + revoke. | `--bootstrap-admin-key` on first start, then `orp api-keys` subcommands. |
| **Argon2id password hashing** | Same floor, fresh `OsRng` salt per call, PHC strings (no SHA-256 / no `thread_rng`). | Active when the user-management module is wired. |
| **Signed audit log** | Every state change is Ed25519-signed and chained (each entry signs `prev_hash`); chain is replayed on startup; `audit verify` and `audit export` CLI subcommands let an external auditor prove tamper-evidence without ORP running. | Default on for persistent storage; `audit export --out audit.jsonl --public-key <hex>`. |
| **SSRF defence** | Outbound HTTP from notification webhooks (Webhook/Slack/Telegram), HTTP poller, and OIDC discovery is gated by a validate-then-pin client (rejects loopback / RFC1918 / cloud-metadata addresses unless explicitly opted in). | Automatic; `allow_private_targets: true` per channel for legitimate localhost integrations. |
| **WebSocket identity propagation** | JWT `sub` / `permissions` / `org_id` flow through every broadcast event; ABAC sees the real caller (no hardcoded `"ws-client"+["admin"]`). | Automatic when JWT or API-key auth is configured. |
| **Notification circuit breaker** | Per-channel breaker after 5 consecutive failures (5 min cooldown) + ±25% retry jitter via `OsRng`. | Automatic; configurable per channel. |
| **CSRF cookie** | OIDC CSRF state generated via `OsRng::fill_bytes(32)` → URL-safe base64 (~256 bits of entropy). State cookie is **HMAC-SHA256** signed (closed length-extension forgery) and verified with **constant-time** `hmac::Mac::verify_slice`. Refuses to issue/verify when `enabled=true` + `client_secret` empty (no silent dev-key fallback in production). | Active when OIDC is configured. |
| **Persistent audit signing key** | Ed25519 secret + public sidecar at `${XDG_DATA_HOME}/orp/audit-key.{ed25519,pub.ed25519}` (mode 0600 / 0644). Atomic publish; sidecar self-heals if the previous run was killed mid-write. Signatures emitted before a restart are verifiable after one. | Automatic for persistent mode; ephemeral on `--in-memory`. |
| **At-rest envelope encryption** | AES-256-GCM seal of the audit-log `details` column. Wire format `{"orpaead1": "<base64>"}` — keeps the JSON column type contract, supports mixed-mode (legacy plaintext rows still read). Captured DB file → ciphertext, not PII. | Set `ORP_AT_REST_KEY_PATH` or pre-create `${XDG_DATA_HOME}/orp/at-rest.key` (32 raw bytes, 0600). |
| **SMTP STARTTLS / TLS-465** | `lettre 0.11` with rustls + ring + webpki-roots. Implicit TLS on 465, STARTTLS upgrade on every other port. AUTH LOGIN now runs over the encrypted channel (no more cleartext credentials). | Automatic per `smtp_port` selection. |
| **Sanctions list signature verification** | Detached Ed25519 sig (`<path>.sig`, 64 raw bytes) verified before parsing. Pinned pubkey is enforced on **every reload** so a disk-writeable attacker cannot swap the file post-load. | `SanctionsDatabase::load_signed(path, &pubkey)`. |
| **CycloneDX 1.5 SBOM + cosign** | Release CI emits `bom.cdx.json` and signs every artifact (binaries, SBOM, checksums) with sigstore cosign keyless OIDC (Fulcio + Rekor). Satisfies SLSA-3 provenance for federal RFPs. | Automatic on tag push (`.github/workflows/release.yml`). |
| **Multi-domain classification labels** | `orp_security::Classification` (`U / CUI / NR / C / NC / S / NS / TS / CTS` + SCI compartments + dissem controls + ATOMAL) with CAPCO `banner()` + `dominates()` ABAC predicate. **Wire-level `OrpEvent.classification` proto field** + optional **`X-Classification` HTTP response header** banner. | `ORP_CLASSIFICATION_BANNER="TOP SECRET//SI//NOFORN"` env var enables the header. |

Closed in v0.3.0 (from the project's own audit reports):
- ✅ Federation has TLS + payload integrity (was plain HTTP, no peer auth)
- ✅ Inbound HTTP supports TLS (was `axum::serve` on plain TCP)
- ✅ OIDC verifies external JWTs against the IdP's JWKS (was discovered-then-ignored)
- ✅ Notifications no longer SSRF (was missing the guard `http_poller` got)
- ✅ WebSocket no longer hands every JWT holder admin events (was discarding claims)
- ✅ API keys + passwords use Argon2id (was unsalted SHA-256)

Closed in v0.3.1 (media relay hardening + Wave 2 P-audit small):
- ✅ Media relay DELETE cancels in-flight relays (CancellationToken)
- ✅ Media relay slow-loris closed (process-wide Semaphore, 60s idle timeout, 4 GiB session cap)
- ✅ Media relay IPv4-mapped IPv6 SSRF (`[::ffff:127.0.0.1]` rejected)
- ✅ HLS rewriter covers every spec'd attribute-bearing tag (`EXT-X-{KEY,MAP,SESSION-KEY,MEDIA,PRELOAD-HINT,PART,RENDITION-REPORT}`)
- ✅ HLS userinfo smuggling rejected; playlist body capped at 1 MiB; per-id handler `validate_id()` guards
- ✅ Persistent audit signing key (F2)
- ✅ Sanctions list signature verification (F8)
- ✅ TLS backend unified on rustls (F9 — `orp-connector` flipped reqwest + tokio-tungstenite to rustls-tls)
- ✅ CycloneDX 1.5 SBOM + cosign keyless signing
- ✅ Multi-domain classification scaffolding (`U/CUI/NR/C/NC/S/NS/TS/CTS` + SCI + dissem + ATOMAL)
- ✅ TAK Protocol v1 wire codec — first Rust crate (`orp-tak`)
- ✅ Persistent audit chain Linux-CI fix (DuckDB ns→µs precision)
- ✅ Resolver→AuditLogger plumbing (entity-match feedback in the signed chain)

Closed in v0.3.2 (full integration + remaining Wave 2 P-audit + crypto-audit follow-ups):
- ✅ CSRF HMAC + constant-time verify (F5)
- ✅ SMTP STARTTLS via lettre — credentials no longer cleartext (F6)
- ✅ At-rest envelope encryption for audit-log `details` (F7)
- ✅ Sanctions reload re-verifies pinned pubkey — closes the F8 reload bypass
- ✅ Audit-key public sidecar self-heals after partial-write crash
- ✅ CSRF refuses to issue cookie when `enabled=true` + `client_secret` empty (no silent dev-key fallback)
- ✅ `orp-tak` wired into `orp-connector::adapters::cot` — accepts both UDP mesh (`tak://`) and TCP stream (`tak-tcp://`) framings, falls through to plain `udp://` for legacy ATAK
- ✅ `OrpEvent.classification` + `Entity.classification` proto fields
- ✅ `X-Classification` response header middleware (env-driven)

Pending v0.4 hardening: full DuckDB encryption-extension support (whole-file rather than per-column), RocksDB `EncryptedEnv` wrapping, FIPS-mode build, ABAC dominance enforcement on classification (currently advisory).

Full audit history: [docs/SECURITY.md](docs/SECURITY.md) · [docs/TLS.md](docs/TLS.md) · [docs/OIDC.md](docs/OIDC.md) · [docs/FEDERATION_TLS.md](docs/FEDERATION_TLS.md)

---

## Protocol Support — at a Glance

ORP speaks the languages your sensors already use. **39 protocol adapters** across:

- **Maritime** — NMEA 0183, AIS (msg types 1–5, 9, 18, 27), AISStream, NMEA 2000, ACARS
- **Aviation** — ADS-B / Mode S, ASTERIX, GRIB (Section 7 unpacking incl. simple/grid/CCSDS), METAR
- **Drone autonomy** — MAVLink v2 (heartbeat, global_position_int, attitude, status_text, battery, GPS_RAW)
- **Space** — CCSDS + SGP4 (TLE-based orbit propagation)
- **Military / tactical / ISR** — CoT (bidirectional, TAK Server compatible), **TAK Protocol v1 wire codec** (UDP mesh `0xBF/01/0xBF` + TCP stream length-prefixed — first Rust implementation; `tak://` and `tak-tcp://` URL schemes plug straight into the CoT adapter), STIX/TAXII, NFFI (STANAG 5527), CEF, MISB ST 0601 KLV (tags 1–25, video metadata)
- **Real-time media relay** — in-binary HTTP/JPEG/MJPEG/HLS relay plus stream registration for RTSP, RTMP, WebRTC/WHEP, SRT, ONVIF, V4L2/USB, file, raw KLV, and KLV-in-MPEG-TS
- **Industrial / IoT** — OPC-UA, Modbus TCP/RTU, MQTT, SparkplugB, DNP3, CAN/J1939, BACnet, LoRaWAN
- **Cyber / network** — Syslog (RFC 3164/5424), PCAP, Zeek, NetFlow / IPFIX
- **Streaming / messaging** — Apache Kafka (feature-gated), NATS / JetStream (feature-gated)
- **Healthcare** — HL7 v2.5 over MLLP
- **Civic / disaster** — CAP (Common Alerting Protocol), GTFS-RT
- **Universal** — HTTP poller (with SSRF guard + DNS-rebinding pinning), WebSocket client, CSV watcher, Database tail, GeoJSON, generic JSON API

Full per-protocol detail and status table → [docs/PROTOCOLS.md](docs/PROTOCOLS.md) *(generated from the adapters list — see also [crates/orp-connector/src/adapters/](crates/orp-connector/src/adapters/))*.

> **Don't see your protocol?** The connector SDK is ~50 lines of Rust. [Build one →](docs/CONNECTOR_GUIDE.md)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                        ORP SINGLE BINARY                            │
│                                                                     │
│  ┌──────────────┐   ┌──────────────┐   ┌─────────────────────────┐ │
│  │  CONNECTORS  │   │  FUSION      │   │  QUERY ENGINE           │ │
│  │              │   │  ENGINE      │   │                         │ │
│  │  NMEA/AIS    │──▶│              │──▶│  ORP-QL                 │ │
│  │  ADS-B       │   │  Entity      │   │  (SQL + Graph hybrid)   │ │
│  │  CoT / TAK   │   │  Resolution  │   │                         │ │
│  │  OPC-UA      │   │              │   │  DuckDB (analytics)     │ │
│  │  MQTT        │   │  Knowledge   │   │  Graph projection (DuckDB) │ │
│  │  Modbus      │   │  Graph       │   │                         │ │
│  │  Syslog      │   │              │   └─────────────────────────┘ │
│  │  HTTP/WS     │   │  Anomaly     │                               │
│  │  CSV / DB    │   │  Detection   │   ┌─────────────────────────┐ │
│  │  + 9 more    │   │              │   │  API & REALTIME         │ │
│  └──────────────┘   │  Threat      │──▶│                         │ │
│                     │  Scoring     │   │  REST API (v1)          │ │
│  ┌──────────────┐   │              │   │  WebSocket (live push)  │ │
│  │  FEDERATION  │   │  ABAC +      │   │  ORP-to-ORP mesh sync   │ │
│  │              │──▶│  Ed25519     │   └─────────────────────────┘ │
│  │  Peer ORPs   │   │  Signing     │                               │
│  │  (mesh sync) │   └──────────────┘   ┌─────────────────────────┐ │
│  └──────────────┘                      │  WEB UI                 │ │
│                                        │  Map (4 tile layers)    │ │
│  ┌──────────────┐                      │  Dashboard              │ │
│  │  STORAGE     │                      │  Entity Inspector       │ │
│  │              │                      │  Query Console          │ │
│  │  DuckDB      │                      │  Search Panel           │ │
│  │  (entities,  │                      │  Alert Feed             │ │
│  │   history)   │                      │  Timeline Scrubber      │ │
│  │  Graph proj. │                      └─────────────────────────┘ │
│  │  (in-DuckDB) │                                                   │
│  └──────────────┘                                                   │
└─────────────────────────────────────────────────────────────────────┘
```

Full architecture deep dive → [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Examples

Runnable demos, each with its own `README.md` and `run.sh`:

| Example | Shows |
|---------|-------|
| [`examples/quickstart-ais/`](examples/quickstart-ais/) | Universal-ingest API + ORP-QL with a sample AIS dataset (no internet needed). |
| [`examples/two-node-federation/`](examples/two-node-federation/) | Two ORP nodes peered together; data on one shows up on the other. |
| [`examples/saved-queries/`](examples/saved-queries/) | A directory of `.orpql` files loaded as saved queries and monitor rules. |
| [`examples/adapter-config/`](examples/adapter-config/) | Annotated `config.yaml` with 6 adapters configured side-by-side. |

```bash
cd examples/quickstart-ais && ./run.sh
```

---

## Documentation Map

- **[docs/QUICKSTART.md](docs/QUICKSTART.md)** — 10-minute tour: install → ingest → query → connect a real adapter → federate.
- **[docs/RECIPES.md](docs/RECIPES.md)** — copy-paste recipes for the most common tasks.
- **[docs/CONFIG.md](docs/CONFIG.md)** — every config field, env var, and CLI flag.
- **[docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md)** — every `orp` subcommand.
- **[docs/ORP_QL_GUIDE.md](docs/ORP_QL_GUIDE.md)** — the query language.
- **[docs/CONNECTOR_GUIDE.md](docs/CONNECTOR_GUIDE.md)** — write your own adapter in ~50 lines.
- **[docs/API_REFERENCE.md](docs/API_REFERENCE.md)** — REST + WebSocket reference.
- **[docs/SECURITY.md](docs/SECURITY.md)** — OIDC, ABAC, Ed25519 audit log.
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — component-by-component deep dive.
- **[CHANGELOG.md](CHANGELOG.md)** — what landed when.

### Benchmarks

ORP ships a `criterion`-based benchmark suite that covers the parser, storage, stream-processor, and query hot paths.

```bash
cargo bench --workspace             # full suite (~5–10 min)
cargo bench -p orp-connector        # parser benches only (~1–2 min)
cargo bench -p orp-storage          # DuckDB write/query benches
```

- Post-v0.2.0 baseline numbers: [`benches/baseline.md`](benches/baseline.md).
- CI policy + how to add new benches: [`docs/BENCHES.md`](docs/BENCHES.md).
- Criterion HTML reports land in `target/criterion/report/index.html`.

Benchmarks are dev-only — `criterion` lives in `[dev-dependencies]`, so the release binary stays single-binary.

---

## Building from Source

### Prerequisites

| Dependency | Required | Version | Purpose |
|------------|----------|---------|---------|
| Rust toolchain | yes | 1.75+ (stable) | Compile core binary |
| `protoc` | yes | 3.20+ | `orp-proto/build.rs` compiles protobuf event schemas |
| Node.js | optional | 20+ | Build the React frontend (omit for `--headless` builds) |
| Docker | optional | any | Containerized deploy via the provided `Dockerfile` |
| `cmake`, `pkg-config` | yes (Linux) | system | Native deps for `duckdb`, `rocksdb`, `rustls` |

### Build

```bash
# 1. Install protoc + native deps
#    macOS:        brew install protobuf
#    Debian/Ubuntu: sudo apt install -y protobuf-compiler cmake pkg-config libssl-dev
#    Fedora:       sudo dnf install -y protobuf-compiler cmake openssl-devel
#    Arch:         sudo pacman -S protobuf cmake pkgconf openssl

# 2. Install Rust 1.75+ (skip if already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 3. Clone and build
git clone https://github.com/shieldofsteel/orp
cd orp
cargo test --workspace            # 1,362 tests
cargo build --release             # target/release/orp (~45 MB, statically linked)

# 4. Cross-compile for Raspberry Pi (ARM64)
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# 5. Docker
docker build -t orp .
docker run -p 9090:9090 orp start --template maritime

# 5a. Distroless image (recommended for production)
docker build -t orp:distroless --target distroless .
docker run -p 9090:9090 orp:distroless --template maritime
```

The `distroless` target is the **recommended production image**. It is built
from `gcr.io/distroless/cc-debian12:nonroot`: smaller than the Debian-slim
runtime, has no shell or package manager (so an attacker who lands a code
execution can't `sh`, `apt`, `curl`, etc.), and always runs as the pre-baked
`nonroot` user (uid/gid 65532) — `--user 0:0` is not allowed.

---

## Why Not TAK / FreeTAKServer / Palantir?

| | TAK Server | FreeTAKServer | Palantir | **ORP** |
|--|-----------|---------------|----------|---------|
| Open source | Restricted GOSS | EPL | ❌ | **Apache 2.0** |
| Modern web UI | ❌ Android-first | ❌ | ✅ | **✅** |
| Maritime domain | Minimal | Minimal | Limited | **First-class** |
| Aviation domain | ❌ | ❌ | Limited | **✅ ADS-B, ASTERIX** |
| Protocol parsers | CoT only | CoT only | Proprietary | **39 open adapters** |
| Multi-tenant SaaS | ❌ | ❌ | ✅ | **✅** |
| AI/anomaly detection | ❌ | ❌ | ✅ | **✅** |
| Edge / Raspberry Pi | ❌ | ⚠️ | ❌ | **✅ 45MB, headless** |
| Query language | ❌ | ❌ | Proprietary | **ORP-QL (open)** |
| Federation mesh | Limited | Limited | ✅ | **✅** |
| Cost | Free | Free | **$50–500M** | **Free** |

---

## Contributing

ORP is early. The protocol universe is large. Help is welcome.

**Highest-impact contributions:**
1. **New protocol adapters** — See [docs/CONNECTOR_GUIDE.md](docs/CONNECTOR_GUIDE.md) — a basic adapter is ~50 lines of Rust. Wishlist: ROS 2 / DDS, IEC 61850, ARINC 429, SAE J2735, Link 16 (J-series via JREAP), STANAG 4586. *Already shipped: MISB ST 0601 KLV, Apache Kafka, NATS, CCSDS+SGP4, HL7 v2.5, MAVLink v2.*
2. **Test coverage** — 1,362 tests and growing. More protocol parsing tests, more edge cases, more fuzz harnesses on parser adapters welcome.
3. **Frontend features** — React/TypeScript. See [frontend/src/components/](frontend/src/components/).
4. **Documentation** — real-world deployment guides, integration recipes.
5. **Performance** — benchmarks, profiling, optimization.

```bash
# Run tests
cargo test

# Check lints (must be zero warnings)
cargo clippy -- -D warnings

# Format
cargo fmt

# Run a specific adapter test
cargo test -p orp-connector nmea
```

Issues are tracked on GitHub. PRs welcome. No CLA required.

---

## License

Apache 2.0 — see [LICENSE](LICENSE).

Use it commercially. Fork it. Embed it in products. Build a business on it. The only thing you can't do is sue us for patent infringement using patents you contributed.

---

<div align="center">

**ORP** — single-binary, single-port, every-protocol — because operational awareness shouldn't cost $50M.

[⭐ Star on GitHub](https://github.com/shieldofsteel/orp) · [📖 Docs](docs/) · [🐛 Issues](https://github.com/shieldofsteel/orp/issues) · [💬 Discussions](https://github.com/shieldofsteel/orp/discussions)

</div>
