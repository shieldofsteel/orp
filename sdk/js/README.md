# @orp/client

Official JavaScript/TypeScript SDK for **ORP** (Open Relationship Protocol).

- ✅ Zero dependencies
- ✅ TypeScript-first with full type coverage
- ✅ Works in Node.js ≥18 and all modern browsers
- ✅ Native `fetch` + native `WebSocket`
- ✅ Real-time subscriptions via WebSocket

---

## Installation

```bash
npm install @orp/client
# or
pnpm add @orp/client
```

---

## Quick Start

### Node.js (CommonJS)

```js
const { ORPClient } = require('@orp/client');

const client = new ORPClient('http://localhost:4000', {
  apiKey: 'my-api-key',
});

(async () => {
  const health = await client.health();
  console.log('ORP status:', health.status);

  const { data, total } = await client.entities({ type: 'Person', limit: 10 });
  console.log(`Found ${total} people`, data);
})();
```

### Node.js / Browser (ESM / TypeScript)

```ts
import { ORPClient } from '@orp/client';

const client = new ORPClient('https://orp.example.com', {
  token: 'eyJhbGci...', // JWT bearer token
});
```

---

## Authentication

Pass **either** a bearer token or an API key in the options:

```ts
// Bearer token
const client = new ORPClient(host, { token: 'eyJhbGci...' });

// API Key (sent as X-API-Key header)
const client = new ORPClient(host, { apiKey: 'sk-...' });
```

---

## API Reference

### `new ORPClient(host, options?)`

| Option | Type | Default | Description |
|---|---|---|---|
| `host` | `string` | required | Base URL of the ORP node |
| `options.token` | `string` | — | Bearer JWT token |
| `options.apiKey` | `string` | — | API key (X-API-Key header) |
| `options.timeout` | `number` | `30000` | Request timeout in ms |
| `options.retries` | `number` | `0` | Retry count on network/5xx errors |
| `options.retryDelay` | `number` | `1000` | Delay between retries in ms |
| `options.headers` | `object` | `{}` | Extra headers added to every request |

---

### `client.entities(params?)`

List entities with optional filtering.

```ts
const result = await client.entities({
  type: 'Organization',
  limit: 25,
  offset: 0,
  near: { lat: 3.1569, lon: 101.7123, radius_km: 10 },
});
// result: PaginatedEntities
// { data: Entity[], total, limit, offset, hasMore, nextOffset? }
```

**Params:**

| Field | Type | Description |
|---|---|---|
| `type` | `string` | Filter by entity type |
| `near.lat` | `number` | Latitude for geo-proximity search |
| `near.lon` | `number` | Longitude |
| `near.radius_km` | `number` | Search radius in km |
| `limit` | `number` | Max results (default: server default) |
| `offset` | `number` | Pagination offset |
| `sortBy` | `string` | Property to sort by |
| `sortOrder` | `'asc' \| 'desc'` | Sort direction |

---

### `client.entity(id)`

Fetch a single entity by ID including its relationships.

```ts
const entity = await client.entity('ent_abc123');
console.log(entity.type, entity.properties);
console.log(entity.relationships); // Relationship[]
```

---

### `client.query(orpql)`

Execute an ORPQL query.

```ts
const result = await client.query(`
  MATCH (p:Person)-[:WORKS_AT]->(o:Organization)
  WHERE o.name = "Shield of Steel"
  RETURN p, o
`);

console.log(result.entities);       // Entity[]
console.log(result.relationships);  // Relationship[]
console.log(result.executionTimeMs);
```

---

### `client.ingest(data)`

Ingest a single entity record.

```ts
const { entity, created } = await client.ingest({
  type: 'Person',
  properties: {
    name: 'John Doe',
    email: 'john@example.com',
    role: 'Security Guard',
  },
  location: { lat: 3.1569, lon: 101.7123 },
});

console.log(`Entity ${created ? 'created' : 'updated'}:`, entity.id);
```

---

### `client.ingestBatch(data[])`

Ingest multiple records in one request.

```ts
const result = await client.ingestBatch([
  { type: 'Person', properties: { name: 'Alice' } },
  { type: 'Person', properties: { name: 'Bob' } },
  {
    type: 'Organization',
    properties: { name: 'Acme Corp' },
    relationships: [
      { type: 'EMPLOYS', toEntityId: 'ent_alice_id' }
    ],
  },
]);

console.log(`Created: ${result.created}, Updated: ${result.updated}, Failed: ${result.failed}`);
if (result.errors?.length) {
  console.error('Ingest errors:', result.errors);
}
```

---

### `client.subscribe(entityType, callback)`

Subscribe to real-time entity events via WebSocket. Returns an unsubscribe function.

```ts
const unsub = client.subscribe('Incident', (event) => {
  switch (event.type) {
    case 'entity.created':
      console.log('New incident:', event.entity);
      break;
    case 'entity.updated':
      console.log('Incident updated:', event.entity);
      break;
    case 'entity.deleted':
      console.log('Incident deleted:', event.entityId);
      break;
  }
});

// Stop listening
unsub();
```

**Event shape (`SubscriptionEvent`):**

```ts
interface SubscriptionEvent {
  type: 'entity.created' | 'entity.updated' | 'entity.deleted'
      | 'relationship.created' | 'relationship.deleted' | 'error';
  entityType: string;
  entityId?: string;
  entity?: Entity;
  relationship?: Relationship;
  timestamp: string;
  peerId?: string;
}
```

Multiple subscriptions are multiplexed over a single WebSocket. The connection auto-reconnects on drop.

---

### `client.health()`

```ts
const health = await client.health();
// {
//   status: 'healthy',
//   version: '1.2.0',
//   uptime: 86400,
//   timestamp: '2026-03-27T00:00:00Z',
//   services: { database: { status: 'up', latencyMs: 2 } },
//   entityCount: 42000
// }
```

---

### `client.connectors()`

```ts
const { connectors, total } = await client.connectors();
connectors.forEach(c => {
  console.log(`${c.name} [${c.type}] — ${c.status}`);
});
```

---

### `client.peers()`

```ts
const { peers, localPeerId } = await client.peers();
peers.forEach(p => {
  console.log(`${p.name} @ ${p.host} — ${p.status} (${p.latencyMs}ms)`);
});
```

---

### `client.destroy()`

Cleanly close the WebSocket and cancel all subscriptions.

```ts
client.destroy();
```

---

## Error Handling

All methods throw `ORPError` on failure.

```ts
import { ORPClient, ORPError } from '@orp/client';

try {
  const entity = await client.entity('nonexistent');
} catch (err) {
  if (err instanceof ORPError) {
    console.error(`ORP error ${err.statusCode}: ${err.message}`);
    console.error('Details:', err.details);
  } else {
    throw err;
  }
}
```

**`ORPError` properties:**

| Property | Type | Description |
|---|---|---|
| `message` | `string` | Human-readable error message |
| `statusCode` | `number` | HTTP status code (0 = network error, 408 = timeout) |
| `details` | `unknown` | Additional error context from the server |

---

## Browser Usage

The SDK uses native browser APIs only (`fetch`, `WebSocket`, `URL`). No polyfills needed for modern browsers.

```html
<script type="module">
  import { ORPClient } from 'https://cdn.example.com/@orp/client/dist/index.esm.js';

  const client = new ORPClient('https://orp.example.com', {
    token: document.cookie.match(/orp_token=([^;]+)/)?.[1],
  });

  const health = await client.health();
  document.getElementById('status').textContent = health.status;
</script>
```

---

## Retry Configuration

```ts
const client = new ORPClient('https://orp.example.com', {
  apiKey: 'sk-...',
  retries: 3,       // retry up to 3 times on network or 5xx errors
  retryDelay: 500,  // wait 500ms between retries
  timeout: 10_000,  // 10s timeout per attempt
});
```

---

## Types

All exported types:

```ts
import type {
  Entity,
  Relationship,
  GeoPoint,
  QueryResult,
  PaginatedEntities,
  PaginatedResult,
  EntitiesParams,
  IngestResult,
  BatchIngestResult,
  HealthResponse,
  ServiceHealth,
  Connector,
  ConnectorsResult,
  Peer,
  PeersResult,
  SubscriptionEvent,
  SubscriptionEventType,
  SubscriptionCallback,
  UnsubscribeFunction,
  ORPClientOptions,
  ORPErrorResponse,
} from '@orp/client';
```

---

## Building from Source

```bash
git clone https://github.com/shieldofsteel/orp.git
cd orp/sdk/js
npm install
npm run build
# Output: dist/index.js, dist/index.d.ts
```

---

## License

MIT
