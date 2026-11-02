import React, { useEffect, useRef, useState } from 'react';
import { Scissors, Download, FileCode, CheckCircle, RefreshCw } from 'lucide-react';
import type { DrawingCut, DrawingOptions } from '../../domain/drawings/DrawingTypes';
import type {
  ExportedDrawing,
  GeneratedDrawing,
  KernelPort
} from '../../domain/kernel/KernelPort';
import type { ModelId, ModelRecord } from '../../domain/models/ModelRecord';

interface SectionDxfPanelProps {
  models: ModelRecord[];
  kernel: KernelPort;
}

type CutPreset = 'plan' | 'section-x' | 'section-y';

function downloadExportedDrawing(res: ExportedDrawing): void {
  const part: BlobPart =
    typeof res.content === 'string' ? res.content : new Uint8Array(res.content);
  const blob = new Blob([part], { type: res.mimeType });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = res.filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

export const SectionDxfPanel: React.FC<SectionDxfPanelProps> = ({ models, kernel }) => {
  const [cutPreset, setCutPreset] = useState<CutPreset>('plan');
  const [position, setPosition] = useState<number>(1.2); // 1.2m typical floor cut
  const [depthStr, setDepthStr] = useState<string>(''); // Optional projection depth
  const [includeHidden, setIncludeHidden] = useState<boolean>(true);
  const [includeHatching, setIncludeHatching] = useState<boolean>(true);
  const [isGenerating, setIsGenerating] = useState<boolean>(false);
  const [statusMessage, setStatusMessage] = useState<string>('');
  const [previewSvg, setPreviewSvg] = useState<string | null>(null);
  const [currentDrawing, setCurrentDrawing] = useState<GeneratedDrawing | null>(null);
  const [drawingModelIds, setDrawingModelIds] = useState<ModelId[]>([]);
  const [drawingFirstModelFileName, setDrawingFirstModelFileName] = useState<string | null>(null);

  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Discard drawing on unmount or when drawing changes
  useEffect(() => {
    return () => {
      if (currentDrawing) {
        void kernel.discardDrawing(currentDrawing.drawingId);
      }
    };
  }, [currentDrawing, kernel]);

  // When a model that a shown drawing came from is closed, clear preview, status, and discard drawing
  useEffect(() => {
    if (!currentDrawing || drawingModelIds.length === 0) return;
    const currentModelIds = new Set(models.map((m) => m.id));
    const anyClosed = drawingModelIds.some((id) => !currentModelIds.has(id));
    if (anyClosed) {
      void kernel.discardDrawing(currentDrawing.drawingId);
      setCurrentDrawing(null);
      setPreviewSvg(null);
      setStatusMessage('');
      setDrawingModelIds([]);
      setDrawingFirstModelFileName(null);
    }
  }, [models, currentDrawing, drawingModelIds, kernel]);

  const handleGenerate = async () => {
    const visibleIds = models.filter((m) => m.visible).map((m) => m.id);
    const modelIds = visibleIds.length > 0 ? visibleIds : models.map((m) => m.id);
    if (modelIds.length === 0) {
      alert('No hay mallas IFC cargadas para generar el plano 2D.');
      return;
    }

    setIsGenerating(true);
    setStatusMessage('Calculando corte 2D...');

    let generated: GeneratedDrawing | null = null;
    try {
      const parsedDepth = depthStr.trim() !== '' ? parseFloat(depthStr) : undefined;
      const depth =
        parsedDepth !== undefined && !Number.isNaN(parsedDepth) ? parsedDepth : undefined;

      const cut: DrawingCut =
        cutPreset === 'plan'
          ? { kind: 'plan', elevation: position, ...(depth !== undefined ? { depth } : {}) }
          : {
              kind: 'section',
              axis: cutPreset === 'section-x' ? 'x' : 'y',
              position,
              look: 'positive',
              ...(depth !== undefined ? { depth } : {})
            };

      const options: DrawingOptions = {
        cut,
        frame: { kind: 'project' },
        includeProjection: true,
        includeHiddenLines: includeHidden,
        includeHatching
      };

      generated = await kernel.generateDrawing(modelIds, options, (stage, pct) => {
        if (mountedRef.current) {
          setStatusMessage(`${stage} (${pct}%)`);
        }
      });

      if (!mountedRef.current) {
        void kernel.discardDrawing(generated.drawingId);
        return;
      }

      const svgRes = await kernel.exportDrawing(generated.drawingId, {
        format: 'svg',
        fileName: 'preview',
        units: 'm',
        showHiddenLines: includeHidden,
        showHatching: includeHatching
      });

      if (!mountedRef.current) {
        void kernel.discardDrawing(generated.drawingId);
        return;
      }

      const svgText =
        typeof svgRes.content === 'string'
          ? svgRes.content
          : new TextDecoder().decode(svgRes.content);

      if (currentDrawing) {
        void kernel.discardDrawing(currentDrawing.drawingId);
      }

      const firstModel = models.find((m) => m.id === modelIds[0]);
      setDrawingFirstModelFileName(firstModel ? firstModel.fileName : null);
      setDrawingModelIds(modelIds);
      setCurrentDrawing(generated);
      setPreviewSvg(svgText);
      setStatusMessage('Plano 2D generado correctamente.');
    } catch (err) {
      if (generated && !mountedRef.current) {
        void kernel.discardDrawing(generated.drawingId);
      }
      if (mountedRef.current) {
        console.error(err);
        setStatusMessage(`Error: ${err instanceof Error ? err.message : 'Error desconocido'}`);
      }
    } finally {
      if (mountedRef.current) {
        setIsGenerating(false);
      }
    }
  };

  const handleExport = async (format: 'dxf' | 'svg') => {
    if (!currentDrawing) {
      alert('No hay ningún dibujo 2D generado para exportar.');
      return;
    }
    try {
      const baseName = drawingFirstModelFileName
        ? drawingFirstModelFileName.replace(/\.ifc$/i, '')
        : 'plano-ifc';
      const fileName = `${baseName}-${cutPreset}`;

      const res = await kernel.exportDrawing(currentDrawing.drawingId, {
        format,
        fileName,
        crsComment: currentDrawing.crsId,
        units: 'm',
        showHiddenLines: includeHidden,
        showHatching: includeHatching
      });

      downloadExportedDrawing(res);
    } catch (err) {
      alert(err instanceof Error ? err.message : 'Error al exportar');
    }
  };

  return (
    <div style={{ fontSize: '13px' }}>
      <div style={{ marginBottom: '12px' }}>
        <label style={{ display: 'block', fontSize: '11px', color: '#94a3b8', marginBottom: '4px', fontWeight: 600 }}>
          TIPO DE DIBUJO 2D
        </label>
        <select
          className="select"
          style={{ width: '100%' }}
          value={cutPreset}
          onChange={(e) => setCutPreset(e.target.value as CutPreset)}
        >
          <option value="plan">Planta arquitectónica a cota (Z horizontal)</option>
          <option value="section-x">Sección vertical (Eje X)</option>
          <option value="section-y">Sección transversal (Eje Y)</option>
        </select>
      </div>

      <div style={{ marginBottom: '12px' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: '4px' }}>
          <label style={{ fontSize: '11px', color: '#94a3b8', fontWeight: 600 }}>
            POSICIÓN DE CORTE (metros)
          </label>
          <span style={{ fontFamily: 'var(--font-mono)', color: '#38bdf8' }}>{position.toFixed(2)} m</span>
        </div>
        <input
          type="number"
          step="0.1"
          className="select"
          style={{ width: '100%' }}
          value={position}
          onChange={(e) => setPosition(parseFloat(e.target.value) || 0)}
        />
      </div>

      <div style={{ marginBottom: '12px' }}>
        <label style={{ display: 'block', fontSize: '11px', color: '#94a3b8', marginBottom: '4px', fontWeight: 600 }}>
          Profundidad de proyección (m)
        </label>
        <input
          type="number"
          step="0.1"
          min="0"
          placeholder="Opcional (sin límite)"
          className="select"
          style={{ width: '100%' }}
          value={depthStr}
          onChange={(e) => setDepthStr(e.target.value)}
        />
      </div>

      <div style={{ marginBottom: '14px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
        <label style={{ display: 'flex', alignItems: 'center', gap: '8px', cursor: 'pointer', color: '#cbd5e1' }}>
          <input
            type="checkbox"
            checked={includeHidden}
            onChange={(e) => setIncludeHidden(e.target.checked)}
          />
          <span>Incluir líneas ocultas (trazos discontinuos)</span>
        </label>
        <label style={{ display: 'flex', alignItems: 'center', gap: '8px', cursor: 'pointer', color: '#cbd5e1' }}>
          <input
            type="checkbox"
            checked={includeHatching}
            onChange={(e) => setIncludeHatching(e.target.checked)}
          />
          <span>Generar sombreados / tramas de materiales</span>
        </label>
      </div>

      <button
        className="btn btn-primary"
        style={{ width: '100%', justifyContent: 'center', padding: '8px' }}
        disabled={isGenerating || models.length === 0}
        onClick={() => void handleGenerate()}
      >
        {isGenerating ? <RefreshCw size={15} className="spin" /> : <Scissors size={15} />}
        <span>{isGenerating ? 'Generando plano...' : 'Generar plano 2D'}</span>
      </button>

      {statusMessage && (
        <div
          style={{
            marginTop: '10px',
            padding: '8px',
            background: 'rgba(15, 23, 42, 0.5)',
            borderRadius: '4px',
            fontSize: '11px',
            color: '#94a3b8',
            display: 'flex',
            alignItems: 'center',
            gap: '6px'
          }}
        >
          <CheckCircle size={13} color="#38bdf8" />
          <span>{statusMessage}</span>
        </div>
      )}

      {/* SVG Preview container */}
      {previewSvg && (
        <div style={{ marginTop: '14px' }}>
          <div style={{ fontSize: '11px', color: '#94a3b8', marginBottom: '6px', fontWeight: 600 }}>
            VISTA PREVIA 2D
          </div>
          <div
            className="drawing-preview"
            style={{
              width: '100%',
              height: '180px',
              backgroundColor: '#ffffff',
              border: '1px solid #334155',
              borderRadius: '6px',
              overflow: 'hidden',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              padding: '6px'
            }}
            dangerouslySetInnerHTML={{ __html: previewSvg }}
          />

          <div style={{ display: 'flex', gap: '8px', marginTop: '10px' }}>
            <button
              className="btn btn-secondary"
              style={{ flex: 1, justifyContent: 'center' }}
              onClick={() => void handleExport('dxf')}
            >
              <Download size={14} />
              <span>Exportar DXF (R12)</span>
            </button>
            <button
              className="btn btn-secondary"
              style={{ flex: 1, justifyContent: 'center' }}
              onClick={() => void handleExport('svg')}
            >
              <FileCode size={14} />
              <span>Exportar SVG</span>
            </button>
          </div>
        </div>
      )}
    </div>
  );
};
