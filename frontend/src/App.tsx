import React, { useEffect } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useAppStore } from './store/useAppStore';
import { useEntities } from './hooks/useEntities';
import { useWebSocket } from './hooks/useWebSocket';
import { MapView } from './components/MapView';
import { EntityInspector } from './components/EntityInspector';
import { QueryBar } from './components/QueryBar';
import { TimelineScrubber } from './components/TimelineScrubber';
import { Sidebar } from './components/Sidebar';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 2,
      staleTime: 5000,
      refetchOnWindowFocus: false,
    },
  },
});

function AppContent() {
  const setEntities = useAppStore((s) => s.setEntities);
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const setSidebarOpen = useAppStore((s) => s.setSidebarOpen);
  const queryResults = useAppStore((s) => s.queryResults);

  // Fetch entities
  const { data: shipData } = useEntities({ type: 'Ship', limit: 500 });
  const { data: portData } = useEntities({ type: 'Port', limit: 200 });

  // WebSocket for real-time updates
  useWebSocket('Ship');

  // Sync fetched entities into store
  useEffect(() => {
    const all = [
      ...(shipData?.data ?? []),
      ...(portData?.data ?? []),
    ];
    if (all.length > 0) {
      setEntities(all);
    }
  }, [shipData, portData, setEntities]);

  return (
    <div className="h-screen w-screen flex flex-col bg-gray-950 text-white overflow-hidden">
      {/* Header */}
      <header className="h-11 flex-shrink-0 bg-gray-900 border-b border-gray-800 flex items-center px-4 gap-4">
        <button
          onClick={() => setSidebarOpen(!sidebarOpen)}
          className="text-gray-400 hover:text-white text-sm"
          aria-label="Toggle sidebar"
        >
          ☰
        </button>
        <div className="flex items-center gap-2">
          <span className="text-blue-400 font-bold text-sm tracking-wide">ORP</span>
          <span className="text-gray-500 text-xs">Data Fusion Console</span>
        </div>
        <div className="flex-1 max-w-2xl mx-auto">
          <QueryBar />
        </div>
        <div className="flex items-center gap-3">
          <ConnectionIndicator />
        </div>
      </header>

      {/* Main content area */}
      <div className="flex flex-1 overflow-hidden">
        {/* Sidebar */}
        <Sidebar />

        {/* Map + Inspector */}
        <div className="flex-1 flex overflow-hidden">
          <MapView />
          <EntityInspector />
        </div>
      </div>

      {/* Query Results Panel (if results exist) */}
      {queryResults.length > 0 && <QueryResultsPanel results={queryResults} />}

      {/* Timeline Scrubber */}
      <TimelineScrubber />
    </div>
  );
}

function ConnectionIndicator() {
  const wsConnected = useAppStore((s) => s.wsConnected);
  return (
    <div className="flex items-center gap-1.5">
      <span
        className={`w-2 h-2 rounded-full ${
          wsConnected ? 'bg-green-500' : 'bg-red-500'
        }`}
      />
      <span className="text-[10px] text-gray-400">
        {wsConnected ? 'Live' : 'Offline'}
      </span>
    </div>
  );
}

function QueryResultsPanel({ results }: { results: Array<Record<string, unknown>> }) {
  const setQueryResults = useAppStore((s) => s.setQueryResults);

  if (results.length === 0) return null;

  const columns = Object.keys(results[0]);

  return (
    <div className="h-48 flex-shrink-0 bg-gray-900 border-t border-gray-800 flex flex-col">
      <div className="flex items-center justify-between px-4 py-1.5 border-b border-gray-800">
        <span className="text-xs text-gray-400">
          Query Results ({results.length} rows)
        </span>
        <button
          onClick={() => setQueryResults([])}
          className="text-gray-500 hover:text-white text-xs"
        >
          ✕ Close
        </button>
      </div>
      <div className="flex-1 overflow-auto">
        <table className="w-full text-xs">
          <thead className="bg-gray-800/50 sticky top-0">
            <tr>
              {columns.map((col) => (
                <th
                  key={col}
                  className="px-3 py-1.5 text-left text-gray-400 font-medium whitespace-nowrap"
                >
                  {col}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-800/50">
            {results.slice(0, 200).map((row, i) => (
              <tr key={i} className="hover:bg-gray-800/30">
                {columns.map((col) => (
                  <td key={col} className="px-3 py-1 text-gray-300 whitespace-nowrap">
                    {row[col] === null || row[col] === undefined
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
      </div>
    </div>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <AppContent />
    </QueryClientProvider>
  );
}
