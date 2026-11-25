import type { ElementRef, ModelBounds, ModelId } from '../models/ModelRecord';
import type { KernelDiagnostic, Vec3d } from '../kernel/KernelPort';

/** Elements from one open model participating in one side of a clash check. */
export interface ClashSetSpec {
  modelId: ModelId;
  /** ExpressIds to include (already filtered by IFC class / storey). */
  expressIds: readonly number[];
}

/** Kernel request for hard/soft clash detection across one or more models (`docs/11_clash.md`). */
export interface ClashRequest {
  setA: readonly ClashSetSpec[];
  setB: readonly ClashSetSpec[];
  /** Metres: 0 = hard clashes only; > 0 = hard + soft clashes within this clearance. */
  clearanceM: number;
  /** Optional upper cap on reported clashes (default 5000). */
  maxResults?: number;
}

export type ClashKind = 'hard' | 'soft';

/** One confirmed hard or soft clash pair in absolute model coordinates (metres, IFC Z-up). */
export interface ClashResultItem {
  elementA: ElementRef;
  elementB: ElementRef;
  kind: ClashKind;
  /** Penetration depth in metres (> 0) for hard clashes; 0 for soft clashes. */
  penetrationM: number;
  /** Minimum surface-to-surface gap in metres for soft clashes; 0 for hard clashes. */
  gapM: number;
  /** Exact CSG intersection volume in m³ when computed for closed hard clashes. */
  volumeM3?: number;
  /** Representative 3D point of the clash in absolute model coordinates. */
  point: Vec3d;
  /** Combined world AABB of the colliding pair, for framing/zoom. */
  bounds: ModelBounds;
}

export interface ClashReport {
  clashes: ClashResultItem[];
  broadPhasePairs: number;
  testedPairs: number;
  truncated: boolean;
  elapsedMs: number;
  diagnostics: KernelDiagnostic[];
}

/** Rule filter for one side of a clash test in the UI. */
export interface ClashFilter {
  /** Concrete ModelId or '*' for all ready models. */
  modelId: ModelId | '*';
  /** Allowed IFC classes; empty means all drawable classes except IfcSpace. */
  ifcClasses: string[];
  /** Allowed storey names; empty means all storeys. */
  storeys: string[];
}

/** User-configured clash detection rule. */
export interface ClashRule {
  name: string;
  setA: ClashFilter;
  setB: ClashFilter;
  /** Clearance in millimetres (0 = hard clashes only). */
  clearanceMm: number;
}

/** One planned cross-section request for F4 batch clash DXF slicing. */
export interface ClashAlignmentSectionSpec {
  fileName: string;
  pkM: number;
  pkLabel: string;
  origin: readonly [number, number, number];
  normal: readonly [number, number, number];
  alignmentPoint: readonly [number, number, number];
  sMin: number;
  sMax: number;
  depthM: number;
  modelId: ModelId;
  /** Parallel to `clashAIds`: owning model for each Element A (defaults to `modelId` when omitted). */
  clashAModelIds?: readonly ModelId[];
  clashAIds: readonly number[];
  clashAGuids: readonly string[];
  /** Parallel to `clashBIds`: owning model for each Element B (defaults to `modelId` when omitted). */
  clashBModelIds?: readonly ModelId[];
  clashBIds: readonly number[];
  clashPoints: readonly (readonly [number, number, number])[];
  annotations: readonly string[];
}

export interface ClashAlignmentDxfRequest {
  modelIds: readonly ModelId[];
  sections: readonly ClashAlignmentSectionSpec[];
  /** Include the non-overlapping master grid `clashes_all.dxf` (default true). */
  includeMasterGrid?: boolean;
  /** Fixed grid columns (0 = auto ceil(sqrt(n))). */
  gridColumns?: number;
  /** Gap between grid cells in metres (default 1.0). */
  gridMarginM?: number;
}

export interface ClashAlignmentDxfSectionOutput {
  fileName: string;
  pkM: number;
  pkLabel: string;
  dxfBytes: Uint8Array;
  svg: string;
  widthM: number;
  heightM: number;
}

export interface ClashAlignmentDxfBatch {
  sections: ClashAlignmentDxfSectionOutput[];
  masterDxfBytes?: Uint8Array;
}

/**
 * What happened to one DXF edit. `skipped` reasons: `guid-mismatch`, `guid-missing`,
 * `not-found`, `not-a-product`, `unsupported-placement`, `no-representation-context`,
 * `invalid-points`, `invalid-radius`, `duplicate-edit`, `already-applied`, `metadata-incomplete`,
 * `invalid-number`, `unsupported-transform`, `duplicate-insert`, `duplicate-polyline`,
 * `ambiguous-element`, `out-of-range`, `other-model`, `edit-conflict` (another section edited the
 * element after this DXF was exported; see {@link ClashDxfConflictPolicy}).
 */
export interface ClashDxfEditStatus {
  status?: 'applied' | 'skipped' | 'detected';
  reason?: string | null;
  /** `GlobalId` (or its truncated block-name prefix) the edit was checked against. */
  globalId?: string | null;
  /** Position in the DXF's edit list, the same for every model the DXF is applied to. */
  index?: number;
  /** Session id of the model the element was exported from, when the DXF records it. */
  owner?: string | null;
  /** Model the edit was applied to (or, when skipped, the model whose answer is reported). */
  modelId?: string;
}

/** Level 1 CAD MOVE edit detected on an exported `INSERT` (`GUBIM_A_<expressId>_<GlobalId>`). */
export interface ClashDxfMoveEdit extends ClashDxfEditStatus {
  kind: 'move';
  expressId: number;
  blockName: string;
  delta2d: [number, number];
  delta3d: [number, number, number];
}

/** Level 2 CAD polyline edit detected on an `EDIT_A_<expressId>` centerline layer. */
export interface ClashDxfRebendEdit extends ClashDxfEditStatus {
  kind: 'rebend';
  expressId: number;
  points3d: [number, number, number][];
}

export type ClashDxfEditItem = ClashDxfMoveEdit | ClashDxfRebendEdit;

/**
 * What a DXF import does with an element another section edited after the DXF was exported
 * (that drawing did not show the edit): `ask` leaves it alone with reason `edit-conflict`,
 * `add` applies this drawing's edit on top, `replace` undoes the other section's edit first,
 * and `skip` leaves it alone.
 */
export type ClashDxfConflictPolicy = 'ask' | 'add' | 'replace' | 'skip';

/** Result of applying an edited clash DXF to an open IFC model's STEP file (F5). */
export interface ClashDxfRoundTripResult {
  modelId: ModelId;
  /** `NaN` when the DXF metadata lost it (files exported before the 2026-11-24 fix). */
  pkM: number;
  applied: number;
  skipped: number;
  edits: ClashDxfEditItem[];
  patchedIfcBytes: Uint8Array;
  auditCsv: string;
  /** False for DXFs whose metadata was clipped on export: moves work, rebends are skipped. */
  metadataComplete?: boolean;
  /**
   * Every model this import patched (a section can hold elements of several federated
   * models). `modelId` / `patchedIfcBytes` above repeat the first of them.
   */
  patchedModels?: { modelId: ModelId; patchedIfcBytes: Uint8Array; applied: number }[];
}
