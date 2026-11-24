import type { ModelId } from '../../domain/models/ModelRecord';
import type { GeometryBatch } from '../../domain/kernel/GeometryBatch';
import type { ElementQuery, GeometryEvent, KernelErrorCode } from '../../domain/kernel/KernelPort';

/** A Worker holds one model, opened by its first request, or one drawing's sheet. */
export type WorkerRequest =
  | { op: 'open'; modelId: ModelId; bytes: ArrayBuffer }
  | { op: 'start-stream' }
  | { op: 'next-batch' }
  | { op: 'details'; expressId: number }
  | { op: 'snapshots' }
  | { op: 'tree' }
  | { op: 'find'; query: ElementQuery }
  | { op: 'mesh'; expressId: number }
  | { op: 'draw-start'; request: number[] }
  | { op: 'draw-next' }
  | { op: 'draw-finish' }
  | { op: 'sheet-new'; request: number[] }
  | { op: 'sheet-add'; modelIndex: number; chunk: ArrayBuffer }
  | { op: 'sheet-drop'; modelIndex: number }
  | { op: 'sheet-generate' }
  | {
      op: 'sheet-export';
      svg: boolean;
      scale: number;
      showHidden: boolean;
      showHatching: boolean;
      comment?: string;
    }
  | { op: 'clash-bounds'; expressIds: Uint32Array }
  | { op: 'clash-mesh'; expressId: number }
  | { op: 'clash-new'; maxCacheBytes: number }
  | { op: 'clash-broad'; aFlat: Float64Array; bFlat: Float64Array; clearanceM: number }
  | {
      op: 'clash-cache';
      modelIndex: number;
      expressId: number;
      positions: Float64Array;
      indices: Uint32Array;
    }
  | {
      op: 'clash-set-active';
      modelIndex: number;
      expressId: number;
      positions: Float64Array;
      indices: Uint32Array;
    }
  | {
      op: 'clash-test';
      cachedModel: number;
      cachedId: number;
      clearanceM: number;
    }
  | {
      op: 'clash-cull-slab';
      tx: number;
      ty: number;
      sMin: number;
      sMax: number;
      eps: number;
    }
  | {
      op: 'clash-slab-chunk';
      expressIds: Uint32Array;
      transform?: number[];
    }
  | { op: 'clash-batch-new' }
  | { op: 'clash-batch-add'; modelIndex: number; chunk: ArrayBuffer }
  | {
      op: 'clash-batch-finish';
      origin: readonly [number, number, number];
      normal: readonly [number, number, number];
      depthM: number;
      clashAModelIndices: Uint32Array;
      clashAIds: Uint32Array;
      clashAGuids: string[];
      /** Session model id of each Element A, written per element for routing re-imports. */
      clashAOwners: string[];
      clashBModelIndices: Uint32Array;
      clashBIds: Uint32Array;
      clashPointsFlat: Float64Array;
      alignmentPoint: readonly [number, number, number];
      modelId: string;
      pkM: number;
      pkLabel: string;
      sMin: number;
      sMax: number;
      annotations: string[];
    }
  | { op: 'clash-batch-merge'; columns: number; marginM: number }
  | {
      op: 'clash-roundtrip-apply';
      dxfBytes: Uint8Array;
      /** This worker's model id, and every open model's: see `applyClashDxf` in wasm-api. */
      selfModelId: string;
      openModelIds: string[];
      /** Edit indices already applied to another model in this import. */
      exclude: Uint32Array;
    }
  | {
      op: 'clash-element-slab';
      expressId: number;
      tx: number;
      ty: number;
      px: number;
      py: number;
      pz: number;
      longMinExtentM: number;
      longAspectRatio: number;
      maxTransverseSlabM: number;
    };

/** One pull of a geometry stream. */
export type StreamStep =
  | { batch: GeometryBatch; processedElements: number; totalElements: number }
  | { complete: Extract<GeometryEvent, { type: 'complete' }> };

/** A diagnostic from a sheet: models are numbered by their place in the drawing's list. */
export interface SheetDiagnostic {
  severity: 'info' | 'warning' | 'error';
  code: string;
  message: string;
  modelIndex?: number;
  expressId?: number;
}

export type WorkerResponse =
  | { id: number; ok: true; value: unknown }
  | { id: number; ok: false; code: KernelErrorCode; message: string; offset?: number };
