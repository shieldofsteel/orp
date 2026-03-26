import React, { useEffect, useState } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useAppStore } from './store/useAppStore';
import { useEntities } from './hooks/useEntities';
import { useWebSocket } from './hooks/useWebSocket';
import { MapView } from './components/MapView';
import { EntityInspector } from './components/EntityInspector';
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

// ── Header ─────────────────────────────────────────────────────────────────
function Header() {
  const wsConnected = useAppStore((s) => s.wsConnected);
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const setSidebarOpen = useAppStore((s) => s.setSidebarOpen);
  const [darkMode, setDarkMode] = useState(true);

  const toggleDark = () => {
    setDarkMode((v) => !v);
    document.documentElement.classList.toggle('dark');
  };

  return (
    <header className="h-11 flex-shrink-0 bg-gray-900 border-b border-gray-800 flex items-center px-4 gap-3 z-20">
      {/* Sidebar toggle */}
      <button
        onClick={() => setSidebarOpen(!sidebarOpen)}
        className="flex flex-col gap-1 p-1.5 rounded hover:bg-gray-800 transition-colors text-gray-400 hover:text-gray-200"
        aria-label="Toggle sidebar"
      >
        <span className="block w-4 h-px bg-current" />
        <span className="block w-4 h-px bg-current" />
        <span className="block w-4 h-px bg-current" />
      </button>

      {/* Logo */}
      <div className="flex items-center gap-2 select-none">
        <div className="flex items-center justify-center w-6 h-6 rounded bg-blue-700 text-white text-[10px] font-bold tracking-tight">
          ORP
        </div>
        <div className="flex flex-col leading-none">
          <span className="text-xs font-semibold text-gray-100 tracking-tight">
            ORP Console
          </span>
          <span className="text-[9px] text-gray-600">Maritime Operations</span>
        </div>
      </div>

      <div className="h-5 w-px bg-gray-800 mx-1" />

      {/* Breadcrumb / status */}
      <div className="flex items-center gap-1.5 text-[10px] text-gray-500">
        <span>Data Fusion</span>
        <span className="text-gray-700">/</span>
        <span className="text-gray-400">Map View</span>
      </div>

      <div className="flex-1" />

      {/* Connection status */}
      <div className="flex items-center gap-1.5">
        {wsConnected ? (
          <>
            <span className="relative flex h-2 w-2">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-50" />
              <span className="relative inline-flex rounded-full h-2 w-2 bg-green-500" />
            </span>
            <span className="text-[10px] text-green-400">Live</span>
          </>
        ) : (
          <>
            <span className="w-2 h-2 rounded-full bg-red-500" />
            <span className="text-[10px] text-red-400">Offline</span>
          </>
        )}
      </div>

      <div className="h-5 w-px bg-gray-800" />

      {/* Settings gear */}
      <button
        className="w-7 h-7 flex items-center justify-center rounded hover:bg-gray-800 text-gray-500 hover:text-gray-300 transition-colors"
        aria-label="Settings"
        title="Settings"
      >
        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
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
        className="w-7 h-7 flex items-center justify-center rounded hover:bg-gray-800 text-gray-500 hover:text-gray-300 transition-colors"
        aria-label="Toggle theme"
        title={darkMode ? 'Switch to light mode' : 'Switch to dark mode'}
      >
        {darkMode ? (
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
          </svg>
        ) : (
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
          </svg>
        )}
      </button>

      {/* User menu */}
      <button
        className="flex items-center gap-1.5 rounded hover:bg-gray-800 px-2 py-1 transition-colors group"
        aria-label="User menu"
      >
        <div className="w-6 h-6 rounded-full bg-blue-800 border border-blue-700 flex items-center justify-center text-[9px] text-blue-200 font-bold">
          OP
        </div>
        <div className="hidden sm:flex flex-col text-left">
          <span className="text-[10px] text-gray-300">Operator</span>
          <span className="text-[9px] text-gray-600">Admin</span>
        </div>
        <svg className="w-2.5 h-2.5 text-gray-600 group-hover:text-gray-400" fill="currentColor" viewBox="0 0 8 8">
          <path d="M0 2l4 4 4-4H0z" />
        </svg>
      </button>
    </header>
  );
}

// ── App Content ─────────────────────────────────────────────────────────────
function AppContent() {
  const setEntities = useAppStore((s) => s.setEntities);

  const { data: shipData } = useEntities({ type: 'Ship', limit: 500 });
  const { data: portData } = useEntities({ type: 'Port', limit: 200 });

  useWebSocket('Ship');

  useEffect(() => {
    const all = [...(shipData?.data ?? []), ...(portData?.data ?? [])];
    if (all.length > 0) setEntities(all);
  }, [shipData, portData, setEntities]);

  return (
    <div className="h-screen w-screen flex flex-col bg-gray-950 text-gray-100 overflow-hidden">
      <Header />

      <div className="flex flex-1 overflow-hidden">
        <Sidebar />

        {/* Map + Inspector */}
        <div className="flex-1 flex overflow-hidden relative">
          <MapView />
          <EntityInspector />
        </div>
      </div>

      <TimelineScrubber />
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
