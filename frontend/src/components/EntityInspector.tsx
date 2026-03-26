import React, { useState, useEffect } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { Entity, RelationshipsResponse } from '../types';

const API_BASE = 'http://localhost:9090/api/v1';

async function fetchEntityFull(id: string): Promise<Entity> {
  const res = await fetch(`${API_BASE}/entities/${encodeURIComponent(id)}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

async function fetchRelationships(id: string): Promise<RelationshipsResponse> {
  const res = await fetch(`${API_BASE}/entities/${encodeURIComponent(id)}/relationships`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

type Tab = 'properties' | 'relationships' | 'history' | 'quality';

const TABS: { id: Tab; label: string }[] = [
  { id: 'properties', label: 'Properties' },
  { id: 'relationships', label: 'Relations' },
  { id: 'history', label: 'History' },
  { id: 'quality', label: 'Quality' },
];

function ConfidenceBar({ value }: { value: number }) {
  const pct = Math.round(value * 100);
  const color =
    pct >= 80 ? 'bg-green-500' : pct >= 50 ? 'bg-amber-400' : 'bg-red-500';
  return (
    <div>
      <div className="flex justify-between text-[10px] text-gray-400 mb-1">
        <span>Confidence</span>
        <span className={pct >= 80 ? 'text-green-400' : pct >= 50 ? 'text-amber-300' : 'text-red-400'}>
          {pct}%
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-gray-700 overflow-hidden">
        <div
          className={`h-full rounded-full transition-all ${color}`}
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}

function FreshnessIndicator({ updatedAt, checkedAt }: { updatedAt: string; checkedAt: string }) {
  const updated = new Date(updatedAt);
  const checked = new Date(checkedAt);
  const ageMs = Date.now() - updated.getTime();
  const ageMin = Math.floor(ageMs / 60_000);

  const freshColor =
    ageMin < 5 ? 'text-green-400' : ageMin < 30 ? 'text-amber-300' : 'text-red-400';

  return (
    <div className="space-y-1.5">
      <div className="flex justify-between text-[10px]">
        <span className="text-gray-500">Last updated</span>
        <span className={freshColor}>
          {ageMin < 1 ? 'just now' : ageMin < 60 ? `${ageMin}m ago` : `${Math.floor(ageMin / 60)}h ago`}
        </span>
      </div>
      <div className="flex justify-between text-[10px]">
        <span className="text-gray-500">Updated at</span>
        <span className="text-gray-400 font-mono">{updated.toLocaleTimeString()}</span>
      </div>
      <div className="flex justify-between text-[10px]">
        <span className="text-gray-500">Checked at</span>
        <span className="text-gray-400 font-mono">{checked.toLocaleTimeString()}</span>
      </div>
    </div>
  );
}

interface RelationListProps {
  items: RelationshipsResponse['outgoing'] | RelationshipsResponse['incoming'];
  direction: 'out' | 'in';
  onSelect: (id: string) => void;
}

function RelationList({ items, direction, onSelect }: RelationListProps) {
  if (items.length === 0) {
    return <div className="text-[10px] text-gray-600 py-2">None</div>;
  }
  return (
    <div className="space-y-1">
      {items.map((rel) => {
        const otherId = direction === 'out'
          ? (rel as RelationshipsResponse['outgoing'][number]).target_id
          : (rel as RelationshipsResponse['incoming'][number]).source_id;
        const otherName = direction === 'out'
          ? (rel as RelationshipsResponse['outgoing'][number]).target_name
          : (rel as RelationshipsResponse['incoming'][number]).source_name;
        const otherType = direction === 'out'
          ? (rel as RelationshipsResponse['outgoing'][number]).target_type
          : (rel as RelationshipsResponse['incoming'][number]).source_type;

        return (
          <div
            key={rel.id}
            className="flex items-start gap-2 rounded-md bg-gray-800/50 px-2 py-1.5 hover:bg-gray-800 group transition-colors"
          >
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <span className="text-[9px] text-blue-400 font-medium bg-blue-950/60 border border-blue-800/50 rounded px-1 py-0.5 whitespace-nowrap">
                  {rel.type}
                </span>
                {direction === 'out' ? (
                  <span className="text-[9px] text-gray-600">→</span>
                ) : (
                  <span className="text-[9px] text-gray-600">←</span>
                )}
              </div>
              <button
                onClick={() => onSelect(otherId)}
                className="mt-0.5 text-[10px] text-gray-300 hover:text-blue-300 truncate block text-left w-full"
              >
                {otherName ?? otherId}
              </button>
              {otherType && (
                <div className="text-[9px] text-gray-600">{otherType}</div>
              )}
            </div>
            {rel.confidence != null && (
              <span className="text-[9px] text-gray-600 flex-shrink-0">
                {(rel.confidence * 100).toFixed(0)}%
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}

export const EntityInspector: React.FC = () => {
  const inspectorOpen = useAppStore((s) => s.inspectorOpen);
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const entities = useAppStore((s) => s.entities);
  const inspectorTab = useAppStore((s) => s.inspectorTab);
  const setInspectorTab = useAppStore((s) => s.setInspectorTab);
  const selectEntity = useAppStore((s) => s.selectEntity);
  const setInspectorOpen = useAppStore((s) => s.setInspectorOpen);

  const [fullEntity, setFullEntity] = useState<Entity | null>(null);
  const [relationships, setRelationships] = useState<RelationshipsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Use the store entity as the base (always up to date via WS)
  const storeEntity = selectedEntityId ? entities.get(selectedEntityId) : null;

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
    ]).then(([entity, rels]) => {
      setFullEntity(entity);
      setRelationships(rels);
      setLoading(false);
    });
  }, [selectedEntityId]);

  const entity = fullEntity ?? storeEntity;

  if (!inspectorOpen || !selectedEntityId) return null;

  const close = () => {
    setInspectorOpen(false);
    selectEntity(null);
  };

  return (
    <div className="orp-inspector flex flex-col w-80 flex-shrink-0 bg-gray-900 border-l border-gray-800 overflow-hidden">
      {/* Header */}
      <div className="flex-shrink-0 px-3 pt-3 pb-2 border-b border-gray-800">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            {loading && !entity ? (
              <div className="text-xs text-gray-500 animate-pulse">Loading…</div>
            ) : entity ? (
              <>
                <div className="text-xs font-semibold text-gray-100 truncate">
                  {entity.name ?? entity.id}
                </div>
                <div className="flex items-center gap-1.5 mt-0.5">
                  <span className="text-[9px] px-1.5 py-0.5 rounded bg-gray-800 border border-gray-700 text-gray-400">
                    {entity.type}
                  </span>
                  <span
                    className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${
                      entity.is_active ? 'bg-green-500' : 'bg-gray-600'
                    }`}
                  />
                  <span className="text-[9px] text-gray-600 truncate">{entity.id}</span>
                </div>
              </>
            ) : (
              <div className="text-xs text-gray-500">{selectedEntityId}</div>
            )}
          </div>
          <button
            onClick={close}
            className="text-gray-600 hover:text-gray-300 flex-shrink-0 text-sm transition-colors"
          >
            ✕
          </button>
        </div>

        {error && (
          <div className="mt-1.5 text-[10px] text-amber-400 bg-amber-950/30 border border-amber-800/40 rounded px-2 py-1">
            Partial data (API unavailable)
          </div>
        )}

        {/* Tabs */}
        <div className="flex gap-0 mt-2.5 border-b border-gray-800 -mx-3 px-3">
          {TABS.map((tab) => (
            <button
              key={tab.id}
              onClick={() => setInspectorTab(tab.id)}
              className={`text-[10px] pb-1.5 px-1.5 mr-2 border-b-2 transition-colors ${
                inspectorTab === tab.id
                  ? 'border-blue-500 text-blue-400'
                  : 'border-transparent text-gray-600 hover:text-gray-400'
              }`}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </div>

      {/* Tab Content */}
      <div className="flex-1 overflow-y-auto orp-scrollbar px-3 py-2.5">
        {!entity && !loading && (
          <div className="text-[10px] text-gray-600 text-center py-8">
            No entity data available
          </div>
        )}

        {entity && inspectorTab === 'properties' && (
          <div>
            {/* Core fields */}
            <div className="space-y-1 mb-3">
              {[
                ['ID', entity.id],
                ['Type', entity.type],
                ['Name', entity.name ?? '—'],
                ['Active', entity.is_active ? 'Yes' : 'No'],
              ].map(([k, v]) => (
                <div key={k} className="flex justify-between text-[10px] py-0.5">
                  <span className="text-gray-600">{k}</span>
                  <span className="text-gray-300 font-mono truncate ml-2 max-w-[60%] text-right">{v}</span>
                </div>
              ))}
              {entity.tags.length > 0 && (
                <div className="flex items-start justify-between text-[10px] py-0.5">
                  <span className="text-gray-600">Tags</span>
                  <div className="flex flex-wrap gap-1 justify-end max-w-[60%]">
                    {entity.tags.map((tag) => (
                      <span key={tag} className="text-[9px] bg-gray-800 border border-gray-700 rounded px-1 text-gray-400">
                        {tag}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>

            {/* Dynamic properties */}
            {Object.keys(entity.properties).length > 0 && (
              <>
                <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                  Properties
                </div>
                <div className="space-y-1">
                  {Object.entries(entity.properties).map(([k, v]) => (
                    <div key={k} className="flex justify-between text-[10px] py-0.5 border-b border-gray-800/50">
                      <span className="text-gray-600 min-w-0 truncate">{k}</span>
                      <span className="text-gray-300 font-mono ml-2 text-right min-w-0 truncate max-w-[60%]">
                        {v == null
                          ? '—'
                          : typeof v === 'object'
                          ? JSON.stringify(v)
                          : String(v)}
                      </span>
                    </div>
                  ))}
                </div>
              </>
            )}

            {/* Geometry */}
            {entity.geometry && (
              <div className="mt-3">
                <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                  Geometry
                </div>
                <div className="text-[10px] font-mono text-gray-500 bg-gray-800/50 rounded px-2 py-1.5 break-all">
                  {entity.geometry.type}:{' '}
                  {JSON.stringify(
                    (entity.geometry.coordinates as number[]).slice(0, 2).map((n) =>
                      n.toFixed(4)
                    )
                  )}
                </div>
              </div>
            )}
          </div>
        )}

        {entity && inspectorTab === 'relationships' && (
          <div className="space-y-3">
            {!relationships && loading && (
              <div className="text-[10px] text-gray-600 animate-pulse">Loading relationships…</div>
            )}
            {relationships && (
              <>
                <div>
                  <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                    Outgoing ({relationships.outgoing.length})
                  </div>
                  <RelationList
                    items={relationships.outgoing}
                    direction="out"
                    onSelect={selectEntity}
                  />
                </div>
                <div>
                  <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                    Incoming ({relationships.incoming.length})
                  </div>
                  <RelationList
                    items={relationships.incoming}
                    direction="in"
                    onSelect={selectEntity}
                  />
                </div>
              </>
            )}
            {!relationships && !loading && (
              <div className="text-[10px] text-gray-600">No relationship data</div>
            )}
          </div>
        )}

        {entity && inspectorTab === 'history' && (
          <div className="space-y-1.5">
            {(entity.history ?? []).length === 0 ? (
              <div className="text-[10px] text-gray-600 py-4 text-center">No history</div>
            ) : (
              (entity.history ?? []).map((h, i) => (
                <div key={i} className="flex gap-2.5">
                  <div className="flex flex-col items-center">
                    <div className="w-1.5 h-1.5 rounded-full bg-blue-500 mt-1 flex-shrink-0" />
                    {i < (entity.history ?? []).length - 1 && (
                      <div className="w-px flex-1 bg-gray-800 mt-0.5" />
                    )}
                  </div>
                  <div className="pb-3 min-w-0">
                    <div className="text-[9px] text-gray-600 font-mono">
                      {new Date(h.timestamp).toLocaleString()}
                    </div>
                    <div className="text-[9px] text-gray-500 mt-0.5">{h.source}</div>
                    <div className="mt-1 space-y-0.5">
                      {Object.entries(h.changed_properties).map(([k, v]) => (
                        <div key={k} className="text-[9px] font-mono">
                          <span className="text-gray-600">{k}:</span>{' '}
                          <span className="text-green-400">{JSON.stringify(v)}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>
        )}

        {entity && inspectorTab === 'quality' && (
          <div className="space-y-4">
            <ConfidenceBar value={entity.confidence} />

            <div>
              <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Freshness
              </div>
              <FreshnessIndicator
                updatedAt={entity.freshness?.updated_at ?? entity.updated_at}
                checkedAt={entity.freshness?.checked_at ?? entity.updated_at}
              />
            </div>

            <div>
              <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Data Sources
              </div>
              <div className="text-[10px] text-gray-400">
                {(entity.history ?? []).length > 0 ? (
                  <div className="space-y-1">
                    {[...new Set((entity.history ?? []).map((h) => h.source))].map((src) => (
                      <div key={src} className="flex items-center gap-1.5">
                        <span className="w-1.5 h-1.5 rounded-full bg-green-500 flex-shrink-0" />
                        <span className="font-mono text-gray-400">{src}</span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <span className="text-gray-600">No source history</span>
                )}
              </div>
            </div>

            <div>
              <div className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Record
              </div>
              <div className="space-y-1">
                {[
                  ['Created', new Date(entity.created_at).toLocaleString()],
                  ['Updated', new Date(entity.updated_at).toLocaleString()],
                  ['History entries', String((entity.history ?? []).length)],
                ].map(([k, v]) => (
                  <div key={k} className="flex justify-between text-[10px]">
                    <span className="text-gray-600">{k}</span>
                    <span className="text-gray-400 font-mono">{v}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
