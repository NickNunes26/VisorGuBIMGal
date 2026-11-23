import {
  formatStationPk,
  type AlignmentProjection,
  type BoundAlignment
} from './landXml';

/**
 * Pure domain planner for LandXML alignment clash DXF batch export (F4, `docs/12_clash_alignment_dxf.md`).
 *
 * Projects clashes onto a bound horizontal alignment, classifies Element A as transverse vs.
 * longitudinal using its `f64` vertices (matching `crates/clash-detector/src/slab.rs`), groups
 * consecutive clashes within `mergeToleranceMm` (default 20 mm), and formats filenames + `summary.csv`.
 */

export type SlabOrientation = 'transverse' | 'longitudinal';

export interface SlabPlanConfig {
  /** Default 20 mm; `0` disables PK merging. */
  mergeToleranceMm: number;
  /** Default 0.15 m. */
  longitudinalMinExtentM?: number;
  /** Default 2.5. */
  longitudinalAspectRatio?: number;
  /** Default 0.25 m. */
  maxTransverseSlabM?: number;
}

export interface ElementASlabSpan {
  orientation: SlabOrientation;
  sMin: number;
  sMax: number;
  sClash: number;
  depthM: number;
  deltaSA: number;
  widthPerpM: number;
  heightPerpM: number;
}

export interface ClashForAlignmentPlan {
  clashIndex: number;
  modelIdA: string;
  expressIdA: number;
  globalIdA?: string;
  labelA: string;
  modelIdB: string;
  expressIdB: number;
  labelB: string;
  kind: 'hard' | 'soft';
  penetrationM: number;
  gapM: number;
  point: readonly [number, number, number];
  /** Precomputed Rust `clash_detector::compute_element_a_slab` span when resolved inside the Worker. */
  precomputedSlab?: ElementASlabSpan;
  /** Flat `f64` xyz vertex positions of Element A in model-local metres (used when `precomputedSlab` is absent). */
  elementAVertices: Float64Array | readonly number[];
}

export interface PlannedClashSection {
  sectionIndex: number;
  fileName: string;
  pkM: number;
  pkLabel: string;
  orientation: SlabOrientation;
  /** Cut plane origin on the front face `sMin` of the slab in model-local metres. */
  origin: [number, number, number];
  /** Horizontal unit tangent `(tx, ty, 0)` in model-local space. */
  normal: [number, number, number];
  /** Alignment axis point at `pkM` in model-local space `[x, y, z]`. */
  alignmentPoint: [number, number, number];
  sMin: number;
  sMax: number;
  depthM: number;
  maxPenetrationM: number;
  clashIndices: number[];
  clashA: Array<{ modelId: string; expressId: number; globalId: string; label: string }>;
  clashB: Array<{ modelId: string; expressId: number; label: string }>;
  clashPoints: Array<[number, number, number]>;
  annotations: string[];
}

const DEFAULT_LONG_MIN_EXTENT_M = 0.15;
const DEFAULT_LONG_ASPECT = 2.5;
const DEFAULT_MAX_TRANSVERSE_M = 0.25;

/**
 * Classifies Element A relative to the horizontal unit tangent `(tx, ty)` and computes its exact
 * slab `[sMin, sMax]` (or `depth = 0` at `clashPoint` when longitudinal).
 */
export function computeElementASlabTs(
  vertices: Float64Array | readonly number[],
  tangentXY: readonly [number, number],
  clashPoint: readonly [number, number, number],
  config: Partial<SlabPlanConfig> = {}
): ElementASlabSpan {
  const len = Math.hypot(tangentXY[0], tangentXY[1]);
  const tx = len > 1e-12 ? tangentXY[0] / len : 1;
  const ty = len > 1e-12 ? tangentXY[1] / len : 0;
  const rx = ty;
  const ry = -tx;
  const sClash = clashPoint[0] * tx + clashPoint[1] * ty;

  let sMinA = Infinity;
  let sMaxA = -Infinity;
  let rMinA = Infinity;
  let rMaxA = -Infinity;
  let zMinA = Infinity;
  let zMaxA = -Infinity;

  for (let i = 0; i + 2 < vertices.length; i += 3) {
    const x = vertices[i];
    const y = vertices[i + 1];
    const z = vertices[i + 2];
    if (!Number.isFinite(x) || !Number.isFinite(y) || !Number.isFinite(z)) continue;
    const s = x * tx + y * ty;
    const r = x * rx + y * ry;
    if (s < sMinA) sMinA = s;
    if (s > sMaxA) sMaxA = s;
    if (r < rMinA) rMinA = r;
    if (r > rMaxA) rMaxA = r;
    if (z < zMinA) zMinA = z;
    if (z > zMaxA) zMaxA = z;
  }

  if (!Number.isFinite(sMinA) || !Number.isFinite(sMaxA)) {
    return {
      orientation: 'longitudinal',
      sMin: sClash,
      sMax: sClash,
      sClash,
      depthM: 0,
      deltaSA: 0,
      widthPerpM: 0,
      heightPerpM: 0
    };
  }

  const deltaSA = Math.max(0, sMaxA - sMinA);
  const widthPerpM = Math.max(0, rMaxA - rMinA);
  const heightPerpM = Math.max(0, zMaxA - zMinA);
  const maxPerp = Math.max(widthPerpM, heightPerpM);

  const minExt = config.longitudinalMinExtentM ?? DEFAULT_LONG_MIN_EXTENT_M;
  const aspect = config.longitudinalAspectRatio ?? DEFAULT_LONG_ASPECT;
  const maxTrans = config.maxTransverseSlabM ?? DEFAULT_MAX_TRANSVERSE_M;

  if (deltaSA > minExt && deltaSA > aspect * maxPerp) {
    return {
      orientation: 'longitudinal',
      sMin: sClash,
      sMax: sClash,
      sClash,
      depthM: 0,
      deltaSA,
      widthPerpM,
      heightPerpM
    };
  }

  let sMin = sMinA;
  let sMax = sMaxA;
  if (Number.isFinite(maxTrans) && maxTrans > 0 && deltaSA > maxTrans) {
    const half = maxTrans * 0.5;
    sMin = Math.max(sMinA, sClash - half);
    sMax = sMin + maxTrans;
    if (sMax > sMaxA) {
      sMax = sMaxA;
      sMin = Math.max(sMinA, sMax - maxTrans);
    }
  }

  return {
    orientation: 'transverse',
    sMin,
    sMax,
    sClash,
    depthM: Math.max(0, sMax - sMin),
    deltaSA,
    widthPerpM,
    heightPerpM
  };
}

/** Formats a safe individual DXF filename inside `individual/`, e.g. `individual/PK_0+124.50_001.dxf`. */
export function formatClashDxfFilename(pkLabel: string, indexOneBased: number): string {
  const safePk = pkLabel.replace(/[^A-Za-z0-9+._-]/g, '_');
  const seq = String(indexOneBased).padStart(3, '0');
  return `individual/PK_${safePk}_${seq}.dxf`;
}

/**
 * Plans all grouped cross-sections for a set of clashes along a bound horizontal alignment.
 */
export function planClashAlignmentSections(
  clashes: readonly ClashForAlignmentPlan[],
  bound: BoundAlignment,
  config: SlabPlanConfig = { mergeToleranceMm: 20 }
): PlannedClashSection[] {
  if (clashes.length === 0) return [];

  interface ProjectedItem {
    clash: ClashForAlignmentPlan;
    proj: AlignmentProjection;
    slab: ElementASlabSpan;
  }

  const projected: ProjectedItem[] = clashes.map((clash) => {
    const proj = bound.projectLocal(clash.point[0], clash.point[1]);
    const slab =
      clash.precomputedSlab ??
      computeElementASlabTs(clash.elementAVertices, proj.tangent, clash.point, config);
    return { clash, proj, slab };
  });

  projected.sort((a, b) => a.proj.station - b.proj.station || a.clash.clashIndex - b.clash.clashIndex);

  const tolM =
    Number.isFinite(config.mergeToleranceMm) && config.mergeToleranceMm > 0
      ? config.mergeToleranceMm / 1000
      : 0;

  const groups: ProjectedItem[][] = [];
  let start = 0;
  while (start < projected.length) {
    let end = start + 1;
    if (tolM > 0) {
      while (end < projected.length && projected[end].proj.station - projected[end - 1].proj.station <= tolM) {
        end++;
      }
    }
    groups.push(projected.slice(start, end));
    start = end;
  }

  return groups.map((group, idx) => {
    // Primary clash: highest penetration, then smallest gap, then lowest PK.
    let primary = group[0];
    for (let i = 1; i < group.length; i++) {
      const c = group[i];
      if (
        c.clash.penetrationM > primary.clash.penetrationM ||
        (c.clash.penetrationM === primary.clash.penetrationM && c.clash.gapM < primary.clash.gapM)
      ) {
        primary = c;
      }
    }

    const tx = primary.proj.tangent[0];
    const ty = primary.proj.tangent[1];
    const anyTransverse = group.some((g) => g.slab.orientation === 'transverse');

    let sMin: number;
    let sMax: number;
    let orientation: SlabOrientation;
    if (anyTransverse) {
      orientation = 'transverse';
      sMin = Infinity;
      sMax = -Infinity;
      for (const g of group) {
        if (g.slab.orientation !== 'transverse') continue;
        const sOnRep = g.clash.point[0] * tx + g.clash.point[1] * ty;
        const back = g.slab.sClash - g.slab.sMin;
        const fwd = g.slab.sMax - g.slab.sClash;
        if (sOnRep - back < sMin) sMin = sOnRep - back;
        if (sOnRep + fwd > sMax) sMax = sOnRep + fwd;
      }
    } else {
      orientation = 'longitudinal';
      const sCut = primary.clash.point[0] * tx + primary.clash.point[1] * ty;
      sMin = sCut;
      sMax = sCut;
    }

    const depthM = Math.max(0, sMax - sMin);
    const sPrimary = primary.clash.point[0] * tx + primary.clash.point[1] * ty;
    const shiftToFront = sMin - sPrimary;
    const origin: [number, number, number] = [
      primary.clash.point[0] + shiftToFront * tx,
      primary.clash.point[1] + shiftToFront * ty,
      primary.clash.point[2]
    ];

    const pkM = primary.proj.station;
    const pkLabel = formatStationPk(pkM, 2);
    const fileName = formatClashDxfFilename(pkLabel, idx + 1);

    const seenA = new Set<string>();
    const clashA: PlannedClashSection['clashA'] = [];
    const seenB = new Set<string>();
    const clashB: PlannedClashSection['clashB'] = [];
    const clashPoints: Array<[number, number, number]> = [];
    let maxPenetrationM = 0;

    for (const g of group) {
      if (g.clash.penetrationM > maxPenetrationM) maxPenetrationM = g.clash.penetrationM;
      clashPoints.push([g.clash.point[0], g.clash.point[1], g.clash.point[2]]);
      const keyA = `${g.clash.modelIdA}:${g.clash.expressIdA}`;
      if (!seenA.has(keyA)) {
        seenA.add(keyA);
        clashA.push({
          modelId: g.clash.modelIdA,
          expressId: g.clash.expressIdA,
          globalId: g.clash.globalIdA ?? '',
          label: g.clash.labelA
        });
      }
      const keyB = `${g.clash.modelIdB}:${g.clash.expressIdB}`;
      if (!seenB.has(keyB)) {
        seenB.add(keyB);
        clashB.push({
          modelId: g.clash.modelIdB,
          expressId: g.clash.expressIdB,
          label: g.clash.labelB
        });
      }
    }

    const depthMmStr = (depthM * 1000).toFixed(1);
    const penMmStr = (maxPenetrationM * 1000).toFixed(1);
    const annotations = [
      `PK ${pkLabel} | ${orientation === 'transverse' ? `Banda ${depthMmStr} mm` : 'Corte 0 mm (longitudinal)'} | Pen. max ${penMmStr} mm`,
      `A: ${clashA.map((a) => `${a.label} (#${a.expressId})`).join(', ')}`,
      `B: ${clashB.map((b) => `${b.label} (#${b.expressId})`).join(', ')}`
    ];

    return {
      sectionIndex: idx,
      fileName,
      pkM,
      pkLabel,
      orientation,
      origin,
      normal: [tx, ty, 0],
      alignmentPoint: [primary.proj.point[0], primary.proj.point[1], primary.clash.point[2]],
      sMin,
      sMax,
      depthM,
      maxPenetrationM,
      clashIndices: group.map((g) => g.clash.clashIndex),
      clashA,
      clashB,
      clashPoints,
      annotations
    };
  });
}

function csvEscape(value: string): string {
  if (/[",\r\n]/.test(value)) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

/** Generates `summary.csv` indexing every exported individual DXF section. */
export function buildClashSummaryCsv(sections: readonly PlannedClashSection[]): string {
  const header =
    'index,file,pk_m,pk_label,orientation,depth_mm,clash_count,max_penetration_mm,elements_a,elements_b';
  const rows = sections.map((s) =>
    [
      String(s.sectionIndex + 1),
      csvEscape(s.fileName),
      s.pkM.toFixed(3),
      csvEscape(s.pkLabel),
      s.orientation,
      (s.depthM * 1000).toFixed(1),
      String(s.clashIndices.length),
      (s.maxPenetrationM * 1000).toFixed(1),
      csvEscape(s.clashA.map((a) => `${a.label} (#${a.expressId})`).join('; ')),
      csvEscape(s.clashB.map((b) => `${b.label} (#${b.expressId})`).join('; '))
    ].join(',')
  );
  return [header, ...rows].join('\r\n') + '\r\n';
}
