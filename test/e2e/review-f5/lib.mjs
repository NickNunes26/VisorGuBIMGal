// Helpers for the F5 review: own ZIP reader, DXF editing, Node kernel readback, browser driving.
import { readFileSync, readdirSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { inflateRawSync } from 'node:zlib';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const cdpMod = await import(pathToFileURL(path.join(REPO, 'test/e2e/cdp.mjs')).href);
export const { launch, connect, sleep, applyMode } = cdpMod;

// ---------------------------------------------------------------- Node kernel (read back only)
const wasm = await import(pathToFileURL(path.join(REPO, 'src/infrastructure/kernel/pkg/wasm_api.js')).href);
await wasm.default({ module_or_path: readFileSync(path.join(REPO, 'src/infrastructure/kernel/pkg/wasm_api_bg.wasm')) });
export { wasm };

export function nodeModel(text) {
  return new wasm.Model(typeof text === 'string' ? new TextEncoder().encode(text) : text);
}
export function nodeBounds(model, id) {
  const v = model.elementMesh(id);
  if (!v) return null;
  try {
    const p = v.takePositions();
    if (!p || p.length === 0) return null;
    const lo = [Infinity, Infinity, Infinity];
    const hi = [-Infinity, -Infinity, -Infinity];
    let nan = 0;
    for (let i = 0; i < p.length; i += 3) {
      for (let k = 0; k < 3; k++) {
        if (!Number.isFinite(p[i + k])) nan++;
        lo[k] = Math.min(lo[k], p[i + k]);
        hi[k] = Math.max(hi[k], p[i + k]);
      }
    }
    return { lo, hi, n: p.length / 3, nan };
  } finally {
    v.free();
  }
}
export const centre = (b) => (b ? [0, 1, 2].map((k) => (b.lo[k] + b.hi[k]) / 2) : null);
export const near = (v, w, tol = 1e-3) => !!v && !!w && v.length === w.length && v.every((x, i) => Math.abs(x - w[i]) <= tol);
export const sub = (a, b) => a.map((x, i) => x - b[i]);
export const r6 = (v) => (v ? v.map((x) => +x.toFixed(6)) : v);

// ---------------------------------------------------------------- ZIP (own reader)
export function readZip(buf) {
  const out = new Map();
  let eocd = -1;
  for (let i = buf.length - 22; i >= 0; i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error('no EOCD');
  const n = buf.readUInt16LE(eocd + 10);
  let at = buf.readUInt32LE(eocd + 16);
  for (let i = 0; i < n; i++) {
    const method = buf.readUInt16LE(at + 10);
    const csize = buf.readUInt32LE(at + 20);
    const nlen = buf.readUInt16LE(at + 28);
    const xlen = buf.readUInt16LE(at + 30);
    const clen = buf.readUInt16LE(at + 32);
    const loff = buf.readUInt32LE(at + 42);
    const name = buf.subarray(at + 46, at + 46 + nlen).toString('utf8');
    const lnl = buf.readUInt16LE(loff + 26);
    const lxl = buf.readUInt16LE(loff + 28);
    const data = buf.subarray(loff + 30 + lnl + lxl, loff + 30 + lnl + lxl + csize);
    out.set(name, method === 0 ? Buffer.from(data) : inflateRawSync(data));
    at += 46 + nlen + xlen + clen;
  }
  return out;
}

// ---------------------------------------------------------------- DXF helpers (own)
export const dxfLines = (t) => t.split(/\r?\n/);
/** Entities of the ENTITIES section: {type, start, end} as line indices, [start, end). */
export function entities(lines) {
  let i = 0;
  while (i + 1 < lines.length && !(lines[i].trim() === '2' && lines[i + 1].trim() === 'ENTITIES')) i++;
  i += 2;
  const ents = [];
  const secStart = i;
  while (i + 1 < lines.length) {
    if (lines[i].trim() === '0') {
      const type = lines[i + 1].trim();
      if (type === 'ENDSEC') break;
      let j = i + 2;
      while (j + 1 < lines.length && lines[j].trim() !== '0') j += 2;
      const pairs = [];
      for (let p = i + 2; p < j; p += 2) pairs.push([lines[p].trim(), lines[p + 1]]);
      ents.push({ type, start: i, end: j, pairs });
      i = j;
    } else i += 2;
  }
  return { ents, secStart, secEnd: i };
}
const val = (ent, code) => ent.pairs.find((p) => p[0] === code)?.[1]?.trim();
/** Groups POLYLINE+VERTEX*+SEQEND into one unit. */
export function units(ents) {
  const out = [];
  for (let i = 0; i < ents.length; i++) {
    if (ents[i].type === 'POLYLINE') {
      let j = i;
      while (j < ents.length && ents[j].type !== 'SEQEND') j++;
      out.push({ type: 'POLYLINE', parts: ents.slice(i, j + 1), layer: val(ents[i], '8') });
      i = j;
    } else out.push({ type: ents[i].type, parts: [ents[i]], layer: val(ents[i], '8') });
  }
  return out;
}
export function rebuild(lines, us, secStart, secEnd) {
  const body = us.flatMap((u) => u.parts.flatMap((p) => lines.slice(p.start, p.end)));
  return [...lines.slice(0, secStart), ...body, ...lines.slice(secEnd)];
}
export function inserts(text) {
  const lines = dxfLines(text);
  return entities(lines).ents
    .filter((e) => e.type === 'INSERT')
    .map((e) => ({ block: val(e, '2'), x: Number(val(e, '10')), y: Number(val(e, '20')), layer: val(e, '8') }));
}
export function metaTexts(text) {
  const lines = dxfLines(text);
  return entities(lines).ents.filter((e) => e.type === 'TEXT' && /^GUBIM_/.test(val(e, '1') ?? '')).map((e) => val(e, '1'));
}
/** Moves INSERTs whose block starts with `GUBIM_A_<id>_` (or matches pred) by (dx,dy). nth: only that occurrence. */
export function moveInsert(text, pred, dx, dy, nth = null) {
  const lines = dxfLines(text);
  const { ents } = entities(lines);
  let seen = 0;
  let moved = 0;
  for (const e of ents) {
    if (e.type !== 'INSERT') continue;
    const block = val(e, '2');
    if (!pred(block)) continue;
    if (nth !== null && seen++ !== nth) continue;
    for (let p = e.start + 2; p < e.end; p += 2) {
      const c = lines[p].trim();
      if (c === '10') lines[p + 1] = (Number(lines[p + 1]) + dx).toFixed(6);
      if (c === '20') lines[p + 1] = (Number(lines[p + 1]) + dy).toFixed(6);
    }
    moved++;
  }
  return { text: lines.join('\r\n'), moved };
}
export const blockOf = (id) => (b) => typeof b === 'string' && b.startsWith(`GUBIM_A_${id}_`);
/** Vertices of the POLYLINE on layer EDIT_A_<id>. */
export function editPolyline(text, id) {
  const us = units(entities(dxfLines(text)).ents);
  const u = us.find((x) => x.type === 'POLYLINE' && new RegExp(`^EDIT_A_${id}(\\b|_|$)`).test(x.layer ?? ''));
  if (!u) return null;
  return u.parts.filter((p) => p.type === 'VERTEX').map((p) => [Number(val(p, '10')), Number(val(p, '20'))]);
}
/** Replaces the EDIT_A_<id> POLYLINE by one through `pts` (written as R12 POLYLINE/VERTEX). */
export function setEditPolyline(text, id, pts, { lw = false, fmt = (v) => String(v) } = {}) {
  const lines = dxfLines(text);
  const { ents, secStart, secEnd } = entities(lines);
  const us = units(ents);
  const idx = us.findIndex((x) => x.type === 'POLYLINE' && new RegExp(`^EDIT_A_${id}(\\b|_|$)`).test(x.layer ?? ''));
  if (idx < 0) throw new Error('no EDIT polyline ' + id);
  const layer = us[idx].layer;
  let rep;
  if (lw) {
    rep = ['0', 'LWPOLYLINE', '8', layer, '90', String(pts.length), '70', '0'];
    for (const [x, y] of pts) rep.push('10', fmt(x), '20', fmt(y));
  } else {
    rep = ['0', 'POLYLINE', '8', layer, '66', '1', '10', '0.0', '20', '0.0', '30', '0.0'];
    for (const [x, y] of pts) rep.push('0', 'VERTEX', '8', layer, '10', fmt(x), '20', fmt(y), '30', '0.0');
    rep.push('0', 'SEQEND', '8', layer);
  }
  const body = [];
  us.forEach((u, i) => {
    if (i === idx) body.push(...rep);
    else body.push(...u.parts.flatMap((p) => lines.slice(p.start, p.end)));
  });
  return [...lines.slice(0, secStart), ...body, ...lines.slice(secEnd)].join('\r\n');
}
/** Kerb (topmost LINE not on app layers): returns its x mid and top y in sheet units. */
export function kerbTop(text) {
  const ents = entities(dxfLines(text)).ents.filter((e) => e.type === 'LINE');
  let best = null;
  for (const e of ents) {
    const layer = val(e, '8') ?? '';
    if (/^(ALIGNMENT_AXIS|CLASH_|GUBIM_|ANNOTATION|EDIT_A_|SHEET_)/.test(layer)) continue;
    const y1 = Number(val(e, '20'));
    const y2 = Number(val(e, '21'));
    if (Math.abs(y1 - y2) > 1e-6) continue;
    const xa = Number(val(e, '10'));
    const xb = Number(val(e, '11'));
    if (!best || y1 > best.y + 1e-6) best = { y: y1, lo: Math.min(xa, xb), hi: Math.max(xa, xb), layer };
    else if (Math.abs(y1 - best.y) <= 1e-6) { best.lo = Math.min(best.lo, xa, xb); best.hi = Math.max(best.hi, xa, xb); }
  }
  if (!best) return null;
  return { y: best.y, x: (best.lo + best.hi) / 2, width: best.hi - best.lo, layer: best.layer };
}
/** Rewrites every GlobalId of `id` (full in GUBIM_ORIG, truncated in block names) to `toGuid`. */
export function retargetGuid(text, id, fromGuid, toGuid) {
  const t = text.split(fromGuid).join(toGuid);
  const pre = `GUBIM_A_${id}_`;
  return t.replace(new RegExp(`GUBIM_A_${id}_[0-9A-Za-z_$]+`, 'g'), (m) => pre + toGuid.slice(0, m.length - pre.length));
}
export function maxLineLen(text) {
  return Math.max(...dxfLines(text).map((l) => l.length));
}

// ---------------------------------------------------------------- browser session
export async function session({ mode, base, outDir, port, name }) {
  const dl = path.join(outDir, `dl-${name}`);
  mkdirSync(dl, { recursive: true });
  const { proc, wsUrl } = await launch({ port, profileDir: path.join(outDir, '..', `profile-${name}-${mode}`) });
  const cdp = await connect(wsUrl);
  const errors = [];
  const consoleLog = [];
  cdp.on('Runtime.consoleAPICalled', (p) => {
    const msg = p.args.map((a) => a.value ?? a.description).join(' ');
    consoleLog.push(`[${p.type}] ${msg}`);
    if (p.type === 'error') errors.push(msg);
  });
  cdp.on('Runtime.exceptionThrown', (p) => errors.push('EXC ' + (p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text)));
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  await applyMode(cdp.send, mode);
  try {
    await cdp.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: dl });
  } catch {
    await cdp.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: dl });
  }
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
  let closed = false;
  const ev = (expr) => {
    if (closed) return Promise.reject(new Error('session closed'));
    return Promise.race([cdp.evaluate(expr), new Promise((_, rej) => setTimeout(() => rej(new Error('evaluate timeout')), 150000))]);
  };
  const waitFor = async (expr, timeout = 60000) => {
    const t0 = Date.now();
    let last;
    while (Date.now() - t0 < timeout) {
      try {
        const v = await ev(expr);
        if (v) return v;
      } catch (e) {
        last = e;
      }
      await sleep(150);
    }
    throw new Error('timeout ' + expr.slice(0, 200) + (last ? ' / ' + last.message.slice(0, 200) : ''));
  };
  const S = {
    cdp,
    ev,
    waitFor,
    errors,
    consoleLog,
    dl,
    async open() {
      await cdp.send('Page.navigate', { url: base });
      await waitFor(`!!window.__gubim_dev__ && !!window.__gubim_kernel__ && !!document.querySelector('input[type=file][accept=".ifc"]')`);
    },
    async setFile(selector, file) {
      const { root } = await cdp.send('DOM.getDocument', { depth: -1 });
      const { nodeId } = await cdp.send('DOM.querySelector', { nodeId: root.nodeId, selector });
      if (!nodeId) throw new Error('no input ' + selector);
      await cdp.send('DOM.setFileInputFiles', { nodeId, files: [path.resolve(file)] });
    },
    async loadIfc(file, expectCount) {
      await S.setFile('input[type=file][accept=".ifc"]', file);
      await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === ${expectCount}`, 90000);
      await sleep(400);
    },
    modelIds: () => ev(`[...window.__gubim_dev__.activeRenderer.models.keys()]`),
    setSelect: (selector, value) =>
      ev(`(() => { const s = document.querySelector(${JSON.stringify(selector)}); if (!s) return false; Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, ${JSON.stringify(value)}); s.dispatchEvent(new Event('change', { bubbles: true })); return s.value === ${JSON.stringify(value)}; })()`),
    clickText: (text) =>
      ev(`(() => { const b = [...document.querySelectorAll('button')].find((x) => x.textContent.trim() === ${JSON.stringify(text)}); if (!b) return false; b.click(); return true; })()`),
    hasButton: (text) => ev(`[...document.querySelectorAll('button')].some((x) => x.textContent.trim() === ${JSON.stringify(text)})`),
    cards: () => ev(`[...document.querySelectorAll('.clash-card')].map((c) => c.textContent.replace(/\\s+/g, ' ').trim())`),
    notices: () =>
      ev(`[...document.querySelectorAll('.clash-alignment-box .clash-notice, .clash-alignment-box .clash-error')].map((n) => n.className.replace('clash-', '') + ': ' + n.textContent.trim())`),
    async openClashTab() {
      await ev(`document.querySelector('.tab-clash').click(), true`);
      await waitFor(`[...document.querySelector('.clash-class-a').options].some((o) => o.value === 'IfcReinforcingBar')`);
    },
    async configure({ modelA, modelB, classA, classB }) {
      if (modelA !== undefined && !(await S.setSelect('.clash-model-a', modelA))) throw new Error('model-a ' + modelA);
      if (modelB !== undefined && !(await S.setSelect('.clash-model-b', modelB))) throw new Error('model-b ' + modelB);
      if (classA !== undefined && !(await S.setSelect('.clash-class-a', classA))) throw new Error('class-a ' + classA);
      if (classB !== undefined && !(await S.setSelect('.clash-class-b', classB))) throw new Error('class-b ' + classB);
    },
    async runClash() {
      await ev(`document.querySelector('.clash-run-btn').click(), true`);
      await sleep(200);
      await waitFor(`!document.querySelector('.clash-run-btn').disabled && !!document.querySelector('.clash-results')`, 90000);
      await sleep(300);
      return S.cards();
    },
    async loadLandXml(file) {
      await S.setFile('input[type=file][accept=".xml,.landxml"]', file);
      await waitFor(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Exportar ZIP DXF')`, 15000);
    },
    async waitDownload(pred, timeout = 60000) {
      const before = new Map(readdirSync(dl).map((n) => [n, statSync(path.join(dl, n)).mtimeMs]));
      return async () => {
        const t0 = Date.now();
        while (Date.now() - t0 < timeout) {
          const f = readdirSync(dl).find((n) => pred(n) && !n.endsWith('.crdownload') && (!before.has(n) || statSync(path.join(dl, n)).mtimeMs !== before.get(n)));
          if (f) {
            await sleep(600);
            return path.join(dl, f);
          }
          await sleep(200);
        }
        throw new Error('download timeout');
      };
    },
    async exportZip() {
      const wait = await S.waitDownload((n) => n.endsWith('.zip'));
      if (!(await S.clickText('Exportar ZIP DXF'))) throw new Error('no export button');
      const p = await wait();
      return { path: p, zip: readZip(readFileSync(p)) };
    },
    async reimport(file) {
      await ev(`(() => {
        const btn = document.querySelector('[data-testid="clash-reimport-dxf-btn"]');
        window.__rt = { busy: false, done: false };
        const mo = new MutationObserver(() => {
          if (btn.disabled) window.__rt.busy = true;
          else if (window.__rt.busy) { window.__rt.done = true; mo.disconnect(); }
        });
        mo.observe(btn, { attributes: true, attributeFilter: ['disabled'] });
        window.__rtmo = mo;
        return true;
      })()`);
      await S.setFile('input[type=file][accept=".dxf"]', file);
      await waitFor(`window.__rt.done`, 120000);
      await sleep(500);
      return S.notices();
    },
    /** The F7 warning: another section edited an element after this DXF was exported. */
    conflictShown: () => ev(`!!document.querySelector('[data-testid="clash-roundtrip-conflict"]')`),
    /** Answers the F7 warning (`replace`, `add`, `skip` or `cancel`) and waits for the import. */
    async resolveConflict(choice) {
      await ev(`(() => {
        const btn = document.querySelector('[data-testid="clash-reimport-dxf-btn"]');
        window.__rt = { busy: false, done: false };
        const mo = new MutationObserver(() => {
          if (btn.disabled) window.__rt.busy = true;
          else if (window.__rt.busy) { window.__rt.done = true; mo.disconnect(); }
        });
        mo.observe(btn, { attributes: true, attributeFilter: ['disabled'] });
        document.querySelector('[data-testid="clash-conflict-${choice}"]').click();
        return true;
      })()`);
      if (choice !== 'cancel') await waitFor(`window.__rt.done`, 120000);
      await sleep(500);
      return S.notices();
    },
    async downloadPatched() {
      const wait = await S.waitDownload((n) => n.endsWith('.ifc'));
      if (!(await S.clickText('Descargar IFC parcheado'))) throw new Error('no patched-IFC button');
      return wait();
    },
    async downloadCsv() {
      const wait = await S.waitDownload((n) => n.endsWith('.csv'));
      if (!(await S.clickText('Registro CSV'))) throw new Error('no CSV button');
      return wait();
    },
    liveBounds: (modelId, expressId) =>
      ev(`(async () => {
        const m = await window.__gubim_kernel__.elementMesh(${JSON.stringify(modelId)}, ${expressId});
        if (!m || !m.positions || m.positions.length === 0) return null;
        const lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
        for (let i = 0; i < m.positions.length; i += 3) for (let k = 0; k < 3; k++) { lo[k] = Math.min(lo[k], m.positions[i + k]); hi[k] = Math.max(hi[k], m.positions[i + k]); }
        return { lo, hi, n: m.positions.length / 3 };
      })()`),
    async shot(file) {
      if (closed) return;
      await ev(`new Promise((r) => { let i = 0; const f = () => (++i >= 4 ? r(true) : requestAnimationFrame(f)); requestAnimationFrame(f); })`);
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
      writeFileSync(file, Buffer.from(data, 'base64'));
    },
    async close() {
      if (closed) return;
      closed = true;
      try {
        cdp.close();
      } catch {}
      try {
        proc.kill();
      } catch {}
      await sleep(500);
    }
  };
  return S;
}
