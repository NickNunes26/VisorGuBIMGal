import type { ElementRef, ModelId } from '../../domain/models/ModelRecord';
import type { DrawingExportConfig, DrawingOptions } from '../../domain/drawings/DrawingTypes';
import type {
  DrawingId,
  ElementDetails,
  ExportedDrawing,
  GeneratedDrawing,
  GeometryEvent,
  KernelPort,
  ModelSummary,
  ProgressHandler,
  SpatialTree
} from '../../domain/kernel/KernelPort';

/**
 * Our kernel for everything except drawings, which stay on IFClite until T-050. The file
 * bytes are kept in JS only for that, and IFClite parses them on the first drawing request.
 * Deleted by T-051, along with the rest of this folder.
 */
export class HybridKernelAdapter implements KernelPort {
  private bytes = new Map<ModelId, ArrayBuffer>();
  private openInLegacy = new Set<ModelId>();

  constructor(
    private readonly kernel: KernelPort,
    private readonly legacy: KernelPort
  ) {}

  async openModel(
    modelId: ModelId,
    bytes: ArrayBuffer,
    onProgress?: ProgressHandler
  ): Promise<ModelSummary> {
    const copy = bytes.slice(0);
    const summary = await this.kernel.openModel(modelId, bytes, onProgress);
    this.bytes.set(modelId, copy);
    return summary;
  }

  streamGeometry(modelId: ModelId): AsyncIterable<GeometryEvent> {
    return this.kernel.streamGeometry(modelId);
  }

  getElementDetails(ref: ElementRef): Promise<ElementDetails | null> {
    return this.kernel.getElementDetails(ref);
  }

  getSpatialTree(modelId: ModelId): Promise<SpatialTree | null> {
    return this.kernel.getSpatialTree(modelId);
  }

  async generateDrawing(
    modelIds: readonly ModelId[],
    options: DrawingOptions,
    onProgress?: ProgressHandler
  ): Promise<GeneratedDrawing> {
    const targets = modelIds.length > 0 ? modelIds : Array.from(this.bytes.keys());
    for (const id of targets) {
      const bytes = this.bytes.get(id);
      if (bytes && !this.openInLegacy.has(id)) {
        await this.legacy.openModel(id, bytes.slice(0));
        this.openInLegacy.add(id);
      }
    }
    return this.legacy.generateDrawing(targets, options, onProgress);
  }

  exportDrawing(drawingId: DrawingId, config: DrawingExportConfig): Promise<ExportedDrawing> {
    return this.legacy.exportDrawing(drawingId, config);
  }

  discardDrawing(drawingId: DrawingId): Promise<void> {
    return this.legacy.discardDrawing(drawingId);
  }

  async closeModel(modelId: ModelId): Promise<void> {
    this.bytes.delete(modelId);
    if (this.openInLegacy.delete(modelId)) await this.legacy.closeModel(modelId);
    await this.kernel.closeModel(modelId);
  }
}
