#!/usr/bin/env bash
# ORP Release Helper
#
# Today this is a thin wrapper around the existing build/release flow whose
# main job is to expose a `--smoke` mode used by CI (and locally, before
# tagging) to verify that a freshly-built `orp` binary actually starts,
# answers `/api/v1/health`, ingests an entity, lists it back, and runs an
# ORP-QL query.
#
# Usage:
#   ./scripts/release.sh --smoke   # build (if needed) + run end-to-end smoke
#   ./scripts/release.sh --help
#
# Exit codes:
#   0   success
#   1   smoke failure (reason printed as `FAIL: <reason>`)

set -uo pipefail

# ── Repo paths ────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/release/orp"

# ── Smoke knobs ───────────────────────────────────────────────────────────────
SMOKE_PORT="${SMOKE_PORT:-19099}"
SMOKE_HOST="127.0.0.1"
SMOKE_BASE="http://${SMOKE_HOST}:${SMOKE_PORT}"
SMOKE_TIMEOUT="${SMOKE_TIMEOUT:-30}"

SMOKE_PID=""
SMOKE_LOG=""

usage() {
  cat <<'EOF'
Usage: scripts/release.sh [--smoke|--help]

  --smoke   Build the release binary if missing, start it on port 19099 in
            in-memory headless --no-auth mode, exercise the public API, and
            kill it. Prints "OK" on success, "FAIL: <reason>" on failure.
  --help    Show this help.
EOF
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  cleanup
  exit 1
}

cleanup() {
  if [ -n "$SMOKE_PID" ] && kill -0 "$SMOKE_PID" 2>/dev/null; then
    kill "$SMOKE_PID" 2>/dev/null || true
    # Give it a beat to flush before SIGKILL.
    for _ in 1 2 3 4 5; do
      kill -0 "$SMOKE_PID" 2>/dev/null || break
      sleep 1
    done
    kill -9 "$SMOKE_PID" 2>/dev/null || true
    wait "$SMOKE_PID" 2>/dev/null || true
  fi
  if [ -n "$SMOKE_LOG" ] && [ -f "$SMOKE_LOG" ]; then
    rm -f "$SMOKE_LOG"
  fi
}

trap cleanup EXIT INT TERM

ensure_binary() {
  if [ ! -x "$BIN" ]; then
    printf '[release] %s not found — building...\n' "$BIN"
    ( cd "$REPO_ROOT" && cargo build --release -p orp-core ) \
      || fail "cargo build --release -p orp-core failed"
  fi
  [ -x "$BIN" ] || fail "binary still missing after build: $BIN"
}

wait_for_health() {
  local deadline=$(( $(date +%s) + SMOKE_TIMEOUT ))
  local body
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if ! kill -0 "$SMOKE_PID" 2>/dev/null; then
      fail "server exited before becoming healthy (see log: $SMOKE_LOG)"
    fi
    if body="$(curl -fsS "${SMOKE_BASE}/api/v1/health" 2>/dev/null)"; then
      printf '%s' "$body"
      return 0
    fi
    sleep 1
  done
  fail "health check timed out after ${SMOKE_TIMEOUT}s"
}

run_smoke() {
  ensure_binary

  SMOKE_LOG="$(mktemp -t orp-smoke.XXXXXX.log)"

  printf '[smoke] starting orp on :%s (in-memory, headless, no-auth)\n' \
    "$SMOKE_PORT"

  "$BIN" start \
    --in-memory \
    --headless \
    --no-auth \
    --port "$SMOKE_PORT" \
    >"$SMOKE_LOG" 2>&1 &
  SMOKE_PID=$!

  # 1+2+3: wait for /health
  local health_body
  health_body="$(wait_for_health)"

  # 4: graph_engine field present?
  printf '%s' "$health_body" | grep -q '"graph_engine"' \
    || fail "health response missing graph_engine field"

  # 5: POST entity. The public ingest path is POST /api/v1/entities.
  local create_body create_status
  create_body=$(curl -sS -o /dev/null -w '%{http_code}' \
    -X POST "${SMOKE_BASE}/api/v1/entities" \
    -H 'Content-Type: application/json' \
    --data '{"id":"smoke-1","type":"generic","name":"smoke-test"}') \
    || fail "POST /api/v1/entities failed (curl error)"
  create_status="$create_body"
  case "$create_status" in
    201|200) ;;
    *) fail "POST /api/v1/entities returned HTTP ${create_status}" ;;
  esac

  # 6: GET entities list and confirm the one we just inserted is in there.
  local list_body
  list_body=$(curl -fsS "${SMOKE_BASE}/api/v1/entities?type=generic") \
    || fail "GET /api/v1/entities?type=generic failed"
  printf '%s' "$list_body" | grep -q '"smoke-1"' \
    || fail "entity smoke-1 not found in list response"

  # 7: ORP-QL query → status=success
  local query_body
  query_body=$(curl -fsS \
    -X POST "${SMOKE_BASE}/api/v1/query" \
    -H 'Content-Type: application/json' \
    --data '{"query":"MATCH (e:Entity) RETURN e LIMIT 3"}') \
    || fail "POST /api/v1/query failed"
  printf '%s' "$query_body" | grep -q '"status":"success"' \
    || fail "query response missing status=success: ${query_body}"

  # 8: cleanup() will kill the server. Print OK + exit 0.
  printf 'OK\n'
}

# ── Argument dispatch ─────────────────────────────────────────────────────────
if [ $# -eq 0 ]; then
  usage
  exit 0
fi

case "${1:-}" in
  --smoke)
    run_smoke
    ;;
  --help|-h)
    usage
    ;;
  *)
    printf 'unknown flag: %s\n' "$1" >&2
    usage
    exit 2
    ;;
esac
