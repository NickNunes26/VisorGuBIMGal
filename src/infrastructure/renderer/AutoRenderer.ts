import type { ElementRef, ModelBounds, ModelId } from '../../domain/models/ModelRecord';
import { findBatchError, type GeometryBatch } from '../../domain/kernel/GeometryBatch';
import type {
  RendererEventHandlers,
  RendererPort,
  ViewPreset
} from '../../domain/kernel/RendererPort';
import { WebGpuRenderer, type RendererDebugStats } from './WebGpuRenderer';
import { WebGl2Renderer } from './WebGl2Renderer';

export type BackendType = 'webgpu' | 'webgl2';

export class AutoRenderer implements RendererPort {
  private activeRenderer: (RendererPort & {
    getDebugStats?: () => RendererDebugStats;
    orbitCamera?: (dx: number, dy: number) => void;
    waitForGpuIdle?: () => Promise<void>;
  }) | null = null;
  private activeBackend: BackendType | null = null;
  private handlers: RendererEventHandlers = {};
  private preInitQueue: Array<(r: RendererPort) => void> = [];
  private backendListeners: Set<(backend: BackendType | null) => void> = new Set();

  onBackendChange(listener: (backend: BackendType | null) => void): () => void {
    this.backendListeners.add(listener);
    listener(this.activeBackend);
    return () => this.backendListeners.delete(listener);
  }

  private notifyBackendChange(backend: BackendType | null): void {
    for (const listener of this.backendListeners) {
      try {
        listener(backend);
      } catch (err) {
        console.error('Error in backend listener:', err);
      }
    }
  }

  getActiveBackend(): BackendType | null {
    return this.activeBackend;
  }

  setHandlers(handlers: RendererEventHandlers): void {
    this.handlers = handlers;
    this.activeRenderer?.setHandlers(handlers);
  }

  async initialize(canvas: HTMLCanvasElement): Promise<void> {
    this.destroy();

    let chosenRenderer: (RendererPort & {
      getDebugStats?: () => RendererDebugStats;
      orbitCamera?: (dx: number, dy: number) => void;
      waitForGpuIdle?: () => Promise<void>;
    }) | null = null;
    let chosenBackend: BackendType | null = null;
    let targetCanvas = canvas;

    let canTryWebGpu = false;
    try {
      canTryWebGpu =
        typeof navigator !== 'undefined' && 'gpu' in navigator && !!navigator.gpu;
    } catch {
      canTryWebGpu = false;
    }

    if (canTryWebGpu) {
      const gpuRenderer = new WebGpuRenderer();
      gpuRenderer.setHandlers(this.handlers);
      try {
        await gpuRenderer.initialize(targetCanvas);
        chosenRenderer = gpuRenderer;
        chosenBackend = 'webgpu';
      } catch (gpuErr) {
        console.warn('Fallo al inicializar WebGPU, alternando a WebGL2:', gpuErr);
        gpuRenderer.destroy();

        if (targetCanvas.parentElement) {
          const replacement = document.createElement('canvas');
          replacement.className = targetCanvas.className;
          replacement.tabIndex = targetCanvas.tabIndex;
          replacement.style.cssText = targetCanvas.style.cssText;
          if (targetCanvas.id) replacement.id = targetCanvas.id;
          targetCanvas.parentElement.replaceChild(replacement, targetCanvas);
          targetCanvas = replacement;
        }
      }
    }

    if (!chosenRenderer) {
      const gl2Renderer = new WebGl2Renderer();
      gl2Renderer.setHandlers(this.handlers);
      try {
        await gl2Renderer.initialize(targetCanvas);
        chosenRenderer = gl2Renderer;
        chosenBackend = 'webgl2';
      } catch (gl2Err) {
        gl2Renderer.destroy();
        this.notifyBackendChange(null);
        throw new Error(
          'Ni WebGPU ni WebGL2 están disponibles en este navegador o equipo. Por favor, utiliza un navegador compatible (Chrome o Edge) con aceleración gráfica habilitada.'
        );
      }
    }

    this.activeRenderer = chosenRenderer;
    this.activeBackend = chosenBackend;
    this.notifyBackendChange(chosenBackend);

    // Drain queued operations
    if (this.preInitQueue.length > 0) {
      const queued = this.preInitQueue.splice(0, this.preInitQueue.length);
      for (const op of queued) {
        op(chosenRenderer);
      }
    }

    if (import.meta.env.DEV && typeof window !== 'undefined') {
      (window as unknown as Record<string, unknown>).__gubim_dev__ = {
        renderer: this,
        activeRenderer: chosenRenderer,
        findBatchError
      };
    }
  }

  addModel(modelId: ModelId): void {
    if (this.activeRenderer) {
      this.activeRenderer.addModel(modelId);
    } else {
      this.preInitQueue.push((r) => r.addModel(modelId));
    }
  }

  addBatch(batch: GeometryBatch): void {
    if (this.activeRenderer) {
      this.activeRenderer.addBatch(batch);
    } else {
      this.preInitQueue.push((r) => r.addBatch(batch));
    }
  }

  finishModel(modelId: ModelId, bounds: ModelBounds | undefined): void {
    if (this.activeRenderer) {
      this.activeRenderer.finishModel(modelId, bounds);
    } else {
      this.preInitQueue.push((r) => r.finishModel(modelId, bounds));
    }
  }

  removeModel(modelId: ModelId): void {
    if (this.activeRenderer) {
      this.activeRenderer.removeModel(modelId);
    } else {
      this.preInitQueue.push((r) => r.removeModel(modelId));
    }
  }

  setModelVisibility(modelId: ModelId, visible: boolean): void {
    if (this.activeRenderer) {
      this.activeRenderer.setModelVisibility(modelId, visible);
    } else {
      this.preInitQueue.push((r) => r.setModelVisibility(modelId, visible));
    }
  }

  setElementVisibility(refs: readonly ElementRef[], visible: boolean): void {
    if (this.activeRenderer) {
      this.activeRenderer.setElementVisibility(refs, visible);
    } else {
      this.preInitQueue.push((r) => r.setElementVisibility(refs, visible));
    }
  }

  setSelection(ref: ElementRef | null): void {
    if (this.activeRenderer) {
      this.activeRenderer.setSelection(ref);
    } else {
      this.preInitQueue.push((r) => r.setSelection(ref));
    }
  }

  setViewPreset(preset: ViewPreset): void {
    if (this.activeRenderer) {
      this.activeRenderer.setViewPreset(preset);
    } else {
      this.preInitQueue.push((r) => r.setViewPreset(preset));
    }
  }

  fitToView(): void {
    if (this.activeRenderer) {
      this.activeRenderer.fitToView();
    } else {
      this.preInitQueue.push((r) => r.fitToView());
    }
  }

  destroy(): void {
    this.preInitQueue.length = 0;
    if (this.activeRenderer) {
      this.activeRenderer.destroy();
      this.activeRenderer = null;
    }
    this.activeBackend = null;
    this.notifyBackendChange(null);
  }

  orbitCamera(dx: number, dy: number): void {
    this.activeRenderer?.orbitCamera?.(dx, dy);
  }

  async waitForGpuIdle(): Promise<void> {
    await this.activeRenderer?.waitForGpuIdle?.();
  }

  getDebugStats(): RendererDebugStats {
    if (this.activeRenderer && typeof this.activeRenderer.getDebugStats === 'function') {
      return this.activeRenderer.getDebugStats();
    }
    return {
      modelCount: 0,
      batchCount: 0,
      recordCount: 0,
      liveGpuBuffers: 0,
      allocatedGpuBytes: 0,
      renderCount: 0,
      lastFrameCpuMs: 0
    };
  }
}
