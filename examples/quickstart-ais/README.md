# `examples/quickstart-ais` — End-to-End AIS Ingestion in 2 Minutes

This example boots an ORP node in `--in-memory` mode, ingests a small AIS dataset
shipped here as `vessels.json`, and runs three ORP-QL queries against it.

It uses ORP's universal-ingest endpoint (`POST /api/v1/ingest`) — *no live
internet feed required*. To swap in a live feed, see the bottom of this README.

## Run

```bash
cd examples/quickstart-ais
./run.sh
```

What happens:

1. `orp start --in-memory --headless --port 19090` runs in the background.
2. We wait for `/api/v1/health` to return `healthy`.
3. We POST `vessels.json` to `/api/v1/ingest/batch` (16 vessels).
4. We run three ORP-QL queries and print the results.
5. We tear down the server.

Expected output (truncated):

```
[1/5] Starting orp on :19090 …
[2/5] Waiting for /health …
[3/5] Ingesting vessels.json …
       16 records ingested (0 failed)
[4/5] Running queries …
   Q1: how many ships, by ship_type?
   …
[5/5] Stopping orp.
OK
```

## Files

| File | Purpose |
|------|---------|
| `run.sh` | The driver. POSIX-bash, `set -euo pipefail`. |
| `vessels.json` | 16 sample vessels in standard ingest format. |
| `queries/q1-count-by-type.orpql` | Group by ship_type. |
| `queries/q2-fast-vessels.orpql` | Threshold filter on speed. |
| `queries/q3-near-rotterdam.orpql` | Geo filter near Rotterdam. |

## Swapping in a live feed

Edit `run.sh`: replace the `curl … /api/v1/ingest/batch` step with one of:

```bash
# Option A — live AISStream.io WebSocket (free key required):
export AISSTREAM_API_KEY=your-key
# Then run `orp start` without `--headless` and skip the curl step entirely.

# Option B — direct NMEA TCP from your AIS receiver:
orp connect ais://192.168.1.42:10110
```

See [`docs/RECIPES.md` §1](../../docs/RECIPES.md#1-ingest-ais-from-aisstreamio)
for the full recipe.
