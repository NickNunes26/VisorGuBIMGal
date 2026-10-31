import {
  generateFloorPlan,
  generateSection,
  exportToDXF,
  exportToSVG,
  type Drawing2D,
  type DXFExportOptions,
  type SVGExportOptions
} from '@ifc-lite/drawing-2d';
import type { MeshData } from '@ifc-lite/geometry';
import type { DrawingOptions, DrawingExportConfig } from '../../domain/drawings/DrawingTypes';
import {
  KernelError,
  type DrawingId,
  type ExportedDrawing,
  type GeneratedDrawing,
  type ProgressHandler
} from '../../domain/kernel/KernelPort';

export class IfcDrawingService {
  private drawings: Map<DrawingId, Drawing2D> = new Map();
  private nextDrawingSeq = 0;

  async generateDrawing(
    meshes: MeshData[],
    options: DrawingOptions,
    crsId: string | undefined,
    onProgress?: ProgressHandler
  ): Promise<GeneratedDrawing> {
    if (meshes.length === 0) {
      throw new KernelError('internal', 'No hay mallas cargadas para generar el dibujo 2D.');
    }

    onProgress?.('Calculando plano de corte 2D', 20);

    let drawing: Drawing2D;

    if (options.cut.kind === 'plan') {
      drawing = await generateFloorPlan(meshes, options.cut.elevation, {
        useGPU: false,
        includeHiddenLines: options.includeHiddenLines,
        includeProjection: options.includeProjection,
        includeEdges: options.includeEdges,
        mergeLines: options.mergeLines,
        onProgress: (stage, progress) => onProgress?.(stage, Math.round(progress * 100))
      });
    } else {
      // Contract uses Z-up where 'y' is the transverse horizontal axis; IFClite uses Y-up ('z').
      const axis = options.cut.axis === 'x' ? 'x' : 'z';
      drawing = await generateSection(meshes, axis, options.cut.position, {
        useGPU: false,
        includeHiddenLines: options.includeHiddenLines,
        includeProjection: options.includeProjection,
        includeEdges: options.includeEdges,
        mergeLines: options.mergeLines,
        onProgress: (stage, progress) => onProgress?.(stage, Math.round(progress * 100))
      });
    }

    const drawingId: DrawingId = `drawing-${++this.nextDrawingSeq}`;
    this.drawings.set(drawingId, drawing);
    onProgress?.('Plano 2D generado', 100);

    return {
      drawingId,
      frame: options.frame,
      crsId,
      diagnostics: []
    };
  }

  async exportDrawing(
    drawingId: DrawingId,
    config: DrawingExportConfig
  ): Promise<ExportedDrawing> {
    const drawing = this.drawings.get(drawingId);
    if (!drawing) {
      throw new KernelError(
        'drawing-not-found',
        'No hay ningún dibujo 2D generado para exportar.'
      );
    }

    if (config.format === 'dxf') {
      const dxfOptions: DXFExportOptions = {
        showHiddenLines: config.showHiddenLines,
        showHatching: config.showHatching,
        metadataComment: config.crsComment
          ? `CRS: ${config.crsComment} | VisorGuBIMGal IFClite export`
          : 'VisorGuBIMGal IFClite export'
      };

      const dxfContent = exportToDXF(drawing, dxfOptions);
      return {
        content: new TextEncoder().encode(dxfContent),
        mimeType: 'application/dxf',
        filename: `${config.fileName}.dxf`
      };
    } else {
      const svgOptions: SVGExportOptions = {
        showHiddenLines: config.showHiddenLines,
        showHatching: config.showHatching,
        backgroundColor: '#090d16'
      };

      const svgContent = exportToSVG(drawing, svgOptions);
      return {
        content: svgContent,
        mimeType: 'image/svg+xml',
        filename: `${config.fileName}.svg`
      };
    }
  }

  async discardDrawing(drawingId: DrawingId): Promise<void> {
    this.drawings.delete(drawingId);
  }
}

export const ifcDrawingService = new IfcDrawingService();
