import React from 'react';
import { useAppStore } from '../store/useAppStore';
import { useConnectors, useHealth } from '../hooks/useEntities';
import { AlertFeed } from './AlertFeed';

const STATUS_DOT: Record<string, string> = {
  healthy: 'bg-green-500',
  degraded: 'bg-yellow-500',
  error: 'bg-red-500',
  unhealthy: 'bg-red-500',
};

export const Sidebar: React.FC = () => {
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const wsConnected = useAppStore((s) => s.wsConnected);
  const entities = useAppStore((s) => s.entities);

  const { data: health } = useHealth();
  const { data: connectors } = useConnectors();

  if (!sidebarOpen) return null;

  const healthData = health as Record<string, unknown> | undefined;
  const systemStatus = (healthData?.status as string) ?? 'unknown';
  const version = (healthData?.version as string) ?? '—';
  const uptime = (healthData?.uptime_seconds as number) ?? 0;

  const connectorList = (connectors?.data ?? []) as Array<Record<string, unknown>>;

  return (
    <aside className="w-64 bg-gray-900 border-r border-gray-800 flex flex-col overflow-hidden">
      {/* System Status */}
      <div className="px-3 py-3 border-b border-gray-800">
        <div className="flex items-center gap-2 mb-2">
          <span
            className={`w-2 h-2 rounded-full ${
              STATUS_DOT[systemStatus] ?? 'bg-gray-500'
            }`}
          />
          <span className="text-xs text-gray-300 font-medium">
            ORP {version}
          </span>
        </div>
        <div className="grid grid-cols-2 gap-x-3 gap-y-1 text-[10px]">
          <div className="text-gray-500">Status</div>
          <div className="text-gray-300">{systemStatus}</div>
          <div className="text-gray-500">Uptime</div>
          <div className="text-gray-300">{formatUptime(uptime)}</div>
          <div className="text-gray-500">WebSocket</div>
          <div className={wsConnected ? 'text-green-400' : 'text-red-400'}>
            {wsConnected ? 'Connected' : 'Disconnected'}
          </div>
          <div className="text-gray-500">Entities</div>
          <div className="text-gray-300">{entities.size.toLocaleString()}</div>
        </div>
      </div>

      {/* Data Sources */}
      <div className="px-3 py-2 border-b border-gray-800">
        <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
          Data Sources
        </h3>
        {connectorList.length === 0 ? (
          <p className="text-xs text-gray-500">No connectors configured</p>
        ) : (
          <ul className="space-y-1.5">
            {connectorList.map((c, i) => {
              const name = (c.source_name as string) ?? (c.name as string) ?? `Connector ${i + 1}`;
              const cType = (c.source_type as string) ?? (c.type as string) ?? 'unknown';
              const enabled = c.enabled !== false;
              return (
                <li key={i} className="flex items-center gap-2">
                  <span
                    className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${
                      enabled ? 'bg-green-500' : 'bg-gray-600'
                    }`}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="text-xs text-gray-300 truncate">{name}</p>
                    <p className="text-[10px] text-gray-500">{cType}</p>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {/* Alert Feed fills remaining space */}
      <div className="flex-1 overflow-hidden flex flex-col min-h-0">
        <AlertFeed />
      </div>
    </aside>
  );
};

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
  return `${Math.floor(seconds / 86400)}d ${Math.floor((seconds % 86400) / 3600)}h`;
}
