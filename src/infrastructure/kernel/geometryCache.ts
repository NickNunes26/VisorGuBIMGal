import { MESH_RECORD_WORDS, type GeometryBatch } from '../../domain/kernel/GeometryBatch';

/**
 * A model's stream kept in the browser's origin private file system, so reopening the same file
 * with the same kernel replays it instead of parsing and evaluating. One file per model and
 * kernel: a header, then records of a summary, the batches, the `complete` event and the
 * spatial tree, then an end record. The header's length is written last, so a file a crash cut
 * short is never read. See docs/05 section 6.
 */

/** What `FileSystemSyncAccessHandle` offers that the cache uses. Tests give an in-memory one. */
export interface CacheFile {
  read(buffer: Uint8Array, options: { at: number }): number;
  write(buffer: Uint8Array, options: { at: number }): number;
  getSize(): number;
  flush(): void;
  close(): void;
}

/** Changes whenever the layout below does, so files of an older layout are never read. */
export const CACHE_FORMAT = 1;
const MAGIC = new TextEncoder().encode('GUBIMGC1');
const HEADER_BYTES = 96;
const LAST_USED_AT = 80;
const LENGTH_AT = 88;
const RECORD_HEAD = 8;
const BATCH_HEAD = 80;

const Kind = { Summary: 1, Batch: 2, Complete: 3, Tree: 4, End: 5 } as const;
type Kind = (typeof Kind)[keyof typeof Kind];

/** The model file's SHA-256 and the kernel's identity, each 32 bytes. */
export interface CacheIdentity {
  file: Uint8Array;
  kernel: Uint8Array;
}

export class CacheCorrupt extends Error {}

/** A batch as the cache holds it: everything but the model, which differs between opens. */
export type CachedBatch = Omit<GeometryBatch, 'modelId'>;

export type CachedStep =
  | { kind: 'batch'; batch: CachedBatch; processedElements: number; totalElements: number }
  | { kind: 'complete'; json: string };

const padded = (length: number) => (length + 3) & ~3;
const bytesOf = (view: ArrayBufferView) => new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((value, at) => value === b[at]);

function header(identity: CacheIdentity, lastUsed: number, length: number): Uint8Array {
  const bytes = new Uint8Array(HEADER_BYTES);
  const view = new DataView(bytes.buffer);
  bytes.set(MAGIC, 0);
  view.setUint32(8, CACHE_FORMAT, true);
  bytes.set(identity.file, 16);
  bytes.set(identity.kernel, 48);
  view.setFloat64(LAST_USED_AT, lastUsed, true);
  view.setFloat64(LENGTH_AT, length, true);
  return bytes;
}

/** Writes a stream as it is made. `finish` makes the file readable; until then it is not. */
export class CacheWriter {
  private at = HEADER_BYTES;
  private readonly file: CacheFile;
  private readonly identity: CacheIdentity;

  constructor(file: CacheFile, identity: CacheIdentity) {
    this.file = file;
    this.identity = identity;
    this.put(header(identity, 0, 0), 0);
  }

  private put(bytes: Uint8Array, at: number): void {
    if (this.file.write(bytes, { at }) !== bytes.length) throw new Error('short write');
  }

  private record(kind: Kind, parts: Uint8Array[]): void {
    const length = parts.reduce((sum, part) => sum + part.length, 0);
    const head = new DataView(new ArrayBuffer(RECORD_HEAD));
    head.setUint32(0, kind, true);
    head.setUint32(4, length, true);
    this.put(new Uint8Array(head.buffer), this.at);
    let at = this.at + RECORD_HEAD;
    for (const part of parts) {
      this.put(part, at);
      at += part.length;
    }
    // Every record starts on a multiple of 4, so its arrays can be read in place.
    const end = this.at + RECORD_HEAD + padded(length);
    if (end > at) this.put(new Uint8Array(end - at), at);
    this.at = end;
  }

  private text(kind: Kind, json: string): void {
    this.record(kind, [new TextEncoder().encode(json)]);
  }

  summary(json: string): void {
    this.text(Kind.Summary, json);
  }

  batch(batch: CachedBatch, processedElements: number, totalElements: number): void {
    const head = new DataView(new ArrayBuffer(BATCH_HEAD));
    batch.origin.forEach((value, axis) => head.setFloat64(axis * 8, value, true));
    [...batch.box.min, ...batch.box.max].forEach((value, at) => head.setFloat32(24 + at * 4, value, true));
    const counts = [batch.positions.length, batch.normals.length, batch.indices.length, batch.meshTable.length];
    [batch.sequence, processedElements, totalElements, ...counts].forEach((value, at) =>
      head.setUint32(48 + at * 4, value, true)
    );
    this.record(Kind.Batch, [
      new Uint8Array(head.buffer),
      bytesOf(batch.positions),
      bytesOf(batch.normals),
      bytesOf(batch.indices),
      bytesOf(batch.meshTable)
    ]);
  }

  complete(json: string): void {
    this.text(Kind.Complete, json);
  }

  tree(json: string | null): void {
    this.text(Kind.Tree, json ?? 'null');
  }

  /** Ends the file and writes its length, which is what makes it readable. */
  finish(now: number): void {
    this.record(Kind.End, []);
    this.file.flush();
    this.put(header(this.identity, now, this.at), 0);
    this.file.flush();
  }
}

interface RecordAt {
  kind: Kind;
  at: number;
  length: number;
}

/**
 * Reads a finished file back. Opening checks the header against the identity and walks every
 * record's head, so a short, foreign or reshaped file is refused before anything is replayed.
 */
export class CacheReader {
  private next = 1;
  private readonly file: CacheFile;
  private readonly records: RecordAt[];

  private constructor(file: CacheFile, records: RecordAt[]) {
    this.file = file;
    this.records = records;
  }

  /** `null` when the file is not a finished cache for this identity. Throws `CacheCorrupt` on a damaged one. */
  static open(file: CacheFile, identity: CacheIdentity, now: number): CacheReader | null {
    const size = file.getSize();
    if (size < HEADER_BYTES) return null;
    const head = new Uint8Array(HEADER_BYTES);
    file.read(head, { at: 0 });
    const view = new DataView(head.buffer);
    if (!same(head.subarray(0, 8), MAGIC) || view.getUint32(8, true) !== CACHE_FORMAT) return null;
    if (!same(head.subarray(16, 48), identity.file) || !same(head.subarray(48, 80), identity.kernel)) return null;
    if (view.getFloat64(LENGTH_AT, true) !== size) return null;

    const records: RecordAt[] = [];
    const recordHead = new DataView(new ArrayBuffer(RECORD_HEAD));
    const batchHead = new DataView(new ArrayBuffer(BATCH_HEAD));
    let at = HEADER_BYTES;
    for (;;) {
      if (at + RECORD_HEAD > size) throw new CacheCorrupt('a record runs past the end');
      file.read(new Uint8Array(recordHead.buffer), { at });
      const kind = recordHead.getUint32(0, true) as Kind;
      const length = recordHead.getUint32(4, true);
      const start = at + RECORD_HEAD;
      if (start + padded(length) > size) throw new CacheCorrupt('a record runs past the end');
      if (kind === Kind.Batch) {
        if (length < BATCH_HEAD) throw new CacheCorrupt('a batch without its head');
        file.read(new Uint8Array(batchHead.buffer), { at: start });
        const [positions, normals, indices, table] = [0, 1, 2, 3].map((k) => batchHead.getUint32(60 + k * 4, true));
        const shaped =
          positions % 3 === 0 && normals === positions && indices % 3 === 0 && table % MESH_RECORD_WORDS === 0;
        if (!shaped || BATCH_HEAD + 4 * (positions + normals + indices + table) !== length) {
          throw new CacheCorrupt('a batch whose arrays do not add up');
        }
      } else if (kind < Kind.Summary || kind > Kind.End) {
        throw new CacheCorrupt(`a record of unknown kind ${kind}`);
      }
      records.push({ kind, at: start, length });
      at = start + padded(length);
      if (kind === Kind.End) break;
    }
    const kinds = records.map((record) => record.kind);
    const batches = kinds.slice(1, -3);
    const ordered =
      at === size &&
      kinds[0] === Kind.Summary &&
      batches.every((kind) => kind === Kind.Batch) &&
      kinds.slice(-3).join() === [Kind.Complete, Kind.Tree, Kind.End].join();
    if (!ordered) throw new CacheCorrupt('records out of order');

    // Marks the file used, for eviction by age.
    const stamp = new DataView(new ArrayBuffer(8));
    stamp.setFloat64(0, now, true);
    file.write(new Uint8Array(stamp.buffer), { at: LAST_USED_AT });
    return new CacheReader(file, records);
  }

  private text(record: RecordAt): string {
    const bytes = new Uint8Array(record.length);
    this.file.read(bytes, { at: record.at });
    return new TextDecoder().decode(bytes);
  }

  summary(): string {
    return this.text(this.records[0]);
  }

  /** The spatial tree's JSON, `null` when the model has none. */
  tree(): string | null {
    const json = this.text(this.records[this.records.length - 2]);
    return json === 'null' ? null : json;
  }

  /** From the first batch again. */
  rewind(): void {
    this.next = 1;
  }

  /** The next batch, or the `complete` event after the last. Throws `CacheCorrupt` on bad indices. */
  step(): CachedStep {
    const record = this.records[this.next];
    if (record.kind === Kind.Complete) return { kind: 'complete', json: this.text(record) };
    this.next += 1;
    const head = new DataView(new ArrayBuffer(BATCH_HEAD));
    this.file.read(new Uint8Array(head.buffer), { at: record.at });
    const [sequence, processedElements, totalElements, ...counts] = [0, 1, 2, 3, 4, 5, 6].map((k) =>
      head.getUint32(48 + k * 4, true)
    );
    let at = record.at + BATCH_HEAD;
    const take = <T extends Float32Array | Uint32Array>(make: (length: number) => T, length: number): T => {
      const array = make(length);
      this.file.read(bytesOf(array), { at });
      at += array.byteLength;
      return array;
    };
    const positions = take((n) => new Float32Array(n), counts[0]);
    const normals = take((n) => new Float32Array(n), counts[1]);
    const indices = take((n) => new Uint32Array(n), counts[2]);
    const meshTable = take((n) => new Uint32Array(n), counts[3]);
    // The renderer trusts indices to stay inside the batch and records inside their arrays.
    const vertices = positions.length / 3;
    for (const index of indices) if (index >= vertices) throw new CacheCorrupt('an index past the vertices');
    for (let r = 0; r < meshTable.length; r += MESH_RECORD_WORDS) {
      const [, firstVertex, vertexCount, firstIndex, indexCount] = meshTable.subarray(r, r + 5);
      if (firstVertex + vertexCount > vertices || firstIndex + indexCount > indices.length || indexCount % 3 !== 0) {
        throw new CacheCorrupt('a record outside its batch');
      }
    }
    const read = (k: number) => head.getFloat32(24 + k * 4, true);
    return {
      kind: 'batch',
      batch: {
        sequence,
        origin: [head.getFloat64(0, true), head.getFloat64(8, true), head.getFloat64(16, true)],
        box: { min: [read(0), read(1), read(2)], max: [read(3), read(4), read(5)] },
        positions,
        normals,
        indices,
        meshTable
      },
      processedElements,
      totalElements
    };
  }
}

// ---------------------------------------------------------------- the origin's file system --

/** The folder in the origin private file system that holds every cache file. */
const FOLDER = 'gubim-geometry-cache';
const SUFFIX = '.gbc';
/** The most all cache files may take together. Older ones go first. */
export const MAX_CACHE_BYTES = 2 * 1024 ** 3;

export const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');

export async function sha256(data: BufferSource): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest('SHA-256', data));
}

/** How much of each end of a model file its key reads. */
const KEY_SAMPLE_BYTES = 1024 ** 2;

/**
 * A model file's key: the SHA-256 of its length and its two ends, quick to take where the whole
 * file's hash takes a fifth of a second per 77 MB. It only names the cache file: the header holds
 * the whole file's hash, which settles whether the cache is this file's. Two files with one key,
 * such as an edit that keeps the length and both ends, share a name, and the later drops the other.
 */
export async function cacheKey(data: Uint8Array): Promise<Uint8Array> {
  const sample = Math.min(KEY_SAMPLE_BYTES, Math.ceil(data.length / 2));
  const bytes = new Uint8Array(8 + 2 * sample);
  new DataView(bytes.buffer).setFloat64(0, data.length, true);
  bytes.set(data.subarray(0, sample), 8);
  bytes.set(data.subarray(data.length - sample), 8 + sample);
  return sha256(bytes);
}

/** Where a model's cache lives: by its file's key, then by the kernel, so a new kernel misses. */
export const cacheName = (key: Uint8Array, kernel: Uint8Array) => `${hex(key)}-${hex(kernel).slice(0, 16)}${SUFFIX}`;

/** An empty file that says a model was opened once, with no cache yet. */
const SEEN = '.seen';
/** Marks older than this are dropped: a file opened once a month ago is not worth a cache. */
const SEEN_DAYS = 30;

/**
 * Whether this file was opened before with this kernel, marking it if not. Writing a cache costs
 * about as much as a fifth of a load, so only a file opened a second time gets one.
 */
export async function seenBefore(folder: FileSystemDirectoryHandle, cache: string): Promise<boolean> {
  const name = cache.replace(SUFFIX, SEEN);
  try {
    await folder.getFileHandle(name);
    return true;
  } catch {
    await folder.getFileHandle(name, { create: true }).catch(() => undefined);
    return false;
  }
}

/** A `FileSystemSyncAccessHandle`, which only Workers have, so the DOM types leave it out. */
export interface SyncAccessHandle extends CacheFile {
  truncate(size: number): void;
}

/** Opens a file for synchronous access, holding it until closed: a second open fails. */
export function syncHandle(file: FileSystemFileHandle): Promise<SyncAccessHandle> {
  return (file as unknown as { createSyncAccessHandle(): Promise<SyncAccessHandle> }).createSyncAccessHandle();
}

/** The cache folder, or `null` where the browser has no origin private file system. */
export async function cacheFolder(): Promise<FileSystemDirectoryHandle | null> {
  try {
    const root = await navigator.storage.getDirectory();
    return await root.getDirectoryHandle(FOLDER, { create: true });
  } catch {
    return null;
  }
}

interface Entry {
  name: string;
  bytes: number;
  lastModified: number;
}

/**
 * Every cache file the folder holds now, or with `marks` every mark too. Files another Worker is
 * writing may be missing.
 */
async function entries(folder: FileSystemDirectoryHandle, marks = false): Promise<Entry[]> {
  const found: Entry[] = [];
  for await (const [name, handle] of (folder as unknown as AsyncIterable<[string, FileSystemHandle]>)) {
    if (handle.kind !== 'file' || !(name.endsWith(SUFFIX) || (marks && name.endsWith(SEEN)))) continue;
    try {
      const file = await (handle as FileSystemFileHandle).getFile();
      found.push({ name, bytes: file.size, lastModified: file.lastModified });
    } catch {
      // Open in another Worker: it will be counted next time.
    }
  }
  return found;
}

/**
 * Keeps the folder under `MAX_CACHE_BYTES`, dropping the least recently used files first, and
 * drops every file of another kernel, which can never be read again by this one.
 */
export async function evict(folder: FileSystemDirectoryHandle, keep: string, now: number): Promise<void> {
  const kernel = keep.slice(65, 65 + 16);
  const found = (await entries(folder, true)).sort((a, b) => b.lastModified - a.lastModified);
  let total = 0;
  for (const entry of found) {
    const stale = entry.name.slice(65, 65 + 16) !== kernel;
    if (entry.name.endsWith(SEEN)) {
      // The kept file's mark has done its job; old marks are not worth keeping.
      const done = entry.name === keep.replace(SUFFIX, SEEN);
      if (done || stale || now - entry.lastModified > SEEN_DAYS * 86400000) {
        await folder.removeEntry(entry.name).catch(() => undefined);
      }
      continue;
    }
    total += entry.bytes;
    if (entry.name !== keep && (stale || total > MAX_CACHE_BYTES)) {
      total -= entry.bytes;
      await folder.removeEntry(entry.name).catch(() => undefined);
    }
  }
}

/** How many models the cache holds and how much it takes. */
export async function cacheUsage(): Promise<{ models: number; bytes: number }> {
  const folder = await cacheFolder();
  if (!folder) return { models: 0, bytes: 0 };
  const found = await entries(folder);
  return { models: found.length, bytes: found.reduce((sum, entry) => sum + entry.bytes, 0) };
}

/** Removes every cache file and mark not in use. Models open now keep theirs until closed. */
export async function clearCache(): Promise<void> {
  const folder = await cacheFolder();
  if (!folder) return;
  for (const entry of await entries(folder, true)) {
    await folder.removeEntry(entry.name).catch(() => undefined);
  }
}
