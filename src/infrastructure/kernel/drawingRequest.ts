import type { DrawingOptions } from '../../domain/drawings/DrawingTypes';
import type { CoordinateReference, ModelId } from '../../domain/models/ModelRecord';
import type { KernelDiagnostic } from '../../domain/kernel/KernelPort';

/** The flat numbers of `DrawingRequest::decode`. Models also get their `localToMap`. */
export function drawingRequest(options: DrawingOptions, localToMap?: readonly number[]): number[] {
  const { cut } = options;
  const kind = cut.kind === 'plan' ? 0 : cut.axis === 'x' ? 1 : 2;
  const position = cut.kind === 'plan' ? cut.elevation : cut.position;
  const look = cut.kind === 'section' && cut.look === 'positive' ? 1 : -1;
  return [
    kind,
    position,
    look,
    cut.depth ?? Number.NaN,
    options.includeProjection ? 1 : 0,
    options.includeHatching ? 1 : 0,
    options.includeProjection && options.includeHiddenLines ? 1 : 0,
    ...(localToMap ?? [])
  ];
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

/** Applies the frame rules of `docs/05` section 5 to a list of models. */
export function frameModels(
  frame: DrawingOptions['frame'],
  models: ReadonlyArray<{ modelId: ModelId; reference?: CoordinateReference }>
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
  models.forEach(({ modelId, reference }, index) => {
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
