// ORP Frontend Type Definitions — Production

// ── Entity Type Registry ──────────────────────────────────────────────────────

/**
 * Configuration for a single entity type discovered in the data.
 * Drives map rendering, legend, layer toggles — zero hardcoding.
 */
export interface EntityTypeConfig {
  /** Canonical type string (lowercase, matches entity.type.toLowerCase()) */
  type: string;
  /** Display label e.g. "Ship", "Aircraft" */
  label: string;
  /** SVG path string OR single emoji for the icon */
  icon: string;
  /** Whether icon is emoji (true) or SVG path (false) */
  iconIsEmoji: boolean;
  /** RGB color triple [r, g, b] — used for markers and legend */
  color: [number, number, number];
  /** CSS hex string derived from color */
  colorHex: string;
  /** Property key that holds speed (knots / km-h / m-s) if applicable */
  speedField?: string;
  /** Property key that holds heading/course in degrees if applicable */
  headingField?: string;
  /** Property key that holds altitude in meters if applicable */
  altitudeField?: string;
  /** Map marker style */
  markerStyle: 'arrow' | 'plane' | 'circle' | 'dot' | 'diamond' | 'square' | 'cross';
  /** Whether to draw course/speed vector lines */
  showVector: boolean;
  /** Whether to draw historical track polyline */
  showTrack: boolean;
  /** Whether to render as polygon (area entity like weather zones) */
  isArea: boolean;
}

/** Auto-discovered registry: type string → config */
export type EntityTypeRegistry = Map<string, EntityTypeConfig>;

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
      type?: string;
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

export interface EntityCreated {
  entity_id: string;
  entity_type: string;
  entity_name?: string;
  properties?: Record<string, unknown>;
  geometry?: {
    type: string;
    coordinates: number[];
  };
  source: string;
}

export interface EntityDeleted {
  entity_id: string;
  entity_type: string;
  entity_name?: string;
  reason?: string;
}

export interface RelationshipChanged {
  relationship_id: string;
  source_id: string;
  source_type: string;
  target_id: string;
  target_type: string;
  relationship_type: string;
  event: 'created' | 'deleted' | 'updated';
  properties?: Record<string, unknown>;
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

export interface ConnectorStats {
  events_per_sec: number;
  last_event_at: string;
  error_count: number;
  total_ingested: number;
}

export interface Connector {
  id: string;
  name: string;
  type: string;
  enabled: boolean;
  status: 'healthy' | 'degraded' | 'error';
  stats: ConnectorStats;
}

export interface Monitor {
  id: string;
  name: string;
  description?: string;
  enabled: boolean;
  severity: 'info' | 'warning' | 'critical';
  condition: string;
  entity_type?: string;
  created_at: string;
  updated_at: string;
  last_triggered?: string;
  trigger_count: number;
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

// WebSocket message union type
export type WebSocketMessage =
  | {
      type: 'entity_update';
      timestamp: string;
      data: EntityUpdate;
    }
  | {
      type: 'entity_created';
      timestamp: string;
      data: EntityCreated;
    }
  | {
      type: 'entity_deleted';
      timestamp: string;
      data: EntityDeleted;
    }
  | {
      type: 'relationship_changed';
      timestamp: string;
      data: RelationshipChanged;
    }
  | {
      type: 'alert_triggered';
      timestamp: string;
      data: AlertEvent & { timestamp: string };
    }
  | {
      type: 'heartbeat';
      timestamp: string;
    }
  | {
      type: 'subscription_created';
      timestamp: string;
      id: string;
      data: { subscription_id: string; entity_type?: string; active_entities?: number };
    }
  | {
      type: 'error';
      timestamp: string;
      error: { code: string; message: string };
    };

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

// ABAC / Auth types
export interface JWTPayload {
  sub: string;
  email: string;
  name: string;
  iat: number;
  exp: number;
  iss: string;
  aud: string;
  scope: string;
  org_id: string;
  permissions: string[];
}

export interface AuthState {
  token: string | null;
  user: JWTPayload | null;
  authenticated: boolean;
}

export interface ABACPolicy {
  resource: string;
  required_permissions: string[];
  sensitivity: 'public' | 'internal' | 'secret';
}

// Timeline types
export interface TimelineState {
  playing: boolean;
  currentTime: Date;
  speed: number; // 1x, 2x, 5x, 10x
  minTime: Date;
  maxTime: Date;
}

// Query types
export type QueryMode = 'structured' | 'natural';

export interface QueryHistoryEntry {
  id: string;
  query: string;
  mode: QueryMode;
  timestamp: Date;
  resultCount: number;
}

export interface SidebarSection {
  id: 'datasources' | 'query' | 'alerts';
  collapsed: boolean;
}
