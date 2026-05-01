# ORP Configuration Reference

> Every config field, with type, default, environment-variable equivalent (when one exists), CLI flag equivalent (when one exists), and what the field actually does.

This reference is generated from the source of truth in
[`crates/orp-config/src/schema.rs`](../crates/orp-config/src/schema.rs) and the
clap definitions in [`crates/orp-core/src/cli/args.rs`](../crates/orp-core/src/cli/args.rs).
If a row contradicts the code, the code wins — please open an issue.

## Quick Orientation

ORP loads config from (in priority order):

1. CLI flags (e.g. `--port 9091`).
2. Environment variables — listed below per field. The env layer is read by the CLI for
   global args (e.g. `ORP_HOST`), and by the OIDC/JWT subsystems when explicitly enabled.
3. `config.yaml` in the working directory (or `--config <path>`). YAML supports
   `${env.VAR_NAME}` substitution; missing vars become empty strings with a warning log.
4. Built-in defaults baked into each `Default` impl in `schema.rs`.

A minimal valid config is `{}` — every section uses `#[serde(default)]`, so a 4-line
`config.yaml` will boot.

```yaml
# Minimum-viable config.yaml
server:
  port: 9090
```

For full config validation:

```bash
orp config validate config.yaml   # succeeds silently if valid; non-zero exit + reason if not
```

---

## Top-Level Layout

```yaml
server:             # listening + log + telemetry
storage:            # DuckDB / RocksDB / SQLite paths and limits
retention:          # event/snapshot/audit-log TTLs
security:           # OIDC, ABAC, audit signing
connectors:         # array of data sources
entity_resolution:  # how the same physical entity is recognised across sources
monitors:           # array of alert rules
api:                # rate limit, CORS, JWT secret, API key header
frontend:           # web UI port, default map view
logging:            # level, format, audit log path
templates:          # named bundles of connectors (rare in user-edited configs)
```

Below: each section, then a CLI-flag table, then env-var table.

---

## `server`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `host` | string | `"0.0.0.0"` | — | — | Interface to bind. `0.0.0.0` listens on all; `127.0.0.1` for loopback only. |
| `port` | u16 | `9090` | — | `--port` (on `start`) | TCP port for the API + WebSocket + frontend (single port for all). |
| `workers` | u32 | `4` | — | — | Reserved for future tokio runtime tuning; not currently honoured by the runtime, which uses tokio's default `multi_thread`. |
| `log_level` | string | `"info"` | `RUST_LOG` (overrides) | — | One of `error`/`warn`/`info`/`debug`/`trace`. `RUST_LOG` env var, when set, supersedes this. |
| `telemetry_enabled` | bool | `false` | — | — | Reserved for future telemetry export (OTLP). Has no effect today. |
| `telemetry_endpoint` | string? | `null` | — | — | Reserved (see above). |

### Defaults at a glance

`server` defaults to `0.0.0.0:9090`, info-level logging, telemetry off — i.e. "boots in the background, no surprises."

---

## `storage`

The storage section configures the four on-disk stores ORP uses. All paths are resolved relative to the process working directory.

### `storage.duckdb`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `path` | string | `"./data.duckdb"` | — | `--in-memory` (on `start`) bypasses entirely | Path to the persistent DuckDB file. Created on first start. `--in-memory` opts into a non-persistent in-memory DuckDB instead. |
| `memory_limit_gb` | u32 | `4` | — | — | Soft cap passed to DuckDB's pragma. Set to roughly half your RAM. |
| `max_connections` | u32 | `10` | — | — | DuckDB connection pool size. |

### `storage.rocksdb`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `path` | string | `"./state.db"` | `ORP_DEDUP_PATH` (overridden during `orp start`) | — | RocksDB directory used for the dedup window + DLQ. `orp start` actually picks `<tmp>/orp-dedup-<pid>` to avoid stale dedup hashes between runs; this field is the long-lived path you'd use in a production config. |
| `cache_size_mb` | u32 | `512` | — | — | LRU cache size for RocksDB block cache. |

### `storage.sqlite`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `path` | string | `"./config.sqlite"` | — | — | SQLite database used for connector + monitor configuration when persisted via `orp config set`. |

### `storage.kuzu` (reserved)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | `"./data.kuzu"` | Reserved for the future `--features kuzu-graph` Cargo feature. **Has no effect today.** The graph engine uses an in-DuckDB projection (see [ARCHITECTURE.md ADR-001](../ARCHITECTURE.md)). |
| `memory_limit_gb` | u32 | `2` | (reserved) |
| `sync_interval_seconds` | u64 | `30` | (reserved) |

---

## `retention`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `events_ttl_days` | u32 | `90` | — | — | Days to keep raw events in DuckDB before sweep. |
| `snapshots_ttl_days` | u32 | `30` | — | — | Days to keep periodic state snapshots. |
| `audit_log_ttl_days` | u32 | `365` | — | — | Days to keep audit log entries. **Set to 0 to disable deletion entirely** (compliance use case). |
| `delete_batch_size` | u32 | `10_000` | — | — | Rows per sweep batch. Keep around 10k unless your I/O is constrained. |

---

## `security`

### `security.oidc`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `enabled` | bool | `false` | — | — | Master OIDC switch. Off ⇒ JWT or API key only. |
| `provider_url` | string? | `null` | — | — | Discovery endpoint (e.g. `https://your-tenant.auth0.com`). The discovery doc is cached per `ORP_OIDC_DISCOVERY_TTL_SECS` (default 1 h). |
| `client_id` | string? | `null` | — | — | OIDC client ID. |
| `client_secret` | string? | `null` | `ORP_OIDC_CLIENT_SECRET` (via `${env.ORP_OIDC_CLIENT_SECRET}` substitution) | — | OIDC client secret. **Never commit literal secrets — always use `${env.…}` substitution.** |
| `scopes` | string[] | `["openid","profile"]` | — | — | OIDC scopes requested. |
| `redirect_uri` | string? | `null` | — | — | OIDC callback URL. |

Other OIDC tunables (env-var only):
- `ORP_OIDC_DISCOVERY_TTL_SECS` — discovery doc TTL (default 3600).

### `security.abac`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `enabled` | bool | `true` | — | — | ABAC enforcement on every request. Disable only for local dev — `--no-auth` on `start` flips this to permissive automatically. |
| `policy_file` | string? | `null` | — | — | Path to a YAML policy file. When unset, the binary's built-in `default_production` (or `default_permissive` in dev) policy applies. |

### `security.signing`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `algorithm` | string | `"Ed25519"` | — | — | Audit-log signing algorithm. Ed25519 is the only one supported today; rotation is handled out-of-band. |
| `private_key_path` | string? | `null` | — | — | Path to the Ed25519 private key. When unset, ORP generates one on first start and stores it in the working directory. |

---

## `api`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `rate_limit_per_minute` | u32 | `1000` | — | — | Per-IP rate limit. The implementation uses a token-bucket at ~100 req/sec; this field is informational. |
| `cors_enabled` | bool | `true` | — | — | Whether to apply the CORS middleware. |
| `cors_allowed_origins` | string[] | `["*"]` | `ORP_CORS_ORIGINS` (overrides) | — | Allow-listed origins. **Production should never use `*`** — set explicit origins. |
| `api_key_header` | string | `"X-API-Key"` | — | — | Header name for API-key auth (some proxies rewrite headers; this lets you align). |
| `jwt_secret` | string? | `null` | `JWT_SECRET` (read directly by `JwtService::from_env`) | — | HMAC secret for legacy JWTs. Use `${env.JWT_SECRET}` substitution; never commit literals. |

---

## `frontend`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `enabled` | bool | `true` | — | `--headless` (on `start`) sets to false | Whether to serve the React frontend bundle. `--headless` skips it. |
| `port` | u16 | `9090` | — | — | Reserved — the frontend is served on the same port as the API. Field exists for future split-port deployments. |
| `assets_path` | string | `"./frontend/dist"` | — | — | Where the built frontend assets live. The release binary normally embeds them; this is for development. |
| `default_map_center` | `[f64; 2]` | `[51.92, 4.27]` | — | — | `[lat, lon]` — Rotterdam by default. |
| `default_zoom` | u8 | `8` | — | — | Initial Leaflet zoom level. |

---

## `logging`

| Field | Type | Default | Env var | CLI flag | Description |
|-------|------|---------|---------|----------|-------------|
| `level` | string | `"info"` | `RUST_LOG` (overrides everything) | — | `error`/`warn`/`info`/`debug`/`trace`. |
| `format` | string | `"json"` | — | — | `json` or `pretty`. JSON is the supported logging format for production. |
| `output` | string | `"stdout"` | — | — | `stdout` or a file path. |
| `audit_log_path` | string | `"./audit.log"` | — | — | On-disk hash-chained audit log. |

---

## `connectors[]`

Each entry:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | (required) | Human-readable identifier. |
| `type` | string | (required) | One of `ais`, `adsb`, `aisstream`, `nmea`, `nmea2000`, `mavlink`, `cot`, `stix`, `nffi`, `cef`, `opcua`, `modbus`, `mqtt`, `sparkplugb`, `dnp3`, `canbus`, `bacnet`, `lorawan`, `syslog`, `pcap`, `zeek`, `netflow`, `metar`, `cap`, `grib`, `gtfs`, `http_poll`, `websocket`, `csv_watcher`, `database`, `geojson`, `generic_api`. (See [README.md](../README.md#protocol-support).) |
| `enabled` | bool | (required) | Whether to start this connector at boot. |
| `url` | string? | `null` | Transport URL (e.g. `tcp://127.0.0.1:30005`, `udp://0.0.0.0:14550`, `wss://stream.example.com`, `file:///var/log/zeek/conn.log`). Whether it's required depends on the connector type. |
| `entity_type` | string | (required) | What ORP labels emitted entities (e.g. `ship`, `aircraft`, `host`). |
| `trust_score` | f64 | (required) | 0.0–1.0. How confidently this source's data should win conflict resolution. |
| `schedule` | string? | `null` | For polling connectors (`http_poll`, `database`): cron expression or `every Ns/Nm`. |
| `headers` | map<string,string> | `{}` | HTTP headers (for HTTP-style connectors). |
| `retry_policy.max_retries` | u32 | (none) | Connector retry count on transient failure. |
| `retry_policy.backoff_ms` | u64 | (none) | Initial backoff in ms. |
| `mapping` | map<string, JSON> | `{}` | Connector-specific mapping config (e.g. JSONPath → entity property, Modbus register list). See per-adapter docs in [`docs/CONNECTOR_GUIDE.md`](CONNECTOR_GUIDE.md). |

---

## `entity_resolution`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Master switch. |
| `phase` | string | `"structural"` | `"structural"` (Phase 1 — exact match on `fields`) or `"probabilistic"` (Phase 2 — ML scorer; see `probabilistic.*`). |
| `structural.fields` | string[] | `["mmsi","icao_hex"]` | Fields used as natural keys. Two events with the same value in any of these fields collapse to the same entity. |
| `probabilistic.enabled` | bool | `false` | Enable ML scoring. Requires `model_path` to point at a trained model. |
| `probabilistic.model_path` | string? | `null` | Path to a serialized model (`.bin` for the bundled `IsolationForestScorer`). |
| `probabilistic.confidence_threshold` | f64 | `0.85` | Minimum confidence to merge two candidate entities. |

---

## `monitors[]`

Each entry:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `rule_id` | string | (required) | Stable identifier for the rule (used in alerts). |
| `name` | string | (required) | Display name. |
| `entity_type` | string | (required) | Which entity type to evaluate against. |
| `condition` | string | (required) | A simple `<property> <op> <value>` expression. Operators: `>`, `<`, `>=`, `<=`, `=`/`==`, `!=`. Value must parse as `f64`. (For richer logic, embed an ORP-QL query — see [`RECIPES.md` §8](RECIPES.md#8-run-a-saved-query-as-a-continuous-alert).) |
| `action` | string | (required) | Today only `"alert"` is implemented — fires on the WebSocket and stores in the alerts table. |
| `action_target` | string? | `null` | Reserved (for webhooks/email). |
| `enabled` | bool | (required) | Whether to load this rule at boot. |

---

## `templates[]`

Pre-baked bundles of connectors. Built-in: `maritime`. You normally don't edit this section directly — use `orp start --template maritime` to apply.

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Template name (e.g. `"maritime"`). |
| `description` | string | Free-form. |
| `connectors` | string[] | Names of connectors from the `connectors:` array to include. |
| `sample_data_ttl_hours` | u32 | How long demo data lives. |

---

## CLI Flags Quick Reference

Global (work on every subcommand):

| Flag | Env | Description |
|------|-----|-------------|
| `--host <URL>` | `ORP_HOST` | ORP server URL for client commands. Default `http://localhost:9090`. |
| `-q`, `--quiet` | — | Suppress non-essential output. |
| `-o`, `--output <FORMAT>` | — | Override subcommand output format (`table`, `json`, `csv`). |

Specific to `orp start`:

| Flag | Description |
|------|-------------|
| `--port <N>` | Override `server.port`. |
| `--config <PATH>` | Path to `config.yaml`. |
| `--template <NAME>` | Use a pre-baked template (currently only `maritime`). |
| `--dev` | Permissive auth, verbose logging. |
| `--headless` | API + WebSocket only — skip the React frontend. |
| `--no-auth` | Implies `--dev`; sets `ORP_DEV_MODE=true`. |
| `--in-memory` | Use an in-memory DuckDB; state vanishes on shutdown. |

Specific to `orp doctor`:

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Validate this config alongside the host checks. |
| `--https-url <URL>` | Probe TLS/cert chain by hitting `<URL>`. Skipped when omitted. |

---

## Environment Variables

ORP-specific env vars (in addition to per-field overrides above):

| Variable | Used by | Description |
|----------|---------|-------------|
| `ORP_HOST` | CLI | Default `--host` for client commands. |
| `ORP_DEV_MODE` | server start | When `"true"`/`"1"`, permissive auth + dev mode. Implied by `--dev`/`--no-auth`. Refused outside dev `ORP_ENV` values for safety. |
| `ORP_ENV` | server start | One of `development`/`dev`/`test`/`ci`/`staging`/`production`. Gates `ORP_DEV_MODE`: setting `ORP_DEV_MODE=true` in production refuses to boot with a loud error. |
| `ORP_CORS_ORIGINS` | server | Override `api.cors_allowed_origins` (comma-separated). |
| `ORP_FED_BASE_INTERVAL_SECS` | federation | Per-peer sync interval on success (default 30). |
| `ORP_FED_MAX_INTERVAL_SECS` | federation | Adaptive backoff cap on failure (default 600). |
| `ORP_OIDC_DISCOVERY_TTL_SECS` | OIDC | TTL for cached discovery doc (default 3600). |
| `ORP_TRACK_LEN` | analytics | Per-entity history buffer size (default 50). Lower = less RAM. |
| `JWT_SECRET` | JWT | HMAC secret for legacy JWTs. |
| `AISSTREAM_API_KEY` | AISStream connector | When set, the live AISStream WebSocket connector activates instead of the demo. |
| `RUST_LOG` | tracing | Standard tracing-subscriber filter, e.g. `info,orp_connector=debug`. Supersedes `logging.level` / `server.log_level`. |
| `NO_COLOR` / `CLICOLOR_FORCE` | CLI | Disable / force ANSI colour output. |

---

## Worked Example — Production-ish `config.yaml`

```yaml
server:
  host: "0.0.0.0"
  port: 9090
  log_level: "info"

storage:
  duckdb:
    path: "/var/lib/orp/data.duckdb"
    memory_limit_gb: 8
  rocksdb:
    path: "/var/lib/orp/state.db"
  sqlite:
    path: "/var/lib/orp/config.sqlite"

retention:
  events_ttl_days: 90
  audit_log_ttl_days: 0     # never delete

security:
  oidc:
    enabled: true
    provider_url: "https://auth.example.com/realms/orp"
    client_id: "orp-prod"
    client_secret: "${env.ORP_OIDC_CLIENT_SECRET}"
    redirect_uri: "https://orp.example.com/auth/callback"
  abac:
    enabled: true
    policy_file: "/etc/orp/abac.yaml"

api:
  cors_enabled: true
  cors_allowed_origins:
    - "https://app.example.com"
  jwt_secret: "${env.JWT_SECRET}"

logging:
  level: "info"
  format: "json"
  output: "stdout"
  audit_log_path: "/var/lib/orp/audit.log"

connectors:
  - name: "aisstream"
    type: "aisstream"
    enabled: true
    entity_type: "ship"
    trust_score: 0.95

  - name: "adsb-local"
    type: "adsb"
    enabled: true
    url: "tcp://127.0.0.1:30005"
    entity_type: "aircraft"
    trust_score: 0.9

monitors:
  - rule_id: "fast-ship"
    name: "Fast vessel"
    entity_type: "ship"
    condition: "speed > 25"
    action: "alert"
    enabled: true
```

Run it:

```bash
ORP_OIDC_CLIENT_SECRET=... JWT_SECRET=... orp start --config config.yaml
```
