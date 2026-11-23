import type { DrawingOptions } from '../../domain/drawings/DrawingTypes';
import type { CoordinateReference, ModelId } from '../../domain/models/ModelRecord';
import type { KernelDiagnostic } from '../../domain/kernel/KernelPort';

/** NaN on the wire means no depth was given, so a depth given as NaN must not look like that. */
function wireDepth(depth: number | undefined): number {
  if (depth === undefined) return Number.NaN;
  return Number.isNaN(depth) ? -1 : depth;
}

/** The flat numbers of `DrawingRequest::decode`. Models also get their `localToMap`. */
export function drawingRequest(options: DrawingOptions, localToMap?: readonly number[]): number[] {
  const { cut } = options;
  const flags =
    (options.includeHatching ? 1 : 0) |
    (options.includeSpaceLabels ? 2 : 0) |
    (options.includeDimensions ? 4 : 0);
  const tail = [
    wireDepth(cut.depth),
    options.includeProjection ? 1 : 0,
    flags,
    options.includeProjection && options.includeHiddenLines ? 1 : 0,
    ...(localToMap ?? [])
  ];
  if (cut.kind === 'plane') {
    return [
      3,
      cut.origin[0],
      cut.origin[1],
      cut.origin[2],
      cut.normal[0],
      cut.normal[1],
      cut.normal[2],
      ...tail
    ];
  }
  const kind = cut.kind === 'plan' ? 0 : cut.axis === 'x' ? 1 : 2;
  const position = cut.kind === 'plan' ? cut.elevation : cut.position;
  const look = cut.kind === 'section' && cut.look === 'positive' ? 1 : -1;
  return [kind, position, look, ...tail];
}

export interface FramedModels {
  /** Position in the original list, which numbers the model inside the drawing. */
  included: Array<{ index: number; modelId: ModelId; localToMap?: readonly number[] }>;
  crsId?: string;
  diagnostics: KernelDiagnostic[];
}

function sameCrs(a: CoordinateReference, b: CoordinateReference): boolean {
  if (a.verticalDatum !== b.verticalDatum) return false;
  if (a.crsId || b.crsId) return a.crsId === b.crsId;
  return a.crsName !== undefined && a.crsName === b.crsName;
}

/** Whether open models exist and the frame took none of them: closed ones do not count. */
export function frameExcludesAll(
  models: ReadonlyArray<{ open: boolean }>,
  included: FramedModels['included']
): boolean {
  return models.some(({ open }) => open) && !included.some(({ index }) => models[index].open);
}

/** Applies the frame rules of `docs/05` section 5 to a list of models. */
export function frameModels(
  frame: DrawingOptions['frame'],
  models: ReadonlyArray<{ modelId: ModelId; open: boolean; reference?: CoordinateReference }>
): FramedModels {
  if (frame.kind === 'project') {
    return {
      included: models.map(({ modelId }, index) => ({ index, modelId })),
      diagnostics: []
    };
  }
  const diagnostics: KernelDiagnostic[] = [];
  const included: FramedModels['included'] = [];
  let common: CoordinateReference | undefined;
  models.forEach(({ modelId, open, reference }, index) => {
    // A closed model has lost its reference too, and that is not the same as never having one.
    if (!open) {
      included.push({ index, modelId });
      return;
    }
    if (!reference?.localToMap) {
      diagnostics.push({
        severity: 'warning',
        code: 'map-frame-no-georeference',
        message: 'El modelo no tiene georreferencia y se omite del plano.',
        modelId
      });
      return;
    }
    common ??= reference;
    if (!sameCrs(common, reference)) {
      diagnostics.push({
        severity: 'warning',
        code: 'map-frame-crs-mismatch',
        message: 'El CRS del modelo difiere del común y se omite del plano.',
        modelId
      });
      return;
    }
    included.push({ index, modelId, localToMap: reference.localToMap });
  });
  const crsId = common?.confidence === 'verified' ? common.crsId : undefined;
  return { included, crsId, diagnostics };
}

/** Exact f64 inverse of a column-major 4x4 affine matrix (`bottom row = [0, 0, 0, 1]`). */
function invertAffineMat4(m: readonly number[]): number[] | null {
  if (m.length !== 16) return null;
  const a00 = m[0], a10 = m[1], a20 = m[2];
  const a01 = m[4], a11 = m[5], a21 = m[6];
  const a02 = m[8], a12 = m[9], a22 = m[10];
  const tx = m[12], ty = m[13], tz = m[14];

  const c00 = a11 * a22 - a12 * a21;
  const c01 = -(a10 * a22 - a12 * a20);
  const c02 = a10 * a21 - a11 * a20;
  const det = a00 * c00 + a01 * c01 + a02 * c02;
  if (!Number.isFinite(det) || Math.abs(det) < 1e-18) return null;

  const invDet = 1 / det;
  // Cofactor transpose / det
  const b00 = c00 * invDet;
  const b01 = -(a01 * a22 - a02 * a21) * invDet;
  const b02 = (a01 * a12 - a02 * a11) * invDet;
  const b10 = c01 * invDet;
  const b11 = (a00 * a22 - a02 * a20) * invDet;
  const b12 = -(a00 * a12 - a02 * a10) * invDet;
  const b20 = c02 * invDet;
  const b21 = -(a00 * a21 - a01 * a20) * invDet;
  const b22 = (a00 * a11 - a01 * a10) * invDet;

  const itx = -(b00 * tx + b01 * ty + b02 * tz);
  const ity = -(b10 * tx + b11 * ty + b12 * tz);
  const itz = -(b20 * tx + b21 * ty + b22 * tz);

  return [
    b00, b10, b20, 0,
    b01, b11, b21, 0,
    b02, b12, b22, 0,
    itx, ity, itz, 1
  ];
}

function mulMat4(a: readonly number[], b: readonly number[]): number[] {
  const out = new Array<number>(16).fill(0);
  for (let col = 0; col < 4; col++) {
    const b0 = b[col * 4];
    const b1 = b[col * 4 + 1];
    const b2 = b[col * 4 + 2];
    const b3 = b[col * 4 + 3];
    out[col * 4] = a[0] * b0 + a[4] * b1 + a[8] * b2 + a[12] * b3;
    out[col * 4 + 1] = a[1] * b0 + a[5] * b1 + a[9] * b2 + a[13] * b3;
    out[col * 4 + 2] = a[2] * b0 + a[6] * b1 + a[10] * b2 + a[14] * b3;
    out[col * 4 + 3] = a[3] * b0 + a[7] * b1 + a[11] * b2 + a[15] * b3;
  }
  return out;
}

function isIdentityMat4(m: readonly number[], eps = 1e-12): boolean {
  for (let col = 0; col < 4; col++) {
    for (let row = 0; row < 4; row++) {
      const expected = col === row ? 1 : 0;
      if (Math.abs(m[col * 4 + row] - expected) > eps) return false;
    }
  }
  return true;
}

/**
 * Computes the column-major 4x4 transform `inv(target.localToMap) * source.localToMap` mapping
 * coordinates from `sourceRef`'s model-local frame into `targetRef`'s model-local frame when
 * both models share a verified CRS. Returns `undefined` when no transform is needed (identity,
 * missing `localToMap`, or mismatched CRS).
 */
export function relativeModelTransform(
  sourceRef: CoordinateReference | undefined,
  targetRef: CoordinateReference | undefined
): number[] | undefined {
  if (!sourceRef?.localToMap || !targetRef?.localToMap) return undefined;
  if (!sameCrs(sourceRef, targetRef)) return undefined;
  const invTarget = invertAffineMat4(targetRef.localToMap);
  if (!invTarget) return undefined;
  const rel = mulMat4(invTarget, sourceRef.localToMap);
  return isIdentityMat4(rel) ? undefined : rel;
}

/**
 * Pulls a slab culling query `(tx, ty, sMin, sMax, eps)` defined in the target model's local
 * space back into the source model's local space given `sourceToTarget` (column-major 4x4).
 */
export function transformSlabQueryToSource(
  tx: number,
  ty: number,
  sMin: number,
  sMax: number,
  eps: number,
  sourceToTarget: readonly number[]
): { tx: number; ty: number; sMin: number; sMax: number; eps: number } {
  const vx = sourceToTarget[0] * tx + sourceToTarget[1] * ty;
  const vy = sourceToTarget[4] * tx + sourceToTarget[5] * ty;
  const norm = Math.hypot(vx, vy);
  if (!Number.isFinite(norm) || norm <= 1e-12) {
    return { tx, ty, sMin, sMax, eps };
  }
  const bDotT = sourceToTarget[12] * tx + sourceToTarget[13] * ty;
  return {
    tx: vx / norm,
    ty: vy / norm,
    sMin: (sMin - bDotT) / norm,
    sMax: (sMax - bDotT) / norm,
    eps: eps / norm
  };
}
