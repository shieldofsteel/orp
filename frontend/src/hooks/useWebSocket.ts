import { useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { AlertEvent, EntityUpdate, WebSocketMessage } from '../types';

const WS_URL = `${window.location.protocol === 'https:' ? 'wss:' : 'ws:'}//${window.location.host}/ws/updates`;
const HEARTBEAT_TIMEOUT_MS = 45_000;
const RECONNECT_BASE_MS = 1000;
const RECONNECT_MAX_MS = 60_000;

export function useWebSocket(entityType?: string) {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttempt = useRef(0);
  const heartbeatTimer = useRef<ReturnType<typeof setTimeout>>();
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>();

  const setWsConnected = useAppStore((s) => s.setWsConnected);
  const updateEntity = useAppStore((s) => s.updateEntity);
  const addAlert = useAppStore((s) => s.addAlert);

  const resetHeartbeatTimer = useCallback(() => {
    if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);
    heartbeatTimer.current = setTimeout(() => {
      // Heartbeat missed — close and reconnect
      wsRef.current?.close();
    }, HEARTBEAT_TIMEOUT_MS);
  }, []);

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    try {
      const ws = new WebSocket(WS_URL);
      wsRef.current = ws;

      ws.onopen = () => {
        setWsConnected(true);
        reconnectAttempt.current = 0;
        resetHeartbeatTimer();

        // Subscribe to entity type
        if (entityType) {
          ws.send(
            JSON.stringify({
              type: 'subscribe',
              id: `sub-${entityType}`,
              data: { entity_type: entityType },
            })
          );
        }
      };

      ws.onmessage = (event) => {
        try {
          const msg: WebSocketMessage = JSON.parse(event.data);

          switch (msg.type) {
            case 'heartbeat':
              resetHeartbeatTimer();
              ws.send(
                JSON.stringify({
                  type: 'heartbeat_ack',
                  timestamp: new Date().toISOString(),
                })
              );
              break;

            case 'entity_update': {
              const data = msg.data as unknown as EntityUpdate;
              if (data?.entity_id) {
                updateEntity(data.entity_id, {
                  properties: Object.fromEntries(
                    Object.entries(data.changes ?? {}).map(([k, v]) => [
                      k,
                      (v as { after: unknown }).after,
                    ])
                  ),
                  updated_at: msg.timestamp,
                });
              }
              break;
            }

            case 'alert_triggered': {
              const alertData = msg.data as unknown as AlertEvent;
              if (alertData) {
                addAlert({
                  ...alertData,
                  id: alertData.id ?? `alert-${Date.now()}`,
                  timestamp: msg.timestamp,
                  acknowledged: false,
                });
              }
              break;
            }

            case 'subscription_created':
              // Subscription confirmed
              break;

            default:
              break;
          }
        } catch {
          // Ignore malformed messages
        }
      };

      ws.onclose = () => {
        setWsConnected(false);
        if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);

        // Exponential backoff reconnection
        const delay = Math.min(
          RECONNECT_BASE_MS * Math.pow(2, reconnectAttempt.current),
          RECONNECT_MAX_MS
        );
        reconnectAttempt.current += 1;

        reconnectTimer.current = setTimeout(() => {
          connect();
        }, delay);
      };

      ws.onerror = () => {
        ws.close();
      };
    } catch {
      // Connection failed — will retry via onclose
    }
  }, [entityType, setWsConnected, updateEntity, addAlert, resetHeartbeatTimer]);

  useEffect(() => {
    connect();

    return () => {
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current);
      if (heartbeatTimer.current) clearTimeout(heartbeatTimer.current);
      wsRef.current?.close();
    };
  }, [connect]);

  const send = useCallback((message: Record<string, unknown>) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(message));
    }
  }, []);

  return { send };
}
