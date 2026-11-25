import React, { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, BookmarkPlus, Download, Eye, FileUp, Play, RefreshCw, X } from 'lucide-react';
import {
  bindAlignmentToModel,
  parseLandXml,
  type HorizontalAlignment
} from '../../domain/alignment/landXml';
import type {
  ClashDxfConflictPolicy,
  ClashDxfRoundTripResult,
  ClashReport,
  ClashRule
} from '../../domain/clash/ClashTypes';
import type { FilePort, SaveFileOptions } from '../../domain/files/FilePort';
import type { KernelPort, SpatialTree } from '../../domain/kernel/KernelPort';
import type { ModelId, ModelRecord } from '../../domain/models/ModelRecord';
import {
  exportClashAlignmentZip,
  previewSingleClashSectionSvg
} from '../viewer/clashAlignmentExport';
import {
  buildClashDisplayEntries,
  buildClashRequest,
  collectClashCatalog,
  createPresetRule,
  PIPE_CLASH_CLASSES,
  STRUCTURE_CLASH_CLASSES,
  summarizeClashes,
  type ClashDisplayEntry,
  type ClashPresetId
} from '../viewer/clashDetector';
import { formatNumber, t, type I18nKey } from '../i18n';
import { skippedReasons } from '../viewer/clashRoundTrip';

interface ClashPanelProps {
  models: ModelRecord[];
  spatialTrees: Record<ModelId, SpatialTree>;
  kernel: Pick<
    KernelPort,
    | 'detectClashes'
    | 'elementMesh'
    | 'getElementDetails'
    | 'exportClashAlignmentDxf'
    | 'applyClashDxfRoundTrip'
    | 'elementSlabSpan'
  >;
  filePort?: Pick<FilePort, 'saveFile'>;
  onInspectPair: (entry: ClashDisplayEntry) => void;
  onClearClashView: () => void;
  onSaveAsBcf: (entry: ClashDisplayEntry) => Promise<void>;
  onReloadPatchedModel?: (
    modelId: ModelId,
    patchedBytes: Uint8Array
  ) => Promise<{ model: ModelRecord; tree: SpatialTree | null } | null>;
}

async function saveOutputWithFallback(
  filePort: Pick<FilePort, 'saveFile'> | undefined,
  options: SaveFileOptions
): Promise<void> {
  if (filePort) {
    await filePort.saveFile(options);
    return;
  }
  const blobPart =
    typeof options.content === 'string'
      ? options.content
      : (options.content as Uint8Array<ArrayBuffer>);
  const blob = new Blob([blobPart], { type: options.mimeType });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = options.defaultName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

const MAX_DISPLAY_ROWS = 200;

const SKIP_REASONS = new Set([
  'guid-mismatch',
  'guid-missing',
  'not-found',
  'not-a-product',
  'unsupported-placement',
  'no-representation-context',
  'invalid-points',
  'invalid-radius',
  'duplicate-edit',
  'already-applied',
  'metadata-incomplete',
  'invalid-number',
  'unsupported-transform',
  'duplicate-insert',
  'duplicate-polyline',
  'ambiguous-element',
  'out-of-range',
  'other-model',
  'edit-conflict'
]);

/** Elements another section edited after the imported DXF was exported (F7). */
interface PendingRoundTripConflict {
  dxfBytes: Uint8Array;
  elements: { key: string; label: string }[];
}

/** «2 GlobalId distinto, 1 ya aplicado» from the skipped edits of a round trip. */
function describeSkippedReasons(edits: Parameters<typeof skippedReasons>[0]): string {
  return Object.entries(skippedReasons(edits))
    .map(([reason, count]) => {
      const code = SKIP_REASONS.has(reason) ? reason : 'unknown';
      return `${count} ${t(`clash.roundTripReasons.${code}` as I18nKey)}`;
    })
    .join(', ');
}
const PRESET_PIPES_SENTINEL = '__preset_pipes__';
const PRESET_STRUCTURE_SENTINEL = '__preset_structure__';

export const ClashPanel: React.FC<ClashPanelProps> = ({
  models,
  spatialTrees,
  kernel,
  filePort,
  onInspectPair,
  onClearClashView,
  onSaveAsBcf,
  onReloadPatchedModel
}) => {
  const readyModels = useMemo(() => models.filter((m) => m.status === 'ready'), [models]);
  const catalog = useMemo(
    () => collectClashCatalog(readyModels, spatialTrees),
    [readyModels, spatialTrees]
  );

  const [preset, setPreset] = useState<ClashPresetId>('all-vs-all');
  const [modelA, setModelA] = useState<ModelId | '*'>('*');
  const [modelB, setModelB] = useState<ModelId | '*'>('*');
  const [classA, setClassA] = useState<string>('');
  const [classB, setClassB] = useState<string>('');
  const [storeyA, setStoreyA] = useState<string>('');
  const [storeyB, setStoreyB] = useState<string>('');
  const [clearanceText, setClearanceText] = useState<string>('0');

  const [report, setReport] = useState<ClashReport | null>(null);
  const [entries, setEntries] = useState<ClashDisplayEntry[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [kindFilter, setKindFilter] = useState<'all' | 'hard' | 'soft'>('all');
  const [busy, setBusy] = useState(false);
  const [savingBcfId, setSavingBcfId] = useState<string | null>(null);
  const [progressText, setProgressText] = useState<string>('');
  const [error, setError] = useState<string | null>(null);
  const latestRun = useRef(0);
  const initializedModelsCountRef = useRef(0);

  // F4 & F5: LandXML alignment, batch DXF export & DXF -> STEP round-trip state
  const landXmlInputRef = useRef<HTMLInputElement | null>(null);
  const roundTripDxfInputRef = useRef<HTMLInputElement | null>(null);
  const exportAbortRef = useRef<AbortController | null>(null);
  const [landXmlFileName, setLandXmlFileName] = useState<string | null>(null);
  const [alignments, setAlignments] = useState<HorizontalAlignment[]>([]);
  const [selectedAlignmentIdx, setSelectedAlignmentIdx] = useState<number>(0);
  const [mergeToleranceText, setMergeToleranceText] = useState<string>('20');
  const [exportBusy, setExportBusy] = useState(false);
  const [roundTripBusy, setRoundTripBusy] = useState(false);
  const [exportProgressText, setExportProgressText] = useState<string>('');
  const [exportStatus, setExportStatus] = useState<string | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  const [roundTripResult, setRoundTripResult] = useState<ClashDxfRoundTripResult | null>(null);
  const [pendingConflict, setPendingConflict] = useState<PendingRoundTripConflict | null>(null);
  const [previewBusyId, setPreviewBusyId] = useState<string | null>(null);
  const [svgPreview, setSvgPreview] = useState<{
    svg: string;
    pkLabel: string;
    depthMm: number;
    orientation: 'transverse' | 'longitudinal';
  } | null>(null);

  const selectedAlignment = alignments[selectedAlignmentIdx] ?? null;
  const primaryModel = useMemo(
    () => readyModels.find((m) => m.id === modelA) ?? readyModels[0] ?? null,
    [readyModels, modelA]
  );
  const boundAlignment = useMemo(() => {
    if (!selectedAlignment) return null;
    return bindAlignmentToModel(
      selectedAlignment,
      primaryModel?.coordinateReference,
      primaryModel?.bounds
    );
  }, [selectedAlignment, primaryModel]);

  // Sync sensible default pair when models are loaded or removed.
  useEffect(() => {
    const validIds = new Set(readyModels.map((m) => m.id));
    if (readyModels.length === 0) {
      setReport(null);
      setEntries([]);
      setActiveId(null);
      initializedModelsCountRef.current = 0;
      return;
    }
    if (initializedModelsCountRef.current !== readyModels.length && preset === 'all-vs-all') {
      initializedModelsCountRef.current = readyModels.length;
      setModelA(readyModels[0].id);
      setModelB(readyModels[1]?.id ?? readyModels[0].id);
      return;
    }
    if (modelA !== '*' && !validIds.has(modelA)) setModelA(readyModels[0].id);
    if (modelB !== '*' && !validIds.has(modelB)) setModelB(readyModels[1]?.id ?? readyModels[0].id);
  }, [readyModels, modelA, modelB, preset]);

  const applyPreset = (nextPreset: ClashPresetId) => {
    setPreset(nextPreset);
    if (nextPreset === 'custom') return;
    const rule = createPresetRule(nextPreset, readyModels, catalog.classes);
    setModelA(rule.setA.modelId);
    setModelB(rule.setB.modelId);
    setStoreyA('');
    setStoreyB('');
    if (nextPreset === 'pipes-vs-structure') {
      setClassA(PRESET_PIPES_SENTINEL);
      setClassB(PRESET_STRUCTURE_SENTINEL);
    } else {
      setClassA('');
      setClassB('');
    }
  };

  const resolveClassList = (val: string): string[] => {
    if (!val) return [];
    if (val === PRESET_PIPES_SENTINEL) {
      const present = PIPE_CLASH_CLASSES.filter((c) => catalog.classes.includes(c));
      return present.length > 0 ? present : [...PIPE_CLASH_CLASSES];
    }
    if (val === PRESET_STRUCTURE_SENTINEL) {
      const present = STRUCTURE_CLASH_CLASSES.filter((c) => catalog.classes.includes(c));
      return present.length > 0 ? present : [...STRUCTURE_CLASH_CLASSES];
    }
    return [val];
  };

  const executeClashDetection = async (
    activeReadyModels: ModelRecord[],
    activeSpatialTrees: Record<ModelId, SpatialTree>
  ): Promise<ClashDisplayEntry[] | null> => {
    if (!kernel.detectClashes) {
      setError(t('clash.unsupportedKernel'));
      return null;
    }
    const ticket = ++latestRun.current;
    setBusy(true);
    setError(null);
    setProgressText('');
    try {
      const activeCatalog = collectClashCatalog(activeReadyModels, activeSpatialTrees);
      const parsedClearance = Number(clearanceText.replace(',', '.'));
      const clearanceMm = Number.isFinite(parsedClearance) && parsedClearance > 0 ? parsedClearance : 0;
      const rule: ClashRule = {
        name: preset,
        setA: {
          modelId: modelA,
          ifcClasses: resolveClassList(classA),
          storeys: storeyA ? [storeyA] : []
        },
        setB: {
          modelId: modelB,
          ifcClasses: resolveClassList(classB),
          storeys: storeyB ? [storeyB] : []
        },
        clearanceMm
      };
      const request = buildClashRequest(rule, activeReadyModels, activeCatalog);
      const result = await kernel.detectClashes(request, (phase, pct) => {
        if (ticket === latestRun.current) {
          setProgressText(t('app.statusPhaseProgress', { phase, pct }));
        }
      });
      if (ticket !== latestRun.current) return null;
      const built = buildClashDisplayEntries(result, activeCatalog);
      setReport(result);
      setEntries(built);
      setActiveId(null);
      setKindFilter('all');
      return built;
    } catch (err) {
      if (ticket !== latestRun.current) return null;
      setError(err instanceof Error ? err.message : t('clash.errorFallback'));
      return null;
    } finally {
      if (ticket === latestRun.current) {
        setBusy(false);
        setProgressText('');
      }
    }
  };

  const handleRun = async (event: React.FormEvent) => {
    event.preventDefault();
    await executeClashDetection(readyModels, spatialTrees);
  };

  const handleClear = () => {
    latestRun.current++;
    setReport(null);
    setEntries([]);
    setActiveId(null);
    setError(null);
    onClearClashView();
  };

  const handleSelectEntry = (entry: ClashDisplayEntry) => {
    setActiveId(entry.id);
    onInspectPair(entry);
  };

  const handleSaveEntryBcf = async (
    event: React.MouseEvent,
    entry: ClashDisplayEntry
  ) => {
    event.stopPropagation();
    setSavingBcfId(entry.id);
    setActiveId(entry.id);
    try {
      await onSaveAsBcf(entry);
    } finally {
      setSavingBcfId(null);
    }
  };

  const handleLoadLandXml = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    setExportError(null);
    setExportStatus(null);
    try {
      const text = await file.text();
      const doc = parseLandXml(text);
      if (doc.alignments.length === 0) {
        setExportError(t('clash.landXmlEmptyError'));
        return;
      }
      setLandXmlFileName(file.name);
      setAlignments(doc.alignments);
      setSelectedAlignmentIdx(0);
      setSvgPreview(null);
    } catch (err) {
      setExportError(err instanceof Error ? err.message : t('clash.landXmlParseError'));
    }
  };

  const handleExportDxfZip = async () => {
    if (!boundAlignment || filteredEntries.length === 0) return;
    const abort = new AbortController();
    exportAbortRef.current = abort;
    setExportBusy(true);
    setExportError(null);
    setExportStatus(null);
    setExportProgressText('');
    try {
      const parsedTol = Number(mergeToleranceText.replace(',', '.'));
      const mergeToleranceMm = Number.isFinite(parsedTol) && parsedTol >= 0 ? parsedTol : 20;
      const result = await exportClashAlignmentZip(
        filteredEntries,
        readyModels.map((m) => m.id),
        boundAlignment,
        kernel,
        {
          mergeToleranceMm,
          signal: abort.signal,
          onProgress: (phase, pct) => {
            setExportProgressText(t('app.statusPhaseProgress', { phase, pct }));
          }
        }
      );
      await saveOutputWithFallback(filePort, {
        defaultName: result.fileName,
        content: result.zipBytes,
        mimeType: 'application/zip',
        filterName: 'ZIP',
        extensions: ['zip']
      });
      setExportStatus(t('clash.exportDoneStatus', { count: result.sections.length }));
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        setExportStatus(t('clash.exportCancelledStatus'));
      } else {
        setExportError(err instanceof Error ? err.message : t('clash.exportErrorFallback'));
      }
    } finally {
      exportAbortRef.current = null;
      setExportBusy(false);
      setExportProgressText('');
    }
  };

  const handleCancelExport = () => {
    exportAbortRef.current?.abort();
  };

  const handleReimportDxf = async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file || !kernel.applyClashDxfRoundTrip || !primaryModel) return;
    setPendingConflict(null);
    await runRoundTrip(new Uint8Array(await file.arrayBuffer()), 'ask');
  };

  /** The user's answer to the F7 warning: apply the same DXF again with that policy. */
  const handleResolveConflict = (policy: Exclude<ClashDxfConflictPolicy, 'ask'> | 'cancel') => {
    const pending = pendingConflict;
    setPendingConflict(null);
    if (!pending) return;
    if (policy === 'cancel') {
      setExportStatus(t('clash.roundTripConflictCancelled'));
      return;
    }
    void runRoundTrip(pending.dxfBytes, policy);
  };

  const runRoundTrip = async (dxfBytes: Uint8Array, conflict: ClashDxfConflictPolicy) => {
    if (!kernel.applyClashDxfRoundTrip || !primaryModel) return;
    setRoundTripBusy(true);
    setExportError(null);
    setExportStatus(t('clash.roundTripBusyStatus'));
    try {
      const rtResult = await kernel.applyClashDxfRoundTrip(primaryModel.id, dxfBytes, { conflict });
      // Elements another section edited after this DXF was exported: nothing is kept from
      // this pass until the user says whether to add, replace or skip them.
      const conflicted = rtResult.edits.filter(
        (e) => e.status === 'skipped' && e.reason === 'edit-conflict'
      );
      if (conflict === 'ask' && conflicted.length > 0) {
        const elements = new Map<string, string>();
        for (const e of conflicted) {
          const key = `${e.modelId ?? ''}#${e.expressId}`;
          elements.set(key, e.globalId ? `#${e.expressId} ${e.globalId}` : `#${e.expressId}`);
        }
        setPendingConflict({
          dxfBytes,
          elements: [...elements].map(([key, label]) => ({ key, label }))
        });
        setExportStatus(null);
        return;
      }
      const reasons = describeSkippedReasons(rtResult.edits);
      if (rtResult.edits.length === 0) {
        setExportStatus(t('clash.roundTripNoEdits'));
        return;
      }
      const patchedNow =
        rtResult.patchedModels ??
        (rtResult.applied > 0
          ? [{ modelId: rtResult.modelId, patchedIfcBytes: rtResult.patchedIfcBytes, applied: rtResult.applied }]
          : []);
      // The change log stays downloadable after an import that applied nothing, and so does a
      // model an earlier import patched: what is in memory is still that patched file.
      setRoundTripResult((prev) => {
        const byModel = new Map((prev?.patchedModels ?? []).map((m) => [m.modelId, m]));
        for (const m of patchedNow) byModel.set(m.modelId, m);
        return { ...rtResult, patchedModels: [...byModel.values()] };
      });
      if (rtResult.applied === 0) {
        setExportStatus(null);
        setExportError(t('clash.roundTripAllSkipped', { skipped: rtResult.skipped, reasons }));
        return;
      }

      let nextModels = readyModels;
      let nextTrees = spatialTrees;
      if (onReloadPatchedModel) {
        for (const patched of patchedNow) {
          const reloaded = await onReloadPatchedModel(patched.modelId, patched.patchedIfcBytes);
          if (reloaded) {
            nextModels = nextModels.map((m) => (m.id === reloaded.model.id ? reloaded.model : m));
            if (reloaded.tree) {
              nextTrees = { ...nextTrees, [reloaded.model.id]: reloaded.tree };
            }
          }
        }
      }

      const newEntries = await executeClashDetection(nextModels, nextTrees);
      const remaining = newEntries ? newEntries.length : entries.length;
      setExportStatus(
        rtResult.skipped > 0
          ? t('clash.roundTripDoneWithSkipped', {
              applied: rtResult.applied,
              skipped: rtResult.skipped,
              reasons,
              remaining
            })
          : t('clash.roundTripDoneStatus', {
              applied: rtResult.applied,
              remaining
            })
      );
    } catch (err) {
      setExportStatus(null);
      setExportError(err instanceof Error ? err.message : t('clash.roundTripErrorFallback'));
    } finally {
      setRoundTripBusy(false);
    }
  };

  const handleDownloadPatchedIfc = async () => {
    if (!roundTripResult) return;
    for (const patched of roundTripResult.patchedModels ?? []) {
      const targetModel = readyModels.find((m) => m.id === patched.modelId);
      const origName = targetModel?.fileName ?? 'model.ifc';
      const patchedName = origName.replace(/\.ifc$/i, '') + '_patched.ifc';
      await saveOutputWithFallback(filePort, {
        defaultName: patchedName,
        content: patched.patchedIfcBytes,
        mimeType: 'application/x-step',
        filterName: 'IFC',
        extensions: ['ifc']
      });
    }
  };

  const handleDownloadRoundTripCsv = async () => {
    if (!roundTripResult) return;
    const targetModel = readyModels.find((m) => m.id === roundTripResult.modelId) ?? primaryModel;
    const origName = (targetModel?.fileName ?? 'model').replace(/\.ifc$/i, '');
    await saveOutputWithFallback(filePort, {
      defaultName: `${origName}_roundtrip_changes.csv`,
      content: roundTripResult.auditCsv,
      mimeType: 'text/csv;charset=utf-8',
      filterName: 'CSV',
      extensions: ['csv']
    });
  };

  const handlePreviewSvg = async (event: React.MouseEvent, entry: ClashDisplayEntry) => {
    event.stopPropagation();
    if (!boundAlignment) return;
    setPreviewBusyId(entry.id);
    setExportError(null);
    try {
      const preview = await previewSingleClashSectionSvg(
        entry,
        readyModels.map((m) => m.id),
        boundAlignment,
        kernel
      );
      setSvgPreview(preview);
    } catch (err) {
      setExportError(err instanceof Error ? err.message : t('clash.exportErrorFallback'));
    } finally {
      setPreviewBusyId(null);
    }
  };

  const summary = useMemo(() => summarizeClashes(entries), [entries]);
  const filteredEntries = useMemo(() => {
    if (kindFilter === 'all') return entries;
    return entries.filter((e) => e.kind === kindFilter);
  }, [entries, kindFilter]);

  if (readyModels.length === 0) {
    return <div className="clash-empty">{t('clash.needModel')}</div>;
  }

  const pipeMatchCount = PIPE_CLASH_CLASSES.filter((c) => catalog.classes.includes(c)).length;
  const structMatchCount = STRUCTURE_CLASH_CLASSES.filter((c) => catalog.classes.includes(c)).length;
  const visibleEntries = filteredEntries.slice(0, MAX_DISPLAY_ROWS);
  const hiddenCount = Math.max(0, filteredEntries.length - visibleEntries.length);

  return (
    <div className="clash-panel">
      <form className="clash-form" onSubmit={(e) => void handleRun(e)}>
        <label className="clash-field">
          <span className="clash-label">{t('clash.presetLabel')}</span>
          <select
            className="select clash-preset-select"
            value={preset}
            onChange={(e) => applyPreset(e.target.value as ClashPresetId)}
          >
            <option value="all-vs-all">{t('clash.presetAllVsAll')}</option>
            <option value="pipes-vs-structure">{t('clash.presetPipesVsStructure')}</option>
            <option value="custom">{t('clash.presetCustom')}</option>
          </select>
        </label>

        <div className="clash-set-box">
          <div className="clash-set-title">{t('clash.setALabel')}</div>
          <select
            className="select clash-model-a"
            value={modelA}
            onChange={(e) => {
              setPreset('custom');
              setModelA(e.target.value);
            }}
            title={t('clash.modelSelectTitle', { set: t('clash.setALabel') })}
          >
            <option value="*">{t('clash.modelAll')}</option>
            {readyModels.map((m) => (
              <option key={m.id} value={m.id}>
                {m.fileName}
              </option>
            ))}
          </select>
          <select
            className="select clash-class-a"
            value={classA}
            onChange={(e) => {
              setPreset('custom');
              setClassA(e.target.value);
            }}
            title={t('clash.classSelectTitle', { set: t('clash.setALabel') })}
          >
            <option value="">{t('clash.classAll')}</option>
            <option value={PRESET_PIPES_SENTINEL}>
              {t('clash.classPresetPipes', { count: pipeMatchCount })}
            </option>
            <option value={PRESET_STRUCTURE_SENTINEL}>
              {t('clash.classPresetStructure', { count: structMatchCount })}
            </option>
            {catalog.classes.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
          {catalog.storeys.length > 0 && (
            <select
              className="select clash-storey-a"
              value={storeyA}
              onChange={(e) => {
                setPreset('custom');
                setStoreyA(e.target.value);
              }}
              title={t('clash.storeySelectTitle', { set: t('clash.setALabel') })}
            >
              <option value="">{t('clash.storeyAll')}</option>
              {catalog.storeys.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          )}
        </div>

        <div className="clash-set-box">
          <div className="clash-set-title">{t('clash.setBLabel')}</div>
          <select
            className="select clash-model-b"
            value={modelB}
            onChange={(e) => {
              setPreset('custom');
              setModelB(e.target.value);
            }}
            title={t('clash.modelSelectTitle', { set: t('clash.setBLabel') })}
          >
            <option value="*">{t('clash.modelAll')}</option>
            {readyModels.map((m) => (
              <option key={m.id} value={m.id}>
                {m.fileName}
              </option>
            ))}
          </select>
          <select
            className="select clash-class-b"
            value={classB}
            onChange={(e) => {
              setPreset('custom');
              setClassB(e.target.value);
            }}
            title={t('clash.classSelectTitle', { set: t('clash.setBLabel') })}
          >
            <option value="">{t('clash.classAll')}</option>
            <option value={PRESET_PIPES_SENTINEL}>
              {t('clash.classPresetPipes', { count: pipeMatchCount })}
            </option>
            <option value={PRESET_STRUCTURE_SENTINEL}>
              {t('clash.classPresetStructure', { count: structMatchCount })}
            </option>
            {catalog.classes.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
          {catalog.storeys.length > 0 && (
            <select
              className="select clash-storey-b"
              value={storeyB}
              onChange={(e) => {
                setPreset('custom');
                setStoreyB(e.target.value);
              }}
              title={t('clash.storeySelectTitle', { set: t('clash.setBLabel') })}
            >
              <option value="">{t('clash.storeyAll')}</option>
              {catalog.storeys.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          )}
        </div>

        <label className="clash-field">
          <span className="clash-label">{t('clash.clearanceLabel')}</span>
          <input
            type="number"
            min={0}
            step={5}
            className="input clash-clearance-input"
            value={clearanceText}
            onChange={(e) => setClearanceText(e.target.value)}
            title={t('clash.clearanceTitle')}
          />
        </label>

        <div className="clash-actions">
          <button
            type="submit"
            className="btn btn-primary clash-run-btn"
            style={{ flex: 1, justifyContent: 'center' }}
            disabled={busy}
          >
            <Play size={13} />
            <span>{busy ? t('clash.running') : t('clash.runBtn')}</span>
          </button>
          {report && (
            <button
              type="button"
              className="btn btn-secondary clash-clear-btn"
              onClick={handleClear}
              title={t('clash.clearTitle')}
            >
              <X size={13} />
              <span>{t('clash.clearBtn')}</span>
            </button>
          )}
        </div>

        {busy && progressText && <div className="clash-progress">{progressText}</div>}
      </form>

      {error && <div className="clash-error">{error}</div>}

      {report && (
        <div className="clash-results">
          <div className="clash-summary">
            <div className="clash-summary-stats">
              {t('clash.summaryStats', {
                total: summary.total,
                broad: report.broadPhasePairs,
                ms: report.elapsedMs
              })}
            </div>
            <div className="clash-filter-pills">
              <button
                type="button"
                className={`clash-pill ${kindFilter === 'all' ? 'active' : ''}`}
                onClick={() => setKindFilter('all')}
              >
                {t('clash.filterAll', { count: summary.total })}
              </button>
              <button
                type="button"
                className={`clash-pill hard ${kindFilter === 'hard' ? 'active' : ''}`}
                onClick={() => setKindFilter('hard')}
              >
                {t('clash.filterHard', { count: summary.hard })}
              </button>
              <button
                type="button"
                className={`clash-pill soft ${kindFilter === 'soft' ? 'active' : ''}`}
                onClick={() => setKindFilter('soft')}
              >
                {t('clash.filterSoft', { count: summary.soft })}
              </button>
            </div>
          </div>

          {report.truncated && (
            <div className="clash-notice">
              {t('clash.truncatedNotice', { count: entries.length })}
            </div>
          )}

          {(filteredEntries.length > 0 ||
            boundAlignment !== null ||
            roundTripResult !== null ||
            pendingConflict !== null) &&
            (kernel.exportClashAlignmentDxf || kernel.applyClashDxfRoundTrip) && (
              <div className="clash-alignment-box" data-testid="clash-alignment-dxf">
                <div className="clash-alignment-header">
                  <span>{t('clash.dxfTitle')}</span>
                  <div className="clash-alignment-actions">
                    {kernel.exportClashAlignmentDxf && (
                      <>
                        <input
                          ref={landXmlInputRef}
                          type="file"
                          accept=".xml,.landxml"
                          style={{ display: 'none' }}
                          onChange={(e) => void handleLoadLandXml(e)}
                        />
                        <button
                          type="button"
                          className="btn btn-secondary btn-xs"
                          onClick={() => landXmlInputRef.current?.click()}
                          title={t('clash.loadLandXmlTitle')}
                        >
                          <FileUp size={12} />
                          <span>
                            {landXmlFileName ? t('clash.changeLandXmlBtn') : t('clash.loadLandXmlBtn')}
                          </span>
                        </button>
                      </>
                    )}
                    {kernel.applyClashDxfRoundTrip && (
                      <>
                        <input
                          ref={roundTripDxfInputRef}
                          type="file"
                          accept=".dxf"
                          style={{ display: 'none' }}
                          onChange={(e) => void handleReimportDxf(e)}
                        />
                        <button
                          type="button"
                          className="btn btn-secondary btn-xs"
                          data-testid="clash-reimport-dxf-btn"
                          disabled={roundTripBusy}
                          onClick={() => roundTripDxfInputRef.current?.click()}
                          title={t('clash.reimportDxfTitle')}
                        >
                          <RefreshCw size={12} />
                          <span>{t('clash.reimportDxfBtn')}</span>
                        </button>
                      </>
                    )}
                  </div>
                </div>

              {boundAlignment && (
                <div className="clash-alignment-controls">
                  {alignments.length > 1 && (
                    <label className="clash-inline-field">
                      <span>{t('clash.alignmentSelectLabel')}</span>
                      <select
                        value={selectedAlignmentIdx}
                        onChange={(e) => setSelectedAlignmentIdx(Number(e.target.value))}
                        title={t('clash.alignmentSelectTitle')}
                      >
                        {alignments.map((al, i) => (
                          <option key={`${al.name}-${i}`} value={i}>
                            {al.name}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}

                  <label className="clash-inline-field">
                    <span>{t('clash.mergeTolLabel')}</span>
                    <input
                      type="number"
                      min="0"
                      step="5"
                      value={mergeToleranceText}
                      onChange={(e) => setMergeToleranceText(e.target.value)}
                      title={t('clash.mergeTolTitle')}
                    />
                  </label>

                  <div className="clash-alignment-actions">
                    <button
                      type="button"
                      className="btn btn-primary btn-xs"
                      disabled={exportBusy}
                      onClick={() => void handleExportDxfZip()}
                      title={t('clash.exportDxfZipTitle')}
                    >
                      <Download size={12} />
                      <span>{t('clash.exportDxfZipBtn')}</span>
                    </button>
                    {exportBusy && (
                      <button
                        type="button"
                        className="btn btn-secondary btn-xs"
                        onClick={handleCancelExport}
                        title={t('clash.cancelExportTitle')}
                      >
                        <X size={12} />
                        <span>{t('clash.cancelExportBtn')}</span>
                      </button>
                    )}
                  </div>
                </div>
              )}

              {boundAlignment?.warnings.some((w) => w.code === 'no-map-conversion-disjoint-bounds') && (
                <div className="clash-notice">{t('clash.landXmlDisjointWarning')}</div>
              )}
              {exportProgressText && <div className="clash-progress">{exportProgressText}</div>}
              {exportStatus && <div className="clash-notice">{exportStatus}</div>}
              {pendingConflict && (
                <div className="clash-notice clash-roundtrip-conflict" data-testid="clash-roundtrip-conflict">
                  <div className="clash-roundtrip-conflict-text">
                    <AlertTriangle size={12} />
                    <span>
                      {t('clash.roundTripConflictText', { count: pendingConflict.elements.length })}
                    </span>
                  </div>
                  <ul className="clash-roundtrip-conflict-list">
                    {pendingConflict.elements.slice(0, 8).map((el) => (
                      <li key={el.key}>{el.label}</li>
                    ))}
                    {pendingConflict.elements.length > 8 && (
                      <li>{t('clash.roundTripConflictMore', { count: pendingConflict.elements.length - 8 })}</li>
                    )}
                  </ul>
                  <div className="clash-alignment-actions">
                    <button
                      type="button"
                      className="btn btn-primary btn-xs"
                      data-testid="clash-conflict-replace"
                      disabled={roundTripBusy}
                      onClick={() => handleResolveConflict('replace')}
                      title={t('clash.roundTripConflictReplaceTitle')}
                    >
                      <span>{t('clash.roundTripConflictReplace')}</span>
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary btn-xs"
                      data-testid="clash-conflict-add"
                      disabled={roundTripBusy}
                      onClick={() => handleResolveConflict('add')}
                      title={t('clash.roundTripConflictAddTitle')}
                    >
                      <span>{t('clash.roundTripConflictAdd')}</span>
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary btn-xs"
                      data-testid="clash-conflict-skip"
                      disabled={roundTripBusy}
                      onClick={() => handleResolveConflict('skip')}
                      title={t('clash.roundTripConflictSkipTitle')}
                    >
                      <span>{t('clash.roundTripConflictSkip')}</span>
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary btn-xs"
                      data-testid="clash-conflict-cancel"
                      disabled={roundTripBusy}
                      onClick={() => handleResolveConflict('cancel')}
                    >
                      <X size={12} />
                      <span>{t('clash.roundTripConflictCancel')}</span>
                    </button>
                  </div>
                </div>
              )}
              {roundTripResult && (
                <div className="clash-alignment-actions" data-testid="clash-roundtrip-downloads">
                  {(roundTripResult.patchedModels?.length ?? 0) > 0 && (
                    <button
                      type="button"
                      className="btn btn-primary btn-xs"
                      onClick={() => void handleDownloadPatchedIfc()}
                      title={t('clash.downloadPatchedIfcTitle')}
                    >
                      <Download size={12} />
                      <span>{t('clash.downloadPatchedIfcBtn')}</span>
                    </button>
                  )}
                  <button
                    type="button"
                    className="btn btn-secondary btn-xs"
                    onClick={() => void handleDownloadRoundTripCsv()}
                    title={t('clash.downloadRoundTripCsvTitle')}
                  >
                    <Download size={12} />
                    <span>{t('clash.downloadRoundTripCsvBtn')}</span>
                  </button>
                </div>
              )}
              {exportError && <div className="clash-error">{exportError}</div>}

              {svgPreview && (
                <div className="clash-svg-preview" data-testid="clash-svg-preview">
                  <div className="clash-svg-preview-bar">
                    <span>
                      {t('clash.previewHeader', {
                        pk: svgPreview.pkLabel,
                        mode:
                          svgPreview.orientation === 'transverse'
                            ? t('clash.previewModeTransverse', {
                                mm: formatNumber(svgPreview.depthMm, { maximumFractionDigits: 1 })
                              })
                            : t('clash.previewModeLongitudinal')
                      })}
                    </span>
                    <button
                      type="button"
                      className="btn btn-secondary btn-xs"
                      onClick={() => setSvgPreview(null)}
                      title={t('clash.previewCloseBtn')}
                    >
                      <X size={12} />
                    </button>
                  </div>
                  <div
                    className="clash-svg-preview-stage"
                    dangerouslySetInnerHTML={{ __html: svgPreview.svg }}
                  />
                </div>
              )}
            </div>
          )}

          {filteredEntries.length === 0 ? (
            <div className="clash-empty">{t('clash.noClashes')}</div>
          ) : (
            <div className="clash-list">
              {visibleEntries.map((entry) => {
                const isSelected = activeId === entry.id;
                const mmVal =
                  entry.kind === 'hard'
                    ? formatNumber(entry.penetrationM * 1000, { maximumFractionDigits: 1 })
                    : formatNumber(entry.gapM * 1000, { maximumFractionDigits: 1 });
                return (
                  <div
                    key={entry.id}
                    className={`clash-card ${entry.kind} ${isSelected ? 'active' : ''}`}
                    onClick={() => handleSelectEntry(entry)}
                    title={t('clash.inspectPairTitle')}
                    data-clash-id={entry.id}
                    data-clash-kind={entry.kind}
                  >
                    <div className="clash-card-top">
                      <span className={`clash-badge ${entry.kind}`}>
                        <AlertTriangle size={11} />
                        <span>
                          {entry.kind === 'hard' ? t('clash.kindHard') : t('clash.kindSoft')}
                        </span>
                      </span>
                      <span className="clash-metric">
                        {entry.kind === 'hard'
                          ? t('clash.penetrationLabel', { mm: mmVal })
                          : t('clash.gapLabel', { mm: mmVal })}
                      </span>
                      {boundAlignment && kernel.exportClashAlignmentDxf && (
                        <button
                          type="button"
                          className="btn btn-secondary btn-xs"
                          disabled={previewBusyId === entry.id}
                          onClick={(e) => void handlePreviewSvg(e, entry)}
                          title={t('clash.previewSvgTitle')}
                        >
                          <Eye size={12} />
                          <span>{t('clash.previewSvgBtn')}</span>
                        </button>
                      )}
                      <button
                        type="button"
                        className="btn btn-secondary btn-xs clash-bcf-btn"
                        disabled={savingBcfId === entry.id}
                        onClick={(e) => void handleSaveEntryBcf(e, entry)}
                        title={t('clash.saveBcfTitle')}
                      >
                        <BookmarkPlus size={12} />
                        <span>{t('clash.saveBcfBtn')}</span>
                      </button>
                    </div>

                    <div className="clash-pair-body">
                      <div className="clash-elem clash-elem-a">
                        <div className="clash-elem-name">
                          {t('clash.pairElementLine', {
                            name: entry.metaA.name,
                            type: entry.metaA.type
                          })}
                        </div>
                        <div className="clash-elem-meta">
                          {entry.metaA.storey
                            ? t('clash.pairMetaLine', {
                                model: entry.metaA.modelName,
                                storey: entry.metaA.storey
                              })
                            : t('clash.pairMetaNoStorey', { model: entry.metaA.modelName })}
                        </div>
                      </div>
                      <div className="clash-elem clash-elem-b">
                        <div className="clash-elem-name">
                          {t('clash.pairElementLine', {
                            name: entry.metaB.name,
                            type: entry.metaB.type
                          })}
                        </div>
                        <div className="clash-elem-meta">
                          {entry.metaB.storey
                            ? t('clash.pairMetaLine', {
                                model: entry.metaB.modelName,
                                storey: entry.metaB.storey
                              })
                            : t('clash.pairMetaNoStorey', { model: entry.metaB.modelName })}
                        </div>
                      </div>
                      {entry.volumeM3 !== undefined && (
                        <div className="clash-volume">
                          {t('clash.volumeLabel', {
                            cm3: formatNumber(entry.volumeM3 * 1e6, { maximumFractionDigits: 1 })
                          })}
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
              {hiddenCount > 0 && (
                <div className="clash-more">
                  {t('common.ellipsisAndMore', { count: hiddenCount })}
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
};
