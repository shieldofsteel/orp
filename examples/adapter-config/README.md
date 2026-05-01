# `examples/adapter-config` — Annotated `config.yaml` with 6 Adapters

This directory is **reference, not runnable** — it ships a single
`config.yaml` showing six representative adapters configured side-by-side, each
heavily commented to explain *why* the fields are set the way they are. The
intent: you copy the relevant block into your own deployment and tune.

The six adapters, chosen to span ORP's range:

1. **`aisstream`** — live global maritime AIS via WebSocket.
2. **`adsb`** — local SDR feed via TCP.
3. **`mavlink`** — UDP listener for drone heartbeat.
4. **`modbus`** — polled industrial PLC tags.
5. **`zeek`** — security log file watcher.
6. **`http_poll`** — generic REST API on a schedule.

Plus a `monitors:` block that wires three rules across them.

## Use it

```bash
cp examples/adapter-config/config.yaml ./config.yaml
# Edit URLs/keys/paths to match your environment.
orp config validate config.yaml
orp start --config config.yaml
```

`orp config validate` will catch typos and out-of-range trust scores before
boot. For a deeper field-by-field reference see [`docs/CONFIG.md`](../../docs/CONFIG.md).
