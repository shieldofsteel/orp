import React, { useEffect, useState, useCallback, useRef } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { Connector, AlertEvent, HealthResponse } from '../types';

const API_BASE = '/api/v1';

// ── Types ──────────────────────────────────────────────────────────────────

interface KPI {
  label: string;
  value: string | number;
  sub?: string;
  color: string;
  trend?: 'up' | 'down' | 'flat';
}

interface EntityTypeStat {
  type: string;
  count: number;
  color: string;
}

interface EpsPoint {
  t: number;
  v: number;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

function fmt(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

// ── Sparkline ──────────────────────────────────────────────────────────────

function Sparkline({ data, color = '#3b82f6', height = 32 }: { data: number[]; color?: string; height?: number }) {
  if (data.length < 2) return <div style={{ height }} className="bg-gray-800/40 flex items-center justify-center text-[9px] text-gray-600">No data</div>;

  const max = Math.max(...data, 1);
  const w = 200;
  const h = height;
  const step = w / (data.length - 1);
  const points = data.map((v, i) => `${i * step},${h - (v / max) * (h - 4)}`).join(' ');

  return (
    <svg width="100%" height={h} viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" className="overflow-visible">
      <defs>
        <linearGradient id="spark-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={color} stopOpacity="0.3" />
          <stop offset="100%" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      <polygon points={`0,${h} ${points} ${(data.length - 1) * step},${h}`} fill="url(#spark-grad)" />
      <polyline points={points} fill="none" stroke={color} strokeWidth="1.5" strokeLinejoin="round" />
      {/* Last point dot */}
      <circle
        cx={(data.length - 1) * step}
        cy={h - (data[data.length - 1] / max) * (h - 4)}
        r="2.5"
        fill={color}
      />
    </svg>
  );
}

// ── PieChart ───────────────────────────────────────────────────────────────

function PieChart({ stats }: { stats: EntityTypeStat[] }) {
  const total = stats.reduce((s, t) => s + t.count, 0);
  if (total === 0) return <div className="flex items-center justify-center h-24 text-gray-600 text-xs">No data</div>;

  const size = 80;
  const cx = size / 2;
  const cy = size / 2;
  const r = 30;
  const gap = 1;

  let cumAngle = -Math.PI / 2;
  const slices = stats.map((s) => {
    const angle = (s.count / total) * 2 * Math.PI;
    const start = cumAngle;
    cumAngle += angle;
    return { ...s, start, angle };
  });

  const describeArc = (start: number, angle: number) => {
    if (angle >= 2 * Math.PI - 0.001) {
      return `M ${cx - r} ${cy} A ${r} ${r} 0 1 1 ${cx - r + 0.001} ${cy}`;
    }
    const end = start + angle - gap / r;
    const x1 = cx + r * Math.cos(start);
    const y1 = cy + r * Math.sin(start);
    const x2 = cx + r * Math.cos(end);
    const y2 = cy + r * Math.sin(end);
    const large = angle > Math.PI ? 1 : 0;
    return `M ${cx} ${cy} L ${x1} ${y1} A ${r} ${r} 0 ${large} 1 ${x2} ${y2} Z`;
  };

  return (
    <div className="flex items-center gap-4">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="flex-shrink-0">
        {slices.map((s) => (
          <path key={s.type} d={describeArc(s.start, s.angle)} fill={s.color} opacity={0.9}>
            <title>{s.type}: {s.count}</title>
          </path>
        ))}
        {/* Donut hole */}
        <circle cx={cx} cy={cy} r={r * 0.55} fill="#030712" />
        <text x={cx} y={cy + 1} textAnchor="middle" dominantBaseline="middle" className="text-[8px]" fill="#6b7280" style={{ fontSize: 9 }}>
          {fmt(total)}
        </text>
      </svg>
      <div className="flex flex-col gap-1">
        {slices.map((s) => (
          <div key={s.type} className="flex items-center gap-1.5">
            <div className="w-2 h-2 flex-shrink-0" style={{ backgroundColor: s.color }} />
            <span className="text-[10px] text-gray-400">{s.type}</span>
            <span className="text-[10px] text-gray-500 ml-auto pl-2">{fmt(s.count)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── KPICard ────────────────────────────────────────────────────────────────

function KPICard({ kpi }: { kpi: KPI }) {
  return (
    <div className="bg-gray-900 border border-gray-800 p-3 flex flex-col gap-1">
      <span className="text-[10px] text-gray-500 uppercase tracking-wider">{kpi.label}</span>
      <div className="flex items-end gap-2">
        <span className={`text-2xl font-bold tabular-nums ${kpi.color}`}>{kpi.value}</span>
        {kpi.trend && (
          <span className={`text-[10px] mb-1 ${kpi.trend === 'up' ? 'text-green-400' : kpi.trend === 'down' ? 'text-red-400' : 'text-gray-500'}`}>
            {kpi.trend === 'up' ? '▲' : kpi.trend === 'down' ? '▼' : '—'}
          </span>
        )}
      </div>
      {kpi.sub && <span className="text-[10px] text-gray-600">{kpi.sub}</span>}
    </div>
  );
}

// ── AlertRow ───────────────────────────────────────────────────────────────

function AlertRow({ alert }: { alert: AlertEvent }) {
  const severityColor: Record<string, string> = {
    critical: 'text-red-400 bg-red-900/20 border-red-800',
    warning: 'text-amber-400 bg-amber-900/20 border-amber-800',
    info: 'text-blue-400 bg-blue-900/20 border-blue-800',
  };
  const cls = severityColor[alert.severity] ?? 'text-gray-400 bg-gray-800 border-gray-700';

  return (
    <div className="flex items-start gap-2.5 px-3 py-2 border-b border-gray-800 hover:bg-gray-800/40 transition-colors">
      <span className={`text-[9px] font-bold px-1.5 py-0.5 border flex-shrink-0 mt-0.5 ${cls}`}>
        {alert.severity.toUpperCase()}
      </span>
      <div className="flex-1 min-w-0">
        <div className="text-xs text-gray-200 truncate">{alert.monitor_name}</div>
        <div className="text-[10px] text-gray-500 mt-0.5">
          {alert.affected_entities.length} entit{alert.affected_entities.length !== 1 ? 'ies' : 'y'} affected
          {' · '}
          {new Date(alert.timestamp).toLocaleTimeString()}
        </div>
      </div>
      {!alert.acknowledged && (
        <div className="w-1.5 h-1.5 bg-red-500 rounded-full flex-shrink-0 mt-1.5 animate-pulse" />
      )}
    </div>
  );
}

// ── ConnectorStatusBar ─────────────────────────────────────────────────────

function ConnectorStatusBar({ connectors }: { connectors: Connector[] }) {
  if (connectors.length === 0) {
    return <div className="text-[10px] text-gray-600 px-3 py-2">No connectors configured</div>;
  }

  const healthy = connectors.filter((c) => c.status === 'healthy').length;
  const degraded = connectors.filter((c) => c.status === 'degraded').length;
  const errored = connectors.filter((c) => c.status === 'error').length;

  return (
    <div className="space-y-1.5 px-3 py-2">
      {/* Summary bar */}
      <div className="flex items-center gap-2 mb-2">
        <div className="flex-1 h-1.5 bg-gray-800 flex overflow-hidden">
          <div className="bg-green-500 h-full transition-all" style={{ width: `${(healthy / connectors.length) * 100}%` }} />
          <div className="bg-amber-500 h-full transition-all" style={{ width: `${(degraded / connectors.length) * 100}%` }} />
          <div className="bg-red-500 h-full transition-all" style={{ width: `${(errored / connectors.length) * 100}%` }} />
        </div>
        <span className="text-[10px] text-gray-400 flex-shrink-0">
          {healthy}/{connectors.length} healthy
        </span>
      </div>

      {/* Connector list */}
      <div className="space-y-1">
        {connectors.map((c) => {
          const statusColor = c.status === 'healthy' ? 'bg-green-500' : c.status === 'degraded' ? 'bg-amber-500' : 'bg-red-500';
          return (
            <div key={c.id} className="flex items-center gap-2">
              <div className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${statusColor}`} />
              <span className="text-[10px] text-gray-400 flex-1 truncate">{c.name}</span>
              <span className="text-[9px] text-gray-600">{(c.stats?.events_per_sec ?? 0).toFixed(1)} e/s</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── Dashboard ──────────────────────────────────────────────────────────────

export function Dashboard({ onNavigate }: { onNavigate?: (tab: string) => void }) {
  const entities = useAppStore((s) => s.entities);
  const alerts = useAppStore((s) => s.alerts);

  const [connectors, setConnectors] = useState<Connector[]>([]);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [epsHistory, setEpsHistory] = useState<number[]>(Array(30).fill(0));
  const [loading, setLoading] = useState(true);
  const epsRef = useRef<number[]>(Array(30).fill(0));

  const token = localStorage.getItem('orp_token');
  const headers: Record<string, string> = token ? { Authorization: `Bearer ${token}` } : {};

  const refresh = useCallback(async () => {
    try {
      const [connRes, healthRes] = await Promise.allSettled([
        fetch(`${API_BASE}/connectors`, { headers }),
        fetch(`${API_BASE}/health`, { headers }),
      ]);

      if (connRes.status === 'fulfilled' && connRes.value.ok) {
        const d = await connRes.value.json();
        const list: Connector[] = Array.isArray(d) ? d : (d.connectors ?? d.data ?? []);
        setConnectors(list);

        // Update EPS sparkline
        const totalEps = list.reduce((s, c) => s + (c.stats?.events_per_sec ?? 0), 0);
        epsRef.current = [...epsRef.current.slice(1), totalEps];
        setEpsHistory([...epsRef.current]);
      }

      if (healthRes.status === 'fulfilled' && healthRes.value.ok) {
        setHealth(await healthRes.value.json());
      }
    } catch (_) {
      // Silently handle failures — dashboard degrades gracefully
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, [refresh]);

  // ── Derived stats ─────────────────────────────────────────────────────

  const totalEntities = entities.size;
  const activeShips = Array.from(entities.values()).filter(
    (e) => e.type === 'Ship' && e.is_active
  ).length;
  const alertsToday = alerts.filter((a) => {
    const ts = new Date(a.timestamp);
    const now = new Date();
    return ts.toDateString() === now.toDateString();
  }).length;
  const healthyConnectors = connectors.filter((c) => c.status === 'healthy').length;
  const totalEps = connectors.reduce((s, c) => s + (c.stats?.events_per_sec ?? 0), 0);
  const criticalAlerts = alerts.filter((a) => a.severity === 'critical' && !a.acknowledged).length;

  const kpis: KPI[] = [
    { label: 'Total Entities', value: fmt(totalEntities), sub: 'across all sources', color: 'text-gray-100', trend: 'flat' },
    { label: 'Active Ships', value: fmt(activeShips), sub: 'transmitting now', color: 'text-blue-400', trend: activeShips > 0 ? 'up' : 'flat' },
    { label: 'Alerts Today', value: alertsToday, sub: `${criticalAlerts} critical`, color: criticalAlerts > 0 ? 'text-red-400' : 'text-amber-400', trend: criticalAlerts > 0 ? 'up' : 'flat' },
    { label: 'Connectors', value: `${healthyConnectors}/${connectors.length}`, sub: `${totalEps.toFixed(1)} e/s total`, color: healthyConnectors === connectors.length ? 'text-green-400' : 'text-amber-400' },
  ];

  // Entity type distribution
  const typeCountMap = new Map<string, number>();
  entities.forEach((e) => {
    typeCountMap.set(e.type, (typeCountMap.get(e.type) ?? 0) + 1);
  });
  const typeColors: Record<string, string> = {
    Ship: '#3b82f6',
    Port: '#f59e0b',
    Aircraft: '#22d3ee',
    WeatherSystem: '#a855f7',
    Zone: '#22c55e',
    Facility: '#f43f5e',
  };
  const typeStats: EntityTypeStat[] = Array.from(typeCountMap.entries())
    .sort((a, b) => b[1] - a[1])
    .map(([type, count]) => ({ type, count, color: typeColors[type] ?? '#6b7280' }));

  const recentAlerts = [...alerts]
    .sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime())
    .slice(0, 8);

  const systemOverall = health?.status ?? (loading ? 'checking' : 'unknown');
  const systemColor = systemOverall === 'healthy' ? 'text-green-400' : systemOverall === 'degraded' ? 'text-amber-400' : 'text-red-400';

  return (
    <div className="h-full overflow-y-auto bg-gray-950 text-gray-200 p-4 space-y-4">
      {/* System health bar */}
      <div className="flex items-center gap-3 px-3 py-2 bg-gray-900 border border-gray-800">
        <div className={`w-2 h-2 rounded-full flex-shrink-0 ${systemOverall === 'healthy' ? 'bg-green-500 animate-pulse' : systemOverall === 'degraded' ? 'bg-amber-500' : 'bg-gray-600'}`} />
        <span className={`text-[11px] font-semibold ${systemColor}`}>
          System {systemOverall.toUpperCase()}
        </span>
        {health && (
          <>
            <div className="h-3 w-px bg-gray-800" />
            {Object.entries(health.components).map(([comp, val]) => (
              <div key={comp} className="flex items-center gap-1">
                <div className={`w-1.5 h-1.5 rounded-full ${val.status === 'healthy' ? 'bg-green-500' : 'bg-amber-500'}`} />
                <span className="text-[9px] text-gray-500">{comp.replace('_', ' ')}</span>
                {(val as { latency_ms?: number }).latency_ms !== undefined && (
                  <span className="text-[9px] text-gray-600">{(val as { latency_ms?: number }).latency_ms}ms</span>
                )}
              </div>
            ))}
          </>
        )}
        <div className="flex-1" />
        <span className="text-[9px] text-gray-600">Auto-refresh 5s</span>
      </div>

      {/* KPI Grid */}
      <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
        {kpis.map((kpi) => <KPICard key={kpi.label} kpi={kpi} />)}
      </div>

      {/* Charts row */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {/* Entity distribution */}
        <div className="bg-gray-900 border border-gray-800 p-3">
          <div className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider mb-3">Entity Distribution</div>
          {typeStats.length === 0 ? (
            <div className="flex items-center justify-center h-24 text-gray-600 text-xs">No entities loaded</div>
          ) : (
            <PieChart stats={typeStats} />
          )}
        </div>

        {/* EPS Sparkline */}
        <div className="bg-gray-900 border border-gray-800 p-3">
          <div className="flex items-center justify-between mb-3">
            <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Events / Second</span>
            <span className="text-lg font-bold text-blue-400 tabular-nums">{totalEps.toFixed(1)}</span>
          </div>
          <Sparkline data={epsHistory} color="#3b82f6" height={48} />
          <div className="flex justify-between mt-1">
            <span className="text-[9px] text-gray-600">30s ago</span>
            <span className="text-[9px] text-gray-600">now</span>
          </div>
        </div>
      </div>

      {/* Alerts + Connectors row */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {/* Top alerts */}
        <div className="bg-gray-900 border border-gray-800 flex flex-col">
          <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800">
            <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Recent Alerts</span>
            <button
              onClick={() => onNavigate?.('map')}
              className="text-[10px] text-gray-600 hover:text-blue-400 transition-colors"
            >
              View all →
            </button>
          </div>
          <div className="flex-1 overflow-y-auto max-h-52">
            {recentAlerts.length === 0 ? (
              <div className="flex items-center justify-center h-20 text-gray-600 text-xs">No alerts</div>
            ) : (
              recentAlerts.map((a) => <AlertRow key={a.id} alert={a} />)
            )}
          </div>
        </div>

        {/* Connector health */}
        <div className="bg-gray-900 border border-gray-800 flex flex-col">
          <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800">
            <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Connector Health</span>
            <span className="text-[10px] text-gray-600">{connectors.length} configured</span>
          </div>
          <div className="flex-1 overflow-y-auto max-h-52">
            <ConnectorStatusBar connectors={connectors} />
          </div>
        </div>
      </div>

      {/* Quick actions */}
      <div className="bg-gray-900 border border-gray-800 p-3">
        <div className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider mb-2">Quick Actions</div>
        <div className="flex flex-wrap gap-2">
          <button
            onClick={() => onNavigate?.('query')}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-900/30 border border-blue-800 text-blue-400 text-[11px] hover:bg-blue-900/50 transition-colors"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z"/></svg>
            Run Saved Query
          </button>
          <button
            onClick={() => onNavigate?.('search')}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 border border-gray-700 text-gray-300 text-[11px] hover:bg-gray-700 transition-colors"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/></svg>
            Search Entities
          </button>
          <button
            onClick={() => onNavigate?.('map')}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 border border-gray-700 text-gray-300 text-[11px] hover:bg-gray-700 transition-colors"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 20l-5.447-2.724A1 1 0 013 16.382V5.618a1 1 0 011.447-.894L9 7m0 13l6-3m-6 3V7m6 10l4.553 2.276A1 1 0 0021 18.382V7.618a1 1 0 00-.553-.894L15 4m0 13V4m0 0L9 7"/></svg>
            Map View
          </button>
          <button
            onClick={() => onNavigate?.('map')}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-red-950/40 border border-red-900 text-red-400 text-[11px] hover:bg-red-900/40 transition-colors"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9"/></svg>
            {criticalAlerts > 0 ? `${criticalAlerts} Critical Alert${criticalAlerts !== 1 ? 's' : ''}` : 'View Alerts'}
          </button>
        </div>
      </div>
    </div>
  );
}
