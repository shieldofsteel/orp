import { useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { WebSocketMessage, Entity } from '../types';

function getWsUrl(): string {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const base = `${protocol}//${window.location.host}/ws/updates`;
  // Attach stored JWT as query param for WS auth
  const token = localStorage.getItem('orp_token') ?? sessionStorage.getItem('orp_token') ?? '';
  if (token) {
    return `${base}?token=${encodeURIComponent(token)}`;
  }
  return base;
}
const HEARTBEAT_TIMEOUT_MS = 45_000;
const RECONNECT_BASE_MS = 1000;
const RECONNECT_MAX_MS = 60_000;

export type SubscriptionConfig =
  | { entity_type: string; resume_from?: string }
  | { entity_id: string }
  | { region: { min_lat: number; min_lon: number; max_lat: number; max_lon: number }; entity_type?: string };

interface UseWebSocketOptions {
  /** Subscribe to all events for this entity type */
  entityType?: string;
  /** Subscribe to specific entity */
  entityId?: string;
  /** Subscribe to events in a geographic region */
  region?: { min_lat: number; min_lon: number; max_lat: number; max_lon: number };
}

export function useWebSocket(
  entityTypeOrOptions?: string | UseWebSocketOptions
) {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttempt = useRef(0);
  const heartbeatTimer = useRef<ReturnType<typeof setTimeout>>();
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>();
  // Track subscriptions so we can re-subscribe after reconnect
  const subscriptionsRef = useRef<Array<{ id: string; config: SubscriptionConfig }>>([]);

  const setWsConnected = useAppStore((s) => s.setWsConnected);
  const updateEntity = useAppStore((s) => s.updateEntity);
  const upsertEntity = useAppStore((s) => s.upsertEntity);
  const removeEntity = useAppStore((s) => s.removeEntity);
  const addAlert = useAppStore((s) => s.addAlert);

  // Normalise options
  const opts: UseWebSocketOptions =
    typeof entityTypeOrOptions === 'string'
      ? { entityType: entityTypeOrOptions }
      : (entityTypeOrOptions ?? {});

  const resetHeartbeatTimer = useCallback(() => {
    if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);
    heartbeatTimer.current = setTimeout(() => {
      // Heartbeat missed — force reconnect
      wsRef.current?.close();
    }, HEARTBEAT_TIMEOUT_MS);
  }, []);

  const sendSubscriptions = useCallback((ws: WebSocket) => {
    for (const sub of subscriptionsRef.current) {
      ws.send(JSON.stringify({ type: 'subscribe', id: sub.id, data: sub.config }));
    }
  }, []);

  const connect = useCallback(() => {
    const WS = globalThis.WebSocket;
    if (wsRef.current?.readyState === WS?.OPEN) return;

    try {
      const url = getWsUrl();
      const ws = new WS(url);
      wsRef.current = ws;

      ws.onopen = () => {
        setWsConnected(true);
        reconnectAttempt.current = 0;
        resetHeartbeatTimer();
        // Re-subscribe (handles both initial and reconnect)
        sendSubscriptions(ws);
      };

      ws.onmessage = (event) => {
        try {
          const msg: WebSocketMessage = JSON.parse(event.data as string);

          switch (msg.type) {
            case 'heartbeat':
              resetHeartbeatTimer();
              ws.send(JSON.stringify({ type: 'heartbeat_ack', timestamp: new Date().toISOString() }));
              break;

            case 'entity_update': {
              const d = msg.data;
              if (!d?.entity_id) break;
              // Extract updated property values
              const propUpdates: Record<string, unknown> = {};
              for (const [key, change] of Object.entries(d.changes ?? {})) {
                if (key === 'position') continue; // handled via geometry
                propUpdates[key] = change.after;
              }
              const update: Partial<Entity> = {
                updated_at: msg.timestamp,
                // Merge properties: spread existing properties first, then overlay updates
                properties: propUpdates as Record<string, unknown>,
              };
              // Ensure properties are merged (not replaced) by fetching existing entity
              const existingEntity = useAppStore.getState().entities.get(d.entity_id);
              if (existingEntity?.properties) {
                update.properties = { ...existingEntity.properties, ...propUpdates };
              }
              if (d.geometry) {
                update.geometry = d.geometry as Entity['geometry'];
              }
              // Handle position change from changes
              const posChange = d.changes?.['position'];
              if (posChange && Array.isArray((posChange as { after: unknown }).after)) {
                update.geometry = {
                  type: 'Point',
                  coordinates: (posChange as { after: number[] }).after,
                };
              }
              updateEntity(d.entity_id, update);
              break;
            }

            case 'entity_created': {
              const d = msg.data;
              if (!d?.entity_id) break;
              const newEntity: Entity = {
                id: d.entity_id,
                type: d.entity_type ?? 'Unknown',
                name: d.entity_name ?? null,
                tags: [],
                properties: d.properties ?? {},
                geometry: d.geometry as Entity['geometry'],
                confidence: 1,
                freshness: { updated_at: msg.timestamp, checked_at: msg.timestamp },
                created_at: msg.timestamp,
                updated_at: msg.timestamp,
                is_active: true,
              };
              upsertEntity(newEntity);
              break;
            }

            case 'entity_deleted': {
              const d = msg.data;
              if (d?.entity_id) removeEntity(d.entity_id);
              break;
            }

            case 'relationship_changed':
              // Relationship changes are informational — no store update needed for MVP
              break;

            case 'alert_triggered': {
              const d = msg.data;
              if (d) {
                addAlert({
                  id: (d as { id?: string }).id ?? `alert-${Date.now()}`,
                  monitor_id: d.monitor_id ?? '',
                  monitor_name: d.monitor_name ?? '',
                  severity: d.severity ?? 'info',
                  affected_entities: d.affected_entities ?? [],
                  timestamp: msg.timestamp,
                  acknowledged: false,
                });
              }
              break;
            }

            case 'subscription_created':
              // Confirmed — no action needed
              break;

            case 'error':
              console.warn('[WS] Server error:', msg.error);
              break;
          }
        } catch {
          // Ignore malformed messages
        }
      };

      ws.onclose = () => {
        setWsConnected(false);
        if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);
        const delay = Math.min(
          RECONNECT_BASE_MS * Math.pow(2, reconnectAttempt.current),
          RECONNECT_MAX_MS
        );
        reconnectAttempt.current += 1;
        reconnectTimer.current = setTimeout(connect, delay);
      };

      ws.onerror = () => ws.close();
    } catch {
      // Connection failed — retry
    }
  }, [setWsConnected, updateEntity, upsertEntity, removeEntity, addAlert, resetHeartbeatTimer, sendSubscriptions]);

  // Build subscription list from options
  useEffect(() => {
    const subs: Array<{ id: string; config: SubscriptionConfig }> = [];

    if (opts.entityType) {
      subs.push({ id: `sub-type-${opts.entityType}`, config: { entity_type: opts.entityType } });
    }
    if (opts.entityId) {
      subs.push({ id: `sub-entity-${opts.entityId}`, config: { entity_id: opts.entityId } });
    }
    if (opts.region) {
      subs.push({ id: `sub-region`, config: { region: opts.region } });
    }

    subscriptionsRef.current = subs;

    // If already connected, send immediately
    if (wsRef.current?.readyState === globalThis.WebSocket?.OPEN) {
      sendSubscriptions(wsRef.current);
    }
  }, [opts.entityType, opts.entityId, opts.region, sendSubscriptions]);

  useEffect(() => {
    connect();
    return () => {
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current);
      if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);
      wsRef.current?.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const send = useCallback((message: Record<string, unknown>) => {
    if (wsRef.current?.readyState === globalThis.WebSocket?.OPEN) {
      wsRef.current.send(JSON.stringify(message));
    }
  }, []);

  const subscribe = useCallback(
    (id: string, config: SubscriptionConfig) => {
      subscriptionsRef.current = [...subscriptionsRef.current, { id, config }];
      if (wsRef.current?.readyState === globalThis.WebSocket?.OPEN) {
        wsRef.current.send(JSON.stringify({ type: 'subscribe', id, data: config }));
      }
    },
    []
  );

  const unsubscribe = useCallback((subscriptionId: string) => {
    subscriptionsRef.current = subscriptionsRef.current.filter((s) => s.id !== subscriptionId);
    if (wsRef.current?.readyState === globalThis.WebSocket?.OPEN) {
      wsRef.current.send(
        JSON.stringify({ type: 'unsubscribe', id: `unsub-${Date.now()}`, data: { subscription_id: subscriptionId } })
      );
    }
  }, []);

  return { send, subscribe, unsubscribe };
}
