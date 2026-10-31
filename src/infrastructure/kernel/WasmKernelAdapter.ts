import type { ElementRef, ModelId } from '../../domain/models/ModelRecord';
import type { DrawingExportConfig, DrawingOptions } from '../../domain/drawings/DrawingTypes';
import {
  KernelError,
  type DrawingId,
  type ElementDetails,
  type ExportedDrawing,
  type GeneratedDrawing,
  type GeometryEvent,
  type KernelPort,
  type ModelSummary,
  type ProgressHandler,
  type SpatialTree
} from '../../domain/kernel/KernelPort';
import type { StreamStep, WorkerRequest, WorkerResponse } from './protocol';

interface Pending {
  resolve(value: unknown): void;
  reject(error: Error): void;
}

const isClosed = (error: unknown) => error instanceof KernelError && error.code === 'model-not-open';

/**
 * One Worker, with its own WASM instance, per model. A crash stays inside its model, and
 * closing a model returns all its memory, which a shared instance could never do: WASM
 * memory only grows.
 */
class ModelWorker {
  private worker = new Worker(new URL('./kernel.worker.ts', import.meta.url), { type: 'module' });
  private nextRequest = 0;
  private pending = new Map<number, Pending>();

  constructor() {
    this.worker.onmessage = ({ data }: MessageEvent<WorkerResponse>) => {
      const pending = this.pending.get(data.id);
      if (!pending) return;
      this.pending.delete(data.id);
      if (data.ok) pending.resolve(data.value);
      else pending.reject(new KernelError(data.code, data.message, data.offset));
    };
    this.worker.onerror = (event) => {
      this.rejectAll(new KernelError('internal', event.message || 'El kernel se ha detenido.'));
    };
  }

  private rejectAll(error: Error): void {
    this.pending.forEach(({ reject }) => reject(error));
    this.pending.clear();
  }

  call<T>(request: WorkerRequest, transfer: Transferable[] = []): Promise<T> {
    const id = this.nextRequest++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as Pending['resolve'], reject });
      this.worker.postMessage({ id, request }, transfer);
    });
  }

  /** Stops at once, even in the middle of evaluating an element. */
  terminate(): void {
    this.worker.terminate();
    this.rejectAll(new KernelError('model-not-open', 'El modelo se ha cerrado.'));
  }
}

/** Our Rust kernel. Drawings arrive with T-050. */
export class WasmKernelAdapter implements KernelPort {
  private workers = new Map<ModelId, ModelWorker>();

  private workerOf(modelId: ModelId): ModelWorker {
    const worker = this.workers.get(modelId);
    if (!worker) throw new KernelError('model-not-open', `El modelo ${modelId} no está abierto.`);
    return worker;
  }

  async openModel(
    modelId: ModelId,
    bytes: ArrayBuffer,
    onProgress?: ProgressHandler
  ): Promise<ModelSummary> {
    if (this.workers.has(modelId)) {
      throw new KernelError('internal', `El modelo ${modelId} ya está abierto.`);
    }
    const worker = new ModelWorker();
    this.workers.set(modelId, worker);
    onProgress?.('Analizando IFC', 0);
    try {
      const summary = await worker.call<ModelSummary>({ op: 'open', modelId, bytes }, [bytes]);
      onProgress?.('Analizando IFC', 100);
      return summary;
    } catch (error) {
      await this.closeModel(modelId);
      throw error;
    }
  }

  async *streamGeometry(modelId: ModelId): AsyncIterable<GeometryEvent> {
    const worker = this.workerOf(modelId);
    try {
      await worker.call({ op: 'start-stream' });
      for (;;) {
        const step = await worker.call<StreamStep>({ op: 'next-batch' });
        if ('complete' in step) {
          yield step.complete;
          return;
        }
        yield { type: 'batch', batch: step.batch };
        yield {
          type: 'progress',
          processedElements: step.processedElements,
          totalElements: step.totalElements
        };
      }
    } catch (error) {
      if (!isClosed(error)) throw error;
    }
  }

  async getElementDetails(ref: ElementRef): Promise<ElementDetails | null> {
    const { modelId, expressId } = ref;
    const worker = this.workers.get(modelId);
    if (!worker) return null;
    try {
      const details = await worker.call<Omit<ElementDetails, 'modelId'> | null>({
        op: 'details',
        expressId
      });
      return details && { ...details, modelId };
    } catch (error) {
      if (isClosed(error)) return null;
      throw error;
    }
  }

  async getSpatialTree(modelId: ModelId): Promise<SpatialTree | null> {
    const worker = this.workers.get(modelId);
    if (!worker) return null;
    try {
      return await worker.call<SpatialTree | null>({ op: 'tree' });
    } catch (error) {
      if (isClosed(error)) return null;
      throw error;
    }
  }

  generateDrawing(
    _modelIds: readonly ModelId[],
    _options: DrawingOptions,
    _onProgress?: ProgressHandler
  ): Promise<GeneratedDrawing> {
    return Promise.reject(new KernelError('internal', 'Planos pendientes de T-050.'));
  }

  exportDrawing(_drawingId: DrawingId, _config: DrawingExportConfig): Promise<ExportedDrawing> {
    return Promise.reject(new KernelError('drawing-not-found', 'Planos pendientes de T-050.'));
  }

  async discardDrawing(_drawingId: DrawingId): Promise<void> {}

  async closeModel(modelId: ModelId): Promise<void> {
    this.workers.get(modelId)?.terminate();
    this.workers.delete(modelId);
  }
}
