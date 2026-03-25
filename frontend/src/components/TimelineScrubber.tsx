import React, { useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';

export const TimelineScrubber: React.FC = () => {
  const timelineMin = useAppStore((s) => s.timelineMin);
  const timelineMax = useAppStore((s) => s.timelineMax);
  const timelineCurrent = useAppStore((s) => s.timelineCurrent);
  const setTimelineCurrent = useAppStore((s) => s.setTimelineCurrent);

  const range = timelineMax.getTime() - timelineMin.getTime();
  const progress = range > 0
    ? ((timelineCurrent.getTime() - timelineMin.getTime()) / range) * 100
    : 100;

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const pct = parseFloat(e.target.value);
      const newTime = new Date(
        timelineMin.getTime() + (pct / 100) * (timelineMax.getTime() - timelineMin.getTime())
      );
      setTimelineCurrent(newTime);
    },
    [timelineMin, timelineMax, setTimelineCurrent]
  );

  const goToStart = useCallback(() => setTimelineCurrent(timelineMin), [timelineMin, setTimelineCurrent]);
  const goToNow = useCallback(() => setTimelineCurrent(new Date()), [setTimelineCurrent]);

  return (
    <div className="bg-gray-900 border-t border-gray-800 px-4 py-2">
      <div className="flex items-center gap-3">
        <button
          onClick={goToStart}
          className="text-xs text-gray-400 hover:text-white transition-colors px-1.5 py-0.5"
          title="Go to start"
        >
          ◀ Start
        </button>

        <div className="flex-1 flex flex-col gap-0.5">
          <input
            type="range"
            min="0"
            max="100"
            step="0.1"
            value={progress}
            onChange={handleChange}
            className="w-full h-1.5 bg-gray-700 rounded-lg appearance-none cursor-pointer accent-blue-500"
          />
          <div className="flex justify-between text-[10px] text-gray-500">
            <span>{formatShortDate(timelineMin)}</span>
            <span className="text-gray-300 font-medium">
              {formatDateTime(timelineCurrent)}
            </span>
            <span>{formatShortDate(timelineMax)}</span>
          </div>
        </div>

        <button
          onClick={goToNow}
          className="text-xs text-gray-400 hover:text-white transition-colors px-1.5 py-0.5"
          title="Go to now"
        >
          Now ▶
        </button>
      </div>
    </div>
  );
};

function formatShortDate(d: Date): string {
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

function formatDateTime(d: Date): string {
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}
