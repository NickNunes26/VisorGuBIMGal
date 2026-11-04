import type { ElementRef, ModelBounds, ModelId } from '../models/ModelRecord';
import type { GeometryBatch } from './GeometryBatch';

export type ViewPreset = 'iso' | 'top' | 'front' | 'side';

/**
 * How an element is drawn. 'default' is its own state: hidden if HiddenByDefault, else visible.
 * A ghost is drawn faint and cannot be picked; a HiddenByDefault element is never a ghost and
 * stays hidden instead.
 */
export type ElementDisplay = 'default' | 'visible' | 'ghost' | 'hidden';

/**
 * One half-space of a section, in absolute model coordinates (metres, IFC Z-up): it keeps the
 * points p with dot(normal, p) <= offset and cuts away the side the normal points to.
 */
export interface SectionPlane {
  normal: readonly [number, number, number];
  offset: number;
}

/** A section box takes six planes; no more are drawn. */
export const MAX_SECTION_PLANES = 6;

export interface RendererEventHandlers {
  onSelect?: (ref: ElementRef | null) => void;
  onHover?: (ref: ElementRef | null) => void;
}

/**
 * Drawing surface. Knows nothing about IFC: it receives batches and identities only.
 * It keeps no JS copy of a batch once uploaded, so geometry lives in one place, the GPU.
 */
export interface RendererPort {
  /** Rejects with a user-readable message when no supported graphics API exists. */
  initialize(canvas: HTMLCanvasElement): Promise<void>;
  setHandlers(handlers: RendererEventHandlers): void;

  addModel(modelId: ModelId): void;
  addBatch(batch: GeometryBatch): void;
  /** Frames the scene if this is the first visible model. */
  finishModel(modelId: ModelId, bounds: ModelBounds | undefined): void;
  /** Destroys the model's GPU resources. Survivors are not re-uploaded. */
  removeModel(modelId: ModelId): void;

  setModelVisibility(modelId: ModelId, visible: boolean): void;
  setElementVisibility(refs: readonly ElementRef[], visible: boolean): void;
  /**
   * Replaces how every element of the model is drawn, including batches still to come: the
   * elements in `entries`, by expressId, take their entry and the rest take `others`.
   */
  setElementDisplay(
    modelId: ModelId,
    others: ElementDisplay,
    entries: ReadonlyMap<number, ElementDisplay>
  ): void;
  setSelection(ref: ElementRef | null): void;
  /**
   * Cuts every model: only points that all planes keep are drawn or picked, ghosts included.
   * An empty list removes the section. Planes past MAX_SECTION_PLANES are ignored.
   */
  setSectionPlanes(planes: readonly SectionPlane[]): void;

  setViewPreset(preset: ViewPreset): void;
  fitToView(): void;

  /** Releases GPU resources and removes every listener it added. */
  destroy(): void;
}
