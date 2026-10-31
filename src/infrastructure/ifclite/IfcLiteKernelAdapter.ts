import { GeometryProcessor, type MeshData } from '@ifc-lite/geometry';
import type {
  CoordinateReference,
  ElementRef,
  ModelBounds,
  ModelId
} from '../../domain/models/ModelRecord';
import type { DrawingExportConfig, DrawingOptions } from '../../domain/drawings/DrawingTypes';
import {
  MESH_RECORD_WORDS,
  MeshField,
  MeshFlag,
  type GeometryBatch
} from '../../domain/kernel/GeometryBatch';
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

function packMeshesToBatch(
  modelId: ModelId,
  sequence: number,
  meshes: readonly MeshData[],
  boundsAcc: {
    minX: number;
    minY: number;
    minZ: number;
    maxX: number;
    maxY: number;
    maxZ: number;
    hasVisibleBounds: boolean;
  }
): GeometryBatch | null {
  const validMeshes = meshes.filter(
    (m) =>
      m.expressId > 0 &&
      m.positions.length > 0 &&
      m.positions.length % 3 === 0 &&
      m.normals.length === m.positions.length &&
      m.indices.length > 0 &&
      m.indices.length % 3 === 0
  );

  if (validMeshes.length === 0) return null;

  let totalFloats = 0;
  let totalIndices = 0;
  for (const m of validMeshes) {
    totalFloats += m.positions.length;
    totalIndices += m.indices.length;
  }

  const positions = new Float32Array(totalFloats);
  const normals = new Float32Array(totalFloats);
  const indices = new Uint32Array(totalIndices);
  const meshTable = new Uint32Array(validMeshes.length * MESH_RECORD_WORDS);

  let vertexCursor = 0;
  let indexCursor = 0;

  for (let r = 0; r < validMeshes.length; r++) {
    const m = validMeshes[r];
    const vCount = m.positions.length / 3;
    const iCount = m.indices.length;
    const ox = m.origin?.[0] ?? 0;
    const oy = m.origin?.[1] ?? 0;
    const oz = m.origin?.[2] ?? 0;

    const ifcType = (m as { ifcType?: string }).ifcType;
    const isHiddenByDefault = ifcType === 'IfcSpace' || ifcType === 'IfcOpeningElement';

    const rCol = Math.max(0, Math.min(255, Math.round((m.color?.[0] ?? 0.8) * 255)));
    const gCol = Math.max(0, Math.min(255, Math.round((m.color?.[1] ?? 0.8) * 255)));
    const bCol = Math.max(0, Math.min(255, Math.round((m.color?.[2] ?? 0.8) * 255)));
    const aCol = Math.max(0, Math.min(255, Math.round((m.color?.[3] ?? 1.0) * 255)));
    const rgba = (((rCol << 24) | (gCol << 16) | (bCol << 8) | aCol) >>> 0);

    let flags = 0;
    if (aCol < 255) flags |= MeshFlag.Transparent;
    if (isHiddenByDefault) flags |= MeshFlag.HiddenByDefault;

    const floatBase = vertexCursor * 3;
    for (let v = 0; v < vCount; v++) {
      const srcIdx = v * 3;
      const dstIdx = floatBase + srcIdx;

      // Convert from @ifc-lite/geometry Y-up (x_ifc, z_ifc, -y_ifc) back to IFC Z-up (x, y, z)
      const xIfc = ox + m.positions[srcIdx];
      const yIfc = -(oz + m.positions[srcIdx + 2]);
      const zIfc = oy + m.positions[srcIdx + 1];

      positions[dstIdx] = xIfc;
      positions[dstIdx + 1] = yIfc;
      positions[dstIdx + 2] = zIfc;

      normals[dstIdx] = m.normals[srcIdx];
      normals[dstIdx + 1] = -m.normals[srcIdx + 2];
      normals[dstIdx + 2] = m.normals[srcIdx + 1];

      if (!isHiddenByDefault) {
        if (xIfc < boundsAcc.minX) boundsAcc.minX = xIfc;
        if (yIfc < boundsAcc.minY) boundsAcc.minY = yIfc;
        if (zIfc < boundsAcc.minZ) boundsAcc.minZ = zIfc;
        if (xIfc > boundsAcc.maxX) boundsAcc.maxX = xIfc;
        if (yIfc > boundsAcc.maxY) boundsAcc.maxY = yIfc;
        if (zIfc > boundsAcc.maxZ) boundsAcc.maxZ = zIfc;
        boundsAcc.hasVisibleBounds = true;
      }
    }

    for (let i = 0; i < iCount; i++) {
      indices[indexCursor + i] = vertexCursor + m.indices[i];
    }

    const tableBase = r * MESH_RECORD_WORDS;
    meshTable[tableBase + MeshField.expressId] = m.expressId >>> 0;
    meshTable[tableBase + MeshField.firstVertex] = vertexCursor >>> 0;
    meshTable[tableBase + MeshField.vertexCount] = vCount >>> 0;
    meshTable[tableBase + MeshField.firstIndex] = indexCursor >>> 0;
    meshTable[tableBase + MeshField.indexCount] = iCount >>> 0;
    meshTable[tableBase + MeshField.rgba] = rgba;
    meshTable[tableBase + MeshField.flags] = flags >>> 0;

    vertexCursor += vCount;
    indexCursor += iCount;
  }

  return {
    modelId,
    sequence,
    origin: [0, 0, 0],
    positions,
    normals,
    indices,
    meshTable
  };
}

export class IfcLiteKernelAdapter implements KernelPort {
  private processor: GeometryProcessor | null = null;
  private openModels: Set<ModelId> = new Set();
  private modelBytes: Map<ModelId, Uint8Array> = new Map();
  private modelCoords: Map<ModelId, CoordinateReference> = new Map();

  private async ensureProcessor(): Promise<GeometryProcessor> {
    if (!this.processor) {
      this.processor = new GeometryProcessor();
      await this.processor.init();
    }
    return this.processor;
  }

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

    const processor = await this.ensureProcessor();
    let sequence = 0;
    let totalMeshes = 0;
    const boundsAcc = {
      minX: Infinity,
      minY: Infinity,
      minZ: Infinity,
      maxX: -Infinity,
      maxY: -Infinity,
      maxZ: -Infinity,
      hasVisibleBounds: false
    };

    for await (const event of processor.processStreaming(bytes)) {
      if (!this.openModels.has(modelId)) {
        return;
      }

      if (event.type === 'batch') {
        totalMeshes = event.totalSoFar;
        const batch = packMeshesToBatch(modelId, ++sequence, event.meshes, boundsAcc);
        if (batch) {
          yield { type: 'batch', batch };
        }
        yield {
          type: 'progress',
          processedElements: event.totalSoFar,
          totalElements: event.totalSoFar
        };
      } else if (event.type === 'complete') {
        totalMeshes = event.totalMeshes;
      }
    }

    if (!this.openModels.has(modelId)) {
      return;
    }

    const bounds: ModelBounds | undefined = boundsAcc.hasVisibleBounds
      ? {
          min: { x: boundsAcc.minX, y: boundsAcc.minY, z: boundsAcc.minZ },
          max: { x: boundsAcc.maxX, y: boundsAcc.maxY, z: boundsAcc.maxZ }
        }
      : undefined;

    yield {
      type: 'complete',
      meshCount: totalMeshes,
      bounds,
      diagnostics: []
    };
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
    const targetIds = modelIds.length > 0 ? modelIds : Array.from(this.openModels);
    const processor = await this.ensureProcessor();
    const meshes: MeshData[] = [];

    for (const id of targetIds) {
      const bytes = this.modelBytes.get(id);
      if (!bytes) continue;
      for await (const event of processor.processStreaming(bytes)) {
        if (event.type === 'batch') {
          meshes.push(...event.meshes);
        }
      }
    }

    let sharedCrs: string | undefined;
    for (const id of targetIds) {
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
    ifcParserService.removeStore(modelId);
  }
}
