import type { CoordinateReference, ModelBounds } from '../models/ModelRecord';
import { child, children, childText, parseXml, type XmlNode } from '../xml/xmlTree';

/**
 * Horizontal LandXML 1.2 alignment parser and f64 geometric evaluator/projector (F1).
 *
 * Supports:
 * - `<Units><Metric>` and `<Units><Imperial>` (linear: meter/millimeter/centimeter/kilometer/foot;
 *   angular & direction: grads/centesimal [400 gon = 360°], decimal degrees, radians).
 * - `<Alignments><Alignment name="..." staStart="...">` including negative `staStart` and multiple axes.
 * - `<CoordGeom>` elements: `<Line>`, `<Curve rot="cw|ccw">`, and `<Spiral spiType="clothoid">`
 *   (tangent-to-curve, curve-to-tangent, compound/ovoid, and S-curves).
 * - LandXML `Northing Easting` coordinate order converted explicitly to `(X, Y) = (Easting, Northing)` in meters.
 * - Inverse `localToMap` transformation in `f64` to bind map-space alignments to IFC model-local space.
 */

export type Vec2 = [number, number];

export interface Bounds2D {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export type TurnDirection = 'cw' | 'ccw';

export interface LandXmlUnits {
  linearUnit: string;
  linearToMeters: number;
  angularUnit: string;
  angularToRadians: number;
  directionUnit: string;
  directionToRadians: number;
}

export interface AlignmentWarning {
  code:
    | 'unsupported-segment'
    | 'skipped-segment'
    | 'no-map-conversion-disjoint-bounds'
    | 'invalid-local-to-map';
  message: string;
  alignmentName?: string;
}

export interface LineSegment {
  kind: 'line';
  staStart: number;
  staEnd: number;
  length: number;
  start: Vec2;
  end: Vec2;
  tangent: Vec2;
}

export interface CurveSegment {
  kind: 'curve';
  staStart: number;
  staEnd: number;
  length: number;
  rot: TurnDirection;
  radius: number;
  start: Vec2;
  center: Vec2;
  end: Vec2;
  /** Polar angle of `start` around `center` in standard math XY radians. */
  startAngle: number;
  /** Signed sweep angle in radians (`> 0` for ccw, `< 0` for cw). */
  sweepAngle: number;
}

export interface SpiralSegment {
  kind: 'spiral';
  staStart: number;
  staEnd: number;
  length: number;
  rot: TurnDirection;
  spiType: string;
  /** Finite radius in meters, or `Infinity` for straight tangent. */
  radiusStart: number;
  /** Finite radius in meters, or `Infinity` for straight tangent. */
  radiusEnd: number;
  /** Signed start curvature `1 / R_start` (0 when straight). */
  k0: number;
  /** Signed end curvature `1 / R_end` (0 when straight). */
  k1: number;
  /** Clothoid parameter A (meters), when known or derivable. */
  constant?: number;
  start: Vec2;
  end: Vec2;
  /** Initial unit tangent angle in standard math XY radians, fitted to `(start, end)`. */
  theta0: number;
}

export type AlignmentSegment = LineSegment | CurveSegment | SpiralSegment;

export interface HorizontalAlignment {
  name: string;
  desc?: string;
  staStart: number;
  staEnd: number;
  length: number;
  segments: AlignmentSegment[];
  bounds2D: Bounds2D;
}

export interface LandXmlDocument {
  units: LandXmlUnits;
  alignments: HorizontalAlignment[];
  warnings: AlignmentWarning[];
}

export interface AlignmentSample {
  station: number;
  point: Vec2;
  /** Unit tangent vector in the direction of increasing station. */
  tangent: Vec2;
  /** Unit right-hand normal `(tangent.y, -tangent.x)` (90° CW from tangent). */
  rightNormal: Vec2;
  /** Signed curvature (1/m; positive CCW, negative CW). */
  curvature: number;
  segmentIndex: number;
}

export interface AlignmentProjection extends AlignmentSample {
  /** Euclidean horizontal distance from query point to projected point (meters). */
  distance: number;
  /** Signed lateral offset along `rightNormal` (positive = right of alignment, negative = left). */
  signedOffset: number;
  /** True when projection fell onto the start or end boundary of the alignment. */
  clamped: boolean;
}

export interface Affine2D {
  /** Column-major 2x2 linear part: x' = a00*x + a01*y + tx, y' = a10*x + a11*y + ty */
  a00: number;
  a10: number;
  a01: number;
  a11: number;
  tx: number;
  ty: number;
}

export interface BoundAlignment {
  alignment: HorizontalAlignment;
  hasMapConversion: boolean;
  localToMap2D: Affine2D;
  mapToLocal2D: Affine2D;
  localBounds2D: Bounds2D;
  warnings: AlignmentWarning[];
  evaluateLocal(station: number): AlignmentSample;
  projectLocal(x: number, y: number): AlignmentProjection;
}

const IDENTITY_AFFINE_2D: Affine2D = { a00: 1, a10: 0, a01: 0, a11: 1, tx: 0, ty: 0 };

// 8-point Gauss-Legendre quadrature nodes and weights on [-1, 1] (exact for degree <= 15).
const GL8_NODES: readonly number[] = [
  -0.9602898564975363,
  -0.7966664774136267,
  -0.5255324099163290,
  -0.1834346424956498,
  0.1834346424956498,
  0.5255324099163290,
  0.7966664774136267,
  0.9602898564975363
];
const GL8_WEIGHTS: readonly number[] = [
  0.1012285362903763,
  0.2223810344533745,
  0.3137066458778873,
  0.3626837833783620,
  0.3626837833783620,
  0.3137066458778873,
  0.2223810344533745,
  0.1012285362903763
];

function linearUnitScale(unitRaw: string | undefined): number {
  const u = (unitRaw ?? 'meter').trim().toLowerCase();
  switch (u) {
    case 'meter':
    case 'meters':
    case 'metre':
    case 'metres':
    case 'm':
      return 1;
    case 'millimeter':
    case 'millimeters':
    case 'millimetre':
    case 'millimetres':
    case 'mm':
      return 1e-3;
    case 'centimeter':
    case 'centimeters':
    case 'centimetre':
    case 'centimetres':
    case 'cm':
      return 1e-2;
    case 'kilometer':
    case 'kilometers':
    case 'kilometre':
    case 'kilometres':
    case 'km':
      return 1e3;
    case 'foot':
    case 'feet':
    case 'internationalfoot':
    case 'ft':
      return 0.3048;
    case 'ussurveyfoot':
      return 1200 / 3937;
    case 'inch':
    case 'inches':
      return 0.0254;
    default:
      return 1;
  }
}

function angularUnitScale(unitRaw: string | undefined): number {
  const u = (unitRaw ?? 'radians').trim().toLowerCase();
  if (u.includes('grad') || u.includes('gon') || u.includes('centesimal')) {
    return Math.PI / 200;
  }
  if (u.includes('deg')) {
    return Math.PI / 180;
  }
  return 1;
}

function parseUnits(root: XmlNode): LandXmlUnits {
  const unitsNode = child(root, 'Units');
  const metric = child(unitsNode, 'Metric');
  const imperial = child(unitsNode, 'Imperial');
  const active = metric ?? imperial;
  const defaultLinear = imperial && !metric ? 'foot' : 'meter';
  const linearUnit = active?.attrs.linearUnit ?? defaultLinear;
  const angularUnit = active?.attrs.angularUnit ?? 'radians';
  const directionUnit = active?.attrs.directionUnit ?? angularUnit;
  return {
    linearUnit,
    linearToMeters: linearUnitScale(linearUnit),
    angularUnit,
    angularToRadians: angularUnitScale(angularUnit),
    directionUnit,
    directionToRadians: angularUnitScale(directionUnit)
  };
}

function parseFiniteNumber(raw: string | undefined): number | undefined {
  if (raw === undefined) return undefined;
  const v = Number(raw.trim());
  return Number.isFinite(v) ? v : undefined;
}

function parseRadiusMeters(raw: string | undefined, linearToMeters: number): number {
  if (raw === undefined) return Infinity;
  const trimmed = raw.trim().toUpperCase();
  if (trimmed === '' || trimmed === 'INF' || trimmed === 'INFINITY' || trimmed === '+INF' || trimmed === '-INF') {
    return Infinity;
  }
  const v = Number(trimmed);
  if (!Number.isFinite(v) || v === 0) return Infinity;
  return Math.abs(v) * linearToMeters;
}

/**
 * LandXML stores 2D/3D point text as `Northing Easting [Elevation]`.
 * Converts explicitly to `(X, Y) = (Easting, Northing)` in meters.
 */
function parseNorthingEastingPoint(text: string | undefined, linearToMeters: number): Vec2 | undefined {
  if (!text) return undefined;
  const parts = text.trim().split(/\s+/);
  if (parts.length < 2) return undefined;
  const northing = Number(parts[0]);
  const easting = Number(parts[1]);
  if (!Number.isFinite(northing) || !Number.isFinite(easting)) return undefined;
  return [easting * linearToMeters, northing * linearToMeters];
}

function parseTurnDirection(raw: string | undefined): TurnDirection {
  const norm = (raw ?? 'ccw').trim().toLowerCase();
  return norm === 'cw' || norm === 'clockwise' ? 'cw' : 'ccw';
}

function emptyBounds(): Bounds2D {
  return { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
}

function expandBounds(b: Bounds2D, p: Vec2): void {
  if (p[0] < b.minX) b.minX = p[0];
  if (p[1] < b.minY) b.minY = p[1];
  if (p[0] > b.maxX) b.maxX = p[0];
  if (p[1] > b.maxY) b.maxY = p[1];
}

/**
 * Evaluates a generalized clothoid in its local frame (origin at start, initial tangent along +X)
 * using composite 8-point Gauss-Legendre quadrature for full f64 accuracy across all deflection
 * angles and compound (ovoid) spirals.
 */
function integrateClothoidLocal(k0: number, k1: number, length: number, u: number): Vec2 {
  if (u <= 0 || length <= 0) return [0, 0];
  const dk = (k1 - k0) / length;
  const maxTurn = Math.abs(k0 * u) + 0.5 * Math.abs(dk) * u * u;
  const subIntervals = Math.max(1, Math.min(64, Math.ceil(maxTurn / 0.25)));
  const h = u / subIntervals;
  const halfH = 0.5 * h;
  let x = 0;
  let y = 0;
  for (let i = 0; i < subIntervals; i++) {
    const mid = (i + 0.5) * h;
    for (let j = 0; j < 8; j++) {
      const s = mid + halfH * GL8_NODES[j];
      const theta = k0 * s + 0.5 * dk * s * s;
      const w = halfH * GL8_WEIGHTS[j];
      x += w * Math.cos(theta);
      y += w * Math.sin(theta);
    }
  }
  return [x, y];
}

function evaluateLine(seg: LineSegment, u: number): { point: Vec2; tangent: Vec2; curvature: number } {
  const t = Math.max(0, Math.min(seg.length, u));
  return {
    point: [seg.start[0] + seg.tangent[0] * t, seg.start[1] + seg.tangent[1] * t],
    tangent: seg.tangent,
    curvature: 0
  };
}

function evaluateCurve(seg: CurveSegment, u: number): { point: Vec2; tangent: Vec2; curvature: number } {
  const clamped = Math.max(0, Math.min(seg.length, u));
  const sign = seg.rot === 'ccw' ? 1 : -1;
  const alpha = seg.startAngle + sign * (clamped / seg.radius);
  const cosA = Math.cos(alpha);
  const sinA = Math.sin(alpha);
  return {
    point: [seg.center[0] + seg.radius * cosA, seg.center[1] + seg.radius * sinA],
    tangent: [-sign * sinA, sign * cosA],
    curvature: sign / seg.radius
  };
}

function evaluateSpiral(seg: SpiralSegment, u: number): { point: Vec2; tangent: Vec2; curvature: number } {
  const clamped = Math.max(0, Math.min(seg.length, u));
  const [lx, ly] = integrateClothoidLocal(seg.k0, seg.k1, seg.length, clamped);
  const cos0 = Math.cos(seg.theta0);
  const sin0 = Math.sin(seg.theta0);
  const px = seg.start[0] + cos0 * lx - sin0 * ly;
  const py = seg.start[1] + sin0 * lx + cos0 * ly;
  const dk = seg.length > 0 ? (seg.k1 - seg.k0) / seg.length : 0;
  const dTheta = seg.k0 * clamped + 0.5 * dk * clamped * clamped;
  const heading = seg.theta0 + dTheta;
  return {
    point: [px, py],
    tangent: [Math.cos(heading), Math.sin(heading)],
    curvature: seg.k0 + dk * clamped
  };
}

function evaluateSegment(seg: AlignmentSegment, u: number): { point: Vec2; tangent: Vec2; curvature: number } {
  switch (seg.kind) {
    case 'line':
      return evaluateLine(seg, u);
    case 'curve':
      return evaluateCurve(seg, u);
    case 'spiral':
      return evaluateSpiral(seg, u);
  }
}

function projectOnLine(seg: LineSegment, q: Vec2): { u: number; point: Vec2; tangent: Vec2; curvature: number; distSq: number } {
  const dx = q[0] - seg.start[0];
  const dy = q[1] - seg.start[1];
  const raw = dx * seg.tangent[0] + dy * seg.tangent[1];
  const u = Math.max(0, Math.min(seg.length, raw));
  const px = seg.start[0] + seg.tangent[0] * u;
  const py = seg.start[1] + seg.tangent[1] * u;
  const ex = q[0] - px;
  const ey = q[1] - py;
  return { u, point: [px, py], tangent: seg.tangent, curvature: 0, distSq: ex * ex + ey * ey };
}

function wrapAnglePositive(rad: number): number {
  const twoPi = 2 * Math.PI;
  let a = rad % twoPi;
  if (a < 0) a += twoPi;
  return a;
}

function projectOnCurve(seg: CurveSegment, q: Vec2): { u: number; point: Vec2; tangent: Vec2; curvature: number; distSq: number } {
  const vx = q[0] - seg.center[0];
  const vy = q[1] - seg.center[1];
  const rQ = Math.hypot(vx, vy);
  const absSweep = Math.abs(seg.sweepAngle);

  let uBest = 0;
  if (rQ > 1e-12) {
    const angleQ = Math.atan2(vy, vx);
    const delta =
      seg.rot === 'ccw'
        ? wrapAnglePositive(angleQ - seg.startAngle)
        : wrapAnglePositive(seg.startAngle - angleQ);
    if (delta <= absSweep) {
      uBest = delta * seg.radius;
    } else {
      const dStartSq = (q[0] - seg.start[0]) ** 2 + (q[1] - seg.start[1]) ** 2;
      const dEndSq = (q[0] - seg.end[0]) ** 2 + (q[1] - seg.end[1]) ** 2;
      uBest = dStartSq <= dEndSq ? 0 : seg.length;
    }
  }

  const evalRes = evaluateCurve(seg, uBest);
  const ex = q[0] - evalRes.point[0];
  const ey = q[1] - evalRes.point[1];
  return { u: uBest, ...evalRes, distSq: ex * ex + ey * ey };
}

function projectOnSpiral(seg: SpiralSegment, q: Vec2): { u: number; point: Vec2; tangent: Vec2; curvature: number; distSq: number } {
  const L = seg.length;
  // Phase 1: coarse sampling to locate candidate local minima.
  const samples = Math.min(256, Math.max(16, Math.ceil(L / 2)));
  const distSqArr = new Float64Array(samples + 1);
  let globalMinIdx = 0;
  let globalMinDistSq = Infinity;

  for (let i = 0; i <= samples; i++) {
    const u = (L * i) / samples;
    const ev = evaluateSpiral(seg, u);
    const ex = q[0] - ev.point[0];
    const ey = q[1] - ev.point[1];
    const d2 = ex * ex + ey * ey;
    distSqArr[i] = d2;
    if (d2 < globalMinDistSq) {
      globalMinDistSq = d2;
      globalMinIdx = i;
    }
  }

  // Gather candidate starting u values: global min + any interior local minima.
  const candidates: number[] = [(L * globalMinIdx) / samples];
  for (let i = 1; i < samples; i++) {
    if (i !== globalMinIdx && distSqArr[i] <= distSqArr[i - 1] && distSqArr[i] <= distSqArr[i + 1]) {
      candidates.push((L * i) / samples);
    }
  }

  // Phase 2: damped Newton-Raphson refinement on each candidate.
  let bestU = 0;
  let bestEval = evaluateSpiral(seg, 0);
  let bestDistSq = (q[0] - bestEval.point[0]) ** 2 + (q[1] - bestEval.point[1]) ** 2;

  const endEval = evaluateSpiral(seg, L);
  const endDistSq = (q[0] - endEval.point[0]) ** 2 + (q[1] - endEval.point[1]) ** 2;
  if (endDistSq < bestDistSq) {
    bestU = L;
    bestEval = endEval;
    bestDistSq = endDistSq;
  }

  for (const uInit of candidates) {
    let u = uInit;
    let ev = evaluateSpiral(seg, u);
    let dx = ev.point[0] - q[0];
    let dy = ev.point[1] - q[1];
    let d2 = dx * dx + dy * dy;

    for (let iter = 0; iter < 16; iter++) {
      // f(u) = 0.5 * |P(u) - Q|^2
      // f'(u) = (P - Q) · T
      // f''(u) = 1 + (P - Q) · (k * N_left), where N_left = (-Ty, Tx)
      const f1 = dx * ev.tangent[0] + dy * ev.tangent[1];
      if (Math.abs(f1) < 1e-12) break;
      const nLeftX = -ev.tangent[1];
      const nLeftY = ev.tangent[0];
      const f2 = 1 + ev.curvature * (dx * nLeftX + dy * nLeftY);
      const step = f2 > 1e-9 ? f1 / f2 : f1;

      // Backtracking line search to guarantee monotonic decrease of d2.
      let alpha = 1;
      let improved = false;
      for (let ls = 0; ls < 8; ls++) {
        const uNext = Math.max(0, Math.min(L, u - alpha * step));
        if (Math.abs(uNext - u) < 1e-11) break;
        const evNext = evaluateSpiral(seg, uNext);
        const dxNext = evNext.point[0] - q[0];
        const dyNext = evNext.point[1] - q[1];
        const d2Next = dxNext * dxNext + dyNext * dyNext;
        if (d2Next < d2) {
          u = uNext;
          ev = evNext;
          dx = dxNext;
          dy = dyNext;
          d2 = d2Next;
          improved = true;
          break;
        }
        alpha *= 0.5;
      }
      if (!improved) break;
    }

    if (d2 < bestDistSq) {
      bestU = u;
      bestEval = ev;
      bestDistSq = d2;
    }
  }

  return { u: bestU, ...bestEval, distSq: bestDistSq };
}

function parseLineNode(node: XmlNode, staStart: number, linearToMeters: number): LineSegment | undefined {
  const start = parseNorthingEastingPoint(childText(node, 'Start'), linearToMeters);
  const end = parseNorthingEastingPoint(childText(node, 'End'), linearToMeters);
  if (!start || !end) return undefined;
  const dx = end[0] - start[0];
  const dy = end[1] - start[1];
  const geomLen = Math.hypot(dx, dy);
  const attrLen = parseFiniteNumber(node.attrs.length);
  const length = geomLen > 1e-9 ? geomLen : (attrLen !== undefined ? attrLen * linearToMeters : 0);
  if (length <= 1e-9 || geomLen <= 1e-9) return undefined;
  return {
    kind: 'line',
    staStart,
    staEnd: staStart + length,
    length,
    start,
    end,
    tangent: [dx / geomLen, dy / geomLen]
  };
}

function parseCurveNode(node: XmlNode, staStart: number, linearToMeters: number): CurveSegment | undefined {
  const start = parseNorthingEastingPoint(childText(node, 'Start'), linearToMeters);
  const center = parseNorthingEastingPoint(childText(node, 'Center'), linearToMeters);
  const end = parseNorthingEastingPoint(childText(node, 'End'), linearToMeters);
  if (!start || !center || !end) return undefined;

  const rot = parseTurnDirection(node.attrs.rot);
  const rStart = Math.hypot(start[0] - center[0], start[1] - center[1]);
  const rAttr = parseFiniteNumber(node.attrs.radius);
  const radius = rStart > 1e-9 ? rStart : (rAttr !== undefined ? Math.abs(rAttr) * linearToMeters : 0);
  if (radius <= 1e-9) return undefined;

  const startAngle = Math.atan2(start[1] - center[1], start[0] - center[0]);
  const endAngle = Math.atan2(end[1] - center[1], end[0] - center[0]);
  let absSweep =
    rot === 'ccw'
      ? wrapAnglePositive(endAngle - startAngle)
      : wrapAnglePositive(startAngle - endAngle);

  // Disambiguate full/near-full circles or >180° arcs if length attribute is provided.
  const attrLenRaw = parseFiniteNumber(node.attrs.length);
  if (attrLenRaw !== undefined && attrLenRaw > 0) {
    const attrSweep = (attrLenRaw * linearToMeters) / radius;
    if (absSweep < 1e-6 && attrSweep > Math.PI) {
      absSweep = 2 * Math.PI;
    } else if (Math.abs(attrSweep - absSweep) > Math.PI && Math.abs(attrSweep - (2 * Math.PI - absSweep)) < 0.1) {
      absSweep = 2 * Math.PI - absSweep;
    }
  }
  if (absSweep <= 1e-9) return undefined;

  const length = radius * absSweep;
  const sweepAngle = rot === 'ccw' ? absSweep : -absSweep;
  return {
    kind: 'curve',
    staStart,
    staEnd: staStart + length,
    length,
    rot,
    radius,
    start,
    center,
    end,
    startAngle,
    sweepAngle
  };
}

function parseSpiralNode(node: XmlNode, staStart: number, linearToMeters: number): SpiralSegment | undefined {
  const start = parseNorthingEastingPoint(childText(node, 'Start'), linearToMeters);
  const end = parseNorthingEastingPoint(childText(node, 'End'), linearToMeters);
  const lengthRaw = parseFiniteNumber(node.attrs.length);
  if (!start || !end || lengthRaw === undefined || lengthRaw <= 0) return undefined;

  const length = lengthRaw * linearToMeters;
  const rot = parseTurnDirection(node.attrs.rot);
  const sign = rot === 'ccw' ? 1 : -1;
  const spiType = (node.attrs.spiType ?? 'clothoid').trim().toLowerCase();

  const radiusStart = parseRadiusMeters(node.attrs.radiusStart, linearToMeters);
  const radiusEnd = parseRadiusMeters(node.attrs.radiusEnd, linearToMeters);
  const k0 = Number.isFinite(radiusStart) ? sign / radiusStart : 0;
  const k1 = Number.isFinite(radiusEnd) ? sign / radiusEnd : 0;

  const constRaw = parseFiniteNumber(node.attrs.constant);
  const constant =
    constRaw !== undefined
      ? Math.abs(constRaw) * linearToMeters
      : Number.isFinite(radiusEnd) && !Number.isFinite(radiusStart)
        ? Math.sqrt(radiusEnd * length)
        : Number.isFinite(radiusStart) && !Number.isFinite(radiusEnd)
          ? Math.sqrt(radiusStart * length)
          : undefined;

  // Fit initial heading theta0 directly from the exact world chord (start -> end) vs the
  // integrated local clothoid chord. This guarantees C0 continuity at both ends and avoids any
  // azimuth convention ambiguity across CAD exporters (Istram, Civil 3D, OpenRoads, Allplan).
  const [locEndX, locEndY] = integrateClothoidLocal(k0, k1, length, length);
  const worldDx = end[0] - start[0];
  const worldDy = end[1] - start[1];
  if (Math.hypot(worldDx, worldDy) <= 1e-9 || Math.hypot(locEndX, locEndY) <= 1e-9) {
    return undefined;
  }
  const theta0 = Math.atan2(worldDy, worldDx) - Math.atan2(locEndY, locEndX);

  return {
    kind: 'spiral',
    staStart,
    staEnd: staStart + length,
    length,
    rot,
    spiType,
    radiusStart,
    radiusEnd,
    k0,
    k1,
    constant,
    start,
    end,
    theta0
  };
}

function computeSegmentBounds(seg: AlignmentSegment, out: Bounds2D): void {
  expandBounds(out, seg.start);
  expandBounds(out, seg.end);
  if (seg.kind === 'curve') {
    // Check cardinal directions (0, π/2, π, 3π/2) that lie inside the arc sweep.
    const absSweep = Math.abs(seg.sweepAngle);
    const cardinals = [0, Math.PI / 2, Math.PI, (3 * Math.PI) / 2];
    for (const c of cardinals) {
      const delta =
        seg.rot === 'ccw'
          ? wrapAnglePositive(c - seg.startAngle)
          : wrapAnglePositive(seg.startAngle - c);
      if (delta > 0 && delta < absSweep) {
        expandBounds(out, [
          seg.center[0] + seg.radius * Math.cos(c),
          seg.center[1] + seg.radius * Math.sin(c)
        ]);
      }
    }
  } else if (seg.kind === 'spiral') {
    const steps = Math.max(8, Math.min(64, Math.ceil(seg.length / 5)));
    for (let i = 1; i < steps; i++) {
      expandBounds(out, evaluateSpiral(seg, (seg.length * i) / steps).point);
    }
  }
}

/**
 * Parses a LandXML 1.2 document and extracts all horizontal alignments with coordinates,
 * stations and lengths normalized to meters (`f64`).
 */
export function parseLandXml(xml: string): LandXmlDocument {
  const root = parseXml(xml);
  const units = parseUnits(root);
  const warnings: AlignmentWarning[] = [];
  const alignments: HorizontalAlignment[] = [];

  const alignmentsGroups = children(root, 'Alignments');
  for (const group of alignmentsGroups) {
    for (const alignNode of children(group, 'Alignment')) {
      const name = alignNode.attrs.name?.trim() || `Alignment_${alignments.length + 1}`;
      const desc = alignNode.attrs.desc?.trim() || undefined;
      const staStartAttr = parseFiniteNumber(alignNode.attrs.staStart);
      const staStart = (staStartAttr ?? 0) * units.linearToMeters;

      const coordGeom = child(alignNode, 'CoordGeom');
      if (!coordGeom) continue;

      const segments: AlignmentSegment[] = [];
      let cursorSta = staStart;
      const bounds = emptyBounds();

      for (const geomNode of coordGeom.children) {
        const segStaAttr = parseFiniteNumber(geomNode.attrs.staStart);
        const segStaStart =
          segStaAttr !== undefined ? segStaAttr * units.linearToMeters : cursorSta;
        let seg: AlignmentSegment | undefined;
        if (geomNode.tag === 'Line') {
          seg = parseLineNode(geomNode, segStaStart, units.linearToMeters);
        } else if (geomNode.tag === 'Curve') {
          seg = parseCurveNode(geomNode, segStaStart, units.linearToMeters);
        } else if (geomNode.tag === 'Spiral') {
          seg = parseSpiralNode(geomNode, segStaStart, units.linearToMeters);
        } else {
          warnings.push({
            code: 'unsupported-segment',
            alignmentName: name,
            message: `Unsupported CoordGeom element <${geomNode.tag}> skipped`
          });
          continue;
        }

        if (!seg) {
          warnings.push({
            code: 'skipped-segment',
            alignmentName: name,
            message: `Malformed or degenerate <${geomNode.tag}> skipped`
          });
          continue;
        }

        segments.push(seg);
        computeSegmentBounds(seg, bounds);
        cursorSta = seg.staEnd;
      }

      if (segments.length === 0) continue;

      const resolvedStaStart = staStartAttr !== undefined ? staStart : segments[0].staStart;
      const resolvedLength = segments.reduce((sum, s) => sum + s.length, 0);

      alignments.push({
        name,
        desc,
        staStart: resolvedStaStart,
        staEnd: cursorSta,
        length: resolvedLength,
        segments,
        bounds2D: bounds
      });
    }
  }

  return { units, alignments, warnings };
}

/** Evaluates position, unit tangent, right-hand normal and curvature at a given station (meters). */
export function evaluateAlignment(alignment: HorizontalAlignment, station: number): AlignmentSample {
  if (alignment.segments.length === 0) {
    throw new Error(`Alignment "${alignment.name}" has no horizontal segments`);
  }

  let idx = 0;
  let clampedSta = station;
  if (station <= alignment.staStart) {
    idx = 0;
    clampedSta = alignment.staStart;
  } else if (station >= alignment.staEnd) {
    idx = alignment.segments.length - 1;
    clampedSta = alignment.staEnd;
  } else {
    // Binary search on segment station intervals (handles station gaps cleanly).
    let lo = 0;
    let hi = alignment.segments.length - 1;
    let found = false;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const s = alignment.segments[mid];
      if (station < s.staStart) {
        hi = mid - 1;
      } else if (station > s.staEnd) {
        lo = mid + 1;
      } else {
        idx = mid;
        found = true;
        break;
      }
    }
    if (!found) {
      // Station fell inside a gap between segments[hi].staEnd and segments[lo].staStart: pick nearer.
      if (hi >= 0 && lo < alignment.segments.length) {
        const dHi = Math.abs(station - alignment.segments[hi].staEnd);
        const dLo = Math.abs(alignment.segments[lo].staStart - station);
        idx = dHi <= dLo ? hi : lo;
      } else {
        idx = Math.max(0, Math.min(alignment.segments.length - 1, hi >= 0 ? hi : lo));
      }
    }
  }

  const seg = alignment.segments[idx];
  const u = Math.max(0, Math.min(seg.length, clampedSta - seg.staStart));
  const ev = evaluateSegment(seg, u);
  const rightNormal: Vec2 = [ev.tangent[1], -ev.tangent[0]];
  return {
    station: seg.staStart + u,
    point: ev.point,
    tangent: ev.tangent,
    rightNormal,
    curvature: ev.curvature,
    segmentIndex: idx
  };
}

/** Projects a 2D map-space point `(Easting, Northing)` onto the horizontal alignment in `f64`. */
export function projectPointOnAlignment(alignment: HorizontalAlignment, point: Vec2): AlignmentProjection {
  if (alignment.segments.length === 0) {
    throw new Error(`Alignment "${alignment.name}" has no horizontal segments`);
  }

  let bestDistSq = Infinity;
  let bestIdx = 0;
  let bestU = 0;
  let bestPoint: Vec2 = alignment.segments[0].start;
  let bestTangent: Vec2 = [1, 0];
  let bestCurvature = 0;

  for (let i = 0; i < alignment.segments.length; i++) {
    const seg = alignment.segments[i];
    const res =
      seg.kind === 'line'
        ? projectOnLine(seg, point)
        : seg.kind === 'curve'
          ? projectOnCurve(seg, point)
          : projectOnSpiral(seg, point);
    if (res.distSq < bestDistSq) {
      bestDistSq = res.distSq;
      bestIdx = i;
      bestU = res.u;
      bestPoint = res.point;
      bestTangent = res.tangent;
      bestCurvature = res.curvature;
    }
  }

  const seg = alignment.segments[bestIdx];
  const station = seg.staStart + bestU;
  const rightNormal: Vec2 = [bestTangent[1], -bestTangent[0]];
  const dx = point[0] - bestPoint[0];
  const dy = point[1] - bestPoint[1];
  const signedOffset = dx * rightNormal[0] + dy * rightNormal[1];
  const clamped =
    (bestIdx === 0 && bestU <= 1e-9) ||
    (bestIdx === alignment.segments.length - 1 && bestU >= seg.length - 1e-9);

  return {
    station,
    point: bestPoint,
    tangent: bestTangent,
    rightNormal,
    curvature: bestCurvature,
    segmentIndex: bestIdx,
    distance: Math.sqrt(bestDistSq),
    signedOffset,
    clamped
  };
}

/** Extracts the horizontal 2D affine transform from a 4x4 column-major `localToMap` matrix. */
export function extractAffine2DFromLocalToMap(localToMap: readonly number[]): Affine2D | null {
  if (localToMap.length !== 16) return null;
  const a00 = localToMap[0];
  const a10 = localToMap[1];
  const a01 = localToMap[4];
  const a11 = localToMap[5];
  const tx = localToMap[12];
  const ty = localToMap[13];
  if (
    !Number.isFinite(a00) ||
    !Number.isFinite(a10) ||
    !Number.isFinite(a01) ||
    !Number.isFinite(a11) ||
    !Number.isFinite(tx) ||
    !Number.isFinite(ty)
  ) {
    return null;
  }
  const det = a00 * a11 - a01 * a10;
  if (!Number.isFinite(det) || Math.abs(det) < 1e-18) return null;
  return { a00, a10, a01, a11, tx, ty };
}

/** Exact `f64` inverse of a 2D affine transform. */
export function invertAffine2D(m: Affine2D): Affine2D | null {
  const det = m.a00 * m.a11 - m.a01 * m.a10;
  if (!Number.isFinite(det) || Math.abs(det) < 1e-18) return null;
  const invDet = 1 / det;
  const a00 = m.a11 * invDet;
  const a01 = -m.a01 * invDet;
  const a10 = -m.a10 * invDet;
  const a11 = m.a00 * invDet;
  const tx = -(a00 * m.tx + a01 * m.ty);
  const ty = -(a10 * m.tx + a11 * m.ty);
  return { a00, a10, a01, a11, tx, ty };
}

export function applyAffine2DPoint(m: Affine2D, p: Vec2): Vec2 {
  return [m.a00 * p[0] + m.a01 * p[1] + m.tx, m.a10 * p[0] + m.a11 * p[1] + m.ty];
}

export function applyAffine2DUnitVector(m: Affine2D, v: Vec2): Vec2 {
  const vx = m.a00 * v[0] + m.a01 * v[1];
  const vy = m.a10 * v[0] + m.a11 * v[1];
  const len = Math.hypot(vx, vy);
  return len > 1e-15 ? [vx / len, vy / len] : [1, 0];
}

function transformBounds2D(bounds: Bounds2D, m: Affine2D): Bounds2D {
  const out = emptyBounds();
  const corners: Vec2[] = [
    [bounds.minX, bounds.minY],
    [bounds.maxX, bounds.minY],
    [bounds.maxX, bounds.maxY],
    [bounds.minX, bounds.maxY]
  ];
  for (const c of corners) expandBounds(out, applyAffine2DPoint(m, c));
  return out;
}

function boundsOverlap2D(a: Bounds2D, b: ModelBounds): boolean {
  return !(a.maxX < b.min.x || a.minX > b.max.x || a.maxY < b.min.y || a.minY > b.max.y);
}

/**
 * Binds a map-space `HorizontalAlignment` to an IFC model's local coordinate frame using the
 * inverse of `coordinateReference.localToMap` in `f64`. When `localToMap` is absent, assumes a
 * common frame and emits a warning if the 2D bounding boxes do not overlap.
 */
export function bindAlignmentToModel(
  alignment: HorizontalAlignment,
  coordinateReference?: CoordinateReference,
  modelBounds?: ModelBounds
): BoundAlignment {
  const warnings: AlignmentWarning[] = [];
  let localToMap2D = IDENTITY_AFFINE_2D;
  let mapToLocal2D = IDENTITY_AFFINE_2D;
  let hasMapConversion = false;

  if (coordinateReference?.localToMap) {
    const fwd = extractAffine2DFromLocalToMap(coordinateReference.localToMap);
    const inv = fwd ? invertAffine2D(fwd) : null;
    if (fwd && inv) {
      localToMap2D = fwd;
      mapToLocal2D = inv;
      hasMapConversion = true;
    } else {
      warnings.push({
        code: 'invalid-local-to-map',
        alignmentName: alignment.name,
        message: 'Model localToMap matrix is not invertible; falling back to identity frame'
      });
    }
  }

  const localBounds2D = hasMapConversion
    ? transformBounds2D(alignment.bounds2D, mapToLocal2D)
    : { ...alignment.bounds2D };

  if (!hasMapConversion && modelBounds && !boundsOverlap2D(localBounds2D, modelBounds)) {
    warnings.push({
      code: 'no-map-conversion-disjoint-bounds',
      alignmentName: alignment.name,
      message:
        'Model has no IfcMapConversion and alignment 2D bounds do not overlap the model bounding box'
    });
  }

  const evaluateLocal = (station: number): AlignmentSample => {
    const mapSample = evaluateAlignment(alignment, station);
    const point = hasMapConversion ? applyAffine2DPoint(mapToLocal2D, mapSample.point) : mapSample.point;
    const tangent = hasMapConversion
      ? applyAffine2DUnitVector(mapToLocal2D, mapSample.tangent)
      : mapSample.tangent;
    const rightNormal: Vec2 = [tangent[1], -tangent[0]];
    return {
      station: mapSample.station,
      point,
      tangent,
      rightNormal,
      curvature: mapSample.curvature,
      segmentIndex: mapSample.segmentIndex
    };
  };

  const projectLocal = (x: number, y: number): AlignmentProjection => {
    const mapQuery: Vec2 = hasMapConversion ? applyAffine2DPoint(localToMap2D, [x, y]) : [x, y];
    const mapProj = projectPointOnAlignment(alignment, mapQuery);
    if (!hasMapConversion) return mapProj;

    const point = applyAffine2DPoint(mapToLocal2D, mapProj.point);
    const tangent = applyAffine2DUnitVector(mapToLocal2D, mapProj.tangent);
    const rightNormal: Vec2 = [tangent[1], -tangent[0]];
    const dx = x - point[0];
    const dy = y - point[1];
    return {
      station: mapProj.station,
      point,
      tangent,
      rightNormal,
      curvature: mapProj.curvature,
      segmentIndex: mapProj.segmentIndex,
      distance: Math.hypot(dx, dy),
      signedOffset: dx * rightNormal[0] + dy * rightNormal[1],
      clamped: mapProj.clamped
    };
  };

  return {
    alignment,
    hasMapConversion,
    localToMap2D,
    mapToLocal2D,
    localBounds2D,
    warnings,
    evaluateLocal,
    projectLocal
  };
}

/**
 * Formats a station in meters into standard civil engineering PK notation (`K+MMM.mm`),
 * supporting negative stations (e.g. `-1446.73` → `"-1+446.73"`).
 */
export function formatStationPk(stationMeters: number, decimals = 2): string {
  const sign = stationMeters < 0 ? '-' : '';
  const abs = Math.abs(stationMeters);
  const rounded = Number(abs.toFixed(decimals));
  const km = Math.floor(rounded / 1000);
  const meters = rounded - km * 1000;
  const wholeMeters = Math.floor(meters);
  const fracStr = decimals > 0 ? meters.toFixed(decimals).slice(String(wholeMeters).length) : '';
  return `${sign}${km}+${String(wholeMeters).padStart(3, '0')}${fracStr}`;
}
