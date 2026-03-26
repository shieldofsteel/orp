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
- [Output Formats](#output-formats)
- [Environment Variables](#environment-variables)
- [Exit Codes](#exit-codes)
- [Configuration File](#configuration-file)
- [Shell Completion](#shell-completion)
- [Piping & Scripting](#piping--scripting)
- [AI Agent Integration](#ai-agent-integration)
- [Comparison with Similar Tools](#comparison-with-similar-tools)
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

Start the ORP server with all services: HTTP API, WebSocket hub, AIS/ADS-B connectors, DuckDB storage, monitor engine, and embedded React dashboard.

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
ORP_DEV_MODE=true orp start

# Start with a JWT secret (production mode)
JWT_SECRET=supersecretkey orp start --config config.yaml
```

**What Happens at Startup**

```
  ╔═══════════════════════════════════════════════════════════╗
  ║   ██████╗ ██████╗ ██████╗                                 ║
  ║  Open Reality Protocol v0.1.0                             ║
  ║  Palantir-grade data fusion in a single binary            ║
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
| Prometheus metrics | `http://localhost:9090/api/v1/metrics` | Auth required |

**Auth Modes**

| Mode | How to Enable | Behaviour |
|------|--------------|-----------|
| Dev mode | `ORP_DEV_MODE=true` | All requests accepted, no JWT needed |
| Production | `JWT_SECRET=<secret>` | Bearer token required on all endpoints except `/health` |
| Locked | Neither set | Server starts but rejects all API requests (safe default) |

---

### `orp query`

Execute an **ORP-QL** query against a running ORP instance at `http://localhost:9090`.

**Synopsis**

```
orp query --query <ORPQL>
orp query -q <ORPQL>
```

**Options**

| Flag | Short | Type | Required | Description |
|------|-------|------|----------|-------------|
| `--query <STRING>` | `-q` | string | ✅ | The ORP-QL query to execute |

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
# Find all ships currently tracked
orp query -q "MATCH (s:ship) RETURN s.name, s.mmsi, s.position LIMIT 20"

# Ships exceeding speed threshold
orp query -q "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.speed ORDER BY s.speed DESC"

# Ships near a location (within 50km of Rotterdam)
orp query -q "MATCH (s:ship) NEAR 51.9, 4.5 WITHIN 50km RETURN s.name, s.speed"

# Find ships with a specific property
orp query -q "MATCH (s:ship) WHERE s.ship_type = 'cargo' RETURN s.name, s.mmsi"

# Count ships by type
orp query -q "MATCH (s:ship) RETURN s.ship_type, COUNT(s) AS total ORDER BY total DESC"

# Graph traversal — ships at a port
orp query -q "MATCH (s:ship)-[r:DOCKED_AT]->(p:port) RETURN s.name, p.name, r.arrival_time"

# Pipe into jq for filtering
orp query -q "MATCH (s:ship) RETURN s" | jq '.results[] | select(.speed > 25) | .name'

# Store results as JSON
orp query -q "MATCH (s:ship) RETURN s.name, s.speed" > ships.json

# Count results
orp query -q "MATCH (s:ship) RETURN s.mmsi" | jq '.results | length'
```

**Output (raw JSON)**

```json
{
  "query": "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.speed",
  "results": [
    { "name": "MV Atlantic Pioneer", "speed": 24.3 },
    { "name": "Vessel Horizon", "speed": 21.8 }
  ],
  "count": 2,
  "elapsed_ms": 12
}
```

**Error When Server is Down**

```
Error: ORP server is not running. Start it with `orp start`
```

---

### `orp status`

Check health and status of a running ORP instance.

**Synopsis**

```
orp status
```

**Options**

None. Uses the endpoint `http://localhost:9090/api/v1/health`.

**Examples**

```bash
# Check if server is running
orp status

# Use in a script — check exit code
orp status && echo "ORP is up" || echo "ORP is down"

# Pretty-print and extract specific field
orp status | jq '.uptime_seconds'

# Monitor in a loop
watch -n 5 orp status
```

**Output (server running)**

```json
{
  "status": "ok",
  "version": "0.1.0",
  "uptime_seconds": 3842,
  "storage": {
    "backend": "duckdb",
    "entity_count": 427,
    "event_count": 18203
  },
  "connectors": {
    "total": 1,
    "active": 1
  },
  "monitors": {
    "rules": 1,
    "alerts_fired": 3
  }
}
```

**Output (server not running)**

```
ORP server is not running.
Start with: orp start --template maritime
```

---

### `orp connectors`

Manage data source connectors (AIS, ADS-B, MQTT, HTTP poller, CSV watcher, WebSocket client).

**Synopsis**

```
orp connectors <SUBCOMMAND>
```

**Subcommands**

| Subcommand | Description |
|-----------|-------------|
| `list` | List all registered connectors with status |

---

#### `orp connectors list`

List all registered connectors with their type, status, and trust score.

**Synopsis**

```
orp connectors list
```

**Examples**

```bash
# List all connectors
orp connectors list

# Filter for active connectors only
orp connectors list | jq '.connectors[] | select(.enabled == true)'

# Get connector IDs
orp connectors list | jq -r '.connectors[].connector_id'
```

**Output**

```json
{
  "connectors": [
    {
      "connector_id": "ais-demo",
      "source_name": "AIS Demo Feed",
      "source_type": "ais",
      "trust_score": 0.95,
      "events_ingested": 4821,
      "enabled": true
    }
  ],
  "total": 1
}
```

---

## Output Formats

ORP CLI outputs **raw JSON** from the API. Use standard UNIX tools to transform:

### Table (via `column`)

```bash
orp query -q "MATCH (s:ship) RETURN s.name, s.mmsi, s.speed" \
  | jq -r '["NAME","MMSI","SPEED"], (.results[] | [.name, .mmsi, (.speed|tostring)]) | @tsv' \
  | column -t -s $'\t'
```

```
NAME                    MMSI         SPEED
MV Atlantic Pioneer     123456789    24.3
Vessel Horizon          987654321    21.8
Rotterdam Carrier       246813579    18.0
```

### CSV

```bash
orp query -q "MATCH (s:ship) RETURN s.name, s.mmsi, s.speed" \
  | jq -r '["name","mmsi","speed"], (.results[] | [.name, .mmsi, .speed]) | @csv'
```

```csv
"name","mmsi","speed"
"MV Atlantic Pioneer",123456789,24.3
"Vessel Horizon",987654321,21.8
```

### Filtered JSON

```bash
orp query -q "MATCH (s:ship) RETURN s" \
  | jq '.results[] | {name, speed, position}'
```

```json
{ "name": "MV Atlantic Pioneer", "speed": 24.3, "position": [51.9, 4.5] }
{ "name": "Vessel Horizon", "speed": 21.8, "position": [52.1, 3.8] }
```

### Pretty table (via `rich` / Python)

```bash
orp query -q "MATCH (s:ship) RETURN s.name, s.speed" | python3 -c "
import json, sys
from rich.table import Table
from rich.console import Console
data = json.load(sys.stdin)
t = Table('Name', 'Speed (kn)')
for r in data['results']:
    t.add_row(r['name'], str(r.get('speed','-')))
Console().print(t)
"
```

---

## Environment Variables

ORP reads these variables at startup and runtime. All can also be set in the config file using `${env.VAR}` syntax.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `ORP_DEV_MODE` | No | `false` | Set to `true` or `1` to enable permissive auth (no JWT required). **Never use in production.** |
| `JWT_SECRET` | Production | — | Secret key for HS256 JWT signing. Minimum 32 chars. Required unless `ORP_DEV_MODE=true`. |
| `ORP_PORT` | No | `9090` | Override the server listen port (equivalent to `--port`). |
| `ORP_CONFIG` | No | `config.yaml` | Path to the configuration YAML file. |
| `ORP_CORS_ORIGINS` | No | `http://localhost:3000` | Comma-separated list of allowed CORS origins. |
| `NO_COLOR` | No | — | When set (any value), disables ANSI colour in terminal output (CLIG compliant). |
| `RUST_LOG` | No | `info` | Logging filter for `tracing` crate. Values: `error`, `warn`, `info`, `debug`, `trace`. |

**Usage Examples**

```bash
# Development — no auth
ORP_DEV_MODE=true orp start

# Production — JWT required
JWT_SECRET="$(openssl rand -hex 32)" orp start --config /etc/orp/prod.yaml

# Custom port via env (useful in Docker)
ORP_PORT=8080 orp start

# Use a config path from env
ORP_CONFIG=/etc/orp/config.yaml orp start

# Disable all colour output (for CI/log files)
NO_COLOR=1 orp status

# Verbose debug logging
RUST_LOG=debug orp start
```

**In config.yaml (env var substitution)**

```yaml
security:
  oidc:
    client_secret: "${env.ORP_OIDC_CLIENT_SECRET}"

api:
  jwt_secret: "${env.JWT_SECRET}"
```

ORP substitutes `${env.VAR}` at load time. If the variable is unset, it is replaced with an empty string and a warning is logged.

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
orp status > /dev/null 2>&1
if [ $? -ne 0 ]; then
  echo "ORP is not running — aborting pipeline" >&2
  exit 1
fi

# Run query only if server is up
orp status > /dev/null && orp query -q "MATCH (s:ship) RETURN s.name"
```

---

## Configuration File

The config file is YAML. ORP loads `config.yaml` in the current directory by default, or the path from `--config` / `ORP_CONFIG`.

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
  telemetry_enabled: true
  telemetry_endpoint: "http://otel-collector:4317"

storage:
  duckdb:
    path: "/data/orp.duckdb"
    memory_limit_gb: 16
    max_connections: 20
  rocksdb:
    path: "/data/state.db"
    cache_size_mb: 1024
  kuzu:
    path: "/data/graph.kuzu"
    memory_limit_gb: 4
    sync_interval_seconds: 30

retention:
  events_ttl_days: 90
  snapshots_ttl_days: 30
  audit_log_ttl_days: 365
  delete_batch_size: 10000

security:
  oidc:
    enabled: true
    provider_url: "https://auth.yourcompany.com"
    client_id: "orp-client"
    client_secret: "${env.ORP_OIDC_CLIENT_SECRET}"
    scopes: ["openid", "profile", "email"]
    redirect_uri: "https://orp.yourcompany.com/auth/callback"
  abac:
    enabled: true
    policy_file: "/etc/orp/policies.yaml"
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

  - name: "weather_api"
    type: "http_poll"
    enabled: true
    url: "https://api.weather.example.com/marine"
    entity_type: "weather_cell"
    trust_score: 0.80
    schedule: "*/5 * * * *"
    headers:
      Authorization: "Bearer ${env.WEATHER_API_KEY}"

monitors:
  - rule_id: "high_speed_ship"
    name: "Ship exceeding speed limit"
    entity_type: "ship"
    condition: "speed > 25"
    action: "alert"
    enabled: true

  - rule_id: "geofence_breach"
    name: "Entity entered restricted zone"
    entity_type: "ship"
    condition: "zone = 'restricted'"
    action: "alert"
    enabled: true

api:
  rate_limit_per_minute: 1000
  cors_enabled: true
  cors_allowed_origins:
    - "https://command.yourcompany.com"
  api_key_header: "X-API-Key"
  jwt_secret: "${env.JWT_SECRET}"

logging:
  level: "info"
  format: "json"
  output: "stdout"
  audit_log_path: "/var/log/orp/audit.log"
```

### Available Templates

| Template | Description |
|----------|-------------|
| `maritime` | AIS ship tracking with Rotterdam port, speed monitors, demo data |

Use with: `orp start --template maritime`

---

## Shell Completion

### Bash

```bash
# Generate and install
orp completions bash > /etc/bash_completion.d/orp

# Or for current user only
orp completions bash >> ~/.bash_completion

# Apply immediately
source /etc/bash_completion.d/orp
```

### Zsh

```bash
# Generate
orp completions zsh > "${fpath[1]}/_orp"

# Or in your ~/.zshrc:
eval "$(orp completions zsh)"

# Apply
exec zsh
```

### Fish

```bash
orp completions fish > ~/.config/fish/completions/orp.fish
```

### PowerShell

```powershell
orp completions powershell | Out-File -Encoding utf8 $PROFILE.CurrentUserAllHosts
```

> **Note:** The `completions` subcommand is planned for v0.2.0. Until then, install shell completions manually from the repository's `completions/` directory.

**What completions provide:**

```bash
orp <TAB>
# start    query    status    connectors    completions    help

orp start --<TAB>
# --config    --template    --port    --help

orp query --<TAB>
# --query    --help

orp connectors <TAB>
# list

orp start --template <TAB>
# maritime
```

---

## Piping & Scripting

ORP is designed to compose cleanly with standard UNIX tools. All query output is valid JSON, all errors go to `stderr`, and exit codes are meaningful.

### Pipeline Examples

```bash
# Count active ships
orp query -q "MATCH (s:ship) RETURN s.mmsi" | jq '.results | length'

# Extract just names as newline-separated list
orp query -q "MATCH (s:ship) RETURN s.name" | jq -r '.results[].name'

# Find the fastest ship
orp query -q "MATCH (s:ship) RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 1" \
  | jq -r '.results[0].name'

# Ships over speed threshold, formatted as CSV
orp query -q "MATCH (s:ship) WHERE s.speed > 20 RETURN s.name, s.mmsi, s.speed" \
  | jq -r '.results[] | [.name, (.mmsi | tostring), (.speed | tostring)] | @csv'

# Save a snapshot of all entities to disk
orp query -q "MATCH (e:ship) RETURN e" \
  | jq '.results' > snapshots/ships-$(date +%Y%m%d-%H%M%S).json

# Alert when a specific ship appears
while true; do
  orp query -q "MATCH (s:ship) WHERE s.mmsi = '123456789' RETURN s.position" \
    | jq -e '.results | length > 0' > /dev/null && \
    notify-send "Ship MV Pioneer is online"
  sleep 30
done

# Chain status check + query
orp status > /dev/null 2>&1 \
  && orp query -q "MATCH (s:ship) RETURN s.name, s.speed" \
  | jq '.results[] | select(.speed > 20) | .name' \
  || echo "Server offline"

# Export all monitors to a file
curl -s -H "Authorization: Bearer $ORP_TOKEN" \
  http://localhost:9090/api/v1/monitors \
  | jq '.' > monitors-backup.json

# Run a query and send results to a webhook
orp query -q "MATCH (s:ship) WHERE s.speed > 25 RETURN s.name, s.speed" \
  | curl -s -X POST https://hooks.example.com/alert \
    -H "Content-Type: application/json" \
    -d @-
```

### In CI/CD Pipelines

```bash
#!/bin/bash
# deploy-check.sh — verify ORP is healthy after deploy

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
# Makefile

.PHONY: dev prod status query-ships

dev:
	ORP_DEV_MODE=true orp start --template maritime

prod:
	JWT_SECRET=$(shell cat .jwt-secret) orp start --config config.prod.yaml

status:
	@orp status | jq -r '"Status: \(.status) | Entities: \(.storage.entity_count) | Uptime: \(.uptime_seconds)s"'

query-ships:
	@orp query -q "MATCH (s:ship) RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 5" \
	  | jq -r '.results[] | "\(.name): \(.speed) kn"'
```

---

## AI Agent Integration

ORP is designed to be used by AI agents (LLMs, Claude, GPT, Gemini) as a data fusion backend. The CLI is the primary interface for autonomous agents.

### Agent Principles

When an AI agent uses the ORP CLI:

1. **Check server health first** — always run `orp status` before any query.
2. **Use JSON output** — all output is structured JSON, parseable without screen-scraping.
3. **Handle errors gracefully** — check exit codes, parse error messages from JSON.
4. **Compose with tools** — pipe into `jq`, `awk`, `python3` for transformations.
5. **Prefer specific queries** — use WHERE clauses and LIMITs to minimise data transferred.

### Example: Claude/GPT Tool Definition

```json
{
  "name": "orp_query",
  "description": "Execute an ORP-QL query against the Open Reality Protocol data fusion engine. Returns JSON with tracked entities (ships, aircraft, vehicles) and their properties.",
  "parameters": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "ORP-QL query. Syntax: MATCH (alias:EntityType) [WHERE condition] RETURN fields [ORDER BY field] [LIMIT n]"
      }
    },
    "required": ["query"]
  }
}
```

**Agent Tool Implementation (Python)**

```python
import subprocess
import json

def orp_query(query: str) -> dict:
    """Execute an ORP-QL query and return parsed JSON results."""
    result = subprocess.run(
        ["orp", "query", "--query", query],
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

**Agent Tool Implementation (TypeScript/Node.js)**

```typescript
import { execSync } from 'child_process';

interface OrpResult {
  results: Record<string, unknown>[];
  count: number;
  elapsed_ms: number;
  error?: string;
}

function orpQuery(query: string): OrpResult {
  try {
    const output = execSync(`orp query --query "${query.replace(/"/g, '\\"')}"`, {
      encoding: 'utf-8',
      timeout: 30000,
    });
    return JSON.parse(output);
  } catch (err: any) {
    return { results: [], count: 0, elapsed_ms: 0, error: err.message };
  }
}

// Usage in an AI tool handler
const result = orpQuery('MATCH (s:ship) WHERE s.speed > 15 RETURN s.name, s.mmsi LIMIT 10');
console.log(`Found ${result.count} fast ships`);
```

### MCP (Model Context Protocol) Server

ORP exposes an MCP-compatible WebSocket endpoint for direct agent integration without shell spawning:

```bash
# Connect via MCP transport
ORP_DEV_MODE=true orp start

# Then connect your MCP client to:
# ws://localhost:9090/ws/updates

# Or use the HTTP REST API directly:
curl -s http://localhost:9090/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{"query": "MATCH (s:ship) RETURN s.name, s.speed LIMIT 5"}'
```

### Agent Workflow Example

```bash
# A typical AI agent session with ORP:

# Step 1: Check environment
orp status | jq '{status, entities: .storage.entity_count}'

# Step 2: Discover what entity types exist
orp query -q "MATCH (e:ship) RETURN COUNT(e) AS ships" | jq '.results[0].ships'
orp query -q "MATCH (e:port) RETURN COUNT(e) AS ports" | jq '.results[0].ports'

# Step 3: Investigate an anomaly
orp query -q "MATCH (s:ship) WHERE s.speed > 25 RETURN s.name, s.mmsi, s.speed, s.position" \
  | jq '.results'

# Step 4: Check monitor alerts
curl -s http://localhost:9090/api/v1/alerts \
  | jq '.alerts[] | {rule: .rule_name, entity: .entity_id, fired_at}'

# Step 5: Query relationships
orp query -q "MATCH (s:ship)-[r:NEAR]->(p:port) RETURN s.name, p.name, r.distance_km" \
  | jq '.results'
```

---

## Comparison with Similar Tools

ORP's CLI design takes inspiration from the best in the industry:

### Command Structure

| Tool | Pattern | ORP |
|------|---------|-----|
| `kubectl get pods` | verb-noun | `orp query "MATCH (s:ship)"` |
| `docker ps --format json` | noun-verb + format flag | `orp query ... \| jq` |
| `gh pr list --json` | noun-verb + `--json` | `orp query ... \| jq` |
| `git log --oneline` | verb + display flag | `orp status \| jq` |

ORP uses a **query-first** model (like `psql` / `clickhouse-client`) since the primary operation is data retrieval via ORP-QL.

### Feature Comparison

| Feature | kubectl | docker | gh CLI | **ORP** |
|---------|---------|--------|--------|---------|
| Structured JSON output | ✅ `--output json` | ✅ `--format json` | ✅ `--json` | ✅ default |
| Shell completions | ✅ | ✅ | ✅ | 🚧 v0.2 |
| Config file | ✅ kubeconfig | ✅ daemon.json | ✅ hosts.yml | ✅ config.yaml |
| Env var config | ✅ `KUBECONFIG` | ✅ `DOCKER_HOST` | ✅ `GH_TOKEN` | ✅ `ORP_*` |
| Templates / contexts | ✅ contexts | ✅ contexts | ✅ hosts | ✅ templates |
| Exit codes | ✅ | ✅ | ✅ | ✅ |
| Piping friendly | ✅ | ✅ | ✅ | ✅ |
| Streaming output | ✅ `--watch` | ✅ | ❌ | 🚧 v0.2 |
| Real-time WebSocket | ❌ | ❌ | ❌ | ✅ `/ws/updates` |
| Embedded dashboard | ❌ | ❌ | ❌ | ✅ Deck.gl |
| Single binary | ✅ | ❌ | ✅ | ✅ |
| Query language | ❌ | ❌ | ❌ | ✅ ORP-QL |

### Philosophical Alignment with CLIG

ORP follows the [Command Line Interface Guidelines](https://clig.dev):

| Principle | Implementation |
|-----------|---------------|
| **Human-first design** | Friendly startup banner, clear error messages, suggests next steps |
| **Composability** | All output is JSON, nothing requires screen-scraping |
| **Consistency** | Same flags as similar tools (`--config`, `--port`, `--help`) |
| **Discoverability** | `--help` on every command, subcommand listing |
| **Robustness** | Server-down errors include fix instructions; no stack traces shown to users |
| **Exit codes** | Standard 0/1/2/130 — scriptable |
| `NO_COLOR` support | ✅ ANSI disabled when `NO_COLOR` is set |
| **stderr for errors** | ✅ All errors to `stderr`, data to `stdout` |

---

## Troubleshooting

### Server won't start

```bash
# Check if port is in use
lsof -i :9090

# Use a different port
orp start --port 8080

# Check config file syntax
orp start --config config.yaml
# Look for: Error: config parse error...
```

### Auth errors (401 Unauthorized)

```bash
# Quick fix: enable dev mode
ORP_DEV_MODE=true orp start

# Production fix: set JWT_SECRET
export JWT_SECRET="$(openssl rand -hex 32)"
orp start

# Then use the token in API calls:
curl -H "Authorization: Bearer $TOKEN" http://localhost:9090/api/v1/entities
```

### Query returns empty results

```bash
# Check entity count
orp status | jq '.storage.entity_count'

# If 0, the demo data may not have loaded — try maritime template
orp start --template maritime

# Check what entity types exist
orp query -q "MATCH (s:ship) RETURN COUNT(s)"
```

### Rate limited (429)

```
{"error": {"code": "RATE_LIMITED", "status": 429, "retry_after_seconds": 1}}
```

ORP uses a token bucket: 100 req/sec per IP. To increase:

```yaml
# config.yaml
api:
  rate_limit_per_minute: 10000
```

### Verbose logging

```bash
RUST_LOG=debug orp start 2>&1 | grep -v "tower_http"
```

---

## API Reference

The full REST API is documented in the OpenAPI spec:

```bash
# View the OpenAPI spec
cat /Users/deepred/orp/openapi.yaml

# Or browse interactively (requires server running)
open http://localhost:9090/api/v1/docs
```

**Key Endpoints**

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/health` | Health check (no auth) |
| `GET` | `/api/v1/metrics` | Prometheus metrics |
| `GET` | `/api/v1/entities` | List entities |
| `POST` | `/api/v1/entities` | Create entity |
| `GET` | `/api/v1/entities/{id}` | Get entity |
| `PUT` | `/api/v1/entities/{id}` | Update entity |
| `DELETE` | `/api/v1/entities/{id}` | Delete entity |
| `GET` | `/api/v1/entities/search` | Search entities |
| `GET` | `/api/v1/entities/{id}/events` | Get entity events |
| `GET` | `/api/v1/entities/{id}/relationships` | Get relationships |
| `POST` | `/api/v1/relationships` | Create relationship |
| `POST` | `/api/v1/query` | Execute ORP-QL |
| `POST` | `/api/v1/graph` | Graph (Cypher passthrough) |
| `GET` | `/api/v1/connectors` | List connectors |
| `POST` | `/api/v1/connectors` | Create connector |
| `PUT` | `/api/v1/connectors/{id}` | Update connector |
| `DELETE` | `/api/v1/connectors/{id}` | Delete connector |
| `GET` | `/api/v1/monitors` | List monitor rules |
| `POST` | `/api/v1/monitors` | Create monitor rule |
| `GET` | `/api/v1/monitors/{id}` | Get monitor rule |
| `PUT` | `/api/v1/monitors/{id}` | Update monitor rule |
| `DELETE` | `/api/v1/monitors/{id}` | Delete monitor rule |
| `GET` | `/api/v1/alerts` | List alerts |
| `POST` | `/api/v1/alerts/{id}/acknowledge` | Acknowledge alert |
| `POST` | `/api/v1/api-keys` | Create API key |
| `GET` | `/api/v1/api-keys` | List API keys |
| `DELETE` | `/api/v1/api-keys/{id}` | Revoke API key |
| `GET` | `/ws/updates` | WebSocket — real-time events |

---

## Roadmap

**v0.2.0 (planned)**
- `orp completions <shell>` — generate shell completions
- `orp query --watch` — streaming/live query output
- `orp connectors add/remove` — connector CRUD from CLI
- `orp monitors list/add/remove` — monitor rule management
- `orp config validate` — validate config file without starting
- `orp config init` — interactive config generator
- `orp logs` — tail server logs
- `--output table|json|csv` flag on query command

**v0.3.0 (planned)**
- `orp context` — manage multiple ORP instances (like `kubectl config use-context`)
- `orp export` — bulk export entities/events to CSV/Parquet
- `orp import` — bulk import from CSV/JSON
- Plugin system for custom connectors

---

*Built with ❤️ using [Rust](https://rust-lang.org), [Clap](https://docs.rs/clap), [Axum](https://docs.rs/axum), [DuckDB](https://duckdb.org), and the principles of [CLIG](https://clig.dev).*
