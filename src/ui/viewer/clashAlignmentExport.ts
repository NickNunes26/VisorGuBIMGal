import {
  buildClashSummaryCsv,
  planClashAlignmentSections,
  type ClashForAlignmentPlan,
  type PlannedClashSection
} from '../../domain/alignment/clashDxfPlan';
import type { BoundAlignment } from '../../domain/alignment/landXml';
import type {
  ClashAlignmentDxfBatch,
  ClashAlignmentSectionSpec
} from '../../domain/clash/ClashTypes';
import type { KernelPort, ProgressHandler } from '../../domain/kernel/KernelPort';
import type { ModelId } from '../../domain/models/ModelRecord';
import type { ClashDisplayEntry } from './clashDetector';
import { StreamingZipEntry, writeZip, type DeflatedZipEntry, type ZipEntry } from './zipArchive';

export interface ClashAlignmentZipExportResult {
  zipBytes: Uint8Array;
  fileName: string;
  sections: PlannedClashSection[];
  batch: ClashAlignmentDxfBatch;
  summaryCsv: string;
}

async function resolveClashesForPlan(
  entries: readonly ClashDisplayEntry[],
  bound: BoundAlignment,
  kernel: Pick<KernelPort, 'elementMesh' | 'getElementDetails' | 'elementSlabSpan'>,
  signal?: AbortSignal
): Promise<ClashForAlignmentPlan[]> {
  const meshCache = new Map<string, Float64Array>();
  const guidCache = new Map<string, string>();
  const out: ClashForAlignmentPlan[] = [];

  for (let i = 0; i < entries.length; i++) {
    if (signal?.aborted) {
      throw new DOMException('Exportación cancelada.', 'AbortError');
    }
    const e = entries[i];
    const keyA = `${e.elementA.modelId}:${e.elementA.expressId}`;

    let precomputedSlab = undefined;
    let verts: Float64Array = new Float64Array(0);
    if (kernel.elementSlabSpan) {
      const proj = bound.projectLocal(e.point[0], e.point[1]);
      precomputedSlab =
        (await kernel.elementSlabSpan(
          e.elementA.modelId,
          e.elementA.expressId,
          proj.tangent,
          e.point
        )) ?? undefined;
    } else {
      let cached = meshCache.get(keyA);
      if (!cached) {
        const mesh = await kernel.elementMesh(e.elementA.modelId, e.elementA.expressId);
        cached = mesh?.positions ?? new Float64Array(0);
        meshCache.set(keyA, cached);
      }
      verts = cached;
    }

    let globalIdA = guidCache.get(keyA);
    if (globalIdA === undefined) {
      const details = await kernel.getElementDetails(e.elementA);
      globalIdA = details?.globalId ?? '';
      guidCache.set(keyA, globalIdA);
    }

    out.push({
      clashIndex: i,
      modelIdA: e.elementA.modelId,
      expressIdA: e.elementA.expressId,
      globalIdA,
      labelA: e.metaA.name,
      modelIdB: e.elementB.modelId,
      expressIdB: e.elementB.expressId,
      labelB: e.metaB.name,
      kind: e.kind,
      penetrationM: e.penetrationM,
      gapM: e.gapM,
      point: e.point,
      precomputedSlab,
      elementAVertices: verts
    });
  }
  return out;
}

function toSectionSpec(section: PlannedClashSection, primaryModelId: ModelId): ClashAlignmentSectionSpec {
  return {
    fileName: section.fileName,
    pkM: section.pkM,
    pkLabel: section.pkLabel,
    origin: section.origin,
    normal: section.normal,
    alignmentPoint: section.alignmentPoint,
    sMin: section.sMin,
    sMax: section.sMax,
    depthM: section.depthM,
    modelId: section.clashA[0]?.modelId ?? primaryModelId,
    clashAModelIds: section.clashA.map((a) => a.modelId),
    clashAIds: section.clashA.map((a) => a.expressId),
    clashAGuids: section.clashA.map((a) => a.globalId),
    clashBModelIds: section.clashB.map((b) => b.modelId),
    clashBIds: section.clashB.map((b) => b.expressId),
    clashPoints: section.clashPoints,
    annotations: section.annotations
  };
}

/**
 * Generates a single `.zip` containing individual `(0,0)` metre DXF cross-sections
 * (`individual/PK_...dxf`), the non-overlapping master grid `clashes_all.dxf`, and `summary.csv`.
 */
export async function exportClashAlignmentZip(
  entries: readonly ClashDisplayEntry[],
  modelIds: readonly ModelId[],
  bound: BoundAlignment,
  kernel: Pick<
    KernelPort,
    'elementMesh' | 'getElementDetails' | 'exportClashAlignmentDxf' | 'elementSlabSpan'
  >,
  options: {
    mergeToleranceMm: number;
    onProgress?: ProgressHandler;
    signal?: AbortSignal;
  }
): Promise<ClashAlignmentZipExportResult> {
  if (!kernel.exportClashAlignmentDxf) {
    throw new Error('El kernel activo no soporta la exportación por lotes de planos DXF de choques.');
  }

  options.onProgress?.('Preparando bandas por Elemento A', 2);
  const forPlan = await resolveClashesForPlan(entries, bound, kernel, options.signal);
  const sections = planClashAlignmentSections(forPlan, bound, {
    mergeToleranceMm: options.mergeToleranceMm
  });

  if (sections.length === 0) {
    throw new Error('No hay choques válidos para exportar.');
  }

  const primaryModelId = modelIds[0] ?? entries[0]?.elementA.modelId ?? 'model';
  // The master grid holds every section's full drawing (gigabytes on a long bridge), so it is
  // deflated piece by piece as the kernel streams it and never kept uncompressed.
  const master = new StreamingZipEntry('clashes_all.dxf');
  let masterWritten = false;
  const batch = await kernel.exportClashAlignmentDxf(
    {
      modelIds,
      sections: sections.map((s) => toSectionSpec(s, primaryModelId)),
      includeMasterGrid: true,
      onMasterDxfChunk: async (chunk) => {
        masterWritten = true;
        await master.write(chunk);
      }
    },
    (phase, pct) => {
      const scaled = 10 + Math.round(pct * 0.85);
      options.onProgress?.(phase, scaled);
    },
    options.signal
  );

  options.onProgress?.('Empaquetando archivo ZIP', 97);
  const summaryCsv = buildClashSummaryCsv(sections);
  const encoder = new TextEncoder();
  const zipEntries: (ZipEntry | DeflatedZipEntry)[] = batch.sections.map((s) => ({
    name: s.fileName,
    data: s.dxfBytes
  }));
  if (masterWritten) {
    zipEntries.push(await master.finish());
  } else if (batch.masterDxfBytes) {
    // A kernel that ignores the sink still hands the whole file back.
    zipEntries.push({
      name: 'clashes_all.dxf',
      data: batch.masterDxfBytes
    });
  }
  zipEntries.push({
    name: 'summary.csv',
    data: encoder.encode(summaryCsv)
  });

  const zipBytes = await writeZip(zipEntries);
  const safeAxis = bound.alignment.name.replace(/[^A-Za-z0-9_-]+/g, '_') || 'eje';
  options.onProgress?.('Completado', 100);

  return {
    zipBytes,
    fileName: `choques_dxf_${safeAxis}.zip`,
    sections,
    batch,
    summaryCsv
  };
}

/** Generates an instant SVG cross-section preview for a single clash entry. */
export async function previewSingleClashSectionSvg(
  entry: ClashDisplayEntry,
  modelIds: readonly ModelId[],
  bound: BoundAlignment,
  kernel: Pick<
    KernelPort,
    'elementMesh' | 'getElementDetails' | 'exportClashAlignmentDxf' | 'elementSlabSpan'
  >
): Promise<{ svg: string; pkLabel: string; depthMm: number; orientation: 'transverse' | 'longitudinal' }> {
  if (!kernel.exportClashAlignmentDxf) {
    throw new Error('El kernel activo no soporta la vista previa de secciones de choques.');
  }
  const forPlan = await resolveClashesForPlan([entry], bound, kernel);
  const [section] = planClashAlignmentSections(forPlan, bound, { mergeToleranceMm: 0 });
  const primaryModelId = modelIds[0] ?? entry.elementA.modelId;
  const batch = await kernel.exportClashAlignmentDxf({
    modelIds,
    sections: [toSectionSpec(section, primaryModelId)],
    includeMasterGrid: false
  });
  const first = batch.sections[0];
  return {
    svg: first?.svg ?? '',
    pkLabel: section.pkLabel,
    depthMm: section.depthM * 1000,
    orientation: section.orientation
  };
}
