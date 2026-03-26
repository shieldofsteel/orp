/**
 * EntityCard — Compact entity card for lists and search results.
 * Military ops aesthetic. Dark theme. Straight corners only.
 */
import React from 'react';
import type { Entity } from '../types';

// ─── Entity kind resolution ────────────────────────────────────────────────────

type EntityKind = 'ship' | 'aircraft' | 'port' | 'sensor' | 'unknown';

function resolveKind(entity: Entity): EntityKind {
  const t = entity.type?.toLowerCase() ?? '';
  if (t.includes('ship') || t.includes('vessel') || t.includes('ais')) return 'ship';
  if (t.includes('aircraft') || t.includes('flight') || t.includes('adsb')) return 'aircraft';
  if (t.includes('port') || t.includes('harbor')) return 'port';
  if (t.includes('sensor') || t.includes('radar') || t.includes('camera')) return 'sensor';
  return 'unknown';
}

function EntityIcon({ kind }: { kind: EntityKind }) {
  const props = { width: 16, height: 16, viewBox: '0 0 24 24', fill: 'none' };
  if (kind === 'ship')
    return (
      <svg {...props} aria-label="Ship">
        <path d="M3 17l1.5-6h15L21 17H3z" stroke="#38bdf8" strokeWidth="1.5" />
        <path d="M8 11V7l4-3 4 3v4" stroke="#38bdf8" strokeWidth="1.5" />
      </svg>
    );
  if (kind === 'aircraft')
    return (
      <svg {...props} aria-label="Aircraft">
        <path d="M12 3L4 14h3l-1 7 6-2 6 2-1-7h3L12 3z" stroke="#34d399" strokeWidth="1.5" />
      </svg>
    );
  if (kind === 'port')
    return (
      <svg {...props} aria-label="Port">
        <rect x="3" y="8" width="18" height="10" stroke="#facc15" strokeWidth="1.5" fill="none" />
        <path d="M8 8V5h8v3" stroke="#facc15" strokeWidth="1.5" />
      </svg>
    );
  if (kind === 'sensor')
    return (
      <svg {...props} aria-label="Sensor">
        <circle cx="12" cy="12" r="3" stroke="#a78bfa" strokeWidth="1.5" fill="none" />
        <path d="M7 7a7 7 0 0 0 0 10M17 7a7 7 0 0 1 0 10" stroke="#a78bfa" strokeWidth="1.5" fill="none" />
      </svg>
    );
  return (
    <svg {...props} aria-label="Unknown entity">
      <circle cx="12" cy="12" r="8" stroke="#6b7280" strokeWidth="1.5" fill="none" />
    </svg>
  );
}

const KIND_BADGE: Record<EntityKind, string> = {
  ship:     'text-sky-400 bg-sky-950 border-sky-800',
  aircraft: 'text-emerald-400 bg-emerald-950 border-emerald-800',
  port:     'text-yellow-400 bg-yellow-950 border-yellow-800',
  sensor:   'text-violet-400 bg-violet-950 border-violet-800',
  unknown:  'text-gray-400 bg-gray-800 border-gray-700',
};

// ─── Time formatting ───────────────────────────────────────────────────────────

function timeAgo(ts: string): string {
  const ms = Date.now() - new Date(ts).getTime();
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

function freshnessColor(ts: string): string {
  const m = (Date.now() - new Date(ts).getTime()) / 60000;
  if (m < 5) return 'text-green-400';
  if (m < 30) return 'text-amber-400';
  return 'text-red-400';
}

// ─── EntityCard ────────────────────────────────────────────────────────────────

export interface EntityCardProps {
  entity: Entity;
  selected?: boolean;
  onClick?: (entity: Entity) => void;
  className?: string;
}

export const EntityCard: React.FC<EntityCardProps> = ({
  entity,
  selected = false,
  onClick,
  className = '',
}) => {
  const kind = resolveKind(entity);
  const ts = entity.freshness?.updated_at ?? entity.updated_at;
  const coords =
    entity.geometry?.type === 'Point'
      ? (entity.geometry.coordinates as [number, number])
      : null;
  const speed = entity.properties?.speed != null ? Number(entity.properties.speed) : null;
  const heading = entity.properties?.heading != null ? Number(entity.properties.heading) : null;

  return (
    <button
      onClick={() => onClick?.(entity)}
      className={[
        'w-full text-left group transition-all duration-100',
        'border bg-[#0d1117]',
        selected
          ? 'border-blue-600 bg-blue-950/20'
          : 'border-gray-800 hover:border-gray-600 hover:bg-gray-900',
        className,
      ].join(' ')}
      aria-pressed={selected}
      aria-label={`${entity.name ?? entity.id} — ${entity.type}${entity.is_active ? ', active' : ', inactive'}`}
    >
      <div className="flex items-start gap-2 px-2.5 py-2">
        {/* Icon */}
        <div className={`flex-shrink-0 mt-0.5 p-1 border ${selected ? 'border-blue-800 bg-blue-950' : 'border-gray-800 bg-gray-900 group-hover:border-gray-700'}`}>
          <EntityIcon kind={kind} />
        </div>

        {/* Content */}
        <div className="flex-1 min-w-0 space-y-0.5">
          {/* Name + type badge */}
          <div className="flex items-center gap-1.5 justify-between">
            <span className="text-[11px] font-mono font-semibold text-gray-200 truncate">
              {entity.name ?? entity.id}
            </span>
            <span className={`flex-shrink-0 text-[7px] font-mono px-1 py-px border uppercase tracking-wider ${KIND_BADGE[kind]}`}>
              {entity.type}
            </span>
          </div>

          {/* Position */}
          {coords && (
            <div className="text-[8px] font-mono text-gray-500">
              {coords[1].toFixed(4)}°N {coords[0].toFixed(4)}°E
            </div>
          )}

          {/* Speed + last updated */}
          <div className="flex items-center gap-2 justify-between">
            <div className="flex items-center gap-2">
              {speed != null && (
                <span className="text-[8px] font-mono text-sky-400">
                  {speed.toFixed(1)} kn
                </span>
              )}
              {heading != null && (
                <span className="text-[8px] font-mono text-gray-600">
                  {heading.toFixed(0)}°
                </span>
              )}
            </div>
            <span className={`text-[8px] font-mono ${freshnessColor(ts)}`}>
              {timeAgo(ts)}
            </span>
          </div>
        </div>

        {/* Status dot */}
        <div className="flex-shrink-0 mt-1.5">
          <span
            className={`block w-1.5 h-1.5 ${entity.is_active ? 'bg-green-400' : 'bg-gray-700'}`}
            aria-hidden="true"
          />
        </div>
      </div>

      {/* Selected accent bar */}
      {selected && (
        <div className="h-px bg-blue-600 w-full" />
      )}
    </button>
  );
};

export default EntityCard;
