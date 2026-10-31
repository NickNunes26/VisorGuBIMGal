import type { ElementRef, ModelBounds, ModelId } from '../../domain/models/ModelRecord';
import type { GeometryBatch } from '../../domain/kernel/GeometryBatch';
import type {
  RendererEventHandlers,
  RendererPort,
  ViewPreset
} from '../../domain/kernel/RendererPort';
import { bimEngine } from './BimEngine';

export class IfcLiteRendererAdapter implements RendererPort {
  initialize(canvas: HTMLCanvasElement): Promise<void> {
    return bimEngine.initialize(canvas);
  }

  setHandlers(handlers: RendererEventHandlers): void {
    bimEngine.setHandlers(handlers);
  }

  addModel(modelId: ModelId): void {
    bimEngine.addModel(modelId);
  }

  addBatch(batch: GeometryBatch): void {
    bimEngine.addBatch(batch);
  }

  finishModel(modelId: ModelId, bounds: ModelBounds | undefined): void {
    bimEngine.finishModel(modelId, bounds);
  }

  removeModel(modelId: ModelId): void {
    bimEngine.removeModel(modelId);
  }

  setModelVisibility(modelId: ModelId, visible: boolean): void {
    bimEngine.setModelVisibility(modelId, visible);
  }

  setElementVisibility(refs: readonly ElementRef[], visible: boolean): void {
    bimEngine.setElementVisibility(refs, visible);
  }

  setSelection(ref: ElementRef | null): void {
    bimEngine.setSelection(ref);
  }

  setViewPreset(preset: ViewPreset): void {
    bimEngine.setViewPreset(preset);
  }

  fitToView(): void {
    bimEngine.fitToView();
  }

  destroy(): void {
    bimEngine.destroy();
  }
}
