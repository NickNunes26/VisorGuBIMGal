import type { CoordinateReference, ElementRef, ModelId } from '../../domain/models/ModelRecord';
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
import { ifcParserService } from './IfcParserService';
import { ifcDrawingService } from './IfcDrawingService';
import { bimEngine } from './BimEngine';

export class IfcLiteKernelAdapter implements KernelPort {
  private openModels: Set<ModelId> = new Set();
  private modelBytes: Map<ModelId, Uint8Array> = new Map();
  private modelCoords: Map<ModelId, CoordinateReference> = new Map();

  async openModel(
    modelId: ModelId,
    bytes: ArrayBuffer,
    onProgress?: ProgressHandler
  ): Promise<ModelSummary> {
    const copy = new Uint8Array(bytes.slice(0));
    const summary = await ifcParserService.parseModelBuffer(modelId, bytes, onProgress);
    this.openModels.add(modelId);
    this.modelBytes.set(modelId, copy);
    this.modelCoords.set(modelId, summary.coordinateReference);
    return summary;
  }

  async *streamGeometry(modelId: ModelId): AsyncIterable<GeometryEvent> {
    if (!this.openModels.has(modelId)) {
      throw new KernelError('model-not-open', `El modelo ${modelId} no está abierto.`);
    }

    const bytes = this.modelBytes.get(modelId);
    if (!bytes) {
      yield { type: 'complete', meshCount: 0, diagnostics: [] };
      return;
    }

    try {
      for await (const event of bimEngine.streamModelGeometry(
        modelId,
        bytes,
        () => this.openModels.has(modelId)
      )) {
        yield event;
      }
    } finally {
      this.modelBytes.delete(modelId);
    }
  }

  async getElementDetails(ref: ElementRef): Promise<ElementDetails | null> {
    if (!this.openModels.has(ref.modelId)) return null;
    return ifcParserService.getElementDetails(ref);
  }

  async getSpatialTree(modelId: ModelId): Promise<SpatialTree | null> {
    if (!this.openModels.has(modelId)) return null;
    return ifcParserService.getSpatialTree(modelId);
  }

  async generateDrawing(
    modelIds: readonly ModelId[],
    options: DrawingOptions,
    onProgress?: ProgressHandler
  ): Promise<GeneratedDrawing> {
    const meshes = bimEngine.getMeshesForModels(modelIds);
    let sharedCrs: string | undefined;
    for (const id of modelIds) {
      const coord = this.modelCoords.get(id);
      if (coord?.crsId) {
        sharedCrs = coord.crsId;
        break;
      }
    }
    return ifcDrawingService.generateDrawing(meshes, options, sharedCrs, onProgress);
  }

  async exportDrawing(
    drawingId: DrawingId,
    config: DrawingExportConfig
  ): Promise<ExportedDrawing> {
    return ifcDrawingService.exportDrawing(drawingId, config);
  }

  async discardDrawing(drawingId: DrawingId): Promise<void> {
    return ifcDrawingService.discardDrawing(drawingId);
  }

  async closeModel(modelId: ModelId): Promise<void> {
    this.openModels.delete(modelId);
    this.modelBytes.delete(modelId);
    this.modelCoords.delete(modelId);
    bimEngine.clearPendingBatches(modelId);
    ifcParserService.removeStore(modelId);
  }
}
