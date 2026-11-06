import type { RendererDebugStats } from '../../domain/kernel/RendererPort';

export interface PerfDisplay {
  frameCpu: string;
  fps: string;
  batches: string;
  pickedBatches: string;
  records: string;
  gpuMemory: string;
  models: string;
  frames: string;
}

export function formatGpuBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unitIdx = 0;
  while (value >= 1024 && unitIdx < units.length - 1) {
    value /= 1024;
    unitIdx++;
  }
  return `${value.toFixed(1)} ${units[unitIdx]}`;
}

export function computeFps(deltaFrames: number, deltaTimeMs: number): number | null {
  if (
    !Number.isFinite(deltaFrames) ||
    !Number.isFinite(deltaTimeMs) ||
    deltaFrames < 0 ||
    deltaTimeMs <= 0
  ) {
    return null;
  }
  if (deltaFrames === 0) return 0;
  return Math.round((deltaFrames * 1000) / deltaTimeMs);
}

export function formatPerfStats(stats: RendererDebugStats, fps: number | null = null): PerfDisplay {
  const cpuMs = Number.isFinite(stats.lastFrameCpuMs) ? Math.max(0, stats.lastFrameCpuMs) : 0;
  return {
    frameCpu: `${cpuMs.toFixed(2)} ms`,
    fps: fps === null ? '—' : fps === 0 ? 'En reposo' : `${fps} FPS`,
    batches: `${stats.drawnBatches} / ${stats.batchCount}`,
    pickedBatches: `${stats.pickedBatches} / ${stats.batchCount}`,
    records: String(stats.recordCount),
    gpuMemory: `${formatGpuBytes(stats.allocatedGpuBytes)} (${stats.liveGpuBuffers} búf.)`,
    models: String(stats.modelCount),
    frames: String(stats.renderCount)
  };
}
