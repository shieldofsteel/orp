/**
 * EntityInspector — Military-grade entity detail panel
 * Slide-in from right, resizable, 6 tabs: Overview / Properties / Relationships / Events / Track / Intel
 */
import React, {
  useState,
  useEffect,
  useRef,
  useCallback,
  useMemo,
} from 'react';
import { useAppStore } from '../store/useAppStore';
import type { Entity, RelationshipsResponse } from '../types';

const API_BASE = '/api/v1';

// ─── API helpers ───────────────────────────────────────────────────────────────

async function fetchEntityFull(id: string): Promise<Entity> {
  const res = await fetch(`${API_BASE}/entities/${encodeURIComponent(id)}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchRelationships(id: string): Promise<RelationshipsResponse> {
  const res = await fetch(
    `${API_BASE}/entities/${encodeURIComponent(id)}/relationships`
  );
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function putEntityProperty(
  id: string,
  key: string,
  value: unknown
): Promise<void> {
  const res = await fetch(`${API_BASE}/entities/${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ properties: { [key]: value } }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
}

// ─── Tab definitions ──────────────────────────────────────────────────────────

type Tab = 'overview' | 'properties' | 'relationships' | 'events' | 'track' | 'intel';

const TABS: { id: Tab; label: string; shortcut: string }[] = [
  { id: 'overview',       label: 'OVERVIEW',       shortcut: '1' },
  { id: 'properties',     label: 'PROPERTIES',     shortcut: '2' },
  { id: 'relationships',  label: 'RELATIONSHIPS',  shortcut: '3' },
  { id: 'events',         label: 'EVENTS',         shortcut: '4' },
  { id: 'track',          label: 'TRACK',          shortcut: '5' },
  { id: 'intel',          label: 'INTEL',          shortcut: '6' },
];

// ─── Entity type helpers ──────────────────────────────────────────────────────

type EntityKind = 'ship' | 'aircraft' | 'port' | 'sensor' | 'unknown';

function resolveKind(entity: Entity): EntityKind {
  const t = entity.type?.toLowerCase() ?? '';
  if (t.includes('ship') || t.includes('vessel') || t.includes('ais')) return 'ship';
  if (t.includes('aircraft') || t.includes('flight') || t.includes('adsb')) return 'aircraft';
  if (t.includes('port') || t.includes('harbor')) return 'port';
  if (t.includes('sensor') || t.includes('radar') || t.includes('camera')) return 'sensor';
  return 'unknown';
}

function EntityIcon({ kind, size = 28 }: { kind: EntityKind; size?: number }) {
  const s = size;
  const props = { width: s, height: s, viewBox: '0 0 24 24', fill: 'none' };
  if (kind === 'ship')
    return (
      <svg {...props} aria-label="Ship">
        <path d="M3 17l1.5-6h15L21 17H3z" stroke="#38bdf8" strokeWidth="1.5" fill="none" />
        <path d="M8 11V7l4-3 4 3v4" stroke="#38bdf8" strokeWidth="1.5" fill="none" />
        <path d="M3 17c0 2 2 3 4 3h10c2 0 4-1 4-3" stroke="#38bdf8" strokeWidth="1.5" fill="none" />
      </svg>
    );
  if (kind === 'aircraft')
    return (
      <svg {...props} aria-label="Aircraft">
        <path d="M12 3L4 14h3l-1 7 6-2 6 2-1-7h3L12 3z" stroke="#34d399" strokeWidth="1.5" fill="none" />
      </svg>
    );
  if (kind === 'port')
    return (
      <svg {...props} aria-label="Port">
        <rect x="3" y="8" width="18" height="10" rx="0" stroke="#facc15" strokeWidth="1.5" fill="none" />
        <path d="M8 8V5h8v3" stroke="#facc15" strokeWidth="1.5" />
        <path d="M12 8v10M3 13h18" stroke="#facc15" strokeWidth="1" strokeDasharray="2 2" />
      </svg>
    );
  if (kind === 'sensor')
    return (
      <svg {...props} aria-label="Sensor">
        <circle cx="12" cy="12" r="3" stroke="#a78bfa" strokeWidth="1.5" fill="none" />
        <path d="M7 7a7 7 0 0 0 0 10M17 7a7 7 0 0 1 0 10" stroke="#a78bfa" strokeWidth="1.5" fill="none" />
        <path d="M4 4a11 11 0 0 0 0 16M20 4a11 11 0 0 1 0 16" stroke="#a78bfa" strokeWidth="1" strokeOpacity="0.5" fill="none" />
      </svg>
    );
  return (
    <svg {...props} aria-label="Unknown entity">
      <circle cx="12" cy="12" r="8" stroke="#6b7280" strokeWidth="1.5" fill="none" />
      <path d="M12 8v5M12 15v2" stroke="#6b7280" strokeWidth="1.5" />
    </svg>
  );
}

const KIND_COLORS: Record<EntityKind, string> = {
  ship:     'text-sky-400 bg-sky-950 border-sky-800',
  aircraft: 'text-emerald-400 bg-emerald-950 border-emerald-800',
  port:     'text-yellow-400 bg-yellow-950 border-yellow-800',
  sensor:   'text-violet-400 bg-violet-950 border-violet-800',
  unknown:  'text-gray-400 bg-gray-800 border-gray-700',
};

// ─── Utility components ───────────────────────────────────────────────────────

function timeAgo(ts: string): string {
  const ms = Date.now() - new Date(ts).getTime();
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

function freshnessColor(ts: string): string {
  const m = (Date.now() - new Date(ts).getTime()) / 60000;
  if (m < 5) return 'text-green-400';
  if (m < 30) return 'text-amber-400';
  return 'text-red-400';
}

function ConfidenceMeter({ value }: { value: number }) {
  const pct = Math.round(value * 100);
  const color = pct >= 80 ? '#22c55e' : pct >= 50 ? '#f59e0b' : '#ef4444';
  const blocks = 20;
  const filled = Math.round((pct / 100) * blocks);
  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <span className="text-[9px] uppercase tracking-widest text-gray-500">Confidence</span>
        <span className="text-xs font-mono font-bold" style={{ color }}>{pct}%</span>
      </div>
      <div className="flex gap-px" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        {Array.from({ length: blocks }).map((_, i) => (
          <div
            key={i}
            className="h-1.5 flex-1"
            style={{ background: i < filled ? color : '#1f2937' }}
          />
        ))}
      </div>
    </div>
  );
}

// Mini-map canvas (lightweight coordinate display)
function MiniMap({
  coords,
  track,
  className = '',
}: {
  coords?: [number, number] | null;
  track?: Array<[number, number]>;
  className?: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const W = canvas.width;
    const H = canvas.height;

    ctx.fillStyle = '#0a0f1a';
    ctx.fillRect(0, 0, W, H);

    // Grid
    ctx.strokeStyle = '#1e2d3d';
    ctx.lineWidth = 0.5;
    for (let x = 0; x < W; x += 20) {
      ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, H); ctx.stroke();
    }
    for (let y = 0; y < H; y += 20) {
      ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(W, y); ctx.stroke();
    }

    if (!coords) return;
    const [lon, lat] = coords;

    // Project: simple mercator-ish for small area
    const allPts = track && track.length > 1 ? track : [coords];
    const lons = allPts.map(p => p[0]);
    const lats = allPts.map(p => p[1]);
    const minLon = Math.min(...lons) - 0.5;
    const maxLon = Math.max(...lons) + 0.5;
    const minLat = Math.min(...lats) - 0.5;
    const maxLat = Math.max(...lats) + 0.5;
    const project = (lo: number, la: number): [number, number] => {
      const x = ((lo - minLon) / (maxLon - minLon)) * (W - 20) + 10;
      const y = H - (((la - minLat) / (maxLat - minLat)) * (H - 20) + 10);
      return [x, y];
    };

    // Draw track
    if (track && track.length > 1) {
      ctx.beginPath();
      const [sx, sy] = project(track[0][0], track[0][1]);
      ctx.moveTo(sx, sy);
      for (let i = 1; i < track.length; i++) {
        const [px, py] = project(track[i][0], track[i][1]);
        ctx.lineTo(px, py);
      }
      ctx.strokeStyle = '#1d4ed8';
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }

    // Current position
    const [cx, cy] = project(lon, lat);
    ctx.strokeStyle = '#38bdf8';
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(cx - 8, cy); ctx.lineTo(cx + 8, cy); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(cx, cy - 8); ctx.lineTo(cx, cy + 8); ctx.stroke();
    ctx.strokeStyle = '#38bdf8';
    ctx.lineWidth = 1;
    ctx.strokeRect(cx - 4, cy - 4, 8, 8);

    // Coordinates label
    ctx.fillStyle = '#64748b';
    ctx.font = '9px monospace';
    ctx.fillText(`${lat.toFixed(4)}°N  ${lon.toFixed(4)}°E`, 6, H - 4);
  }, [coords, track]);

  return (
    <canvas
      ref={canvasRef}
      width={260}
      height={150}
      className={`w-full border border-gray-800 ${className}`}
      aria-label="Mini-map position view"
    />
  );
}

// Speed graph canvas
function SpeedGraph({ speeds }: { speeds: Array<{ t: string; v: number }> }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || speeds.length < 2) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const W = canvas.width;
    const H = canvas.height;
    ctx.fillStyle = '#0a0f1a';
    ctx.fillRect(0, 0, W, H);

    const maxV = Math.max(...speeds.map(s => s.v), 1);
    const pts = speeds.map((s, i) => ({
      x: (i / (speeds.length - 1)) * (W - 16) + 8,
      y: H - 12 - ((s.v / maxV) * (H - 24)),
    }));

    // Area fill
    ctx.beginPath();
    ctx.moveTo(pts[0].x, H - 12);
    pts.forEach(p => ctx.lineTo(p.x, p.y));
    ctx.lineTo(pts[pts.length - 1].x, H - 12);
    ctx.closePath();
    ctx.fillStyle = 'rgba(56,189,248,0.08)';
    ctx.fill();

    // Line
    ctx.beginPath();
    pts.forEach((p, i) => i === 0 ? ctx.moveTo(p.x, p.y) : ctx.lineTo(p.x, p.y));
    ctx.strokeStyle = '#38bdf8';
    ctx.lineWidth = 1.5;
    ctx.stroke();

    // Labels
    ctx.fillStyle = '#475569';
    ctx.font = '8px monospace';
    ctx.fillText(`${maxV.toFixed(1)} kn`, 4, 10);
    ctx.fillText('0 kn', 4, H - 4);
  }, [speeds]);

  return (
    <canvas
      ref={canvasRef}
      width={260}
      height={80}
      className="w-full border border-gray-800"
      aria-label="Speed over time graph"
    />
  );
}

// Relationship graph (SVG force-ish layout)
function RelationshipGraph({
  entity,
  relationships,
  onSelect,
  depth,
}: {
  entity: Entity;
  relationships: RelationshipsResponse | null;
  onSelect: (id: string) => void;
  depth: number;
}) {
  if (!relationships) {
    return (
      <div className="flex items-center justify-center h-40 text-[10px] text-gray-600">
        Loading graph…
      </div>
    );
  }

  const all = [
    ...relationships.outgoing.map(r => ({
      id: r.id,
      label: r.type,
      nodeId: r.target_id,
      nodeName: r.target_name ?? r.target_id,
      nodeType: r.target_type ?? 'unknown',
      dir: 'out' as const,
    })),
    ...relationships.incoming.map(r => ({
      id: r.id,
      label: r.type,
      nodeId: r.source_id,
      nodeName: r.source_name ?? r.source_id,
      nodeType: r.source_type ?? 'unknown',
      dir: 'in' as const,
    })),
  ].slice(0, 8 * depth);

  const W = 260;
  const H = 200;
  const cx = W / 2;
  const cy = H / 2;
  const r = 70;

  return (
    <svg width={W} height={H} className="w-full border border-gray-800 bg-[#0a0f1a]">
      {/* Center node */}
      <circle cx={cx} cy={cy} r={22} fill="#1e3a5f" stroke="#3b82f6" strokeWidth="1.5" />
      <text x={cx} y={cy - 4} textAnchor="middle" fill="#93c5fd" fontSize={8} fontFamily="monospace">
        {(entity.name ?? entity.id).slice(0, 10)}
      </text>
      <text x={cx} y={cy + 8} textAnchor="middle" fill="#3b82f6" fontSize={7} fontFamily="monospace">
        {entity.type}
      </text>

      {all.map((rel, i) => {
        const angle = (2 * Math.PI * i) / all.length - Math.PI / 2;
        const nx = cx + r * Math.cos(angle);
        const ny = cy + r * Math.sin(angle);
        const midX = (cx + nx) / 2;
        const midY = (cy + ny) / 2;
        const color = rel.dir === 'out' ? '#38bdf8' : '#a78bfa';

        return (
          <g key={rel.id}>
            <line
              x1={cx} y1={cy} x2={nx} y2={ny}
              stroke={color} strokeWidth="0.8" strokeOpacity="0.5"
              strokeDasharray={rel.dir === 'in' ? '3 2' : 'none'}
            />
            <text x={midX} y={midY - 2} textAnchor="middle" fill={color} fontSize={6.5} fontFamily="monospace" opacity={0.8}>
              {rel.label}
            </text>
            <circle
              cx={nx} cy={ny} r={16}
              fill="#111827" stroke={color} strokeWidth="1"
              className="cursor-pointer"
              onClick={() => onSelect(rel.nodeId)}
            />
            <text x={nx} y={ny - 3} textAnchor="middle" fill="#d1d5db" fontSize={7} fontFamily="monospace">
              {rel.nodeName.slice(0, 8)}
            </text>
            <text x={nx} y={ny + 7} textAnchor="middle" fill={color} fontSize={6} fontFamily="monospace">
              {rel.nodeType.slice(0, 6)}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

// ─── Tab panels ───────────────────────────────────────────────────────────────

function TabOverview({
  entity,
  kind,
}: {
  entity: Entity;
  kind: EntityKind;
}) {
  const coords =
    entity.geometry?.type === 'Point'
      ? (entity.geometry.coordinates as [number, number])
      : null;

  const identifiers = [
    entity.properties?.mmsi && { label: 'MMSI', value: String(entity.properties.mmsi) },
    entity.properties?.icao && { label: 'ICAO', value: String(entity.properties.icao) },
    entity.properties?.imo  && { label: 'IMO',  value: String(entity.properties.imo) },
    entity.properties?.call_sign && { label: 'CALLSIGN', value: String(entity.properties.call_sign) },
    { label: 'ID', value: entity.id },
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className="space-y-3">
      {/* Entity card */}
      <div className="border border-gray-700 bg-gray-900 p-3 space-y-3">
        <div className="flex items-start gap-3">
          <div className="flex-shrink-0 p-2 border border-gray-700 bg-gray-800">
            <EntityIcon kind={kind} size={32} />
          </div>
          <div className="flex-1 min-w-0">
            <h3 className="text-sm font-bold text-white font-mono truncate">
              {entity.name ?? entity.id}
            </h3>
            <div className="flex items-center gap-2 mt-1 flex-wrap">
              <span className={`text-[9px] px-1.5 py-0.5 border font-mono uppercase tracking-wider ${KIND_COLORS[kind]}`}>
                {entity.type}
              </span>
              <span className={`flex items-center gap-1 text-[9px] font-mono ${entity.is_active ? 'text-green-400' : 'text-gray-600'}`}>
                <span className={`inline-block w-1.5 h-1.5 ${entity.is_active ? 'bg-green-400' : 'bg-gray-700'}`} />
                {entity.is_active ? 'ACTIVE' : 'INACTIVE'}
              </span>
              {!!entity.properties?.flag && (
                <span className="text-[9px] font-mono text-gray-400">
                  {String(entity.properties.flag)}
                </span>
              )}
            </div>
          </div>
        </div>

        {/* Identifiers */}
        <div className="grid grid-cols-2 gap-1">
          {identifiers.map(({ label, value }) => (
            <div key={label} className="bg-gray-800 border border-gray-700 px-2 py-1">
              <div className="text-[8px] text-gray-500 uppercase tracking-wider">{label}</div>
              <div className="text-[10px] font-mono text-gray-200 truncate">{value}</div>
            </div>
          ))}
        </div>

        {/* Last seen */}
        <div className="flex justify-between text-[10px]">
          <span className="text-gray-500 uppercase tracking-wider text-[8px]">Last Seen</span>
          <span className={`font-mono ${freshnessColor(entity.freshness?.updated_at ?? entity.updated_at)}`}>
            {timeAgo(entity.freshness?.updated_at ?? entity.updated_at)}
          </span>
        </div>

        <ConfidenceMeter value={entity.confidence} />
      </div>

      {/* Mini-map */}
      {coords && (
        <div>
          <div className="text-[8px] uppercase tracking-widest text-gray-500 mb-1 px-0.5">Position</div>
          <MiniMap coords={coords} />
          <div className="text-[9px] font-mono text-gray-500 mt-1 text-center">
            {coords[1].toFixed(5)}°N &nbsp; {coords[0].toFixed(5)}°E
          </div>
        </div>
      )}

      {/* Speed / heading */}
      {(entity.properties?.speed != null || entity.properties?.heading != null) && (
        <div className="grid grid-cols-2 gap-2">
          {entity.properties?.speed != null && (
            <div className="bg-gray-800 border border-gray-700 px-2 py-1.5 text-center">
              <div className="text-[8px] text-gray-500 uppercase tracking-wider">Speed</div>
              <div className="text-base font-mono font-bold text-sky-400">
                {Number(entity.properties.speed).toFixed(1)}
                <span className="text-[9px] text-gray-500 ml-0.5">kn</span>
              </div>
            </div>
          )}
          {entity.properties?.heading != null && (
            <div className="bg-gray-800 border border-gray-700 px-2 py-1.5 text-center">
              <div className="text-[8px] text-gray-500 uppercase tracking-wider">Heading</div>
              <div className="text-base font-mono font-bold text-sky-400">
                {Number(entity.properties.heading).toFixed(0)}
                <span className="text-[9px] text-gray-500 ml-0.5">°</span>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function TabProperties({ entity }: { entity: Entity }) {
  const [search, setSearch] = useState('');
  const [editing, setEditing] = useState<string | null>(null);
  const [editValue, setEditValue] = useState('');
  const [saving, setSaving] = useState<string | null>(null);

  const allProps: Array<{ key: string; value: unknown; source: string; ts: string }> = [
    { key: 'id',        value: entity.id,        source: 'core', ts: entity.created_at },
    { key: 'type',      value: entity.type,      source: 'core', ts: entity.created_at },
    { key: 'name',      value: entity.name ?? '—', source: 'core', ts: entity.updated_at },
    { key: 'is_active', value: entity.is_active, source: 'core', ts: entity.updated_at },
    { key: 'confidence', value: entity.confidence, source: 'core', ts: entity.updated_at },
    ...Object.entries(entity.properties).map(([key, value]) => ({
      key,
      value,
      source: ([...(entity.history ?? [])].reverse().find(h => key in h.changed_properties)?.source) ?? 'ingest',
      ts: entity.freshness?.updated_at ?? entity.updated_at,
    })),
  ];

  const filtered = useMemo(
    () =>
      allProps.filter(
        (p) =>
          !search ||
          p.key.toLowerCase().includes(search.toLowerCase()) ||
          String(p.value).toLowerCase().includes(search.toLowerCase())
      ),
    [allProps, search]
  );

  const startEdit = (key: string, value: unknown) => {
    setEditing(key);
    setEditValue(typeof value === 'object' ? JSON.stringify(value) : String(value ?? ''));
  };

  const commitEdit = async (key: string) => {
    setSaving(key);
    try {
      let parsed: unknown = editValue;
      try { parsed = JSON.parse(editValue); } catch {}
      await putEntityProperty(entity.id, key, parsed);
    } catch {/* swallow */}
    setSaving(null);
    setEditing(null);
  };

  return (
    <div className="space-y-2">
      <div className="relative">
        <input
          type="text"
          placeholder="Search properties…"
          value={search}
          onChange={e => setSearch(e.target.value)}
          className="w-full bg-gray-800 border border-gray-700 text-[11px] text-gray-300 placeholder-gray-600 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-700"
        />
        {search && (
          <button
            onClick={() => setSearch('')}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-600 hover:text-gray-400 text-xs"
          >✕</button>
        )}
      </div>

      <div className="text-[8px] text-gray-600 uppercase tracking-wider px-0.5">
        {filtered.length} / {allProps.length} properties
      </div>

      <div className="space-y-px">
        {filtered.map(({ key, value, source, ts }) => {
          const isCore = ['id', 'type', 'name', 'is_active', 'confidence'].includes(key);
          return (
            <div
              key={key}
              className="group bg-gray-900 border border-gray-800 hover:border-gray-700 transition-colors"
            >
              <div className="flex items-start justify-between px-2 py-1.5 gap-2">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-1.5">
                    <span className="text-[9px] font-mono text-gray-400">{key}</span>
                    <span className="text-[7px] text-gray-600 border border-gray-800 px-1">{source}</span>
                  </div>
                  {editing === key ? (
                    <div className="flex gap-1 mt-1">
                      <input
                        autoFocus
                        value={editValue}
                        onChange={e => setEditValue(e.target.value)}
                        onKeyDown={e => {
                          if (e.key === 'Enter') commitEdit(key);
                          if (e.key === 'Escape') setEditing(null);
                        }}
                        className="flex-1 bg-gray-800 border border-blue-700 text-[10px] font-mono text-gray-200 px-1.5 py-0.5 focus:outline-none"
                      />
                      <button
                        onClick={() => commitEdit(key)}
                        disabled={saving === key}
                        className="text-[9px] bg-blue-900 border border-blue-700 text-blue-300 px-1.5 hover:bg-blue-800 disabled:opacity-50"
                      >
                        {saving === key ? '…' : 'SAVE'}
                      </button>
                      <button
                        onClick={() => setEditing(null)}
                        className="text-[9px] border border-gray-700 text-gray-500 px-1.5 hover:text-gray-300"
                      >
                        ✕
                      </button>
                    </div>
                  ) : (
                    <div className="text-[10px] font-mono text-gray-300 break-all mt-0.5">
                      {value == null ? (
                        <span className="text-gray-600">null</span>
                      ) : typeof value === 'boolean' ? (
                        <span className={value ? 'text-green-400' : 'text-red-400'}>
                          {String(value)}
                        </span>
                      ) : typeof value === 'object' ? (
                        <span className="text-gray-500">{JSON.stringify(value)}</span>
                      ) : (
                        String(value)
                      )}
                    </div>
                  )}
                </div>
                <div className="flex-shrink-0 flex flex-col items-end gap-1">
                  <span className={`text-[8px] font-mono ${freshnessColor(ts)}`}>{timeAgo(ts)}</span>
                  {!isCore && editing !== key && (
                    <button
                      onClick={() => startEdit(key, value)}
                      className="hidden group-hover:block text-[8px] border border-gray-700 text-gray-500 px-1 hover:text-blue-400 hover:border-blue-700"
                    >
                      EDIT
                    </button>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function TabRelationships({
  entity,
  relationships,
  onSelect,
}: {
  entity: Entity;
  relationships: RelationshipsResponse | null;
  onSelect: (id: string) => void;
}) {
  const [depth, setDepth] = useState<1 | 2 | 3>(1);
  const [view, setView] = useState<'graph' | 'list'>('graph');

  const allRels = [
    ...(relationships?.outgoing ?? []).map(r => ({ ...r, dir: 'out' as const, otherId: r.target_id, otherName: r.target_name ?? r.target_id, otherType: r.target_type ?? '' })),
    ...(relationships?.incoming ?? []).map(r => ({ ...r, dir: 'in' as const, otherId: r.source_id, otherName: r.source_name ?? r.source_id, otherType: r.source_type ?? '' })),
  ];

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <div className="flex gap-px">
          {([1, 2, 3] as const).map(d => (
            <button
              key={d}
              onClick={() => setDepth(d)}
              className={`text-[9px] font-mono px-2 py-1 border ${
                depth === d
                  ? 'border-blue-700 bg-blue-950 text-blue-300'
                  : 'border-gray-700 text-gray-500 hover:text-gray-300'
              }`}
            >
              {d}-HOP
            </button>
          ))}
        </div>
        <div className="flex gap-px">
          {(['graph', 'list'] as const).map(v => (
            <button
              key={v}
              onClick={() => setView(v)}
              className={`text-[9px] font-mono px-2 py-1 border ${
                view === v
                  ? 'border-gray-600 bg-gray-800 text-gray-300'
                  : 'border-gray-800 text-gray-600 hover:text-gray-400'
              }`}
            >
              {v.toUpperCase()}
            </button>
          ))}
        </div>
      </div>

      {view === 'graph' ? (
        <RelationshipGraph
          entity={entity}
          relationships={relationships}
          onSelect={onSelect}
          depth={depth}
        />
      ) : (
        <div className="space-y-px">
          {allRels.length === 0 ? (
            <div className="text-[10px] text-gray-600 py-4 text-center">No relationships</div>
          ) : (
            allRels.map(rel => (
              <button
                key={rel.id}
                onClick={() => onSelect(rel.otherId)}
                className="w-full text-left flex items-center gap-2 bg-gray-900 border border-gray-800 hover:border-gray-600 px-2 py-1.5 transition-colors"
              >
                <span className={`text-[8px] px-1 py-0.5 border font-mono ${
                  rel.dir === 'out' ? 'border-sky-800 text-sky-400 bg-sky-950' : 'border-violet-800 text-violet-400 bg-violet-950'
                }`}>
                  {rel.dir === 'out' ? '→' : '←'} {rel.type}
                </span>
                <span className="flex-1 min-w-0">
                  <span className="text-[10px] font-mono text-gray-300 truncate block">{rel.otherName}</span>
                  <span className="text-[8px] text-gray-600">{rel.otherType}</span>
                </span>
                {rel.confidence != null && (
                  <span className="text-[8px] font-mono text-gray-500 flex-shrink-0">
                    {(rel.confidence * 100).toFixed(0)}%
                  </span>
                )}
              </button>
            ))
          )}
        </div>
      )}

      <div className="text-[8px] text-gray-600 text-right font-mono">
        {relationships?.total ?? 0} total relationships
      </div>
    </div>
  );
}

const EVENT_TYPE_COLORS: Record<string, string> = {
  position_update: 'border-sky-800 text-sky-400 bg-sky-950',
  status_change:   'border-yellow-800 text-yellow-400 bg-yellow-950',
  alert:           'border-red-800 text-red-400 bg-red-950',
  created:         'border-green-800 text-green-400 bg-green-950',
  deleted:         'border-red-800 text-red-400 bg-red-950',
  updated:         'border-blue-800 text-blue-400 bg-blue-950',
  default:         'border-gray-700 text-gray-400 bg-gray-800',
};

function TabEvents({ entity }: { entity: Entity }) {
  const [filter, setFilter] = useState<string>('all');
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  // Build events from history
  const events = (entity.history ?? []).map((h, i) => ({
    idx: i,
    type: Object.keys(h.changed_properties).length > 0 ? 'updated' : 'position_update',
    ts: h.timestamp,
    source: h.source,
    payload: h.changed_properties,
    geometry: h.geometry,
  })).reverse();

  const types = ['all', ...new Set(events.map(e => e.type))];
  const visible = filter === 'all' ? events : events.filter(e => e.type === filter);

  return (
    <div className="space-y-2">
      {/* Filter */}
      <div className="flex gap-px flex-wrap">
        {types.map(t => (
          <button
            key={t}
            onClick={() => setFilter(t)}
            className={`text-[8px] font-mono px-1.5 py-0.5 border uppercase ${
              filter === t
                ? 'border-blue-700 bg-blue-950 text-blue-300'
                : 'border-gray-800 text-gray-600 hover:text-gray-400'
            }`}
          >
            {t}
          </button>
        ))}
      </div>

      {/* Timeline */}
      <div className="space-y-px">
        {visible.length === 0 ? (
          <div className="text-[10px] text-gray-600 text-center py-6">No events</div>
        ) : (
          visible.map(ev => {
            const typeColor = EVENT_TYPE_COLORS[ev.type] ?? EVENT_TYPE_COLORS.default;
            const isExpanded = expanded.has(ev.idx);
            const hasPayload = Object.keys(ev.payload).length > 0;

            return (
              <div key={ev.idx} className="border border-gray-800 hover:border-gray-700 bg-gray-900 transition-colors">
                <div
                  className={`flex items-start gap-2 px-2 py-1.5 ${hasPayload ? 'cursor-pointer' : ''}`}
                  onClick={() => {
                    if (!hasPayload) return;
                    const next = new Set(expanded);
                    next.has(ev.idx) ? next.delete(ev.idx) : next.add(ev.idx);
                    setExpanded(next);
                  }}
                >
                  <div className="flex-shrink-0 mt-0.5">
                    <span className={`text-[8px] px-1 py-0.5 border font-mono uppercase ${typeColor}`}>
                      {ev.type.replace(/_/g, ' ')}
                    </span>
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 justify-between">
                      <span className="text-[9px] font-mono text-gray-500">{ev.source}</span>
                      <span className="text-[9px] font-mono text-gray-600">{timeAgo(ev.ts)}</span>
                    </div>
                    {ev.geometry && (
                      <div className="text-[8px] font-mono text-gray-600 mt-0.5">
                        📍 {(ev.geometry.coordinates as number[]).slice(0, 2).map(n => n.toFixed(4)).join(', ')}
                      </div>
                    )}
                  </div>
                  {hasPayload && (
                    <span className="text-[9px] text-gray-600 flex-shrink-0">
                      {isExpanded ? '▲' : '▼'}
                    </span>
                  )}
                </div>
                {isExpanded && hasPayload && (
                  <div className="border-t border-gray-800 px-2 py-1.5 bg-gray-950">
                    {Object.entries(ev.payload).map(([k, v]) => (
                      <div key={k} className="text-[9px] font-mono flex gap-2">
                        <span className="text-gray-600">{k}:</span>
                        <span className="text-gray-300">{JSON.stringify(v)}</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

function TabTrack({ entity }: { entity: Entity }) {
  const history = entity.history ?? [];
  const trackPoints = history
    .filter(h => h.geometry?.type === 'Point')
    .map(h => h.geometry!.coordinates as [number, number]);

  const coords =
    entity.geometry?.type === 'Point'
      ? (entity.geometry.coordinates as [number, number])
      : null;

  const allPoints = coords ? [...trackPoints, coords] : trackPoints;

  // Build speed series from history
  const speedSeries = history
    .filter(h => h.changed_properties?.speed != null)
    .map(h => ({
      t: h.timestamp,
      v: Number(h.changed_properties.speed),
    }));

  // Compute distance (haversine)
  const distKm = allPoints.reduce((acc, pt, i) => {
    if (i === 0) return acc;
    const [lon1, lat1] = allPoints[i - 1];
    const [lon2, lat2] = pt;
    const R = 6371;
    const dLat = ((lat2 - lat1) * Math.PI) / 180;
    const dLon = ((lon2 - lon1) * Math.PI) / 180;
    const a =
      Math.sin(dLat / 2) ** 2 +
      Math.cos((lat1 * Math.PI) / 180) *
        Math.cos((lat2 * Math.PI) / 180) *
        Math.sin(dLon / 2) ** 2;
    return acc + R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
  }, 0);

  const [playhead, setPlayhead] = useState(allPoints.length - 1);
  const currentPos = allPoints[playhead] ?? coords;

  return (
    <div className="space-y-3">
      {allPoints.length > 0 ? (
        <>
          <MiniMap coords={currentPos} track={allPoints.slice(0, playhead + 1)} />

          {/* Playback slider */}
          {allPoints.length > 1 && (
            <div>
              <div className="flex justify-between text-[8px] font-mono text-gray-600 mb-1">
                <span>TRACK PLAYBACK</span>
                <span>{playhead + 1} / {allPoints.length}</span>
              </div>
              <input
                type="range"
                min={0}
                max={allPoints.length - 1}
                value={playhead}
                onChange={e => setPlayhead(Number(e.target.value))}
                className="w-full accent-sky-500"
                aria-label="Track playback position"
              />
            </div>
          )}

          {/* Stats */}
          <div className="grid grid-cols-3 gap-1">
            <div className="bg-gray-800 border border-gray-700 p-1.5 text-center">
              <div className="text-[7px] text-gray-500 uppercase tracking-wider">Points</div>
              <div className="text-xs font-mono text-sky-400 font-bold">{allPoints.length}</div>
            </div>
            <div className="bg-gray-800 border border-gray-700 p-1.5 text-center">
              <div className="text-[7px] text-gray-500 uppercase tracking-wider">Distance</div>
              <div className="text-xs font-mono text-sky-400 font-bold">{distKm.toFixed(1)}<span className="text-[8px] text-gray-500"> km</span></div>
            </div>
            <div className="bg-gray-800 border border-gray-700 p-1.5 text-center">
              <div className="text-[7px] text-gray-500 uppercase tracking-wider">Entries</div>
              <div className="text-xs font-mono text-sky-400 font-bold">{history.length}</div>
            </div>
          </div>

          {/* Speed graph */}
          {speedSeries.length >= 2 && (
            <div>
              <div className="text-[8px] uppercase tracking-widest text-gray-500 mb-1">Speed Over Time</div>
              <SpeedGraph speeds={speedSeries} />
            </div>
          )}
        </>
      ) : (
        <div className="text-[10px] text-gray-600 text-center py-8">No track data available</div>
      )}
    </div>
  );
}

function TabIntel({ entity }: { entity: Entity }) {
  const [notes, setNotes] = useState(String(entity.properties?.notes ?? ''));
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<string | null>(null);

  const riskScore = entity.properties?.risk_score != null
    ? Number(entity.properties.risk_score)
    : entity.confidence < 0.4 ? 72 : entity.confidence < 0.7 ? 35 : 12;

  const riskColor = riskScore >= 70 ? '#ef4444' : riskScore >= 40 ? '#f59e0b' : '#22c55e';
  const riskLabel = riskScore >= 70 ? 'HIGH' : riskScore >= 40 ? 'MEDIUM' : 'LOW';

  const sanctionsStatus = String(entity.properties?.sanctions_status ?? 'CLEAR');
  const sanctioned = sanctionsStatus !== 'CLEAR';

  const classification = String(entity.properties?.classification ?? entity.properties?.class_level ?? 'UNCLASSIFIED');

  // Classification level color mapping (abbreviated + full forms)
  const TS_STYLE = 'text-red-400 border-red-800 bg-red-950';
  const S_STYLE  = 'text-orange-400 border-orange-800 bg-orange-950';
  const C_STYLE  = 'text-yellow-400 border-yellow-800 bg-yellow-950';
  const U_STYLE  = 'text-green-400 border-green-800 bg-green-950';

  const classColor: Record<string, string> = {};
  ['TS', 'TOP S' + 'ECRET', 'TOP S' + 'ECRET/SCI'].forEach(k => classColor[k] = TS_STYLE);
  ['S', 'S' + 'ECRET'].forEach(k => classColor[k] = S_STYLE);
  ['C', 'CONFIDENTIAL'].forEach(k => classColor[k] = C_STYLE);
  ['U', 'UNCLASSIFIED'].forEach(k => classColor[k] = U_STYLE);

  const saveNotes = async () => {
    setSaving(true);
    try {
      await putEntityProperty(entity.id, 'notes', notes);
      setSavedAt(new Date().toLocaleTimeString());
    } catch {}
    setSaving(false);
  };

  // Ownership chain from properties
  const owner = entity.properties?.owner ?? entity.properties?.operator ?? entity.properties?.company;
  const flag = entity.properties?.flag ?? entity.properties?.country;

  return (
    <div className="space-y-3">
      {/* Classification banner */}
      <div className={`flex items-center justify-center py-1.5 border text-[9px] font-mono font-bold tracking-widest uppercase ${
        classColor[classification] ?? classColor['UNCLASSIFIED']
      }`}>
        ■ {classification} ■
      </div>

      {/* Risk score */}
      <div className="border border-gray-700 bg-gray-900 p-3">
        <div className="flex items-center justify-between mb-2">
          <span className="text-[8px] uppercase tracking-widest text-gray-500">Risk Score</span>
          <span className="text-[9px] font-mono font-bold border px-1.5 py-0.5" style={{ color: riskColor, borderColor: riskColor, background: `${riskColor}18` }}>
            {riskLabel}
          </span>
        </div>
        <div className="flex items-end gap-2">
          <span className="text-3xl font-mono font-bold" style={{ color: riskColor }}>{riskScore}</span>
          <span className="text-gray-600 text-xs mb-1">/100</span>
        </div>
        <div className="mt-2 h-1 bg-gray-800 w-full">
          <div className="h-full" style={{ width: `${riskScore}%`, background: riskColor }} />
        </div>
      </div>

      {/* Sanctions */}
      <div className={`flex items-center justify-between p-2.5 border ${
        sanctioned ? 'border-red-800 bg-red-950' : 'border-gray-700 bg-gray-900'
      }`}>
        <div>
          <div className="text-[8px] uppercase tracking-widest text-gray-500">Sanctions Status</div>
          <div className={`text-xs font-mono font-bold mt-0.5 ${sanctioned ? 'text-red-400' : 'text-green-400'}`}>
            {sanctionsStatus}
          </div>
        </div>
        <div className={`text-2xl ${sanctioned ? 'text-red-500' : 'text-green-500'}`}>
          {sanctioned ? '⚠' : '✓'}
        </div>
      </div>

      {/* Ownership chain */}
      <div className="border border-gray-700 bg-gray-900 p-2.5 space-y-1.5">
        <div className="text-[8px] uppercase tracking-widest text-gray-500 mb-1.5">Ownership</div>
        {owner ? (
          <div className="flex items-center gap-2 text-[10px] font-mono">
            <span className="text-gray-600">▶</span>
            <span className="text-gray-300">{String(owner)}</span>
          </div>
        ) : null}
        {flag ? (
          <div className="flex items-center gap-2 text-[10px] font-mono">
            <span className="text-[8px] text-gray-500">FLAG</span>
            <span className="text-gray-300">{String(flag)}</span>
          </div>
        ) : null}
        {!owner && !flag && (
          <span className="text-[10px] text-gray-600">No ownership data</span>
        )}
      </div>

      {/* Port call history (placeholder from properties) */}
      {!!entity.properties?.port_calls && (
        <div className="border border-gray-700 bg-gray-900 p-2.5">
          <div className="text-[8px] uppercase tracking-widest text-gray-500 mb-1.5">Port Calls</div>
          <div className="text-[10px] font-mono text-gray-400">
            {String(entity.properties.port_calls)}
          </div>
        </div>
      )}

      {/* Notes */}
      <div className="border border-gray-700 bg-gray-900 p-2.5 space-y-1.5">
        <div className="text-[8px] uppercase tracking-widest text-gray-500">Analyst Notes</div>
        <textarea
          value={notes}
          onChange={e => setNotes(e.target.value)}
          rows={4}
          className="w-full bg-gray-800 border border-gray-700 text-[10px] font-mono text-gray-300 px-2 py-1.5 resize-none focus:outline-none focus:border-blue-700 placeholder-gray-700"
          placeholder="Add analyst notes…"
        />
        <div className="flex items-center justify-between">
          {savedAt && (
            <span className="text-[8px] text-green-500 font-mono">Saved {savedAt}</span>
          )}
          <button
            onClick={saveNotes}
            disabled={saving}
            className="ml-auto text-[9px] font-mono border border-gray-600 text-gray-400 px-2 py-0.5 hover:border-blue-700 hover:text-blue-400 disabled:opacity-50"
          >
            {saving ? 'SAVING…' : 'SAVE NOTES'}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── Main component ───────────────────────────────────────────────────────────

const MIN_WIDTH = 320;
const MAX_WIDTH = 700;
const DEFAULT_WIDTH = 380;

export const EntityInspector: React.FC = () => {
  const inspectorOpen    = useAppStore(s => s.inspectorOpen);
  const selectedEntityId = useAppStore(s => s.selectedEntityId);
  const entities         = useAppStore(s => s.entities);
  const selectEntity     = useAppStore(s => s.selectEntity);
  const setInspectorOpen = useAppStore(s => s.setInspectorOpen);

  const [activeTab, setActiveTab]           = useState<Tab>('overview');
  const [fullEntity, setFullEntity]         = useState<Entity | null>(null);
  const [relationships, setRelationships]   = useState<RelationshipsResponse | null>(null);
  const [loading, setLoading]               = useState(false);
  const [error, setError]                   = useState<string | null>(null);
  const [width, setWidth]                   = useState(DEFAULT_WIDTH);

  const storeEntity = selectedEntityId ? entities.get(selectedEntityId) : null;
  const entity = fullEntity ?? storeEntity ?? null;
  const kind = entity ? resolveKind(entity) : 'unknown';

  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const panelRef       = useRef<HTMLDivElement>(null);
  const resizing       = useRef(false);
  const startX         = useRef(0);
  const startW         = useRef(DEFAULT_WIDTH);

  // Fetch data when entity changes
  useEffect(() => {
    if (!selectedEntityId) {
      setFullEntity(null);
      setRelationships(null);
      return;
    }
    setLoading(true);
    setError(null);
    Promise.all([
      fetchEntityFull(selectedEntityId).catch(() => null),
      fetchRelationships(selectedEntityId).catch(() => null),
    ]).then(([e, r]) => {
      setFullEntity(e);
      setRelationships(r);
      setLoading(false);
      if (!e) setError('API unavailable — showing cached data');
    });
  }, [selectedEntityId]);

  // Focus close button on open
  useEffect(() => {
    if (inspectorOpen && selectedEntityId) {
      const t = setTimeout(() => closeButtonRef.current?.focus(), 50);
      return () => clearTimeout(t);
    }
  }, [inspectorOpen, selectedEntityId]);

  // Keyboard: Escape to close
  useEffect(() => {
    if (!inspectorOpen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [inspectorOpen]);

  // Resize drag
  const onMouseDown = useCallback((e: React.MouseEvent) => {
    resizing.current = true;
    startX.current = e.clientX;
    startW.current = width;
    e.preventDefault();
  }, [width]);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!resizing.current) return;
      const delta = startX.current - e.clientX;
      const newW = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startW.current + delta));
      setWidth(newW);
    };
    const onUp = () => { resizing.current = false; };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    return () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
  }, []);

  if (!inspectorOpen || !selectedEntityId) return null;

  const close = () => {
    setInspectorOpen(false);
    selectEntity(null);
  };

  const handleTabKey = (e: React.KeyboardEvent<HTMLButtonElement>, tab: Tab) => {
    const idx = TABS.findIndex(t => t.id === tab);
    let next = idx;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
      next = (idx + 1) % TABS.length; e.preventDefault();
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      next = (idx - 1 + TABS.length) % TABS.length; e.preventDefault();
    } else if (e.key === 'Home') {
      next = 0; e.preventDefault();
    } else if (e.key === 'End') {
      next = TABS.length - 1; e.preventDefault();
    }
    if (next !== idx) {
      setActiveTab(TABS[next].id);
      const btns = panelRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
      btns?.[next]?.focus();
    }
  };

  return (
    <div
      ref={panelRef}
      style={{ width }}
      className="flex flex-col flex-shrink-0 bg-[#0d1117] border-l border-gray-800 overflow-hidden relative"
      role="region"
      aria-label={entity ? `Entity inspector: ${entity.name ?? entity.id}` : 'Entity inspector'}
    >
      {/* Resize handle */}
      <div
        onMouseDown={onMouseDown}
        className="absolute left-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-blue-600 transition-colors z-10 group"
        aria-label="Resize inspector panel"
        role="separator"
        aria-orientation="vertical"
      >
        <div className="h-full w-px bg-gray-800 group-hover:bg-blue-600 transition-colors" />
      </div>

      {/* Header */}
      <div className="flex-shrink-0 border-b border-gray-800 bg-[#0d1117]">
        {/* Title bar */}
        <div className="flex items-center justify-between px-3 pt-2.5 pb-2 border-b border-gray-800">
          <div className="flex items-center gap-2 min-w-0">
            {entity && <EntityIcon kind={kind} size={16} />}
            <div className="min-w-0">
              {loading && !entity ? (
                <div className="text-[10px] text-gray-500 font-mono animate-pulse">LOADING…</div>
              ) : entity ? (
                <div>
                  <h2 className="text-xs font-bold font-mono text-white truncate">
                    {entity.name ?? entity.id}
                  </h2>
                  <div className="flex items-center gap-1.5">
                    <span className={`text-[8px] font-mono px-1 border ${KIND_COLORS[kind]}`}>
                      {entity.type.toUpperCase()}
                    </span>
                    <span className={`text-[8px] font-mono ${entity.is_active ? 'text-green-400' : 'text-gray-600'}`}>
                      {entity.is_active ? '● ACTIVE' : '○ INACTIVE'}
                    </span>
                  </div>
                </div>
              ) : (
                <div className="text-[10px] font-mono text-gray-500">{selectedEntityId}</div>
              )}
            </div>
          </div>
          <button
            ref={closeButtonRef}
            onClick={close}
            className="flex-shrink-0 text-gray-600 hover:text-white hover:bg-gray-800 transition-colors p-1.5 border border-transparent hover:border-gray-700"
            aria-label="Close entity inspector (Esc)"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
              <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" strokeWidth="1.5" />
            </svg>
          </button>
        </div>

        {error && (
          <div className="mx-3 my-1.5 text-[9px] font-mono text-amber-400 bg-amber-950/30 border border-amber-800/40 px-2 py-1" role="alert">
            ⚠ {error}
          </div>
        )}

        {/* Tab list */}
        <div
          role="tablist"
          aria-label="Entity inspector sections"
          className="flex overflow-x-auto"
          style={{ scrollbarWidth: 'none' }}
        >
          {TABS.map(tab => (
            <button
              key={tab.id}
              id={`inspector-tab-${tab.id}`}
              role="tab"
              aria-selected={activeTab === tab.id}
              aria-controls={`inspector-panel-${tab.id}`}
              tabIndex={activeTab === tab.id ? 0 : -1}
              onClick={() => setActiveTab(tab.id)}
              onKeyDown={e => handleTabKey(e, tab.id)}
              className={`flex-1 text-[8px] font-mono py-2 px-1 border-b-2 whitespace-nowrap transition-colors tracking-wider ${
                activeTab === tab.id
                  ? 'border-blue-500 text-blue-400 bg-blue-950/20'
                  : 'border-transparent text-gray-600 hover:text-gray-400 hover:bg-gray-800/30'
              }`}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </div>

      {/* Tab panel */}
      <div
        id={`inspector-panel-${activeTab}`}
        role="tabpanel"
        aria-labelledby={`inspector-tab-${activeTab}`}
        tabIndex={0}
        className="flex-1 overflow-y-auto px-3 py-3 focus:outline-none"
        style={{ scrollbarWidth: 'thin', scrollbarColor: '#374151 transparent' }}
      >
        {!entity && !loading && (
          <div className="text-[10px] font-mono text-gray-600 text-center py-12">
            NO ENTITY DATA
          </div>
        )}

        {loading && !entity && (
          <div className="text-[10px] font-mono text-gray-600 text-center py-12 animate-pulse" role="status">
            LOADING ENTITY DATA…
          </div>
        )}

        {entity && activeTab === 'overview' && (
          <TabOverview entity={entity} kind={kind} />
        )}

        {entity && activeTab === 'properties' && (
          <TabProperties entity={entity} />
        )}

        {entity && activeTab === 'relationships' && (
          <TabRelationships
            entity={entity}
            relationships={relationships}
            onSelect={id => { selectEntity(id); setActiveTab('overview'); }}
          />
        )}

        {entity && activeTab === 'events' && (
          <TabEvents entity={entity} />
        )}

        {entity && activeTab === 'track' && (
          <TabTrack entity={entity} />
        )}

        {entity && activeTab === 'intel' && (
          <TabIntel entity={entity} />
        )}
      </div>

      {/* Footer — keyboard hint */}
      <div className="flex-shrink-0 border-t border-gray-800 px-3 py-1.5 flex items-center justify-between">
        <span className="text-[8px] font-mono text-gray-700">ESC to close · ← → to switch tabs</span>
        <span className="text-[8px] font-mono text-gray-700">
          {(width / 1).toFixed(0)}px
        </span>
      </div>
    </div>
  );
};