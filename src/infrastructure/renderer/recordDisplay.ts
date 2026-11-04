import { MeshFlag } from '../../domain/kernel/GeometryBatch';
import type { ElementDisplay } from '../../domain/kernel/RendererPort';

/** Record state bits above the kernel's own three; the shaders test the same values. */
export const FLAG_VISIBLE = 8;
export const FLAG_SELECTED = 16;
export const FLAG_GHOST = 32;

/** What both renderers keep per model to decide how its records are drawn. */
export interface DisplayState {
  /** 4 u32 words per record: [expressId, modelNumericId, rgba, stateFlags]. */
  recordWords: number[];
  /** Set by setElementDisplay and setElementVisibility: listed elements, then the rest. */
  displayEntries: Map<number, ElementDisplay>;
  displayOthers: ElementDisplay;
  /** Set with the display: false means no record of the model is a ghost. */
  displayMayGhost: boolean;
}

/** Replaces the model's display, leaving its records to be refreshed by the caller. */
export function setDisplay(
  model: DisplayState,
  others: ElementDisplay,
  entries: ReadonlyMap<number, ElementDisplay>
): void {
  model.displayOthers = others;
  model.displayEntries = new Map(entries);
  model.displayMayGhost = others === 'ghost';
  for (const display of entries.values()) {
    if (display === 'ghost') model.displayMayGhost = true;
  }
}

/** The visible and ghost bits of a record, from its model's display and its kernel flags. */
export function displayFlags(model: DisplayState, expressId: number, kernelFlags: number): number {
  const display = model.displayEntries.get(expressId) ?? model.displayOthers;
  const hiddenByDefault = (kernelFlags & MeshFlag.HiddenByDefault) !== 0;
  if (display === 'hidden' || (hiddenByDefault && display !== 'visible')) return 0;
  return display === 'ghost' ? FLAG_VISIBLE | FLAG_GHOST : FLAG_VISIBLE;
}

/** Recomputes one record's visible and ghost bits in place; true if they changed. */
export function refreshDisplayFlags(model: DisplayState, recordIndex: number): boolean {
  const wordIdx = recordIndex * 4 + 3;
  const prevFlags = model.recordWords[wordIdx];
  const expressId = model.recordWords[recordIndex * 4];
  const nextFlags =
    ((prevFlags & ~(FLAG_VISIBLE | FLAG_GHOST)) | displayFlags(model, expressId, prevFlags)) >>> 0;
  if (nextFlags === prevFlags) return false;
  model.recordWords[wordIdx] = nextFlags;
  return true;
}
