import type { ElementRef, ModelBounds, ModelId } from '../models/ModelRecord';
import type { GeometryBatch } from './GeometryBatch';

export type ViewPreset = 'iso' | 'top' | 'front' | 'side';

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
  setSelection(ref: ElementRef | null): void;

  setViewPreset(preset: ViewPreset): void;
  fitToView(): void;

  /** Releases GPU resources and removes every listener it added. */
  destroy(): void;
}
