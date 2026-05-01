#!/usr/bin/env bash
# examples/two-node-federation — boot two ORPs locally, peer them, verify sync.
#
# Run with:    cd examples/two-node-federation && ./run.sh
# Override:    ORP_BIN=./target/release/orp ./run.sh

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────
ORP_BIN="${ORP_BIN:-orp}"
ALPHA_PORT="${ALPHA_PORT:-19090}"
BETA_PORT="${BETA_PORT:-19091}"
ALPHA="http://127.0.0.1:${ALPHA_PORT}"
BETA="http://127.0.0.1:${BETA_PORT}"
WAIT_SYNC_SECS="${WAIT_SYNC_SECS:-12}"
ALPHA_LOG="$(mktemp -t orp-alpha.XXXXXX.log)"
BETA_LOG="$(mktemp -t orp-beta.XXXXXX.log)"
ALPHA_PID=""
BETA_PID=""
TMP_BETA_DEDUP="$(mktemp -d -t orp-beta-dedup.XXXXXX)"

# ── Cleanup ───────────────────────────────────────────────────────────────────
cleanup() {
  for pid in "$ALPHA_PID" "$BETA_PID"; do
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      for _ in 1 2 3 4 5; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
      done
      kill -9 "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -f "$ALPHA_LOG" "$BETA_LOG" 2>/dev/null || true
  rm -rf "$TMP_BETA_DEDUP" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ── Sanity ────────────────────────────────────────────────────────────────────
if ! command -v "$ORP_BIN" >/dev/null 2>&1; then
  printf 'error: orp binary not found at %s. Set ORP_BIN.\n' "$ORP_BIN" >&2
  exit 1
fi

wait_health() {
  local url="$1"; local pid="$2"; local label="$3"
  local deadline=$(( $(date +%s) + 30 ))
  until curl -fsS "${url}/api/v1/health" >/dev/null 2>&1; do
    if ! kill -0 "$pid" 2>/dev/null; then
      printf 'error: %s exited before becoming healthy.\n' "$label" >&2
      return 1
    fi
    if [ "$(date +%s)" -ge "$deadline" ]; then
      printf 'error: %s did not become healthy in 30s.\n' "$label" >&2
      return 1
    fi
    sleep 1
  done
}

# ── 1. Start alpha + beta ─────────────────────────────────────────────────────
printf '[1/6] Starting alpha on :%s ...\n' "$ALPHA_PORT"
ORP_FED_BASE_INTERVAL_SECS="${ORP_FED_BASE_INTERVAL_SECS:-5}" \
  "$ORP_BIN" start \
    --in-memory --headless --no-auth --port "$ALPHA_PORT" \
    >"$ALPHA_LOG" 2>&1 &
ALPHA_PID=$!

printf '[1/6] Starting beta  on :%s ...\n' "$BETA_PORT"
ORP_DEDUP_PATH="$TMP_BETA_DEDUP" \
ORP_FED_BASE_INTERVAL_SECS="${ORP_FED_BASE_INTERVAL_SECS:-5}" \
  "$ORP_BIN" start \
    --in-memory --headless --no-auth --port "$BETA_PORT" \
    >"$BETA_LOG" 2>&1 &
BETA_PID=$!

printf '[2/6] Waiting for both /health ...\n'
wait_health "$ALPHA" "$ALPHA_PID" alpha || { cat "$ALPHA_LOG" >&2; exit 1; }
wait_health "$BETA"  "$BETA_PID"  beta  || { cat "$BETA_LOG"  >&2; exit 1; }
printf '       both healthy.\n'

# ── 3. Register peers in both directions ─────────────────────────────────────
printf '[3/6] Registering peers ...\n'
"$ORP_BIN" --host "$ALPHA" peer add "127.0.0.1:${BETA_PORT}"  --name beta  --trust-score 0.85 || true
"$ORP_BIN" --host "$BETA"  peer add "127.0.0.1:${ALPHA_PORT}" --name alpha --trust-score 0.85 || true

printf '\n   alpha peers:\n'
"$ORP_BIN" --host "$ALPHA" peer list || true
printf '   beta peers:\n'
"$ORP_BIN" --host "$BETA"  peer list || true

# ── 4. Ingest data on each side ───────────────────────────────────────────────
printf '\n[4/6] Ingesting on alpha (vessel) and beta (aircraft) ...\n'
curl -fsS -X POST "${ALPHA}/api/v1/ingest" \
  -H 'Content-Type: application/json' \
  -d '{"name":"DEMO VESSEL","entity_type":"ship","mmsi":111111111,"lat":51.92,"lon":4.27,"speed":12.0}' >/dev/null
curl -fsS -X POST "${BETA}/api/v1/ingest" \
  -H 'Content-Type: application/json' \
  -d '{"name":"DEMO AIRCRAFT","entity_type":"aircraft","icao":"abcd12","lat":52.0,"lon":4.5,"speed":420}' >/dev/null

# ── 5. Wait for federation sync, then query both sides ───────────────────────
printf '[5/6] Waiting %ss for federation sync ...\n' "$WAIT_SYNC_SECS"
sleep "$WAIT_SYNC_SECS"

printf '\n   alpha → ships:\n'
"$ORP_BIN" --host "$ALPHA" entities search --entity-type ship --limit 5 || true
printf '   alpha → aircraft (should include DEMO AIRCRAFT after sync):\n'
"$ORP_BIN" --host "$ALPHA" entities search --entity-type aircraft --limit 5 || true

printf '\n   beta → ships (should include DEMO VESSEL after sync):\n'
"$ORP_BIN" --host "$BETA" entities search --entity-type ship --limit 5 || true
printf '   beta → aircraft:\n'
"$ORP_BIN" --host "$BETA" entities search --entity-type aircraft --limit 5 || true

# ── 6. Done ──────────────────────────────────────────────────────────────────
printf '\n[6/6] Stopping nodes ...\n'
printf 'OK\n'
