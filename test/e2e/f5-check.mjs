// Browser check for F1-F5 (docs/12): LandXML clash sections, DXF ZIP export and DXF -> IFC round trip,
// through the real UI on one backend. Regression suite for the 2026-11-24 review findings.
//
// Model (generated here, IFC4, site at (1000, 2000, 0)): deck slab, one tendon along X at z=0.5 and
// four stirrups along Y: x=5 (plain), x=10 (parent tilted: local Z = world X), x=15 (parent
// rotated 90 deg about Z), x=20 (z=0.8, clear). A decoy copy with other GlobalIds but the same
// expressIds is opened first.
// Checks:
//   F1 3 hard clashes (x=5, 10, 15), LandXML binds, ZIP holds 3 sections at PK 0+105/110/115.
//   F2 every DXF value fits the R12 limit of 255 characters; GUBIM_ORIG carries the full GlobalId.
//   F3 moving the x=10 INSERT +0.30 m and re-importing the raw DXF applies 1 edit, 2 clashes left.
//   F4 re-importing the same DXF applies nothing ("ya aplicado") and changes no geometry.
//   F5 a DXF from an earlier session (unknown model id), re-imported while the decoy is the
//      primary model, skips the decoy (GlobalId differs) and moves the bridge's x=15 bar.
//   F6 1 clash left; the decoy is untouched.
//   F7 the downloaded IFC: only the two moved bars' records changed, GlobalIds identical, both bars
//      exactly +0.30 m in world Z (geometry read back by the kernel in Node), orientation kept.
//   F8 no console errors.
// Usage: node f5-check.mjs <gpu|gl2|none> <baseUrl> <outDir>
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { launch, connect, sleep, applyMode } from './cdp.mjs';

registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (err) {
      if (specifier.startsWith('.') && context.parentURL) {
        const b = path.resolve(path.dirname(fileURLToPath(context.parentURL)), specifier);
        for (const c of [`${b}.ts`, path.join(b, 'index.ts')]) {
          if (existsSync(c)) return nextResolve(pathToFileURL(c).href, context);
        }
      }
      throw err;
    }
  }
});

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const { readZip } = await import(pathToFileURL(path.join(REPO, 'src/ui/viewer/zipArchive.ts')).href);
const wasm = await import(pathToFileURL(path.join(REPO, 'src/infrastructure/kernel/pkg/wasm_api.js')).href);
await wasm.default({
  module_or_path: readFileSync(path.join(REPO, 'src/infrastructure/kernel/pkg/wasm_api_bg.wasm'))
});

const [mode = 'gpu', base = 'http://127.0.0.1:5173/', outRoot = './out'] = process.argv.slice(2);
const outDir = path.resolve(outRoot, `f5-${mode}`);
rmSync(outDir, { recursive: true, force: true });
const dl = path.join(outDir, 'downloads');
mkdirSync(dl, { recursive: true });

// ---------------------------------------------------------------------------------------------
// Test model
function buildModel(guidPrefix) {
  let id = 0;
  const L = [];
  const e = (s) => {
    id++;
    L.push(`#${id}=${s};`);
    return id;
  };
  let g = 0;
  const chars = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$';
  const guid = () => {
    g++;
    let s = guidPrefix;
    let n = g;
    for (let i = 0; i < 12; i++) {
      s += chars[n % 64];
      n = Math.floor(n / 64);
    }
    return s;
  };
  const pt = (x, y, z) => e(`IFCCARTESIANPOINT((${x.toFixed(4)},${y.toFixed(4)},${z.toFixed(4)}))`);
  const dir = (x, y, z) => e(`IFCDIRECTION((${x.toFixed(4)},${y.toFixed(4)},${z.toFixed(4)}))`);
  const ax = (p, z, x) => e(`IFCAXIS2PLACEMENT3D(#${p},${z ? '#' + z : '$'},${x ? '#' + x : '$'})`);
  const lp = (rel, a) => e(`IFCLOCALPLACEMENT(${rel ? '#' + rel : '$'},#${a})`);

  const ua = e(`IFCUNITASSIGNMENT((#${e('IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)')},#${e('IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)')}))`);
  const ctx = e(`IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#${ax(pt(0, 0, 0), null, null)},$)`);
  // 'Axis' first on purpose (Revit order): a rebend must not attach its Body to it.
  e(`IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Axis','Model',*,*,*,*,#${ctx},$,.GRAPH_VIEW.,$)`);
  const body = e(`IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#${ctx},$,.MODEL_VIEW.,$)`);
  const project = e(`IFCPROJECT('${guid()}',$,'F5 E2E',$,$,$,$,(#${ctx}),#${ua})`);
  const sitePl = lp(null, ax(pt(1000, 2000, 0), null, null));
  const site = e(`IFCSITE('${guid()}',$,'Site',$,$,#${sitePl},$,$,.ELEMENT.,$,$,$,$,$)`);
  const bldPl = lp(sitePl, ax(pt(0, 0, 0), null, null));
  const bld = e(`IFCBUILDING('${guid()}',$,'Bridge',$,$,#${bldPl},$,$,.ELEMENT.,$,$,$)`);
  const stPl = lp(bldPl, ax(pt(0, 0, 0), null, null));
  const storey = e(`IFCBUILDINGSTOREY('${guid()}',$,'Deck',$,$,#${stPl},$,$,.ELEMENT.,0.)`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${project},(#${site}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${site},(#${bld}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${bld},(#${storey}))`);
  const shape = (item, kind) =>
    e(`IFCPRODUCTDEFINITIONSHAPE($,$,(#${e(`IFCSHAPEREPRESENTATION(#${body},'Body','${kind}',(#${item}))`)}))`);
  const disk = (a, b, r) => e(`IFCSWEPTDISKSOLID(#${e(`IFCPOLYLINE((#${pt(...a)},#${pt(...b)}))`)},${r},$,$,$)`);
  const products = {};
  const prof = e(`IFCRECTANGLEPROFILEDEF(.AREA.,$,#${e(`IFCAXIS2PLACEMENT2D(#${e('IFCCARTESIANPOINT((15.,0.))')},$)`)},30.,4.)`);
  const ext = e(`IFCEXTRUDEDAREASOLID(#${prof},#${ax(pt(0, 0, 0), null, null)},#${dir(0, 0, 1)},1.)`);
  products.deck = e(`IFCSLAB('${guid()}',$,'Deck slab',$,$,#${lp(stPl, ax(pt(0, 0, 0), null, null))},#${shape(ext, 'SweptSolid')},$,.FLOOR.)`);
  products.tendon = e(`IFCTENDON('${guid()}',$,'Tendon T1',$,$,#${lp(stPl, ax(pt(0, 0, 0.5), null, null))},#${shape(disk([0, 0, 0], [30, 0, 0], 0.04), 'AdvancedSweptSolid')},$,$,.STRAND.,0.08,$,$,$,$,$,$)`);
  const bar = (name, placement, a, b) =>
    e(`IFCREINFORCINGBAR('${guid()}',$,'${name}',$,$,#${placement},#${shape(disk(a, b, 0.01), 'AdvancedSweptSolid')},$,$,0.02,$,$,.LIGATURE.,$)`);
  products.bar5 = bar('Stirrup x5', lp(stPl, ax(pt(5, 0, 0.5), null, null)), [0, -1.8, 0], [0, 1.8, 0]);
  const tilt = lp(stPl, ax(pt(10, 0, 0.5), dir(1, 0, 0), dir(0, 1, 0)));
  products.bar10 = bar('Stirrup x10 tilted', lp(tilt, ax(pt(0, 0, 0), null, null)), [-1.8, 0, 0], [1.8, 0, 0]);
  const rot = lp(stPl, ax(pt(15, 0, 0), dir(0, 0, 1), dir(0, 1, 0)));
  products.bar15 = bar('Stirrup x15 rotZ', lp(rot, ax(pt(0, 0, 0.5), null, null)), [-1.8, 0, 0], [1.8, 0, 0]);
  products.bar20 = bar('Stirrup x20 clear', lp(stPl, ax(pt(20, 0, 0.8), null, null)), [0, -1.8, 0], [0, 1.8, 0]);
  e(`IFCRELCONTAINEDINSPATIALSTRUCTURE('${guid()}',$,$,$,(${Object.values(products).map((p) => '#' + p).join(',')}),#${storey})`);
  const text = [
    'ISO-10303-21;',
    'HEADER;',
    "FILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');",
    "FILE_NAME('f5_bridge.ifc','2026-11-24T00:00:00',(''),(''),'gen','gen','');",
    "FILE_SCHEMA(('IFC4'));",
    'ENDSEC;',
    'DATA;',
    ...L,
    'ENDSEC;',
    'END-ISO-10303-21;',
    ''
  ].join('\r\n');
  return { text, products };
}

// Axis along world +X at y=2000 (Northing Easting order), station 100 at x=1000.
const LANDXML = `<?xml version="1.0" encoding="UTF-8"?>
<LandXML version="1.2">
  <Units><Metric linearUnit="meter" angularUnit="grads" directionUnit="grads"/></Units>
  <Alignments>
    <Alignment name="EJE_F5" staStart="100.0">
      <CoordGeom>
        <Line length="30.0"><Start>2000.0 1000.0</Start><End>2000.0 1030.0</End></Line>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>
`;

const { text: ifcText, products } = buildModel('3F5e2eTest');
const { text: decoyText } = buildModel('0Dec0yTest');
const ifcPath = path.join(outDir, 'f5_bridge.ifc');
const decoyPath = path.join(outDir, 'f5_decoy.ifc');
const xmlPath = path.join(outDir, 'f5_axis.xml');
writeFileSync(ifcPath, ifcText);
writeFileSync(decoyPath, decoyText);
writeFileSync(xmlPath, LANDXML);

// ---------------------------------------------------------------------------------------------
const results = [];
const check = (id, ok, detail) => {
  results.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${id} ${JSON.stringify(detail)}`);
};

const port = 9390 + ({ gpu: 1, gl2: 2, none: 3 }[mode] ?? 9);
const { proc, wsUrl } = await launch({ port, profileDir: path.resolve(outRoot, `profile-f5-${mode}`) });
const cdp = await connect(wsUrl);
const errors = [];
cdp.on('Runtime.consoleAPICalled', (p) => {
  if (p.type === 'error') errors.push(p.args.map((a) => a.value ?? a.description).join(' '));
});
cdp.on('Runtime.exceptionThrown', (p) =>
  errors.push(p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text)
);
await cdp.send('Runtime.enable');
await cdp.send('Page.enable');
await applyMode(cdp.send, mode);
try {
  await cdp.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: dl });
} catch {
  await cdp.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: dl });
}

const ev = cdp.evaluate;
async function waitFor(expr, timeout = 60000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    try {
      const v = await ev(expr);
      if (v) return v;
    } catch {}
    await sleep(150);
  }
  throw new Error('timeout ' + expr);
}
const setSelect = (selector, value) =>
  ev(
    `(() => { const s = document.querySelector(${JSON.stringify(selector)}); Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(s, ${JSON.stringify(value)}); s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`
  );
async function setFile(selector, file) {
  const { root } = await cdp.send('DOM.getDocument', { depth: -1 });
  const { nodeId } = await cdp.send('DOM.querySelector', { nodeId: root.nodeId, selector });
  if (!nodeId) throw new Error('no input ' + selector);
  await cdp.send('DOM.setFileInputFiles', { nodeId, files: [path.resolve(file)] });
}
const clickText = (text) =>
  ev(
    `(() => { const b = [...document.querySelectorAll('button')].find((x) => x.textContent.trim() === ${JSON.stringify(text)}); if (!b) return false; b.click(); return true; })()`
  );
async function shot(name) {
  await ev(
    `new Promise((r) => { let i = 0; const f = () => (++i >= 4 ? r(true) : requestAnimationFrame(f)); requestAnimationFrame(f); })`
  );
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(path.join(outDir, `${name}.png`), Buffer.from(data, 'base64'));
}
async function waitDownload(pred, timeout = 30000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    const f = readdirSync(dl).find((n) => pred(n) && !n.endsWith('.crdownload'));
    if (f) {
      await sleep(300);
      return path.join(dl, f);
    }
    await sleep(200);
  }
  throw new Error('download timeout');
}
const notices = () =>
  ev(
    `[...document.querySelectorAll('.clash-alignment-box .clash-notice, .clash-alignment-box .clash-error')].map((n) => n.className.replace('clash-', '') + ': ' + n.textContent.trim())`
  );
const cards = () =>
  ev(`[...document.querySelectorAll('.clash-card')].map((c) => c.textContent.replace(/\\s+/g, ' ').trim())`);
async function runClash() {
  await ev(`document.querySelector('.clash-run-btn').click(), true`);
  await waitFor(`!document.querySelector('.clash-run-btn').disabled && !!document.querySelector('.clash-results')`);
  await sleep(300);
  return cards();
}
async function reimport(file) {
  const before = JSON.stringify(await notices());
  await setFile('input[type=file][accept=".dxf"]', file);
  await waitFor(
    `(() => { const box = document.querySelector('.clash-alignment-box'); const btn = document.querySelector('[data-testid="clash-reimport-dxf-btn"]'); const now = JSON.stringify([...box.querySelectorAll('.clash-notice, .clash-error')].map((n) => n.className.replace('clash-', '') + ': ' + n.textContent.trim())); return btn && !btn.disabled && now !== ${JSON.stringify(before)} && !/Aplicando parche/.test(now); })()`,
    90000
  );
  await sleep(300);
  return notices();
}
/** World bounds of an element in an open model, through the app's kernel port. */
const liveBounds = (modelId, expressId) =>
  ev(`(async () => {
    const m = await window.__gubim_kernel__.elementMesh(${JSON.stringify(modelId)}, ${expressId});
    if (!m) return null;
    const lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
    for (let i = 0; i < m.positions.length; i += 3) for (let k = 0; k < 3; k++) { lo[k] = Math.min(lo[k], m.positions[i + k]); hi[k] = Math.max(hi[k], m.positions[i + k]); }
    return { lo, hi };
  })()`);

function moveInsert(dxfText, expressId, dx, dy) {
  const lines = dxfText.split(/\r?\n/);
  let moved = 0;
  for (let i = 0; i + 1 < lines.length; i += 2) {
    if (lines[i].trim() === '0' && lines[i + 1].trim() === 'INSERT') {
      let j = i + 2;
      let block = '';
      let ix = -1;
      let iy = -1;
      while (j + 1 < lines.length && lines[j].trim() !== '0') {
        const c = lines[j].trim();
        if (c === '2') block = lines[j + 1].trim();
        if (c === '10') ix = j + 1;
        if (c === '20') iy = j + 1;
        j += 2;
      }
      if (block.startsWith(`GUBIM_A_${expressId}_`)) {
        lines[ix] = (Number(lines[ix]) + dx).toFixed(6);
        lines[iy] = (Number(lines[iy]) + dy).toFixed(6);
        moved++;
      }
    }
  }
  return { text: lines.join('\r\n'), moved };
}
function bounds(model, id) {
  const v = model.elementMesh(id);
  if (!v) return null;
  try {
    const p = v.takePositions();
    const lo = [Infinity, Infinity, Infinity];
    const hi = [-Infinity, -Infinity, -Infinity];
    for (let i = 0; i < p.length; i += 3) {
      for (let k = 0; k < 3; k++) {
        lo[k] = Math.min(lo[k], p[i + k]);
        hi[k] = Math.max(hi[k], p[i + k]);
      }
    }
    return { lo, hi };
  } finally {
    v.free();
  }
}
const centre = (b) => (b ? [0, 1, 2].map((k) => (b.lo[k] + b.hi[k]) / 2) : null);
const near = (v, w, tol = 1e-4) => !!v && v.every((x, i) => Math.abs(x - w[i]) < tol);

try {
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1400, height: 900, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: base });
  await waitFor(`!!window.__gubim_dev__ && !!window.__gubim_kernel__ && !!document.querySelector('input[type=file][accept=".ifc"]')`);
  await setFile('input[type=file][accept=".ifc"]', decoyPath);
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 1`);
  await setFile('input[type=file][accept=".ifc"]', ifcPath);
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 2`);
  await sleep(500);
  const [decoyId, bridgeId] = await ev(`[...window.__gubim_dev__.activeRenderer.models.keys()]`);

  // F1: clashes on the bridge only, LandXML, ZIP.
  await ev(`document.querySelector('.tab-clash').click(), true`);
  await waitFor(`[...document.querySelector('.clash-class-a').options].some((o) => o.value === 'IfcReinforcingBar')`);
  await setSelect('.clash-model-a', bridgeId);
  await setSelect('.clash-model-b', bridgeId);
  await setSelect('.clash-class-a', 'IfcReinforcingBar');
  await setSelect('.clash-class-b', 'IfcTendon');
  const cards1 = await runClash();
  await setFile('input[type=file][accept=".xml,.landxml"]', xmlPath);
  await waitFor(`[...document.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Exportar ZIP DXF')`, 15000);
  await clickText('Exportar ZIP DXF');
  const zipPath = await waitDownload((n) => n.endsWith('.zip'), 60000);
  await shot('01-exported');
  const zip = await readZip(new Uint8Array(readFileSync(zipPath)));
  const names = [...zip.keys()];
  const individual = names.filter((n) => n.startsWith('individual/') && n.endsWith('.dxf'));
  check(
    'F1 3 clashes, LandXML binds, ZIP holds 3 sections at PK 0+105/110/115 + master + summary',
    cards1.length === 3 &&
      individual.length === 3 &&
      ['0+105', '0+110', '0+115'].every((pk) => individual.some((n) => n.includes(pk))) &&
      names.includes('clashes_all.dxf') &&
      names.includes('summary.csv'),
    { clashes: cards1.length, names }
  );

  const dec = (b) => new TextDecoder().decode(b);
  const dxfOf = (pk) => dec(zip.get(individual.find((n) => n.includes(pk))));
  const longest = Math.max(...individual.flatMap((n) => dec(zip.get(n)).split(/\r?\n/).map((l) => l.length)));
  const dxf10 = dxfOf('0+110');
  const dxf15 = dxfOf('0+115');
  const guid10 = ifcText.match(new RegExp(`#${products.bar10}=IFCREINFORCINGBAR\\('([^']+)'`))[1];
  check('F2 every DXF value <= 255 chars; GUBIM_ORIG carries the full GlobalId', longest <= 255 && dxf10.includes(`guid=${guid10}`), {
    longest,
    guid10
  });

  // F3: raw exported DXF, one INSERT moved.
  const e10 = moveInsert(dxf10, products.bar10, 0, 0.3);
  const ed10 = path.join(outDir, 'edited_PK110.dxf');
  writeFileSync(ed10, e10.text);
  const n3 = await reimport(ed10);
  await shot('02-reimport-110');
  check('F3 re-import of the edited PK 110 DXF: 1 applied, 2 clashes left', e10.moved === 1 && n3.some((s) => /\(1 elementos actualizados\)\. Choques restantes: 2\./.test(s)), n3);

  // F4: same file again.
  const b10before = await liveBounds(bridgeId, products.bar10);
  const n4 = await reimport(ed10);
  const b10after = await liveBounds(bridgeId, products.bar10);
  check('F4 the same DXF again applies nothing (ya aplicado) and moves nothing', n4.some((s) => /^error: .*ya aplicado/.test(s)) && near(centre(b10after), centre(b10before), 1e-9), {
    n4,
    before: centre(b10before),
    after: centre(b10after)
  });

  // F5: DXF from an earlier session, decoy as primary model.
  const e15 = moveInsert(dxf15, products.bar15, 0, 0.3);
  const stale = e15.text.replace(/\bmodel=[^\s\r\n]+/g, 'model=model-1700000000000-old0');
  const ed15 = path.join(outDir, 'edited_PK115_stale.dxf');
  writeFileSync(ed15, stale);
  await setSelect('.clash-model-a', decoyId);
  const decoyBefore = await liveBounds(decoyId, products.bar15);
  const n5 = await reimport(ed15);
  const decoyAfter = await liveBounds(decoyId, products.bar15);
  const bridge15 = await liveBounds(bridgeId, products.bar15);
  check('F5 stale-session DXF skips the decoy and moves the bridge bar', n5.some((s) => /1 elementos actualizados/.test(s)) && near(centre(decoyAfter), centre(decoyBefore), 1e-9) && near(centre(bridge15), [1015, 2000, 0.8]), {
    n5,
    decoy: centre(decoyAfter),
    bridge15: centre(bridge15)
  });

  await setSelect('.clash-model-a', bridgeId);
  const cards6 = await runClash();
  await shot('03-final');
  check('F6 one clash left (x=5 stirrup); decoy x=15 bar still at z=0.5', cards6.length === 1 && /x5/.test(cards6[0] ?? '') && near(centre(decoyAfter), [1015, 2000, 0.5]), { cards6 });

  // F7: the patched IFC, read back by the kernel in Node.
  await clickText('Descargar IFC parcheado');
  const patchedPath = await waitDownload((n) => n.endsWith('.ifc'));
  const patched = readFileSync(patchedPath, 'utf8');
  writeFileSync(path.join(outDir, 'patched.ifc'), patched);
  const patchedLines = new Set(patched.split(/\r?\n/));
  const changedIds = ifcText
    .split('\r\n')
    .filter((l) => l.startsWith('#') && !patchedLines.has(l))
    .map((l) => Number(l.match(/^#(\d+)=/)[1]))
    .sort((a, b) => a - b);
  const guids = (t) => [...t.matchAll(/IFC[A-Z]+\('([0-9A-Za-z_$]{22})'/g)].map((m) => m[1]).sort().join(',');
  const m0 = new wasm.Model(new TextEncoder().encode(ifcText));
  const m1 = new wasm.Model(new TextEncoder().encode(patched));
  const delta = {};
  for (const [k, eid] of Object.entries(products)) {
    const a = centre(bounds(m0, eid));
    const b = centre(bounds(m1, eid));
    delta[k] = a && b ? b.map((x, i) => +(x - a[i]).toFixed(6)) : null;
  }
  const s10 = bounds(m1, products.bar10);
  check(
    'F7 patched IFC: only bars x10/x15 changed, GlobalIds identical, both +0.30 m world Z, orientation kept',
    JSON.stringify(changedIds) === JSON.stringify([products.bar10, products.bar15].sort((a, b) => a - b)) &&
      guids(ifcText) === guids(patched) &&
      near(delta.bar10, [0, 0, 0.3]) &&
      near(delta.bar15, [0, 0, 0.3]) &&
      ['deck', 'tendon', 'bar5', 'bar20'].every((k) => near(delta[k], [0, 0, 0])) &&
      Math.abs(s10.lo[1] - 1998.2) < 1e-3 &&
      Math.abs(s10.hi[1] - 2001.8) < 1e-3,
    { changedIds, delta }
  );
  check('F8 no console errors', errors.length === 0, errors.slice(0, 5));
} catch (err) {
  console.log('FAIL exception', err.stack ?? err);
  results.push(false);
  try {
    await shot('99-error');
  } catch {}
} finally {
  cdp.close();
  try {
    proc.kill();
  } catch {}
}
const ok = results.length > 0 && results.every(Boolean);
console.log(`f5 ${mode}: ${results.filter(Boolean).length}/${results.length} pass`);
process.exit(ok ? 0 : 1);
