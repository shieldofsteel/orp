import React, { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { Entity } from '../types';

const API_BASE = '/api/v1';

// ── Types ──────────────────────────────────────────────────────────────────

interface SearchCriteria {
  entityType: string;
  name: string;
  mmsiOrIcao: string;
  flag: string;
  speedMin: string;
  speedMax: string;
  nearLat: string;
  nearLon: string;
  nearRadiusKm: string;
  polygon: Array<[number, number]>;
  timeFrom: string;
  timeTo: string;
  propertyKey: string;
  propertyValue: string;
}

interface SavedSearch {
  id: string;
  name: string;
  criteria: SearchCriteria;
  createdAt: string;
}

const EMPTY_CRITERIA: SearchCriteria = {
  entityType: '',
  name: '',
  mmsiOrIcao: '',
  flag: '',
  speedMin: '',
  speedMax: '',
  nearLat: '',
  nearLon: '',
  nearRadiusKm: '',
  polygon: [],
  timeFrom: '',
  timeTo: '',
  propertyKey: '',
  propertyValue: '',
};

const ENTITY_TYPES = ['Ship', 'Port', 'Aircraft', 'WeatherSystem', 'Zone', 'Facility'];
const FLAG_CODES = ['US', 'GB', 'NL', 'DE', 'FR', 'CN', 'SG', 'PA', 'LR', 'MH', 'BS', 'CY', 'MT'];

// ── EntityCard ─────────────────────────────────────────────────────────────

function EntityCard({
  entity,
  selected,
  onSelect,
  onInspect,
}: {
  entity: Entity;
  selected: boolean;
  onSelect: (id: string, checked: boolean) => void;
  onInspect: (entity: Entity) => void;
}) {
  const lat = Array.isArray(entity.geometry?.coordinates)
    ? (entity.geometry!.coordinates as number[])[1]
    : null;
  const lon = Array.isArray(entity.geometry?.coordinates)
    ? (entity.geometry!.coordinates as number[])[0]
    : null;
  const speed = entity.properties?.speed as number | undefined;
  const flag = entity.properties?.flag as string | undefined;

  const typeColor: Record<string, string> = {
    Ship: 'text-blue-400 bg-blue-900/30 border-blue-800',
    Port: 'text-amber-400 bg-amber-900/30 border-amber-800',
    Aircraft: 'text-cyan-400 bg-cyan-900/30 border-cyan-800',
    WeatherSystem: 'text-purple-400 bg-purple-900/30 border-purple-800',
    Zone: 'text-green-400 bg-green-900/30 border-green-800',
    Facility: 'text-rose-400 bg-rose-900/30 border-rose-800',
  };
  const colorClass = typeColor[entity.type] ?? 'text-gray-400 bg-gray-800 border-gray-700';

  return (
    <div
      className={`flex items-start gap-2.5 px-3 py-2 border-b border-gray-800 hover:bg-gray-800/60 transition-colors cursor-pointer ${selected ? 'bg-blue-950/40' : ''}`}
      onClick={() => onInspect(entity)}
      role="row"
    >
      <input
        type="checkbox"
        checked={selected}
        onChange={(e) => { e.stopPropagation(); onSelect(entity.id, e.target.checked); }}
        onClick={(e) => e.stopPropagation()}
        className="mt-0.5 accent-blue-500 cursor-pointer flex-shrink-0"
        aria-label={`Select ${entity.name ?? entity.id}`}
      />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className={`text-[10px] font-semibold px-1.5 py-0.5 border ${colorClass}`}>
            {entity.type}
          </span>
          <span className="text-xs font-medium text-gray-200 truncate">
            {entity.name ?? entity.id}
          </span>
          {flag && (
            <span className="text-[10px] text-gray-500 ml-auto flex-shrink-0">{flag}</span>
          )}
        </div>
        <div className="flex items-center gap-3 mt-0.5 text-[10px] text-gray-500">
          {lat !== null && lon !== null && (
            <span>{lat.toFixed(3)}°, {lon.toFixed(3)}°</span>
          )}
          {speed !== undefined && (
            <span>{speed} kn</span>
          )}
          <span className="ml-auto text-gray-600 truncate max-w-[120px]">{entity.id.slice(0, 12)}…</span>
        </div>
      </div>
    </div>
  );
}

// ── PolygonDrawHelper ──────────────────────────────────────────────────────

function PolygonDrawHelper({
  polygon,
  onChange,
}: {
  polygon: Array<[number, number]>;
  onChange: (pts: Array<[number, number]>) => void;
}) {
  const [input, setInput] = useState('');
  const [error, setError] = useState('');

  const addPoint = () => {
    const parts = input.split(',').map((s) => parseFloat(s.trim()));
    if (parts.length !== 2 || parts.some(isNaN)) {
      setError('Enter: lat, lon');
      return;
    }
    setError('');
    setInput('');
    onChange([...polygon, [parts[0], parts[1]]]);
  };

  return (
    <div className="space-y-1">
      <div className="flex gap-1">
        <input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && addPoint()}
          placeholder="lat, lon"
          className="flex-1 bg-gray-800 border border-gray-700 text-gray-200 text-[11px] px-2 py-1 focus:outline-none focus:border-blue-600"
        />
        <button
          onClick={addPoint}
          className="px-2 py-1 bg-blue-700 text-white text-[10px] hover:bg-blue-600 transition-colors"
        >
          +
        </button>
        {polygon.length > 0 && (
          <button
            onClick={() => onChange([])}
            className="px-2 py-1 bg-gray-700 text-gray-300 text-[10px] hover:bg-gray-600 transition-colors"
          >
            Clear
          </button>
        )}
      </div>
      {error && <span className="text-red-400 text-[10px]">{error}</span>}
      {polygon.length > 0 && (
        <div className="text-[10px] text-gray-500">
          {polygon.length} point{polygon.length !== 1 ? 's' : ''}:{' '}
          {polygon.map((p, i) => `(${p[0].toFixed(2)},${p[1].toFixed(2)})`).join(' → ')}
        </div>
      )}
    </div>
  );
}

// ── SearchPanel ────────────────────────────────────────────────────────────

export function SearchPanel() {
  const [criteria, setCriteria] = useState<SearchCriteria>(EMPTY_CRITERIA);
  const [results, setResults] = useState<Entity[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [savedSearches, setSavedSearches] = useState<SavedSearch[]>(() => {
    try { return JSON.parse(localStorage.getItem('orp_saved_searches') ?? '[]'); } catch { return []; }
  });
  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [saveName, setSaveName] = useState('');
  const [activeTab, setActiveTab] = useState<'form' | 'saved'>('form');
  const [sortField, setSortField] = useState<'name' | 'type' | 'speed'>('name');
  const [sortAsc, setSortAsc] = useState(true);

  const selectEntity = useAppStore((s) => s.selectEntity);
  const setInspectorOpen = useAppStore((s) => s.setInspectorOpen);
  const token = localStorage.getItem('orp_token');

  const set = (key: keyof SearchCriteria, value: unknown) =>
    setCriteria((prev) => ({ ...prev, [key]: value }));

  // ── Build ORP-QL query from criteria ──────────────────────────────────
  const buildQuery = useCallback((c: SearchCriteria): string => {
    const conditions: string[] = [];
    let matchClause = `MATCH (e${c.entityType ? `:${c.entityType}` : ''})`;

    if (c.name) conditions.push(`e.name LIKE "${c.name}"`);
    if (c.mmsiOrIcao) conditions.push(`(e.mmsi = "${c.mmsiOrIcao}" OR e.icao = "${c.mmsiOrIcao}")`);
    if (c.flag) conditions.push(`e.flag = "${c.flag}"`);
    if (c.speedMin) conditions.push(`e.speed >= ${c.speedMin}`);
    if (c.speedMax) conditions.push(`e.speed <= ${c.speedMax}`);
    if (c.timeFrom) conditions.push(`e.updated_at >= "${c.timeFrom}"`);
    if (c.timeTo) conditions.push(`e.updated_at <= "${c.timeTo}"`);
    if (c.propertyKey && c.propertyValue) conditions.push(`e.${c.propertyKey} = "${c.propertyValue}"`);

    const geoConditions: string[] = [];
    if (c.nearLat && c.nearLon && c.nearRadiusKm) {
      geoConditions.push(`NEAR(e, ${c.nearLat}, ${c.nearLon}, ${c.nearRadiusKm})`);
    }
    if (c.polygon.length >= 3) {
      const coords = c.polygon.map(([lat, lon]) => `[${lat},${lon}]`).join(',');
      geoConditions.push(`WITHIN(e, POLYGON([${coords}]))`);
    }

    const allConditions = [...conditions, ...geoConditions];
    const whereClause = allConditions.length > 0 ? ` WHERE ${allConditions.join(' AND ')}` : '';
    return `${matchClause}${whereClause} RETURN e LIMIT 500`;
  }, []);

  // ── Execute search ────────────────────────────────────────────────────
  const runSearch = useCallback(async (c: SearchCriteria) => {
    setLoading(true);
    setError('');
    setSelectedIds(new Set());
    try {
      const query = buildQuery(c);
      const res = await fetch(`${API_BASE}/query`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({ query }),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      const rows: Entity[] = Array.isArray(data) ? data : (data.results ?? data.data ?? []);
      setResults(rows);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Search failed');
      setResults([]);
    } finally {
      setLoading(false);
    }
  }, [buildQuery, token]);

  // Real-time filtering on text fields
  const debouncedSearch = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (debouncedSearch.current) clearTimeout(debouncedSearch.current);
    debouncedSearch.current = setTimeout(() => {
      if (criteria.name || criteria.mmsiOrIcao || criteria.entityType) {
        runSearch(criteria);
      }
    }, 400);
    return () => { if (debouncedSearch.current) clearTimeout(debouncedSearch.current); };
  }, [criteria.name, criteria.mmsiOrIcao, criteria.entityType]);

  // ── Sorted results ────────────────────────────────────────────────────
  const sortedResults = useMemo(() => {
    return [...results].sort((a, b) => {
      let av: unknown, bv: unknown;
      if (sortField === 'name') { av = a.name ?? ''; bv = b.name ?? ''; }
      else if (sortField === 'type') { av = a.type; bv = b.type; }
      else { av = (a.properties?.speed as number) ?? -1; bv = (b.properties?.speed as number) ?? -1; }
      if (av === bv) return 0;
      const cmp = av! < bv! ? -1 : 1;
      return sortAsc ? cmp : -cmp;
    });
  }, [results, sortField, sortAsc]);

  // ── Selection ─────────────────────────────────────────────────────────
  const handleSelect = (id: string, checked: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (checked) next.add(id); else next.delete(id);
      return next;
    });
  };
  const selectAll = () => setSelectedIds(new Set(sortedResults.map((e) => e.id)));
  const clearSelection = () => setSelectedIds(new Set());

  // ── Export CSV ────────────────────────────────────────────────────────
  const exportCsv = () => {
    const rows = sortedResults.filter((e) => selectedIds.size === 0 || selectedIds.has(e.id));
    if (rows.length === 0) return;
    const headers = ['id', 'type', 'name', 'lat', 'lon', 'speed', 'flag', 'updated_at'];
    const lines = rows.map((e) => {
      const coords = e.geometry?.coordinates as number[] | undefined;
      return [
        e.id, e.type, e.name ?? '', coords?.[1] ?? '', coords?.[0] ?? '',
        (e.properties?.speed as number) ?? '',
        (e.properties?.flag as string) ?? '',
        e.updated_at,
      ].map((v) => `"${String(v).replace(/"/g, '""')}"`).join(',');
    });
    const blob = new Blob([headers.join(',') + '\n' + lines.join('\n')], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url; a.download = 'orp_search_results.csv'; a.click();
    URL.revokeObjectURL(url);
  };

  // ── Saved searches ────────────────────────────────────────────────────
  const saveSearch = () => {
    if (!saveName.trim()) return;
    const entry: SavedSearch = {
      id: crypto.randomUUID(),
      name: saveName.trim(),
      criteria: { ...criteria },
      createdAt: new Date().toISOString(),
    };
    const updated = [entry, ...savedSearches];
    setSavedSearches(updated);
    localStorage.setItem('orp_saved_searches', JSON.stringify(updated));
    setSaveDialogOpen(false);
    setSaveName('');
  };
  const loadSearch = (s: SavedSearch) => {
    setCriteria(s.criteria);
    setActiveTab('form');
    runSearch(s.criteria);
  };
  const deleteSearch = (id: string) => {
    const updated = savedSearches.filter((s) => s.id !== id);
    setSavedSearches(updated);
    localStorage.setItem('orp_saved_searches', JSON.stringify(updated));
  };

  const handleInspect = (entity: Entity) => {
    selectEntity(entity.id);
    setInspectorOpen(true);
  };

  // ── Render ────────────────────────────────────────────────────────────
  return (
    <div className="h-full flex flex-col bg-gray-950 text-gray-200 overflow-hidden">
      {/* Header */}
      <div className="flex-shrink-0 flex items-center justify-between px-4 py-2 border-b border-gray-800 bg-gray-900">
        <h2 className="text-xs font-semibold text-gray-300 tracking-widest uppercase">Advanced Search</h2>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setSaveDialogOpen(true)}
            className="text-[10px] px-2 py-1 border border-gray-700 text-gray-400 hover:border-blue-600 hover:text-blue-400 transition-colors"
          >
            Save Search
          </button>
          <button
            onClick={() => { setCriteria(EMPTY_CRITERIA); setResults([]); setError(''); }}
            className="text-[10px] px-2 py-1 border border-gray-700 text-gray-500 hover:border-gray-500 hover:text-gray-300 transition-colors"
          >
            Reset
          </button>
        </div>
      </div>

      {/* Tab switcher */}
      <div className="flex-shrink-0 flex border-b border-gray-800">
        {(['form', 'saved'] as const).map((tab) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={`px-4 py-1.5 text-[10px] font-medium uppercase tracking-wider border-r border-gray-800 transition-colors ${
              activeTab === tab ? 'bg-gray-800 text-blue-400 border-b-2 border-b-blue-500' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800/50'
            }`}
          >
            {tab === 'form' ? 'Search Form' : `Saved (${savedSearches.length})`}
          </button>
        ))}
      </div>

      {activeTab === 'saved' ? (
        /* ── Saved searches list ── */
        <div className="flex-1 overflow-y-auto">
          {savedSearches.length === 0 ? (
            <div className="flex items-center justify-center h-32 text-gray-600 text-xs">
              No saved searches yet
            </div>
          ) : (
            savedSearches.map((s) => (
              <div key={s.id} className="flex items-center gap-2 px-3 py-2 border-b border-gray-800 hover:bg-gray-800/50 group">
                <div className="flex-1 min-w-0">
                  <div className="text-xs font-medium text-gray-200">{s.name}</div>
                  <div className="text-[10px] text-gray-500">
                    {new Date(s.createdAt).toLocaleString()}
                    {s.criteria.entityType && ` · ${s.criteria.entityType}`}
                    {s.criteria.name && ` · name: ${s.criteria.name}`}
                  </div>
                </div>
                <button
                  onClick={() => loadSearch(s)}
                  className="text-[10px] px-2 py-0.5 bg-blue-800 text-blue-200 hover:bg-blue-700 opacity-0 group-hover:opacity-100 transition-opacity"
                >
                  Load
                </button>
                <button
                  onClick={() => deleteSearch(s.id)}
                  className="text-[10px] px-2 py-0.5 bg-gray-800 text-gray-400 hover:bg-red-900/50 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity"
                >
                  ✕
                </button>
              </div>
            ))
          )}
        </div>
      ) : (
        /* ── Search form + results ── */
        <div className="flex-1 flex flex-col overflow-hidden">
          {/* Form */}
          <div className="flex-shrink-0 overflow-y-auto max-h-80 border-b border-gray-800">
            <div className="p-3 space-y-3">
              {/* Row 1: type + name */}
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">Entity Type</label>
                  <select
                    value={criteria.entityType}
                    onChange={(e) => set('entityType', e.target.value)}
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  >
                    <option value="">Any</option>
                    {ENTITY_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
                  </select>
                </div>
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">Name (fuzzy)</label>
                  <input
                    value={criteria.name}
                    onChange={(e) => set('name', e.target.value)}
                    placeholder="e.g. Maersk…"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
              </div>

              {/* Row 2: MMSI/ICAO + Flag */}
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">MMSI / ICAO</label>
                  <input
                    value={criteria.mmsiOrIcao}
                    onChange={(e) => set('mmsiOrIcao', e.target.value)}
                    placeholder="e.g. 123456789"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
                <div>
                  <label className="block text-[10px] text-gray-500 mb-1">Flag</label>
                  <select
                    value={criteria.flag}
                    onChange={(e) => set('flag', e.target.value)}
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  >
                    <option value="">Any</option>
                    {FLAG_CODES.map((f) => <option key={f} value={f}>{f}</option>)}
                  </select>
                </div>
              </div>

              {/* Row 3: Speed range */}
              <div>
                <label className="block text-[10px] text-gray-500 mb-1">Speed Range (knots)</label>
                <div className="grid grid-cols-2 gap-2">
                  <input
                    value={criteria.speedMin}
                    onChange={(e) => set('speedMin', e.target.value)}
                    placeholder="Min"
                    type="number"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                  <input
                    value={criteria.speedMax}
                    onChange={(e) => set('speedMax', e.target.value)}
                    placeholder="Max"
                    type="number"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
              </div>

              {/* Row 4: Near point */}
              <div>
                <label className="block text-[10px] text-gray-500 mb-1">Near Point</label>
                <div className="grid grid-cols-3 gap-2">
                  <input
                    value={criteria.nearLat}
                    onChange={(e) => set('nearLat', e.target.value)}
                    placeholder="Lat"
                    type="number"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                  <input
                    value={criteria.nearLon}
                    onChange={(e) => set('nearLon', e.target.value)}
                    placeholder="Lon"
                    type="number"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                  <input
                    value={criteria.nearRadiusKm}
                    onChange={(e) => set('nearRadiusKm', e.target.value)}
                    placeholder="km"
                    type="number"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
              </div>

              {/* Row 5: Polygon */}
              <div>
                <label className="block text-[10px] text-gray-500 mb-1">
                  In Polygon (add points)
                </label>
                <PolygonDrawHelper
                  polygon={criteria.polygon}
                  onChange={(pts) => set('polygon', pts)}
                />
              </div>

              {/* Row 6: Time range */}
              <div>
                <label className="block text-[10px] text-gray-500 mb-1">Time Range</label>
                <div className="grid grid-cols-2 gap-2">
                  <input
                    value={criteria.timeFrom}
                    onChange={(e) => set('timeFrom', e.target.value)}
                    type="datetime-local"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                  <input
                    value={criteria.timeTo}
                    onChange={(e) => set('timeTo', e.target.value)}
                    type="datetime-local"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
              </div>

              {/* Row 7: Property filter */}
              <div>
                <label className="block text-[10px] text-gray-500 mb-1">Property Filter</label>
                <div className="grid grid-cols-2 gap-2">
                  <input
                    value={criteria.propertyKey}
                    onChange={(e) => set('propertyKey', e.target.value)}
                    placeholder="key"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                  <input
                    value={criteria.propertyValue}
                    onChange={(e) => set('propertyValue', e.target.value)}
                    placeholder="value"
                    className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
                  />
                </div>
              </div>
            </div>

            {/* Search button */}
            <div className="px-3 pb-3">
              <button
                onClick={() => runSearch(criteria)}
                disabled={loading}
                className="w-full py-2 bg-blue-700 hover:bg-blue-600 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-semibold tracking-wider uppercase transition-colors"
              >
                {loading ? 'Searching…' : 'Run Search'}
              </button>
            </div>
          </div>

          {/* Results header */}
          {(results.length > 0 || error) && (
            <div className="flex-shrink-0 flex items-center gap-2 px-3 py-1.5 bg-gray-900 border-b border-gray-800">
              {error ? (
                <span className="text-red-400 text-[10px]">{error}</span>
              ) : (
                <>
                  <span className="text-[10px] text-gray-400">
                    {sortedResults.length} result{sortedResults.length !== 1 ? 's' : ''}
                    {selectedIds.size > 0 && ` · ${selectedIds.size} selected`}
                  </span>
                  <div className="flex-1" />
                  {/* Sort controls */}
                  {(['name', 'type', 'speed'] as const).map((f) => (
                    <button
                      key={f}
                      onClick={() => { if (sortField === f) setSortAsc((v) => !v); else { setSortField(f); setSortAsc(true); } }}
                      className={`text-[10px] px-1.5 py-0.5 border transition-colors ${sortField === f ? 'border-blue-600 text-blue-400' : 'border-gray-700 text-gray-500 hover:text-gray-300'}`}
                    >
                      {f} {sortField === f ? (sortAsc ? '↑' : '↓') : ''}
                    </button>
                  ))}
                  <div className="h-3 w-px bg-gray-700" />
                  <button onClick={selectAll} className="text-[10px] text-gray-500 hover:text-gray-300">All</button>
                  <button onClick={clearSelection} className="text-[10px] text-gray-500 hover:text-gray-300">None</button>
                  <button
                    onClick={exportCsv}
                    className="text-[10px] px-2 py-0.5 bg-gray-800 text-gray-300 hover:bg-gray-700 border border-gray-700 transition-colors"
                  >
                    CSV
                  </button>
                </>
              )}
            </div>
          )}

          {/* Results list */}
          <div className="flex-1 overflow-y-auto" role="table" aria-label="Search results">
            {loading ? (
              <div className="flex items-center justify-center h-20 text-gray-500 text-xs">
                <span className="animate-pulse">Searching…</span>
              </div>
            ) : sortedResults.length === 0 && !error ? (
              <div className="flex items-center justify-center h-20 text-gray-600 text-xs">
                No results — run a search above
              </div>
            ) : (
              sortedResults.map((entity) => (
                <EntityCard
                  key={entity.id}
                  entity={entity}
                  selected={selectedIds.has(entity.id)}
                  onSelect={handleSelect}
                  onInspect={handleInspect}
                />
              ))
            )}
          </div>
        </div>
      )}

      {/* Save dialog */}
      {saveDialogOpen && (
        <div className="absolute inset-0 bg-black/60 flex items-center justify-center z-50">
          <div className="bg-gray-900 border border-gray-700 p-4 w-72 space-y-3">
            <h3 className="text-xs font-semibold text-gray-200">Save Search</h3>
            <input
              value={saveName}
              onChange={(e) => setSaveName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && saveSearch()}
              placeholder="Search name…"
              autoFocus
              className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
            />
            <div className="flex gap-2">
              <button
                onClick={saveSearch}
                disabled={!saveName.trim()}
                className="flex-1 py-1.5 bg-blue-700 hover:bg-blue-600 disabled:opacity-50 text-white text-xs font-medium transition-colors"
              >
                Save
              </button>
              <button
                onClick={() => { setSaveDialogOpen(false); setSaveName(''); }}
                className="flex-1 py-1.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs font-medium transition-colors border border-gray-700"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
