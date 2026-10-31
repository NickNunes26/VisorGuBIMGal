// Metres in the drawing frame, IFC Z-up. `depth` limits projection beyond the cut; omit for no limit.
export type DrawingCut =
  | { kind: 'plan'; elevation: number; depth?: number }
  | {
      kind: 'section';
      axis: 'x' | 'y';
      position: number;
      look: 'positive' | 'negative';
      depth?: number;
    };

// 'project': absolute model coordinates, taken as shared by every model, as the 3D view does.
// 'map': each model through its localToMap. Models without one are skipped with a diagnostic.
export type DrawingFrame = { kind: 'project' } | { kind: 'map' };

export interface DrawingOptions {
  cut: DrawingCut;
  frame: DrawingFrame;
  includeProjection: boolean;
  includeHiddenLines: boolean;    // Requires includeProjection
  includeHatching: boolean;
  includeEdges: boolean;
  mergeLines: boolean;
}

export interface DrawingExportConfig {
  format: 'dxf' | 'svg';
  fileName: string;
  crsComment?: string;
  units: 'm' | 'mm';
  showHiddenLines: boolean;       // Export can omit what was generated, never add to it
  showHatching: boolean;
}
