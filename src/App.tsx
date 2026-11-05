import React, { useState, useCallback, useEffect, useMemo } from 'react';
import { Header } from './ui/components/Header';
import { ModelsPanel } from './ui/components/ModelsPanel';
import { SpatialTreePanel } from './ui/components/SpatialTreePanel';
import { PropertiesPanel } from './ui/components/PropertiesPanel';
import { SectionDxfPanel } from './ui/components/SectionDxfPanel';
import { GisMapPanel } from './ui/components/GisMapPanel';
import { DisplayBar } from './ui/components/DisplayBar';
import { SectionBar } from './ui/components/SectionBar';
import { SearchPanel } from './ui/components/SearchPanel';
import { NO_SECTION, sceneBounds, sectionPlanes, type SectionState } from './ui/viewer/section';
import { BimViewer } from './ui/viewer/BimViewer';
import {
  SHOW_ALL,
  classCounts,
  modelDisplay,
  sameTarget,
  withoutModel,
  type DisplayRule,
  type DisplayTarget
} from './ui/viewer/displayRule';

import type {
  ElementRef,
  ModelBounds,
  ModelId,
  ModelRecord,
  SelectedElementDetails
} from './domain/models/ModelRecord';
import type { KernelDiagnostic, SpatialTree } from './domain/kernel/KernelPort';
import { kernel, renderer } from './infrastructure/composition';
import { leafletService } from './infrastructure/gis/LeafletService';

import { Layers, FolderTree, Sliders, Scissors, Search } from 'lucide-react';

export default function App() {
  const [models, setModels] = useState<ModelRecord[]>([]);
  const [spatialTrees, setSpatialTrees] = useState<Record<ModelId, SpatialTree>>({});
  const [selectedRef, setSelectedRef] = useState<ElementRef | null>(null);
  const [, setHoveredRef] = useState<ElementRef | null>(null);
  const [selectedElement, setSelectedElement] = useState<SelectedElementDetails | null>(null);
  const [statusText, setStatusText] = useState<string>('Listo. Abre un archivo IFC para comenzar.');
  const [isWorking, setIsWorking] = useState<boolean>(false);
  const [, setRendererError] = useState<string | null>(null);
  const [activeBackend, setActiveBackend] = useState<'webgpu' | 'webgl2' | null>(null);
  const [displayRule, setDisplayRule] = useState<DisplayRule>(SHOW_ALL);

  // Every model follows the display rule, including one loaded after the rule was set
  useEffect(() => {
    for (const model of models) {
      const { others, entries } = modelDisplay(displayRule, model.id, spatialTrees[model.id]);
      renderer.setElementDisplay(model.id, others, entries);
    }
  }, [displayRule, models, spatialTrees]);

  const ifcClasses = useMemo(() => classCounts(Object.values(spatialTrees)), [spatialTrees]);

  const [section, setSection] = useState<SectionState>(NO_SECTION);
  const bounds = useMemo(() => sceneBounds(models), [models]);
  useEffect(() => {
    renderer.setSectionPlanes(sectionPlanes(section, bounds));
  }, [section, bounds]);

  const handleIsolate = useCallback((target: DisplayTarget) => {
    setDisplayRule((prev) => ({ ...prev, isolate: target }));
  }, []);

  const handleHide = useCallback((target: DisplayTarget) => {
    setDisplayRule((prev) =>
      prev.hidden.some((hidden) => sameTarget(hidden, target))
        ? prev
        : { ...prev, hidden: [...prev.hidden, target] }
    );
  }, []);

  // Subscribe to renderer backend changes
  useEffect(() => {
    const stop = renderer.onBackendChange(setActiveBackend);
    return () => void stop();
  }, []);

  // Tab state
  const [leftTab, setLeftTab] = useState<'models' | 'tree' | 'search'>('models');
  const [rightTab, setRightTab] = useState<'props' | 'dxf'>('props');
  const [isGisExpanded, setIsGisExpanded] = useState<boolean>(false);

  // Load IFC file handler (local File or static fetch)
  const loadIfcFile = useCallback(
    async (fileInput: File | { name: string; size: number; arrayBuffer: () => Promise<ArrayBuffer> }) => {
      const modelId: ModelId = `model-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
      let opened = false;
      setIsWorking(true);
      setStatusText(`Cargando ${fileInput.name}...`);

      try {
        const buffer = await fileInput.arrayBuffer();

        // 1. Open model in kernel
        const summary = await kernel.openModel(modelId, buffer, (phase, pct) => {
          setStatusText(`${phase} (${pct}%)`);
        });
        opened = true;

        // 2. Stream geometry from kernel to renderer
        setStatusText(`Procesando geometría 3D (${activeBackend === 'webgl2' ? 'WebGL2' : 'WebGPU'})...`);
        renderer.addModel(modelId);

        let meshCount = 0;
        let bounds: ModelBounds | undefined;
        let streamDiagnostics: KernelDiagnostic[] = [];
        let completed = false;

        for await (const event of kernel.streamGeometry(modelId)) {
          if (event.type === 'batch') {
            renderer.addBatch(event.batch);
          } else if (event.type === 'progress') {
            setStatusText(`Cargadas ${event.processedElements} mallas 3D...`);
          } else if (event.type === 'complete') {
            completed = true;
            meshCount = event.meshCount;
            bounds = event.bounds;
            streamDiagnostics = event.diagnostics;
          }
        }

        if (!completed) {
          renderer.removeModel(modelId);
          return;
        }

        renderer.finishModel(modelId, bounds);

        // 3. Load spatial tree for the model
        const tree = await kernel.getSpatialTree(modelId);
        if (tree) {
          setSpatialTrees((prev) => ({ ...prev, [modelId]: tree }));
        }

        const units: 'm' | 'mm' = summary.lengthUnitScale < 0.01 ? 'mm' : 'm';
        const newModel: ModelRecord = {
          id: modelId,
          fileName: fileInput.name,
          fileSize: fileInput.size,
          schema: summary.schemaIdentifier || summary.schema,
          status: 'ready',
          visible: true,
          units,
          elementCount: summary.elementCount,
          meshCount,
          coordinateReference: summary.coordinateReference,
          bounds,
          diagnostics: [...summary.diagnostics, ...streamDiagnostics]
        };

        setModels((prev) => [...prev, newModel]);
        setStatusText(`${fileInput.name} cargado con ${meshCount} mallas.`);
      } catch (err) {
        if (opened) {
          renderer.removeModel(modelId);
          try {
            await kernel.closeModel(modelId);
          } catch {
            // ignore close error
          }
        }
        console.error('Error loading IFC:', err);
        setStatusText(`Error al cargar ${fileInput.name}: ${err instanceof Error ? err.message : 'Error'}`);
      } finally {
        setIsWorking(false);
      }
    },
    [activeBackend]
  );

  // Sample model loader
  const handleLoadSample = useCallback(
    async (sampleUrl: string, sampleName: string) => {
      try {
        setIsWorking(true);
        setStatusText(`Descargando muestra ${sampleName}...`);
        const resp = await fetch(sampleUrl);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const blob = await resp.blob();
        const file = new File([blob], sampleName.split(' ')[0] + '.ifc', { type: 'application/octet-stream' });
        await loadIfcFile(file);
      } catch (err) {
        console.error(err);
        setStatusText(`Error al descargar muestra: ${err instanceof Error ? err.message : 'Error'}`);
        setIsWorking(false);
      }
    },
    [loadIfcFile]
  );

  // Selection handler keyed by ElementRef
  const handleSelectElement = useCallback(
    async (ref: ElementRef | null) => {
      setSelectedRef(ref);
      renderer.setSelection(ref);

      if (!ref) {
        setSelectedElement(null);
        return;
      }

      const model = models.find((m) => m.id === ref.modelId) || models[0];
      if (!model) return;

      const resolvedRef: ElementRef = { modelId: model.id, expressId: ref.expressId, globalId: ref.globalId };
      const details = await kernel.getElementDetails(resolvedRef);

      setSelectedElement(details ? { ...details, modelName: model.fileName } : null);
      setRightTab('props');
    },
    [models]
  );

  // Model visibility toggle
  const handleToggleVisibility = useCallback((id: ModelId) => {
    setModels((prev) =>
      prev.map((m) => {
        if (m.id === id) {
          const next = !m.visible;
          renderer.setModelVisibility(id, next);
          return { ...m, visible: next };
        }
        return m;
      })
    );
  }, []);

  // Remove model
  const handleRemoveModel = useCallback((id: ModelId) => {
    renderer.removeModel(id);
    void kernel.closeModel(id);
    setModels((prev) => prev.filter((m) => m.id !== id));
    setSpatialTrees((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setSelectedRef((prev) => {
      if (prev?.modelId === id) {
        renderer.setSelection(null);
        return null;
      }
      return prev;
    });
    setSelectedElement((prev) => (prev?.modelId === id ? null : prev));
    setDisplayRule((prev) => withoutModel(prev, id));
  }, []);

  // Locate model on GIS map
  const handleLocateOnGis = useCallback((model: ModelRecord) => {
    setIsGisExpanded(true);
    if (model.coordinateReference?.latLng) {
      const [lat, lng] = model.coordinateReference.latLng;
      setTimeout(() => {
        leafletService.setCenter(lat, lng, 16);
      }, 100);
    }
  }, []);

  return (
    <div className="app-container">
      <Header
        onOpenFile={(file) => void loadIfcFile(file)}
        onLoadSample={handleLoadSample}
        onSetPreset={(p) => renderer.setViewPreset(p)}
        onFitView={() => renderer.fitToView()}
        isGisExpanded={isGisExpanded}
        onToggleGis={() => setIsGisExpanded((prev) => !prev)}
        isWorking={isWorking}
        activeBackend={activeBackend}
        isWebGpuAvailable={activeBackend === 'webgpu'}
      />

      <div className="workspace">
        {/* Left Sidebar */}
        <aside className="panel sidebar-left">
          <div className="panel-tabs">
            <button
              className={`tab-btn ${leftTab === 'models' ? 'active' : ''}`}
              onClick={() => setLeftTab('models')}
            >
              <Layers size={13} />
              <span>Modelos</span>
            </button>
            <button
              className={`tab-btn ${leftTab === 'tree' ? 'active' : ''}`}
              onClick={() => setLeftTab('tree')}
            >
              <FolderTree size={13} />
              <span>Árbol</span>
            </button>
            <button
              className={`tab-btn tab-search ${leftTab === 'search' ? 'active' : ''}`}
              onClick={() => setLeftTab('search')}
            >
              <Search size={13} />
              <span>Buscar</span>
            </button>
          </div>

          <div className="panel-body">
            {leftTab === 'models' && (
              <ModelsPanel
                models={models}
                onToggleVisibility={handleToggleVisibility}
                onRemoveModel={handleRemoveModel}
                onLocateOnGis={handleLocateOnGis}
                onSelectElement={(ref) => void handleSelectElement(ref)}
              />
            )}
            {leftTab === 'tree' && (
              <SpatialTreePanel
                models={models}
                spatialTrees={spatialTrees}
                onSelectElement={(ref) => void handleSelectElement(ref)}
                selectedRef={selectedRef}
                onIsolate={handleIsolate}
                onHide={handleHide}
              />
            )}
            {/* Kept mounted, so the search and its results survive a change of tab */}
            <div style={{ display: leftTab === 'search' ? 'block' : 'none' }}>
              <SearchPanel
                models={models}
                spatialTrees={spatialTrees}
                kernel={kernel}
                selectedRef={selectedRef}
                onSelectElement={(ref) => void handleSelectElement(ref)}
                onIsolate={handleIsolate}
                onHide={handleHide}
              />
            </div>
          </div>
        </aside>

        {/* Center Viewport */}
        <main className="viewport-center">
          <BimViewer
            renderer={renderer}
            onOpenFile={(file) => void loadIfcFile(file)}
            onSelectElement={(ref) => void handleSelectElement(ref)}
            onHoverElement={setHoveredRef}
            onInitError={setRendererError}
            statusText={statusText}
            isWorking={isWorking}
          />

          {models.length > 0 && (
            <div className="viewport-tools">
              <DisplayBar
                classes={ifcClasses}
                rule={displayRule}
                selectedRef={selectedRef}
                onIsolate={handleIsolate}
                onHide={handleHide}
                onToggleGhost={() => setDisplayRule((prev) => ({ ...prev, ghost: !prev.ghost }))}
                onShowAll={() => setDisplayRule((prev) => ({ ...SHOW_ALL, ghost: prev.ghost }))}
              />
              <SectionBar section={section} bounds={bounds} onChange={setSection} />
            </div>
          )}

          {/* GIS Split Panel (Bottom) */}
          {isGisExpanded && (
            <GisMapPanel
              models={models}
              onClose={() => setIsGisExpanded(false)}
            />
          )}
        </main>

        {/* Right Sidebar */}
        <aside className="panel sidebar-right">
          <div className="panel-tabs">
            <button
              className={`tab-btn ${rightTab === 'props' ? 'active' : ''}`}
              onClick={() => setRightTab('props')}
            >
              <Sliders size={13} />
              <span>Propiedades</span>
            </button>
            <button
              className={`tab-btn ${rightTab === 'dxf' ? 'active' : ''}`}
              onClick={() => setRightTab('dxf')}
            >
              <Scissors size={13} />
              <span>Planos y DXF</span>
            </button>
          </div>

          <div className="panel-body">
            {rightTab === 'props' ? (
              <PropertiesPanel element={selectedElement} />
            ) : (
              <SectionDxfPanel models={models} kernel={kernel} />
            )}
          </div>
        </aside>
      </div>
    </div>
  );
}
