// Metres in absolute model coordinates, IFC Z-up.
export type DrawingCut =
  | { kind: 'plan'; elevation: number }
  | { kind: 'section'; axis: 'x' | 'y'; position: number };

export interface DrawingOptions {
  cut: DrawingCut;
  includeHiddenLines: boolean;
  includeProjection: boolean;
  includeEdges: boolean;
  mergeLines: boolean;
}

// Legacy IFClite adapter types. T-011 moves the UI to DrawingOptions and deletes these.
export type DrawingCutType = 'floorplan' | 'section-x' | 'section-z';

export interface SectionCutOptions {
  cutType: DrawingCutType;
  position: number;       // Elevation for floor plan, X or Z position for section
  includeHiddenLines: boolean;
  includeProjection: boolean;
  includeEdges: boolean;
  mergeLines: boolean;
}

export interface DrawingExportConfig {
  format: 'dxf' | 'svg';
  fileName: string;
  crsComment?: string;
  units: 'm' | 'mm';
  showHiddenLines: boolean;
  showHatching: boolean;
}
