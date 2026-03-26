import React, {
  useState,
  useRef,
  useCallback,
  useEffect,
  useMemo,
} from 'react';

const API_BASE = '/api/v1';

// ── Types ──────────────────────────────────────────────────────────────────

interface QueryHistoryEntry {
  id: string;
  query: string;
  timestamp: string;
  durationMs: number;
  rowCount: number;
  error?: string;
}

interface SavedQuery {
  id: string;
  name: string;
  query: string;
  savedAt: string;
}

type ResultView = 'table' | 'json' | 'chart' | 'plan';

// ── Syntax Highlighting ────────────────────────────────────────────────────

const KEYWORDS = /\b(MATCH|WHERE|RETURN|NEAR|WITHIN|AND|OR|NOT|LIMIT|ORDER BY|ASC|DESC|LIKE)\b/g;
const IDENTIFIERS = /\b([A-Z][a-zA-Z]*)\b(?=\s*[:(])/g;
const STRINGS = /"([^"\\]|\\.)*"/g;
const NUMBERS = /\b\d+(\.\d+)?\b/g;
const VARIABLES = /\b[a-z_][a-zA-Z0-9_]*(?=\.)/g;
const COMMENTS = /\/\/.*/g;

function highlightORP(code: string): React.ReactNode[] {
  // Build a flat array of styled spans
  type Segment = { start: number; end: number; kind: string };
  const segments: Segment[] = [];

  const push = (re: RegExp, kind: string) => {
    let m: RegExpExecArray | null;
    re.lastIndex = 0;
    while ((m = re.exec(code)) !== null) {
      segments.push({ start: m.index, end: m.index + m[0].length, kind });
    }
  };

  push(COMMENTS, 'comment');
  push(STRINGS, 'string');
  push(KEYWORDS, 'kw');
  push(NUMBERS, 'number');
  push(VARIABLES, 'variable');
  push(IDENTIFIERS, 'type');

  // Remove overlaps — keep first found
  segments.sort((a, b) => a.start - b.start || b.end - a.end);
  const filtered: Segment[] = [];
  let cursor = 0;
  for (const seg of segments) {
    if (seg.start >= cursor) {
      filtered.push(seg);
      cursor = seg.end;
    }
  }

  const colorMap: Record<string, string> = {
    kw: 'text-blue-400 font-semibold',
    type: 'text-amber-400',
    string: 'text-green-400',
    number: 'text-purple-400',
    variable: 'text-cyan-300',
    comment: 'text-gray-500 italic',
  };

  const result: React.ReactNode[] = [];
  let pos = 0;
  for (const seg of filtered) {
    if (pos < seg.start) result.push(code.slice(pos, seg.start));
    result.push(
      <span key={`${seg.start}-${seg.kind}`} className={colorMap[seg.kind] ?? ''}>
        {code.slice(seg.start, seg.end)}
      </span>
    );
    pos = seg.end;
  }
  if (pos < code.length) result.push(code.slice(pos));
  return result;
}

// ── SyntaxEditor ───────────────────────────────────────────────────────────

function SyntaxEditor({
  value,
  onChange,
  onExecute,
}: {
  value: string;
  onChange: (v: string) => void;
  onExecute: () => void;
}) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const highlightRef = useRef<HTMLDivElement>(null);

  const syncScroll = () => {
    if (textareaRef.current && highlightRef.current) {
      highlightRef.current.scrollTop = textareaRef.current.scrollTop;
      highlightRef.current.scrollLeft = textareaRef.current.scrollLeft;
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      onExecute();
    }
    // Tab → 2 spaces
    if (e.key === 'Tab') {
      e.preventDefault();
      const ta = e.currentTarget;
      const start = ta.selectionStart;
      const end = ta.selectionEnd;
      const next = value.slice(0, start) + '  ' + value.slice(end);
      onChange(next);
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = start + 2;
      });
    }
  };

  const lines = value.split('\n');

  return (
    <div className="relative flex-1 font-mono text-xs overflow-hidden" style={{ minHeight: 120 }}>
      {/* Line numbers */}
      <div
        className="absolute left-0 top-0 bottom-0 w-9 bg-gray-900 border-r border-gray-800 text-right pr-2 text-gray-600 select-none overflow-hidden pointer-events-none"
        style={{ lineHeight: '20px', paddingTop: 8, paddingBottom: 8 }}
        aria-hidden="true"
      >
        {lines.map((_, i) => (
          <div key={i} style={{ height: 20 }}>{i + 1}</div>
        ))}
      </div>

      {/* Highlight layer */}
      <div
        ref={highlightRef}
        className="absolute inset-0 pl-11 pr-2 overflow-auto pointer-events-none whitespace-pre-wrap break-words text-gray-200"
        style={{ lineHeight: '20px', paddingTop: 8, paddingBottom: 8 }}
        aria-hidden="true"
      >
        {highlightORP(value)}
      </div>

      {/* Actual textarea */}
      <textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        onScroll={syncScroll}
        spellCheck={false}
        className="absolute inset-0 pl-11 pr-2 bg-transparent text-transparent caret-gray-200 resize-none focus:outline-none overflow-auto w-full h-full"
        style={{ lineHeight: '20px', paddingTop: 8, paddingBottom: 8 }}
        aria-label="ORP-QL query editor"
        placeholder="MATCH (e:Ship) WHERE e.speed > 20 RETURN e LIMIT 100"
      />
    </div>
  );
}

// ── TableView ──────────────────────────────────────────────────────────────

function TableView({ rows }: { rows: Array<Record<string, unknown>> }) {
  const [sortCol, setSortCol] = useState('');
  const [sortAsc, setSortAsc] = useState(true);
  const [filter, setFilter] = useState('');

  const columns = useMemo(() => {
    const cols = new Set<string>();
    rows.forEach((r) => Object.keys(r).forEach((k) => cols.add(k)));
    return Array.from(cols);
  }, [rows]);

  const sorted = useMemo(() => {
    let out = rows;
    if (filter) {
      const lc = filter.toLowerCase();
      out = out.filter((r) =>
        Object.values(r).some((v) => String(v).toLowerCase().includes(lc))
      );
    }
    if (sortCol) {
      out = [...out].sort((a, b) => {
        const av = a[sortCol] ?? '';
        const bv = b[sortCol] ?? '';
        const cmp = String(av).localeCompare(String(bv), undefined, { numeric: true });
        return sortAsc ? cmp : -cmp;
      });
    }
    return out;
  }, [rows, sortCol, sortAsc, filter]);

  const handleSort = (col: string) => {
    if (sortCol === col) setSortAsc((v) => !v);
    else { setSortCol(col); setSortAsc(true); }
  };

  if (rows.length === 0) {
    return <div className="flex items-center justify-center h-24 text-gray-600 text-xs">No rows returned</div>;
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="flex-shrink-0 px-3 py-1.5 border-b border-gray-800 flex items-center gap-2">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter rows…"
          className="bg-gray-800 border border-gray-700 text-gray-200 text-[11px] px-2 py-0.5 focus:outline-none focus:border-blue-600 w-48"
        />
        <span className="text-[10px] text-gray-500">{sorted.length} / {rows.length} rows</span>
      </div>
      <div className="flex-1 overflow-auto">
        <table className="w-full text-[11px] border-collapse">
          <thead className="sticky top-0 bg-gray-900 z-10">
            <tr>
              {columns.map((col) => (
                <th
                  key={col}
                  onClick={() => handleSort(col)}
                  className="text-left px-3 py-1.5 text-gray-400 font-medium border-b border-gray-800 cursor-pointer hover:text-gray-200 whitespace-nowrap"
                >
                  {col}
                  {sortCol === col && <span className="ml-1 text-blue-400">{sortAsc ? '↑' : '↓'}</span>}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sorted.map((row, i) => (
              <tr key={i} className="border-b border-gray-800/60 hover:bg-gray-800/40 transition-colors">
                {columns.map((col) => {
                  const val = row[col];
                  return (
                    <td key={col} className="px-3 py-1.5 text-gray-300 max-w-xs truncate">
                      {val === null || val === undefined ? (
                        <span className="text-gray-600 italic">null</span>
                      ) : typeof val === 'object' ? (
                        <span className="text-amber-400">{JSON.stringify(val)}</span>
                      ) : typeof val === 'number' ? (
                        <span className="text-purple-400">{val}</span>
                      ) : typeof val === 'boolean' ? (
                        <span className={val ? 'text-green-400' : 'text-red-400'}>{String(val)}</span>
                      ) : (
                        String(val)
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ── JsonTree ───────────────────────────────────────────────────────────────

function JsonTree({ data, depth = 0 }: { data: unknown; depth?: number }) {
  const [collapsed, setCollapsed] = useState(depth > 1);

  if (data === null) return <span className="text-gray-500">null</span>;
  if (typeof data === 'boolean') return <span className={data ? 'text-green-400' : 'text-red-400'}>{String(data)}</span>;
  if (typeof data === 'number') return <span className="text-purple-400">{data}</span>;
  if (typeof data === 'string') return <span className="text-green-400">"{data}"</span>;

  if (Array.isArray(data)) {
    if (data.length === 0) return <span className="text-gray-500">[]</span>;
    return (
      <span>
        <button onClick={() => setCollapsed((v) => !v)} className="text-gray-400 hover:text-gray-200 mr-1 font-mono">
          {collapsed ? '▶' : '▼'}
        </button>
        {collapsed ? (
          <span className="text-gray-500">[{data.length} items]</span>
        ) : (
          <div className="pl-4 border-l border-gray-800">
            {data.map((item, i) => (
              <div key={i} className="my-0.5">
                <span className="text-gray-600 mr-1">{i}:</span>
                <JsonTree data={item} depth={depth + 1} />
              </div>
            ))}
          </div>
        )}
      </span>
    );
  }

  if (typeof data === 'object') {
    const keys = Object.keys(data as object);
    if (keys.length === 0) return <span className="text-gray-500">{'{}'}</span>;
    return (
      <span>
        <button onClick={() => setCollapsed((v) => !v)} className="text-gray-400 hover:text-gray-200 mr-1 font-mono">
          {collapsed ? '▶' : '▼'}
        </button>
        {collapsed ? (
          <span className="text-gray-500">{'{'}{keys.length} keys{'}'}</span>
        ) : (
          <div className="pl-4 border-l border-gray-800">
            {keys.map((k) => (
              <div key={k} className="my-0.5">
                <span className="text-cyan-300 mr-1">"{k}":</span>
                <JsonTree data={(data as Record<string, unknown>)[k]} depth={depth + 1} />
              </div>
            ))}
          </div>
        )}
      </span>
    );
  }

  return <span className="text-gray-300">{String(data)}</span>;
}

// ── MiniChart ──────────────────────────────────────────────────────────────

function MiniChart({ rows }: { rows: Array<Record<string, unknown>> }) {
  // Auto-detect numeric columns
  const numericCols = useMemo(() => {
    const cols: string[] = [];
    if (rows.length === 0) return cols;
    Object.keys(rows[0]).forEach((k) => {
      if (typeof rows[0][k] === 'number') cols.push(k);
    });
    return cols;
  }, [rows]);

  const [xCol, setXCol] = useState('');
  const [yCol, setYCol] = useState(numericCols[0] ?? '');

  useEffect(() => {
    if (numericCols.length > 0 && !yCol) setYCol(numericCols[0]);
  }, [numericCols]);

  const allCols = useMemo(() => {
    if (!rows.length) return [];
    return Object.keys(rows[0]);
  }, [rows]);

  if (numericCols.length === 0) {
    return (
      <div className="flex items-center justify-center h-32 text-gray-600 text-xs">
        No numeric columns detected
      </div>
    );
  }

  const labelCol = xCol || allCols[0];
  const values = rows.slice(0, 40).map((r) => ({
    label: String(r[labelCol] ?? ''),
    value: typeof r[yCol] === 'number' ? (r[yCol] as number) : 0,
  }));
  const maxVal = Math.max(...values.map((v) => v.value), 1);

  return (
    <div className="p-3 h-full flex flex-col overflow-hidden">
      {/* Controls */}
      <div className="flex items-center gap-3 mb-3 flex-shrink-0">
        <div className="flex items-center gap-1.5">
          <label className="text-[10px] text-gray-500">X:</label>
          <select
            value={labelCol}
            onChange={(e) => setXCol(e.target.value)}
            className="bg-gray-800 border border-gray-700 text-gray-200 text-[11px] px-1.5 py-0.5 focus:outline-none"
          >
            {allCols.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        </div>
        <div className="flex items-center gap-1.5">
          <label className="text-[10px] text-gray-500">Y:</label>
          <select
            value={yCol}
            onChange={(e) => setYCol(e.target.value)}
            className="bg-gray-800 border border-gray-700 text-gray-200 text-[11px] px-1.5 py-0.5 focus:outline-none"
          >
            {numericCols.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        </div>
      </div>

      {/* Bar chart */}
      <div className="flex-1 overflow-x-auto overflow-y-hidden">
        <div className="flex items-end gap-px h-full" style={{ minWidth: values.length * 20 }}>
          {values.map((v, i) => (
            <div key={i} className="flex flex-col items-center gap-1 flex-1 h-full justify-end group">
              <div
                title={`${v.label}: ${v.value}`}
                className="w-full bg-blue-600 hover:bg-blue-500 transition-colors min-h-[2px]"
                style={{ height: `${(v.value / maxVal) * 80}%` }}
              />
              <span className="text-[8px] text-gray-600 truncate w-full text-center">{v.label}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

// ── QueryConsole ───────────────────────────────────────────────────────────

const DEFAULT_QUERY = 'MATCH (e:Ship)\nWHERE e.speed > 20\nRETURN e.id, e.name, e.properties\nLIMIT 50';

const STARTER_QUERIES = [
  { label: 'Fast Ships', q: 'MATCH (e:Ship) WHERE e.speed > 20 RETURN e.id, e.name, e.properties LIMIT 100' },
  { label: 'Active Ports', q: 'MATCH (e:Port) WHERE e.congestion > 0.5 RETURN e.id, e.name, e.properties LIMIT 50' },
  { label: 'Near Rotterdam', q: 'MATCH (e:Ship) WHERE NEAR(e, 51.9, 4.27, 50) RETURN e.id, e.name, e.properties LIMIT 100' },
  { label: 'Weather Systems', q: 'MATCH (e:WeatherSystem) RETURN e.id, e.name, e.properties LIMIT 50' },
  { label: 'Ship → Port', q: 'MATCH (s:Ship)-[:HEADING_TO]->(p:Port) RETURN s.name, p.name LIMIT 100' },
];

export function QueryConsole() {
  const [query, setQuery] = useState(DEFAULT_QUERY);
  const [results, setResults] = useState<Array<Record<string, unknown>>>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [duration, setDuration] = useState<number | null>(null);
  const [view, setView] = useState<ResultView>('table');
  const [history, setHistory] = useState<QueryHistoryEntry[]>(() => {
    try { return JSON.parse(localStorage.getItem('orp_query_history') ?? '[]'); } catch { return []; }
  });
  const [savedQueries, setSavedQueries] = useState<SavedQuery[]>(() => {
    try { return JSON.parse(localStorage.getItem('orp_saved_queries') ?? '[]'); } catch { return []; }
  });
  const [panel, setPanel] = useState<'history' | 'saved' | 'snippets' | null>(null);
  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [saveName, setSaveName] = useState('');
  const [executionPlan, setExecutionPlan] = useState<unknown>(null);

  const token = localStorage.getItem('orp_token');

  // ── Execute ──────────────────────────────────────────────────────────
  const runQuery = useCallback(async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError('');
    setResults([]);
    setExecutionPlan(null);
    const t0 = performance.now();

    try {
      const res = await fetch(`${API_BASE}/query`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({ query: query.trim() }),
      });

      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(body?.error?.message ?? `HTTP ${res.status}`);
      }

      const data = await res.json();
      const rows: Array<Record<string, unknown>> = Array.isArray(data)
        ? data
        : (data.results ?? data.data ?? []);
      const ms = Math.round(performance.now() - t0);
      setResults(rows);
      setDuration(ms);

      const entry: QueryHistoryEntry = {
        id: crypto.randomUUID(),
        query: query.trim(),
        timestamp: new Date().toISOString(),
        durationMs: ms,
        rowCount: rows.length,
      };
      const newHistory = [entry, ...history].slice(0, 50);
      setHistory(newHistory);
      localStorage.setItem('orp_query_history', JSON.stringify(newHistory));
    } catch (err) {
      const ms = Math.round(performance.now() - t0);
      const msg = err instanceof Error ? err.message : 'Query failed';
      setError(msg);
      setDuration(ms);
      const entry: QueryHistoryEntry = {
        id: crypto.randomUUID(),
        query: query.trim(),
        timestamp: new Date().toISOString(),
        durationMs: ms,
        rowCount: 0,
        error: msg,
      };
      const newHistory = [entry, ...history].slice(0, 50);
      setHistory(newHistory);
      localStorage.setItem('orp_query_history', JSON.stringify(newHistory));
    } finally {
      setLoading(false);
    }
  }, [query, history, token]);

  // ── Explain ──────────────────────────────────────────────────────────
  const runExplain = useCallback(async () => {
    if (!query.trim()) return;
    setLoading(true);
    setError('');
    try {
      const res = await fetch(`${API_BASE}/query/explain`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({ query: query.trim() }),
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const plan = await res.json();
      setExecutionPlan(plan);
      setView('plan');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Explain failed');
    } finally {
      setLoading(false);
    }
  }, [query, token]);

  // ── Export ───────────────────────────────────────────────────────────
  const exportData = (format: 'csv' | 'json' | 'geojson') => {
    if (results.length === 0) return;
    let content: string;
    let filename: string;
    let mime: string;

    if (format === 'json') {
      content = JSON.stringify(results, null, 2);
      filename = 'orp_results.json';
      mime = 'application/json';
    } else if (format === 'geojson') {
      const features = results.filter((r) => r.geometry).map((r) => ({
        type: 'Feature',
        geometry: r.geometry,
        properties: { ...r, geometry: undefined },
      }));
      content = JSON.stringify({ type: 'FeatureCollection', features }, null, 2);
      filename = 'orp_results.geojson';
      mime = 'application/geo+json';
    } else {
      const cols = Array.from(new Set(results.flatMap((r) => Object.keys(r))));
      const lines = results.map((r) =>
        cols.map((c) => `"${String(r[c] ?? '').replace(/"/g, '""')}"`).join(',')
      );
      content = cols.join(',') + '\n' + lines.join('\n');
      filename = 'orp_results.csv';
      mime = 'text/csv';
    }

    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url; a.download = filename; a.click();
    URL.revokeObjectURL(url);
  };

  // ── Save query ────────────────────────────────────────────────────────
  const saveQuery = () => {
    if (!saveName.trim()) return;
    const entry: SavedQuery = {
      id: crypto.randomUUID(),
      name: saveName.trim(),
      query: query.trim(),
      savedAt: new Date().toISOString(),
    };
    const updated = [entry, ...savedQueries];
    setSavedQueries(updated);
    localStorage.setItem('orp_saved_queries', JSON.stringify(updated));
    setSaveDialogOpen(false);
    setSaveName('');
  };

  const deleteSaved = (id: string) => {
    const updated = savedQueries.filter((s) => s.id !== id);
    setSavedQueries(updated);
    localStorage.setItem('orp_saved_queries', JSON.stringify(updated));
  };

  // ── Render ────────────────────────────────────────────────────────────
  return (
    <div className="h-full flex flex-col bg-gray-950 text-gray-200 overflow-hidden">
      {/* Header toolbar */}
      <div className="flex-shrink-0 flex items-center gap-2 px-3 py-2 bg-gray-900 border-b border-gray-800">
        <span className="text-[10px] font-semibold text-gray-400 tracking-widest uppercase mr-1">ORP-QL</span>
        <button
          onClick={runQuery}
          disabled={loading}
          className="flex items-center gap-1.5 px-3 py-1 bg-blue-700 hover:bg-blue-600 disabled:opacity-50 text-white text-[11px] font-semibold transition-colors"
        >
          <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
          {loading ? 'Running…' : 'Run'}
          <span className="text-blue-300 text-[9px]">⌘↵</span>
        </button>
        <button
          onClick={runExplain}
          disabled={loading}
          className="px-2.5 py-1 border border-gray-700 text-gray-400 hover:border-amber-600 hover:text-amber-400 text-[11px] transition-colors"
        >
          Explain
        </button>
        <button
          onClick={() => setSaveDialogOpen(true)}
          className="px-2.5 py-1 border border-gray-700 text-gray-400 hover:border-gray-500 hover:text-gray-200 text-[11px] transition-colors"
        >
          Save
        </button>
        <div className="flex-1" />

        {/* Side panel toggles */}
        {(['history', 'saved', 'snippets'] as const).map((p) => (
          <button
            key={p}
            onClick={() => setPanel(panel === p ? null : p)}
            className={`px-2.5 py-1 border text-[11px] transition-colors ${
              panel === p ? 'border-blue-600 text-blue-400 bg-blue-900/20' : 'border-gray-700 text-gray-500 hover:text-gray-300'
            }`}
          >
            {p === 'history' ? `History (${history.length})` : p === 'saved' ? `Library (${savedQueries.length})` : 'Snippets'}
          </button>
        ))}
      </div>

      <div className="flex-1 flex overflow-hidden">
        {/* Editor + results */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {/* Editor pane */}
          <div className="flex-shrink-0 border-b border-gray-800 bg-gray-900" style={{ height: 200 }}>
            <SyntaxEditor
              value={query}
              onChange={setQuery}
              onExecute={runQuery}
            />
          </div>

          {/* Status bar */}
          <div className="flex-shrink-0 flex items-center gap-3 px-3 py-1 bg-gray-900/80 border-b border-gray-800 text-[10px]">
            {loading ? (
              <span className="text-blue-400 animate-pulse">Executing…</span>
            ) : error ? (
              <span className="text-red-400 truncate">{error}</span>
            ) : results.length > 0 ? (
              <>
                <span className="text-green-400">{results.length} rows</span>
                {duration !== null && <span className="text-gray-500">{duration}ms</span>}
              </>
            ) : (
              <span className="text-gray-600">Ready — Ctrl+Enter to run</span>
            )}
            <div className="flex-1" />
            {results.length > 0 && (
              <div className="flex items-center gap-1">
                {/* View tabs */}
                {(['table', 'json', 'chart', 'plan'] as const).map((v) => (
                  <button
                    key={v}
                    onClick={() => setView(v)}
                    className={`px-2 py-0.5 transition-colors ${
                      view === v ? 'text-blue-400 border-b border-blue-500' : 'text-gray-500 hover:text-gray-300'
                    }`}
                  >
                    {v.charAt(0).toUpperCase() + v.slice(1)}
                  </button>
                ))}
                <div className="w-px h-3 bg-gray-700 mx-1" />
                {/* Exports */}
                {(['csv', 'json', 'geojson'] as const).map((fmt) => (
                  <button
                    key={fmt}
                    onClick={() => exportData(fmt)}
                    className="text-gray-600 hover:text-gray-300 uppercase transition-colors"
                  >
                    {fmt}
                  </button>
                ))}
              </div>
            )}
          </div>

          {/* Results pane */}
          <div className="flex-1 overflow-hidden bg-gray-950">
            {error ? (
              <div className="p-4">
                <div className="bg-red-950/40 border border-red-900 text-red-400 text-xs px-3 py-2 font-mono">
                  {error}
                </div>
              </div>
            ) : loading ? (
              <div className="flex items-center justify-center h-full">
                <div className="flex flex-col items-center gap-2 text-gray-500 text-xs">
                  <div className="w-5 h-5 border border-blue-600 border-t-transparent animate-spin" />
                  Executing query…
                </div>
              </div>
            ) : view === 'plan' && executionPlan ? (
              <div className="p-4 font-mono text-[11px] text-gray-300 overflow-auto h-full">
                <div className="text-amber-400 font-semibold mb-2">Execution Plan</div>
                <JsonTree data={executionPlan} />
              </div>
            ) : view === 'json' ? (
              <div className="p-4 font-mono text-[11px] overflow-auto h-full">
                <JsonTree data={results} />
              </div>
            ) : view === 'chart' ? (
              <MiniChart rows={results} />
            ) : (
              <TableView rows={results} />
            )}
          </div>
        </div>

        {/* Side panel */}
        {panel && (
          <div className="w-72 flex-shrink-0 border-l border-gray-800 flex flex-col overflow-hidden bg-gray-900">
            {panel === 'history' && (
              <>
                <div className="flex-shrink-0 px-3 py-2 border-b border-gray-800 flex items-center justify-between">
                  <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Query History</span>
                  <button
                    onClick={() => { setHistory([]); localStorage.removeItem('orp_query_history'); }}
                    className="text-[10px] text-gray-600 hover:text-red-400"
                  >
                    Clear
                  </button>
                </div>
                <div className="flex-1 overflow-y-auto">
                  {history.length === 0 ? (
                    <div className="flex items-center justify-center h-20 text-gray-600 text-xs">No history</div>
                  ) : (
                    history.map((h) => (
                      <div
                        key={h.id}
                        className="px-3 py-2 border-b border-gray-800 hover:bg-gray-800/50 cursor-pointer group"
                        onClick={() => setQuery(h.query)}
                      >
                        <div className="text-[10px] text-gray-500">
                          {new Date(h.timestamp).toLocaleTimeString()}
                          {' · '}{h.durationMs}ms
                          {h.error ? (
                            <span className="text-red-400 ml-1">ERR</span>
                          ) : (
                            <span className="text-green-400 ml-1">{h.rowCount}r</span>
                          )}
                        </div>
                        <div className="text-[10px] text-gray-400 truncate mt-0.5 font-mono">{h.query}</div>
                      </div>
                    ))
                  )}
                </div>
              </>
            )}

            {panel === 'saved' && (
              <>
                <div className="flex-shrink-0 px-3 py-2 border-b border-gray-800">
                  <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Saved Queries</span>
                </div>
                <div className="flex-1 overflow-y-auto">
                  {savedQueries.length === 0 ? (
                    <div className="flex items-center justify-center h-20 text-gray-600 text-xs">No saved queries</div>
                  ) : (
                    savedQueries.map((s) => (
                      <div
                        key={s.id}
                        className="px-3 py-2 border-b border-gray-800 hover:bg-gray-800/50 group"
                      >
                        <div className="flex items-center justify-between">
                          <span
                            className="text-xs text-gray-200 cursor-pointer hover:text-blue-400"
                            onClick={() => { setQuery(s.query); setPanel(null); }}
                          >
                            {s.name}
                          </span>
                          <button
                            onClick={() => deleteSaved(s.id)}
                            className="text-[10px] text-gray-600 hover:text-red-400 opacity-0 group-hover:opacity-100"
                          >
                            ✕
                          </button>
                        </div>
                        <div className="text-[9px] text-gray-600">{new Date(s.savedAt).toLocaleDateString()}</div>
                      </div>
                    ))
                  )}
                </div>
              </>
            )}

            {panel === 'snippets' && (
              <>
                <div className="flex-shrink-0 px-3 py-2 border-b border-gray-800">
                  <span className="text-[10px] font-semibold text-gray-400 uppercase tracking-wider">Starter Queries</span>
                </div>
                <div className="flex-1 overflow-y-auto">
                  {STARTER_QUERIES.map((s) => (
                    <div
                      key={s.label}
                      className="px-3 py-2 border-b border-gray-800 hover:bg-gray-800/50 cursor-pointer"
                      onClick={() => { setQuery(s.q); setPanel(null); }}
                    >
                      <div className="text-xs text-blue-400 font-medium">{s.label}</div>
                      <div className="text-[10px] text-gray-500 font-mono truncate mt-0.5">{s.q}</div>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Save dialog */}
      {saveDialogOpen && (
        <div className="absolute inset-0 bg-black/60 flex items-center justify-center z-50">
          <div className="bg-gray-900 border border-gray-700 p-4 w-72 space-y-3">
            <h3 className="text-xs font-semibold text-gray-200">Save Query</h3>
            <input
              value={saveName}
              onChange={(e) => setSaveName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && saveQuery()}
              placeholder="Query name…"
              autoFocus
              className="w-full bg-gray-800 border border-gray-700 text-gray-200 text-xs px-2 py-1.5 focus:outline-none focus:border-blue-600"
            />
            <div className="flex gap-2">
              <button
                onClick={saveQuery}
                disabled={!saveName.trim()}
                className="flex-1 py-1.5 bg-blue-700 hover:bg-blue-600 disabled:opacity-50 text-white text-xs font-medium"
              >
                Save
              </button>
              <button
                onClick={() => { setSaveDialogOpen(false); setSaveName(''); }}
                className="flex-1 py-1.5 bg-gray-800 hover:bg-gray-700 text-gray-300 text-xs font-medium border border-gray-700"
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
