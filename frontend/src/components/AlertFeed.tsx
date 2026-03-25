import React from 'react';
import { useAppStore } from '../store/useAppStore';
import { useAlerts } from '../hooks/useEntities';

const SEVERITY_STYLES: Record<string, string> = {
  critical: 'bg-red-900/30 border-red-700 text-red-300',
  warning: 'bg-yellow-900/30 border-yellow-700 text-yellow-300',
  info: 'bg-blue-900/30 border-blue-700 text-blue-300',
};

const SEVERITY_DOT: Record<string, string> = {
  critical: 'bg-red-500',
  warning: 'bg-yellow-500',
  info: 'bg-blue-500',
};

export const AlertFeed: React.FC = () => {
  const alerts = useAppStore((s) => s.alerts);
  const selectEntity = useAppStore((s) => s.selectEntity);
  const { data: serverAlerts } = useAlerts(50);

  // Merge local WS alerts with server-fetched alerts
  const allAlerts = React.useMemo(() => {
    const wsAlerts = alerts;
    const fetched = (serverAlerts?.data ?? []).map((a) => ({
      id: (a.alert_id as string) ?? String(Math.random()),
      monitor_id: (a.rule_id as string) ?? '',
      monitor_name: (a.rule_name as string) ?? '',
      severity: ((a.severity as string) ?? 'info').toLowerCase() as 'info' | 'warning' | 'critical',
      affected_entities: (a.affected_entities as Array<{ entity_id: string; entity_type: string; reason: string }>) ?? [],
      timestamp: (a.timestamp as string) ?? new Date().toISOString(),
      acknowledged: (a.acknowledged as boolean) ?? false,
    }));

    // Deduplicate by id
    const seen = new Set(wsAlerts.map((a) => a.id));
    return [
      ...wsAlerts,
      ...fetched.filter((a) => !seen.has(a.id)),
    ].sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime());
  }, [alerts, serverAlerts]);

  return (
    <div className="flex flex-col h-full">
      <div className="px-3 py-2 border-b border-gray-800 flex items-center justify-between">
        <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide">
          Alerts
        </h3>
        {allAlerts.length > 0 && (
          <span className="text-[10px] bg-red-600 text-white px-1.5 py-0.5 rounded-full font-medium">
            {allAlerts.length}
          </span>
        )}
      </div>
      <div className="flex-1 overflow-y-auto">
        {allAlerts.length === 0 ? (
          <div className="p-3 text-xs text-gray-500 text-center">
            No alerts
          </div>
        ) : (
          <ul className="divide-y divide-gray-800/50">
            {allAlerts.slice(0, 50).map((alert) => (
              <li
                key={alert.id}
                className={`px-3 py-2 border-l-2 ${
                  SEVERITY_STYLES[alert.severity] ?? SEVERITY_STYLES.info
                }`}
              >
                <div className="flex items-start gap-2">
                  <span
                    className={`w-1.5 h-1.5 rounded-full mt-1.5 flex-shrink-0 ${
                      SEVERITY_DOT[alert.severity] ?? SEVERITY_DOT.info
                    }`}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="text-xs font-medium truncate">
                      {alert.monitor_name || 'Alert'}
                    </p>
                    {alert.affected_entities?.length > 0 && (
                      <div className="mt-0.5">
                        {alert.affected_entities.slice(0, 3).map((ae, i) => (
                          <button
                            key={i}
                            onClick={() => selectEntity(ae.entity_id)}
                            className="text-[10px] text-blue-400 hover:text-blue-300 mr-2 underline"
                          >
                            {ae.entity_id}
                          </button>
                        ))}
                      </div>
                    )}
                    <p className="text-[10px] text-gray-500 mt-0.5">
                      {new Date(alert.timestamp).toLocaleTimeString()}
                    </p>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
};
