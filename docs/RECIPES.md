# ORP Recipes

> Copy-paste recipes for common tasks. Each is one paragraph of context and 5–15 lines of CLI / config / curl. Verified against the binary in this repo.

Conventions:

- `$ORP` is shorthand for `http://localhost:9090` — set it with `export ORP=http://localhost:9090` once and the snippets read more naturally.
- Recipes assume `orp start --in-memory` (or persistent equivalent) is already running. Many use `orp doctor` first to make sure the host is healthy.
- Any time we POST JSON, we use `curl -fsS` so transient failures fail loud.

---

## Table of Contents

1. [Ingest AIS from AISStream.io](#1-ingest-ais-from-aisstreamio)
2. [Tail a Zeek conn.log](#2-tail-a-zeek-connlog)
3. [Subscribe to MAVLink Heartbeat from a Drone](#3-subscribe-to-mavlink-heartbeat-from-a-drone)
4. [Watch ADS-B from Your Local SDR](#4-watch-ads-b-from-your-local-sdr)
5. [Mirror Modbus Tags into ORP](#5-mirror-modbus-tags-into-orp)
6. [Export the Audit Log for a Security Review](#6-export-the-audit-log-for-a-security-review)
7. [Set Up Federation Between Two ORP Nodes](#7-set-up-federation-between-two-orp-nodes)
8. [Run a Saved Query as a Continuous Alert](#8-run-a-saved-query-as-a-continuous-alert)

---

## 1. Ingest AIS from AISStream.io

**Context.** [aisstream.io](https://aisstream.io/) is a free, no-card global AIS WebSocket feed. ORP ships with a built-in `AisStreamConnector` that activates the moment `AISSTREAM_API_KEY` is set in the environment, replacing the demo connector. This is the fastest way to get *real* maritime traffic flowing through your laptop.

```bash
# 1. Get a free key at https://aisstream.io and set it.
export AISSTREAM_API_KEY=your-aisstream-key

# 2. Start ORP (Ctrl-C the demo first if it's running).
orp start --in-memory

# 3. In another terminal — within ~30s the ship store fills.
orp entities search --entity-type ship --limit 5
orp query "MATCH (s:ship) WHERE s.lat IS NOT NULL RETURN s.entity_id, s.name, s.lat, s.lon LIMIT 10"
```

**Expected output.** `entities search` returns a table of MMSI-keyed ships with names like `EVER GIVEN`, lat/lon, and a confidence around 0.95. The full WebSocket payload — vessel type, course, dimensions, destination — is preserved as flattened JSON properties on the entity.

---

## 2. Tail a Zeek conn.log

**Context.** Zeek (formerly Bro) is the de-facto open-source NSM. It writes a `conn.log` per protocol-aware connection. ORP's `zeek` adapter parses Zeek's TSV log format, maps source/destination addresses to `host` entities, and creates a `connection` event for every line. Best paired with the `csv_watcher` adapter when you'd rather handle CSV, but native Zeek TSV is the supported path.

```bash
# 1. Bring Zeek up against an interface (or a pcap):
sudo zeek -i eth0 -C       # live
zeek -r capture.pcap       # offline

# 2. Tell ORP to watch the file. Replace the path with where Zeek writes.
orp connect "http://localhost:9090/api/v1/connectors" || true   # ensure server up
curl -fsS -X POST $ORP/api/v1/connectors -H 'Content-Type: application/json' \
  -d '{
    "name": "zeek-conn",
    "connector_type": "zeek",
    "entity_type": "host",
    "trust_score": 0.85,
    "url": "file:///var/log/zeek/current/conn.log"
  }'

# 3. Watch hosts roll in.
orp entities search --entity-type host --limit 20
```

**Caveat.** The Zeek adapter assumes the canonical TSV header. If you've customised Zeek's logger, point `csv_watcher` at the file instead and supply a `mapping` block (see CONFIG.md).

---

## 3. Subscribe to MAVLink Heartbeat from a Drone

**Context.** ORP's MAVLink v2 adapter is a UDP listener that decodes the standard PX4 / ArduPilot / Skydio / Auterion / ModalAI ground-control messages: `HEARTBEAT`, `GLOBAL_POSITION_INT`, `VFR_HUD`, `ATTITUDE`, `GPS_RAW_INT`, `SYS_STATUS`. Per-vehicle deduplication uses `(system_id, component_id)`. Standard ground-station port is 14550.

```bash
# Set the drone (or QGroundControl) to forward MAVLink to your laptop:
#   QGC > Application Settings > MAVLink > Add UDP target > <your-ip>:14550

# Register the listener:
curl -fsS -X POST $ORP/api/v1/connectors -H 'Content-Type: application/json' \
  -d '{
    "name": "drone-fleet",
    "connector_type": "mavlink",
    "entity_type": "aircraft",
    "trust_score": 0.95,
    "url": "mavlink://0.0.0.0:14550"
  }'

# Live tail:
orp events --since 30s --output json | jq '.data[] | {entity:.entity_id, type:.event_type}'
```

**Verification.** A healthy drone produces a `HEARTBEAT` every second, `GLOBAL_POSITION_INT` typically at 4–10 Hz. If `orp entities search --entity-type aircraft` returns nothing after 5 s, suspect firewall: `sudo lsof -iUDP:14550` should show ORP listening.

---

## 4. Watch ADS-B from Your Local SDR

**Context.** ADS-B 1090 MHz aircraft broadcasts can be received with a $25 RTL-SDR dongle running [`dump1090`](https://github.com/flightaware/dump1090). `dump1090` exposes a Beast-format TCP feed on port 30005 and an SBS1 (BaseStation) feed on 30003. ORP's `adsb` adapter consumes Beast natively.

```bash
# 1. Run dump1090 (Mac/Linux). FlightAware's fork is the most common:
dump1090-fa --net &

# 2. Tell ORP to consume:
orp connect adsb://127.0.0.1:30005 --name local-sdr --trust-score 0.9

# 3. Aircraft should populate within a minute (depends on traffic over your antenna).
orp entities search --entity-type aircraft --limit 20
orp query "MATCH (a:aircraft) WHERE a.altitude < 10000 RETURN a.callsign, a.altitude, a.speed LIMIT 20"
```

**Tip.** ASTERIX (CAT 21 / 048) is also supported via the `asterix` adapter for radar feeds. It's the supported path if you have ATC-grade equipment instead of an SDR.

---

## 5. Mirror Modbus Tags into ORP

**Context.** The `modbus` adapter polls a Modbus TCP server (PLC, energy meter, etc.) on a schedule and creates `sensor` entities — one per polled register, deduplicated on `(server, register_addr)`. Use this to put Modbus state into ORP-QL alongside everything else.

```bash
# Connect to a Modbus TCP server at 192.168.1.50:502, polling once a second.
curl -fsS -X POST $ORP/api/v1/connectors -H 'Content-Type: application/json' \
  -d '{
    "name": "plc-floor-1",
    "connector_type": "modbus",
    "entity_type": "sensor",
    "trust_score": 0.9,
    "url": "tcp://192.168.1.50:502",
    "schedule": "every 1s",
    "mapping": {
      "registers": [
        { "name": "tank_level_pct", "address": 30001, "type": "input", "scale": 0.1 },
        { "name": "pump_running",   "address": 10001, "type": "coil"  }
      ]
    }
  }'

orp entities search --entity-type sensor --limit 5
```

**Watch out.** Modbus has no concept of timestamp; ORP stamps the entity with the poll completion time. If you need sub-second precision use SparkplugB (MQTT-backed industrial framing) instead.

---

## 6. Export the Audit Log for a Security Review

**Context.** Every state-changing API call is appended to a hash-chained, Ed25519-signed audit log on disk (`./audit.log` by default; `logging.audit_log_path` to override). For a security review you want the last N days of entries plus a verification of the chain.

```bash
# 1. Fetch the live audit log via the API:
curl -fsS "$ORP/api/v1/audit/log?since=$(date -u -d '7 days ago' +%FT%TZ)" > audit-7d.json
jq '.data | length' audit-7d.json

# 2. Or copy the on-disk file directly (preserves the binary signatures):
cp ./audit.log /tmp/orp-audit-snapshot.log
shasum -a 256 /tmp/orp-audit-snapshot.log

# 3. Verify the chain — every entry's prev_hash must equal SHA-256 of the previous entry,
#    and every Ed25519 signature must validate against the server's published pubkey.
curl -fsS $ORP/api/v1/audit/verify | jq .
```

**What to hand the auditor.** The audit JSON (or the on-disk binary log), the server's Ed25519 public key, and the output of `audit/verify` proving `chain_valid: true` and `signatures_valid: <count>`.

---

## 7. Set Up Federation Between Two ORP Nodes

**Context.** ORP-to-ORP peer mesh sync is built in. Each node sends only deltas, conflict resolution prefers the highest-confidence source, and per-peer adaptive backoff means a flapping uplink doesn't burn CPU. Trust score on the peer registration scales how confidently we accept its data.

```bash
# Terminal 1 — Alpha:
orp start --in-memory --port 9090

# Terminal 2 — Beta (different dedup path so they don't collide):
ORP_DEDUP_PATH=/tmp/orp-beta orp start --in-memory --port 9091

# Terminal 3 — register peers in both directions:
orp --host http://localhost:9090 peer add localhost:9091 --name beta  --trust-score 0.85
orp --host http://localhost:9091 peer add localhost:9090 --name alpha --trust-score 0.85

# Confirm:
orp --host http://localhost:9090 peer list
orp --host http://localhost:9091 peer list

# Ingest into Alpha; wait one sync interval; observe Beta:
curl -fsS -X POST http://localhost:9090/api/v1/ingest -H 'Content-Type: application/json' \
  -d '{"name":"Drone-12","entity_type":"aircraft","icao":"abcd12","lat":51.5,"lon":-0.1}'
sleep 35
orp --host http://localhost:9091 entities search --entity-type aircraft
```

**Tuning.** `ORP_FED_BASE_INTERVAL_SECS` (default 30) is the success-case interval. `ORP_FED_MAX_INTERVAL_SECS` (default 600) caps the backoff. A complete docker-compose-based two-node demo is in [examples/two-node-federation/](../examples/two-node-federation/).

---

## 8. Run a Saved Query as a Continuous Alert

**Context.** ORP monitor rules fire when a property threshold is crossed. They're declarative, persist across restarts (when stored in `config.yaml`), and emit alerts to the API + WebSocket `/ws` channel for downstream consumption (Slack, PagerDuty, etc.). For more nuanced predicates the monitor body itself can carry an ORP-QL query.

```bash
# Simple property threshold — alert on any vessel doing >25 knots.
orp monitors add \
  --name "fast-ship" \
  --entity-type ship \
  --condition "speed > 25" \
  --severity warning

# List active rules
orp monitors list

# Tail alerts as they fire (over WebSocket):
websocat ws://localhost:9090/ws | jq 'select(.type == "alert")'
```

For the saved-query pattern (load `.orpql` files at startup so they survive restarts), see [examples/saved-queries/](../examples/saved-queries/) — a directory layout and `run.sh` that wires saved queries through `orp config validate` + `orp monitors add` and verifies they actually fire.
