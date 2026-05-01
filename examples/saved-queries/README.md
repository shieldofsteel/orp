# `examples/saved-queries` — Load `.orpql` Files as Saved Queries / Monitors

Demonstrates two patterns for keeping ORP queries in version control:

1. **Saved queries (interactive).** A directory of `.orpql` files you can pipe
   into `orp query --file -` from a script, CI job, or notebook.
2. **Monitor rules from queries.** Loading the same files as monitor rules at
   server startup so they fire continuously and emit alerts on the WebSocket.

## Run

```bash
cd examples/saved-queries
./run.sh
```

What happens:

1. Boots `orp start --in-memory --headless --no-auth` on :19090.
2. Ingests a tiny demo dataset (4 vessels, 3 aircraft, 2 sensors).
3. For each `.orpql` file in `queries/`, runs `orp query --file <file>` and
   prints the results.
4. Registers the `monitor.*` rules in `monitors.yaml` via the API.
5. POSTs an event that should fire each monitor; tails the alert feed for ~3s.
6. Tears down.

## Files

| File | Purpose |
|------|---------|
| `run.sh` | Driver. POSIX-bash, `set -euo pipefail`. |
| `queries/q1-fast-vessels.orpql` | Saved-query example. |
| `queries/q2-low-altitude-aircraft.orpql` | Saved-query example. |
| `queries/q3-sensors-by-status.orpql` | Saved-query example. |
| `monitors.yaml` | Monitor-rule definitions. Loaded with `orp monitors add`. |

## Pattern: saved queries in CI

```bash
for q in queries/*.orpql; do
  echo ">>> $q"
  orp query --file "$q" --output json | jq '.'
done
```

ORP-QL files can include line comments (`-- ...`); `run.sh` strips them before
sending. The CLI also accepts queries on stdin (`orp query --file -`), so the
above can be piped from a text generator.
