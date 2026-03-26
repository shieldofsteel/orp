import React, { useEffect, useRef } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { AlertEvent } from '../types';

const SEVERITY_CONFIG = {
  critical: {
    badge: 'bg-red-900/80 text-red-300 border border-red-700',
    indicator: 'bg-red-500',
    border: 'border-l-red-500',
    label: 'CRIT',
    srLabel: 'Critical alert',
  },
  warning: {
    badge: 'bg-amber-900/70 text-amber-300 border border-amber-700',
    indicator: 'bg-amber-400',
    border: 'border-l-amber-400',
    label: 'WARN',
    srLabel: 'Warning alert',
  },
  info: {
    badge: 'bg-blue-900/60 text-blue-300 border border-blue-700',
    indicator: 'bg-blue-400',
    border: 'border-l-blue-400',
    label: 'INFO',
    srLabel: 'Information alert',
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

function formatTimestampVerbose(ts: string): string {
  const d = new Date(ts);
  const now = Date.now();
  const diffMs = now - d.getTime();
  const diffMin = Math.floor(diffMs / 60_000);
  if (diffMs < 60_000) return 'just now';
  if (diffMin < 60) return `${diffMin} minute${diffMin !== 1 ? 's' : ''} ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr} hour${diffHr !== 1 ? 's' : ''} ago`;
  return d.toLocaleDateString(undefined, { month: 'long', day: 'numeric', year: 'numeric' });
}

interface AlertCardProps {
  alert: AlertEvent;
  onAck: (id: string) => void;
  onSelectEntity: (id: string) => void;
}

function AlertCard({ alert, onAck, onSelectEntity }: AlertCardProps) {
  const cfg = SEVERITY_CONFIG[alert.severity] ?? SEVERITY_CONFIG.info;
  const firstEntity = alert.affected_entities[0];

  // Build a descriptive screen-reader summary of this alert
  const alertSummary = [
    cfg.srLabel,
    `: ${alert.monitor_name}`,
    firstEntity ? `. Affected entity: ${firstEntity.entity_id}` : '',
    firstEntity?.reason ? `. Reason: ${firstEntity.reason}` : '',
    alert.affected_entities.length > 1
      ? `. Plus ${alert.affected_entities.length - 1} more entities.`
      : '',
    `. Received ${formatTimestampVerbose(alert.timestamp)}.`,
    alert.acknowledged ? ' Acknowledged.' : ' Unacknowledged.',
  ].join('');

  return (
    <article
      className={`border-l-2 ${cfg.border} bg-gray-800/60 hover:bg-gray-800 rounded-none p-2.5 transition-colors ${
        alert.acknowledged ? 'opacity-50' : ''
      }`}
      aria-label={alertSummary}
      aria-live="off" /* individual cards don't announce; the feed region handles announcements */
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <span
            className={`mt-0.5 flex-shrink-0 rounded-none px-1.5 py-0.5 text-[9px] font-bold tracking-wider ${cfg.badge}`}
            aria-hidden="true" /* severity announced via article aria-label */
          >
            {cfg.label}
          </span>
          <div className="min-w-0">
            <div className="text-xs font-medium text-gray-200 truncate">{alert.monitor_name}</div>
            {firstEntity && (
              <button
                onClick={() => onSelectEntity(firstEntity.entity_id)}
                className="mt-0.5 text-[10px] text-blue-400 hover:text-blue-300 truncate block text-left"
                aria-label={`Select entity ${firstEntity.entity_id} on map`}
              >
                {firstEntity.entity_id}
              </button>
            )}
            {firstEntity?.reason && (
              <div className="mt-0.5 text-[10px] text-gray-500 truncate">{firstEntity.reason}</div>
            )}
            {alert.affected_entities.length > 1 && (
              <div className="mt-0.5 text-[10px] text-gray-500">
                +{alert.affected_entities.length - 1} more{' '}
                <span className="sr-only">
                  affected {alert.affected_entities.length - 1 === 1 ? 'entity' : 'entities'}
                </span>
              </div>
            )}
          </div>
        </div>
        <div className="flex flex-col items-end gap-1 flex-shrink-0">
          <time
            dateTime={alert.timestamp}
            className="text-[9px] text-gray-500 whitespace-nowrap"
            aria-hidden="true" /* timestamp included in article aria-label */
          >
            {formatTimestamp(alert.timestamp)}
          </time>
          {!alert.acknowledged && (
            <button
              onClick={() => onAck(alert.id)}
              className="text-[9px] text-gray-500 hover:text-gray-300 border border-gray-700 hover:border-gray-500 rounded-none px-1.5 py-0.5 transition-colors"
              aria-label={`Acknowledge alert: ${alert.monitor_name}`}
            >
              <span aria-hidden="true">ACK</span>
              <span className="sr-only">Acknowledge</span>
            </button>
          )}
          {alert.acknowledged && (
            <span className="sr-only">Acknowledged</span>
          )}
        </div>
      </div>
    </article>
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

  // Tracks the last announced alert count to avoid duplicate announcements
  const lastAnnouncedCountRef = useRef(0);
  const [announcement, setAnnouncement] = React.useState('');

  // Auto-scroll to top when new alert arrives (alerts are prepended)
  useEffect(() => {
    if (autoScrollRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = 0;
    }
  }, [alerts.length]);

  // Announce new alerts to screen readers via aria-live
  useEffect(() => {
    if (alerts.length > lastAnnouncedCountRef.current) {
      const newCount = alerts.length - lastAnnouncedCountRef.current;
      const latest = alerts[0]; // most recent is first
      if (latest) {
        const cfg = SEVERITY_CONFIG[latest.severity] ?? SEVERITY_CONFIG.info;
        setAnnouncement(
          `${newCount} new ${cfg.srLabel.toLowerCase()}${newCount > 1 ? 's' : ''}. Latest: ${latest.monitor_name}${
            latest.affected_entities[0]
              ? ` — ${latest.affected_entities[0].entity_id}`
              : ''
          }.`
        );
      }
      lastAnnouncedCountRef.current = alerts.length;
    }
  }, [alerts]);

  const visible = alerts.slice(0, maxVisible);
  const unacknowledged = alerts.filter((a) => !a.acknowledged).length;
  const critical = alerts.filter((a) => a.severity === 'critical' && !a.acknowledged).length;

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Visually-hidden aria-live region for screen reader announcements */}
      <div
        aria-live="polite"
        aria-atomic="true"
        className="sr-only"
        role="status"
      >
        {announcement}
      </div>

      {/* Header */}
      <div className="flex items-center justify-between mb-2 px-0.5">
        <div className="flex items-center gap-2">
          {critical > 0 && (
            <>
              <span className="relative flex h-2 w-2" aria-hidden="true">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75" />
                <span className="relative inline-flex rounded-full h-2 w-2 bg-red-500" />
              </span>
              {/* Screen-reader-only announcement for critical count */}
              <span className="sr-only">
                {critical} critical unacknowledged alert{critical !== 1 ? 's' : ''}
              </span>
            </>
          )}
          <span className="text-[10px] text-gray-500" aria-hidden="true">
            {unacknowledged > 0 ? (
              <span className="text-amber-400">{unacknowledged} unacked</span>
            ) : (
              'All clear'
            )}
          </span>
          {/* Accessible equivalent */}
          <span className="sr-only">
            {unacknowledged > 0
              ? `${unacknowledged} unacknowledged alert${unacknowledged !== 1 ? 's' : ''}`
              : 'All alerts acknowledged'}
          </span>
        </div>
        {alerts.length > 0 && (
          <button
            onClick={clearAlerts}
            className="text-[9px] text-gray-600 hover:text-gray-400 transition-colors"
            aria-label={`Clear all ${alerts.length} alerts`}
          >
            Clear all
          </button>
        )}
      </div>

      {/* Alert feed — role="log" for continuous updates; aria-live="polite" for new items */}
      <div
        ref={scrollRef}
        role="log"
        aria-live="polite"
        aria-relevant="additions"
        aria-label="Alert feed — live alerts"
        aria-atomic="false"
        className="flex-1 overflow-y-auto orp-scrollbar space-y-1.5 min-h-0"
        onScroll={(e) => {
          autoScrollRef.current = (e.currentTarget.scrollTop < 50);
        }}
      >
        {visible.length === 0 ? (
          <div
            className="text-center py-6 text-[10px] text-gray-600"
            role="status"
          >
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
