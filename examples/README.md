# ORP Examples

Runnable demos. Each subdirectory has its own `README.md` and `run.sh` (or
equivalent). All examples assume an `orp` binary on `PATH` — install with
[scripts/install.sh](../scripts/install.sh) or `cargo build --release -p orp-core`.

| Example | What it shows | Time | Network needed? |
|---------|---------------|------|-----------------|
| [`quickstart-ais/`](quickstart-ais/) | Universal-ingest API with a sample AIS dataset; queries it back via ORP-QL. | ~2 min | No |
| [`two-node-federation/`](two-node-federation/) | Two ORP nodes peered together; data ingested in one appears in the other. | ~3 min | No (localhost) |
| [`saved-queries/`](saved-queries/) | A directory of `.orpql` files loaded as monitor rules at startup. | ~2 min | No |
| [`adapter-config/`](adapter-config/) | Annotated `config.yaml` showing 6 adapters configured side-by-side. Reference, not runnable. | — | — |

To run an example:

```bash
cd examples/quickstart-ais
./run.sh
```

All `run.sh` scripts use `set -euo pipefail` and clean up the ORP processes
they spawn on exit (so `Ctrl-C` is safe).
