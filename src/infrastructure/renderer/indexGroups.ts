import { MESH_RECORD_WORDS, MeshField, MeshFlag } from '../../domain/kernel/GeometryBatch';

/**
 * A batch's indices sorted for the passes: the opaque ones, those of closed meshes first, whose
 * back faces may be culled, and the transparent ones, each in record order.
 */
export interface IndexGroups {
  opaque: Uint32Array;
  /** How many of `opaque`, from the start, belong to closed meshes. */
  closedCount: number;
  transparent: Uint32Array;
}

export function isTransparentRecord(rawFlags: number, rgba: number): boolean {
  return (rawFlags & MeshFlag.Transparent) !== 0 || (rgba & 0xff) < 255;
}

/** Splits `indices` by the records of `meshTable`. Hands `indices` back whole when it can. */
export function groupIndices(indices: Uint32Array, meshTable: Uint32Array): IndexGroups {
  const recordCount = meshTable.length / MESH_RECORD_WORDS;
  let closedCount = 0;
  let openCount = 0;
  let transparentCount = 0;
  for (let r = 0; r < recordCount; r++) {
    const base = r * MESH_RECORD_WORDS;
    const count = meshTable[base + MeshField.indexCount] >>> 0;
    const rawFlags = meshTable[base + MeshField.flags] >>> 0;
    if (isTransparentRecord(rawFlags, meshTable[base + MeshField.rgba] >>> 0)) transparentCount += count;
    else if ((rawFlags & MeshFlag.Closed) !== 0) closedCount += count;
    else openCount += count;
  }

  const opaqueCount = closedCount + openCount;
  if (indices.length === opaqueCount && (closedCount === 0 || openCount === 0)) {
    return { opaque: indices, closedCount, transparent: new Uint32Array(0) };
  }
  if (indices.length === transparentCount) {
    return { opaque: new Uint32Array(0), closedCount: 0, transparent: indices };
  }

  const opaque = new Uint32Array(opaqueCount);
  const transparent = new Uint32Array(transparentCount);
  let closedAt = 0;
  let openAt = closedCount;
  let transparentAt = 0;
  for (let r = 0; r < recordCount; r++) {
    const base = r * MESH_RECORD_WORDS;
    const first = meshTable[base + MeshField.firstIndex] >>> 0;
    const count = meshTable[base + MeshField.indexCount] >>> 0;
    const rawFlags = meshTable[base + MeshField.flags] >>> 0;
    const range = indices.subarray(first, first + count);
    if (isTransparentRecord(rawFlags, meshTable[base + MeshField.rgba] >>> 0)) {
      transparent.set(range, transparentAt);
      transparentAt += count;
    } else if ((rawFlags & MeshFlag.Closed) !== 0) {
      opaque.set(range, closedAt);
      closedAt += count;
    } else {
      opaque.set(range, openAt);
      openAt += count;
    }
  }
  return { opaque, closedCount, transparent };
}
