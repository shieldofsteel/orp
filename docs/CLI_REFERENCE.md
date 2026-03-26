# ORP CLI Reference

> **Open Reality Protocol** — Palantir-grade data fusion in a single binary.  
> Version: 0.1.0 · [GitHub](https://github.com/shieldofsteel/orp) · [API Reference](./API_DOCUMENTATION.md) · [Architecture](../ARCHITECTURE.md)

---

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Command Reference](#command-reference)
  - [orp start](#orp-start)
  - [orp query](#orp-query)
  - [orp status](#orp-status)
  - [orp connectors](#orp-connectors)
  - [orp entities](#orp-entities)
  - [orp events](#orp-events)
  - [orp monitors](#orp-monitors)
  - [orp config](#orp-config)
  - [orp version](#orp-version)
  - [orp completions](#orp-completions)
- [Output Formats](#output-formats)
- [Environment Variables](#environment-variables)
- [Exit Codes](#exit-codes)
- [Configuration File](#configuration-file)
- [Shell Completion](#shell-completion)
- [Piping & Scripting](#piping--scripting)
- [AI Agent Integration](#ai-agent-integration)
- [Troubleshooting](#troubleshooting)

---

## Installation

### From Releases (recommended)

```bash
# macOS (Apple Silicon)
curl -L https://github.com/shieldofsteel/orp/releases/latest/download/orp-aarch64-apple-darwin.tar.gz | tar -xz
sudo mv orp /usr/local/bin/

# macOS (Intel)
curl -L https://github.com/shieldofsteel/orp/releases/latest/download/orp-x86_64-apple-darwin.tar.gz | tar -xz
sudo mv orp /usr/local/bin/

# Linux (x86_64)
curl -L https://github.com/shieldofsteel/orp/releases/latest/download/orp-x86_64-unknown-linux-gnu.tar.gz | tar -xz
sudo mv orp /usr/local/bin/
```

### From Source

```bash
git clone https://github.com/shieldofsteel/orp
cd orp
cargo build --release
# Binary at: target/release/orp
sudo install -m 755 target/release/orp /usr/local/bin/orp
```

### Verify Installation

```bash
orp --version
# orp 0.1.0

orp --help
# ORP — Open Reality Protocol: Palantir-grade data fusion in a single binary
```

---

## Quick Start

```bash
# 1. Start with the maritime template (AIS ships, ports, monitors pre-configured)
orp start --template maritime

# 2. In another terminal — check the server is healthy
orp status

# 3. Run your first ORP-QL query
orp query "MATCH (s:ship) WHERE s.speed > 15 RETURN s.name, s.speed, s.position LIMIT 10"

# 4. View the live dashboard
open http://localhost:9090
```

---

## Command Reference

### Global Flags

These flags apply to every command:

| Flag | Short | Description |
|------|-------|-------------|
| `--help` | `-h` | Show help for a command |
| `--version` | `-V` | Print version number and exit |

---

### `orp start`

Start the ORP server with all services: HTTP API, WebSocket hub, AIS connectors, DuckDB storage, monitor engine, and embedded React dashboard.

**Synopsis**

```
orp start [OPTIONS]
```

**Options**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--config <PATH>` | `-c` | path | `config.yaml` | Path to a YAML config file |
| `--template <NAME>` | `-t` | string | — | Use a pre-configured template (e.g. `maritime`) |
| `--port <PORT>` | `-p` | u16 | `9090` | Override the server port |
| `--dev` | — | bool | `false` | Enable dev mode (permissive auth, verbose logging) |

**Examples**

```bash
# Start with all defaults (loads config.yaml if present, else uses built-in defaults)
orp start

# Start on a custom port
orp start --port 8080

# Start with the maritime template (ships + ports pre-loaded, AIS demo active)
orp start --template maritime

# Start with a custom config file
orp start --config /etc/orp/production.yaml

# Start in dev mode (auth is permissive — no JWT required)
orp start --dev

# Alternatively, enable dev mode via environment variable
ORP_DEV_MODE=true orp start

# Start with a JWT secret (production mode)
JWT_SECRET=supersecretkey orp start --config config.yaml
```

**What Happens at Startup**

```
  ╔═══════════════════════════════════════════════════════════╗
  ║                                                           ║
  ║   ██████╗ ██████╗ ██████╗                                 ║
  ║  ██╔═══██╗██╔══██╗██╔══██╗                                ║
  ║  ██║   ██║██████╔╝██████╔╝                                ║
  ║  ██║   ██║██╔══██╗██╔═══╝                                 ║
  ║  ╚██████╔╝██║  ██║██║                                     ║
  ║   ╚═════╝ ╚═╝  ╚═╝╚═╝                                    ║
  ║                                                           ║
  ║  Open Reality Protocol v0.1.0                             ║
  ║  Palantir-grade data fusion in a single binary            ║
  ║                                                           ║
  ╚═══════════════════════════════════════════════════════════╝

[INFO] Initializing DuckDB storage...
[INFO] Loading demo port data...  (10 synthetic ports)
[INFO] AIS connector started (demo mode)
[INFO] Monitor engine initialized with 1 rules
[INFO] Starting HTTP server on 0.0.0.0:9090
[INFO] Dashboard: http://localhost:9090/
[INFO] API:       http://localhost:9090/api/v1/
[INFO] Health:    http://localhost:9090/api/v1/health
```

**Services Started**

| Service | URL | Notes |
|---------|-----|-------|
| React Dashboard | `http://localhost:9090/` | Deck.gl live map |
| REST API | `http://localhost:9090/api/v1/` | OpenAPI 3.1 |
| WebSocket | `ws://localhost:9090/ws/updates` | Real-time entity + alert events |
| Health endpoint | `http://localhost:9090/api/v1/health` | No auth required |

**Auth Modes**

| Mode | How to Enable | Behaviour |
|------|--------------|-----------|
| Dev mode | `--dev` flag or `ORP_DEV_MODE=true` | All requests accepted, no JWT needed |
| Production | `JWT_SECRET=<secret>` | Bearer token required on all endpoints except `/health` |
| Locked | Neither set | Server starts but rejects all API requests (safe default) |

---

### `orp query`

Execute an **ORP-QL** query against a running ORP instance at `http://localhost:9090`.

**Synopsis**

```
orp query [OPTIONS] [QUERY]
```

The query can be provided as a positional argument (inline) or read from a file with `--file`.

**Options**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `[QUERY]` | — | string | — | The ORP-QL query string (positional, inline) |
| `--file <PATH>` | `-f` | path | — | Read the query from a file |
| `--output <FORMAT>` | `-o` | `table\|json\|csv` | `table` | Output format |

**ORP-QL Syntax Quick Reference**

```
MATCH (<alias>:<EntityType>)
  [WHERE <condition> [AND <condition>]*]
  [NEAR <lat>, <lon> WITHIN <distance>km]
RETURN <alias>.<field>[, <alias>.<field>]*
  [ORDER BY <alias>.<field> [ASC|DESC]]
  [LIMIT <n>]
```

**Examples**

```bash
# Find all ships currently tracked (inline query, default table output)
orp query "MATCH (s:ship) RETURN s.name, s.mmsi, s.position LIMIT 20"

# Ships exceeding speed threshold
orp query "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.speed ORDER BY s.speed DESC"

# Ships near a location (within 50km of Rotterdam)
orp query "MATCH (s:ship) NEAR 51.9, 4.5 WITHIN 50km RETURN s.name, s.speed"

# Output as JSON
orp query --output json "MATCH (s:ship) RETURN s.name, s.speed"

# Output as CSV
orp query --output csv "MATCH (s:ship) RETURN s.name, s.mmsi, s.speed"

# Read query from a file
orp query --file queries/fast_ships.ql

# Pipe JSON output into jq
orp query --output json "MATCH (s:ship) RETURN s" | jq '.results[] | select(.speed > 25) | .name'
```

**Output: `--output table` (default)**

```
name                     │ speed
─────────────────────────┼──────
MV Atlantic Pioneer      │ 24.3
Vessel Horizon           │ 21.8

2 rows in 12.0ms
```

**Output: `--output json`**

```json
{
  "query": "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.speed",
  "results": [
    { "name": "MV Atlantic Pioneer", "speed": 24.3 },
    { "name": "Vessel Horizon", "speed": 21.8 }
  ],
  "columns": ["name", "speed"],
  "metadata": {
    "rows_returned": 2,
    "execution_time_ms": 12.0
  }
}
```

**Output: `--output csv`**

```
name,speed
MV Atlantic Pioneer,24.3
Vessel Horizon,21.8
```

**Error When Server is Down**

```
✗ ORP server is not running. Start it with `orp start`
```

(Without color/`NO_COLOR` set: `ERROR: ORP server is not running. Start it with 'orp start'`)

---

### `orp status`

Check health and status of a running ORP instance.

**Synopsis**

```
orp status
```

**Options**

None. Connects to `http://localhost:9090/api/v1/health`.

**Examples**

```bash
# Check if server is running
orp status

# Use in a script — check exit code
orp status && echo "ORP is up" || echo "ORP is down"

# Monitor in a loop
watch -n 5 orp status
```

**Output (server running)**

```
ORP Status

  Status:  ● healthy
  Version: 0.1.0
  Uptime:  1h 4m 2s

Components
  ● storage (2.1ms)
  ● query_engine (0.8ms)
```

(Without color/`NO_COLOR` set: status indicator shows as `healthy` / `ERR`, components as `OK name` / `ERR name`)

**Output (server not running)**

```
✗ ORP server is not running.
Start with: orp start --template maritime
```

---

### `orp connectors`

Manage data source connectors (AIS, ADS-B, MQTT, HTTP, etc.).

**Synopsis**

```
orp connectors <SUBCOMMAND>
```

**Subcommands**

| Subcommand | Description |
|-----------|-------------|
| `list` | List all registered connectors with status |
| `add` | Register a new connector |
| `remove <ID>` | Remove a connector by ID |

---

#### `orp connectors list`

List all registered connectors with their type, enabled status, and ID.

**Synopsis**

```
orp connectors list
```

**Examples**

```bash
orp connectors list
```

**Output**

```
Connectors

  ● AIS Demo Feed [ais] — ais-demo
```

(Green `●` = enabled, red `●` = disabled. Without color: `ON` / `OFF`)

---

#### `orp connectors add`

Register a new data source connector.

**Synopsis**

```
orp connectors add --name <NAME> --connector-type <TYPE> --entity-type <TYPE> [--trust-score <SCORE>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | required | Connector name |
| `--connector-type <TYPE>` | string | required | Connector type (`ais`, `adsb`, `http`, `mqtt`) |
| `--entity-type <TYPE>` | string | required | Entity type this connector produces |
| `--trust-score <SCORE>` | f64 | `0.8` | Trust score for data from this connector (0.0–1.0) |

**Examples**

```bash
# Register an AIS connector
orp connectors add \
  --name "AIS Live" \
  --connector-type ais \
  --entity-type ship \
  --trust-score 0.95

# Register an HTTP polling connector
orp connectors add \
  --name "Weather API" \
  --connector-type http \
  --entity-type weather_cell
```

**Output**

```
✓ Connector 'AIS Live' registered.
```

---

#### `orp connectors remove`

Remove a registered connector by its ID.

**Synopsis**

```
orp connectors remove <ID>
```

**Arguments**

| Argument | Description |
|----------|-------------|
| `<ID>` | Connector ID to remove (positional) |

**Examples**

```bash
orp connectors remove ais-demo
```

**Output**

```
✓ Connector 'ais-demo' removed.
```

---

### `orp entities`

Manage and query tracked entities.

**Synopsis**

```
orp entities <SUBCOMMAND>
```

**Subcommands**

| Subcommand | Description |
|-----------|-------------|
| `search` | Search entities with optional geo and type filters |
| `get <ID>` | Get a specific entity by ID |

---

#### `orp entities search`

Search tracked entities with optional geographic proximity, type, and limit filters.

**Synopsis**

```
orp entities search [OPTIONS]
```

**Options**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--near <LAT,LON>` | — | string | — | Search near a location, e.g. `51.9,4.5` |
| `--radius <KM>` | — | f64 | `50.0` | Radius in km (used with `--near`) |
| `--entity-type <TYPE>` | `-t` | string | — | Filter by entity type (e.g. `ship`, `aircraft`) |
| `--limit <N>` | `-l` | usize | `100` | Maximum number of results |
| `--output <FORMAT>` | `-o` | `table\|json\|csv` | `table` | Output format |

**Examples**

```bash
# Search all entities (up to 100)
orp entities search

# Ships only
orp entities search --entity-type ship

# Ships near Rotterdam within 30km
orp entities search --near 51.9,4.5 --radius 30 --entity-type ship

# Get 10 results as JSON
orp entities search --limit 10 --output json

# CSV export
orp entities search --entity-type ship --output csv > ships.csv
```

**Output (`--output table`)**

```
id                  │ type │ name                │ confidence
────────────────────┼──────┼─────────────────────┼───────────
ship-123456789      │ ship │ MV Atlantic Pioneer  │ 0.95
ship-987654321      │ ship │ Vessel Horizon       │ 0.90

2 entities found
```

---

#### `orp entities get`

Get a specific entity by its ID.

**Synopsis**

```
orp entities get <ID> [OPTIONS]
```

**Arguments**

| Argument | Description |
|----------|-------------|
| `<ID>` | Entity ID (positional) |

**Options**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--output <FORMAT>` | `-o` | `table\|json\|csv` | `json` | Output format |

**Examples**

```bash
# Get entity as JSON (default)
orp entities get ship-123456789

# Explicit format
orp entities get ship-123456789 --output json
```

**Output**

```json
{
  "id": "ship-123456789",
  "entity_type": "ship",
  "properties": {
    "name": "MV Atlantic Pioneer",
    "mmsi": "123456789",
    "speed": 24.3,
    "heading": 270.0
  },
  "confidence": 0.95,
  "last_updated": "2024-01-15T10:30:00Z"
}
```

---

### `orp events`

View entity events ingested by ORP.

**Synopsis**

```
orp events [OPTIONS]
```

**Options**

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--entity <ID>` | — | string | — | Filter events by entity ID |
| `--since <TIME>` | — | string | — | Only show events since (e.g. `1h`, `30m`, `2d`, or ISO 8601 date) |
| `--limit <N>` | `-l` | usize | `50` | Maximum number of events returned |
| `--output <FORMAT>` | `-o` | `table\|json\|csv` | `table` | Output format |

**Relative time values for `--since`:** `<N>h` (hours), `<N>m` (minutes), `<N>d` (days).

**Examples**

```bash
# View the last 50 events (default)
orp events

# Events for a specific entity
orp events --entity ship-123456789

# Events in the last hour
orp events --since 1h

# Events in the last 30 minutes for a specific ship, as JSON
orp events --entity ship-123456789 --since 30m --output json

# Last 200 events as CSV
orp events --limit 200 --output csv > events.csv
```

**Output (`--output table`)**

```
id          │ entity_id        │ event_type      │ timestamp
────────────┼──────────────────┼─────────────────┼─────────────────────
evt-001     │ ship-123456789   │ position_update │ 2024-01-15T10:30:00Z
evt-002     │ ship-987654321   │ position_update │ 2024-01-15T10:30:01Z
```

---

### `orp monitors`

Manage monitor rules that evaluate entity properties and fire alerts.

**Synopsis**

```
orp monitors <SUBCOMMAND>
```

**Subcommands**

| Subcommand | Description |
|-----------|-------------|
| `list` | List all monitor rules |
| `add` | Add a new monitor rule |
| `remove <ID>` | Remove a monitor rule by ID |

---

#### `orp monitors list`

List all configured monitor rules.

**Synopsis**

```
orp monitors list
```

**Examples**

```bash
orp monitors list
```

**Output** — raw JSON from the API:

```json
{
  "data": [
    {
      "rule_id": "high_speed_ship",
      "name": "Ship exceeding speed limit",
      "entity_type": "ship",
      "condition": { "type": "property_threshold", "property": "speed", "operator": ">", "value": 25 },
      "severity": "warning",
      "enabled": true
    }
  ],
  "count": 1
}
```

---

#### `orp monitors add`

Add a new monitor rule.

**Synopsis**

```
orp monitors add --name <NAME> --entity-type <TYPE> --condition <EXPR> [--severity <LEVEL>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name <NAME>` | string | required | Human-readable monitor name |
| `--entity-type <TYPE>` | string | required | Entity type to monitor (e.g. `ship`) |
| `--condition <EXPR>` | string | required | Condition expression (e.g. `"speed > 25"`) |
| `--severity <LEVEL>` | string | `warning` | Severity level: `info`, `warning`, `critical` |

**Condition syntax:** `<property> <operator> <value>` where operators are `>`, `<`, `>=`, `<=`, `=`, `!=`.

**Examples**

```bash
# Alert when a ship exceeds 25 knots
orp monitors add \
  --name "High Speed Ship" \
  --entity-type ship \
  --condition "speed > 25" \
  --severity warning

# Critical alert for extreme speed
orp monitors add \
  --name "Dangerously Fast Ship" \
  --entity-type ship \
  --condition "speed > 40" \
  --severity critical
```

**Output**

```
✓ Monitor 'High Speed Ship' created.
```

---

#### `orp monitors remove`

Remove a monitor rule by its ID.

**Synopsis**

```
orp monitors remove <ID>
```

**Arguments**

| Argument | Description |
|----------|-------------|
| `<ID>` | Monitor rule ID (positional) |

**Examples**

```bash
orp monitors remove high_speed_ship
```

**Output**

```
✓ Monitor 'high_speed_ship' removed.
```

---

### `orp config`

Manage and validate ORP configuration files.

**Synopsis**

```
orp config <SUBCOMMAND>
```

**Subcommands**

| Subcommand | Description |
|-----------|-------------|
| `validate <FILE>` | Validate a configuration file without starting the server |

---

#### `orp config validate`

Validate a YAML configuration file and report any errors.

**Synopsis**

```
orp config validate <FILE>
```

**Arguments**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the config file to validate (positional) |

**Examples**

```bash
# Validate before deploying
orp config validate config.yaml

# Validate a production config
orp config validate /etc/orp/production.yaml
```

**Output (valid)**

```
Validating: config.yaml

✓ Configuration is valid.
```

**Output (invalid)**

```
Validating: config.yaml

✗ Configuration invalid: missing field `server.port` at line 3
```

Exits with code `1` if the configuration is invalid.

---

### `orp version`

Show ORP version and build information.

**Synopsis**

```
orp version
```

**Examples**

```bash
orp version
```

**Output**

```
ORP — Open Reality Protocol

  Version:  0.1.0
  Edition:  Rust 2021
  Target:   aarch64
  OS:       macos
```

> **Note:** `orp --version` (the global flag) prints only the short version string (`orp 0.1.0`). `orp version` (the subcommand) prints full build info.

---

### `orp completions`

Generate shell completion scripts for ORP.

**Synopsis**

```
orp completions <SHELL>
```

**Arguments**

| Argument | Description |
|----------|-------------|
| `<SHELL>` | Shell to generate completions for: `bash`, `zsh`, `fish`, `powershell`, `elvish` |

**Examples**

```bash
# Bash
orp completions bash > /etc/bash_completion.d/orp
source /etc/bash_completion.d/orp

# Zsh
orp completions zsh > "${fpath[1]}/_orp"

# Fish
orp completions fish > ~/.config/fish/completions/orp.fish

# PowerShell
orp completions powershell | Out-File -Encoding utf8 $PROFILE.CurrentUserAllHosts
```

**What completions provide:**

```bash
orp <TAB>
# start  query  status  connectors  entities  events  monitors  config  version  completions  help

orp start --<TAB>
# --config  --template  --port  --dev  --help

orp query --<TAB>
# --file  --output  --help

orp connectors <TAB>
# list  add  remove

orp entities <TAB>
# search  get

orp monitors <TAB>
# list  add  remove

orp config <TAB>
# validate
```

---

## Output Formats

Commands that support `--output` accept three formats: `table` (default), `json`, and `csv`.

| Format | Flag | Best For |
|--------|------|----------|
| `table` | `--output table` | Human reading in terminal |
| `json` | `--output json` | Piping to `jq`, scripting, AI agents |
| `csv` | `--output csv` | Spreadsheets, data pipelines |

Commands with `--output` support: `query`, `entities search`, `entities get`, `events`.

### Pipe JSON into jq

```bash
orp query --output json "MATCH (s:ship) RETURN s.name, s.speed" \
  | jq '.results[] | select(.speed > 25) | .name'
```

### CSV export

```bash
orp entities search --entity-type ship --output csv > ships.csv

orp events --limit 1000 --output csv > audit.csv
```

### Colored output

ORP uses color in terminal output by default (success = green ✓, errors = red ✗, headers = cyan bold). Color is automatically disabled when:
- `NO_COLOR` is set in the environment (any value)
- Output is piped (the `colored` crate respects TTY detection)

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ORP_DEV_MODE` | `false` | Set to `true` or `1` to enable permissive auth. Equivalent to `orp start --dev`. **Never use in production.** |
| `JWT_SECRET` | — | Secret key for HS256 JWT signing. Minimum 32 chars. Required in production unless `ORP_DEV_MODE=true`. |
| `NO_COLOR` | — | When set (any value), disables ANSI colour in all terminal output ([CLIG compliant](https://no-color.org)). |
| `RUST_LOG` | `info` | Logging filter for the `tracing` crate. Values: `error`, `warn`, `info`, `debug`, `trace`. |

**Usage Examples**

```bash
# Development — no auth
orp start --dev
# or
ORP_DEV_MODE=true orp start

# Production — JWT required
JWT_SECRET="$(openssl rand -hex 32)" orp start --config /etc/orp/prod.yaml

# Disable all colour output (for CI/log files)
NO_COLOR=1 orp status

# Verbose debug logging
RUST_LOG=debug orp start
```

> **Note:** Port and config path are configured via `--port` / `--config` flags or in the config file. There are no `ORP_PORT` or `ORP_CONFIG` environment variables read directly by the CLI binary.

---

## Exit Codes

ORP follows standard UNIX conventions:

| Code | Meaning | When |
|------|---------|------|
| `0` | Success | Command completed without error |
| `1` | General error | Unrecoverable error (config invalid, storage init failed, etc.) |
| `2` | Usage error | Invalid arguments, unknown flag, missing required option |
| `130` | Interrupted | Process received SIGINT (Ctrl-C) |

**In Scripts**

```bash
# Check if server is alive before querying
if ! orp status > /dev/null 2>&1; then
  echo "ORP is not running — aborting pipeline" >&2
  exit 1
fi

# Run query only if server is up
orp status > /dev/null && orp query --output json "MATCH (s:ship) RETURN s.name"

# Validate config before starting
orp config validate config.yaml && orp start --config config.yaml
```

---

## Configuration File

The config file is YAML. ORP loads `config.yaml` in the current directory by default, or the path passed via `--config`.

### Minimal Config

```yaml
server:
  host: "0.0.0.0"
  port: 9090
```

### Full Production Config

```yaml
server:
  host: "0.0.0.0"
  port: 9090
  workers: 8
  log_level: "info"

storage:
  duckdb:
    path: "/data/orp.duckdb"
    memory_limit_gb: 16
    max_connections: 20

security:
  abac:
    enabled: true
  signing:
    algorithm: "Ed25519"
    private_key_path: "/etc/orp/signing.key"

connectors:
  - name: "ais_live"
    type: "ais"
    enabled: true
    url: "tcp://ais.example.com:5631"
    entity_type: "ship"
    trust_score: 0.95

  - name: "adsb_live"
    type: "adsb"
    enabled: true
    url: "tcp://adsb-receiver.local:30002"
    entity_type: "aircraft"
    trust_score: 0.90

monitors:
  - rule_id: "high_speed_ship"
    name: "Ship exceeding speed limit"
    entity_type: "ship"
    condition: "speed > 25"
    action: "alert"
    enabled: true

api:
  rate_limit_per_minute: 1000
  cors_enabled: true
  cors_allowed_origins:
    - "https://command.yourcompany.com"
  jwt_secret: "${env.JWT_SECRET}"

logging:
  level: "info"
  format: "json"
  output: "stdout"
```

### Available Templates

| Template | Description |
|----------|-------------|
| `maritime` | AIS ship tracking with demo port data, speed monitor pre-configured |

Use with: `orp start --template maritime`

---

## Shell Completion

Shell completions are generated directly by the CLI (`orp completions <shell>`) — see [`orp completions`](#orp-completions) above.

### Quick Install

```bash
# Bash (system-wide)
orp completions bash | sudo tee /etc/bash_completion.d/orp

# Zsh
orp completions zsh > "${fpath[1]}/_orp"

# Fish
orp completions fish > ~/.config/fish/completions/orp.fish

# PowerShell
orp completions powershell | Out-File -Encoding utf8 $PROFILE.CurrentUserAllHosts
```

---

## Piping & Scripting

ORP is designed to compose cleanly with standard UNIX tools. Use `--output json` for machine-readable output, `--output csv` for data pipelines.

### Pipeline Examples

```bash
# Count active ships
orp query --output json "MATCH (s:ship) RETURN s.mmsi" | jq '.results | length'

# Extract just names as newline-separated list
orp query --output json "MATCH (s:ship) RETURN s.name" | jq -r '.results[].name'

# Find the fastest ship
orp query --output json "MATCH (s:ship) RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 1" \
  | jq -r '.results[0].name'

# Ships over speed threshold, piped as CSV
orp query --output csv "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.mmsi, s.speed"

# Save entity snapshot
orp entities search --entity-type ship --output json \
  | jq '.data' > snapshots/ships-$(date +%Y%m%d-%H%M%S).json

# Chain status check + query
orp status > /dev/null 2>&1 \
  && orp query --output json "MATCH (s:ship) RETURN s.name, s.speed" \
  | jq '.results[] | select(.speed > 20) | .name' \
  || echo "Server offline"
```

### In CI/CD Pipelines

```bash
#!/bin/bash
# health-check.sh — verify ORP is healthy after deploy

set -euo pipefail

MAX_RETRIES=10
RETRY_DELAY=3

for i in $(seq 1 $MAX_RETRIES); do
  if orp status > /dev/null 2>&1; then
    echo "✅ ORP is healthy (attempt $i/$MAX_RETRIES)"
    exit 0
  fi
  echo "Waiting for ORP... (attempt $i/$MAX_RETRIES)"
  sleep $RETRY_DELAY
done

echo "❌ ORP failed to become healthy after $MAX_RETRIES attempts" >&2
exit 1
```

### In Makefiles

```makefile
.PHONY: dev prod status query-ships

dev:
	orp start --dev --template maritime

prod:
	JWT_SECRET=$(shell cat .jwt-secret) orp start --config config.prod.yaml

status:
	@orp status

query-ships:
	@orp query "MATCH (s:ship) RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 5"
```

---

## AI Agent Integration

ORP is designed to be used by AI agents (LLMs, Claude, GPT, Gemini) as a data fusion backend.

### Agent Principles

1. **Check server health first** — always run `orp status` before any query.
2. **Use `--output json`** — structured output, parseable without screen-scraping.
3. **Handle errors gracefully** — check exit codes, parse error messages.
4. **Prefer specific queries** — use WHERE clauses and LIMITs to minimize data transferred.

### Agent Tool Implementation (Python)

```python
import subprocess
import json

def orp_query(query: str) -> dict:
    """Execute an ORP-QL query and return parsed JSON results."""
    result = subprocess.run(
        ["orp", "query", "--output", "json", query],
        capture_output=True,
        text=True,
        timeout=30
    )
    if result.returncode != 0:
        return {"error": result.stderr.strip(), "results": []}
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return {"error": "Invalid JSON response", "raw": result.stdout}

def orp_status() -> bool:
    """Check if ORP server is running."""
    result = subprocess.run(["orp", "status"], capture_output=True, timeout=5)
    return result.returncode == 0

# Example agent usage
if orp_status():
    ships = orp_query("MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.speed, s.position LIMIT 5")
    for ship in ships.get("results", []):
        print(f"Fast ship: {ship['name']} at {ship['speed']} knots")
```

### Agent Tool Implementation (TypeScript)

```typescript
import { execSync } from 'child_process';

interface OrpResult {
  results: Record<string, unknown>[];
  columns: string[];
  metadata: { rows_returned: number; execution_time_ms: number };
  error?: string;
}

function orpQuery(query: string): OrpResult {
  try {
    const output = execSync(`orp query --output json "${query.replace(/"/g, '\\"')}"`, {
      encoding: 'utf-8',
      timeout: 30000,
      env: { ...process.env, NO_COLOR: '1' },
    });
    return JSON.parse(output);
  } catch (err: any) {
    return { results: [], columns: [], metadata: { rows_returned: 0, execution_time_ms: 0 }, error: err.message };
  }
}
```

> **Tip:** Set `NO_COLOR=1` when calling ORP from scripts to avoid ANSI codes in captured output.

---

## Troubleshooting

### Server won't start

```bash
# Check if port is in use
lsof -i :9090

# Use a different port
orp start --port 8080

# Validate config before starting
orp config validate config.yaml
```

### Auth errors (401 Unauthorized)

```bash
# Quick fix: enable dev mode via flag
orp start --dev

# Or via env var
ORP_DEV_MODE=true orp start

# Production fix: set JWT_SECRET
export JWT_SECRET="$(openssl rand -hex 32)"
orp start
```

### Query returns empty results

```bash
# Check if the server is seeded with data
orp entities search --limit 5

# If empty, try the maritime template
orp start --template maritime

# Run a broad query to see what types exist
orp query "MATCH (s:ship) RETURN s.name LIMIT 5"
orp query "MATCH (p:port) RETURN p.name LIMIT 5"
```

### Config validation fails

```bash
# Validate before starting
orp config validate config.yaml

# Outputs specific error with file and line number
```

### Verbose logging

```bash
RUST_LOG=debug orp start
```

---

## API Reference

The full REST API is documented in the OpenAPI spec:

```bash
# Browse interactively (requires server running)
open http://localhost:9090/api/v1/docs
```

**Key Endpoints**

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/health` | Health check (no auth) |
| `GET` | `/api/v1/entities` | List entities |
| `GET` | `/api/v1/entities/{id}` | Get entity |
| `GET` | `/api/v1/entities/search` | Search entities |
| `POST` | `/api/v1/query` | Execute ORP-QL |
| `GET` | `/api/v1/events` | List events |
| `GET` | `/api/v1/connectors` | List connectors |
| `POST` | `/api/v1/connectors` | Create connector |
| `DELETE` | `/api/v1/connectors/{id}` | Delete connector |
| `GET` | `/api/v1/monitors` | List monitor rules |
| `POST` | `/api/v1/monitors` | Create monitor rule |
| `DELETE` | `/api/v1/monitors/{id}` | Delete monitor rule |
| `GET` | `/ws/updates` | WebSocket — real-time events |

---

## Roadmap

**v0.2.0 (planned)**
- `orp query --watch` — streaming/live query output
- `orp config init` — interactive config generator
- `orp logs` — tail server logs
- `orp context` — manage multiple ORP instances (like `kubectl config use-context`)

**v0.3.0 (planned)**
- `orp export` — bulk export entities/events to CSV/Parquet
- `orp import` — bulk import from CSV/JSON
- Plugin system for custom connectors

---

*Built with ❤️ using [Rust](https://rust-lang.org), [Clap](https://docs.rs/clap), [Axum](https://docs.rs/axum), [DuckDB](https://duckdb.org), and the principles of [CLIG](https://clig.dev).*
