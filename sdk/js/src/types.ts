// ─── Core Entity Types ────────────────────────────────────────────────────────

export interface Entity {
  id: string;
  type: string;
  properties: Record<string, unknown>;
  relationships?: Relationship[];
  location?: GeoPoint;
  createdAt: string;
  updatedAt: string;
  source?: string;
  confidence?: number;
  metadata?: Record<string, unknown>;
}

export interface Relationship {
  id: string;
  type: string;
  fromEntityId: string;
  toEntityId: string;
  fromEntity?: Entity;
  toEntity?: Entity;
  properties?: Record<string, unknown>;
  weight?: number;
  createdAt: string;
  updatedAt: string;
}

export interface GeoPoint {
  lat: number;
  lon: number;
  altitude?: number;
}

// ─── Query Types ─────────────────────────────────────────────────────────────

export interface QueryResult {
  entities: Entity[];
  relationships: Relationship[];
  bindings?: Record<string, unknown>[];
  total: number;
  executionTimeMs: number;
  query: string;
}

// ─── Pagination ───────────────────────────────────────────────────────────────

export interface PaginatedResult<T> {
  data: T[];
  total: number;
  limit: number;
  offset: number;
  hasMore: boolean;
  nextOffset?: number;
}

export type PaginatedEntities = PaginatedResult<Entity>;

// ─── Request Parameters ───────────────────────────────────────────────────────

export interface EntitiesParams {
  type?: string;
  near?: {
    lat: number;
    lon: number;
    radius_km: number;
  };
  limit?: number;
  offset?: number;
  properties?: Record<string, unknown>;
  sortBy?: string;
  sortOrder?: 'asc' | 'desc';
}

// ─── Ingest Types ─────────────────────────────────────────────────────────────

export interface IngestResult {
  entity: Entity;
  created: boolean;
  relationshipsCreated?: number;
}

export interface BatchIngestResult {
  entities: Entity[];
  created: number;
  updated: number;
  failed: number;
  errors?: Array<{
    index: number;
    message: string;
    data: Record<string, unknown>;
  }>;
  totalRelationshipsCreated?: number;
}

// ─── Health & System ─────────────────────────────────────────────────────────

export interface HealthResponse {
  status: 'healthy' | 'degraded' | 'unhealthy';
  version: string;
  uptime: number;
  timestamp: string;
  services: {
    database: ServiceHealth;
    federation?: ServiceHealth;
    connectors?: ServiceHealth;
    [key: string]: ServiceHealth | undefined;
  };
  entityCount?: number;
  relationshipCount?: number;
}

export interface ServiceHealth {
  status: 'up' | 'down' | 'degraded';
  latencyMs?: number;
  message?: string;
}

// ─── Connectors ───────────────────────────────────────────────────────────────

export interface Connector {
  id: string;
  name: string;
  type: string;
  status: 'active' | 'inactive' | 'error';
  description?: string;
  version?: string;
  config?: Record<string, unknown>;
  lastSync?: string;
  entitiesIngested?: number;
  errorMessage?: string;
}

export interface ConnectorsResult {
  connectors: Connector[];
  total: number;
}

// ─── Federation / Peers ───────────────────────────────────────────────────────

export interface Peer {
  id: string;
  name: string;
  host: string;
  status: 'connected' | 'disconnected' | 'pending' | 'error';
  version?: string;
  lastSeen?: string;
  latencyMs?: number;
  entityCount?: number;
  capabilities?: string[];
  trustLevel?: 'full' | 'partial' | 'readonly';
}

export interface PeersResult {
  peers: Peer[];
  total: number;
  localPeerId: string;
}

// ─── WebSocket Events ─────────────────────────────────────────────────────────

export type SubscriptionEventType =
  | 'entity.created'
  | 'entity.updated'
  | 'entity.deleted'
  | 'relationship.created'
  | 'relationship.deleted'
  | 'error';

export interface SubscriptionEvent {
  type: SubscriptionEventType;
  entityType: string;
  entityId?: string;
  entity?: Entity;
  relationship?: Relationship;
  timestamp: string;
  peerId?: string;
}

export type SubscriptionCallback = (event: SubscriptionEvent) => void;

export type UnsubscribeFunction = () => void;

// ─── Client Options ───────────────────────────────────────────────────────────

export interface ORPClientOptions {
  token?: string;
  apiKey?: string;
  timeout?: number;
  retries?: number;
  retryDelay?: number;
  headers?: Record<string, string>;
}

// ─── Error Types ─────────────────────────────────────────────────────────────

export interface ORPErrorResponse {
  error: string;
  message: string;
  statusCode: number;
  details?: unknown;
}
