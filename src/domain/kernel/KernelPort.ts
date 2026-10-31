import type {
  CoordinateReference,
  ElementRef,
  ModelBounds,
  ModelId,
  SelectedElementDetails
} from '../models/ModelRecord';
import type { DrawingExportConfig, DrawingOptions } from '../drawings/DrawingTypes';
import type { GeometryBatch } from './GeometryBatch';

export type Vec3d = readonly [number, number, number];

export type ProgressHandler = (phase: string, percent: number) => void;

export interface KernelDiagnostic {
  severity: 'info' | 'warning' | 'error';
  code: string;
  message: string;
  expressId?: number;
}

export type KernelErrorCode =
  | 'parse'
  | 'unsupported-schema'
  | 'out-of-memory'
  | 'model-not-open'
  | 'internal';

/** Thrown for failures of a whole operation. Per-element problems are diagnostics instead. */
export class KernelError extends Error {
  constructor(
    readonly code: KernelErrorCode,
    message: string,
    /** Byte offset in the IFC file, for 'parse'. */
    readonly offset?: number
  ) {
    super(message);
    this.name = 'KernelError';
  }
}

export interface ModelSummary {
  schema: string;
  /** Every entity in the file. */
  entityCount: number;
  /** IfcProduct instances that can appear in the viewer. */
  elementCount: number;
  /** Metres per file length unit: 0.001 for millimetres. */
  lengthUnitScale: number;
  /** Subtracted from model coordinates before vertices become f32. See docs/05 section 2. */
  originShift: Vec3d;
  coordinateReference: CoordinateReference;
  diagnostics: KernelDiagnostic[];
}

export type GeometryEvent =
  | { type: 'batch'; batch: GeometryBatch }
  | { type: 'progress'; processedElements: number; totalElements: number }
  | {
      type: 'complete';
      meshCount: number;
      /** Absolute model coordinates, undefined when the model has no geometry. */
      bounds?: ModelBounds;
      diagnostics: KernelDiagnostic[];
    };

export interface SpatialElement {
  expressId: number;
  type: string;
  name?: string;
}

export interface SpatialNode {
  expressId: number;
  /** IfcProject, IfcSite, IfcBuilding, IfcBuildingStorey, IfcSpace... */
  type: string;
  name?: string;
  longName?: string;
  /** Metres, for storeys. */
  elevation?: number;
  children: SpatialNode[];
  elements: SpatialElement[];
}

export type ElementDetails = Omit<SelectedElementDetails, 'modelName'>;

export type DrawingId = string;

export interface ExportedDrawing {
  content: string;
  mimeType: string;
  filename: string;
}

/**
 * Everything the app may ask of the kernel. Implementations run off the main thread, so
 * every method is asynchronous. Nothing here exposes kernel internals.
 */
export interface KernelPort {
  /** Takes ownership of `bytes`: the buffer is transferred and detached for the caller. */
  openModel(
    modelId: ModelId,
    bytes: ArrayBuffer,
    onProgress?: ProgressHandler
  ): Promise<ModelSummary>;

  /** Ends with exactly one 'complete' event. closeModel ends it early without an error. */
  streamGeometry(modelId: ModelId): AsyncIterable<GeometryEvent>;

  getElementDetails(ref: ElementRef): Promise<ElementDetails | null>;

  /** The IfcProject root. */
  getSpatialTree(modelId: ModelId): Promise<SpatialNode | null>;

  /** Cuts the given models together, in absolute model coordinates. */
  generateDrawing(
    modelIds: readonly ModelId[],
    options: DrawingOptions,
    onProgress?: ProgressHandler
  ): Promise<DrawingId>;

  /** SVG output is safe to inject into the DOM: see docs/05 section 5. */
  exportDrawing(drawingId: DrawingId, config: DrawingExportConfig): Promise<ExportedDrawing>;

  discardDrawing(drawingId: DrawingId): Promise<void>;

  /** Frees everything held for the model. Calling it twice is harmless. */
  closeModel(modelId: ModelId): Promise<void>;
}
