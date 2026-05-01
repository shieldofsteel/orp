<div align="center">

# ORP — Open Reality Protocol

### A single Rust binary that fuses 30+ protocols into one queryable real-time picture.

[![Tests](https://img.shields.io/badge/tests-1122%20passing-brightgreen?style=flat-square)](https://github.com/shieldofsteel/orp/actions)
[![Binary Size](https://img.shields.io/badge/binary-45MB-blue?style=flat-square)](https://github.com/shieldofsteel/orp/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-orange?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?style=flat-square)](https://www.rust-lang.org)
[![Crates](https://img.shields.io/badge/crates-13%20workspace%20crates-red?style=flat-square)](crates/)
[![Adapters](https://img.shields.io/badge/adapters-34-purple?style=flat-square)](crates/orp-connector/src/adapters/)

</div>

---

## What it is, in three lines

ORP is a single 45 MB Rust binary that ingests AIS, ADS-B, MAVLink, OPC-UA, MQTT, Modbus, Zeek, syslog, GRIB and 25 more protocols into one queryable real-time graph — with an ORP-QL query language, federation mesh sync, signed audit log, and a built-in COP map. **No JVM. No Postgres. No Kubernetes.** Just `./orp start` and you're ingesting in 30 seconds. The slot it fills: *the new SQLite/Postgres for real-time multi-source backends*.

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
| Protocol adapters out of the box | 0 | 0 | proprietary | proprietary | **34 open** |
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

**Fuses data from any source** — 34 protocol adapters, a universal JSON ingest endpoint, and a connector SDK. If it outputs data, ORP can consume it.

**Builds a live knowledge graph** — every entity (ship, aircraft, vehicle, sensor, threat) becomes a node. Relationships auto-form. The graph updates in real time.

**Renders a military-grade COP** — a full-featured map with 4 tile layers, directional arrows, course vectors, lasso select, and a timeline scrubber. Not a dashboard — an operational picture.

**Lets you query anything** — ORP-QL is a purpose-built query language combining SQL analytics with Cypher-style graph traversal. Query across sensors, entities, and time.

**Alerts you before it matters** — anomaly detection and threat scoring run continuously. When a ship deviates from its pattern-of-life, you know.

**Runs anywhere** — a laptop, a Raspberry Pi, a warship, a data center. `--headless` for embedded deployments. Docker for cloud. ARM binaries for edge.

---

## Protocol Support — at a Glance

ORP speaks the languages your sensors already use. 34 protocol adapters across maritime (NMEA 0183, AIS, NMEA 2000, ACARS), aviation (ADS-B / Mode S, ASTERIX, GRIB), drone autonomy (MAVLink v2), military / tactical (CoT, STIX/TAXII, NFFI, CEF), industrial / IoT (OPC-UA, Modbus TCP/RTU, MQTT, SparkplugB, DNP3, CAN/J1939, BACnet, LoRaWAN), cyber / network (Syslog, PCAP, Zeek, NetFlow / IPFIX), weather / environment (METAR, CAP), transport (GTFS-RT), and a universal-ingest endpoint that accepts any JSON.

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
cargo test --workspace            # 1,122 tests
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
| Protocol parsers | CoT only | CoT only | Proprietary | **34 open adapters** |
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
1. **New protocol adapters** — See [docs/CONNECTOR_GUIDE.md](docs/CONNECTOR_GUIDE.md) — a basic adapter is ~50 lines of Rust. Wishlist: ROS 2 / DDS, IEC 61850, MISB ST 0601 KLV (started), Apache Kafka (started), ARINC 429, CCSDS (started), HL7/FHIR (started), SAE J2735.
2. **Test coverage** — 1,122 tests is a start. More protocol parsing tests, more edge cases.
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
