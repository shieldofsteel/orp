# ORP API & Frontend Specification

**Version:** 1.0 (Phase 1 — Maritime MVP)
**Date:** March 26, 2026
**Audience:** 490 senior engineers (ex-Microsoft, Apple, Palantir, NVIDIA)
**Status:** Production specification. This is the contract between backend and frontend. This must be exact.

---

## Section 0: Document Scope

This specification covers:

1. **REST API v1** — All endpoints, request/response schemas, error handling, authentication, pagination
2. **WebSocket Protocol** — Real-time entity updates, message formats, subscriptions
3. **Frontend Architecture** — React component tree, state management, data fetching patterns
4. **Map Rendering** — Deck.gl configuration, performance targets, layer specifications
5. **ORP-QL v0.1 Grammar** — EBNF formal grammar for Phase 1 query language
6. **Authentication & Authorization** — OIDC flow, JWT token format, ABAC policy evaluation
7. **Error Response Contract** — Standard error format used by all endpoints

**Out of scope (Phase 2+):**
- WASM plugin system
- AI inference pipeline
- Advanced graph traversal (>3 hops)
- Temporal queries
- Scenario forking

---

# Part 1: REST API v1 Specification

## 1.1 Base Information

**Base URL:** `http://localhost:9090/api/v1`
**Content-Type:** All requests and responses use `application/json`
**Authentication:** Bearer token in `Authorization` header (OIDC JWT)
**Default Pagination:** 100 items per page, max 1000
**Rate Limiting:** 1000 req/sec per client API key

**OpenAPI 3.1 Info:**
```json
{
  "openapi": "3.1.0",
  "info": {
    "title": "ORP Data Fusion API",
    "version": "1.0.0",
    "description": "REST API for entity CRUD, queries, graph traversal, and real-time monitoring",
    "contact": {
      "name": "ORP Team",
      "url": "https://github.com/orproject/orp"
    },
    "license": {
      "name": "Apache 2.0",
      "url": "https://www.apache.org/licenses/LICENSE-2.0"
    }
  },
  "servers": [
    {
      "url": "http://localhost:9090/api/v1",
      "description": "Local development"
    }
  ]
}
```

---

## 1.2 Authentication & Authorization

### OIDC Flow

All API requests require a Bearer token. Token obtained via:

```
1. User navigates to: http://localhost:9090/auth/login
2. UI redirects to OIDC provider (embedded Keycloak-lite or external)
3. User authenticates (OIDC flow)
4. Provider returns: { "access_token": "...", "refresh_token": "...", "expires_in": 3600 }
5. Frontend stores in secure httpOnly cookie (no XSS vector)
6. Frontend sends: Authorization: Bearer <access_token> on all API calls
```

### JWT Token Format (HS256 or RS256)

```json
{
  "sub": "user-id-12345",
  "email": "alice@company.com",
  "name": "Alice Chen",
  "iat": 1711411200,
  "exp": 1711414800,
  "iss": "http://localhost:9090/auth",
  "aud": "orp-client",
  "scope": "api:read api:write entities:read entities:write graph:read",
  "org_id": "org-456",
  "permissions": [
    "entities:read",
    "entities:write",
    "graph:read",
    "monitors:write",
    "admin"
  ]
}
```

### ABAC (Attribute-Based Access Control) Policy Evaluation

Every request is evaluated against resource attributes:

```
REQUEST:
  user.permissions = ["entities:read", "graph:read"]
  resource = /api/v1/entities/ship-mmsi-123456
  resource.tags = ["maritime", "public"]
  resource.sensitivity = "public"

POLICY ENGINE:
  IF (user.permissions.includes("entities:read"))
    AND (resource.sensitivity != "secret")
  THEN allow(read)

RESULT: 200 OK (allowed)
```

### API Key Scoping (for integrations)

```bash
# Create API key for monitoring service
curl -X POST http://localhost:9090/api/v1/api-keys \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "monitoring-service",
    "scopes": ["entities:read", "monitors:read"],
    "rate_limit": 10000,
    "expires_in": 31536000
  }'

# Response
{
  "api_key": "orpk_prod_abc123def456ghi789",
  "scopes": ["entities:read", "monitors:read"],
  "rate_limit": 10000,
  "created_at": "2026-03-26T10:00:00Z",
  "expires_at": "2027-03-26T10:00:00Z"
}

# Use on requests
curl http://localhost:9090/api/v1/entities \
  -H "X-API-Key: orpk_prod_abc123def456ghi789"
```

---

## 1.3 Standard Error Response Contract

**Every endpoint uses this error format.** No exceptions.

```typescript
interface ErrorResponse {
  error: {
    code: string;           // Machine-readable error code
    status: number;         // HTTP status code (duplicate for clarity)
    message: string;        // Human-readable error message
    details?: {
      [key: string]: any    // Additional context
    };
    trace_id?: string;      // For debugging (if telemetry enabled)
    timestamp: string;      // ISO 8601
  };
}
```

**Error codes (non-exhaustive):**

| Code | HTTP | Description | Details |
|------|------|-------------|---------|
| `INVALID_REQUEST` | 400 | Malformed JSON, missing required fields | `{ field: "name", reason: "required" }` |
| `INVALID_QUERY` | 400 | ORP-QL syntax error | `{ near: "MATCH", message: "unexpected token" }` |
| `UNAUTHORIZED` | 401 | Missing/invalid token | `{ reason: "token_expired" }` |
| `FORBIDDEN` | 403 | User lacks permissions | `{ required_scope: "entities:write" }` |
| `NOT_FOUND` | 404 | Entity/resource not found | `{ id: "ship-123", type: "Ship" }` |
| `CONFLICT` | 409 | Duplicate entity ID or state conflict | `{ field: "id", existing_entity: {...} }` |
| `RATE_LIMITED` | 429 | API rate limit exceeded | `{ retry_after_seconds: 60 }` |
| `VALIDATION_ERROR` | 422 | Semantic validation failed | `{ field: "speed", reason: "must be >= 0" }` |
| `INTERNAL_ERROR` | 500 | Server error | `{ trace_id: "abc123" }` |

**Example error response:**

```json
{
  "error": {
    "code": "INVALID_QUERY",
    "status": 400,
    "message": "ORP-QL syntax error at position 15",
    "details": {
      "near": "WHERE",
      "expected": ["property filter", "geospatial function"],
      "got": "unknown_function"
    },
    "trace_id": "req_2026-03-26_10_30_45_abc123",
    "timestamp": "2026-03-26T10:30:45Z"
  }
}
```

---

## 1.4 Pagination

All list endpoints support pagination via query params:

```typescript
interface PaginationParams {
  page?: number;           // Default: 1 (1-indexed)
  limit?: number;          // Default: 100, max: 1000
  sort_by?: string;        // Field name (e.g., "created_at", "name")
  sort_order?: "asc" | "desc";  // Default: "desc"
}

interface PaginatedResponse<T> {
  data: T[];
  pagination: {
    page: number;
    limit: number;
    total_count: number;
    total_pages: number;
    has_next: boolean;
    has_prev: boolean;
  };
  links?: {
    first: string;
    last: string;
    next?: string;
    prev?: string;
  };
}
```

**Example:**

```bash
GET /api/v1/entities?page=2&limit=50&sort_by=created_at&sort_order=desc

{
  "data": [ {...}, {...}, ... ],
  "pagination": {
    "page": 2,
    "limit": 50,
    "total_count": 5432,
    "total_pages": 109,
    "has_next": true,
    "has_prev": true
  },
  "links": {
    "first": "/api/v1/entities?page=1&limit=50",
    "last": "/api/v1/entities?page=109&limit=50",
    "next": "/api/v1/entities?page=3&limit=50",
    "prev": "/api/v1/entities?page=1&limit=50"
  }
}
```

---

## 1.5 Entity CRUD Endpoints

### 1.5.1 GET /api/v1/entities — List All Entities

**Purpose:** Fetch paginated list of all entities with optional filtering.

**Request:**

```typescript
interface ListEntitiesRequest {
  // Query parameters
  page?: number;                    // Default: 1
  limit?: number;                   // Default: 100, max: 1000
  type?: string;                    // Filter by entity type (e.g., "Ship", "Port", "WeatherSystem")
  tags?: string[];                  // Filter by tags (OR logic)
  created_after?: string;           // ISO 8601 timestamp
  created_before?: string;          // ISO 8601 timestamp
  updated_after?: string;           // ISO 8601 timestamp
  sort_by?: string;                 // Field name
  sort_order?: "asc" | "desc";      // Default: "desc"
}
```

**Response:**

```typescript
interface ListEntitiesResponse {
  data: Array<{
    id: string;                     // Unique entity ID
    type: string;                   // Entity type (Ship, Port, WeatherSystem, etc.)
    name: string;                   // Human-readable name
    tags: string[];                 // User-defined tags
    properties: Record<string, any>;// Type-specific properties
    geometry?: {
      type: "Point" | "LineString" | "Polygon";
      coordinates: number[] | number[][] | number[][][];  // GeoJSON format
    };
    confidence: number;             // Data confidence [0.0, 1.0]
    freshness: {
      updated_at: string;           // ISO 8601, last property change
      checked_at: string;           // ISO 8601, last validation
    };
    created_at: string;             // ISO 8601
    updated_at: string;             // ISO 8601
  }>;
  pagination: PaginatedResponse['pagination'];
  links: PaginatedResponse['links'];
}
```

**Example:**

```bash
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/entities?type=Ship&limit=10"

{
  "data": [
    {
      "id": "ship-imo-1234567",
      "type": "Ship",
      "name": "Maersk Seatrade",
      "tags": ["container", "international", "active"],
      "properties": {
        "imo": "1234567",
        "mmsi": "219018200",
        "call_sign": "OMCN",
        "type": "Container ship",
        "length": 366.0,
        "beam": 48.8,
        "draft": 14.5,
        "speed": 18.5,
        "course": 225.0,
        "destination": "Rotterdam",
        "eta": "2026-03-28T14:00:00Z"
      },
      "geometry": {
        "type": "Point",
        "coordinates": [3.2847, 51.9225]  // [lon, lat]
      },
      "confidence": 0.99,
      "freshness": {
        "updated_at": "2026-03-26T10:28:45Z",
        "checked_at": "2026-03-26T10:30:00Z"
      },
      "created_at": "2026-03-20T14:00:00Z",
      "updated_at": "2026-03-26T10:28:45Z"
    }
  ],
  "pagination": {
    "page": 1,
    "limit": 10,
    "total_count": 2847,
    "total_pages": 285,
    "has_next": true,
    "has_prev": false
  }
}
```

---

### 1.5.2 POST /api/v1/entities — Create Entity

**Purpose:** Ingest a new entity or update existing by ID.

**Request:**

```typescript
interface CreateEntityRequest {
  id: string;                       // Unique entity ID (required, must be URI-safe)
  type: string;                     // Entity type (required)
  name: string;                     // Human-readable name
  tags?: string[];                  // User-defined tags
  properties: {
    [key: string]: any              // Type-specific properties
  };
  geometry?: {
    type: "Point" | "LineString" | "Polygon" | "MultiPoint";
    coordinates: number[] | number[][] | number[][][];  // GeoJSON
  };
  confidence?: number;              // [0.0, 1.0], default: 1.0
  source?: string;                  // Data source identifier (e.g., "ais-feed-1", "user-manual")
  metadata?: Record<string, any>;   // Internal metadata (not exposed in queries)
}
```

**Response: 201 Created**

```typescript
interface CreateEntityResponse {
  id: string;
  type: string;
  name: string;
  tags: string[];
  properties: Record<string, any>;
  geometry?: GeoJSON;
  confidence: number;
  created_at: string;
  updated_at: string;
}
```

**Example:**

```bash
curl -X POST http://localhost:9090/api/v1/entities \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "port-rdam-01",
    "type": "Port",
    "name": "Port of Rotterdam",
    "tags": ["major", "european", "container"],
    "properties": {
      "country": "Netherlands",
      "iata_code": "NLRTM",
      "total_teu": 14000000,
      "congestion": 0.65,
      "container_terminals": 10
    },
    "geometry": {
      "type": "Point",
      "coordinates": [4.2706, 51.9289]
    },
    "confidence": 1.0,
    "source": "openport-database"
  }'

{
  "id": "port-rdam-01",
  "type": "Port",
  "name": "Port of Rotterdam",
  "tags": ["major", "european", "container"],
  "properties": {
    "country": "Netherlands",
    "iata_code": "NLRTM",
    "total_teu": 14000000,
    "congestion": 0.65,
    "container_terminals": 10
  },
  "geometry": {
    "type": "Point",
    "coordinates": [4.2706, 51.9289]
  },
  "confidence": 1.0,
  "created_at": "2026-03-26T10:35:00Z",
  "updated_at": "2026-03-26T10:35:00Z"
}
```

---

### 1.5.3 GET /api/v1/entities/{id} — Get Single Entity

**Purpose:** Fetch complete entity with all relationships.

**Response:**

```typescript
interface GetEntityResponse {
  id: string;
  type: string;
  name: string;
  tags: string[];
  properties: Record<string, any>;
  geometry?: GeoJSON;
  confidence: number;
  freshness: {
    updated_at: string;
    checked_at: string;
  };
  created_at: string;
  updated_at: string;
  relationships: {
    outgoing: Array<{
      relationship_type: string;
      target_id: string;
      target_type: string;
      target_name: string;
      properties?: Record<string, any>;
    }>;
    incoming: Array<{
      relationship_type: string;
      source_id: string;
      source_type: string;
      source_name: string;
      properties?: Record<string, any>;
    }>;
  };
  history?: Array<{
    timestamp: string;
    changed_properties: Record<string, any>;
    source: string;
  }>;
}
```

**Example:**

```bash
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:9090/api/v1/entities/ship-imo-1234567

{
  "id": "ship-imo-1234567",
  "type": "Ship",
  "name": "Maersk Seatrade",
  "tags": ["container", "international", "active"],
  "properties": {
    "imo": "1234567",
    "mmsi": "219018200",
    "type": "Container ship",
    "speed": 18.5,
    "course": 225.0,
    "destination": "Rotterdam"
  },
  "geometry": {
    "type": "Point",
    "coordinates": [3.2847, 51.9225]
  },
  "confidence": 0.99,
  "freshness": {
    "updated_at": "2026-03-26T10:28:45Z",
    "checked_at": "2026-03-26T10:30:00Z"
  },
  "created_at": "2026-03-20T14:00:00Z",
  "updated_at": "2026-03-26T10:28:45Z",
  "relationships": {
    "outgoing": [
      {
        "relationship_type": "HEADING_TO",
        "target_id": "port-rdam-01",
        "target_type": "Port",
        "target_name": "Port of Rotterdam",
        "properties": {
          "eta": "2026-03-28T14:00:00Z",
          "cargo": "containers"
        }
      },
      {
        "relationship_type": "OPERATED_BY",
        "target_id": "org-maersk",
        "target_type": "Organization",
        "target_name": "A.P. Moller - Maersk"
      }
    ],
    "incoming": [
      {
        "relationship_type": "THREATENS",
        "source_id": "weather-storm-1",
        "source_type": "WeatherSystem",
        "source_name": "Storm Cell Alpha",
        "properties": {
          "distance_km": 180,
          "severity": "high"
        }
      }
    ]
  },
  "history": [
    {
      "timestamp": "2026-03-26T10:28:45Z",
      "changed_properties": {
        "speed": 18.5,
        "course": 225.0
      },
      "source": "ais-feed-1"
    }
  ]
}
```

---

### 1.5.4 PUT /api/v1/entities/{id} — Update Entity

**Purpose:** Merge properties into existing entity (upsert).

**Request:**

```typescript
interface UpdateEntityRequest {
  properties?: Record<string, any>;  // Merge into existing properties
  geometry?: GeoJSON;                // Update position/shape
  name?: string;                     // Update name
  tags?: string[];                   // Replace tags
  confidence?: number;               // Update confidence score
}
```

**Response: 200 OK (updated entity)**

```bash
curl -X PUT http://localhost:9090/api/v1/entities/ship-imo-1234567 \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "properties": {
      "speed": 19.2,
      "course": 230.0,
      "destination": "Antwerp"
    },
    "geometry": {
      "type": "Point",
      "coordinates": [3.5, 51.95]
    }
  }'

{
  "id": "ship-imo-1234567",
  "type": "Ship",
  "properties": {
    "imo": "1234567",
    "mmsi": "219018200",
    "type": "Container ship",
    "speed": 19.2,        // Updated
    "course": 230.0,      // Updated
    "destination": "Antwerp"  // Updated
  },
  "geometry": {
    "type": "Point",
    "coordinates": [3.5, 51.95]  // Updated
  },
  "updated_at": "2026-03-26T10:35:30Z"
}
```

---

### 1.5.5 DELETE /api/v1/entities/{id} — Delete Entity

**Purpose:** Soft-delete entity (marks deleted, preserves history for audit).

**Response: 204 No Content**

```bash
curl -X DELETE http://localhost:9090/api/v1/entities/ship-imo-1234567 \
  -H "Authorization: Bearer $TOKEN"

# 204 No Content (no response body)
```

---

## 1.6 Entity Search Endpoint

### 1.6.1 GET /api/v1/entities/search — Advanced Search

**Purpose:** Multi-criterion search with geospatial predicates.

**Request:**

```typescript
interface EntitySearchRequest {
  // Query params
  type?: string;                     // Entity type filter
  near?: {
    lat: number;                     // Latitude
    lon: number;                     // Longitude
    radius_km: number;               // Search radius
  };
  within?: {
    min_lat: number;
    min_lon: number;
    max_lat: number;
    max_lon: number;
  };
  properties?: Record<string, any>; // Property filters (exact match or range)
  tags?: string[];                   // Tag filters (OR)
  text_search?: string;              // Full-text search on name + properties
  created_after?: string;            // ISO 8601
  created_before?: string;           // ISO 8601
  page?: number;
  limit?: number;
  sort_by?: string;
  sort_order?: "asc" | "desc";
}
```

**Response:**

```typescript
interface EntitySearchResponse {
  data: Array<EntityResult>;
  pagination: PaginationInfo;
  search_time_ms: number;
}
```

**Examples:**

```bash
# Ships within 50km of Rotterdam
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/entities/search?type=Ship&near=51.9225,4.2706,50"

# Ports with congestion > 70%
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/entities/search?type=Port&properties=congestion%3E0.7"

# All entities updated in last hour
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/entities/search?updated_after=$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ)"

# Full-text search
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/entities/search?text_search=Maersk&type=Ship"
```

---

## 1.7 Events Endpoint

### 1.7.1 GET /api/v1/events — Query Events

**Purpose:** Fetch entity change events (audit log, state changes).

**Request:**

```typescript
interface EventsRequest {
  entity_id?: string;                // Filter by specific entity
  entity_type?: string;              // Filter by entity type
  event_type?: string;               // e.g., "property_change", "relationship_created", "deleted"
  since?: string;                    // ISO 8601 (return events after this timestamp)
  until?: string;                    // ISO 8601
  source?: string;                   // Filter by data source
  page?: number;
  limit?: number;
}
```

**Response:**

```typescript
interface EventsResponse {
  data: Array<{
    id: string;                      // Event ID
    timestamp: string;               // ISO 8601 (when event occurred)
    entity_id: string;
    entity_type: string;
    entity_name: string;
    event_type: "property_change" | "relationship_created" | "relationship_deleted" | "entity_deleted" | "entity_created";
    changes: {
      before?: Record<string, any>;
      after?: Record<string, any>;
    };
    source: string;                  // Data source
    trace_id?: string;               // For correlation
  }>;
  pagination: PaginationInfo;
}
```

**Example:**

```bash
curl -H "Authorization: Bearer $TOKEN" \
  "http://localhost:9090/api/v1/events?entity_id=ship-imo-1234567&since=2026-03-26T00:00:00Z"

{
  "data": [
    {
      "id": "evt_1001",
      "timestamp": "2026-03-26T10:28:45Z",
      "entity_id": "ship-imo-1234567",
      "entity_type": "Ship",
      "entity_name": "Maersk Seatrade",
      "event_type": "property_change",
      "changes": {
        "before": { "speed": 18.2, "course": 220.0 },
        "after": { "speed": 18.5, "course": 225.0 }
      },
      "source": "ais-feed-1"
    },
    {
      "id": "evt_1002",
      "timestamp": "2026-03-26T10:30:00Z",
      "entity_id": "ship-imo-1234567",
      "entity_type": "Ship",
      "entity_name": "Maersk Seatrade",
      "event_type": "relationship_created",
      "changes": {
        "after": {
          "relationship_type": "NEAR",
          "target_id": "weather-storm-1",
          "distance_km": 185
        }
      },
      "source": "system-monitor-1"
    }
  ],
  "pagination": {
    "page": 1,
    "limit": 100,
    "total_count": 42,
    "total_pages": 1,
    "has_next": false,
    "has_prev": false
  }
}
```

---

## 1.8 Relationships Endpoint

### 1.8.1 GET /api/v1/entities/{id}/relationships — Get Entity Relationships

**Purpose:** Fetch relationship graph (both incoming and outgoing).

**Response:**

```typescript
interface RelationshipsResponse {
  entity_id: string;
  entity_type: string;
  outgoing: Array<{
    id: string;                      // Relationship ID
    type: string;                    // Relationship type (e.g., "HEADING_TO", "OPERATED_BY")
    target_id: string;
    target_type: string;
    target_name: string;
    properties?: Record<string, any>;
    strength?: number;               // Confidence/strength [0.0, 1.0]
    created_at: string;
    updated_at: string;
  }>;
  incoming: Array<{
    id: string;
    type: string;
    source_id: string;
    source_type: string;
    source_name: string;
    properties?: Record<string, any>;
    strength?: number;
    created_at: string;
    updated_at: string;
  }>;
  statistics: {
    total_outgoing: number;
    total_incoming: number;
  };
}
```

---

## 1.9 Query Endpoints

### 1.9.1 POST /api/v1/query — Execute ORP-QL

**Purpose:** Execute structured ORP-QL queries against the entity database.

**Request:**

```typescript
interface QueryRequest {
  query: string;                     // ORP-QL query (see Section 3 for grammar)
  timeout_ms?: number;               // Query timeout, default: 5000ms
  limit?: number;                    // Result limit, default: 100, max: 10000
  offset?: number;                   // Result offset (pagination alternative)
}
```

**Response:**

```typescript
interface QueryResponse {
  status: "success" | "error";
  results: Array<Record<string, any>>;
  metadata: {
    execution_time_ms: number;
    rows_returned: number;
    rows_scanned?: number;
  };
  query_plan?: string;               // For debugging (if verbose mode enabled)
}
```

**Examples:**

```bash
# Simple entity filter
curl -X POST http://localhost:9090/api/v1/query \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship) WHERE s.speed > 15 RETURN s.id, s.name, s.speed"
  }'

{
  "status": "success",
  "results": [
    { "s.id": "ship-imo-1234567", "s.name": "Maersk Seatrade", "s.speed": 18.5 },
    { "s.id": "ship-imo-7654321", "s.name": "COSCO Shipping Universe", "s.speed": 17.0 }
  ],
  "metadata": {
    "execution_time_ms": 145,
    "rows_returned": 2,
    "rows_scanned": 2847
  }
}

# Geospatial query
curl -X POST http://localhost:9090/api/v1/query \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50) RETURN s.id, s.name, DISTANCE(s, 51.9225, 4.2706) as dist_km"
  }'

{
  "status": "success",
  "results": [
    { "s.id": "ship-imo-1234567", "s.name": "Maersk Seatrade", "dist_km": 8.3 },
    { "s.id": "ship-imo-9876543", "s.name": "Ever Given", "dist_km": 24.1 }
  ],
  "metadata": {
    "execution_time_ms": 89,
    "rows_returned": 2
  }
}

# Graph traversal
curl -X POST http://localhost:9090/api/v1/query \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (s:Ship)-[r:HEADING_TO]->(p:Port {name: \"Rotterdam\"}) RETURN s.id, s.name, r.eta"
  }'
```

---

### 1.9.2 POST /api/v1/query/natural — Natural Language Query (Phase 2)

**Purpose:** Translate natural language to ORP-QL and execute.

**Request:**

```typescript
interface NaturalLanguageQueryRequest {
  query: string;                     // Natural language question
  confidence_threshold?: number;     // Min confidence [0.0, 1.0] for template match
  timeout_ms?: number;               // Default: 5000ms
}
```

**Response:**

```typescript
interface NaturalLanguageQueryResponse {
  status: "success" | "partial" | "ambiguous";
  natural_query: string;
  interpreted_query: string;         // Generated ORP-QL
  confidence: number;                // [0.0, 1.0]
  interpretation_method: "template" | "model" | "interactive";  // How it was translated
  results: Array<Record<string, any>>;
  alternatives?: Array<{             // If ambiguous
    query: string;
    confidence: number;
  }>;
  metadata: {
    execution_time_ms: number;
    rows_returned: number;
  };
}
```

**Example:**

```bash
curl -X POST http://localhost:9090/api/v1/query/natural \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "Which tankers are near Rotterdam?"
  }'

{
  "status": "success",
  "natural_query": "Which tankers are near Rotterdam?",
  "interpreted_query": "MATCH (s:Ship {type: \"tanker\"}) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50) RETURN s.id, s.name, s.type, DISTANCE(s, 51.9225, 4.2706) as distance_km",
  "confidence": 0.98,
  "interpretation_method": "template",
  "results": [
    {
      "s.id": "ship-imo-1111111",
      "s.name": "Torm Hafnia",
      "s.type": "tanker",
      "distance_km": 22.5
    },
    {
      "s.id": "ship-imo-2222222",
      "s.name": "Nordic Bridge",
      "s.type": "tanker",
      "distance_km": 35.8
    }
  ],
  "metadata": {
    "execution_time_ms": 156,
    "rows_returned": 2
  }
}
```

---

## 1.10 Graph Query Endpoint

### 1.10.1 POST /api/v1/graph — Execute Cypher Queries (Kuzu)

**Purpose:** Execute property graph queries (Cypher) for relationship traversal.

**Request:**

```typescript
interface GraphQueryRequest {
  query: string;                     // Cypher query
  timeout_ms?: number;               // Default: 5000ms
}
```

**Response:**

```typescript
interface GraphQueryResponse {
  status: "success" | "error";
  results: Array<Record<string, any>>;
  metadata: {
    execution_time_ms: number;
    rows_returned: number;
  };
  query_plan?: string;
}
```

**Example:**

```bash
# Find all ports reachable by ships in this company
curl -X POST http://localhost:9090/api/v1/graph \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (org:Organization {id: \"org-maersk\"})-[:OWNS]->(s:Ship)-[:HEADING_TO]->(p:Port) RETURN p.id, p.name, COUNT(*) as ships_heading_there"
  }'

{
  "status": "success",
  "results": [
    {
      "p.id": "port-rdam-01",
      "p.name": "Port of Rotterdam",
      "ships_heading_there": 12
    },
    {
      "p.id": "port-antwerp",
      "p.name": "Port of Antwerp",
      "ships_heading_there": 8
    }
  ],
  "metadata": {
    "execution_time_ms": 234,
    "rows_returned": 2
  }
}
```

---

## 1.11 Connector Management Endpoints

### 1.11.1 GET /api/v1/connectors — List Connectors

**Purpose:** List all configured data source connectors.

**Response:**

```typescript
interface ConnectorsResponse {
  data: Array<{
    id: string;                      // Connector ID
    name: string;                    // Human-readable name
    type: string;                    // Connector type (ais, ads-b, http, mqtt, csv, etc.)
    enabled: boolean;
    status: "healthy" | "degraded" | "error";
    config: {
      // Type-specific config (sanitized, no secrets)
      [key: string]: any
    };
    stats: {
      total_ingested: number;        // Total events ingested
      last_event_at: string;         // ISO 8601
      error_count: number;           // Errors in last 24h
      events_per_sec: number;        // Current throughput
    };
    created_at: string;
    updated_at: string;
  }>;
}
```

---

### 1.11.2 POST /api/v1/connectors — Create Connector

**Purpose:** Configure a new data source.

**Request:**

```typescript
interface CreateConnectorRequest {
  name: string;                      // Human-readable name
  type: "ais" | "ads-b" | "http" | "mqtt" | "csv" | "json";
  enabled?: boolean;                 // Default: true
  config: {
    // Type-specific configuration
    // See connector spec below
  };
  entity_mapping: {
    entity_type: string;             // e.g., "Ship", "Aircraft"
    id_field: string;                // Which config field maps to entity ID
    name_field?: string;
    geometry_field?: string;
    properties: Record<string, string>;  // Field mappings
  };
}
```

**Example (AIS Connector):**

```bash
curl -X POST http://localhost:9090/api/v1/connectors \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "AIS Feed - Global",
    "type": "ais",
    "enabled": true,
    "config": {
      "host": "ais-feed.company.com",
      "port": 5005,
      "protocol": "tcp",
      "filters": {
        "min_speed": 1.0,
        "max_speed": 50.0
      }
    },
    "entity_mapping": {
      "entity_type": "Ship",
      "id_field": "mmsi",
      "name_field": "ship_name",
      "geometry_field": "position",
      "properties": {
        "imo": "imo_number",
        "call_sign": "call_sign",
        "type": "ship_type",
        "speed": "speed_over_ground",
        "course": "course_over_ground"
      }
    }
  }'
```

---

### 1.11.3 PUT /api/v1/connectors/{id} — Update Connector

**Purpose:** Update connector configuration.

**Response: 200 OK**

---

### 1.11.4 DELETE /api/v1/connectors/{id} — Delete Connector

**Purpose:** Disable and remove connector.

**Response: 204 No Content**

---

## 1.12 Monitor (Alert Rules) Endpoints

### 1.12.1 GET /api/v1/monitors — List Alert Rules

**Purpose:** Fetch all configured alert monitors.

**Response:**

```typescript
interface MonitorsResponse {
  data: Array<{
    id: string;
    name: string;
    description?: string;
    enabled: boolean;
    entity_type: string;             // e.g., "Ship"
    condition: {
      type: "threshold" | "geospatial" | "relationship" | "anomaly";
      // Type-specific condition structure
    };
    action: {
      type: "webhook" | "email" | "internal_alert" | "custom";
      target?: string;
      template?: string;
    };
    last_triggered?: string;
    trigger_count: number;
    created_at: string;
    updated_at: string;
  }>;
}
```

---

### 1.12.2 POST /api/v1/monitors — Create Alert Rule

**Purpose:** Set up a new monitor/alert.

**Request:**

```typescript
interface CreateMonitorRequest {
  name: string;
  description?: string;
  enabled?: boolean;
  entity_type: string;
  condition: {
    type: "threshold" | "geospatial" | "relationship" | "anomaly";
    query?: string;                  // ORP-QL or Cypher query that should return 0 rows if healthy
  };
  action: {
    type: "webhook" | "email" | "internal_alert" | "custom";
    target?: string;
    template?: string;
  };
  cooldown_seconds?: number;         // Don't re-trigger for N seconds
}
```

**Example:**

```bash
curl -X POST http://localhost:9090/api/v1/monitors \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Ships speeding in coastal zone",
    "description": "Alert if ship speed > 12 knots within 10km of port",
    "enabled": true,
    "entity_type": "Ship",
    "condition": {
      "type": "threshold",
      "query": "MATCH (s:Ship) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=10) AND s.speed > 12 RETURN s.id"
    },
    "action": {
      "type": "webhook",
      "target": "https://monitoring.company.com/webhook/orpalerts"
    },
    "cooldown_seconds": 300
  }'
```

---

### 1.12.3 PUT /api/v1/monitors/{id} — Update Monitor

### 1.12.4 DELETE /api/v1/monitors/{id} — Delete Monitor

---

## 1.13 Health & Metrics Endpoints

### 1.13.1 GET /api/v1/health — System Health Check

**Purpose:** Health status for load balancers, uptime monitoring.

**Response:**

```typescript
interface HealthResponse {
  status: "healthy" | "degraded" | "unhealthy";
  timestamp: string;
  version: string;
  components: {
    database: {
      status: "healthy" | "degraded" | "error";
      latency_ms?: number;
    };
    graph_engine: {
      status: "healthy" | "degraded" | "error";
      latency_ms?: number;
    };
    stream_processor: {
      status: "healthy" | "degraded" | "error";
      events_per_sec?: number;
      queue_depth?: number;
    };
    api_server: {
      status: "healthy" | "degraded" | "error";
    };
  };
}
```

---

### 1.13.2 GET /api/v1/metrics — Prometheus Metrics

**Purpose:** Scrape for Prometheus.

**Response:** Prometheus text format (OpenTelemetry)

```
# HELP orp_entities_total Total number of entities
# TYPE orp_entities_total gauge
orp_entities_total{type="Ship"} 2847
orp_entities_total{type="Port"} 512
orp_entities_total{type="WeatherSystem"} 64

# HELP orp_query_latency_ms Query execution latency
# TYPE orp_query_latency_ms histogram
orp_query_latency_ms_bucket{le="50"} 450
orp_query_latency_ms_bucket{le="100"} 780
orp_query_latency_ms_bucket{le="500"} 1200
orp_query_latency_ms_bucket{le="5000"} 1250

# HELP orp_stream_throughput_events_per_sec Stream processor throughput
# TYPE orp_stream_throughput_events_per_sec gauge
orp_stream_throughput_events_per_sec 12500
```

---

# Part 2: WebSocket Protocol

## 2.1 WebSocket Connection

**Endpoint:** `ws://localhost:9090/ws/updates` or `wss://` for TLS

**Authentication:** Bearer token in query param or header

```javascript
// Connect with Bearer token
const ws = new WebSocket(
  'ws://localhost:9090/ws/updates?token=' + encodeURIComponent(accessToken)
);
```

---

## 2.2 Message Format

All WebSocket messages are JSON:

```typescript
interface WebSocketMessage {
  type: string;                      // Message type
  id?: string;                       // Message ID (for req/resp pairing)
  timestamp: string;                 // ISO 8601
  data?: Record<string, any>;        // Type-specific payload
  error?: {
    code: string;
    message: string;
  };
}
```

---

## 2.3 Subscription Messages

### 2.3.1 Subscribe to Entity Type

```javascript
// Client → Server
{
  "type": "subscribe",
  "id": "sub-1",
  "data": {
    "entity_type": "Ship",           // All ships
    "properties_only": ["speed", "course", "position"]  // Optional: limit fields
  }
}

// Server → Client (confirmation)
{
  "type": "subscription_created",
  "id": "sub-1",
  "timestamp": "2026-03-26T10:40:00Z",
  "data": {
    "subscription_id": "sub-1",
    "entity_type": "Ship",
    "active_entities": 2847
  }
}
```

### 2.3.2 Subscribe to Geographic Region

```javascript
// Client → Server
{
  "type": "subscribe",
  "id": "sub-2",
  "data": {
    "entity_type": "Ship",
    "region": {
      "min_lat": 51.5,
      "min_lon": 3.0,
      "max_lat": 52.5,
      "max_lon": 5.0
    }
  }
}
```

### 2.3.3 Subscribe to Specific Entity

```javascript
// Client → Server
{
  "type": "subscribe",
  "id": "sub-3",
  "data": {
    "entity_id": "ship-imo-1234567"
  }
}
```

### 2.3.4 Unsubscribe

```javascript
// Client → Server
{
  "type": "unsubscribe",
  "id": "unsub-1",
  "data": {
    "subscription_id": "sub-1"
  }
}
```

---

## 2.4 Update Messages (Server → Client)

### 2.4.1 Entity Updated

```javascript
{
  "type": "entity_update",
  "timestamp": "2026-03-26T10:42:15Z",
  "data": {
    "entity_id": "ship-imo-1234567",
    "entity_type": "Ship",
    "changes": {
      "speed": { "before": 18.5, "after": 19.2 },
      "course": { "before": 225.0, "after": 230.0 },
      "position": {
        "before": [3.2847, 51.9225],
        "after": [3.5000, 51.9500]
      }
    },
    "source": "ais-feed-1"
  }
}
```

### 2.4.2 Entity Created

```javascript
{
  "type": "entity_created",
  "timestamp": "2026-03-26T10:43:00Z",
  "data": {
    "entity_id": "ship-imo-9999999",
    "entity_type": "Ship",
    "entity_name": "New Vessel",
    "properties": {
      "mmsi": "123456789",
      "speed": 15.0,
      "course": 180.0
    },
    "geometry": {
      "type": "Point",
      "coordinates": [4.0, 51.8]
    },
    "source": "ais-feed-1"
  }
}
```

### 2.4.3 Entity Deleted

```javascript
{
  "type": "entity_deleted",
  "timestamp": "2026-03-26T10:45:00Z",
  "data": {
    "entity_id": "ship-imo-5555555",
    "entity_type": "Ship",
    "entity_name": "Deleted Ship",
    "reason": "no_recent_updates"
  }
}
```

### 2.4.4 Relationship Changed

```javascript
{
  "type": "relationship_changed",
  "timestamp": "2026-03-26T10:46:30Z",
  "data": {
    "relationship_id": "rel-12345",
    "source_id": "ship-imo-1234567",
    "source_type": "Ship",
    "target_id": "weather-storm-1",
    "target_type": "WeatherSystem",
    "relationship_type": "THREATENS",
    "event": "created",       // or "deleted", "updated"
    "properties": {
      "distance_km": 150,
      "severity": "high"
    }
  }
}
```

### 2.4.5 Alert Triggered

```javascript
{
  "type": "alert_triggered",
  "timestamp": "2026-03-26T10:47:00Z",
  "data": {
    "monitor_id": "monitor-speed-check",
    "monitor_name": "Ships speeding in coastal zone",
    "severity": "warning",    // or "critical", "info"
    "affected_entities": [
      {
        "entity_id": "ship-imo-1234567",
        "entity_type": "Ship",
        "reason": "speed 19.2 knots in 10km zone"
      }
    ],
    "timestamp": "2026-03-26T10:47:00Z"
  }
}
```

---

## 2.5 Heartbeat & Connection Management

### 2.5.1 Heartbeat (keep-alive)

```javascript
// Server → Client (every 30 seconds)
{
  "type": "heartbeat",
  "timestamp": "2026-03-26T10:50:00Z"
}

// Client → Server (acknowledge, optional but recommended)
{
  "type": "heartbeat_ack",
  "timestamp": "2026-03-26T10:50:00Z"
}
```

### 2.5.2 Reconnection Strategy

If connection drops:

1. Client waits 1 second, attempts reconnect
2. On each failed attempt, exponential backoff: 2s, 4s, 8s, 16s (max 60s)
3. On successful reconnect, resubscribe to all subscriptions
4. Server maintains subscription state for 5 minutes after disconnect

```javascript
// Client auto-resubscribes
{
  "type": "subscribe",
  "id": "sub-1",
  "data": {
    "entity_type": "Ship",
    "resume_from": "2026-03-26T10:50:30Z"  // Resume from last disconnect
  }
}
```

---

# Part 3: Frontend Architecture

## 3.1 Component Tree

```
App (layout, auth context)
├── Header (logo, user menu, settings)
├── Sidebar
│   ├── DataSourcePanel
│   │   └── ConnectorStatus
│   ├── QueryPanel
│   │   ├── StructuredQueryBuilder
│   │   └── NaturalLanguageInput
│   └── AlertFeed
├── MainContent
│   ├── MapView
│   │   ├── DeckGL layers (ships, ports, weather)
│   │   └── CesiumJS 3D toggle
│   ├── EntityInspector (right panel)
│   │   ├── PropertyTable
│   │   ├── RelationshipGraph
│   │   └── HistoryTimeline
│   └── TimelineScrubber (bottom)
└── NotificationCenter
```

---

## 3.2 State Management

**Framework: Zustand (lightweight, no boilerplate)**

Rationale over Redux Toolkit:
- Redux Toolkit is enterprise-grade but heavier
- Zustand is simpler, faster for team shipping in 5 minutes
- Single store vs Redux's slice pattern = fewer files
- Easier onboarding for new contributors
- Better DevTools integration for this use case

**Store structure:**

```typescript
// store/useAppStore.ts
import { create } from 'zustand';

interface AppState {
  // UI State
  selectedEntityId: string | null;
  selectedEntities: Set<string>;
  mapCenter: [number, number];
  mapZoom: number;

  // Map/View State
  mapMode: '2d' | '3d';
  showWeatherLayer: boolean;
  showShipTracksLayer: boolean;

  // Query State
  lastQuery: string;
  queryResults: Array<Record<string, any>>;
  queryLoading: boolean;
  queryError: string | null;

  // Subscription State
  wsConnected: boolean;
  subscriptions: Map<string, Subscription>;

  // Data
  entities: Map<string, Entity>;
  relationships: Map<string, Relationship>;

  // Actions
  selectEntity: (id: string) => void;
  updateEntity: (id: string, changes: Partial<Entity>) => void;
  setMapCenter: (center: [number, number]) => void;
  setQueryResults: (results: Array<Record<string, any>>) => void;
  addSubscription: (sub: Subscription) => void;
  removeSubscription: (id: string) => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  selectedEntityId: null,
  selectedEntities: new Set(),
  mapCenter: [4.27, 51.92],  // Rotterdam
  mapZoom: 8,
  mapMode: '2d',
  showWeatherLayer: true,
  showShipTracksLayer: true,
  lastQuery: '',
  queryResults: [],
  queryLoading: false,
  queryError: null,
  wsConnected: false,
  subscriptions: new Map(),
  entities: new Map(),
  relationships: new Map(),

  selectEntity: (id: string) => set({ selectedEntityId: id }),

  updateEntity: (id: string, changes: Partial<Entity>) => {
    set((state) => {
      const entity = state.entities.get(id);
      if (!entity) return state;
      state.entities.set(id, { ...entity, ...changes });
      return { entities: new Map(state.entities) };
    });
  },

  setMapCenter: (center: [number, number]) => set({ mapCenter: center }),

  setQueryResults: (results: Array<Record<string, any>>) =>
    set({ queryResults: results, queryLoading: false }),

  addSubscription: (sub: Subscription) => {
    set((state) => {
      state.subscriptions.set(sub.id, sub);
      return { subscriptions: new Map(state.subscriptions) };
    });
  },

  removeSubscription: (id: string) => {
    set((state) => {
      state.subscriptions.delete(id);
      return { subscriptions: new Map(state.subscriptions) };
    });
  },
}));
```

---

## 3.3 Data Fetching (React Query)

```typescript
// hooks/useEntities.ts
import { useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';

export function useEntities(filters?: EntityFilters) {
  return useQuery({
    queryKey: ['entities', filters],
    queryFn: () => api.getEntities(filters),
    staleTime: 5000,     // 5 seconds
    gcTime: 1000 * 60 * 5, // 5 minutes (formerly cacheTime)
    refetchOnWindowFocus: false,
    retry: 2,
  });
}

// hooks/useEntitySearch.ts
export function useEntitySearch(query: string, type?: string) {
  return useQuery({
    queryKey: ['entities', 'search', query, type],
    queryFn: () => api.searchEntities({ query, type }),
    staleTime: 10000,
    enabled: query.length > 0,  // Only fetch if query is not empty
  });
}

// hooks/useWebSocketUpdates.ts
export function useWebSocketUpdates(entityType?: string) {
  const [updates, setUpdates] = React.useState<EntityUpdate[]>([]);

  React.useEffect(() => {
    const ws = new WebSocket('ws://localhost:9090/ws/updates?token=' + getToken());

    ws.onopen = () => {
      ws.send(JSON.stringify({
        type: 'subscribe',
        data: { entity_type: entityType }
      }));
    };

    ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      if (msg.type === 'entity_update') {
        setUpdates((prev) => [msg.data, ...prev.slice(0, 99)]);  // Keep last 100
      }
    };

    return () => ws.close();
  }, [entityType]);

  return updates;
}
```

---

## 3.4 Component Specs

### 3.4.1 MapView Component

```typescript
// components/MapView.tsx
import DeckGL from '@deck.gl/react';
import { IconLayer, ScatterplotLayer, PathLayer, PolygonLayer, HeatmapLayer } from '@deck.gl/layers';
import { StaticMap } from 'react-map-gl';
import Cesium from 'cesium';

interface MapViewProps {
  entities: Map<string, Entity>;
  selectedEntityId?: string;
  mapMode: '2d' | '3d';
  center: [number, number];
  zoom: number;
  onSelectEntity: (id: string) => void;
  showWeatherLayer: boolean;
  showShipTracksLayer: boolean;
}

export const MapView: React.FC<MapViewProps> = ({
  entities,
  selectedEntityId,
  mapMode,
  center,
  zoom,
  onSelectEntity,
  showWeatherLayer,
  showShipTracksLayer,
}) => {
  const [viewState, setViewState] = React.useState({
    longitude: center[0],
    latitude: center[1],
    zoom: zoom,
    pitch: 0,
    bearing: 0,
  });

  // Extract ships for icon layer
  const ships = Array.from(entities.values()).filter((e) => e.type === 'Ship');

  // Extract ports for scatterplot layer
  const ports = Array.from(entities.values()).filter((e) => e.type === 'Port');

  // Extract weather systems for polygon layer
  const weatherSystems = Array.from(entities.values()).filter((e) => e.type === 'WeatherSystem');

  const layers = [
    // Ship layer
    new IconLayer({
      id: 'ship-layer',
      data: ships,
      pickable: true,
      iconAtlas: '/icons/vessel-atlas.png',
      iconMapping: {
        ship: { x: 0, y: 0, width: 32, height: 32 },
        tanker: { x: 32, y: 0, width: 32, height: 32 },
        container: { x: 64, y: 0, width: 32, height: 32 },
      },
      getIcon: (d) => d.properties.type?.toLowerCase() || 'ship',
      getPosition: (d) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
      getSize: 20,
      getColor: (d) => (d.id === selectedEntityId ? [255, 0, 0, 255] : [0, 100, 200, 255]),
      getAngle: (d) => d.properties.course || 0,
      onHover: (info) => {
        // Cursor feedback
      },
      onClick: (info) => {
        if (info.object) {
          onSelectEntity(info.object.id);
        }
      },
    }),

    // Port layer
    new ScatterplotLayer({
      id: 'port-layer',
      data: ports,
      pickable: true,
      radiusScale: 100,
      radiusMinPixels: 4,
      radiusMaxPixels: 100,
      getPosition: (d) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
      getRadius: (d) => d.properties.size || 5,
      getColor: (d) => [255, 140, 0, 160],
      onClick: (info) => {
        if (info.object) {
          onSelectEntity(info.object.id);
        }
      },
    }),

    // Weather layer (if enabled)
    ...(showWeatherLayer
      ? [
          new PolygonLayer({
            id: 'weather-layer',
            data: weatherSystems,
            pickable: true,
            stroked: true,
            filled: true,
            extruded: false,
            getPolygon: (d) => {
              // Extract polygon from geometry
              if (d.geometry.type === 'Polygon') {
                return d.geometry.coordinates[0];
              }
              return [];
            },
            getColor: (d) => [200, 50, 50, 100],
            getLineColor: [200, 0, 0, 255],
            onClick: (info) => {
              if (info.object) {
                onSelectEntity(info.object.id);
              }
            },
          }),
        ]
      : []),

    // Ship tracks layer (if enabled)
    ...(showShipTracksLayer
      ? [
          new PathLayer({
            id: 'ship-tracks-layer',
            data: ships.filter((s) => s.history && s.history.length > 1),
            pickable: false,
            getPath: (d) =>
              (d.history || [])
                .map((h) => [h.geometry.coordinates[0], h.geometry.coordinates[1]])
                .slice(-50),  // Last 50 positions
            getColor: [200, 200, 200, 100],
            getWidth: 2,
            widthMinPixels: 1,
          }),
        ]
      : []),

    // Density heatmap (for large datasets)
    new HeatmapLayer({
      id: 'heatmap-layer',
      data: ships,
      getPosition: (d) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
      getWeight: 1,
      colorRange: [
        [26, 26, 127, 255],
        [255, 0, 0, 255],
      ],
      radiusPixels: 30,
      intensity: 1,
      threshold: 0.03,
      opacity: 0.5,
    }),
  ];

  if (mapMode === '3d') {
    return <CesiumGlobe entities={entities} />;
  }

  return (
    <DeckGL
      viewState={viewState}
      onViewStateChange={(event) => setViewState(event.viewState)}
      controller={true}
      layers={layers}
    >
      <StaticMap mapboxAccessToken={process.env.REACT_APP_MAPBOX_TOKEN} />
    </DeckGL>
  );
};
```

**Performance targets:**
- 60 FPS with 50K entities visible
- Layer culling: only render entities in viewport + 20% buffer
- Use memoization for expensive calculations
- Deck.gl binary data format for large datasets

### 3.4.2 QueryBar Component

```typescript
// components/QueryBar.tsx
interface QueryBarProps {
  mode: 'structured' | 'natural';
  onQuery: (query: string) => Promise<void>;
  loading: boolean;
  error?: string;
}

export const QueryBar: React.FC<QueryBarProps> = ({ mode, onQuery, loading, error }) => {
  const [query, setQuery] = React.useState('');
  const [suggestions, setSuggestions] = React.useState<string[]>([]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await onQuery(query);
  };

  React.useEffect(() => {
    if (query.length < 2) {
      setSuggestions([]);
      return;
    }

    // Debounced suggestion fetch
    const timer = setTimeout(async () => {
      if (mode === 'natural') {
        // Autocomplete suggestions from templates
        const matches = QUERY_TEMPLATES.filter((t) =>
          t.description.toLowerCase().includes(query.toLowerCase())
        );
        setSuggestions(matches.map((m) => m.description));
      } else {
        // ORP-QL keyword suggestions
        setSuggestions(['MATCH', 'WHERE', 'RETURN', 'LIMIT'].filter((k) =>
          k.toLowerCase().startsWith(query.toLowerCase())
        ));
      }
    }, 200);

    return () => clearTimeout(timer);
  }, [query, mode]);

  return (
    <div className="query-bar">
      <form onSubmit={handleSubmit}>
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={mode === 'natural' ? 'Ask in English...' : 'ORP-QL query...'}
          disabled={loading}
          autoComplete="off"
        />
        <button type="submit" disabled={loading || !query}>
          {loading ? 'Querying...' : 'Execute'}
        </button>
      </form>
      {suggestions.length > 0 && (
        <ul className="suggestions">
          {suggestions.map((s) => (
            <li key={s} onClick={() => setQuery(s)}>
              {s}
            </li>
          ))}
        </ul>
      )}
      {error && <div className="error">{error}</div>}
    </div>
  );
};
```

### 3.4.3 EntityInspector Component

```typescript
// components/EntityInspector.tsx
interface EntityInspectorProps {
  entity?: Entity;
  relationships?: RelationshipsResponse;
  loading: boolean;
  onClose: () => void;
}

export const EntityInspector: React.FC<EntityInspectorProps> = ({
  entity,
  relationships,
  loading,
  onClose,
}) => {
  if (!entity) {
    return <div className="entity-inspector empty">Select an entity to inspect</div>;
  }

  return (
    <div className="entity-inspector">
      <header>
        <h2>{entity.name}</h2>
        <button onClick={onClose}>×</button>
      </header>

      <section className="properties">
        <h3>Properties</h3>
        <table>
          <tbody>
            {Object.entries(entity.properties).map(([key, value]) => (
              <tr key={key}>
                <td className="key">{key}</td>
                <td className="value">{String(value)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="relationships">
        <h3>Relationships ({(relationships?.outgoing.length || 0) + (relationships?.incoming.length || 0)})</h3>
        <div className="rel-tabs">
          <div className="rel-outgoing">
            <h4>Connected To ({relationships?.outgoing.length || 0})</h4>
            <ul>
              {relationships?.outgoing.map((rel) => (
                <li key={rel.id}>
                  <strong>{rel.relationship_type}</strong> → {rel.target_name}
                </li>
              ))}
            </ul>
          </div>
          <div className="rel-incoming">
            <h4>Connected From ({relationships?.incoming.length || 0})</h4>
            <ul>
              {relationships?.incoming.map((rel) => (
                <li key={rel.id}>
                  {rel.source_name} ← <strong>{rel.relationship_type}</strong>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </section>

      <section className="freshness">
        <h3>Data Quality</h3>
        <p>Confidence: {(entity.confidence * 100).toFixed(0)}%</p>
        <p>Updated: {new Date(entity.freshness.updated_at).toLocaleString()}</p>
      </section>
    </div>
  );
};
```

### 3.4.4 TimelineScrubber Component

```typescript
// components/TimelineScrubber.tsx
interface TimelineScrubberProps {
  minTime: Date;
  maxTime: Date;
  currentTime: Date;
  onTimeChange: (time: Date) => void;
  loading: boolean;
}

export const TimelineScrubber: React.FC<TimelineScrubberProps> = ({
  minTime,
  maxTime,
  currentTime,
  onTimeChange,
  loading,
}) => {
  const progress = ((currentTime.getTime() - minTime.getTime()) /
    (maxTime.getTime() - minTime.getTime())) * 100;

  return (
    <div className="timeline-scrubber">
      <div className="time-display">
        <span>{minTime.toISOString().split('T')[0]}</span>
        <span className="current">{currentTime.toISOString()}</span>
        <span>{maxTime.toISOString().split('T')[0]}</span>
      </div>
      <input
        type="range"
        min="0"
        max="100"
        value={progress}
        onChange={(e) => {
          const newTime = new Date(
            minTime.getTime() + (parseFloat(e.target.value) / 100) * (maxTime.getTime() - minTime.getTime())
          );
          onTimeChange(newTime);
        }}
        disabled={loading}
        className="scrubber-slider"
      />
      <div className="controls">
        <button onClick={() => onTimeChange(minTime)}>← Start</button>
        <button onClick={() => onTimeChange(new Date())}>Now →</button>
      </div>
    </div>
  );
};
```

---

## 3.5 TypeScript Interfaces

```typescript
// types/entities.ts

interface Entity {
  id: string;
  type: string;
  name: string;
  tags: string[];
  properties: Record<string, any>;
  geometry?: {
    type: 'Point' | 'LineString' | 'Polygon';
    coordinates: number[] | number[][] | number[][][];
  };
  confidence: number;
  freshness: {
    updated_at: string;
    checked_at: string;
  };
  created_at: string;
  updated_at: string;
  history?: Array<{
    timestamp: string;
    changed_properties: Record<string, any>;
    source: string;
  }>;
}

interface Relationship {
  id: string;
  type: string;
  source_id: string;
  target_id: string;
  properties?: Record<string, any>;
  strength?: number;
  created_at: string;
  updated_at: string;
}

interface EntityUpdate {
  entity_id: string;
  entity_type: string;
  changes: Record<string, { before: any; after: any }>;
  source: string;
  timestamp: string;
}

interface Subscription {
  id: string;
  type: 'entity_type' | 'region' | 'entity' | 'query';
  config: Record<string, any>;
  created_at: Date;
}

interface QueryResult {
  [key: string]: any;
}

interface AlertEvent {
  monitor_id: string;
  monitor_name: string;
  severity: 'info' | 'warning' | 'critical';
  affected_entities: Array<{
    entity_id: string;
    entity_type: string;
    reason: string;
  }>;
  timestamp: string;
}
```

---

# Part 4: Map Rendering Specification

## 4.1 Deck.gl Layer Configuration

### Performance Targets

- **60 FPS** with 50K entities visible on screen
- Sub-100ms layer update latency
- <200MB GPU memory for typical maritime scenario (5K ships + 500 ports)
- Automatic layer culling outside viewport

### Layer Specifications

#### 4.1.1 IconLayer (Ships)

```typescript
new IconLayer({
  id: 'ship-icons',
  data: shipEntities,
  pickable: true,
  iconAtlas: 'https://cdn.example.com/vessel-icons-2x.png',
  iconMapping: {
    // Define sprite coordinates in atlas
    'container': { x: 0, y: 0, width: 32, height: 32 },
    'tanker': { x: 32, y: 0, width: 32, height: 32 },
    'bulkcarrier': { x: 64, y: 0, width: 32, height: 32 },
    'general': { x: 96, y: 0, width: 32, height: 32 },
  },
  getIcon: (d: Entity) => (d.properties.type || 'general').toLowerCase(),
  getPosition: (d: Entity) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
  getSize: (d: Entity) => (d.id === selectedId ? 24 : 20),
  getColor: (d: Entity) => {
    // Red if selected, else based on speed
    if (d.id === selectedId) return [255, 0, 0, 255];
    const speed = d.properties.speed || 0;
    if (speed > 20) return [255, 100, 0, 255];  // Fast: orange
    if (speed > 10) return [0, 150, 255, 255];  // Moderate: blue
    return [100, 200, 100, 255];                // Slow: green
  },
  getAngle: (d: Entity) => d.properties.course || 0,
  getPixelOffset: [0, -10],  // Offset for label above icon
  updateTriggers: {
    getColor: [selectedId],
    getSize: [selectedId],
  },
  onHover: (info) => {
    // Show tooltip
    setHoveredId(info.object?.id);
  },
  onClick: (info) => {
    if (info.object) {
      selectEntity(info.object.id);
    }
  },
  transitions: {
    getPosition: {
      type: 'linear',
      duration: 500,  // Smooth ship movement
    },
    getAngle: {
      type: 'linear',
      duration: 500,
    },
  },
  // Optimize: only render entities within viewport + buffer
  extensions: [new DataFilterExtension()],
  filterSize: 1,
  getFilterValue: (d) => isInViewport(d) ? 1 : 0,
});
```

#### 4.1.2 ScatterplotLayer (Ports)

```typescript
new ScatterplotLayer({
  id: 'ports',
  data: portEntities,
  pickable: true,
  radiusScale: 100,
  radiusMinPixels: 4,
  radiusMaxPixels: 60,
  getPosition: (d: Entity) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
  getRadius: (d: Entity) => {
    // Size by throughput
    const teu = d.properties.total_teu || 0;
    return Math.log(teu + 1) / 10;
  },
  getColor: (d: Entity) => {
    const congestion = d.properties.congestion || 0;
    if (congestion > 0.8) return [255, 0, 0, 200];     // Red: high
    if (congestion > 0.5) return [255, 200, 0, 200];   // Yellow: medium
    return [0, 200, 0, 200];                           // Green: low
  },
  getLineColor: [0, 0, 0, 255],
  lineWidthMinPixels: 1,
  onClick: (info) => {
    if (info.object) selectEntity(info.object.id);
  },
});
```

#### 4.1.3 PathLayer (Ship Tracks)

```typescript
new PathLayer({
  id: 'ship-tracks',
  data: shipEntities.filter((s) => s.history && s.history.length > 1),
  pickable: false,
  getPath: (d: Entity) =>
    (d.history || [])
      .sort((a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime())
      .slice(-100)  // Last 100 positions (reasonable memory limit)
      .map((h) => [h.geometry.coordinates[0], h.geometry.coordinates[1]]),
  getColor: (d: Entity) => {
    if (d.id === selectedId) return [255, 0, 0, 150];
    return [150, 150, 150, 100];
  },
  getWidth: (d: Entity) => (d.id === selectedId ? 3 : 1),
  widthMinPixels: 1,
  widthMaxPixels: 5,
  fadeHead: true,
  fadeTrail: true,
  capRounded: true,
  jointRounded: true,
  updateTriggers: {
    getColor: [selectedId],
    getWidth: [selectedId],
  },
});
```

#### 4.1.4 PolygonLayer (Weather Systems)

```typescript
new PolygonLayer({
  id: 'weather-systems',
  data: weatherEntities,
  pickable: true,
  stroked: true,
  filled: true,
  extruded: false,
  wireframe: false,
  getPolygon: (d: Entity) => {
    if (d.geometry.type === 'Polygon') {
      return d.geometry.coordinates[0];  // First ring only
    }
    return [];
  },
  getFillColor: (d: Entity) => {
    const severity = d.properties.severity || 'low';
    switch (severity) {
      case 'critical':
        return [200, 0, 0, 100];      // Dark red
      case 'high':
        return [255, 100, 0, 100];    // Orange
      case 'moderate':
        return [255, 200, 0, 80];     // Yellow
      default:
        return [100, 200, 255, 60];   // Light blue
    }
  },
  getLineColor: [0, 0, 0, 255],
  getLineWidth: 2,
  lineWidthMinPixels: 1,
  onClick: (info) => {
    if (info.object) selectEntity(info.object.id);
  },
});
```

#### 4.1.5 HeatmapLayer (Density)

```typescript
new HeatmapLayer({
  id: 'ship-density',
  data: shipEntities,
  getPosition: (d: Entity) => [d.geometry.coordinates[0], d.geometry.coordinates[1]],
  getWeight: 1,
  colorRange: [
    [26, 26, 127, 255],      // Dark blue (low density)
    [55, 48, 163, 255],
    [63, 0, 250, 255],
    [255, 0, 0, 255],        // Red (high density)
  ],
  radiusPixels: 50,
  intensity: 0.8,
  threshold: 0.05,
  opacity: 0.3,
  updateTriggers: {
    getWeight: [timeWindow],  // Update on time change
  },
});
```

---

## 4.2 Performance Optimization Strategies

```typescript
// 1. Viewport culling
const visibleEntities = entities.filter((e) => {
  const [lon, lat] = e.geometry.coordinates;
  return (
    lon >= viewState.longitude - 5 &&
    lon <= viewState.longitude + 5 &&
    lat >= viewState.latitude - 5 &&
    lat <= viewState.latitude + 5
  );
});

// 2. Level-of-detail (LOD) rendering
const getLOD = (zoom: number) => {
  if (zoom < 5) return 'low';     // Show aggregated clusters
  if (zoom < 8) return 'medium';  // Show simplified geometries
  return 'high';                  // Show full detail
};

// 3. Data batching (binary format)
const shipBuffer = Float32Array.from(
  ships.flatMap((s) => [
    s.geometry.coordinates[0],
    s.geometry.coordinates[1],
    s.properties.course || 0,
    s.properties.speed || 0,
  ])
);

// 4. Memoization
const memoizedLayers = React.useMemo(
  () => [iconLayer, scatterplotLayer, pathLayer],
  [selectedId, entities, viewState]
);
```

---

# Part 5: ORP-QL v0.1 Grammar

## 5.1 EBNF Formal Grammar

```ebnf
(* ORP-QL v0.1 — Phase 1 Query Language *)

Query = Match Where? Return OrderBy? Limit?
       ;

Match = "MATCH" Pattern
      ;

Pattern = Entity ( "-" Relationship "-" Entity )*
        ;

Entity = "(" Identifier ":" Type PropertyFilter? ")"
       ;

Relationship = "[" ( ":" RelationshipType )? "]"
             ;

PropertyFilter = "{" Property ( "," Property )* "}"
               ;

Property = Identifier ":" Literal
         ;

Type = Identifier
     ;

Identifier = Letter ( Letter | Digit | "_" )*
           ;

Where = "WHERE" Condition ( "AND" Condition | "OR" Condition )*
      ;

Condition = PropertyCondition
          | GeospatialPredicate
          | ComparisonOp
          ;

PropertyCondition = Identifier ComparisonOperator Literal
                  ;

ComparisonOperator = "=" | "!=" | ">" | "<" | ">=" | "<=" | "LIKE"
                   ;

GeospatialPredicate = "NEAR" "(" Identifier "," LatLonRadius ")"
                    | "WITHIN" "(" Identifier "," BoundingBox ")"
                    | "DISTANCE" "(" Identifier "," LatLon ")" ComparisonOperator Number
                    ;

LatLonRadius = "lat=" Number "," "lon=" Number "," "radius_km=" Number
             ;

BoundingBox = "min_lat=" Number "," "min_lon=" Number
            "," "max_lat=" Number "," "max_lon=" Number
            ;

LatLon = "lat=" Number "," "lon=" Number
       ;

Return = "RETURN" ReturnExpression ( "," ReturnExpression )*
       ;

ReturnExpression = Identifier ( "as" Alias )?
                 | FunctionCall ( "as" Alias )?
                 ;

FunctionCall = "COUNT" "(" Identifier ")"
             | "SUM" "(" Identifier ")"
             | "AVG" "(" Identifier ")"
             | "MIN" "(" Identifier ")"
             | "MAX" "(" Identifier ")"
             | "DISTANCE" "(" Identifier "," LatLon ")"
             ;

Alias = Identifier
      ;

OrderBy = "ORDER" "BY" Identifier ( "ASC" | "DESC" )?
        ;

Limit = "LIMIT" Number
      ;

Literal = String | Number | Boolean
        ;

String = '"' [^"]* '"'
       ;

Number = Digit+ ( "." Digit+ )?
       ;

Boolean = "true" | "false"
        ;

Digit = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
      ;

Letter = "A" | ... | "Z" | "a" | ... | "z"
       ;
```

---

## 5.2 Query Examples

### Simple Entity Filter

```
MATCH (s:Ship)
WHERE s.speed > 15
RETURN s.id, s.name, s.speed
LIMIT 100
```

### Geospatial Query

```
MATCH (s:Ship)
WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50)
RETURN s.id, s.name, DISTANCE(s, lat=51.9225, lon=4.2706) as distance_km
ORDER BY distance_km ASC
```

### Graph Traversal

```
MATCH (s:Ship)-[:HEADING_TO]->(p:Port {name: "Rotterdam"})
WHERE s.speed > 10
RETURN s.id, s.name, p.name as destination
```

### Aggregation

```
MATCH (s:Ship)
WHERE WITHIN(s, min_lat=50, min_lon=0, max_lat=55, max_lon=10)
RETURN COUNT(s) as ship_count, AVG(s.speed) as avg_speed
```

### Complex Filter

```
MATCH (s:Ship)-[:THREATENS]->(w:WeatherSystem)
WHERE s.type = "tanker"
  AND w.severity = "high"
  AND DISTANCE(s, lat=51.5, lon=-2.0) < 200
RETURN s.id, s.name, w.name as threat, DISTANCE(s, lat=51.5, lon=-2.0) as distance_km
ORDER BY distance_km ASC
LIMIT 50
```

---

# Part 6: Authentication & Authorization

## 6.1 OIDC Authorization Code Flow

```
┌─────────────┐                                    ┌──────────────┐
│   Browser   │                                    │  OIDC Provider │
└──────┬──────┘                                    └────────┬───────┘
       │                                                   │
       │  1. User clicks "Login"                         │
       │─────────────────────────────────────────────────>│
       │                                                   │
       │  2. Redirect to auth/login                      │
       │<─────────────────────────────────────────────────│
       │                                                   │
       │  3. User authenticates (credentials, MFA, SSO)  │
       │─────────────────────────────────────────────────>│
       │                                                   │
       │  4. Authorization code                          │
       │<─────────────────────────────────────────────────│
       │                                                   │
       │  5. POST /auth/callback?code=xxx (backend)      │
       │─────────────────────────────────────────────────>│
       │                                                   │
       │  6. Exchange code for tokens (backend)          │
       │─────────────────────────────────────────────────>│
       │                                                   │
       │  7. Tokens (access_token, refresh_token)        │
       │<─────────────────────────────────────────────────│
       │                                                   │
       │  8. Set httpOnly cookie (access_token)          │
       │<─────────────────────────────────────────────────│
       │                                                   │
       │  9. Redirect to /dashboard                      │
       │<─────────────────────────────────────────────────│
       │                                                   │
```

### Configuration

```yaml
# config.yaml
auth:
  provider: "keycloak"  # "keycloak" (embedded) or "oidc" (external)
  keycloak:
    realm_url: "http://localhost:8080/realms/orp"
    client_id: "orp-frontend"
    client_secret: "${AUTH_CLIENT_SECRET}"
  oidc:
    provider_url: "https://accounts.google.com"
    client_id: "xxx"
    client_secret: "${AUTH_CLIENT_SECRET}"
    redirect_uri: "http://localhost:9090/auth/callback"
  jwt:
    signing_algorithm: "RS256"
    public_key: "${JWT_PUBLIC_KEY_FILE}"  # For verification
    expiration_seconds: 3600
    refresh_expiration_seconds: 86400
```

---

## 6.2 ABAC Policy Evaluation

Every request is evaluated against attribute-based access control policies.

### Policy Example

```json
{
  "policies": [
    {
      "id": "policy-1",
      "name": "Users can read own entities",
      "effect": "allow",
      "principal": { "type": "user" },
      "action": ["entities:read"],
      "resource": {
        "type": "entity",
        "attribute_match": {
          "owner_id": "${subject.sub}"
        }
      }
    },
    {
      "id": "policy-2",
      "name": "Admins can read all entities",
      "effect": "allow",
      "principal": {
        "type": "user",
        "attribute_match": { "role": "admin" }
      },
      "action": ["entities:read", "entities:write", "entities:delete"],
      "resource": { "type": "entity" }
    },
    {
      "id": "policy-3",
      "name": "Can only read non-sensitive data",
      "effect": "allow",
      "principal": { "type": "user" },
      "action": ["entities:read"],
      "resource": {
        "type": "entity",
        "attribute_match": { "sensitivity": ["public", "internal"] }
      }
    }
  ]
}
```

### Evaluation Algorithm

```typescript
interface PolicyEvaluationContext {
  subject: {
    sub: string;              // User ID
    permissions: string[];    // ["entities:read", "entities:write"]
    role?: string;
    org_id?: string;
  };
  action: string;            // "entities:read"
  resource: {
    type: string;            // "entity"
    id: string;
    attributes: Record<string, any>;  // { sensitivity: "public", owner_id: "user-123" }
  };
}

function evaluatePolicies(context: PolicyEvaluationContext): boolean {
  for (const policy of policies) {
    if (policyMatches(policy, context)) {
      return policy.effect === 'allow';
    }
  }
  return false;  // Default deny
}

function policyMatches(policy: Policy, context: PolicyEvaluationContext): boolean {
  // Check principal
  if (!principalMatches(policy.principal, context.subject)) {
    return false;
  }

  // Check action
  if (!policy.action.includes(context.action)) {
    return false;
  }

  // Check resource
  if (policy.resource.type !== context.resource.type) {
    return false;
  }

  // Check attribute matching
  if (policy.resource.attribute_match) {
    for (const [key, expectedValue] of Object.entries(policy.resource.attribute_match)) {
      const actualValue = context.resource.attributes[key];
      if (!attributeMatches(actualValue, expectedValue)) {
        return false;
      }
    }
  }

  return true;
}
```

---

# Part 7: Implementation Checklist for Engineers

## Phase 1 Delivery (Months 2-10)

- [ ] **Core REST API** — All 13 endpoint groups implemented, tested, documented
- [ ] **WebSocket real-time** — Connection, subscriptions, message delivery
- [ ] **React frontend** — Component tree, Zustand store, React Query integration
- [ ] **Map rendering** — Deck.gl with 4 layer types, <60ms latency for 50K entities
- [ ] **ORP-QL v0.1** — EBNF parser, execution engine, <500ms query latency
- [ ] **OIDC auth** — Login flow, JWT validation, ABAC policy enforcement
- [ ] **Error handling** — Standard error format on all endpoints
- [ ] **Performance** — Binary size <350MB, memory <3GB for 1M entities
- [ ] **Documentation** — OpenAPI spec, TypeScript interfaces, component props
- [ ] **Integration tests** — 50+ integration tests covering happy path + error cases

## Testing Matrix

| Component | Unit | Integration | Performance |
|-----------|------|-------------|-------------|
| REST Endpoints | 100+ | 80+ | 20+ |
| WebSocket | 30+ | 20+ | 10+ |
| ORP-QL Parser | 50+ | 30+ | 5+ |
| Deck.gl Layers | 40+ | 15+ | 15+ |
| ABAC Engine | 25+ | 15+ | — |

---

**End of Specification**

**This document is the source of truth for all API contracts, frontend architecture, and data formats. Any deviation requires RFC approval from the Technical Steering Committee.**
