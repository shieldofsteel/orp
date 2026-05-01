# `examples/two-node-federation` — Two ORP Nodes, Federated

Boots two ORP nodes on `localhost`, registers them as peers of each other, ingests
data into one, and verifies it appears in the other after the federation sync
runs.

You can run this either with the bundled `docker-compose.yml` or with the plain
shell script in `run.sh` (no Docker required).

## Run (no Docker)

```bash
cd examples/two-node-federation
./run.sh
```

What happens:

1. Two ORP nodes start: `alpha` on :19090, `beta` on :19091, both `--in-memory --headless --no-auth`.
2. Each is registered as the other's peer with `trust_score=0.85`.
3. We POST a vessel to `alpha` and an aircraft to `beta`.
4. We override `ORP_FED_BASE_INTERVAL_SECS=5` so we don't have to wait 30 s.
5. After ~10 s we query each node and confirm both entities appear on both nodes.
6. Both nodes are torn down.

## Run (Docker)

```bash
docker compose up
# In another terminal:
docker compose exec orp-fed-alpha orp peer add orp-fed-beta:9090
docker compose exec orp-fed-beta  orp peer add orp-fed-alpha:9090
```

The compose file is the upstream `docker-compose.yml` — see the `federation` profile.

## Files

| File | Purpose |
|------|---------|
| `run.sh` | Single-shell-script runner. POSIX-bash, `set -euo pipefail`. |
| `docker-compose.yml` | Two-node compose stack (uses ports 19092/19093). |
| `configs/alpha.yaml` | Annotated config for the alpha node. |
| `configs/beta.yaml`  | Annotated config for the beta node. |

## Tuning

- `ORP_FED_BASE_INTERVAL_SECS` — sync interval on success (default 30; example uses 5).
- `ORP_FED_MAX_INTERVAL_SECS` — adaptive backoff cap on failure (default 600).

When the peer is unreachable, the per-peer interval doubles up to the cap. When the
peer recovers, it resets to base. This is the Wave-2 federation hardening referenced
in `CHANGELOG.md`.
