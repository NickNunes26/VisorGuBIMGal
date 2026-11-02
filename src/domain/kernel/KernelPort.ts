import type {
  AttributeValue,
  CoordinateReference,
  ElementRef,
  MaterialInfo,
  ModelBounds,
  ModelId,
  SelectedElementDetails
} from '../models/ModelRecord';
import type { DrawingExportConfig, DrawingFrame, DrawingOptions } from '../drawings/DrawingTypes';
import type { GeometryBatch } from './GeometryBatch';

export type { AttributeValue };

export type Vec3d = readonly [number, number, number];

export type ProgressHandler = (phase: string, percent: number) => void;

export type SchemaId = 'IFC2X3' | 'IFC4' | 'IFC4X3';

/**
 * A problem that does not stop the operation. With `expressId` it concerns that element,
 * without it the model `modelId`, and without either a whole drawing. `code` comes from an
 * open set: show unknown ones as they are.
 */
export interface KernelDiagnostic {
  severity: 'info' | 'warning' | 'error';
  code: string;
  message: string;
  modelId?: ModelId;
  expressId?: number;
}

export type KernelErrorCode =
  | 'parse'
  | 'unsupported-schema'
  | 'out-of-memory'
  | 'model-not-open'
  | 'drawing-not-found'
  | 'drawing-frame-empty'
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
  schema: SchemaId;
  /** The FILE_SCHEMA text as written, e.g. 'IFC4X3_ADD2'. */
  schemaIdentifier: string;
  /** Every entity in the file. */
  entityCount: number;
  /** IfcProduct instances with a Representation, excluding IfcFeatureElementSubtraction. */
  elementCount: number;
  /** Metres per file length unit, resolved from IfcProject.UnitsInContext. */
  lengthUnitScale: number;
  /** 'default' means the file declared no length unit and metres were assumed. */
  lengthUnitSource: 'ifc' | 'default';
  coordinateReference: CoordinateReference;
  diagnostics: KernelDiagnostic[];
}

export type GeometryEvent =
  | { type: 'batch'; batch: GeometryBatch }
  | { type: 'progress'; processedElements: number; totalElements: number }
  | {
      type: 'complete';
      /** meshTable records, not elements. */
      meshCount: number;
      /** Absolute model coordinates of everything not HiddenByDefault. Undefined if nothing is. */
      bounds?: ModelBounds;
      diagnostics: KernelDiagnostic[];
    };

export interface SpatialElement {
  expressId: number;
  type: string;
  name?: string;
  /** 'references' is IfcRelReferencedInSpatialStructure: the element also appears at its container. */
  via: 'contains' | 'references';
}

export interface SpatialNode {
  expressId: number;
  /** IfcProject, IfcSite, IfcBuilding, IfcBuildingStorey, IfcSpace... */
  type: string;
  name?: string;
  longName?: string;
  /** Metres, for storeys. */
  elevation?: number;
  /** Ascending expressId. */
  children: SpatialNode[];
  /** Ascending expressId. Excludes IfcFeatureElementSubtraction. */
  elements: SpatialElement[];
}

export interface SpatialTree {
  root: SpatialNode;
  /** Products with no 'contains' relation, ascending expressId. */
  unassigned: SpatialElement[];
}

export type ElementDetails = Omit<SelectedElementDetails, 'modelName' | 'attributes'> & {
  /** Keyed by EXPRESS attribute name. */
  attributes: Record<string, AttributeValue>;
  materials: MaterialInfo[];
};

export type DrawingId = string;

export interface GeneratedDrawing {
  drawingId: DrawingId;
  frame: DrawingFrame;
  /** The verified CRS shared by every included model, for a 'map' frame. */
  crsId?: string;
  diagnostics: KernelDiagnostic[];
}

export interface ExportedDrawing {
  /** SVG is text. DXF is encoded bytes: pass them to Blob as they are. */
  content: string | Uint8Array;
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

  /**
   * A stream that runs to its end emits exactly one 'complete' event. closeModel ends it early
   * with no error and no 'complete', which is how the two cases differ.
   */
  streamGeometry(modelId: ModelId): AsyncIterable<GeometryEvent>;

  getElementDetails(ref: ElementRef): Promise<ElementDetails | null>;

  getSpatialTree(modelId: ModelId): Promise<SpatialTree | null>;

  /**
   * Cuts the given models together in the frame named by `options`, so hidden lines work
   * across models. An empty list means every open model, in opening order. The same models
   * in the same order give identical bytes. Rejects only when no model is left: docs/05 section 5.
   */
  generateDrawing(
    modelIds: readonly ModelId[],
    options: DrawingOptions,
    onProgress?: ProgressHandler
  ): Promise<GeneratedDrawing>;

  /** Serialises only. SVG output is safe to inject: see docs/05 section 5. */
  exportDrawing(drawingId: DrawingId, config: DrawingExportConfig): Promise<ExportedDrawing>;

  /** Drawings outlive the models they came from until this is called. Idempotent. */
  discardDrawing(drawingId: DrawingId): Promise<void>;

  /** Frees everything held for the model. Calling it twice is harmless. */
  closeModel(modelId: ModelId): Promise<void>;
}
