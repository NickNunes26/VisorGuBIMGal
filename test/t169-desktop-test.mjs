import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import {
  GIS_OFFLINE_ERROR_THRESHOLD,
  initialGisOnlineState,
  reduceGisOnlineState
} from '../src/ui/viewer/gisOnline.ts';
import { WebFilePort } from '../src/infrastructure/files/WebFilePort.ts';
import {
  DESKTOP_READ_CHUNK,
  DesktopFilePort,
  desktopFileUrl,
  desktopSaveUrl,
  isDesktopHost,
  readDesktopFile
} from '../src/infrastructure/files/DesktopFilePort.ts';

let passed = 0;
async function test(name, fn) {
  await fn();
  console.log(`PASS ${name}`);
  passed++;
}

await test('gisOnline marks offline immediately when navigator.onLine is false', () => {
  const s0 = initialGisOnlineState(false);
  assert.equal(s0.status, 'offline');
  const s1 = reduceGisOnlineState(s0, 'browser-online');
  assert.equal(s1.status, 'online');
  const s2 = reduceGisOnlineState(s1, 'browser-offline');
  assert.equal(s2.status, 'offline');
});

await test('gisOnline transitions offline after threshold consecutive tile errors and recovers on load or retry', () => {
  let state = initialGisOnlineState(true);
  assert.equal(state.status, 'online');
  for (let i = 1; i < GIS_OFFLINE_ERROR_THRESHOLD; i++) {
    state = reduceGisOnlineState(state, 'tile-error');
    assert.equal(state.status, 'online', 'single transient error should not flip offline');
  }
  state = reduceGisOnlineState(state, 'tile-error');
  assert.equal(state.status, 'offline');

  // Retrying resets status to online and zeroes consecutive errors.
  const retried = reduceGisOnlineState(state, 'retry');
  assert.equal(retried.status, 'online');
  assert.equal(retried.consecutiveTileErrors, 0);

  // Tile load marks online and records that a tile loaded.
  const loaded = reduceGisOnlineState(state, 'tile-load');
  assert.equal(loaded.status, 'online');
  assert.equal(loaded.anyTileLoaded, true);
});

await test('WebFilePort implements FilePort cleanly for web host', async () => {
  const port = new WebFilePort();
  assert.equal(port.usesNativePicker, false);
  assert.deepEqual(await port.pickIfcFiles(), []);
  assert.equal(await port.openPath('C:\\model.ifc'), null);
  assert.deepEqual(await port.consumeInitialFiles(), []);
  const unsub = port.onExternalOpen(() => {});
  assert.equal(typeof unsub, 'function');
  unsub();
});

await test('no desktop host branches leak outside composition.ts and DesktopFilePort.ts (E2, E9)', () => {
  const root = path.resolve('src');
  const allowed = new Set([
    path.join(root, 'infrastructure', 'composition.ts'),
    path.join(root, 'infrastructure', 'files', 'DesktopFilePort.ts')
  ]);
  const violations = [];
  function walk(dir) {
    for (const entry of readdirSync(dir)) {
      const full = path.join(dir, entry);
      if (statSync(full).isDirectory()) {
        walk(full);
      } else if (full.endsWith('.ts') || full.endsWith('.tsx')) {
        if (allowed.has(full)) continue;
        const text = readFileSync(full, 'utf8');
        if (/__TAURI|@tauri-apps|TauriFilePort|chrome\.webview|isDesktopHost|DesktopFilePort/.test(text)) {
          violations.push(path.relative(root, full));
        }
      }
    }
  }
  walk(root);
  assert.deepEqual(violations, [], `desktop references found outside composition: ${violations.join(', ')}`);
});

await test('Tauri is gone: no src-tauri, no @tauri-apps packages (E9)', () => {
  assert.throws(() => statSync(path.resolve('src-tauri')), /ENOENT/);
  const pkg = JSON.parse(readFileSync(path.resolve('package.json'), 'utf8'));
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  assert.deepEqual(Object.keys(deps).filter((d) => d.startsWith('@tauri-apps')), []);
  assert.doesNotMatch(JSON.stringify(pkg.scripts), /tauri/);
  // The shell's resolved crates: no Tauri, and none of the MPL-2.0 crates it pulled in (E9).
  const lock = readFileSync(path.resolve('desktop', 'Cargo.lock'), 'utf8');
  const crates = [...lock.matchAll(/^name = "([^"]+)"$/gm)].map((m) => m[1]);
  assert.ok(crates.includes('webview2-com') && crates.includes('windows'), 'lock must list the shell crates');
  const banned = ['option-ext', 'cssparser', 'cssparser-macros', 'selectors', 'dtoa-short', 'servo_arc'];
  assert.deepEqual(crates.filter((c) => c.startsWith('tauri') || banned.includes(c)), []);
});

await test('vite.config.ts preserves /VisorGuBIMGal/ for web production and uses ./ for desktop (E3)', () => {
  const cfg = readFileSync(path.resolve('vite.config.ts'), 'utf8');
  assert.match(cfg, /GUBIM_DESKTOP/);
  assert.match(cfg, /'\/VisorGuBIMGal\/'/);
  assert.match(cfg, /'\.\/'/);
});

await test('the desktop build carries a strict Content Security Policy (E9)', () => {
  const csp = readFileSync(path.resolve('desktop', 'content-security-policy.txt'), 'utf8').trim();
  assert.ok(csp.includes("default-src 'self'"), 'CSP must restrict default-src to self');
  assert.ok(csp.includes("script-src 'self' 'wasm-unsafe-eval'"), 'CSP must allow only self scripts + wasm');
  assert.ok(csp.includes("object-src 'none'"), 'CSP must disallow plugins/objects');
  assert.match(csp, /connect-src [^;]*https:\/\/gubim-file\.localhost/, 'CSP must allow the local file host');
  assert.ok(!csp.includes("'unsafe-eval'"), 'CSP must not allow unsafe-eval (only wasm-unsafe-eval)');
  assert.ok(!csp.includes('frame-ancestors'), 'frame-ancestors is ignored in a <meta> CSP');
  const build = readFileSync(path.resolve('scripts', 'build-desktop.mjs'), 'utf8');
  assert.match(build, /content-security-policy\.txt/);
  assert.match(build, /http-equiv="Content-Security-Policy"/);
});

await test('index.css has balanced braces so .gis-map-stage is never swallowed by a CSS syntax error', () => {
  const css = readFileSync(path.resolve('src', 'ui', 'styles', 'index.css'), 'utf8');
  // Strip comments and check brace balance, and ensure .gis-map-stage starts at brace depth 0.
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  let depth = 0;
  const stageIdx = stripped.indexOf('.gis-map-stage');
  assert.ok(stageIdx > 0, '.gis-map-stage selector must exist');
  for (let i = 0; i < stripped.length; i++) {
    if (i === stageIdx) {
      assert.equal(depth, 0, `.gis-map-stage must be at top-level CSS depth (got ${depth})`);
    }
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}') depth--;
    assert.ok(depth >= 0, `unexpected closing brace at offset ${i}`);
  }
  assert.equal(depth, 0, `unbalanced CSS braces: final depth ${depth}`);
});

await test('Header IFC file input supports multi-file upload', () => {
  const header = readFileSync(path.resolve('src', 'ui', 'components', 'Header.tsx'), 'utf8');
  assert.match(header, /type="file"[\s\S]*?accept="\.ifc"[\s\S]*?multiple/);
});

await test('desktop opens several IFCs at once: native multi-select dialog and HTML5 drop (E2, E9)', () => {
  // The desktop host bypasses `<input multiple>` with its native dialog, so that dialog itself
  // must be multi-select and its result must reach the multi-file loader.
  const host = readFileSync(path.resolve('desktop', 'src', 'host.rs'), 'utf8');
  assert.match(host, /"pickIfcFiles" =>/);
  assert.match(host, /FOS_ALLOWMULTISELECT/);
  // HTML5 drops reach BimViewer (several .ifc files) as long as the shell leaves external drops on.
  assert.doesNotMatch(host, /SetAllowExternalDrop\(false\)/);

  const port = readFileSync(path.resolve('src', 'infrastructure', 'files', 'DesktopFilePort.ts'), 'utf8');
  assert.match(port, /this\.call<DesktopFileToken\[\]>\('pickIfcFiles'\)/);

  const header = readFileSync(path.resolve('src', 'ui', 'components', 'Header.tsx'), 'utf8');
  const nativeBranch = /if \(filePort\.usesNativePicker\) \{([\s\S]*?)\r?\n    \}\r?\n/.exec(header);
  assert.ok(nativeBranch, 'Header must have a native-picker branch');
  assert.match(nativeBranch[1], /pickIfcFiles\(\)/);
  assert.match(nativeBranch[1], /onOpenFiles\(picked\)/);
});

/** A stand-in for window.chrome.webview and for the shell behind it. */
function fakeShell(handler) {
  const listeners = [];
  const sent = [];
  const host = {
    postMessage(msg) {
      sent.push(msg);
      const reply = handler(msg);
      if (reply) queueMicrotask(() => listeners.forEach((l) => l({ data: { id: msg.id, ...reply } })));
    },
    addEventListener(type, l) {
      if (type === 'message') listeners.push(l);
    },
    emit(data) {
      listeners.forEach((l) => l({ data }));
    }
  };
  return { host, sent };
}

/** A fetch that serves `bytes` as the shell does: ranges only, at most DESKTOP_READ_CHUNK. */
function fakeFileFetch(bytes, calls) {
  return async (url) => {
    calls.push(url);
    const m = /\?start=(\d+)&end=(\d+)$/.exec(url);
    if (!m) return new Response('Bad range', { status: 400 });
    const [start, end] = [Number(m[1]), Number(m[2])];
    if (end - start > DESKTOP_READ_CHUNK || end > bytes.length) return new Response('Bad range', { status: 416 });
    return new Response(bytes.slice(start, end));
  };
}

await test('isDesktopHost needs a WebView2 bridge with postMessage and addEventListener (E9)', () => {
  assert.equal(isDesktopHost(undefined), false);
  assert.equal(isDesktopHost({}), false);
  assert.equal(isDesktopHost({ chrome: {} }), false);
  assert.equal(isDesktopHost({ chrome: { webview: { postMessage() {} } } }), false);
  assert.equal(isDesktopHost({ chrome: { webview: { postMessage() {}, addEventListener() {} } } }), true);
});

await test('DesktopFilePort reads a file in bounded ranges and reassembles it exactly (E4, E9)', async () => {
  const size = 2 * DESKTOP_READ_CHUNK + 12345;
  const bytes = new Uint8Array(size);
  for (let i = 0; i < size; i++) bytes[i] = (i * 31 + (i >> 16)) & 0xff;
  const calls = [];
  const { host, sent } = fakeShell((msg) =>
    msg.cmd === 'takePendingFiles' ? { ok: true, result: [{ token: 'f-7', name: 'big.ifc', size }] } : null
  );
  const port = new DesktopFilePort(host, fakeFileFetch(bytes, calls));
  const [file] = await port.consumeInitialFiles();
  assert.deepEqual(sent, [{ id: 1, cmd: 'takePendingFiles', args: null }]);
  assert.equal(file.name, 'big.ifc');
  assert.equal(file.size, size);
  const read = new Uint8Array(await file.arrayBuffer());
  assert.equal(read.length, size);
  assert.ok(read.every((b, i) => b === bytes[i]), 'bytes must match');
  assert.deepEqual(calls, [
    desktopFileUrl('f-7', 0, DESKTOP_READ_CHUNK),
    desktopFileUrl('f-7', DESKTOP_READ_CHUNK, 2 * DESKTOP_READ_CHUNK),
    desktopFileUrl('f-7', 2 * DESKTOP_READ_CHUNK, size)
  ]);
  // An empty file needs no request; a file that shrank under us is an error, not a short read.
  const before = calls.length;
  assert.equal((await readDesktopFile({ token: 'f-0', name: 'e.ifc', size: 0 }, fakeFileFetch(bytes, calls))).byteLength, 0);
  assert.equal(calls.length, before);
  await assert.rejects(readDesktopFile({ token: 'f-8', name: 'x.ifc', size: 10 }, async () => new Response(new Uint8Array(4))), /cambió/);
  await assert.rejects(readDesktopFile({ token: 'f-9', name: 'x.ifc', size: 10 }, async () => new Response('x', { status: 404 })), /404/);
});

await test('DesktopFilePort: open-file events, picking, error replies (E1, E9)', async () => {
  const { host, sent } = fakeShell((msg) => {
    if (msg.cmd === 'pickIfcFiles') return { ok: true, result: [{ token: 'f-1', name: 'a.ifc', size: 1 }, { token: 'f-2', name: 'b.ifc', size: 2 }] };
    if (msg.cmd === 'registerIfcPath') return { ok: false, error: 'no' };
    return null;
  });
  const port = new DesktopFilePort(host, async () => new Response(new Uint8Array(0)));
  assert.equal(port.usesNativePicker, true);
  const picked = await port.pickIfcFiles();
  assert.deepEqual(picked.map((p) => p.name), ['a.ifc', 'b.ifc']);
  await assert.rejects(port.openPath('C:\\x.ifc'), /no/);
  assert.deepEqual(sent.map((m) => [m.id, m.cmd]), [[1, 'pickIfcFiles'], [2, 'registerIfcPath']]);
  assert.deepEqual(sent[1].args, { path: 'C:\\x.ifc' });

  const seen = [];
  const stop = port.onExternalOpen((src) => seen.push(src.name));
  host.emit({ event: 'open-file', file: { token: 'f-3', name: 'segundo.ifc', size: 9 } });
  host.emit({ id: 999, ok: true, result: [] }); // a reply nobody waits for is ignored
  stop();
  host.emit({ event: 'open-file', file: { token: 'f-4', name: 'tercero.ifc', size: 9 } });
  assert.deepEqual(seen, ['segundo.ifc']);
});

await test('DesktopFilePort saves by posting the bytes; the shell answers 1 saved, 0 cancelled (E2, E9)', async () => {
  assert.equal(
    desktopSaveUrl({ defaultName: 'plano planta ñ.dxf', filterName: 'Plano DXF', extensions: ['dxf', 'svg'] }),
    'https://gubim-file.localhost/__save?name=plano%20planta%20%C3%B1.dxf&filter=Plano%20DXF&ext=dxf%2Csvg'
  );
  const posted = [];
  let answer = '1';
  let status = 200;
  const port = new DesktopFilePort(fakeShell(() => null).host, async (url, init) => {
    posted.push({ url, method: init.method, type: init.body.type, bytes: new Uint8Array(await init.body.arrayBuffer()) });
    return new Response(answer, { status });
  });
  assert.equal(await port.saveFile({ defaultName: 'a.csv', content: 'x;y', mimeType: 'text/csv', extensions: ['csv'] }), true);
  assert.equal(posted[0].method, 'POST');
  assert.equal(posted[0].type, '', 'an untyped Blob keeps the request simple (no preflight)');
  assert.deepEqual([...posted[0].bytes], [...new TextEncoder().encode('x;y')]);
  answer = '0';
  assert.equal(await port.saveFile({ defaultName: 'b.glb', content: new Uint8Array([1, 2, 3]), mimeType: 'model/gltf-binary' }), false);
  assert.deepEqual([...posted[1].bytes], [1, 2, 3]);
  answer = 'No se pudo escribir';
  status = 500;
  await assert.rejects(port.saveFile({ defaultName: 'c.txt', content: 'z', mimeType: 'text/plain' }), /No se pudo escribir/);
});

console.log(`\nt169-desktop-test: ${passed}/${passed} pass`);
