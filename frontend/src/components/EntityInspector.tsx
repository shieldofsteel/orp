import React, { useState, useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { Entity, RelationshipsResponse } from '../types';

const API_BASE = '/api/v1';

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
  const textColor =
    pct >= 80 ? 'text-green-400' : pct >= 50 ? 'text-amber-300' : 'text-red-400';
  return (
    <div>
      <div className="flex justify-between text-[10px] text-gray-400 mb-1">
        <span id="confidence-label">Confidence</span>
        <span className={textColor} aria-hidden="true">{pct}%</span>
      </div>
      <div
        role="progressbar"
        aria-labelledby="confidence-label"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuetext={`${pct}% data confidence`}
        className="h-1.5 rounded-full bg-gray-700 overflow-hidden"
      >
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
  const ageText =
    ageMin < 1 ? 'just now' : ageMin < 60 ? `${ageMin} minutes ago` : `${Math.floor(ageMin / 60)} hours ago`;

  return (
    <dl className="space-y-1.5">
      <div className="flex justify-between text-[10px]">
        <dt className="text-gray-500">Last updated</dt>
        <dd className={freshColor}>
          <time dateTime={updatedAt} aria-label={`Last updated ${ageText}`}>
            {ageMin < 1 ? 'just now' : ageMin < 60 ? `${ageMin}m ago` : `${Math.floor(ageMin / 60)}h ago`}
          </time>
        </dd>
      </div>
      <div className="flex justify-between text-[10px]">
        <dt className="text-gray-500">Updated at</dt>
        <dd className="text-gray-400 font-mono">
          <time dateTime={updatedAt}>{updated.toLocaleTimeString()}</time>
        </dd>
      </div>
      <div className="flex justify-between text-[10px]">
        <dt className="text-gray-500">Checked at</dt>
        <dd className="text-gray-400 font-mono">
          <time dateTime={checkedAt}>{checked.toLocaleTimeString()}</time>
        </dd>
      </div>
    </dl>
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
    <ul className="space-y-1 list-none p-0 m-0" aria-label={direction === 'out' ? 'Outgoing relationships' : 'Incoming relationships'}>
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
          <li
            key={rel.id}
            className="flex items-start gap-2 rounded-md bg-gray-800/50 px-2 py-1.5 hover:bg-gray-800 group transition-colors"
          >
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <span
                  className="text-[9px] text-blue-400 font-medium bg-blue-950/60 border border-blue-800/50 rounded px-1 py-0.5 whitespace-nowrap"
                  aria-label={`Relationship type: ${rel.type}`}
                >
                  {rel.type}
                </span>
                <span className="text-[9px] text-gray-600" aria-hidden="true">
                  {direction === 'out' ? '→' : '←'}
                </span>
              </div>
              <button
                onClick={() => onSelect(otherId)}
                className="mt-0.5 text-[10px] text-gray-300 hover:text-blue-300 truncate block text-left w-full"
                aria-label={`Select entity: ${otherName ?? otherId}${otherType ? ` (${otherType})` : ''}`}
              >
                {otherName ?? otherId}
              </button>
              {otherType && (
                <div className="text-[9px] text-gray-600" aria-hidden="true">{otherType}</div>
              )}
            </div>
            {rel.confidence != null && (
              <span className="text-[9px] text-gray-600 flex-shrink-0" aria-label={`Confidence: ${(rel.confidence * 100).toFixed(0)}%`}>
                {(rel.confidence * 100).toFixed(0)}%
              </span>
            )}
          </li>
        );
      })}
    </ul>
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

  const storeEntity = selectedEntityId ? entities.get(selectedEntityId) : null;

  // Refs for managing focus
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const tablistRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

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

  // Move focus to close button when panel opens
  useEffect(() => {
    if (inspectorOpen && selectedEntityId) {
      // Small delay to let rendering complete
      setTimeout(() => closeButtonRef.current?.focus(), 50);
    }
  }, [inspectorOpen, selectedEntityId]);

  const entity = fullEntity ?? storeEntity;

  if (!inspectorOpen || !selectedEntityId) return null;

  const close = () => {
    setInspectorOpen(false);
    selectEntity(null);
  };

  const currentTabIndex = TABS.findIndex((t) => t.id === inspectorTab);

  // Keyboard navigation within tab list: Left/Right arrow keys
  const handleTabKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>, tabId: Tab) => {
    const idx = TABS.findIndex((t) => t.id === tabId);
    let nextIdx = idx;

    if (e.key === 'ArrowRight') {
      nextIdx = (idx + 1) % TABS.length;
      e.preventDefault();
    } else if (e.key === 'ArrowLeft') {
      nextIdx = (idx - 1 + TABS.length) % TABS.length;
      e.preventDefault();
    } else if (e.key === 'Home') {
      nextIdx = 0;
      e.preventDefault();
    } else if (e.key === 'End') {
      nextIdx = TABS.length - 1;
      e.preventDefault();
    }

    if (nextIdx !== idx) {
      setInspectorTab(TABS[nextIdx].id);
      // Focus the newly selected tab button
      const tabBtns = tablistRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
      tabBtns?.[nextIdx]?.focus();
    }
  };

  const tabPanelId = `inspector-panel-${inspectorTab}`;
  const tabId = (id: Tab) => `inspector-tab-${id}`;

  return (
    <div
      className="orp-inspector flex flex-col w-80 flex-shrink-0 bg-gray-900 border-l border-gray-800 overflow-hidden"
      role="region"
      aria-label={entity ? `Entity inspector: ${entity.name ?? entity.id}` : 'Entity inspector'}
    >
      {/* Header */}
      <div className="flex-shrink-0 px-3 pt-3 pb-2 border-b border-gray-800">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            {loading && !entity ? (
              <div className="text-xs text-gray-500 animate-pulse" aria-live="polite" role="status">
                Loading…
              </div>
            ) : entity ? (
              <>
                <h2 className="text-xs font-semibold text-gray-100 truncate m-0">
                  {entity.name ?? entity.id}
                </h2>
                <div className="flex items-center gap-1.5 mt-0.5">
                  <span
                    className="text-[9px] px-1.5 py-0.5 rounded bg-gray-800 border border-gray-700 text-gray-400"
                    aria-label={`Entity type: ${entity.type}`}
                  >
                    {entity.type}
                  </span>
                  <span
                    className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${
                      entity.is_active ? 'bg-green-500' : 'bg-gray-600'
                    }`}
                    aria-label={entity.is_active ? 'Active' : 'Inactive'}
                    role="img"
                  />
                  <span className="text-[9px] text-gray-600 truncate" aria-label={`ID: ${entity.id}`}>
                    {entity.id}
                  </span>
                </div>
              </>
            ) : (
              <div className="text-xs text-gray-500">{selectedEntityId}</div>
            )}
          </div>
          <button
            ref={closeButtonRef}
            onClick={close}
            className="text-gray-600 hover:text-gray-300 flex-shrink-0 text-sm transition-colors p-1 rounded hover:bg-gray-800"
            aria-label="Close entity inspector"
          >
            <span aria-hidden="true">✕</span>
          </button>
        </div>

        {error && (
          <div
            className="mt-1.5 text-[10px] text-amber-400 bg-amber-950/30 border border-amber-800/40 rounded px-2 py-1"
            role="alert"
          >
            Partial data (API unavailable)
          </div>
        )}

        {/* Tab list */}
        <div
          ref={tablistRef}
          role="tablist"
          aria-label="Entity inspector sections"
          className="flex gap-0 mt-2.5 border-b border-gray-800 -mx-3 px-3"
        >
          {TABS.map((tab) => (
            <button
              key={tab.id}
              id={tabId(tab.id)}
              role="tab"
              aria-selected={inspectorTab === tab.id}
              aria-controls={tabPanelId}
              onClick={() => setInspectorTab(tab.id)}
              onKeyDown={(e) => handleTabKeyDown(e, tab.id)}
              tabIndex={inspectorTab === tab.id ? 0 : -1}
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

      {/* Tab Panel */}
      <div
        id={tabPanelId}
        ref={panelRef}
        role="tabpanel"
        aria-labelledby={tabId(inspectorTab)}
        tabIndex={0}
        className="flex-1 overflow-y-auto orp-scrollbar px-3 py-2.5 focus:outline-none focus-visible:ring-1 focus-visible:ring-blue-600"
      >
        {!entity && !loading && (
          <div className="text-[10px] text-gray-600 text-center py-8">
            No entity data available
          </div>
        )}

        {entity && inspectorTab === 'properties' && (
          <div>
            <h3 className="sr-only">Entity properties</h3>
            {/* Core fields */}
            <dl className="space-y-1 mb-3">
              {[
                ['ID', entity.id],
                ['Type', entity.type],
                ['Name', entity.name ?? '—'],
                ['Active', entity.is_active ? 'Yes' : 'No'],
              ].map(([k, v]) => (
                <div key={k} className="flex justify-between text-[10px] py-0.5">
                  <dt className="text-gray-600">{k}</dt>
                  <dd className="text-gray-300 font-mono truncate ml-2 max-w-[60%] text-right">{v}</dd>
                </div>
              ))}
              {entity.tags.length > 0 && (
                <div className="flex items-start justify-between text-[10px] py-0.5">
                  <dt className="text-gray-600">Tags</dt>
                  <dd className="flex flex-wrap gap-1 justify-end max-w-[60%]">
                    {entity.tags.map((tag) => (
                      <span key={tag} className="text-[9px] bg-gray-800 border border-gray-700 rounded px-1 text-gray-400">
                        {tag}
                      </span>
                    ))}
                  </dd>
                </div>
              )}
            </dl>

            {/* Dynamic properties */}
            {Object.keys(entity.properties).length > 0 && (
              <>
                <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                  Properties
                </h3>
                <dl className="space-y-1">
                  {Object.entries(entity.properties).map(([k, v]) => (
                    <div key={k} className="flex justify-between text-[10px] py-0.5 border-b border-gray-800/50">
                      <dt className="text-gray-600 min-w-0 truncate">{k}</dt>
                      <dd className="text-gray-300 font-mono ml-2 text-right min-w-0 truncate max-w-[60%]">
                        {v == null
                          ? '—'
                          : typeof v === 'object'
                          ? JSON.stringify(v)
                          : String(v)}
                      </dd>
                    </div>
                  ))}
                </dl>
              </>
            )}

            {/* Geometry */}
            {entity.geometry && (
              <div className="mt-3">
                <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                  Geometry
                </h3>
                <div
                  className="text-[10px] font-mono text-gray-500 bg-gray-800/50 rounded px-2 py-1.5 break-all"
                  aria-label={`Geometry type ${entity.geometry.type} with coordinates ${JSON.stringify(
                    (entity.geometry.coordinates as number[]).slice(0, 2).map((n) => n.toFixed(4))
                  )}`}
                >
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
            <h3 className="sr-only">Entity relationships</h3>
            {!relationships && loading && (
              <div className="text-[10px] text-gray-600 animate-pulse" role="status" aria-live="polite">
                Loading relationships…
              </div>
            )}
            {relationships && (
              <>
                <div>
                  <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                    Outgoing ({relationships.outgoing.length})
                  </h3>
                  <RelationList
                    items={relationships.outgoing}
                    direction="out"
                    onSelect={selectEntity}
                  />
                </div>
                <div>
                  <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-1.5 font-semibold">
                    Incoming ({relationships.incoming.length})
                  </h3>
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
          <div>
            <h3 className="sr-only">Change history</h3>
            <ol className="space-y-1.5 list-none p-0 m-0" aria-label="Entity change history, most recent first">
              {(entity.history ?? []).length === 0 ? (
                <li className="text-[10px] text-gray-600 py-4 text-center">No history</li>
              ) : (
                (entity.history ?? []).map((h, i) => (
                  <li key={i} className="flex gap-2.5">
                    <div className="flex flex-col items-center" aria-hidden="true">
                      <div className="w-1.5 h-1.5 rounded-full bg-blue-500 mt-1 flex-shrink-0" />
                      {i < (entity.history ?? []).length - 1 && (
                        <div className="w-px flex-1 bg-gray-800 mt-0.5" />
                      )}
                    </div>
                    <div className="pb-3 min-w-0">
                      <time
                        dateTime={h.timestamp}
                        className="text-[9px] text-gray-600 font-mono"
                      >
                        {new Date(h.timestamp).toLocaleString()}
                      </time>
                      <div className="text-[9px] text-gray-500 mt-0.5">{h.source}</div>
                      <dl className="mt-1 space-y-0.5">
                        {Object.entries(h.changed_properties).map(([k, v]) => (
                          <div key={k} className="text-[9px] font-mono">
                            <dt className="inline text-gray-600">{k}:</dt>{' '}
                            <dd className="inline text-green-400">{JSON.stringify(v)}</dd>
                          </div>
                        ))}
                      </dl>
                    </div>
                  </li>
                ))
              )}
            </ol>
          </div>
        )}

        {entity && inspectorTab === 'quality' && (
          <div className="space-y-4">
            <h3 className="sr-only">Data quality indicators</h3>
            <ConfidenceBar value={entity.confidence} />

            <div>
              <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Freshness
              </h3>
              <FreshnessIndicator
                updatedAt={entity.freshness?.updated_at ?? entity.updated_at}
                checkedAt={entity.freshness?.checked_at ?? entity.updated_at}
              />
            </div>

            <div>
              <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Data Sources
              </h3>
              <div className="text-[10px] text-gray-400">
                {(entity.history ?? []).length > 0 ? (
                  <ul className="space-y-1 list-none p-0 m-0" aria-label="Data sources">
                    {[...new Set((entity.history ?? []).map((h) => h.source))].map((src) => (
                      <li key={src} className="flex items-center gap-1.5">
                        <span className="w-1.5 h-1.5 rounded-full bg-green-500 flex-shrink-0" aria-hidden="true" />
                        <span className="font-mono text-gray-400">{src}</span>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <span className="text-gray-600">No source history</span>
                )}
              </div>
            </div>

            <div>
              <h3 className="text-[9px] uppercase tracking-wider text-gray-600 mb-2 font-semibold">
                Record
              </h3>
              <dl className="space-y-1">
                {[
                  ['Created', new Date(entity.created_at).toLocaleString()],
                  ['Updated', new Date(entity.updated_at).toLocaleString()],
                  ['History entries', String((entity.history ?? []).length)],
                ].map(([k, v]) => (
                  <div key={k} className="flex justify-between text-[10px]">
                    <dt className="text-gray-600">{k}</dt>
                    <dd className="text-gray-400 font-mono">{v}</dd>
                  </div>
                ))}
              </dl>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
