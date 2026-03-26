# Getting Started with ORP

**Time to complete:** 30–45 minutes · **Difficulty:** Beginner · **Requires:** macOS or Linux, internet connection

By the end of this guide you will have:

- ORP installed and running
- A live maritime dashboard showing real ships on a map
- Run your first ORP-QL queries
- Created a custom HTTP connector
- Set up an automated alert
- Configured user authentication

---

## Table of Contents

1. [Install ORP](#1-install-orp)
2. [Start with the Maritime Template](#2-start-with-the-maritime-template)
3. [Exploring the Console](#3-exploring-the-console)
4. [Your First ORP-QL Queries](#4-your-first-orp-ql-queries)
5. [Using the API Directly](#5-using-the-api-directly)
6. [Create a Custom Connector](#6-create-a-custom-connector)
7. [Set Up Alerts](#7-set-up-alerts)
8. [Configure Authentication](#8-configure-authentication)
9. [What's Next](#9-whats-next)

---

## 1. Install ORP

### One-Line Install (Recommended)

```bash
curl -fsSL https://orp.dev/install | sh
```

This script:
1. Detects your OS and architecture (Linux x86_64, Linux ARM64, macOS Intel, macOS Apple Silicon)
2. Downloads the signed binary from the latest release
3. Verifies the SHA-256 checksum
4. Installs to `/usr/local/bin/orp`
5. Prints the version

```
✓ Detected: macOS arm64
✓ Downloading orp v0.1.0...
✓ Verifying checksum...
✓ Installed to /usr/local/bin/orp

ORP v0.1.0 — Open Reality Protocol
Run: orp start --template maritime
```

### Verify Installation

```bash
orp --version
# ORP v0.1.0 (built 2026-03-26, git: abc1234)

orp --help
# ORP — Open Reality Protocol
# 
# USAGE:
#   orp <COMMAND>
# 
# COMMANDS:
#   start       Start ORP with a template or config file
#   query       Run an ORP-QL query from the command line
#   connector   Manage connectors
#   verify      Verify audit log integrity
#   version     Print version information
```

### Install from Source

If you prefer to build from source:

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone and build
git clone https://github.com/orproject/orp.git
cd orp
cargo build --release

# The binary is at:
./target/release/orp --version
```

Build takes approximately 5–10 minutes on a modern machine.

---

## 2. Start with the Maritime Template

The maritime template connects to a public AIS feed, pulls weather data from NOAA, and loads port geometry from OpenStreetMap. It's the fastest way to see ORP in action.

```bash
orp start --template maritime
```

You'll see startup output like:

```
ORP v0.1.0 starting...
✓ Config loaded: maritime template
✓ Storage initialized at ~/.orp/data/
  - DuckDB: ready (0 entities)
  - Kuzu: ready (0 nodes)
  - RocksDB: ready (clean state)
✓ Security: Ed25519 signing enabled
✓ Audit log: initialized (seq 0)
✓ Connectors starting:
  - ais-global: connecting to 153.44.253.27:9999...
  - weather-noaa: polling https://api.weather.gov/alerts...
  - osm-ports: loading harbor geometries...
✓ HTTP server listening on http://127.0.0.1:9090
✓ WebSocket server on ws://127.0.0.1:9090/ws/updates

Opening browser...

Ingesting:  1,247 ships  |  0 alerts  |  3 weather systems  |  ▶ LIVE
```

**ORP opens your browser automatically.** If it doesn't, navigate to [http://localhost:9090](http://localhost:9090).

Within 30–60 seconds, you'll see thousands of ships updating in real-time on the map.

### Understanding the Template Config

The maritime template creates a config at `~/.orp/config.yaml`. Open it to see what's configured:

```bash
cat ~/.orp/config.yaml
```

Key sections:
- **`connectors:`** — the three active data sources
- **`storage:`** — where data files are stored
- **`server:`** — HTTP port and CORS settings

---

## 3. Exploring the Console

The ORP Console is a web interface served directly from the binary.

### The Map

- **Pan:** Click and drag
- **Zoom:** Scroll wheel, or pinch on touchpad
- **Click any entity:** Opens the Entity Inspector panel on the right
- **Color coding:**
  - 🔵 Blue — cargo ships
  - 🟢 Green — tankers
  - 🟡 Yellow — passenger vessels
  - 🔴 Red — vessels with active alerts
  - ✈️ White — aircraft (if ADS-B enabled)

### Entity Inspector

Click any ship on the map. The Entity Inspector slides in from the right:

```
╔═══════════════════════════════════════════╗
║  EVER GIVEN (MMSI: 353136000)             ║
║  ────────────────────────────────────     ║
║  Type:      Container Ship                ║
║  Flag:      Panama                        ║
║  Position:  51.9225°N, 4.4792°E          ║
║  Speed:     12.3 kn  │  Course: 245°      ║
║  Heading:   243°      │  Destination: RTM  ║
║  Confidence: 0.95     │  Source: ais-global║
║                                           ║
║  RELATIONSHIPS                            ║
║  ──────────────                           ║
║  → HEADING_TO Rotterdam (ETA 4h 30m)     ║
║  ← OWNS Evergreen Marine Corp.           ║
║                                           ║
║  HISTORY                                  ║
║  ──────                                   ║
║  14:32 UTC  Position update               ║
║  14:31 UTC  Speed changed 11.8→12.3 kn   ║
╚═══════════════════════════════════════════╝
```

### Query Bar

The query bar at the top of the screen accepts ORP-QL queries. Click it or press `/` to focus.

Try typing:

```
ships near Rotterdam
```

ORP uses template-based natural language matching to run:

```sql
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 50km)
RETURN s
ORDER BY distance(s.position, point(51.9225, 4.4792))
```

Results appear as highlighted entities on the map and in a table below the query bar.

### Alert Feed

The alert feed (bottom-left panel) shows real-time anomaly notifications:

```
🔴 CRITICAL  14:35 UTC
   Ship MMSI:123456789 deviated from route by 52 km
   [View on map]  [Acknowledge]

⚠️  WARNING   14:33 UTC  
   Vessel speed > 25 kn detected in restricted area
   [View on map]  [Acknowledge]
```

### Timeline Scrubber

The timeline scrubber at the bottom of the screen lets you replay past states:

1. Click the timeline bar and drag left to go back in time
2. The map updates to show entity positions at that timestamp
3. The current time indicator shows "PLAYBACK 2026-03-26 08:30 UTC"
4. Click "LIVE" to return to real-time

---

## 4. Your First ORP-QL Queries

Let's run queries progressively from simple to complex.

### From the Console

Click the Query Bar (or press `/`) and try these:

**Basic filter:**
```sql
MATCH (s:Ship)
WHERE s.speed > 20
RETURN s.name, s.speed, s.entity_id
LIMIT 10
```

**Geospatial search:**
```sql
MATCH (s:Ship)
WHERE near(s.position, point(51.9225, 4.4792), 100km)
  AND s.ship_type = "cargo"
RETURN s.name, s.mmsi, s.speed, s.destination
ORDER BY s.speed DESC
```

**Graph traversal:**
```sql
MATCH (s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.name = "Rotterdam"
RETURN s.name, s.mmsi, s.eta, p.congestion
ORDER BY s.eta
```

**Multi-hop graph:**
```sql
MATCH (org:Organization)-[:OWNS]->(s:Ship)-[:HEADING_TO]->(p:Port)
WHERE p.congestion > 0.7
RETURN org.name, s.name, p.name, p.congestion
ORDER BY p.congestion DESC
LIMIT 20
```

**Aggregate:**
```sql
MATCH (s:Ship)
WHERE within(s.position, bbox(-10, 35, 40, 65))
RETURN s.ship_type, count(*) AS vessel_count, avg(s.speed) AS avg_speed
GROUP BY s.ship_type
ORDER BY vessel_count DESC
```

### From the CLI

You can also run queries directly from your terminal:

```bash
# Simple query
orp query "MATCH (s:Ship) WHERE s.speed > 20 RETURN s.name, s.speed LIMIT 5"

# Query with JSON output
orp query --format json "MATCH (s:Ship) WHERE near(s.position, point(51.9, 4.5), 50km) RETURN s"

# Query to a file
orp query --output results.json "MATCH (s:Ship) RETURN s.entity_id, s.name, s.speed"
```

---

## 5. Using the API Directly

ORP exposes a full REST API at `http://localhost:9090/api/v1`.

### List Ships

```bash
curl http://localhost:9090/api/v1/entities?type=Ship&limit=5 | jq .
```

Response:
```json
{
  "data": [
    {
      "id": "mmsi:353136000",
      "type": "Ship",
      "name": "EVER GIVEN",
      "properties": {
        "mmsi": 353136000,
        "ship_type": "cargo",
        "flag": "PA",
        "speed": 12.3,
        "course": 245.0,
        "heading": 243.0,
        "destination": "NLRTM"
      },
      "geometry": {
        "type": "Point",
        "coordinates": [4.4792, 51.9225]
      }
    }
  ],
  "pagination": {
    "page": 1, "limit": 5, "total_count": 2847
  }
}
```

### Get a Single Entity

```bash
curl http://localhost:9090/api/v1/entities/mmsi:353136000 | jq .
```

### Search Geospatially

```bash
curl "http://localhost:9090/api/v1/entities/search?lat=51.9225&lon=4.4792&radius_km=50&type=Ship" | jq .
```

### Execute an ORP-QL Query

```bash
curl -X POST http://localhost:9090/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship) WHERE s.speed > 20 AND s.ship_type = \"cargo\" RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 10"
  }' | jq .
```

### Get Entity Relationships

```bash
curl http://localhost:9090/api/v1/entities/mmsi:353136000/relationships | jq .
```

---

## 6. Create a Custom Connector

Let's build a custom connector that polls a JSON REST API and injects entities into ORP.

We'll connect to the [Open-Meteo weather API](https://open-meteo.com/) to pull temperature observations for major ports.

### Step 1: Understand the HTTP Connector

ORP's built-in `http_poll` connector can poll any REST API and map the response to ORP entities using a JSONPath mapping. No code required.

### Step 2: Define the Connector Config

Add this to your `~/.orp/config.yaml` under `connectors:`:

```yaml
connectors:
  # ... existing connectors ...

  - id: "port-weather"
    type: "http_poll"
    enabled: true
    config:
      url: "https://api.open-meteo.com/v1/forecast"
      method: GET
      params:
        latitude: "51.9225"          # Rotterdam
        longitude: "4.4792"
        current_weather: "true"
        temperature_unit: "celsius"
      poll_interval_secs: 300        # every 5 minutes
      
      # Map the response to an ORP entity
      mapping:
        entity_id: "weather-rotterdam-current"
        entity_type: "WeatherObservation"
        name: "Rotterdam Weather"
        geo:
          lat: "$.latitude"
          lon: "$.longitude"
        properties:
          temperature: "$.current_weather.temperature"
          wind_speed: "$.current_weather.windspeed"
          wind_direction: "$.current_weather.winddirection"
          is_day: "$.current_weather.is_day"
          weather_code: "$.current_weather.weathercode"
```

### Step 3: Reload the Config

```bash
# ORP watches the config file for changes. Alternatively:
orp connector reload port-weather

# Or restart ORP
# Ctrl+C then orp start --config ~/.orp/config.yaml
```

### Step 4: Verify the Connector

```bash
# Check connector status
curl http://localhost:9090/api/v1/connectors/port-weather | jq .

# Query the new entity
orp query "MATCH (w:WeatherObservation {entity_id: 'weather-rotterdam-current'}) RETURN w"
```

### Step 5: Build a WASM Connector (Advanced, Phase 2)

For full custom logic (custom protocols, complex transformations), you can write a WASM connector in any language. This is a Phase 2 feature. See [docs/CONNECTOR_GUIDE.md](CONNECTOR_GUIDE.md) for the full SDK reference.

---

## 7. Set Up Alerts

Alerts fire when a monitor rule's conditions are met. Let's create two rules.

### Alert 1: Ship Entering a Restricted Zone

Create a monitor via the API:

```bash
curl -X POST http://localhost:9090/api/v1/monitors \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Ship Entered Restricted Zone",
    "description": "Alert when any cargo ship enters the designated exclusion zone",
    "enabled": true,
    "rule": {
      "type": "geofence_entry",
      "entity_type": "Ship",
      "condition": {
        "ship_type": "cargo"
      },
      "zone": {
        "type": "Polygon",
        "coordinates": [
          [3.0, 51.0], [6.0, 51.0], [6.0, 53.0], [3.0, 53.0], [3.0, 51.0]
        ]
      }
    },
    "severity": "WARNING",
    "actions": [
      { "type": "webhook", "url": "https://hooks.slack.com/services/YOUR/SLACK/WEBHOOK" },
      { "type": "console" }
    ]
  }'
```

### Alert 2: Speed Threshold Exceeded

```bash
curl -X POST http://localhost:9090/api/v1/monitors \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Speed Anomaly — Fast Vessel",
    "enabled": true,
    "rule": {
      "type": "property_threshold",
      "entity_type": "Ship",
      "condition": "ship.speed > 25 AND ship.ship_type = \"fishing_vessel\""
    },
    "severity": "CRITICAL",
    "debounce_seconds": 60,
    "actions": [{ "type": "console" }]
  }'
```

### Alert 3: Using ORP-QL Monitor DSL

You can also define monitors using ORP-QL:

```bash
curl -X POST http://localhost:9090/api/v1/monitors \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Ships Near Storm",
    "rule": {
      "type": "orp_ql",
      "query": "MATCH (s:Ship), (w:WeatherSystem) WHERE w.severity = \"CRITICAL\" AND near(s.position, w.center, w.radius_km * 1.5) RETURN s, w",
      "fire_when": "result_count > 0",
      "eval_interval_secs": 120
    },
    "severity": "WARNING"
  }'
```

### View and Acknowledge Alerts

```bash
# List all fired alerts
curl http://localhost:9090/api/v1/alerts | jq .

# Acknowledge an alert
curl -X POST http://localhost:9090/api/v1/alerts/alert-id-123/acknowledge \
  -d '{"note": "Investigated — false positive, vessel has permit"}'
```

---

## 8. Configure Authentication

By default, ORP runs without authentication (safe for local development). For production use, configure OIDC.

### Option A: OIDC with an External Provider

Edit `~/.orp/config.yaml`:

```yaml
auth:
  mode: "oidc"
  oidc:
    issuer: "https://YOUR_TENANT.auth0.com/"
    client_id: "YOUR_CLIENT_ID"
    client_secret: "${env.OIDC_CLIENT_SECRET}"
    redirect_uri: "http://localhost:9090/auth/callback"
    scopes: ["openid", "email", "profile"]
```

Set the environment variable:

```bash
export OIDC_CLIENT_SECRET=your_secret_here
orp start --config ~/.orp/config.yaml
```

Users now must log in via your identity provider before accessing the console or API.

### Option B: API Keys

For programmatic access, create API keys:

```bash
# First, authenticate as admin
export TOKEN=$(curl -s -X POST http://localhost:9090/auth/token \
  -d '{"username":"admin","password":"admin"}' | jq -r '.access_token')

# Create a read-only API key
curl -X POST http://localhost:9090/api/v1/api-keys \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "monitoring-dashboard",
    "scopes": ["entities:read", "monitors:read"],
    "expires_in": 31536000
  }'
```

Use the key:

```bash
curl http://localhost:9090/api/v1/entities \
  -H "X-API-Key: YOUR_API_KEY_HERE"
```

### ABAC Policies

Define attribute-based access control rules to restrict data visibility:

```yaml
# In config.yaml
security:
  abac:
    policies:
      - name: "cargo-value-restricted"
        description: "Only admins can see cargo_value property"
        effect: DENY
        conditions:
          resource.property: "cargo_value"
          subject.permissions: { not_contains: "admin" }
```

See [docs/SECURITY.md](SECURITY.md) for the full policy language reference.

---

## 9. What's Next

You now have ORP running with live maritime data, custom connectors, alerts, and authentication.

### Explore More

- **[ORP-QL Guide](ORP_QL_GUIDE.md)** — 20+ query examples covering every language feature
- **[API Reference](API_REFERENCE.md)** — every endpoint with curl examples
- **[Security Guide](SECURITY.md)** — OIDC, ABAC, Ed25519, audit log deep dive
- **[Architecture](../ARCHITECTURE.md)** — how ORP works under the hood

### Try Other Templates

```bash
# Aircraft tracking
orp start --template adsb

# Supply chain monitoring
orp start --template supply-chain

# Climate + shipping correlation
orp start --template climate
```

### Add More Connectors

```bash
# List available built-in connector types
orp connector list-types

# Add ADS-B aircraft tracking
orp connector add --type adsb --id my-adsb --host localhost --port 30003
```

### Join the Community

- [GitHub Issues](https://github.com/orproject/orp/issues) — bug reports, feature requests
- [Discord](https://discord.gg/orp) — real-time help, show your use case
- [Community Forum](https://github.com/orproject/orp/discussions) — longer discussions

### Contribute

Read [CONTRIBUTING.md](../CONTRIBUTING.md) to learn how to contribute code, documentation, or connectors.

---

_Having trouble? Check the [FAQ](FAQ.md) or ask in [Discord](https://discord.gg/orp)._
