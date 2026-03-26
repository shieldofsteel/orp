import React, { useEffect, useState, useRef, useCallback, useMemo } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useAppStore } from './store/useAppStore';
import { useEntities } from './hooks/useEntities';
import { useEntityTypes } from './hooks/useEntityTypes';
import { useWebSocket } from './hooks/useWebSocket';
import { MapView } from './components/MapView';
import { EntityInspector } from './components/EntityInspector';
import { TimelineScrubber } from './components/TimelineScrubber';
import { Sidebar } from './components/Sidebar';
import { LoginPage } from './components/LoginPage';
import { Dashboard } from './components/Dashboard';
import { SearchPanel } from './components/SearchPanel';
import { QueryConsole } from './components/QueryConsole';

type AppTab = 'map' | 'dashboard' | 'search' | 'query';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 2,
      staleTime: 5000,
      refetchOnWindowFocus: false,
    },
  },
});

// ── Header ─────────────────────────────────────────────────────────────────
function Header({
  onLogout,
  activeTab,
  onTabChange,
  entitySummary,
}: {
  onLogout: () => void;
  activeTab: AppTab;
  onTabChange: (tab: AppTab) => void;
  entitySummary: Array<{ label: string; count: number; colorHex: string }>;
}) {
  const wsConnected = useAppStore((s) => s.wsConnected);
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const setSidebarOpen = useAppStore((s) => s.setSidebarOpen);
  const [darkMode, setDarkMode] = useState(true);

  const toggleDark = () => {
    setDarkMode((v) => !v);
    document.documentElement.classList.toggle('dark');
  };

  const TABS: { id: AppTab; label: string }[] = [
    { id: 'map', label: 'Map' },
    { id: 'dashboard', label: 'Dashboard' },
    { id: 'search', label: 'Search' },
    { id: 'query', label: 'Query' },
  ];

  return (
    <header
      role="banner"
      aria-label="ORP Console application header"
      className="h-11 flex-shrink-0 bg-gray-900 border-b border-gray-800 flex items-center px-4 gap-3 z-20"
    >
      {/* Sidebar toggle */}
      <button
        onClick={() => setSidebarOpen(!sidebarOpen)}
        className="flex flex-col gap-1 p-1.5 rounded-none hover:bg-gray-800 transition-colors text-gray-400 hover:text-gray-200"
        aria-label={sidebarOpen ? 'Close sidebar navigation' : 'Open sidebar navigation'}
        aria-expanded={sidebarOpen}
        aria-controls="sidebar-nav"
      >
        <span className="block w-4 h-px bg-current" aria-hidden="true" />
        <span className="block w-4 h-px bg-current" aria-hidden="true" />
        <span className="block w-4 h-px bg-current" aria-hidden="true" />
      </button>

      {/* Logo / App name */}
      <div className="flex items-center gap-2 select-none" aria-hidden="false">
        <div
          className="flex items-center justify-center w-6 h-6 rounded-none bg-blue-700 text-white text-[10px] font-bold tracking-tight"
          aria-hidden="true"
        >
          ORP
        </div>
        <div className="flex flex-col leading-none">
          {/* h1 for app name — visually styled small but semantically correct */}
          <h1 className="text-xs font-semibold text-gray-100 tracking-tight m-0 p-0 leading-none">
            ORP Console
          </h1>
        </div>
      </div>

      <div className="h-5 w-px bg-gray-800 mx-1" aria-hidden="true" />

      {/* Dynamic entity type counts */}
      {entitySummary.length > 0 && (
        <div className="hidden md:flex items-center gap-2 text-[10px] font-mono">
          {entitySummary.map(({ label, count, colorHex }, i) => (
            <React.Fragment key={label}>
              {i > 0 && <span className="text-gray-700">·</span>}
              <span style={{ color: colorHex }}>
                {count} <span className="text-gray-500">{label.toLowerCase()}{count !== 1 ? 's' : ''}</span>
              </span>
            </React.Fragment>
          ))}
        </div>
      )}

      <div className="h-5 w-px bg-gray-800 mx-1" aria-hidden="true" />

      {/* Tab navigation */}
      <nav aria-label="Application sections" className="flex items-center h-full">
        <span className="text-[10px] text-gray-600 mr-2 hidden sm:inline">Data Fusion /</span>
        <div className="flex items-center h-full" role="tablist">
          {TABS.map((tab) => (
            <button
              key={tab.id}
              role="tab"
              aria-selected={activeTab === tab.id}
              onClick={() => onTabChange(tab.id)}
              className={`h-full px-4 text-[11px] font-medium tracking-wide border-b-2 transition-colors focus:outline-none ${
                activeTab === tab.id
                  ? 'text-blue-400 border-blue-500 bg-blue-950/20'
                  : 'text-gray-500 border-transparent hover:text-gray-300 hover:border-gray-700'
              }`}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </nav>

      <div className="flex-1" />

      {/* Connection status — live region so screen readers hear changes */}
      <div
        aria-live="polite"
        aria-atomic="true"
        className="flex items-center gap-1.5"
        aria-label={wsConnected ? 'WebSocket connected — live updates active' : 'WebSocket disconnected — offline mode'}
      >
        {wsConnected ? (
          <>
            <span className="relative flex h-2 w-2" aria-hidden="true">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-50" />
              <span className="relative inline-flex rounded-full h-2 w-2 bg-green-500" />
            </span>
            <span className="text-[10px] text-green-400">Live</span>
          </>
        ) : (
          <>
            <span className="w-2 h-2 rounded-none bg-red-500" aria-hidden="true" />
            <span className="text-[10px] text-red-400">Offline</span>
          </>
        )}
      </div>

      <div className="h-5 w-px bg-gray-800" aria-hidden="true" />

      {/* Settings gear */}
      <button
        className="w-7 h-7 flex items-center justify-center rounded-none hover:bg-gray-800 text-gray-500 hover:text-gray-300 transition-colors"
        aria-label="Open settings"
        title="Settings"
      >
        <svg
          className="w-3.5 h-3.5"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={1.5}
            d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
          />
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
        </svg>
      </button>

      {/* Dark/light toggle */}
      <button
        onClick={toggleDark}
        className="w-7 h-7 flex items-center justify-center rounded-none hover:bg-gray-800 text-gray-500 hover:text-gray-300 transition-colors"
        aria-label={darkMode ? 'Switch to light mode' : 'Switch to dark mode'}
        aria-pressed={darkMode}
        title={darkMode ? 'Switch to light mode' : 'Switch to dark mode'}
      >
        {darkMode ? (
          <svg
            className="w-3.5 h-3.5"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
          </svg>
        ) : (
          <svg
            className="w-3.5 h-3.5"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
          </svg>
        )}
      </button>

      {/* User menu */}
      <button
        className="flex items-center gap-1.5 rounded-none hover:bg-gray-800 px-2 py-1 transition-colors group"
        aria-label="Open user menu for Operator"
        aria-haspopup="menu"
      >
        <div
          className="w-6 h-6 rounded-none bg-blue-800 border border-blue-700 flex items-center justify-center text-[9px] text-blue-200 font-bold"
          aria-hidden="true"
        >
          OP
        </div>
        <div className="hidden sm:flex flex-col text-left" aria-hidden="true">
          <span className="text-[10px] text-gray-300">Operator</span>
          <span className="text-[9px] text-gray-600">Admin</span>
        </div>
        <svg
          className="w-2.5 h-2.5 text-gray-600 group-hover:text-gray-400"
          fill="currentColor"
          viewBox="0 0 8 8"
          aria-hidden="true"
          focusable="false"
        >
          <path d="M0 2l4 4 4-4H0z" />
        </svg>
      </button>

      {/* Logout button */}
      <button
        onClick={onLogout}
        className="w-7 h-7 flex items-center justify-center rounded-none hover:bg-gray-800 text-gray-500 hover:text-red-400 transition-colors"
        aria-label="Log out"
        title="Log out"
      >
        <svg
          className="w-3.5 h-3.5"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={1.5}
            d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1"
          />
        </svg>
      </button>
    </header>
  );
}

// ── App Content ─────────────────────────────────────────────────────────────
function AppContent({ onLogout }: { onLogout: () => void }) {
  const [activeTab, setActiveTab] = useState<AppTab>('map');
  const setEntities = useAppStore((s) => s.setEntities);
  const entities    = useAppStore((s) => s.entities);
  const inspectorOpen = useAppStore((s) => s.inspectorOpen);
  const setInspectorOpen = useAppStore((s) => s.setInspectorOpen);
  const selectEntity = useAppStore((s) => s.selectEntity);

  // Load all entities without type filter — fully dynamic
  const { data: allData } = useEntities({ limit: 1000 });

  useWebSocket('');

  useEffect(() => {
    const all = allData?.data ?? [];
    if (all.length > 0) setEntities(all);
  }, [allData, setEntities]);

  // Dynamic registry from live entity store
  const registry = useEntityTypes(entities);

  // Build header entity summary: [{label, count, colorHex}]
  const entitySummary = useMemo(() => {
    const summary: Array<{ label: string; count: number; colorHex: string }> = [];
    for (const [type, config] of registry) {
      let count = 0;
      for (const e of entities.values()) {
        if (e.type?.toLowerCase() === type) count++;
      }
      if (count > 0) summary.push({ label: config.label, count, colorHex: config.colorHex });
    }
    return summary;
  }, [registry, entities]);

  // Global keyboard handler: Escape closes inspector
  const handleGlobalKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape' && inspectorOpen) {
        setInspectorOpen(false);
        selectEntity(null);
      }
    },
    [inspectorOpen, setInspectorOpen, selectEntity]
  );

  useEffect(() => {
    document.addEventListener('keydown', handleGlobalKeyDown);
    return () => document.removeEventListener('keydown', handleGlobalKeyDown);
  }, [handleGlobalKeyDown]);

  return (
    <div className="h-screen w-screen flex flex-col bg-gray-950 text-gray-100 overflow-hidden">
      {/* Skip-to-content link — visually hidden until focused */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-blue-700 focus:text-white focus:px-3 focus:py-2 focus:rounded-none focus:text-sm focus:font-medium"
      >
        Skip to main content
      </a>

      <Header onLogout={onLogout} activeTab={activeTab} onTabChange={setActiveTab} entitySummary={entitySummary} />

      <div className="flex flex-1 overflow-hidden">
        {/* Sidebar navigation */}
        <nav
          id="sidebar-nav"
          role="navigation"
          aria-label="ORP sidebar navigation"
          aria-hidden={!useAppStore.getState().sidebarOpen}
        >
          <Sidebar />
        </nav>

        {/* Main content area */}
        <main
          id="main-content"
          role="main"
          aria-label="Operations main content"
          className="flex-1 flex overflow-hidden relative"
          tabIndex={-1}
        >
          {/* Map tab */}
          {activeTab === 'map' && (
            <>
              <MapView />
              {inspectorOpen && (
                <aside role="complementary" aria-label="Entity details inspector">
                  <EntityInspector />
                </aside>
              )}
            </>
          )}

          {/* Dashboard tab */}
          {activeTab === 'dashboard' && (
            <div className="flex-1 overflow-hidden">
              <Dashboard onNavigate={(tab) => setActiveTab(tab as AppTab)} />
            </div>
          )}

          {/* Search tab */}
          {activeTab === 'search' && (
            <div className="flex-1 overflow-hidden relative">
              <SearchPanel />
            </div>
          )}

          {/* Query tab */}
          {activeTab === 'query' && (
            <div className="flex-1 overflow-hidden relative">
              <QueryConsole />
            </div>
          )}
        </main>
      </div>

      <TimelineScrubber />
    </div>
  );
}

export default function App() {
  const [token, setToken] = useState<string | null>(() =>
    localStorage.getItem('orp_token')
  );
  const [devMode, setDevMode] = useState(false);

  // Auto-detect dev mode: if /api/v1/health responds without auth, skip login
  useEffect(() => {
    if (!token) {
      fetch('/api/v1/health')
        .then(r => { if (r.ok) { setDevMode(true); } })
        .catch(() => {});
    }
  }, [token]);

  const handleLogin = useCallback((jwt: string) => {
    setToken(jwt);
    localStorage.setItem('orp_token', jwt);
  }, []);

  const handleLogout = useCallback(() => {
    localStorage.removeItem('orp_token');
    setToken(null);
    setDevMode(false);
  }, []);

  if (!token && !devMode) {
    return <LoginPage onLogin={handleLogin} />;
  }

  return (
    <QueryClientProvider client={queryClient}>
      <AppContent onLogout={handleLogout} />
    </QueryClientProvider>
  );
}
