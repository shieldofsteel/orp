# ORP Connector Guide

> **Connect any data source to ORP in minutes.**

ORP's connector system is intentionally generic. Anything that produces data — a REST API, a database, a syslog stream, a WebSocket feed — can be ingested with zero modifications to ORP's core. This guide covers every connector type available today and shows you exactly how to add your own.

---

## Table of Contents

1. [Core Concepts](#1-core-concepts)
2. [Writing a Custom Connector](#2-writing-a-custom-connector)
3. [Generic REST API Connector](#3-generic-rest-api-connector)
   - [Shodan](#31-shodan)
   - [VirusTotal](#32-virustotal)
   - [AbuseIPDB](#33-abuseipdb)
   - [FlightAware](#34-flightaware)
   - [OpenWeatherMap](#35-openweathermap)
   - [USGS Earthquakes](#36-usgs-earthquakes)
   - [Custom REST API](#37-custom-rest-api)
4. [Syslog / CEF Connector](#4-syslog--cef-connector)
5. [Database Connector](#5-database-connector)
6. [Built-in Connectors Reference](#6-built-in-connectors-reference)
7. [YAML Configuration Reference](#7-yaml-configuration-reference)
8. [Troubleshooting](#8-troubleshooting)

---

## 1. Core Concepts

### SourceEvent

Every connector emits `SourceEvent` values on a Tokio channel. These are the atoms of ORP.

```rust
pub struct SourceEvent {
    pub connector_id: String,       // which connector produced this
    pub entity_id:    String,       // e.g. "ship:123456789" or "host:192.168.1.1"
    pub entity_type:  String,       // e.g. "ship", "host", "threat", "earthquake"
    pub properties:   HashMap<String, JsonValue>,
    pub timestamp:    DateTime<Utc>,
    pub latitude:     Option<f64>,
    pub longitude:    Option<f64>,
}
```

### The `Connector` Trait

```rust
#[async_trait]
pub trait Connector: Send + Sync {
    fn connector_id(&self) -> &str;
    async fn start(&self, tx: Sender<SourceEvent>) -> Result<(), ConnectorError>;
    async fn stop(&self)  -> Result<(), ConnectorError>;
    async fn health_check(&self) -> Result<(), ConnectorError>;
    fn config(&self) -> &ConnectorConfig;
    fn stats(&self)  -> ConnectorStats;
}
```

Your connector spawns a background Tokio task in `start()`, sends events to `tx`, and stops when `stop()` is called.

### ConnectorConfig

Shared base config for all connectors:

```rust
pub struct ConnectorConfig {
    pub connector_id:   String,
    pub connector_type: String,
    pub url:            Option<String>,
    pub entity_type:    String,   // default entity type produced
    pub enabled:        bool,
    pub trust_score:    f32,      // 0.0–1.0
    pub properties:     HashMap<String, JsonValue>,  // connector-specific settings
}
```

Everything connector-specific goes into `properties`.

---

## 2. Writing a Custom Connector

### Step 1 — Create your file

```
crates/orp-connector/src/adapters/my_connector.rs
```

### Step 2 — Implement the struct

```rust
use crate::traits::{Connector, ConnectorConfig, ConnectorError, ConnectorStats, SourceEvent};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use chrono::Utc;

pub struct MyConnector {
    config: ConnectorConfig,
    running: Arc<AtomicBool>,
    events_count: Arc<AtomicU64>,
    errors_count: Arc<AtomicU64>,
}

impl MyConnector {
    pub fn new(config: ConnectorConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            events_count: Arc::new(AtomicU64::new(0)),
            errors_count: Arc::new(AtomicU64::new(0)),
        }
    }
}
```

### Step 3 — Implement the trait

```rust
#[async_trait]
impl Connector for MyConnector {
    fn connector_id(&self) -> &str { &self.config.connector_id }

    async fn start(&self, tx: tokio::sync::mpsc::Sender<SourceEvent>) -> Result<(), ConnectorError> {
        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let events_count = self.events_count.clone();
        let connector_id = self.config.connector_id.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                // --- fetch / compute your data here ---
                let event = SourceEvent {
                    connector_id: connector_id.clone(),
                    entity_id: "my_entity:001".to_string(),
                    entity_type: "my_type".to_string(),
                    properties: std::collections::HashMap::new(),
                    timestamp: Utc::now(),
                    latitude: None,
                    longitude: None,
                };

                if tx.send(event).await.is_err() { return; }
                events_count.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), ConnectorError> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ConnectorError> {
        if self.running.load(Ordering::SeqCst) { Ok(()) }
        else { Err(ConnectorError::ConnectionError("not running".into())) }
    }

    fn config(&self) -> &ConnectorConfig { &self.config }

    fn stats(&self) -> ConnectorStats {
        ConnectorStats {
            events_processed: self.events_count.load(Ordering::Relaxed),
            errors: self.errors_count.load(Ordering::Relaxed),
            last_event_timestamp: Some(Utc::now()),
            uptime_seconds: 0,
        }
    }
}
```

### Step 4 — Export the module

In `crates/orp-connector/src/adapters/mod.rs`:

```rust
pub mod my_connector;
```

### Step 5 — Register in the connector factory (optional)

If you use a YAML-driven setup, add a match arm in your factory function so the new connector type can be instantiated by name.

### Step 6 — Write tests

Every connector should have at minimum:
- Unit test for its parsing / mapping logic (no network I/O)
- Test that `health_check()` fails before `start()`
- Test that `stats()` starts at zero

---

## 3. Generic REST API Connector

The **`GenericApiConnector`** can connect to any JSON HTTP API without writing Rust code — just provide YAML configuration.

### Features

| Feature | Supported |
|---------|-----------|
| API key (header) | ✅ |
| API key (query param) | ✅ |
| Bearer token | ✅ |
| No auth | ✅ |
| OAuth2 client credentials | Planned |
| Offset pagination | ✅ |
| Page-number pagination | ✅ |
| Cursor pagination | ✅ |
| No pagination | ✅ |
| JSONPath field mapping | ✅ (dot-notation + `[n]` arrays) |
| Static properties | ✅ |
| Configurable polling interval | ✅ |
| Request timeout | ✅ |

### 3.1 Shodan

> Internet-wide device scanner. Discovers exposed services, ports, and vulnerabilities.

**Entity type:** `device`

```yaml
connector_id: shodan-exposed-devices
connector_type: generic_api/shodan
entity_type: device
enabled: true
trust_score: 0.85
properties:
  generic_api:
    url: "https://api.shodan.io/shodan/host/search?key=YOUR_API_KEY&query=port:22+country:US"
    auth:
      type: none          # key is in the URL above
    poll_interval_secs: 300
    timeout_secs: 30
    pagination:
      strategy: page
      page_param: page
      size_param: minify
      page_size: 100
      total_pages_field: "total"
    mapping:
      items_path: "matches"
      id_field: "ip_str"
      lat_field: "location.latitude"
      lon_field: "location.longitude"
      timestamp_field: "timestamp"
      include_fields:
        - ip_str
        - port
        - org
        - hostnames
        - os
        - vulns
        - product
        - version
      static_properties:
        source: shodan
```

**Using the built-in template (Rust):**

```rust
let connector = GenericApiConnector::from_template(
    "shodan-1",      // connector_id
    "shodan",        // template name
    "YOUR_API_KEY",  // injected into auth / URL
    "device",        // entity_type
)?;
```

---

### 3.2 VirusTotal

> Threat intelligence platform. Aggregates malware analysis from 70+ AV engines.

**Entity type:** `ioc` (Indicator of Compromise)

```yaml
connector_id: virustotal-iocs
connector_type: generic_api/virustotal
entity_type: ioc
enabled: true
trust_score: 0.95
properties:
  generic_api:
    url: "https://www.virustotal.com/api/v3/intelligence/search?query=type:file+positives:5+"
    auth:
      type: api_key_header
      header: x-apikey
      key: YOUR_VT_API_KEY
    poll_interval_secs: 600
    timeout_secs: 30
    pagination:
      strategy: cursor
      cursor_param: cursor
      next_cursor_field: "meta.cursor"
    mapping:
      items_path: "data"
      id_field: "id"
      entity_type_field: "type"
      timestamp_field: "attributes.last_analysis_date"
      include_fields:
        - id
        - type
        - attributes.meaningful_name
        - attributes.sha256
        - attributes.last_analysis_stats
        - attributes.names
      static_properties:
        source: virustotal
```

---

### 3.3 AbuseIPDB

> IP reputation database. Identifies malicious IP addresses reported by the community.

**Entity type:** `threat`

```yaml
connector_id: abuseipdb-blacklist
connector_type: generic_api/abuseipdb
entity_type: threat
enabled: true
trust_score: 0.9
properties:
  generic_api:
    url: "https://api.abuseipdb.com/api/v2/blacklist?confidenceMinimum=90&limit=10000"
    headers:
      Accept: application/json
    auth:
      type: api_key_header
      header: Key
      key: YOUR_ABUSEIPDB_API_KEY
    poll_interval_secs: 3600
    timeout_secs: 60
    pagination:
      strategy: none
    mapping:
      items_path: "data"
      id_field: "ipAddress"
      timestamp_field: "lastReportedAt"
      include_fields:
        - ipAddress
        - abuseConfidenceScore
        - countryCode
        - usageType
        - isp
        - domain
        - totalReports
      static_properties:
        source: abuseipdb
        threat_type: malicious_ip
```

---

### 3.4 FlightAware

> Aviation tracking. Real-time flight positions via AeroAPI.

**Entity type:** `aircraft`

```yaml
connector_id: flightaware-live
connector_type: generic_api/flightaware
entity_type: aircraft
enabled: true
trust_score: 0.9
properties:
  generic_api:
    url: 'https://aeroapi.flightaware.com/aeroapi/flights/search?query=-latlong "25 -130 50 -60"'
    auth:
      type: api_key_header
      header: x-apikey
      key: YOUR_FLIGHTAWARE_API_KEY
    poll_interval_secs: 60
    timeout_secs: 30
    pagination:
      strategy: cursor
      cursor_param: cursor
      next_cursor_field: "next"
    mapping:
      items_path: "flights"
      id_field: "fa_flight_id"
      lat_field: "last_position.latitude"
      lon_field: "last_position.longitude"
      timestamp_field: "last_position.timestamp"
      include_fields:
        - fa_flight_id
        - ident
        - aircraft_type
        - origin.code
        - destination.code
        - status
        - last_position.altitude
        - last_position.groundspeed
        - last_position.heading
      static_properties:
        source: flightaware
```

---

### 3.5 OpenWeatherMap

> Weather station data. Current conditions from thousands of stations worldwide.

**Entity type:** `weather_station`

```yaml
connector_id: owm-stations
connector_type: generic_api/openweathermap
entity_type: weather_station
enabled: true
trust_score: 0.8
properties:
  generic_api:
    # bbox search: ?bbox=lon_left,lat_bottom,lon_right,lat_top,zoom
    url: "https://api.openweathermap.org/data/2.5/box/city?bbox=-180,-90,180,90,2&appid=YOUR_OWM_KEY"
    auth:
      type: none   # key embedded in URL
    poll_interval_secs: 600
    timeout_secs: 30
    pagination:
      strategy: none
    mapping:
      items_path: "list"
      id_field: "id"
      lat_field: "coord.lat"
      lon_field: "coord.lon"
      timestamp_field: "dt"
      include_fields:
        - name
        - weather
        - main.temp
        - main.humidity
        - wind.speed
        - wind.deg
        - clouds.all
        - visibility
      static_properties:
        source: openweathermap
```

---

### 3.6 USGS Earthquakes

> Real-time USGS earthquake feed. No API key required.

**Entity type:** `earthquake`

```yaml
connector_id: usgs-earthquakes
connector_type: generic_api/usgs
entity_type: earthquake
enabled: true
trust_score: 1.0
properties:
  generic_api:
    # Options: all_hour, all_day, all_week, significant_hour, significant_day, 2.5_day …
    url: "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_day.geojson"
    auth:
      type: none
    poll_interval_secs: 300
    timeout_secs: 30
    pagination:
      strategy: none
    mapping:
      items_path: "features"
      id_field: "id"
      # GeoJSON geometry.coordinates = [lon, lat, depth]
      lat_field: "geometry.coordinates[1]"
      lon_field: "geometry.coordinates[0]"
      timestamp_field: "properties.time"
      include_fields:
        - properties.mag
        - properties.place
        - properties.status
        - properties.alert
        - properties.tsunami
        - properties.sig
      static_properties:
        source: usgs
```

---

### 3.7 Custom REST API

Template for any API not listed above.

```yaml
connector_id: my-custom-api
connector_type: generic_api/custom
entity_type: asset
enabled: true
trust_score: 0.8
properties:
  generic_api:
    url: "https://api.example.com/v1/assets"
    headers:
      Accept: application/json
      X-Custom-Header: my-value
    auth:
      # Choose ONE of:
      # API key in header:
      type: api_key_header
      header: Authorization
      key: "Token YOUR_API_KEY_HERE"

      # OR API key as query param:
      # type: api_key_query
      # param: api_key
      # key: YOUR_API_KEY_HERE

      # OR Bearer token:
      # type: bearer
      # token: my-bearer-token

      # OR no auth:
      # type: none

    poll_interval_secs: 60
    timeout_secs: 30

    pagination:
      # No pagination:
      strategy: none

      # OR offset:
      # strategy: offset
      # offset_param: offset
      # limit_param: limit
      # page_size: 100

      # OR page number:
      # strategy: page
      # page_param: page
      # size_param: per_page
      # page_size: 50
      # total_pages_field: "meta.total_pages"

      # OR cursor:
      # strategy: cursor
      # cursor_param: after
      # next_cursor_field: "paging.next_cursor"

    mapping:
      # Dot-path to the array of items in the response.
      # Leave empty "" for root-level arrays.
      items_path: "data"

      # Field whose value becomes the entity_id.
      id_field: "uuid"

      # Optional geo fields.
      lat_field: "location.lat"
      lon_field: "location.lng"

      # Optional timestamp field (ISO-8601 string or Unix epoch).
      timestamp_field: "created_at"

      # If the entity type should come from a field in the item:
      # entity_type_field: "type"

      # Whitelist of fields to include (empty = all).
      include_fields:
        - uuid
        - name
        - status
        - tags

      # Fields to always strip (e.g. internal IDs, PII).
      exclude_fields:
        - internal_ref
        - user_email

      # Merged into every entity's properties.
      static_properties:
        source: my-api
        environment: production
```

---

## 4. Syslog / CEF Connector

The `SyslogConnector` listens on a UDP or TCP port and ingests RFC 5424 / RFC 3164 syslog messages. It automatically detects and parses **CEF** (Common Event Format) embedded in syslog lines.

### Entity types produced

| Type | When |
|------|------|
| `threat` | CEF severity ≥ 7, IDS/IPS product name, or "attack/malware/exploit/intrusion" in message |
| `vulnerability` | CEF product name contains "vuln/qualys/nessus/openvas" |
| `network_event` | CEF firewall product, or "denied/blocked/drop/firewall/nat" in message |
| `host` | Everything else |

### YAML Configuration

```yaml
connector_id: syslog-firewall
connector_type: syslog
entity_type: network_event
enabled: true
trust_score: 0.9
properties:
  bind_addr: "0.0.0.0:514"   # UDP port 514 (standard syslog)
  transport: udp              # "udp" | "tcp" | "both"
  parse_cef: true             # attempt CEF parsing on every line
```

**TCP syslog (e.g. for TLS-terminated relays):**

```yaml
connector_id: syslog-tcp
connector_type: syslog
entity_type: host
enabled: true
trust_score: 0.9
properties:
  bind_addr: "0.0.0.0:601"   # RFC 3195 TCP syslog
  transport: tcp
  parse_cef: false
```

**Both UDP and TCP simultaneously:**

```yaml
connector_id: syslog-dual
connector_type: syslog
entity_type: host
enabled: true
trust_score: 0.9
properties:
  bind_addr: "0.0.0.0:514"
  transport: both             # UDP on :514, TCP on :515
  parse_cef: true
```

### Forwarding devices to ORP

**Cisco ASA / Firepower:**
```
logging enable
logging trap informational
logging host inside <ORP_SERVER_IP> 514
logging device-id hostname
```

**Fortinet FortiGate:**
```
config log syslogd setting
  set status enable
  set server <ORP_SERVER_IP>
  set port 514
  set format cef
end
```

**Snort / Suricata IDS (CEF output):**
```yaml
# suricata.yaml
outputs:
  - syslog:
      enabled: yes
      facility: local5
      format: cef
      identity: suricata
```

**Linux rsyslog → ORP:**
```
# /etc/rsyslog.d/orp.conf
*.* @<ORP_SERVER_IP>:514   # UDP
*.* @@<ORP_SERVER_IP>:601  # TCP
```

### Using in Rust

```rust
use orp_connector::adapters::syslog::{SyslogConnector, SyslogConfig, SyslogTransport};
use orp_connector::traits::ConnectorConfig;

let config = ConnectorConfig {
    connector_id: "fw-syslog".to_string(),
    connector_type: "syslog".to_string(),
    url: None,
    entity_type: "network_event".to_string(),
    enabled: true,
    trust_score: 0.9,
    properties: Default::default(),
};

let syslog_config = SyslogConfig {
    bind_addr: "0.0.0.0:5514".to_string(), // non-privileged port for dev
    transport: SyslogTransport::Udp,
    parse_cef: true,
    ..Default::default()
};

let connector = SyslogConnector::new(config, syslog_config);
let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
connector.start(tx).await?;
```

### Parsing logic

**RFC 5424** format is detected when the message starts with `<NNN>1 ` (digit 1 is the version).
**RFC 3164** is assumed otherwise (legacy BSD syslog).

**CEF** is detected by scanning for `CEF:` anywhere in the line. Fields are parsed as `key=value` pairs with proper handling of values containing spaces (e.g. `msg=Login failed for user root`).

---

## 5. Database Connector

The `DatabaseConnector` polls any SQL database on a configurable interval and maps each row to a `SourceEvent`.

### Supported databases

| Database | Connection string prefix |
|----------|--------------------------|
| PostgreSQL | `postgres://` or `postgresql://` |
| MySQL / MariaDB | `mysql://` or `mariadb://` |
| SQLite | `sqlite://` or `sqlite:` |

> **Note:** The connector ships a `QueryExecutor` trait. In the default build it uses `NoopExecutor` (returns no rows). Swap in a real `sqlx`-backed executor for production (see below).

### YAML Configuration

```yaml
connector_id: pg-assets
connector_type: database
entity_type: asset
enabled: true
trust_score: 0.8
properties:
  connection_string: "${env.DATABASE_URL}"  # e.g. postgres://host:5432/orp_prod
  query: "SELECT id, hostname, ip_address, lat, lon, last_seen FROM assets ORDER BY last_seen DESC LIMIT 1000"
  poll_interval_secs: 30
  id_field: id
  lat_field: lat
  lon_field: lon
  timestamp_field: last_seen
```

### Incremental ingestion (watermark)

For large tables use `watermark_field` to avoid full scans. The connector passes the last-seen maximum value as `$1` (Postgres) or `?` (MySQL/SQLite) in the query.

```yaml
connector_id: pg-events-incremental
connector_type: database
entity_type: security_event
enabled: true
trust_score: 0.85
properties:
  connection_string: "${env.SECURITY_DB_URL}"  # e.g. postgres://localhost/security
  # On the first poll $1 = NULL (no watermark yet) — handle with COALESCE:
  query: >
    SELECT id, src_ip, dst_ip, event_type, severity, lat, lon, created_at
    FROM security_events
    WHERE created_at > COALESCE($1::timestamptz, '1970-01-01')
    ORDER BY created_at ASC
    LIMIT 5000
  poll_interval_secs: 10
  id_field: id
  timestamp_field: created_at
  watermark_field: created_at     # tracks highest created_at seen
  exclude_columns:
    - raw_payload     # large binary — omit from properties
    - internal_notes
```

### MySQL example

```yaml
connector_id: mysql-inventory
connector_type: database
entity_type: device
enabled: true
trust_score: 0.8
properties:
  connection_string: "${env.MYSQL_URL}"  # e.g. mysql://mysql.example.com:3306/inventory
  query: "SELECT device_id AS id, name, ip, latitude AS lat, longitude AS lon FROM devices WHERE active = 1"
  poll_interval_secs: 60
  id_field: id
  lat_field: lat
  lon_field: lon
```

### SQLite example (local audit log)

```yaml
connector_id: sqlite-audit
connector_type: database
entity_type: audit_record
enabled: true
trust_score: 1.0
properties:
  connection_string: "sqlite:///var/log/orp/audit.db"
  query: "SELECT rowid AS id, action, user, resource, ts FROM audit_log ORDER BY ts DESC LIMIT 500"
  poll_interval_secs: 5
  id_field: id
  timestamp_field: ts
```

### Injecting a real sqlx executor (Rust)

```rust
use orp_connector::adapters::database::{DatabaseConnector, QueryExecutor, DatabaseConfig};
use orp_connector::traits::ConnectorError;
use async_trait::async_trait;
use std::collections::HashMap;
use serde_json::Value as JsonValue;
use sqlx::PgPool;

/// Real PostgreSQL executor backed by sqlx.
pub struct PgExecutor(pub PgPool);

#[async_trait]
impl QueryExecutor for PgExecutor {
    async fn execute(
        &self,
        query: &str,
        watermark: Option<&str>,
    ) -> Result<Vec<HashMap<String, JsonValue>>, ConnectorError> {
        let rows = sqlx::query(query)
            .bind(watermark)
            .fetch_all(&self.0)
            .await
            .map_err(|e| ConnectorError::ConnectionError(e.to_string()))?;

        let mut result = vec![];
        for row in rows {
            use sqlx::Row;
            let mut map = HashMap::new();
            for col in row.columns() {
                let val: Option<String> = row.try_get(col.name()).ok();
                map.insert(col.name().to_string(), serde_json::json!(val));
            }
            result.push(map);
        }
        Ok(result)
    }
}

// Wiring it up:
let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap()).await?;
let connector = DatabaseConnector::from_connector_config(config)?
    .with_executor(std::sync::Arc::new(PgExecutor(pool)));
```

---

## 6. Built-in Connectors Reference

| Module | Description | Entity types |
|--------|-------------|--------------|
| `ais` | AIS NMEA TCP stream + CSV | `ship` |
| `adsb` | ADS-B aircraft positions | `aircraft` |
| `http_poller` | Simple JSON HTTP polling | configurable |
| `mqtt` | MQTT topic subscription | configurable |
| `websocket_client` | WebSocket JSON stream | configurable |
| `csv_watcher` | File-system CSV watcher | configurable |
| `generic_api` | Universal REST API | configurable |
| `syslog` | RFC 5424/3164 + CEF | `host`, `network_event`, `threat`, `vulnerability` |
| `database` | SQL database polling | configurable |

---

## 7. YAML Configuration Reference

### Auth types

```yaml
# No authentication
auth:
  type: none

# API key sent as HTTP header
auth:
  type: api_key_header
  header: X-Api-Key          # header name
  key: abc123                # key value

# API key as query parameter
auth:
  type: api_key_query
  param: apikey              # query param name
  key: abc123

# Bearer token
auth:
  type: bearer
  token: eyJhbGci...
```

### Pagination strategies

```yaml
# No pagination
pagination:
  strategy: none

# Offset / limit  (?offset=0&limit=100)
pagination:
  strategy: offset
  offset_param: offset
  limit_param: limit
  page_size: 100
  total_field: "meta.total"  # optional

# Page number  (?page=1&per_page=50)
pagination:
  strategy: page
  page_param: page
  size_param: per_page
  page_size: 50
  total_pages_field: "meta.pages"  # optional

# Cursor  (?after=TOKEN)
pagination:
  strategy: cursor
  cursor_param: after
  next_cursor_field: "paging.next"
```

### Mapping

```yaml
mapping:
  items_path: "data.results"    # dot-path; "" = root array
  id_field: "uuid"              # required
  lat_field: "geo.lat"          # optional, supports dot-notation
  lon_field: "geo.lon"
  timestamp_field: "created_at" # ISO-8601 or Unix epoch (s or ms)
  entity_type_field: "kind"     # derive entity_type from a field
  include_fields: []            # empty = include all
  exclude_fields:
    - password_hash
    - internal_id
  static_properties:
    source: my-api
    env: production
```

---

## 8. Troubleshooting

### Events not appearing

1. Check `health_check()` returns `Ok`.
2. Check `stats().errors` — if non-zero, look at logs with `RUST_LOG=orp_connector=debug`.
3. For HTTP connectors: try `curl` with the same URL and headers.
4. For syslog: verify firewall allows UDP/TCP on the bind port. Test with `logger -n <host> -P 514 "test message"`.
5. For database: verify the connection string is correct and the ORP process has network access to the DB host.

### High error count on generic_api connector

- Check `timeout_secs` — increase if the upstream API is slow.
- Check rate limits: reduce poll frequency or add backoff.
- Verify `auth` config: wrong header name or key format.

### Syslog messages missing CEF data

- Confirm the device is configured to output CEF format (not just plain syslog).
- Check that `parse_cef: true` is set.
- Use `SyslogConnector::process_line(raw, "test", true)` in a unit test to debug parsing.

### Database connector sends stale data

- Add a `watermark_field` pointing to an indexed `updated_at` or `created_at` column.
- Ensure the query uses `WHERE updated_at > COALESCE($1, '1970-01-01')`.
- Reduce `poll_interval_secs`.

### Building with real database support

Add `sqlx` to `Cargo.toml` with the appropriate features:

```toml
[dependencies]
sqlx = { version = "0.7", features = ["runtime-tokio-rustls", "postgres", "mysql", "sqlite", "chrono", "uuid"] }
```

Then implement `QueryExecutor` for your pool type (see §5 above).
