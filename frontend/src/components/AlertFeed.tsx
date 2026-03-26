import React, { useEffect, useRef } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { AlertEvent } from '../types';

const SEVERITY_CONFIG = {
  critical: {
    badge: 'bg-red-900/80 text-red-300 border border-red-700',
    indicator: 'bg-red-500',
    border: 'border-l-red-500',
    label: 'CRIT',
  },
  warning: {
    badge: 'bg-amber-900/70 text-amber-300 border border-amber-700',
    indicator: 'bg-amber-400',
    border: 'border-l-amber-400',
    label: 'WARN',
  },
  info: {
    badge: 'bg-blue-900/60 text-blue-300 border border-blue-700',
    indicator: 'bg-blue-400',
    border: 'border-l-blue-400',
    label: 'INFO',
  },
};

function formatTimestamp(ts: string): string {
  const d = new Date(ts);
  const now = Date.now();
  const diffMs = now - d.getTime();
  if (diffMs < 60_000) return 'just now';
  if (diffMs < 3_600_000) return `${Math.floor(diffMs / 60_000)}m ago`;
  if (diffMs < 86_400_000) return `${Math.floor(diffMs / 3_600_000)}h ago`;
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

interface AlertCardProps {
  alert: AlertEvent;
  onAck: (id: string) => void;
  onSelectEntity: (id: string) => void;
}

function AlertCard({ alert, onAck, onSelectEntity }: AlertCardProps) {
  const cfg = SEVERITY_CONFIG[alert.severity] ?? SEVERITY_CONFIG.info;
  const firstEntity = alert.affected_entities[0];

  return (
    <div
      className={`border-l-2 ${cfg.border} bg-gray-800/60 hover:bg-gray-800 rounded-r-md p-2.5 transition-colors ${
        alert.acknowledged ? 'opacity-50' : ''
      }`}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <span className={`mt-0.5 flex-shrink-0 rounded px-1.5 py-0.5 text-[9px] font-bold tracking-wider ${cfg.badge}`}>
            {cfg.label}
          </span>
          <div className="min-w-0">
            <div className="text-xs font-medium text-gray-200 truncate">{alert.monitor_name}</div>
            {firstEntity && (
              <button
                onClick={() => onSelectEntity(firstEntity.entity_id)}
                className="mt-0.5 text-[10px] text-blue-400 hover:text-blue-300 truncate block text-left"
              >
                {firstEntity.entity_id}
              </button>
            )}
            {firstEntity?.reason && (
              <div className="mt-0.5 text-[10px] text-gray-500 truncate">{firstEntity.reason}</div>
            )}
            {alert.affected_entities.length > 1 && (
              <div className="mt-0.5 text-[10px] text-gray-500">
                +{alert.affected_entities.length - 1} more entities
              </div>
            )}
          </div>
        </div>
        <div className="flex flex-col items-end gap-1 flex-shrink-0">
          <span className="text-[9px] text-gray-500 whitespace-nowrap">
            {formatTimestamp(alert.timestamp)}
          </span>
          {!alert.acknowledged && (
            <button
              onClick={() => onAck(alert.id)}
              className="text-[9px] text-gray-500 hover:text-gray-300 border border-gray-700 hover:border-gray-500 rounded px-1.5 py-0.5 transition-colors"
            >
              ACK
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

interface AlertFeedProps {
  maxVisible?: number;
}

export const AlertFeed: React.FC<AlertFeedProps> = ({ maxVisible = 50 }) => {
  const alerts = useAppStore((s) => s.alerts);
  const acknowledgeAlert = useAppStore((s) => s.acknowledgeAlert);
  const clearAlerts = useAppStore((s) => s.clearAlerts);
  const selectEntity = useAppStore((s) => s.selectEntity);

  const scrollRef = useRef<HTMLDivElement>(null);
  const autoScrollRef = useRef(true);

  // Auto-scroll to top when new alert arrives (alerts are prepended)
  useEffect(() => {
    if (autoScrollRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = 0;
    }
  }, [alerts.length]);

  const visible = alerts.slice(0, maxVisible);
  const unacknowledged = alerts.filter((a) => !a.acknowledged).length;
  const critical = alerts.filter((a) => a.severity === 'critical' && !a.acknowledged).length;

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Header */}
      <div className="flex items-center justify-between mb-2 px-0.5">
        <div className="flex items-center gap-2">
          {critical > 0 && (
            <span className="relative flex h-2 w-2">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2 w-2 bg-red-500" />
            </span>
          )}
          <span className="text-[10px] text-gray-500">
            {unacknowledged > 0 ? (
              <span className="text-amber-400">{unacknowledged} unacked</span>
            ) : (
              'All clear'
            )}
          </span>
        </div>
        {alerts.length > 0 && (
          <button
            onClick={clearAlerts}
            className="text-[9px] text-gray-600 hover:text-gray-400 transition-colors"
          >
            Clear all
          </button>
        )}
      </div>

      {/* Feed */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto orp-scrollbar space-y-1.5 min-h-0"
        onScroll={(e) => {
          autoScrollRef.current = (e.currentTarget.scrollTop < 50);
        }}
      >
        {visible.length === 0 ? (
          <div className="text-center py-6 text-[10px] text-gray-600">
            No alerts
          </div>
        ) : (
          visible.map((alert) => (
            <AlertCard
              key={alert.id}
              alert={alert}
              onAck={acknowledgeAlert}
              onSelectEntity={selectEntity}
            />
          ))
        )}
      </div>
    </div>
  );
};
