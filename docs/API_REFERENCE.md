# ORP API Reference

**Version:** v1 · **Base URL:** `http://localhost:9090/api/v1` · **Format:** JSON

All endpoints return `Content-Type: application/json`. Authentication is Bearer token (OIDC JWT) or `X-API-Key` header when auth is enabled.

---

## Table of Contents

- [Authentication](#authentication)
- [Errors](#errors)
- [Pagination](#pagination)
- [Entities](#entities)
- [Queries](#queries)
- [Graph](#graph)
- [Connectors](#connectors)
- [Monitors & Alerts](#monitors--alerts)
- [System](#system)
- [WebSocket](#websocket)

---

## Authentication

```bash
# Bearer token (OIDC)
curl http://localhost:9090/api/v1/entities \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiJ9..."

# API Key
curl http://localhost:9090/api/v1/entities \
  -H "X-API-Key: YOUR_API_KEY_HERE"

# No auth (development mode — auth.mode = "none")
curl http://localhost:9090/api/v1/entities
```

---

## Errors

All error responses follow this format:

```json
{
  "error": {
    "code": "NOT_FOUND",
    "status": 404,
    "message": "Entity with id 'mmsi:999999999' not found",
    "details": { "id": "mmsi:999999999", "type": "Ship" },
    "trace_id": "req_abc123",
    "timestamp": "2026-03-26T14:30:00Z"
  }
}
```

| Code | HTTP | Meaning |
|------|------|---------|
| `INVALID_REQUEST` | 400 | Malformed JSON or missing required fields |
| `INVALID_QUERY` | 400 | ORP-QL syntax error |
| `UNAUTHORIZED` | 401 | Missing or expired token |
| `FORBIDDEN` | 403 | Insufficient permissions |
| `NOT_FOUND` | 404 | Entity or resource not found |
| `RATE_LIMITED` | 429 | Too many requests |
| `INTERNAL_ERROR` | 500 | Server error |

---

## Pagination

List endpoints accept:

| Param | Default | Max | Description |
|-------|---------|-----|-------------|
| `page` | 1 | — | Page number (1-indexed) |
| `limit` | 100 | 1000 | Items per page |
| `sort_by` | `created_at` | — | Field to sort by |
| `sort_order` | `desc` | — | `asc` or `desc` |

Response wrapper:

```json
{
  "data": [...],
  "pagination": {
    "page": 1, "limit": 100, "total_count": 5432,
    "total_pages": 55, "has_next": true, "has_prev": false
  }
}
```

---

## Entities

### GET /entities — List Entities

Fetch a paginated, filterable list of entities.

**Query Parameters:**

| Param | Type | Description |
|-------|------|-------------|
| `type` | string | Filter by entity type (e.g., `Ship`, `Port`, `Aircraft`) |
| `tags` | string[] | Filter by tags (OR logic). Repeat param: `?tags=maritime&tags=active` |
| `created_after` | ISO 8601 | Entities created after this time |
| `updated_after` | ISO 8601 | Entities updated after this time |
| `page` | int | Page number |
| `limit` | int | Items per page |

```bash
# All ships
curl "http://localhost:9090/api/v1/entities?type=Ship&limit=10"

# Entities updated in last hour
curl "http://localhost:9090/api/v1/entities?updated_after=2026-03-26T13:00:00Z"

# Tagged entities
curl "http://localhost:9090/api/v1/entities?tags=maritime&tags=watch-list"
```

**Response:**

```json
{
  "data": [
    {
      "id": "mmsi:353136000",
      "type": "Ship",
      "name": "EVER GIVEN",
      "tags": ["cargo", "suez-canal"],
      "properties": {
        "mmsi": 353136000,
        "ship_type": "cargo",
        "flag": "PA",
        "imo": "9811000",
        "speed": 12.3,
        "course": 245.0,
        "heading": 243.0,
        "destination": "NLRTM",
        "draught": 14.5,
        "length": 400,
        "beam": 59
      },
      "geometry": {
        "type": "Point",
        "coordinates": [4.4792, 51.9225]
      },
      "source_id": "ais-global",
      "confidence": 0.95,
      "created_at": "2026-03-26T08:00:00Z",
      "updated_at": "2026-03-26T14:32:15Z"
    }
  ],
  "pagination": { "page": 1, "limit": 10, "total_count": 2847, "total_pages": 285 }
}
```

---

### GET /entities/{id} — Get Entity

Full entity details including all properties and relationships.

```bash
curl http://localhost:9090/api/v1/entities/mmsi:353136000
```

**Response:**

```json
{
  "id": "mmsi:353136000",
  "type": "Ship",
  "name": "EVER GIVEN",
  "tags": ["cargo"],
  "properties": { ... },
  "geometry": { "type": "Point", "coordinates": [4.4792, 51.9225] },
  "relationships": [
    {
      "rel_type": "HEADING_TO",
      "direction": "outbound",
      "target": { "id": "port-rotterdam", "type": "Port", "name": "Rotterdam" },
      "properties": { "eta": "2026-03-26T18:00:00Z", "distance_km": 82.4 }
    },
    {
      "rel_type": "OWNS",
      "direction": "inbound",
      "source": { "id": "org-evergreen", "type": "Organization", "name": "Evergreen Marine Corp." }
    }
  ],
  "data_quality": {
    "freshness_secs": 47,
    "confidence": 0.95,
    "source_count": 1,
    "consistency": "consistent"
  },
  "source_id": "ais-global",
  "created_at": "2026-03-26T08:00:00Z",
  "updated_at": "2026-03-26T14:32:15Z"
}
```

---

### GET /entities/search — Search Entities

Geospatial, property, and type search.

**Query Parameters:**

| Param | Type | Description |
|-------|------|-------------|
| `lat` | float | Center latitude |
| `lon` | float | Center longitude |
| `radius_km` | float | Search radius in kilometers |
| `bbox` | string | Bounding box: `min_lon,min_lat,max_lon,max_lat` |
| `type` | string | Entity type filter |
| `q` | string | Full-text search on name |
| `property.*` | any | Filter by property: `?property.speed_gt=20` |

```bash
# Ships within 50km of Rotterdam
curl "http://localhost:9090/api/v1/entities/search?lat=51.9225&lon=4.4792&radius_km=50&type=Ship"

# Ships in North Sea bounding box
curl "http://localhost:9090/api/v1/entities/search?bbox=-5,50,10,60&type=Ship"

# Fast ships (custom property filter)
curl "http://localhost:9090/api/v1/entities/search?type=Ship&property.speed_gt=20"

# Search by name
curl "http://localhost:9090/api/v1/entities/search?q=ever+given"
```

---

### GET /entities/{id}/relationships — Get Relationships

```bash
curl http://localhost:9090/api/v1/entities/mmsi:353136000/relationships

# Filter by relationship type
curl "http://localhost:9090/api/v1/entities/mmsi:353136000/relationships?type=HEADING_TO"

# Only outbound relationships
curl "http://localhost:9090/api/v1/entities/mmsi:353136000/relationships?direction=outbound"
```

**Response:**

```json
{
  "data": [
    {
      "relationship_id": "rel-abc123",
      "rel_type": "HEADING_TO",
      "source_id": "mmsi:353136000",
      "target_id": "port-rotterdam",
      "target": {
        "id": "port-rotterdam",
        "type": "Port",
        "name": "Rotterdam",
        "geometry": { "type": "Point", "coordinates": [4.4792, 51.9225] }
      },
      "properties": {
        "eta": "2026-03-26T18:00:00Z",
        "distance_km": 82.4
      },
      "confidence": 0.85,
      "created_at": "2026-03-26T12:00:00Z"
    }
  ]
}
```

---

### GET /entities/{id}/events — Get Event History

Retrieve the stream of events (state changes, position updates) for an entity.

```bash
# Last 100 events
curl http://localhost:9090/api/v1/entities/mmsi:353136000/events

# Events from last 6 hours
curl "http://localhost:9090/api/v1/entities/mmsi:353136000/events?since=6h"

# Only position updates
curl "http://localhost:9090/api/v1/entities/mmsi:353136000/events?type=PositionUpdate"
```

**Response:**

```json
{
  "data": [
    {
      "event_id": "01965a3b-7c4d-7def-8a12-3456789abcde",
      "entity_id": "mmsi:353136000",
      "event_type": "PositionUpdate",
      "timestamp": "2026-03-26T14:32:15Z",
      "geo": { "lat": 51.9225, "lon": 4.4792, "alt": null },
      "payload": {
        "type": "PositionUpdate",
        "course": 245.0,
        "speed": 12.3,
        "heading": 243.0
      },
      "source_id": "ais-global",
      "confidence": 0.95
    }
  ]
}
```

---

### POST /entities — Create Entity

Manually create an entity (not from a connector).

```bash
curl -X POST http://localhost:9090/api/v1/entities \
  -H "Content-Type: application/json" \
  -d '{
    "id": "custom-buoy-001",
    "type": "Buoy",
    "name": "North Sea Observation Buoy #1",
    "tags": ["monitoring", "weather"],
    "properties": {
      "buoy_type": "weather",
      "operator": "KNMI",
      "water_depth_m": 42
    },
    "geometry": {
      "type": "Point",
      "coordinates": [3.25, 53.40]
    }
  }'
```

**Response:** `201 Created` with the created entity.

---

### PATCH /entities/{id} — Update Entity

Update properties of an entity.

```bash
curl -X PATCH http://localhost:9090/api/v1/entities/custom-buoy-001 \
  -H "Content-Type: application/json" \
  -d '{
    "properties": {
      "last_maintenance": "2026-03-01",
      "status": "operational"
    },
    "tags": ["monitoring", "weather", "maintained"]
  }'
```

---

### DELETE /entities/{id} — Delete Entity

Soft-deletes an entity (sets `is_active = false`).

```bash
curl -X DELETE http://localhost:9090/api/v1/entities/custom-buoy-001

# Hard delete (requires admin permission)
curl -X DELETE "http://localhost:9090/api/v1/entities/custom-buoy-001?hard=true"
```

---

## Queries

### POST /query — Execute ORP-QL

Run an ORP-QL query against the entity graph.

```bash
curl -X POST http://localhost:9090/api/v1/query \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship) WHERE s.speed > 20 RETURN s.name, s.speed ORDER BY s.speed DESC LIMIT 10",
    "timeout_ms": 5000
  }'
```

**Response:**

```json
{
  "results": [
    { "s.name": "MAERSK SPEED", "s.speed": 23.1 },
    { "s.name": "MSC VELOCITY", "s.speed": 22.8 }
  ],
  "meta": {
    "result_count": 2,
    "execution_ms": 145,
    "engine": "duckdb",
    "query_plan": "SeqScan(Ship) → Filter(speed > 20) → Sort → Limit"
  }
}
```

**Request Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | ORP-QL query string |
| `timeout_ms` | int | No | Max execution time (default: 30000) |
| `explain` | bool | No | Return query plan without executing |

---

### POST /query/natural — Natural Language Query _(Phase 2)_

```bash
curl -X POST http://localhost:9090/api/v1/query/natural \
  -H "Content-Type: application/json" \
  -d '{
    "question": "Which ships near Rotterdam are going faster than 15 knots?"
  }'
```

**Response:**

```json
{
  "interpreted_query": "MATCH (s:Ship) WHERE near(s.position, point(51.9225, 4.4792), 50km) AND s.speed > 15 RETURN s.name, s.speed ORDER BY s.speed DESC",
  "confidence": 0.93,
  "results": [...],
  "meta": { "execution_ms": 1240, "nl_model": "phi-2-q4k" }
}
```

---

## Graph

### POST /graph — Execute Cypher Query

Execute a raw Kuzu Cypher query for advanced graph traversal.

```bash
curl -X POST http://localhost:9090/api/v1/graph \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship)-[:HEADING_TO]->(p:Port)<-[:OPERATES]-(org:Organization) WHERE p.congestion > 0.8 RETURN s.name, p.name, org.name, p.congestion ORDER BY p.congestion DESC"
  }'
```

**Response:**

```json
{
  "results": [
    {
      "s.name": "EVER GIVEN",
      "p.name": "Rotterdam",
      "org.name": "Port of Rotterdam Authority",
      "p.congestion": 0.92
    }
  ],
  "meta": {
    "result_count": 1,
    "execution_ms": 312,
    "engine": "kuzu"
  }
}
```

---

## Connectors

### GET /connectors — List Connectors

```bash
curl http://localhost:9090/api/v1/connectors
```

**Response:**

```json
{
  "data": [
    {
      "id": "ais-global",
      "type": "ais",
      "status": "running",
      "config": {
        "host": "153.44.253.27",
        "port": 9999
      },
      "metrics": {
        "events_ingested": 8472931,
        "events_per_sec": 28430,
        "errors_last_hour": 0,
        "latency_p50_ms": 12,
        "last_event_at": "2026-03-26T14:32:15Z"
      },
      "health": "healthy",
      "uptime_secs": 7200
    }
  ]
}
```

---

### POST /connectors — Register Connector

```bash
curl -X POST http://localhost:9090/api/v1/connectors \
  -H "Content-Type: application/json" \
  -d '{
    "id": "my-iot-sensor",
    "type": "mqtt",
    "config": {
      "broker": "mqtt://broker.example.com:1883",
      "topics": ["sensors/+/temperature", "sensors/+/humidity"],
      "client_id": "orp-sensor-connector",
      "entity_type": "SensorReading"
    }
  }'
```

---

### GET /connectors/{id} — Get Connector Status

```bash
curl http://localhost:9090/api/v1/connectors/ais-global
```

---

### DELETE /connectors/{id} — Deregister Connector

```bash
curl -X DELETE http://localhost:9090/api/v1/connectors/my-iot-sensor
```

---

## Monitors & Alerts

### GET /monitors — List Monitors

```bash
curl http://localhost:9090/api/v1/monitors
```

**Response:**

```json
{
  "data": [
    {
      "id": "mon-001",
      "name": "Speed Anomaly",
      "enabled": true,
      "rule": {
        "type": "property_threshold",
        "entity_type": "Ship",
        "condition": "ship.speed > 25"
      },
      "severity": "WARNING",
      "fired_count": 14,
      "last_fired_at": "2026-03-26T14:20:00Z"
    }
  ]
}
```

---

### POST /monitors — Create Monitor

```bash
curl -X POST http://localhost:9090/api/v1/monitors \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Route Deviation Alert",
    "enabled": true,
    "rule": {
      "type": "route_deviation",
      "entity_type": "Ship",
      "deviation_km": 50,
      "min_speed_kn": 5
    },
    "severity": "CRITICAL",
    "debounce_seconds": 300,
    "actions": [
      { "type": "console" },
      {
        "type": "webhook",
        "url": "https://hooks.slack.com/YOUR/WEBHOOK",
        "payload_template": "{\"text\": \"🚨 {{entity.name}} deviated {{deviation_km}}km from route\"}"
      }
    ]
  }'
```

---

### DELETE /monitors/{id} — Delete Monitor

```bash
curl -X DELETE http://localhost:9090/api/v1/monitors/mon-001
```

---

### GET /alerts — List Alerts

```bash
# All unacknowledged alerts
curl http://localhost:9090/api/v1/alerts

# Alerts in last 24 hours
curl "http://localhost:9090/api/v1/alerts?since=24h"

# By severity
curl "http://localhost:9090/api/v1/alerts?severity=CRITICAL"
```

**Response:**

```json
{
  "data": [
    {
      "id": "alert-xyz789",
      "monitor_id": "mon-001",
      "monitor_name": "Speed Anomaly",
      "severity": "WARNING",
      "entity_id": "mmsi:123456789",
      "entity_name": "FAST FISHER",
      "message": "Vessel speed 27.3 kn exceeds threshold 25 kn",
      "evidence": {
        "property": "speed",
        "value": 27.3,
        "threshold": 25.0,
        "position": { "lat": 51.5, "lon": 3.2 }
      },
      "fired_at": "2026-03-26T14:20:00Z",
      "acknowledged": false
    }
  ]
}
```

---

### POST /alerts/{id}/acknowledge — Acknowledge Alert

```bash
curl -X POST http://localhost:9090/api/v1/alerts/alert-xyz789/acknowledge \
  -H "Content-Type: application/json" \
  -d '{
    "note": "Investigated — vessel has speed trial permit until 16:00 UTC",
    "acknowledged_by": "user:alice"
  }'
```

---

## System

### GET /health — System Health

```bash
curl http://localhost:9090/api/v1/health
```

**Response:**

```json
{
  "status": "healthy",
  "version": "0.1.0",
  "uptime_secs": 7245,
  "components": {
    "duckdb": { "status": "healthy", "entities": 2847, "size_mb": 142 },
    "kuzu": { "status": "healthy", "nodes": 2891, "edges": 4123, "lag_secs": 18 },
    "rocksdb": { "status": "healthy", "dedup_window_size": 1240000 },
    "stream_processor": {
      "status": "healthy",
      "events_per_sec": 28430,
      "queue_depth": 420,
      "batch_latency_ms": 45
    }
  },
  "connectors": {
    "ais-global": "healthy",
    "weather-noaa": "healthy",
    "osm-ports": "healthy"
  }
}
```

---

### GET /metrics — Prometheus Metrics

```bash
curl http://localhost:9090/api/v1/metrics
```

Returns Prometheus text format:

```
# HELP orp_entities_total Total number of active entities
# TYPE orp_entities_total gauge
orp_entities_total{type="Ship"} 2847
orp_entities_total{type="Port"} 44
orp_entities_total{type="WeatherSystem"} 3

# HELP orp_events_ingested_total Total events ingested since start
# TYPE orp_events_ingested_total counter
orp_events_ingested_total{connector="ais-global"} 8472931
orp_events_ingested_total{connector="weather-noaa"} 1440

# HELP orp_query_duration_seconds Query execution duration
# TYPE orp_query_duration_seconds histogram
orp_query_duration_seconds_bucket{engine="duckdb",le="0.1"} 9823
orp_query_duration_seconds_bucket{engine="duckdb",le="0.5"} 9971
...
```

---

### GET /version — Version Info

```bash
curl http://localhost:9090/api/v1/version
```

```json
{
  "version": "0.1.0",
  "git_commit": "abc1234def567",
  "build_date": "2026-03-26T00:00:00Z",
  "rust_version": "1.76.0",
  "features": ["ais", "adsb", "mqtt", "http_poll", "oidc"],
  "binary_size_mb": 298
}
```

---

## WebSocket

### WS /ws/updates — Real-Time Updates

Connect with any WebSocket client:

```bash
# Using websocat CLI
websocat ws://localhost:9090/ws/updates

# Using wscat
wscat -c ws://localhost:9090/ws/updates
```

**Message Protocol:**

#### Client → Server: Auth

```json
{ "type": "auth", "token": "Bearer eyJ..." }
```

Response:
```json
{ "type": "auth_ok", "user_id": "user-123", "permissions": ["entities:read"] }
```

#### Client → Server: Subscribe

```json
{
  "type": "subscribe",
  "subscription_id": "north-sea-ships",
  "filter": {
    "entity_types": ["Ship"],
    "bbox": {
      "min_lat": 50.0, "max_lat": 58.0,
      "min_lon": -5.0, "max_lon": 10.0
    },
    "update_interval_ms": 1000
  }
}
```

Subscribe to a specific entity:
```json
{
  "type": "subscribe",
  "subscription_id": "watch-ever-given",
  "filter": { "entity_ids": ["mmsi:353136000"] }
}
```

#### Server → Client: Entity Update

```json
{
  "type": "entity_update",
  "subscription_id": "north-sea-ships",
  "entity": {
    "id": "mmsi:353136000",
    "type": "Ship",
    "name": "EVER GIVEN",
    "geometry": { "type": "Point", "coordinates": [4.4792, 51.9225] },
    "properties": { "speed": 12.4, "course": 246.0 },
    "updated_at": "2026-03-26T14:32:16Z"
  }
}
```

#### Server → Client: Alert

```json
{
  "type": "alert",
  "alert": {
    "id": "alert-xyz789",
    "severity": "WARNING",
    "entity_id": "mmsi:123456789",
    "entity_name": "FAST FISHER",
    "message": "Speed anomaly: 27.3 kn",
    "fired_at": "2026-03-26T14:32:00Z"
  }
}
```

#### Client → Server: Unsubscribe

```json
{ "type": "unsubscribe", "subscription_id": "north-sea-ships" }
```

#### Keep-Alive

```json
{ "type": "ping" }
```

```json
{ "type": "pong" }
```

---

## Rate Limits

| Tier | Limit | Window |
|------|-------|--------|
| Default (API key) | 1,000 req | per second |
| Query endpoint | 100 req | per second |
| WebSocket connections | 100 | per IP |

Rate limit response headers:

```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 947
X-RateLimit-Reset: 1711411260
```

When exceeded: `429 Too Many Requests` with `Retry-After: 1` header.

---

_For ORP-QL syntax reference, see [ORP_QL_GUIDE.md](ORP_QL_GUIDE.md)._
_For security and authentication details, see [SECURITY.md](SECURITY.md)._
