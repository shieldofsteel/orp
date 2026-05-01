# ORP Quickstart — Zero to Live Data in 10 Minutes

> **Goal:** install ORP, ingest your first event, query it back, plug in a real adapter, and federate two nodes — without reading any other doc first.

This guide is *concrete*. Every command is copy-pasteable and verified against the binary that ships from this repo. If a command doesn't work, that's a bug — open an issue.

**Estimated time:** 10 minutes (5 if you skip federation).
**Prerequisites:** macOS or Linux, `curl`, `jq` recommended for pretty-printing JSON.

---

## Table of Contents

1. [Install](#1-install)
2. [First Run](#2-first-run)
3. [Ingest Your First Event](#3-ingest-your-first-event)
4. [Query It Back](#4-query-it-back)
5. [Connect a Real Adapter (NMEA over UDP)](#5-connect-a-real-adapter-nmea-over-udp)
6. [Federation: Two Nodes on Localhost](#6-federation-two-nodes-on-localhost)
7. [What to Read Next](#7-what-to-read-next)

---

## 1. Install

Pick one. They all give you a binary called `orp` on `PATH`.

### Option A — One-line installer (preferred)

```bash
curl -fsSL https://raw.githubusercontent.com/shieldofsteel/orp/master/scripts/install.sh | sh
```

The installer detects your OS + arch, downloads the matching tarball from the [latest GitHub release](https://github.com/shieldofsteel/orp/releases/latest), verifies its SHA-256 checksum, and drops `orp` into `/usr/local/bin` (falling back to `~/.local/bin` if that's not writable).

> Until the first tagged release lands, the URL above will 404. Use Option B or C in the meantime.

### Option B — Homebrew (macOS)

```bash
# Once the formula is in homebrew-core (tracked in #TBD):
brew install orp

# In the meantime:
brew tap shieldofsteel/orp
brew install orp
```

### Option C — Cargo (any Rust toolchain >= 1.75)

```bash
# Native deps — protoc is required because orp-proto compiles its event schemas at build time.
brew install protobuf            # macOS
sudo apt install -y protobuf-compiler cmake pkg-config libssl-dev   # Debian/Ubuntu

cargo install --git https://github.com/shieldofsteel/orp orp-core
```

The Cargo install takes ~5–10 minutes the first time. The release binary is ~45 MB statically linked.

### Verify

```bash
orp --version
orp doctor          # green-light preflight checks
```

If `orp doctor` shows any red `✗` checks, fix those first — `orp start` will fail in confusing ways otherwise.

---

## 2. First Run

```bash
orp serve --in-memory
```

Wait — `serve`? The binary uses `start`, not `serve`. The full command is:

```bash
orp start --in-memory
```

`--in-memory` opts into a non-persistent DuckDB instance — perfect for the quickstart, demos, and CI. Without it, ORP creates `./data.duckdb`, `./state.db`, and `./audit.log` in the working directory (real persistence — that's the SQLite-style promise).

Expected first-run output (excerpts):

```
  ╔═══════════════════════════════════════════════════════════╗
  ║  Open Reality Protocol v0.1.0                             ║
  ║  Palantir-grade data fusion in a single binary            ║
  ╚═══════════════════════════════════════════════════════════╝

INFO Initializing DuckDB storage (in-memory). All state is lost on shutdown.
INFO Loaded 10 ports
INFO Starting AIS connector (demo mode)...
INFO Starting HTTP server on 0.0.0.0:9090
INFO Dashboard: http://localhost:9090/
INFO API:       http://localhost:9090/api/v1/
INFO Health:    http://localhost:9090/api/v1/health
```

In a second terminal:

```bash
orp status
```

You should see the components green-lit. The web UI is at http://localhost:9090 — but for this quickstart we'll do everything via the API.

> **Headless?** Pass `--headless` to skip serving the React frontend; useful on a Raspberry Pi or in CI. The API + WebSocket still come up on the same port.

---

## 3. Ingest Your First Event

ORP's universal-ingest endpoint accepts *any* JSON object and auto-detects the entity type. No schema registration needed.

```bash
curl -fsS -X POST http://localhost:9090/api/v1/ingest \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Patrol Unit 7",
    "entity_type": "vehicle",
    "lat": 51.505,
    "lon": -0.09,
    "speed_kn": 28.4,
    "heading": 87,
    "source": "fleet_tracker"
  }' | jq .
```

Expected response (request_id will differ):

```json
{
  "entity_id": "...",
  "entity_type": "vehicle",
  "ingested_at": "2026-05-01T..."
}
```

The auto-detection rules live in [`crates/orp-core/src/server/ingest.rs`](../crates/orp-core/src/server/ingest.rs). Some highlights:

| Payload contains          | Becomes type |
|---------------------------|--------------|
| `mmsi` or `imo`           | `ship`       |
| `icao` or callsign+alt    | `aircraft`   |
| `ip` or `hostname`        | `host`       |
| `cve` or `vulnerability`  | `threat`     |
| `temperature`/`humidity`  | `sensor`     |
| `plate` or `vin`          | `vehicle`    |
| `lat` + `lon` only        | `point`      |
| (fallback)                | `generic`    |

If you set `entity_type` explicitly (as above), it wins.

---

## 4. Query It Back

ORP-QL is SQL with a Cypher-flavoured `MATCH` clause for graph traversal. The CLI is the easiest way in:

```bash
orp query "MATCH (v:vehicle) RETURN v.entity_id, v.name, v.speed_kn LIMIT 10"
```

You can also POST directly:

```bash
curl -fsS -X POST http://localhost:9090/api/v1/query \
  -H 'Content-Type: application/json' \
  -d '{"query": "MATCH (v:vehicle) RETURN v LIMIT 10"}' | jq .
```

Common patterns:

```bash
# All entities updated in the last 5 minutes
orp query "MATCH (e:Entity) WHERE e.updated_at > NOW() - INTERVAL '5 minutes' RETURN e LIMIT 50"

# Geospatial search via the dedicated endpoint (fastest path)
curl -fsS "http://localhost:9090/api/v1/entities/search?lat=51.5&lon=-0.1&radius_km=25" | jq .

# Or via the CLI
orp entities search --near 51.5,-0.1 --radius 25 --limit 50
```

---

## 5. Connect a Real Adapter (NMEA over UDP)

The simplest way to get *real* live data flowing through your local ORP is to connect to a public AIS feed. ORP ships with two AIS connectors: a TCP/UDP NMEA listener (`AisConnector`) and an `aisstream.io` WebSocket connector. We'll use the AISStream path because it works through a NAT without port forwarding.

### Option 1 — AISStream.io (recommended, requires free API key)

Get a key at https://aisstream.io/ (free, no card), then:

```bash
# Stop the running ORP (Ctrl-C in the first terminal), then restart with:
AISSTREAM_API_KEY=your-key-here orp start --in-memory
```

When ORP detects `AISSTREAM_API_KEY`, it boots the live connector instead of the demo one. You'll see ships flowing within 30 seconds:

```bash
orp entities search --entity-type ship --limit 5
```

### Option 2 — Direct NMEA TCP (no API key, but needs a feed)

If you have an AIS receiver on your LAN serving NMEA over TCP (typical port: 10110):

```bash
orp connect ais://192.168.1.42:10110
orp entities search --entity-type ship
```

For a public test feed, see [examples/quickstart-ais/](../examples/quickstart-ais/) — it ships with a small replay file you can pump into ORP using the bundled `aisstream-bridge.py`.

---

## 6. Federation: Two Nodes on Localhost

Federation is ORP-to-ORP peer mesh sync. Each node shares only what its ABAC policy permits, conflict resolution prefers the highest-confidence source, and the link is delta-only.

Run two ORPs side-by-side:

```bash
# Terminal 1 — Alpha (port 9090)
orp start --in-memory --port 9090

# Terminal 2 — Beta (port 9091, different RocksDB path so they don't fight)
ORP_DEDUP_PATH=/tmp/orp-beta-dedup orp start --in-memory --port 9091
```

In a third terminal, register them as peers of each other:

```bash
orp --host http://localhost:9090 peer add localhost:9091 --name beta --trust-score 0.85
orp --host http://localhost:9091 peer add localhost:9090 --name alpha --trust-score 0.85

orp --host http://localhost:9090 peer list
orp --host http://localhost:9091 peer list
```

Now ingest into Alpha and watch it appear in Beta:

```bash
curl -fsS -X POST http://localhost:9090/api/v1/ingest \
  -H 'Content-Type: application/json' \
  -d '{"name":"Drone 12","entity_type":"aircraft","lat":51.5,"lon":-0.1,"icao":"abcd12"}'

# Wait one sync interval (default 30s; configurable via ORP_FED_BASE_INTERVAL_SECS).
sleep 35

orp --host http://localhost:9091 entities search --entity-type aircraft
```

A full two-node demo with `docker-compose` and config files is in [examples/two-node-federation/](../examples/two-node-federation/).

> **Adaptive backoff.** When a peer is unreachable, the sync interval doubles up to `ORP_FED_MAX_INTERVAL_SECS` (default 600 s). When it recovers, it resets to base. This protects flapping satellite/4G uplinks from burning bandwidth.

---

## 7. What to Read Next

You now have ORP installed, ingesting real data, queryable from the CLI, and federated across two nodes. From here:

- **[RECIPES.md](RECIPES.md)** — copy-paste recipes for AIS, ADS-B, MAVLink, Modbus, Zeek, audit-log export, continuous alerts, and more.
- **[ARCHITECTURE.md](../ARCHITECTURE.md)** — how the binary is laid out, where each crate fits, the storage and graph projection model.
- **[CONFIG.md](CONFIG.md)** — every config field, env-var equivalent, and CLI flag.
- **[CLI_REFERENCE.md](CLI_REFERENCE.md)** — full reference for every `orp` subcommand.
- **[ORP_QL_GUIDE.md](ORP_QL_GUIDE.md)** — the query language top-to-bottom.
- **[CONNECTOR_GUIDE.md](CONNECTOR_GUIDE.md)** — write your own protocol adapter in ~50 lines of Rust.
- **[SECURITY.md](SECURITY.md)** — OIDC, ABAC, Ed25519 audit log, hash chain integrity.
- **[examples/](../examples/)** — runnable demos: AIS quickstart, two-node federation, saved queries, annotated multi-adapter config.

If you find a rough edge, that's a UX bug — please open an issue. The goal is "Postgres-grade DX in a single binary".
