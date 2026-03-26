import React, { useState, useRef, useEffect, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';
import type { QueryMode, QueryHistoryEntry } from '../types';

const API_BASE = 'http://localhost:9090/api/v1';

// ORP-QL keyword suggestions
const ORPQL_KEYWORDS = [
  'MATCH (e:Ship)',
  'MATCH (e:Port)',
  'MATCH (e:WeatherSystem)',
  'WHERE e.speed > 20',
  'WHERE e.congestion > 0.7',
  'RETURN e.id, e.name, e.properties',
  'LIMIT 100',
  'MATCH (s:Ship)-[:HEADING_TO]->(p:Port)',
  'MATCH (s:Ship)-[:THREATENS]->(w:WeatherSystem)',
  'WHERE distance(e.geometry, POINT(4.27, 51.92)) < 50',
];

const NATURAL_TEMPLATES = [
  'Show all ships near Rotterdam',
  'Find ships moving faster than 20 knots',
  'List ports with high congestion',
  'Show ships heading to Rotterdam',
  'Find all active weather systems',
  'Show ships operated by Maersk',
  'List entities updated in the last hour',
  'Find ships within 50km of port',
];

async function executeQuery(
  query: string,
  mode: QueryMode
): Promise<Array<Record<string, unknown>>> {
  const endpoint =
    mode === 'natural'
      ? `${API_BASE}/query/natural`
      : `${API_BASE}/query`;

  const body =
    mode === 'natural'
      ? JSON.stringify({ query })
      : JSON.stringify({ query });

  const res = await fetch(endpoint, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body,
  });

  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error(
      (err as { error?: { message?: string } })?.error?.message ?? `HTTP ${res.status}`
    );
  }

  const json = await res.json();
  // Normalise: the API returns { data: [...] } or { results: [...] }
  return (json as { data?: unknown[]; results?: unknown[] }).data ??
    (json as { results?: unknown[] }).results ??
    (Array.isArray(json) ? json : [json]);
}

type SortDir = 'asc' | 'desc';

interface ResultsTableProps {
  results: Array<Record<string, unknown>>;
}

function ResultsTable({ results }: ResultsTableProps) {
  const [sortCol, setSortCol] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>('asc');

  if (results.length === 0) return null;

  const columns = Object.keys(results[0]);

  const sorted = sortCol
    ? [...results].sort((a, b) => {
        const av = a[sortCol];
        const bv = b[sortCol];
        const cmp =
          av == null ? -1 : bv == null ? 1 : String(av).localeCompare(String(bv));
        return sortDir === 'asc' ? cmp : -cmp;
      })
    : results;

  const handleSort = (col: string) => {
    if (sortCol === col) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortCol(col);
      setSortDir('asc');
    }
  };

  return (
    <div className="overflow-auto orp-scrollbar max-h-48 border-t border-gray-800">
      <table className="w-full text-[11px]">
        <thead className="sticky top-0 bg-gray-900 z-10">
          <tr>
            {columns.map((col) => (
              <th
                key={col}
                onClick={() => handleSort(col)}
                className="px-3 py-1.5 text-left text-gray-500 font-medium whitespace-nowrap cursor-pointer hover:text-gray-300 select-none border-b border-gray-800"
              >
                {col}
                {sortCol === col && (
                  <span className="ml-1 text-blue-400">
                    {sortDir === 'asc' ? '↑' : '↓'}
                  </span>
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-gray-800/50">
          {sorted.slice(0, 200).map((row, i) => (
            <tr key={i} className="hover:bg-gray-800/40 transition-colors">
              {columns.map((col) => (
                <td key={col} className="px-3 py-1 text-gray-300 whitespace-nowrap font-mono text-[10px]">
                  {row[col] == null
                    ? '—'
                    : typeof row[col] === 'object'
                    ? JSON.stringify(row[col])
                    : String(row[col])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {results.length > 200 && (
        <div className="px-3 py-1 text-[10px] text-gray-600 border-t border-gray-800">
          Showing 200 of {results.length} rows
        </div>
      )}
    </div>
  );
}

export const QueryBar: React.FC = () => {
  const queryMode = useAppStore((s) => s.queryMode);
  const setQueryMode = useAppStore((s) => s.setQueryMode);
  const queryLoading = useAppStore((s) => s.queryLoading);
  const queryError = useAppStore((s) => s.queryError);
  const queryResults = useAppStore((s) => s.queryResults);
  const queryHistory = useAppStore((s) => s.queryHistory);
  const setQueryResults = useAppStore((s) => s.setQueryResults);
  const setQueryLoading = useAppStore((s) => s.setQueryLoading);
  const setQueryError = useAppStore((s) => s.setQueryError);
  const setLastQuery = useAppStore((s) => s.setLastQuery);
  const addQueryHistory = useAppStore((s) => s.addQueryHistory);

  const [query, setQuery] = useState('');
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [activeSuggestion, setActiveSuggestion] = useState(-1);

  const inputRef = useRef<HTMLTextAreaElement>(null);
  const suggestionsRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>();

  // Debounced suggestion generation
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (query.length < 1) {
      setSuggestions([]);
      setShowSuggestions(false);
      return;
    }
    debounceRef.current = setTimeout(() => {
      const q = query.toLowerCase();
      const list =
        queryMode === 'structured'
          ? ORPQL_KEYWORDS.filter((k) => k.toLowerCase().includes(q))
          : NATURAL_TEMPLATES.filter((t) => t.toLowerCase().includes(q));
      setSuggestions(list.slice(0, 8));
      setShowSuggestions(list.length > 0);
      setActiveSuggestion(-1);
    }, 180);
  }, [query, queryMode]);

  const handleSubmit = useCallback(async () => {
    const q = query.trim();
    if (!q || queryLoading) return;

    setQueryLoading(true);
    setQueryError(null);
    setLastQuery(q);
    setShowSuggestions(false);

    try {
      const results = await executeQuery(q, queryMode);
      setQueryResults(results as Array<Record<string, unknown>>);
      addQueryHistory({
        id: `qh-${Date.now()}`,
        query: q,
        mode: queryMode,
        timestamp: new Date(),
        resultCount: results.length,
      });
    } catch (err) {
      setQueryError((err as Error).message);
      setQueryResults([]);
    } finally {
      setQueryLoading(false);
    }
  }, [query, queryMode, queryLoading, setQueryLoading, setQueryError, setLastQuery, setQueryResults, addQueryHistory]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      handleSubmit();
      return;
    }
    if (showSuggestions) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setActiveSuggestion((i) => Math.min(i + 1, suggestions.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setActiveSuggestion((i) => Math.max(i - 1, -1));
      } else if (e.key === 'Enter' && activeSuggestion >= 0) {
        e.preventDefault();
        setQuery(suggestions[activeSuggestion]);
        setShowSuggestions(false);
      } else if (e.key === 'Escape') {
        setShowSuggestions(false);
      }
    }
  };

  const pickHistory = (entry: QueryHistoryEntry) => {
    setQuery(entry.query);
    setQueryMode(entry.mode);
    setShowHistory(false);
    inputRef.current?.focus();
  };

  return (
    <div className="flex flex-col gap-1.5">
      {/* Mode Toggle */}
      <div className="flex items-center gap-1 mb-1">
        {(['structured', 'natural'] as QueryMode[]).map((m) => (
          <button
            key={m}
            onClick={() => setQueryMode(m)}
            className={`text-[10px] px-2 py-0.5 rounded border transition-colors ${
              queryMode === m
                ? 'bg-blue-900/60 border-blue-700 text-blue-300'
                : 'border-gray-700 text-gray-500 hover:text-gray-300 hover:border-gray-600'
            }`}
          >
            {m === 'structured' ? 'ORP-QL' : 'Natural'}
          </button>
        ))}
        {queryHistory.length > 0 && (
          <button
            onClick={() => setShowHistory((v) => !v)}
            className="ml-auto text-[10px] text-gray-600 hover:text-gray-400 transition-colors"
          >
            History ({queryHistory.length})
          </button>
        )}
      </div>

      {/* History dropdown */}
      {showHistory && (
        <div className="rounded-md border border-gray-700 bg-gray-900 divide-y divide-gray-800 mb-1 orp-scrollbar overflow-y-auto max-h-36">
          {queryHistory.map((entry) => (
            <button
              key={entry.id}
              onClick={() => pickHistory(entry)}
              className="w-full text-left px-3 py-1.5 hover:bg-gray-800 transition-colors"
            >
              <div className="text-[10px] text-gray-300 truncate font-mono">{entry.query}</div>
              <div className="text-[9px] text-gray-600 mt-0.5">
                {entry.mode} · {entry.resultCount} results ·{' '}
                {entry.timestamp.toLocaleTimeString()}
              </div>
            </button>
          ))}
        </div>
      )}

      {/* Query Input */}
      <div className="relative">
        <textarea
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          onFocus={() => query.length > 0 && suggestions.length > 0 && setShowSuggestions(true)}
          onBlur={() => setTimeout(() => setShowSuggestions(false), 150)}
          placeholder={
            queryMode === 'structured'
              ? 'MATCH (e:Ship) WHERE e.speed > 20 RETURN e.name LIMIT 100'
              : 'Show ships near Rotterdam moving faster than 15 knots…'
          }
          rows={2}
          disabled={queryLoading}
          className={`w-full bg-gray-800/80 border text-[11px] font-mono rounded-md px-3 py-2 text-gray-200 placeholder-gray-600 resize-none outline-none transition-colors ${
            queryError
              ? 'border-red-700 focus:border-red-500'
              : 'border-gray-700 focus:border-blue-600'
          }`}
          spellCheck={false}
        />
        {/* Syntax hint for ORP-QL */}
        {queryMode === 'structured' && query.length === 0 && (
          <div className="absolute right-2 bottom-2 text-[9px] text-gray-600 pointer-events-none">
            ⌘↵ to run
          </div>
        )}

        {/* Suggestions dropdown */}
        {showSuggestions && suggestions.length > 0 && (
          <div
            ref={suggestionsRef}
            className="absolute left-0 top-full mt-1 z-50 w-full rounded-md border border-gray-700 bg-gray-900 shadow-xl overflow-hidden"
          >
            {suggestions.map((s, i) => (
              <button
                key={s}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setQuery(s);
                  setShowSuggestions(false);
                  inputRef.current?.focus();
                }}
                className={`w-full text-left px-3 py-1.5 text-[10px] font-mono transition-colors ${
                  i === activeSuggestion
                    ? 'bg-blue-900/50 text-blue-200'
                    : 'text-gray-400 hover:bg-gray-800 hover:text-gray-200'
                }`}
              >
                {s}
              </button>
            ))}
          </div>
        )}
      </div>

      {/* Execute button + error */}
      <div className="flex items-center gap-2">
        <button
          onClick={handleSubmit}
          disabled={!query.trim() || queryLoading}
          className="flex items-center gap-1.5 bg-blue-700 hover:bg-blue-600 disabled:bg-gray-800 disabled:text-gray-600 disabled:cursor-not-allowed text-white text-[11px] font-medium px-3 py-1.5 rounded-md transition-colors"
        >
          {queryLoading ? (
            <>
              <span className="w-3 h-3 border-2 border-blue-400 border-t-transparent rounded-full animate-spin" />
              Running…
            </>
          ) : (
            <>Execute <span className="text-[9px] text-blue-300 ml-0.5">⌘↵</span></>
          )}
        </button>
        {queryError && (
          <span className="text-[10px] text-red-400 truncate">{queryError}</span>
        )}
        {!queryError && queryResults.length > 0 && !queryLoading && (
          <span className="text-[10px] text-gray-500">
            {queryResults.length} row{queryResults.length !== 1 ? 's' : ''}
          </span>
        )}
      </div>

      {/* Results table */}
      {queryResults.length > 0 && (
        <div className="mt-1 rounded-md border border-gray-800 overflow-hidden">
          <div className="flex items-center justify-between px-3 py-1 bg-gray-900 border-b border-gray-800">
            <span className="text-[10px] text-gray-500">Results</span>
            <button
              onClick={() => setQueryResults([])}
              className="text-[9px] text-gray-600 hover:text-gray-400 transition-colors"
            >
              ✕
            </button>
          </div>
          <ResultsTable results={queryResults} />
        </div>
      )}
    </div>
  );
};
