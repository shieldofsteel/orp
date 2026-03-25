import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useAppStore } from '../store/useAppStore';
import { useORPQuery } from '../hooks/useEntities';

const QUERY_TEMPLATES = [
  'MATCH (s:Ship) WHERE s.speed > 15 RETURN s.id, s.name, s.speed',
  'MATCH (s:Ship) WHERE NEAR(s, lat=51.9225, lon=4.2706, radius_km=50) RETURN s.id, s.name',
  'MATCH (s:Ship)-[:HEADING_TO]->(p:Port) RETURN s.name, p.name',
  'MATCH (p:Port) RETURN p.id, p.name, p.congestion',
  'MATCH (s:Ship) RETURN COUNT(s) as ship_count, AVG(s.speed) as avg_speed',
];

const ORP_KEYWORDS = ['MATCH', 'WHERE', 'RETURN', 'LIMIT', 'ORDER', 'BY', 'ASC', 'DESC', 'AND', 'OR', 'NEAR', 'WITHIN', 'DISTANCE', 'COUNT', 'SUM', 'AVG', 'MIN', 'MAX'];

export const QueryBar: React.FC = () => {
  const [query, setQuery] = useState('');
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [selectedSuggestionIdx, setSelectedSuggestionIdx] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);

  const queryLoading = useAppStore((s) => s.queryLoading);
  const queryError = useAppStore((s) => s.queryError);
  const setQueryResults = useAppStore((s) => s.setQueryResults);
  const setQueryLoading = useAppStore((s) => s.setQueryLoading);
  const setQueryError = useAppStore((s) => s.setQueryError);
  const setLastQuery = useAppStore((s) => s.setLastQuery);

  const queryMutation = useORPQuery();

  // Autocomplete suggestions
  useEffect(() => {
    if (query.length < 2) {
      setSuggestions([]);
      return;
    }

    const timer = setTimeout(() => {
      const words = query.split(/\s+/);
      const lastWord = words[words.length - 1]?.toUpperCase() ?? '';

      if (lastWord.length >= 1) {
        // Suggest ORP-QL keywords
        const keywordMatches = ORP_KEYWORDS.filter((k) =>
          k.startsWith(lastWord) && k !== lastWord
        );
        // Suggest template queries
        const templateMatches = QUERY_TEMPLATES.filter((t) =>
          t.toLowerCase().includes(query.toLowerCase())
        );
        setSuggestions([...keywordMatches.slice(0, 4), ...templateMatches.slice(0, 3)]);
      } else {
        setSuggestions([]);
      }
    }, 150);

    return () => clearTimeout(timer);
  }, [query]);

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (!query.trim() || queryLoading) return;

      setQueryLoading(true);
      setQueryError(null);
      setLastQuery(query);
      setShowSuggestions(false);

      try {
        const result = await queryMutation.mutateAsync(query);
        setQueryResults(result.results);
      } catch (err) {
        setQueryError(err instanceof Error ? err.message : 'Query failed');
      }
    },
    [query, queryLoading, queryMutation, setQueryResults, setQueryLoading, setQueryError, setLastQuery]
  );

  const handleSuggestionClick = useCallback(
    (suggestion: string) => {
      // If it's a keyword, append it; if it's a full template, replace
      if (ORP_KEYWORDS.includes(suggestion)) {
        const words = query.split(/\s+/);
        words[words.length - 1] = suggestion;
        setQuery(words.join(' ') + ' ');
      } else {
        setQuery(suggestion);
      }
      setShowSuggestions(false);
      inputRef.current?.focus();
    },
    [query]
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!showSuggestions || suggestions.length === 0) return;

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedSuggestionIdx((i) => Math.min(i + 1, suggestions.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedSuggestionIdx((i) => Math.max(i - 1, -1));
      } else if (e.key === 'Enter' && selectedSuggestionIdx >= 0) {
        e.preventDefault();
        handleSuggestionClick(suggestions[selectedSuggestionIdx]);
      } else if (e.key === 'Escape') {
        setShowSuggestions(false);
      }
    },
    [showSuggestions, suggestions, selectedSuggestionIdx, handleSuggestionClick]
  );

  return (
    <div className="relative">
      <form onSubmit={handleSubmit} className="flex gap-2">
        <div className="relative flex-1">
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setShowSuggestions(true);
              setSelectedSuggestionIdx(-1);
            }}
            onFocus={() => setShowSuggestions(true)}
            onBlur={() => setTimeout(() => setShowSuggestions(false), 200)}
            onKeyDown={handleKeyDown}
            placeholder="ORP-QL query… (e.g. MATCH (s:Ship) WHERE s.speed > 15 RETURN s)"
            disabled={queryLoading}
            className="w-full bg-gray-800 text-gray-200 text-sm px-3 py-2 rounded border border-gray-700 focus:border-blue-500 focus:outline-none placeholder-gray-500 font-mono"
            autoComplete="off"
            spellCheck={false}
          />
          {showSuggestions && suggestions.length > 0 && (
            <ul className="absolute top-full left-0 right-0 mt-1 bg-gray-800 border border-gray-700 rounded shadow-lg z-50 max-h-48 overflow-y-auto">
              {suggestions.map((s, i) => (
                <li
                  key={i}
                  onMouseDown={() => handleSuggestionClick(s)}
                  className={`px-3 py-1.5 text-xs cursor-pointer ${
                    i === selectedSuggestionIdx
                      ? 'bg-blue-600 text-white'
                      : 'text-gray-300 hover:bg-gray-700'
                  } ${ORP_KEYWORDS.includes(s) ? 'font-mono text-blue-300' : ''}`}
                >
                  {s}
                </li>
              ))}
            </ul>
          )}
        </div>
        <button
          type="submit"
          disabled={queryLoading || !query.trim()}
          className="px-4 py-2 bg-blue-600 hover:bg-blue-700 disabled:bg-gray-700 disabled:text-gray-500 text-white text-sm font-medium rounded transition-colors whitespace-nowrap"
        >
          {queryLoading ? 'Running…' : 'Execute'}
        </button>
      </form>
      {queryError && (
        <div className="mt-1.5 text-xs text-red-400 bg-red-900/20 px-3 py-1.5 rounded">
          {queryError}
        </div>
      )}
    </div>
  );
};
