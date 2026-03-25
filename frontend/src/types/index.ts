// ORP Frontend Type Definitions

export interface GeoJSON {
  type: 'Point' | 'LineString' | 'Polygon';
  coordinates: number[] | number[][] | number[][][];
}

export interface Entity {
  id: string;
  type: string;
  name: string | null;
  tags: string[];
  properties: Record<string, unknown>;
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
  is_active: boolean;
  history?: Array<{
    timestamp: string;
    changed_properties: Record<string, unknown>;
    source: string;
    geometry?: {
      coordinates: number[];
    };
  }>;
}

export interface Relationship {
  id: string;
  type: string;
  source_id: string;
  target_id: string;
  source_type?: string;
  source_name?: string;
  target_type?: string;
  target_name?: string;
  properties?: Record<string, unknown>;
  strength?: number;
  confidence?: number;
  created_at: string;
  updated_at: string;
}

export interface EntityUpdate {
  entity_id: string;
  entity_type: string;
  changes: Record<string, { before: unknown; after: unknown }>;
  source: string;
  timestamp: string;
  geometry?: {
    type: string;
    coordinates: number[];
  };
}

export interface Subscription {
  id: string;
  type: 'entity_type' | 'region' | 'entity' | 'query';
  config: Record<string, unknown>;
  created_at: Date;
}

export interface AlertEvent {
  id: string;
  monitor_id: string;
  monitor_name: string;
  severity: 'info' | 'warning' | 'critical';
  affected_entities: Array<{
    entity_id: string;
    entity_type: string;
    reason: string;
  }>;
  timestamp: string;
  acknowledged: boolean;
}

export interface QueryResult {
  [key: string]: unknown;
}

export interface PaginationInfo {
  page: number;
  limit: number;
  total_count: number;
  total_pages: number;
  has_next: boolean;
  has_prev: boolean;
}

export interface PaginatedResponse<T> {
  data: T[];
  pagination: PaginationInfo;
}

export interface EntityFilters {
  page?: number;
  limit?: number;
  type?: string;
  tags?: string[];
  created_after?: string;
  created_before?: string;
  sort_by?: string;
  sort_order?: 'asc' | 'desc';
}

export interface SearchParams {
  type?: string;
  near?: string;
  text_search?: string;
  limit?: number;
}

export interface Connector {
  id: string;
  name: string;
  type: string;
  enabled: boolean;
  status: 'healthy' | 'degraded' | 'error';
  stats: {
    total_ingested: number;
    last_event_at: string;
    error_count: number;
    events_per_sec: number;
  };
}

export interface HealthResponse {
  status: 'healthy' | 'degraded' | 'unhealthy';
  timestamp: string;
  version: string;
  uptime_seconds: number;
  components: {
    database: { status: string; latency_ms?: number };
    stream_processor: { status: string; latency_ms?: number };
    api_server: { status: string };
    monitor_engine: { status: string };
  };
}

export interface WebSocketMessage {
  type: string;
  id?: string;
  timestamp: string;
  data?: Record<string, unknown>;
  error?: {
    code: string;
    message: string;
  };
}

export interface RelationshipsResponse {
  entity_id: string;
  outgoing: Array<{
    id: string;
    type: string;
    target_id: string;
    target_type?: string;
    target_name?: string;
    properties?: Record<string, unknown>;
    confidence?: number;
  }>;
  incoming: Array<{
    id: string;
    type: string;
    source_id: string;
    source_type?: string;
    source_name?: string;
    properties?: Record<string, unknown>;
    confidence?: number;
  }>;
  total: number;
}
