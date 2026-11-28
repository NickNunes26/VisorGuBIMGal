import type { FilePort, OpenedFileSource, SaveFileOptions } from '../../domain/files/FilePort';

/** A local file the desktop shell lets the page read (`desktop/src/logic.rs`, `FileToken`). */
export interface DesktopFileToken {
  token: string;
  name: string;
  size: number;
}

/** The WebView2 bridge the shell (E9) listens on: `window.chrome.webview`. */
interface WebViewBridge {
  postMessage(message: unknown): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
}

function bridgeOf(win: unknown): WebViewBridge | null {
  if (!win || typeof win !== 'object') return null;
  const chrome = (win as { chrome?: { webview?: unknown } }).chrome;
  const webview = chrome?.webview as Partial<WebViewBridge> | undefined;
  return webview && typeof webview.postMessage === 'function' && typeof webview.addEventListener === 'function'
    ? (webview as WebViewBridge)
    : null;
}

/** True only inside the VisorGuBIMGal desktop shell (a WebView2 host). */
export function isDesktopHost(win: unknown = typeof window !== 'undefined' ? window : undefined): boolean {
  return bridgeOf(win) !== null;
}

/** Host that serves local files and receives saved files (`desktop/src/logic.rs`, `FILE_HOST`). */
export const DESKTOP_FILE_ORIGIN = 'https://gubim-file.localhost';

/**
 * Largest range one read asks for; the shell refuses more (`desktop/src/logic.rs`, `MAX_RANGE`).
 * WebView2 copies each response several times inside the shell process, so a large model is read
 * range by range and the shell's memory stays bounded (E4). It never crosses the message channel.
 */
export const DESKTOP_READ_CHUNK = 16 * 1024 * 1024;

/** URL of bytes `[start, end)` of a registered file. */
export function desktopFileUrl(token: string, start: number, end: number): string {
  return `${DESKTOP_FILE_ORIGIN}/${encodeURIComponent(token)}?start=${start}&end=${end}`;
}

/** Reads a registered file range by range into one buffer of its size. */
export async function readDesktopFile(
  meta: DesktopFileToken,
  fetchImpl: typeof fetch,
  chunk: number = DESKTOP_READ_CHUNK
): Promise<ArrayBuffer> {
  const out = new Uint8Array(meta.size);
  for (let start = 0; start < meta.size; start += chunk) {
    const end = Math.min(start + chunk, meta.size);
    const response = await fetchImpl(desktopFileUrl(meta.token, start, end));
    if (!response.ok) {
      throw new Error(`No se pudo leer el archivo local (${response.status}).`);
    }
    const part = new Uint8Array(await response.arrayBuffer());
    if (part.length !== end - start) {
      throw new Error('El archivo local cambió mientras se leía.');
    }
    out.set(part, start);
  }
  return out.buffer;
}

/** URL the bytes of a file to save are posted to; the shell then shows its save dialog. */
export function desktopSaveUrl(options: Pick<SaveFileOptions, 'defaultName' | 'filterName' | 'extensions'>): string {
  const q = new URLSearchParams();
  q.set('name', options.defaultName);
  if (options.filterName) q.set('filter', options.filterName);
  if (options.extensions?.length) q.set('ext', options.extensions.join(','));
  // URLSearchParams writes spaces as '+'; the shell decodes %XX only.
  return `${DESKTOP_FILE_ORIGIN}/__save?${q.toString().replace(/\+/g, '%20')}`;
}

function toOpenedFileSource(meta: DesktopFileToken, fetchImpl: typeof fetch): OpenedFileSource {
  return {
    name: meta.name,
    size: meta.size,
    arrayBuffer: () => readDesktopFile(meta, fetchImpl)
  };
}

type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void };

/**
 * Desktop implementation of `FilePort` for the own Win32 + WebView2 shell (E9): native dialogs,
 * files from a double-click or a second instance, and local files streamed by URL.
 */
export class DesktopFilePort implements FilePort {
  readonly usesNativePicker = true;
  private nextId = 1;
  private readonly waiting = new Map<number, Pending>();
  private readonly listeners = new Set<(source: OpenedFileSource) => void>();

  private readonly host: WebViewBridge | null;
  private readonly fetchImpl: typeof fetch;

  constructor(
    host: WebViewBridge | null = bridgeOf(typeof window !== 'undefined' ? window : undefined),
    fetchImpl: typeof fetch = (...args) => fetch(...args)
  ) {
    this.host = host;
    this.fetchImpl = fetchImpl;
    this.host?.addEventListener('message', (event) => this.onMessage(event.data));
  }

  private onMessage(data: unknown): void {
    if (!data || typeof data !== 'object') return;
    const msg = data as { id?: number; ok?: boolean; result?: unknown; error?: string; event?: string; file?: DesktopFileToken };
    if (msg.event === 'open-file' && msg.file) {
      const source = toOpenedFileSource(msg.file, this.fetchImpl);
      for (const listener of this.listeners) listener(source);
      return;
    }
    if (typeof msg.id !== 'number') return;
    const pending = this.waiting.get(msg.id);
    if (!pending) return;
    this.waiting.delete(msg.id);
    if (msg.ok) pending.resolve(msg.result);
    else pending.reject(new Error(msg.error ?? 'Error del anfitrión de escritorio.'));
  }

  private call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    if (!this.host) return Promise.reject(new Error('No hay anfitrión de escritorio.'));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.waiting.set(id, { resolve: resolve as (value: unknown) => void, reject });
      this.host!.postMessage({ id, cmd, args: args ?? null });
    });
  }

  private sources(list: unknown): OpenedFileSource[] {
    return Array.isArray(list) ? (list as DesktopFileToken[]).map((m) => toOpenedFileSource(m, this.fetchImpl)) : [];
  }

  async pickIfcFiles(): Promise<OpenedFileSource[]> {
    return this.sources(await this.call<DesktopFileToken[]>('pickIfcFiles'));
  }

  async saveFile(options: SaveFileOptions): Promise<boolean> {
    const bytes = typeof options.content === 'string' ? new TextEncoder().encode(options.content) : options.content;
    // A Blob with no type keeps this a simple request: no CORS preflight.
    const response = await this.fetchImpl(desktopSaveUrl(options), {
      method: 'POST',
      body: new Blob([bytes as Uint8Array<ArrayBuffer>])
    });
    const text = await response.text();
    if (!response.ok) throw new Error(text || `No se pudo guardar el archivo (${response.status}).`);
    return text.trim() === '1';
  }

  async openPath(filePath: string): Promise<OpenedFileSource | null> {
    const meta = await this.call<DesktopFileToken | null>('registerIfcPath', { path: filePath });
    return meta ? toOpenedFileSource(meta, this.fetchImpl) : null;
  }

  onExternalOpen(listener: (source: OpenedFileSource) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  async consumeInitialFiles(): Promise<OpenedFileSource[]> {
    return this.sources(await this.call<DesktopFileToken[]>('takePendingFiles'));
  }
}
