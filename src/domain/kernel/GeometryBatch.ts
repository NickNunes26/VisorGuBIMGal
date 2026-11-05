import type { ModelId } from '../models/ModelRecord';

/** Bump on any breaking change to this file or to KernelPort/RendererPort. */
export const CONTRACT_VERSION = 1;

/** Number of u32 words per record in GeometryBatch.meshTable. */
export const MESH_RECORD_WORDS = 7;

/** Word offsets inside one meshTable record. */
export const MeshField = {
  expressId: 0,
  firstVertex: 1,
  vertexCount: 2,
  firstIndex: 3,
  indexCount: 4,
  /** 0xRRGGBBAA, non-premultiplied sRGB. */
  rgba: 5,
  flags: 6
} as const;

export const MeshFlag = {
  /** Alpha below 255: draw in the transparent pass. */
  Transparent: 1 << 0,
  /** Kernel could not evaluate it fully (a void was not cut, say). The 'complete' event names it. */
  Degraded: 1 << 1,
  /** Pickable, but starts hidden. Only IfcSpace today. Left out of 'complete' bounds. */
  HiddenByDefault: 1 << 2,
  /**
   * The mesh is closed and winds outward: every side is shared by two of its triangles running
   * opposite ways, and every connected part encloses a positive volume. Seen from outside, its
   * back faces never show, so renderers may cull them. Pieces of a split mesh keep it.
   */
  Closed: 1 << 3
} as const;

/** Largest vertex distance from a batch origin, in metres. f32 keeps about 0.5 mm here. */
export const MAX_BATCH_RADIUS = 4096;

/** How far a normal's length may stray from 1. */
const NORMAL_TOLERANCE = 1e-3;

/**
 * One transferable block of triangles. Positions are metres, IFC Z-up, as f32 relative to
 * `origin`. Indices are zero-based within this batch. An element may own several records,
 * one per colour for instance, in one batch unless it is wider than MAX_BATCH_RADIUS and the
 * kernel split it into pieces across batches. Triangles run counter-clockwise seen
 * from the side their normal points to.
 */
export interface GeometryBatch {
  modelId: ModelId;
  /** Starts at 1 in each stream and rises by one. */
  sequence: number;
  /** Absolute model coordinates, as f64. Absolute vertex = origin + position. */
  origin: readonly [number, number, number];
  /**
   * Least and greatest of `positions` on each axis, relative to `origin` like them, exactly.
   * Zeros when there are none. Renderers skip a batch whose box is out of view.
   */
  box: { min: readonly [number, number, number]; max: readonly [number, number, number] };
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
  meshTable: Uint32Array;
}

/** Returns the first wire-format violation, or null. For tests and mock data, not hot paths. */
export function findBatchError(batch: GeometryBatch): string | null {
  const { origin, positions, normals, indices, meshTable } = batch;
  if (origin.length !== 3 || !origin.every(Number.isFinite)) return 'origin is not three finite numbers';
  if (positions.length % 3 !== 0) return 'positions length is not a multiple of 3';
  if (normals.length !== positions.length) return 'normals length differs from positions length';
  // A lone element above the radius is allowed if Degraded, so this checks only finiteness.
  if (!positions.every(Number.isFinite)) return 'positions contain a non-finite value';
  for (let axis = 0; axis < 3; axis++) {
    let least = positions.length > 0 ? Infinity : 0;
    let greatest = positions.length > 0 ? -Infinity : 0;
    for (let at = axis; at < positions.length; at += 3) {
      least = Math.min(least, positions[at]);
      greatest = Math.max(greatest, positions[at]);
    }
    if (batch.box.min[axis] !== least || batch.box.max[axis] !== greatest) {
      return `box differs from the positions on axis ${axis}`;
    }
  }
  for (let at = 0; at < normals.length; at += 3) {
    const length = Math.hypot(normals[at], normals[at + 1], normals[at + 2]);
    if (!(Math.abs(length - 1) <= NORMAL_TOLERANCE)) return `normal ${at / 3} is not unit length`;
  }
  if (indices.length % 3 !== 0) return 'indices length is not a multiple of 3';
  if (meshTable.length % MESH_RECORD_WORDS !== 0) {
    return `meshTable length is not a multiple of ${MESH_RECORD_WORDS}`;
  }

  const vertexTotal = positions.length / 3;
  for (let at = 0; at < meshTable.length; at += MESH_RECORD_WORDS) {
    const record = at / MESH_RECORD_WORDS;
    const expressId = meshTable[at + MeshField.expressId];
    const firstVertex = meshTable[at + MeshField.firstVertex];
    const vertexCount = meshTable[at + MeshField.vertexCount];
    const firstIndex = meshTable[at + MeshField.firstIndex];
    const indexCount = meshTable[at + MeshField.indexCount];

    if (expressId === 0) return `record ${record}: expressId is 0`;
    if (firstVertex + vertexCount > vertexTotal) return `record ${record}: vertices out of range`;
    if (firstIndex + indexCount > indices.length) return `record ${record}: indices out of range`;
    if (indexCount % 3 !== 0) return `record ${record}: indexCount is not a multiple of 3`;

    for (let i = firstIndex; i < firstIndex + indexCount; i++) {
      const vertex = indices[i];
      if (vertex < firstVertex || vertex >= firstVertex + vertexCount) {
        return `record ${record}: index ${i} leaves the record's vertex range`;
      }
    }
  }
  return null;
}
