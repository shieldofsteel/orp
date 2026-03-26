import React, { useState, useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';

const SPEEDS = [1, 2, 5, 10] as const;

// Generate fake event density sparkline data (12 buckets)
function generateSparkline(minTime: Date, maxTime: Date): number[] {
  const buckets = 24;
  const data = Array.from({ length: buckets }, () => Math.random());
  // Normalise
  const max = Math.max(...data);
  return data.map((v) => v / max);
}

interface SparklineProps {
  data: number[];
  currentPct: number;
}

function Sparkline({ data, currentPct }: SparklineProps) {
  const width = 100;
  const height = 20;
  const barW = width / data.length;

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="w-full"
      style={{ height: 20 }}
      preserveAspectRatio="none"
    >
      {data.map((v, i) => {
        const x = i * barW;
        const barH = Math.max(1, v * height * 0.9);
        const isPast = (i / data.length) <= currentPct;
        return (
          <rect
            key={i}
            x={x + 0.5}
            y={height - barH}
            width={barW - 1}
            height={barH}
            rx={1}
            fill={isPast ? '#3b82f6' : '#374151'}
            opacity={isPast ? 0.8 : 0.4}
          />
        );
      })}
      {/* Current position indicator */}
      <line
        x1={currentPct * width}
        y1={0}
        x2={currentPct * width}
        y2={height}
        stroke="#60a5fa"
        strokeWidth={1.5}
      />
    </svg>
  );
}

function formatTime(d: Date): string {
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatShort(d: Date): string {
  return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

export const TimelineScrubber: React.FC = () => {
  const timeline = useAppStore((s) => s.timeline);
  const setTimelineCurrent = useAppStore((s) => s.setTimelineCurrent);
  const setTimelinePlaying = useAppStore((s) => s.setTimelinePlaying);
  const setTimelineSpeed = useAppStore((s) => s.setTimelineSpeed);
  const setTimelineRange = useAppStore((s) => s.setTimelineRange);

  const { playing, currentTime, speed, minTime, maxTime } = timeline;

  const [sparkData] = useState(() => generateSparkline(minTime, maxTime));
  const playIntervalRef = useRef<ReturnType<typeof setInterval>>();

  const totalMs = maxTime.getTime() - minTime.getTime();
  const currentPct =
    totalMs > 0
      ? Math.max(0, Math.min(1, (currentTime.getTime() - minTime.getTime()) / totalMs))
      : 0;

  const sliderValue = Math.round(currentPct * 10000);

  const handleSliderChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const pct = parseInt(e.target.value, 10) / 10000;
      const newTime = new Date(minTime.getTime() + pct * totalMs);
      setTimelineCurrent(newTime);
      if (playing) setTimelinePlaying(false);
    },
    [minTime, totalMs, playing, setTimelineCurrent, setTimelinePlaying]
  );

  // Playback animation — advances by speed * 1min per 100ms frame
  useEffect(() => {
    if (!playing) {
      if (playIntervalRef.current) clearInterval(playIntervalRef.current);
      return;
    }
    playIntervalRef.current = setInterval(() => {
      setTimelineCurrent(
        new Date(
          Math.min(
            currentTime.getTime() + speed * 60_000,
            maxTime.getTime()
          )
        )
      );
    }, 100);
    return () => clearInterval(playIntervalRef.current);
  }, [playing, currentTime, speed, maxTime, setTimelineCurrent]);

  // Stop at end
  useEffect(() => {
    if (playing && currentTime.getTime() >= maxTime.getTime()) {
      setTimelinePlaying(false);
    }
  }, [playing, currentTime, maxTime, setTimelinePlaying]);

  const jumpToNow = () => {
    const now = new Date();
    setTimelineRange(new Date(now.getTime() - 24 * 60 * 60 * 1000), now);
    setTimelineCurrent(now);
    setTimelinePlaying(false);
  };

  return (
    <div className="flex-shrink-0 bg-gray-900 border-t border-gray-800 px-4 py-2">
      {/* Sparkline */}
      <div className="mb-1 px-0.5">
        <Sparkline data={sparkData} currentPct={currentPct} />
      </div>

      {/* Controls row */}
      <div className="flex items-center gap-3">
        {/* Play/Pause */}
        <button
          onClick={() => setTimelinePlaying(!playing)}
          className="flex-shrink-0 w-7 h-7 flex items-center justify-center rounded-none bg-gray-800 hover:bg-gray-700 border border-gray-700 text-gray-300 hover:text-white transition-colors"
          aria-label={playing ? 'Pause' : 'Play'}
        >
          {playing ? (
            <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 8 8">
              <rect x="1" y="0.5" width="2" height="7" rx="0.5" />
              <rect x="5" y="0.5" width="2" height="7" rx="0.5" />
            </svg>
          ) : (
            <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 8 8">
              <path d="M1.5 1L6.5 4L1.5 7V1Z" />
            </svg>
          )}
        </button>

        {/* Speed selector */}
        <div className="flex gap-0.5 flex-shrink-0">
          {SPEEDS.map((s) => (
            <button
              key={s}
              onClick={() => setTimelineSpeed(s)}
              className={`text-[9px] w-6 h-5 rounded-none border transition-colors ${
                speed === s
                  ? 'bg-blue-900/60 border-blue-700 text-blue-300'
                  : 'border-gray-700 text-gray-600 hover:text-gray-400 hover:border-gray-600'
              }`}
            >
              {s}×
            </button>
          ))}
        </div>

        {/* Min time label */}
        <span className="text-[9px] text-gray-600 flex-shrink-0 font-mono">
          {formatShort(minTime)}
        </span>

        {/* Slider */}
        <input
          type="range"
          min={0}
          max={10000}
          value={sliderValue}
          onChange={handleSliderChange}
          className="flex-1 orp-range"
          aria-label="Timeline position"
        />

        {/* Max time label */}
        <span className="text-[9px] text-gray-600 flex-shrink-0 font-mono">
          {formatShort(maxTime)}
        </span>

        {/* Current time display */}
        <div className="flex-shrink-0 font-mono text-[10px] text-gray-300 w-36 text-center bg-gray-800/60 border border-gray-700 rounded-none px-2 py-0.5">
          {formatTime(currentTime)}
        </div>

        {/* Now button */}
        <button
          onClick={jumpToNow}
          className="flex-shrink-0 text-[9px] text-gray-500 hover:text-blue-400 border border-gray-700 hover:border-blue-700 rounded-none px-2 py-1 transition-colors"
        >
          NOW
        </button>
      </div>
    </div>
  );
};
