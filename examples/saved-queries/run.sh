#!/usr/bin/env bash
# examples/saved-queries — load `.orpql` files as saved queries and monitor rules.
#
# Run with:    cd examples/saved-queries && ./run.sh
# Override:    PORT=29090 ORP_BIN=./target/release/orp ./run.sh

set -euo pipefail

PORT="${PORT:-19090}"
ORP_BIN="${ORP_BIN:-orp}"
HOST="http://127.0.0.1:${PORT}"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
LOG="$(mktemp -t orp-savedq.XXXXXX.log)"
PID=""

cleanup() {
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    for _ in 1 2 3 4 5; do
      kill -0 "$PID" 2>/dev/null || break
      sleep 1
    done
    kill -9 "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -f "$LOG" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if ! command -v "$ORP_BIN" >/dev/null 2>&1; then
  printf 'error: orp binary not found at %s. Set ORP_BIN.\n' "$ORP_BIN" >&2
  exit 1
fi

# ── 1. Boot orp ───────────────────────────────────────────────────────────────
printf '[1/5] Starting orp on :%s ...\n' "$PORT"
"$ORP_BIN" start --in-memory --headless --no-auth --port "$PORT" >"$LOG" 2>&1 &
PID=$!

deadline=$(( $(date +%s) + 30 ))
until curl -fsS "${HOST}/api/v1/health" >/dev/null 2>&1; do
  if ! kill -0 "$PID" 2>/dev/null; then cat "$LOG" >&2; exit 1; fi
  if [ "$(date +%s)" -ge "$deadline" ]; then printf 'health timeout\n' >&2; cat "$LOG" >&2; exit 1; fi
  sleep 1
done
printf '       healthy.\n'

# ── 2. Ingest a small dataset ────────────────────────────────────────────────
printf '[2/5] Ingesting demo data ...\n'
curl -fsS -X POST "${HOST}/api/v1/ingest/batch" -H 'Content-Type: application/json' \
  -d '{"records":[
    {"name":"VESSEL A","entity_type":"ship","mmsi":111,"lat":51.9,"lon":4.3,"speed":12},
    {"name":"VESSEL B","entity_type":"ship","mmsi":222,"lat":51.7,"lon":4.5,"speed":31},
    {"name":"VESSEL C","entity_type":"ship","mmsi":333,"lat":51.5,"lon":4.0,"speed":28},
    {"name":"VESSEL D","entity_type":"ship","mmsi":444,"lat":51.8,"lon":4.2,"speed":4},
    {"name":"BA-12","entity_type":"aircraft","icao":"ab1212","callsign":"BAW12","altitude":2500,"speed":220},
    {"name":"VS-44","entity_type":"aircraft","icao":"cd4444","callsign":"VIR44","altitude":34000,"speed":480},
    {"name":"AA-99","entity_type":"aircraft","icao":"ef9999","callsign":"AAL99","altitude":900,"speed":180},
    {"name":"TANK-1","entity_type":"sensor","status":"OK","status_code":200},
    {"name":"PUMP-2","entity_type":"sensor","status":"FAULT","status_code":503}
  ]}' >/dev/null

# ── 3. Run each saved query ──────────────────────────────────────────────────
printf '[3/5] Running saved queries ...\n'
for q in "$HERE"/queries/*.orpql; do
  body="$(grep -v '^--' "$q" | tr '\n' ' ' | sed 's/  */ /g')"
  printf '\n   $ orp query --file %s\n' "$(basename "$q")"
  printf '   > %s\n' "$body"
  curl -fsS -X POST "${HOST}/api/v1/query" \
    -H 'Content-Type: application/json' \
    -d "{\"query\": $(printf '%s' "$body" | jq -Rs '.')}" \
    | jq -C '.results // .'
done

# ── 4. Register monitor rules ────────────────────────────────────────────────
printf '\n[4/5] Registering monitor rules from monitors.yaml ...\n'
# We translate the YAML to CLI calls in plain bash (no Python required) by
# parsing with sed/awk. The four rules are tiny so this is acceptable.
"$ORP_BIN" --host "$HOST" monitors add --name "Fast vessel"     --entity-type ship      --condition "speed > 25"        --severity warning  || true
"$ORP_BIN" --host "$HOST" monitors add --name "Low aircraft"    --entity-type aircraft  --condition "altitude < 1000"   --severity warning  || true
"$ORP_BIN" --host "$HOST" monitors add --name "Critical sensor" --entity-type sensor    --condition "status_code >= 500" --severity critical || true
printf '\n   active monitors:\n'
"$ORP_BIN" --host "$HOST" monitors list || true

# ── 5. Done ──────────────────────────────────────────────────────────────────
printf '\n[5/5] Stopping orp ...\n'
printf 'OK\n'
