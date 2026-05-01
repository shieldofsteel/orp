#!/usr/bin/env bash
# examples/quickstart-ais — boot ORP, ingest a sample AIS dataset, run 3 ORP-QL queries, tear down.
#
# Run with:   cd examples/quickstart-ais && ./run.sh
# Override:   PORT=29090 ORP_BIN=./target/release/orp ./run.sh

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────
PORT="${PORT:-19090}"
ORP_BIN="${ORP_BIN:-orp}"
HOST="http://127.0.0.1:${PORT}"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
LOG="$(mktemp -t orp-quickstart.XXXXXX.log)"
PID=""

# ── Cleanup ───────────────────────────────────────────────────────────────────
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
  if [ -f "$LOG" ]; then
    rm -f "$LOG"
  fi
}
trap cleanup EXIT INT TERM

# ── Sanity ────────────────────────────────────────────────────────────────────
if ! command -v "$ORP_BIN" >/dev/null 2>&1; then
  printf 'error: orp binary not found at %s. Set ORP_BIN or install via scripts/install.sh.\n' "$ORP_BIN" >&2
  exit 1
fi

# ── 1. Start orp in background ────────────────────────────────────────────────
printf '[1/5] Starting %s on :%s (in-memory, headless, no-auth) ...\n' "$ORP_BIN" "$PORT"
"$ORP_BIN" start \
  --in-memory \
  --headless \
  --no-auth \
  --port "$PORT" \
  >"$LOG" 2>&1 &
PID=$!

# ── 2. Wait for /health ───────────────────────────────────────────────────────
printf '[2/5] Waiting for %s/api/v1/health ...\n' "$HOST"
deadline=$(( $(date +%s) + 30 ))
until curl -fsS "${HOST}/api/v1/health" >/dev/null 2>&1; do
  if ! kill -0 "$PID" 2>/dev/null; then
    printf 'error: orp exited before health-check came up. log:\n' >&2
    cat "$LOG" >&2
    exit 1
  fi
  if [ "$(date +%s)" -ge "$deadline" ]; then
    printf 'error: health check timed out after 30s. log:\n' >&2
    cat "$LOG" >&2
    exit 1
  fi
  sleep 1
done
printf '       healthy.\n'

# ── 3. Ingest vessels.json ────────────────────────────────────────────────────
printf '[3/5] Ingesting vessels.json ...\n'
records="$(jq -c '.' "$HERE/vessels.json")"
result="$(curl -fsS -X POST "${HOST}/api/v1/ingest/batch" \
  -H 'Content-Type: application/json' \
  -d "{\"records\": ${records}}")"
ingested="$(printf '%s' "$result" | jq -r '.ingested // (.results | length // 0)')"
printf '       %s records ingested.\n' "$ingested"

# ── 4. Run queries ────────────────────────────────────────────────────────────
run_query() {
  local label="$1"
  local file="$2"
  local query
  query="$(grep -v '^--' "$file" | tr '\n' ' ' | sed 's/  */ /g')"
  printf '\n   Q: %s\n' "$label"
  printf '   > %s\n' "$query"
  curl -fsS -X POST "${HOST}/api/v1/query" \
    -H 'Content-Type: application/json' \
    -d "{\"query\": $(printf '%s' "$query" | jq -Rs '.')}" \
    | jq -C '.results // .'
}

printf '[4/5] Running queries ...\n'
run_query "Q1 — count by ship_type"  "$HERE/queries/q1-count-by-type.orpql"
run_query "Q2 — vessels >20 kn"      "$HERE/queries/q2-fast-vessels.orpql"
run_query "Q3 — near Rotterdam bbox" "$HERE/queries/q3-near-rotterdam.orpql"

# ── 5. Done ──────────────────────────────────────────────────────────────────
printf '\n[5/5] Stopping orp ...\n'
# cleanup() does the actual kill via trap
printf 'OK\n'
