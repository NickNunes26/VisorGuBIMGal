import type { ModelId } from '../../domain/models/ModelRecord';
import type { GeometryBatch } from '../../domain/kernel/GeometryBatch';
import type { GeometryEvent, KernelErrorCode } from '../../domain/kernel/KernelPort';

/** Each Worker holds one model, opened by its first request. */
export type WorkerRequest =
  | { op: 'open'; modelId: ModelId; bytes: ArrayBuffer }
  | { op: 'start-stream' }
  | { op: 'next-batch' }
  | { op: 'details'; expressId: number }
  | { op: 'tree' };

/** One pull of a geometry stream. */
export type StreamStep =
  | { batch: GeometryBatch; processedElements: number; totalElements: number }
  | { complete: Extract<GeometryEvent, { type: 'complete' }> };

export type WorkerResponse =
  | { id: number; ok: true; value: unknown }
  | { id: number; ok: false; code: KernelErrorCode; message: string; offset?: number };
