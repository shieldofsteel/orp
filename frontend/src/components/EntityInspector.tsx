import React from 'react';
import { useAppStore } from '../store/useAppStore';
import { useEntity, useEntityRelationships, useEntityEvents } from '../hooks/useEntities';

export const EntityInspector: React.FC = () => {
  const selectedEntityId = useAppStore((s) => s.selectedEntityId);
  const inspectorOpen = useAppStore((s) => s.inspectorOpen);
  const selectEntity = useAppStore((s) => s.selectEntity);

  const { data: entity, isLoading: entityLoading } = useEntity(selectedEntityId);
  const { data: relationships } = useEntityRelationships(selectedEntityId);
  const { data: events } = useEntityEvents(selectedEntityId);

  if (!inspectorOpen || !selectedEntityId) {
    return null;
  }

  return (
    <div className="w-80 bg-gray-900 border-l border-gray-800 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-gray-800">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold text-white truncate">
            {entityLoading ? 'Loading…' : (entity?.name ?? selectedEntityId)}
          </h2>
          {entity && (
            <span className="text-xs text-gray-400">{entity.type}</span>
          )}
        </div>
        <button
          onClick={() => selectEntity(null)}
          className="text-gray-500 hover:text-white text-lg leading-none ml-2"
          aria-label="Close inspector"
        >
          ×
        </button>
      </div>

      <div className="flex-1 overflow-y-auto">
        {entityLoading ? (
          <div className="p-4 text-gray-500 text-sm">Loading entity…</div>
        ) : entity ? (
          <>
            {/* Properties table */}
            <section className="px-4 py-3 border-b border-gray-800">
              <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
                Properties
              </h3>
              <table className="w-full text-xs">
                <tbody>
                  {Object.entries(entity.properties).map(([key, value]) => (
                    <tr key={key} className="border-b border-gray-800/50">
                      <td className="py-1.5 pr-3 text-gray-400 font-medium whitespace-nowrap align-top">
                        {key}
                      </td>
                      <td className="py-1.5 text-gray-200 break-all">
                        {formatValue(value)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>

            {/* Geometry */}
            {entity.geometry && (
              <section className="px-4 py-3 border-b border-gray-800">
                <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
                  Position
                </h3>
                <p className="text-xs text-gray-300">
                  {entity.geometry.type === 'Point'
                    ? `${(entity.geometry.coordinates as number[])[1]?.toFixed(5)}°N, ${(entity.geometry.coordinates as number[])[0]?.toFixed(5)}°E`
                    : entity.geometry.type}
                </p>
              </section>
            )}

            {/* Relationships */}
            <section className="px-4 py-3 border-b border-gray-800">
              <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
                Relationships ({relationships?.total ?? 0})
              </h3>
              {relationships && relationships.outgoing.length > 0 && (
                <div className="mb-2">
                  <h4 className="text-[10px] text-gray-500 uppercase mb-1">
                    Outgoing ({relationships.outgoing.length})
                  </h4>
                  <ul className="space-y-1">
                    {relationships.outgoing.map((rel) => (
                      <li
                        key={rel.id}
                        className="text-xs text-gray-300 cursor-pointer hover:text-blue-400"
                        onClick={() => selectEntity(rel.target_id)}
                      >
                        <span className="text-blue-400 font-medium">{rel.type}</span>
                        {' → '}
                        {rel.target_name ?? rel.target_id}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {relationships && relationships.incoming.length > 0 && (
                <div>
                  <h4 className="text-[10px] text-gray-500 uppercase mb-1">
                    Incoming ({relationships.incoming.length})
                  </h4>
                  <ul className="space-y-1">
                    {relationships.incoming.map((rel) => (
                      <li
                        key={rel.id}
                        className="text-xs text-gray-300 cursor-pointer hover:text-blue-400"
                        onClick={() => selectEntity(rel.source_id)}
                      >
                        {rel.source_name ?? rel.source_id}
                        {' ← '}
                        <span className="text-blue-400 font-medium">{rel.type}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {(!relationships ||
                (relationships.outgoing.length === 0 &&
                  relationships.incoming.length === 0)) && (
                <p className="text-xs text-gray-500">No relationships</p>
              )}
            </section>

            {/* Data Quality */}
            <section className="px-4 py-3 border-b border-gray-800">
              <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
                Data Quality
              </h3>
              <div className="flex items-center gap-2 mb-1">
                <span className="text-xs text-gray-400">Confidence:</span>
                <div className="flex-1 bg-gray-800 rounded-full h-1.5">
                  <div
                    className="bg-green-500 h-1.5 rounded-full transition-all"
                    style={{ width: `${(entity.confidence ?? 1) * 100}%` }}
                  />
                </div>
                <span className="text-xs text-gray-300">
                  {((entity.confidence ?? 1) * 100).toFixed(0)}%
                </span>
              </div>
              <p className="text-xs text-gray-500">
                Updated: {entity.updated_at ? new Date(entity.updated_at).toLocaleString() : '—'}
              </p>
            </section>

            {/* Event History */}
            <section className="px-4 py-3">
              <h3 className="text-xs font-medium text-gray-400 uppercase tracking-wide mb-2">
                History ({events?.count ?? 0})
              </h3>
              {events && events.data.length > 0 ? (
                <ul className="space-y-1.5 max-h-48 overflow-y-auto">
                  {events.data.slice(0, 20).map((evt, i) => (
                    <li key={i} className="text-xs">
                      <span className="text-gray-500">
                        {evt.timestamp
                          ? new Date(evt.timestamp as string).toLocaleTimeString()
                          : ''}
                      </span>{' '}
                      <span className="text-gray-300">
                        {evt.event_type as string}
                      </span>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-xs text-gray-500">No history available</p>
              )}
            </section>
          </>
        ) : (
          <div className="p-4 text-gray-500 text-sm">Entity not found</div>
        )}
      </div>
    </div>
  );
};

function formatValue(value: unknown): string {
  if (value === null || value === undefined) return '—';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}
