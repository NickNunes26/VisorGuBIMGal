import type { ModelId } from '../../domain/models/ModelRecord';
import type { GeometryBatch } from '../../domain/kernel/GeometryBatch';
import type { KernelDiagnostic, KernelErrorCode, ModelSummary } from '../../domain/kernel/KernelPort';
import type { StreamStep, WorkerRequest, WorkerResponse } from './protocol';
import init, { allocationFailed, Model, Sheet } from './pkg/wasm_api';

/** Large enough to avoid chatter, small enough that the app stays responsive. */
const MAX_TRIANGLES_PER_BATCH = 65536;

class RequestError extends Error {
  constructor(
    readonly code: KernelErrorCode,
    message: string,
    readonly offset?: number
  ) {
    super(message);
  }
}

/** A chunk that keeps the relay light. An element is never split, so some run larger. */
const MAX_TRIANGLES_PER_CHUNK = 65536;

const ready = init();
let open: { modelId: ModelId; model: Model } | null = null;
let sheet: Sheet | null = null;
let sequence = 0;

function current(): { modelId: ModelId; model: Model } {
  if (!open) throw new RequestError('model-not-open', 'El modelo no está abierto.');
  return open;
}

/** Kernel errors arrive as `code|offset|message`. See `open_error` in wasm-api. */
function toRequestError(error: unknown): RequestError {
  if (error instanceof RequestError) return error;
  const text = error instanceof Error ? error.message : String(error);
  // A trap leaves the instance unusable: later calls must not reach it.
  if (error instanceof WebAssembly.RuntimeError) {
    open = null;
    sheet = null;
    return allocationFailed()
      ? new RequestError('out-of-memory', 'El modelo no cabe en la memoria disponible.')
      : new RequestError('internal', `El kernel se ha detenido: ${text}`);
  }
  const [code, offset, ...message] = text.split('|');
  if ((code === 'parse' || code === 'unsupported-schema') && message.length > 0) {
    return new RequestError(code, message.join('|'), offset ? Number(offset) : undefined);
  }
  if (error instanceof RangeError || /memory/i.test(text)) {
    return new RequestError('out-of-memory', text);
  }
  return new RequestError('internal', text);
}

function withModelId(diagnostics: KernelDiagnostic[], modelId: ModelId): KernelDiagnostic[] {
  return diagnostics.map((diagnostic) => ({ ...diagnostic, modelId }));
}

function openModel(modelId: ModelId, bytes: ArrayBuffer): ModelSummary {
  const model = new Model(new Uint8Array(bytes));
  open = { modelId, model };
  const summary = JSON.parse(model.summaryJson()) as ModelSummary;
  return { ...summary, diagnostics: withModelId(summary.diagnostics, modelId) };
}

/** Each `take` hands over a fresh buffer, so transferring it leaves WASM memory alone. */
function readBatch(modelId: ModelId, model: Model): GeometryBatch | null {
  const view = model.nextBatch(MAX_TRIANGLES_PER_BATCH);
  if (!view) return null;
  try {
    const [x, y, z] = view.origin();
    sequence += 1;
    return {
      modelId,
      sequence,
      origin: [x, y, z],
      positions: view.takePositions(),
      normals: view.takeNormals(),
      indices: view.takeIndices(),
      meshTable: view.takeMeshTable()
    };
  } finally {
    view.free();
  }
}

function nextBatch(): { value: StreamStep; transfer: Transferable[] } {
  const { modelId, model } = current();
  const batch = readBatch(modelId, model);
  if (batch) {
    const { positions, normals, indices, meshTable } = batch;
    return {
      value: {
        batch,
        processedElements: model.processedElements(),
        totalElements: model.totalElements()
      },
      transfer: [positions.buffer, normals.buffer, indices.buffer, meshTable.buffer] as Transferable[]
    };
  }
  const complete = JSON.parse(model.finishStream());
  const diagnostics = withModelId(complete.diagnostics, modelId);
  return { value: { complete: { type: 'complete', ...complete, diagnostics } }, transfer: [] };
}

function currentSheet(): Sheet {
  if (!sheet) throw new RequestError('drawing-not-found', 'El plano no existe.');
  return sheet;
}

function nextChunk(): { value: unknown; transfer: Transferable[] } {
  const chunk = current().model.nextDrawingChunk(MAX_TRIANGLES_PER_CHUNK);
  return chunk
    ? { value: chunk.buffer, transfer: [chunk.buffer as ArrayBuffer] }
    : { value: null, transfer: [] };
}

function exportSheet(
  request: Extract<WorkerRequest, { op: 'sheet-export' }>
): { value: unknown; transfer: Transferable[] } {
  const { svg, scale, showHidden, showHatching, comment } = request;
  const bytes = currentSheet().export(svg, scale, showHidden, showHatching, comment);
  if (!bytes) throw new RequestError('drawing-not-found', 'El plano aún no está generado.');
  return { value: bytes, transfer: [bytes.buffer as ArrayBuffer] };
}

function handle(request: WorkerRequest): { value: unknown; transfer: Transferable[] } {
  switch (request.op) {
    case 'draw-start':
      current().model.startDrawing(new Float64Array(request.request));
      return { value: undefined, transfer: [] };
    case 'draw-next':
      return nextChunk();
    case 'draw-finish':
      return { value: JSON.parse(current().model.finishDrawing()), transfer: [] };
    case 'sheet-new':
      sheet = new Sheet(new Float64Array(request.request));
      return { value: undefined, transfer: [] };
    case 'sheet-add':
      currentSheet().addChunk(request.modelIndex, new Uint8Array(request.chunk));
      return { value: undefined, transfer: [] };
    case 'sheet-drop':
      currentSheet().dropModel(request.modelIndex);
      return { value: undefined, transfer: [] };
    case 'sheet-generate':
      return { value: JSON.parse(currentSheet().generate()), transfer: [] };
    case 'sheet-export':
      return exportSheet(request);
    case 'open':
      return { value: openModel(request.modelId, request.bytes), transfer: [] };
    case 'start-stream':
      current().model.startStream();
      sequence = 0;
      return { value: undefined, transfer: [] };
    case 'next-batch':
      return nextBatch();
    case 'details': {
      const json = current().model.elementDetailsJson(request.expressId);
      return { value: json ? JSON.parse(json) : null, transfer: [] };
    }
    case 'tree': {
      const json = current().model.spatialTreeJson();
      return { value: json ? JSON.parse(json) : null, transfer: [] };
    }
  }
}

self.onmessage = async (event: MessageEvent<{ id: number; request: WorkerRequest }>) => {
  const { id, request } = event.data;
  try {
    await ready;
    const { value, transfer } = handle(request);
    const response: WorkerResponse = { id, ok: true, value };
    self.postMessage(response, { transfer });
  } catch (error) {
    const { code, message, offset } = toRequestError(error);
    const response: WorkerResponse = { id, ok: false, code, message, offset };
    self.postMessage(response);
  }
};
