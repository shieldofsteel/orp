/**
 * @orp/client — Official JavaScript/TypeScript SDK for ORP
 * Zero dependencies. Works in Node.js (≥18) and modern browsers.
 */

import {
  Entity,
  EntitiesParams,
  PaginatedEntities,
  QueryResult,
  IngestResult,
  BatchIngestResult,
  HealthResponse,
  ConnectorsResult,
  PeersResult,
  SubscriptionCallback,
  SubscriptionEvent,
  UnsubscribeFunction,
  ORPClientOptions,
  ORPErrorResponse,
} from './types.js';

export * from './types.js';

// ─── Error Class ─────────────────────────────────────────────────────────────

export class ORPError extends Error {
  public readonly statusCode: number;
  public readonly details: unknown;

  constructor(message: string, statusCode: number, details?: unknown) {
    super(message);
    this.name = 'ORPError';
    this.statusCode = statusCode;
    this.details = details;
    // Fix prototype chain for instanceof checks in transpiled code
    Object.setPrototypeOf(this, ORPError.prototype);
  }
}

// ─── WebSocket subscription state ────────────────────────────────────────────

interface Subscription {
  entityType: string;
  callback: SubscriptionCallback;
}

// ─── Client ──────────────────────────────────────────────────────────────────

export class ORPClient {
  private readonly baseUrl: string;
  private readonly wsUrl: string;
  private readonly options: Required<ORPClientOptions>;

  private ws: WebSocket | null = null;
  private wsReady: boolean = false;
  private wsReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private subscriptions: Map<string, Subscription> = new Map();
  private subIdCounter: number = 0;

  constructor(host: string, options: ORPClientOptions = {}) {
    // Normalise host — strip trailing slash
    const cleanHost = host.replace(/\/+$/, '');

    // Derive HTTP and WS base URLs
    if (cleanHost.startsWith('http://') || cleanHost.startsWith('https://')) {
      this.baseUrl = cleanHost;
      this.wsUrl = cleanHost.replace(/^http/, 'ws');
    } else {
      this.baseUrl = `https://${cleanHost}`;
      this.wsUrl = `wss://${cleanHost}`;
    }

    this.options = {
      token: options.token ?? '',
      apiKey: options.apiKey ?? '',
      timeout: options.timeout ?? 30_000,
      retries: options.retries ?? 0,
      retryDelay: options.retryDelay ?? 1_000,
      headers: options.headers ?? {},
    };
  }

  // ─── Private HTTP helpers ──────────────────────────────────────────────────

  private buildHeaders(extra: Record<string, string> = {}): Record<string, string> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      ...this.options.headers,
      ...extra,
    };

    if (this.options.token) {
      headers['Authorization'] = `Bearer ${this.options.token}`;
    } else if (this.options.apiKey) {
      headers['X-API-Key'] = this.options.apiKey;
    }

    return headers;
  }

  private buildUrl(path: string, params?: Record<string, string | number | boolean | undefined>): string {
    const url = new URL(`${this.baseUrl}${path}`);
    if (params) {
      for (const [key, value] of Object.entries(params)) {
        if (value !== undefined && value !== null && value !== '') {
          url.searchParams.set(key, String(value));
        }
      }
    }
    return url.toString();
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    queryParams?: Record<string, string | number | boolean | undefined>,
    attempt = 0
  ): Promise<T> {
    const url = this.buildUrl(path, queryParams);
    const headers = this.buildHeaders();

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.options.timeout);

    let response: Response;
    try {
      response = await fetch(url, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
    } catch (err) {
      clearTimeout(timeoutId);
      const isAbort = err instanceof Error && err.name === 'AbortError';
      if (!isAbort && attempt < this.options.retries) {
        await this.delay(this.options.retryDelay);
        return this.request<T>(method, path, body, queryParams, attempt + 1);
      }
      throw new ORPError(
        isAbort ? `Request timed out after ${this.options.timeout}ms` : `Network error: ${(err as Error).message}`,
        isAbort ? 408 : 0
      );
    } finally {
      clearTimeout(timeoutId);
    }

    if (!response.ok) {
      let errBody: Partial<ORPErrorResponse> = {};
      try {
        errBody = (await response.json()) as Partial<ORPErrorResponse>;
      } catch {
        // Non-JSON error body — ignore
      }

      // Retry on server errors if configured
      if (response.status >= 500 && attempt < this.options.retries) {
        await this.delay(this.options.retryDelay);
        return this.request<T>(method, path, body, queryParams, attempt + 1);
      }

      throw new ORPError(
        errBody.message ?? errBody.error ?? `HTTP ${response.status}`,
        response.status,
        errBody.details
      );
    }

    // 204 No Content
    if (response.status === 204) {
      return undefined as unknown as T;
    }

    return response.json() as Promise<T>;
  }

  private delay(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  // ─── Public API methods ────────────────────────────────────────────────────

  /**
   * List entities with optional filtering by type, location, or limit.
   */
  async entities(params: EntitiesParams = {}): Promise<PaginatedEntities> {
    const query: Record<string, string | number | boolean | undefined> = {};

    if (params.type) query['type'] = params.type;
    if (params.limit !== undefined) query['limit'] = params.limit;
    if (params.offset !== undefined) query['offset'] = params.offset;
    if (params.sortBy) query['sortBy'] = params.sortBy;
    if (params.sortOrder) query['sortOrder'] = params.sortOrder;

    if (params.near) {
      query['lat'] = params.near.lat;
      query['lon'] = params.near.lon;
      query['radius_km'] = params.near.radius_km;
    }

    if (params.properties) {
      // Encode extra property filters as JSON string
      query['properties'] = JSON.stringify(params.properties);
    }

    return this.request<PaginatedEntities>('GET', '/api/entities', undefined, query);
  }

  /**
   * Fetch a single entity by ID, including its relationships.
   */
  async entity(id: string): Promise<Entity> {
    if (!id) throw new ORPError('Entity ID is required', 400);
    return this.request<Entity>('GET', `/api/entities/${encodeURIComponent(id)}`);
  }

  /**
   * Execute an ORPQL query and return matched entities and relationships.
   */
  async query(orpql: string): Promise<QueryResult> {
    if (!orpql || !orpql.trim()) throw new ORPError('Query string is required', 400);
    return this.request<QueryResult>('POST', '/api/query', { query: orpql });
  }

  /**
   * Ingest a single entity record. Returns the created/updated entity.
   */
  async ingest(data: Record<string, unknown>): Promise<IngestResult> {
    return this.request<IngestResult>('POST', '/api/ingest', data);
  }

  /**
   * Ingest multiple entity records in a single request.
   */
  async ingestBatch(data: Record<string, unknown>[]): Promise<BatchIngestResult> {
    if (!Array.isArray(data) || data.length === 0) {
      throw new ORPError('ingestBatch requires a non-empty array', 400);
    }
    return this.request<BatchIngestResult>('POST', '/api/ingest/batch', { records: data });
  }

  /**
   * Get the health status of the ORP node.
   */
  async health(): Promise<HealthResponse> {
    return this.request<HealthResponse>('GET', '/api/health');
  }

  /**
   * List all registered data connectors.
   */
  async connectors(): Promise<ConnectorsResult> {
    return this.request<ConnectorsResult>('GET', '/api/connectors');
  }

  /**
   * List all federation peers.
   */
  async peers(): Promise<PeersResult> {
    return this.request<PeersResult>('GET', '/api/federation/peers');
  }

  // ─── WebSocket subscription ────────────────────────────────────────────────

  /**
   * Subscribe to real-time events for a given entity type.
   * Returns an unsubscribe function.
   *
   * @example
   * const unsub = client.subscribe('Person', (event) => {
   *   console.log('Event:', event.type, event.entity);
   * });
   * // Later:
   * unsub();
   */
  subscribe(entityType: string, callback: SubscriptionCallback): UnsubscribeFunction {
    const id = String(++this.subIdCounter);
    this.subscriptions.set(id, { entityType, callback });

    // Ensure WS is open and send subscribe message once ready
    this.ensureWebSocket(() => {
      this.wsSend({ action: 'subscribe', entityType, subscriptionId: id });
    });

    return () => {
      this.subscriptions.delete(id);
      if (this.wsReady && this.ws) {
        this.wsSend({ action: 'unsubscribe', entityType, subscriptionId: id });
      }
      // If no subscriptions remain, close the socket
      if (this.subscriptions.size === 0) {
        this.closeWebSocket();
      }
    };
  }

  // ─── WebSocket internals ───────────────────────────────────────────────────

  private ensureWebSocket(onReady: () => void): void {
    if (this.ws && this.wsReady) {
      onReady();
      return;
    }

    if (this.ws && !this.wsReady) {
      // Already connecting — queue the action
      const originalOnOpen = this.ws.onopen;
      this.ws.onopen = (ev) => {
        if (typeof originalOnOpen === 'function') originalOnOpen.call(this.ws, ev);
        onReady();
      };
      return;
    }

    const wsUrl = this.buildWsUrl();
    let socket: WebSocket;

    try {
      socket = new WebSocket(wsUrl);
    } catch (err) {
      console.error('[ORP] WebSocket construction failed:', err);
      return;
    }

    this.ws = socket;
    this.wsReady = false;

    socket.onopen = () => {
      this.wsReady = true;
      // Re-subscribe all active subscriptions after reconnect
      this.subscriptions.forEach((sub, id) => {
        this.wsSend({ action: 'subscribe', entityType: sub.entityType, subscriptionId: id });
      });
      onReady();
    };

    socket.onmessage = (ev) => {
      let event: SubscriptionEvent;
      try {
        event = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as SubscriptionEvent;
      } catch {
        return;
      }
      this.dispatchEvent(event);
    };

    socket.onerror = (ev) => {
      console.error('[ORP] WebSocket error', ev);
    };

    socket.onclose = () => {
      this.wsReady = false;
      this.ws = null;
      // Auto-reconnect if there are active subscriptions
      if (this.subscriptions.size > 0) {
        this.wsReconnectTimer = setTimeout(() => {
          this.ensureWebSocket(() => {/* subscriptions re-sent in onopen */});
        }, 3_000);
      }
    };
  }

  private buildWsUrl(): string {
    const wsBase = this.wsUrl;
    const params = new URLSearchParams();
    if (this.options.token) params.set('token', this.options.token);
    else if (this.options.apiKey) params.set('apiKey', this.options.apiKey);

    const qs = params.toString();
    return `${wsBase}/api/subscribe${qs ? '?' + qs : ''}`;
  }

  private wsSend(payload: unknown): void {
    if (this.ws && this.wsReady) {
      this.ws.send(JSON.stringify(payload));
    }
  }

  private dispatchEvent(event: SubscriptionEvent): void {
    this.subscriptions.forEach((sub) => {
      if (!event.entityType || sub.entityType === event.entityType || sub.entityType === '*') {
        try {
          sub.callback(event);
        } catch (err) {
          console.error('[ORP] Subscription callback threw:', err);
        }
      }
    });
  }

  private closeWebSocket(): void {
    if (this.wsReconnectTimer !== null) {
      clearTimeout(this.wsReconnectTimer);
      this.wsReconnectTimer = null;
    }
    if (this.ws) {
      this.ws.onclose = null; // prevent auto-reconnect
      this.ws.close();
      this.ws = null;
      this.wsReady = false;
    }
  }

  /**
   * Cleanly close the client — disconnects WebSocket and cancels any pending reconnects.
   */
  destroy(): void {
    this.subscriptions.clear();
    this.closeWebSocket();
  }
}

// Default export for convenience
export default ORPClient;
