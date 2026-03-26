import React, { useState, useEffect } from 'react';
import { useAppStore } from '../store/useAppStore';
import { QueryBar } from './QueryBar';
import { AlertFeed } from './AlertFeed';
import type { Connector } from '../types';

const API_BASE = '/api/v1';

/** Raw data-source shape returned by the backend `/api/v1/connectors` endpoint */
interface RawDataSource {
  source_id: string;
  source_name: string;
  source_type: string;
  trust_score: number;
  events_ingested: number;
  entities_provided: number;
  error_count: number;
  enabled: boolean;
  last_heartbeat: string | null;
  certificate_fingerprint: string | null;
}

/** Convert a backend DataSource to the frontend Connector shape */
function toConnector(ds: RawDataSource): Connector {
  const status: Connector['status'] =
    !ds.enabled ? 'error'
    : ds.error_count > 0 ? 'degraded'
    : 'healthy';

  return {
    id: ds.source_id,
    name: ds.source_name,
    type: ds.source_type,
    enabled: ds.enabled,
    status,
    stats: {
      events_per_sec: 0,
      last_event_at: ds.last_heartbeat ?? new Date().toISOString(),
      error_count: ds.error_count ?? 0,
      total_ingested: ds.events_ingested ?? 0,
    },
  };
}

const STATUS_CONFIG = {
  healthy: { dot: 'bg-green-500', text: 'text-green-400', label: 'Healthy' },
  degraded: { dot: 'bg-amber-400', text: 'text-amber-400', label: 'Degraded' },
  error: { dot: 'bg-red-500', text: 'text-red-400', label: 'Error' },
};

function formatLastEvent(ts: string): string {
  const d = new Date(ts);
  const diff = Date.now() - d.getTime();
  if (diff < 5_000) return 'just now';
  if (diff < 60_000) return `${Math.floor(diff / 1_000)}s ago`;
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  return `${Math.floor(diff / 3_600_000)}h ago`;
}

function ConnectorCard({ connector }: { connector: Connector }) {
  const cfg = STATUS_CONFIG[connector.status] ?? STATUS_CONFIG.error;

  return (
    <div className={`rounded-none border bg-gray-800/50 p-2.5 transition-colors ${
      connector.enabled ? 'border-gray-700' : 'border-gray-800 opacity-50'
    }`}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <span className={`w-1.5 h-1.5 rounded-none flex-shrink-0 ${cfg.dot}`} />
            <span className="text-[11px] font-medium text-gray-200 truncate">
              {connector.name}
            </span>
          </div>
          <div className="flex items-center gap-1.5 mt-1">
            <span className="text-[9px] text-gray-600 bg-gray-800 border border-gray-700 rounded-none px-1 py-0.5">
              {connector.type}
            </span>
            <span className={`text-[9px] ${cfg.text}`}>{cfg.label}</span>
          </div>
        </div>
        {!connector.enabled && (
          <span className="text-[9px] text-gray-600 flex-shrink-0">disabled</span>
        )}
      </div>

      {connector.enabled && (
        <div className="mt-2 grid grid-cols-3 gap-1">
          <div className="text-center">
            <div className="text-[11px] font-mono text-blue-300">
              {connector?.stats?.events_per_sec?.toFixed(1) ?? "0"}
            </div>
            <div className="text-[8px] text-gray-600">evt/s</div>
          </div>
          <div className="text-center">
            <div className="text-[11px] font-mono text-gray-300">
              {connector?.stats?.error_count > 0 ? (
                <span className="text-red-400">{connector?.stats?.error_count ?? 0}</span>
              ) : (
                <span className="text-green-400">0</span>
              )}
            </div>
            <div className="text-[8px] text-gray-600">errors</div>
          </div>
          <div className="text-center">
            <div className="text-[10px] font-mono text-gray-400 truncate">
              {formatLastEvent(connector?.stats?.last_event_at)}
            </div>
            <div className="text-[8px] text-gray-600">last evt</div>
          </div>
        </div>
      )}
    </div>
  );
}

interface SectionProps {
  title: string;
  badge?: number;
  collapsed: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}

function Section({ title, badge, collapsed, onToggle, children }: SectionProps) {
  return (
    <div className="flex flex-col min-h-0">
      <button
        onClick={onToggle}
        className="flex items-center justify-between px-3 py-2 hover:bg-gray-800/50 transition-colors flex-shrink-0 group"
      >
        <div className="flex items-center gap-2">
          <span className={`text-[9px] text-gray-600 transition-transform ${collapsed ? '' : 'rotate-90'}`}>▶</span>
          <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500 group-hover:text-gray-400">
            {title}
          </span>
          {badge != null && badge > 0 && (
            <span className="text-[9px] bg-red-900/70 border border-red-800/60 text-red-300 rounded-none px-1.5 min-w-[18px] text-center">
              {badge}
            </span>
          )}
        </div>
      </button>
      {!collapsed && (
        <div className="flex-1 overflow-y-auto orp-scrollbar min-h-0 px-3 pb-3">
          {children}
        </div>
      )}
    </div>
  );
}

export const Sidebar: React.FC = () => {
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const sidebarSections = useAppStore((s) => s.sidebarSections);
  const toggleSidebarSection = useAppStore((s) => s.toggleSidebarSection);
  const alerts = useAppStore((s) => s.alerts);
  const wsConnected = useAppStore((s) => s.wsConnected);
  const showHeatmapLayer = useAppStore((s) => s.showHeatmapLayer);
  const showWeatherLayer = useAppStore((s) => s.showWeatherLayer);
  const showShipTracksLayer = useAppStore((s) => s.showShipTracksLayer);
  const toggleHeatmapLayer = useAppStore((s) => s.toggleHeatmapLayer);
  const toggleWeatherLayer = useAppStore((s) => s.toggleWeatherLayer);
  const toggleShipTracksLayer = useAppStore((s) => s.toggleShipTracksLayer);

  const [connectors, setConnectors] = useState<Connector[]>([]);

  const unackedAlerts = alerts.filter((a) => !a.acknowledged).length;

  // Fetch real connectors from API and map DataSource → Connector
  useEffect(() => {
    fetch(`${API_BASE}/connectors`)
      .then((r) => r.ok ? r.json() : null)
      .then((data) => {
        const raw = (data as { data?: RawDataSource[] })?.data;
        if (Array.isArray(raw)) {
          setConnectors(raw.map(toConnector));
        }
      })
      .catch(() => {/* API unavailable — leave empty */});
  }, []);

  const section = (id: 'datasources' | 'query' | 'alerts') =>
    sidebarSections.find((s) => s.id === id) ?? { collapsed: false };

  if (!sidebarOpen) return null;

  return (
    <div className="flex-shrink-0 w-72 bg-gray-900 border-r border-gray-800 flex flex-col overflow-hidden">
      {/* Layer Toggles */}
      <div className="flex-shrink-0 px-3 py-2 border-b border-gray-800">
        <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
          Layers
        </div>
        <div className="flex gap-1.5 flex-wrap">
          {[
            { label: 'Weather', active: showWeatherLayer, toggle: toggleWeatherLayer },
            { label: 'Tracks', active: showShipTracksLayer, toggle: toggleShipTracksLayer },
            { label: 'Heatmap', active: showHeatmapLayer, toggle: toggleHeatmapLayer },
          ].map(({ label, active, toggle }) => (
            <button
              key={label}
              onClick={toggle}
              className={`text-[9px] px-2 py-0.5 rounded-none border transition-colors ${
                active
                  ? 'bg-blue-900/50 border-blue-700 text-blue-300'
                  : 'border-gray-700 text-gray-600 hover:border-gray-600 hover:text-gray-400'
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {/* Sections */}
      <div className="flex-1 flex flex-col min-h-0 divide-y divide-gray-800">
        <Section
          title="Data Sources"
          collapsed={section('datasources').collapsed}
          onToggle={() => toggleSidebarSection('datasources')}
        >
          <div className="space-y-2 pt-1">
            {connectors.length === 0 ? (
              <div className="text-center py-4 text-[10px] text-gray-600">
                No connectors configured
              </div>
            ) : (
              connectors.map((c) => (
                <ConnectorCard key={c.id} connector={c} />
              ))
            )}
          </div>
        </Section>

        <Section
          title="Query"
          collapsed={section('query').collapsed}
          onToggle={() => toggleSidebarSection('query')}
        >
          <div className="pt-1">
            <QueryBar />
          </div>
        </Section>

        <Section
          title="Alerts"
          badge={unackedAlerts}
          collapsed={section('alerts').collapsed}
          onToggle={() => toggleSidebarSection('alerts')}
        >
          <div className="pt-1 flex flex-col flex-1 min-h-0" style={{ height: 220 }}>
            <AlertFeed />
          </div>
        </Section>
      </div>

      {/* Footer */}
      <div className="flex-shrink-0 px-3 py-2 border-t border-gray-800 flex items-center gap-2">
        <span className={`w-1.5 h-1.5 rounded-none flex-shrink-0 ${wsConnected ? 'bg-green-500' : 'bg-red-500'}`} />
        <span className="text-[9px] text-gray-600">
          {wsConnected ? 'WS connected' : 'WS disconnected'}
        </span>
      </div>
    </div>
  );
};
