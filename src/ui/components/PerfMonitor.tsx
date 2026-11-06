import React, { useEffect, useRef, useState } from 'react';
import type { RendererPort } from '../../domain/kernel/RendererPort';
import { computeFps, formatPerfStats, type PerfDisplay } from '../viewer/perfMonitor';

interface PerfMonitorProps {
  renderer: RendererPort;
}

export const PerfMonitor: React.FC<PerfMonitorProps> = ({ renderer }) => {
  const [display, setDisplay] = useState<PerfDisplay>(() =>
    formatPerfStats(renderer.getDebugStats(), null)
  );
  const lastSampleRef = useRef<{ frames: number; time: number } | null>(null);

  useEffect(() => {
    const initial = renderer.getDebugStats();
    const now = performance.now();
    lastSampleRef.current = { frames: initial.renderCount, time: now };
    setDisplay(formatPerfStats(initial, 0));

    const timer = window.setInterval(() => {
      const stats = renderer.getDebugStats();
      const t = performance.now();
      const prev = lastSampleRef.current;
      const fps = prev ? computeFps(stats.renderCount - prev.frames, t - prev.time) : null;
      lastSampleRef.current = { frames: stats.renderCount, time: t };
      setDisplay(formatPerfStats(stats, fps));
    }, 250);

    return () => window.clearInterval(timer);
  }, [renderer]);

  return (
    <div className="perf-monitor-hud" aria-label="Monitor de rendimiento">
      <div className="perf-monitor-title">Rendimiento</div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">Ritmo</span>
        <span className="perf-monitor-val perf-fps">{display.fps}</span>
      </div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">CPU fotograma</span>
        <span className="perf-monitor-val perf-cpu">{display.frameCpu}</span>
      </div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">Lotes dibujados</span>
        <span className="perf-monitor-val perf-batches">{display.batches}</span>
      </div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">Lotes selección</span>
        <span className="perf-monitor-val perf-picked">{display.pickedBatches}</span>
      </div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">Memoria GPU</span>
        <span className="perf-monitor-val perf-gpu">{display.gpuMemory}</span>
      </div>
      <div className="perf-monitor-row">
        <span className="perf-monitor-label">Mallas / Modelos</span>
        <span className="perf-monitor-val perf-records">
          {display.records} / {display.models}
        </span>
      </div>
    </div>
  );
};
