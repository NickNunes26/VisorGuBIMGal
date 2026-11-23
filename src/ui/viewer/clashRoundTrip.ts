/**
 * DXF round-trip change log and IFC STEP patch helpers for Phase F5
 * (`docs/12_clash_alignment_dxf.md` §3.5).
 */
import type {
  ClashDxfEditItem,
  ClashDxfMoveEdit,
  ClashDxfRebendEdit
} from '../../domain/clash/ClashTypes';

export type DxfMoveEdit = ClashDxfMoveEdit;
export type DxfRebendEdit = ClashDxfRebendEdit;
export type DxfRoundTripEditItem = ClashDxfEditItem;

export interface DxfRoundTripDiffResult {
  modelId: string;
  pkM: number;
  applied?: number;
  skipped?: number;
  edits: DxfRoundTripEditItem[];
}

/**
 * Extracts the `model=<modelId>` token from a clash DXF's `GUBIM_META` comment or text entity,
 * allowing multi-model sessions to route an edited DXF directly to the model it was sliced from.
 */
export function extractMetaModelIdFromDxf(dxfBytes: Uint8Array): string | null {
  const text = new TextDecoder('utf-8', { fatal: false }).decode(dxfBytes);
  const match = /GUBIM_META\s+[^\r\n]*?\bmodel=([^\s\r\n]+)/.exec(text);
  return match?.[1]?.trim() || null;
}

/**
 * Packs Level 1 move edits into flat `[expressId, dx, dy, dz, ...]` scaled from metres into
 * the IFC file's length units (`1 / lengthUnitScale`).
 */
export function packMoveEditsForStepPatch(
  edits: readonly DxfRoundTripEditItem[],
  lengthUnitScale = 1
): Float64Array {
  const invScale = Number.isFinite(lengthUnitScale) && lengthUnitScale > 0 ? 1 / lengthUnitScale : 1;
  const moves = edits.filter((e): e is DxfMoveEdit => e.kind === 'move');
  const flat = new Float64Array(moves.length * 4);
  for (let i = 0; i < moves.length; i++) {
    const m = moves[i];
    flat[i * 4] = m.expressId;
    flat[i * 4 + 1] = m.delta3d[0] * invScale;
    flat[i * 4 + 2] = m.delta3d[1] * invScale;
    flat[i * 4 + 3] = m.delta3d[2] * invScale;
  }
  return flat;
}

/** Formats a CSV audit log of all applied DXF round-trip displacements/rebends. */
export function buildRoundTripAuditCsv(diff: DxfRoundTripDiffResult): string {
  const header = 'model_id,pk_m,kind,express_id,dx_mm,dy_mm,dz_mm,detail';
  const rows = diff.edits.map((e) => {
    if (e.kind === 'move') {
      return [
        diff.modelId,
        diff.pkM.toFixed(3),
        'move',
        String(e.expressId),
        (e.delta3d[0] * 1000).toFixed(2),
        (e.delta3d[1] * 1000).toFixed(2),
        (e.delta3d[2] * 1000).toFixed(2),
        e.blockName
      ].join(',');
    }
    return [
      diff.modelId,
      diff.pkM.toFixed(3),
      'rebend',
      String(e.expressId),
      '0.00',
      '0.00',
      '0.00',
      `${e.points3d.length} vertices`
    ].join(',');
  });
  return [header, ...rows].join('\r\n') + '\r\n';
}
