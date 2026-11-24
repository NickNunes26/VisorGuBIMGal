// F5 review scenarios through the real UI (review of 2026-11-24, see REPORT.md).
// Usage: node scenarios.mjs <gpu|gl2|none> <baseUrl> [outRoot] [scenario,...]
// Needs a Vite dev server at baseUrl. outRoot must be outside the repo (Chrome profiles there crash
// the Vite watcher); default <tmp>/gubim-e2e-review-f5.
import os from 'node:os';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { bridge, gridModel, linearModel, landXml } from './gen.mjs';
import {
  REPO, session, sleep, nodeModel, nodeBounds, centre, near, sub, r6, readZip,
  dxfLines, entities, units, rebuild, inserts, metaTexts, moveInsert, blockOf, editPolyline,
  setEditPolyline, kerbTop, maxLineLen, retargetGuid
} from './lib.mjs';

const [mode = 'gpu', base = 'http://127.0.0.1:5199/', outArg = '', only = ''] = process.argv.slice(2);
const outRoot = outArg || path.join(os.tmpdir(), 'gubim-e2e-review-f5');
const REPO_ROOT = path.resolve(REPO);
if (path.resolve(outRoot).toLowerCase().startsWith(REPO_ROOT.toLowerCase())) throw new Error('outRoot must be outside the repo: Chrome profiles there crash the Vite watcher');
const want = only ? only.split(',') : null;
const root = path.resolve(outRoot, mode);
mkdirSync(root, { recursive: true });
const PORT0 = 9500 + ({ gpu: 0, gl2: 40, none: 80 }[mode] ?? 120);
let portN = 0;

const results = [];
let cachedSign = null;
const AXIS = landXml('EJE_REV', 100, [1000, 2000], [1030, 2000]);
const AXIS_UTM = landXml('EJE_UTM', 100, [540000, 4700000], [540030, 4700000]);

function W(dir, name, text) {
  const p = path.join(dir, name);
  writeFileSync(p, text);
  return p;
}
const dec = (b) => new TextDecoder().decode(b);
const sectionDxf = (zip, pk) => {
  const n = [...zip.keys()].find((k) => k.startsWith('individual/') && k.includes(pk));
  return n ? dec(zip.get(n)) : null;
};
const appliedOf = (notes) => {
  const m = notes.join(' | ').match(/\((\d+) elementos actualizados/);
  return m ? Number(m[1]) : 0;
};
const remainingOf = (notes) => {
  const m = notes.join(' | ').match(/Choques restantes: (\d+)/);
  return m ? Number(m[1]) : null;
};
/** Sheet->world mapping for a vertical section across an axis along +X, anchored on a bar. */
function frameFromKerb(dxf, insert, barCentre, kerbWorld = [1.4, 1.4]) {
  const kt = kerbTop(dxf);
  const dxs = kt.x - insert.x;
  const sign = Math.sign(dxs);
  return {
    sign,
    kerbDx: dxs,
    kerbDy: kt.y - insert.y,
    okScale: Math.abs(Math.abs(dxs) - kerbWorld[0]) < 0.02 && Math.abs(kt.y - insert.y - (1.4 - barCentre[2])) < 0.02 && Math.abs(kt.width - 0.4) < 0.02,
    toWorld: ([px, py]) => [barCentre[0], barCentre[1] + sign * (px - insert.x), barCentre[2] + (py - insert.y)]
  };
}

async function scenario(name, fn) {
  if (want && !want.includes(name)) return;
  const dir = path.join(root, name);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const checks = [];
  const check = (id, ok, detail) => {
    checks.push({ id, ok: !!ok, detail });
    console.log(`${ok ? 'PASS' : 'FAIL'} [${mode}] ${name} ${id} ${JSON.stringify(detail)}`);
  };
  const sessions = [];
  const open = async () => {
    const S = await session({ mode, base, outDir: dir, port: PORT0 + portN++ % 40, name: `${name}-${sessions.length}` });
    sessions.push(S);
    await S.open();
    return S;
  };
  const t0 = Date.now();
  try {
    await fn({ dir, check, open });
  } catch (err) {
    check('exception', false, String(err.stack ?? err).slice(0, 1500));
    for (const S of sessions) {
      try {
        await S.shot(path.join(dir, `99-error-${sessions.indexOf(S)}.png`));
      } catch {}
    }
  } finally {
    const errs = sessions.flatMap((S) => S.errors);
    check('no console errors', errs.length === 0, errs.slice(0, 6));
    writeFileSync(path.join(dir, 'console.log'), sessions.map((S, i) => `--- session ${i}\n` + S.consoleLog.join('\n')).join('\n'));
    for (const S of sessions) await S.close();
  }
  writeFileSync(path.join(dir, 'checks.json'), JSON.stringify(checks, null, 1));
  results.push({ name, ok: checks.every((c) => c.ok), checks, secs: Math.round((Date.now() - t0) / 1000) });
}

/** Load a bridge, run bars vs tendons, bind LandXML, export the ZIP. */
async function bridgeFlow(S, dir, files, { classA = 'IfcReinforcingBar', classB = 'IfcTendon', axis = AXIS, models } = {}) {
  let n = 0;
  for (const f of files) await S.loadIfc(f, ++n);
  const ids = await S.modelIds();
  await S.openClashTab();
  const sel = models ? models(ids) : { a: ids[ids.length - 1], b: ids[ids.length - 1] };
  await S.configure({ modelA: sel.a, modelB: sel.b, classA, classB });
  const cards = await S.runClash();
  await S.loadLandXml(W(dir, 'axis.xml', axis));
  const { zip, path: zipPath } = await S.exportZip();
  return { ids, cards, zip, zipPath };
}

// =============================================================================================
await scenario('r12-utm', async ({ dir, check, open }) => {
  const m = bridge({ origin: [540000, 4700000, 0], guidPrefix: '3UtmRev' });
  const S = await open();
  const { ids, cards, zip } = await bridgeFlow(S, dir, [W(dir, 'utm.ifc', m.text)], { axis: AXIS_UTM });
  const bid = ids[0];
  const names = [...zip.keys()];
  const dxfs = names.filter((n) => n.endsWith('.dxf'));
  const longest = Math.max(...dxfs.map((n) => maxLineLen(dec(zip.get(n)))));
  check('4 clashes, 4 sections', cards.length === 4 && names.filter((n) => n.startsWith('individual/')).length === 4, { cards: cards.length, names });
  check('every DXF line <= 255 chars (UTM)', longest <= 255, { longest });
  const d110 = sectionDxf(zip, '0+110');
  const meta = metaTexts(d110);
  const metaRecs = meta.filter((t) => t.startsWith('GUBIM_META'));
  const keys = new Set(metaRecs.flatMap((t) => t.split(/\s+/).slice(1).map((x) => x.split('=')[0])));
  const allKeys = ['v', 'model', 'pk', 'sMin', 'sMax', 'ox', 'oy', 'oz', 'rx', 'ry', 'rz', 'ux', 'uy', 'uz', 'lx', 'ly', 'lz', 'nx', 'ny'];
  check('GUBIM_META v=2 split over records, all 19 keys present', metaRecs.length >= 1 && allKeys.every((k) => keys.has(k)) && metaRecs.every((r) => r.length <= 240), { records: metaRecs.map((r) => r.length), missing: allKeys.filter((k) => !keys.has(k)) });
  const orig = meta.filter((t) => t.startsWith('GUBIM_ORIG'));
  check('GUBIM_ORIG carries the full GlobalId', orig.some((t) => t.includes(`guid=${m.guids.bar10}`)), orig);
  writeFileSync(path.join(dir, 'PK110.dxf'), d110);
  // Move bar10: dx=+0.2, dy=+0.3 (sheet metres).
  const ins = inserts(d110).find((i) => blockOf(m.products.bar10)(i.block));
  const fr = frameFromKerb(d110, ins, m.centres.bar10);
  check('section frame from kerb: up=+Z, 1 sheet m = 1 world m', fr.okScale, { kerbDx: fr.kerbDx, kerbDy: fr.kerbDy, sign: fr.sign });
  const e = moveInsert(d110, blockOf(m.products.bar10), 0.2, 0.3);
  const n1 = await S.reimport(W(dir, 'PK110_move.dxf', e.text));
  const b = await S.liveBounds(bid, m.products.bar10);
  const want10 = [m.centres.bar10[0], m.centres.bar10[1] + fr.sign * 0.2, m.centres.bar10[2] + 0.3];
  check('UTM move bar10 lands at expected world position', appliedOf(n1) === 1 && near(centre(b), want10, 1e-3), { n1, got: r6(centre(b)), want: want10 });
  // Rebend bar5 (PK105) at UTM scale.
  const d105 = sectionDxf(zip, '0+105');
  const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
  const fr5 = frameFromKerb(d105, i5, m.centres.bar5);
  const V = [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]];
  const n2 = await S.reimport(W(dir, 'PK105_rebend.dxf', setEditPolyline(d105, m.products.bar5, V)));
  const b5 = await S.liveBounds(bid, m.products.bar5);
  const w = V.map(fr5.toWorld);
  const ok5 = b5 && Math.abs(b5.lo[2] - (w[1][2] - 0.01)) < 0.006 && Math.abs(b5.hi[2] - (w[0][2] + 0.01)) < 0.006 && Math.abs((b5.lo[0] + b5.hi[0]) / 2 - w[1][0]) < 1e-3 && Math.abs(b5.lo[1] - Math.min(w[0][1], w[2][1])) < 0.012 && Math.abs(b5.hi[1] - Math.max(w[0][1], w[2][1])) < 0.012;
  check('UTM rebend bar5: mesh follows the drawn V', appliedOf(n2) === 1 && ok5, { n2, lo: r6(b5?.lo), hi: r6(b5?.hi), wantPts: w.map(r6) });
  check('UTM: clashes left 2 after both edits', remainingOf(n2) === 2, n2);
  await S.shot(path.join(dir, '01-utm-final.png'));
});

// =============================================================================================
for (const schema of ['IFC2X3', 'IFC4', 'IFC4X3_ADD2']) {
  await scenario(`rebend-${schema}`, async ({ dir, check, open }) => {
    const m = bridge({ schema, guidPrefix: '2Sch' + schema.slice(3, 6).replace('_', 'x') });
    const S = await open();
    const ifcPath = W(dir, `bridge_${schema}.ifc`, m.text);
    const { ids, cards, zip } = await bridgeFlow(S, dir, [ifcPath]);
    const bid = ids[0];
    check('4 clashes', cards.length === 4, cards.length);
    const d105 = sectionDxf(zip, '0+105');
    const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
    const orig = editPolyline(d105, m.products.bar5);
    check('EDIT_A polyline exported for bar5 (2 pts spanning the bar)', orig && orig.length === 2 && Math.abs(Math.abs(orig[1][0] - orig[0][0]) - 3.6) < 0.01, orig);
    const fr = frameFromKerb(d105, i5, m.centres.bar5);
    const V = [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]];
    const n = await S.reimport(W(dir, 'PK105_V.dxf', setEditPolyline(d105, m.products.bar5, V)));
    const b = await S.liveBounds(bid, m.products.bar5);
    const w = V.map(fr.toWorld);
    const geomOk = b && Math.abs(b.lo[2] - (w[1][2] - 0.01)) < 0.006 && Math.abs(b.hi[2] - (w[0][2] + 0.01)) < 0.006 && Math.abs(b.lo[1] - Math.min(w[0][1], w[2][1])) < 0.012 && Math.abs(b.hi[1] - Math.max(w[0][1], w[2][1])) < 0.012 && Math.abs(b.hi[0] - b.lo[0] - 0.02) < 0.003;
    check('rebend applied, live mesh follows the V', appliedOf(n) === 1 && geomOk, { n, lo: r6(b?.lo), hi: r6(b?.hi), want: w.map(r6) });
    check('clashes left 3', remainingOf(n) === 3, n);
    const pp = await S.downloadPatched();
    const patched = readFileSync(pp, 'utf8');
    writeFileSync(path.join(dir, 'patched.ifc'), patched);
    const added = patched.slice(patched.indexOf('/* GUBIM_F5'));
    writeFileSync(path.join(dir, 'added.txt'), added);
    const rep = added.match(/IFCSHAPEREPRESENTATION\(#(\d+),'Body','AdvancedSweptSolid'/);
    check('new Body representation uses the Body subcontext (not Axis)', rep && Number(rep[1]) === m.ids.body, { ctx: rep?.[1], body: m.ids.body, axis: m.ids.axisCtx });
    const list3d = /IFCCARTESIANPOINTLIST3D\(/.test(added);
    const idx = /IFCINDEXEDPOLYCURVE\(/.test(added);
    const sds = added.match(/IFCSWEPTDISKSOLID\(([^;]*)\);/)?.[1];
    let schemaOk;
    if (schema === 'IFC2X3') schemaOk = !list3d && !idx && /IFCPOLYLINE\(\(/.test(added) && /,\$,0\.,2\.$/.test(sds ?? '');
    else if (schema === 'IFC4') schemaOk = list3d && idx && /IFCCARTESIANPOINTLIST3D\(\(\([^;]*\)\)\);/.test(added);
    else schemaOk = list3d && idx && /IFCCARTESIANPOINTLIST3D\(\([^;]*\),\$\);/.test(added);
    check(`schema-specific entities for ${schema}`, schemaOk, { sds, list3d, idx });
    check('radius from the bar (0.01 m)', /,0\.010000,/.test(sds ?? ''), sds);
    // Text diff: only the bar5 record changed among the original lines; GlobalIds identical.
    const pl = new Set(patched.split(/\r?\n/));
    const changed = m.text.split('\r\n').filter((l) => l.startsWith('#') && !pl.has(l)).map((l) => Number(l.match(/^#(\d+)=/)[1]));
    const g = (t) => [...t.matchAll(/IFC[A-Z0-9]+\('([0-9A-Za-z_$]{22})'/g)].map((x) => x[1]).sort().join(',');
    check('only the bar5 record changed; GlobalIds identical', JSON.stringify(changed) === JSON.stringify([m.products.bar5]) && g(m.text) === g(patched), { changed });
    // Fresh kernel (Node) reads the patched file.
    const nm = nodeModel(patched);
    const nb = nodeBounds(nm, m.products.bar5);
    const nb0 = nodeBounds(nodeModel(m.text), m.products.bar10);
    const nb10 = nodeBounds(nm, m.products.bar10);
    const diag = JSON.parse(nm.summaryJson()).diagnostics ?? [];
    check('patched file re-read in Node: bar5 has a mesh with the V, others untouched, no diagnostics', nb && nb.nan === 0 && near(nb.lo, b.lo, 1e-4) && near(nb.hi, b.hi, 1e-4) && near(centre(nb10), centre(nb0), 1e-9) && diag.length === 0, { lo: r6(nb?.lo), hi: r6(nb?.hi), diag: diag.slice(0, 3) });
    // Other representations of bar5 (it had Body + Axis).
    const prodLine = patched.split(/\r?\n/).find((l) => l.startsWith(`#${m.products.bar5}=`));
    const pdsId = prodLine.split(',')[6];
    const pds = patched.split(/\r?\n/).find((l) => l.startsWith(`${pdsId}=`));
    check('[observation] bar5 Axis representation kept after rebend', /,\(#\d+,#\d+\)\)/.test(pds ?? ''), { pds });
    await S.shot(path.join(dir, `01-${schema}.png`));
  });
}

// =============================================================================================
await scenario('placements', async ({ dir, check, open }) => {
  // (a) bridge, mm units, rotated parents.
  {
    const m = bridge({ unit: 'mm', guidPrefix: '1MmRev' });
    const S = await open();
    const { ids, cards, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge_mm.ifc', m.text)]);
    const bid = ids[0];
    check('mm model: 4 clashes', cards.length === 4, cards.length);
    for (const [key, pk, dx, dy] of [['bar10', '0+110', 0.2, 0.3], ['bar15', '0+115', -0.15, 0.25]]) {
      const d = sectionDxf(zip, pk);
      const ins = inserts(d).find((i) => blockOf(m.products[key])(i.block));
      const fr = frameFromKerb(d, ins, m.centres[key]);
      check(`${key}: sheet in metres for a mm file (kerb offsets)`, fr.okScale, { kerbDx: fr.kerbDx, kerbDy: fr.kerbDy });
      const n = await S.reimport(W(dir, `mm_${key}.dxf`, moveInsert(d, blockOf(m.products[key]), dx, dy).text));
      const b = await S.liveBounds(bid, m.products[key]);
      const want = [m.centres[key][0], m.centres[key][1] + fr.sign * dx, m.centres[key][2] + dy];
      check(`mm ${key} (rotated parent) moved by the drawn vector`, appliedOf(n) === 1 && near(centre(b), want, 1e-3) && Math.abs(b.hi[1] - b.lo[1] - 3.6) < 1e-3, { n, got: r6(centre(b)), want });
    }
    const pp = await S.downloadPatched();
    const patched = readFileSync(pp, 'utf8');
    writeFileSync(path.join(dir, 'patched_mm.ifc'), patched);
    const nm = nodeModel(patched);
    const c10 = centre(nodeBounds(nm, m.products.bar10));
    const c15 = centre(nodeBounds(nm, m.products.bar15));
    check('mm patched file re-read in Node: both moves present', near(c10, [1010, 2000 + Math.sign(c10[1] - 2000) * 0.2, 0.8], 1e-3) && near(c15, [1015, 2000 - Math.sign(c10[1] - 2000) * 0.15, 0.75], 1e-3), { c10: r6(c10), c15: r6(c15) });
    await S.close();
  }
  // (b) grid placement, IFC4X3 and IFC4.
  for (const schema of ['IFC4X3_ADD2', 'IFC4']) {
    const m = gridModel({ schema, guidPrefix: '0Grd' + schema.length });
    const S = await open();
    const { ids, cards, zip } = await bridgeFlow(S, dir, [W(dir, `grid_${schema}.ifc`, m.text)]);
    check(`grid ${schema}: 1 clash`, cards.length === 1, cards);
    const d = sectionDxf(zip, '0+108');
    const ins = inserts(d).find((i) => blockOf(m.products.gbar)(i.block));
    // Same LandXML direction as the bridge: right = sign * world Y, measured on the bridge kerb.
    const signY = await rightSign(open, dir);
    const n = await S.reimport(W(dir, `grid_${schema}.dxf`, moveInsert(d, blockOf(m.products.gbar), 0.2, 0.3).text));
    const b = await S.liveBounds(ids[0], m.products.gbar);
    const want = [1008, 2000 + signY * 0.2, 0.8];
    check(`IfcGridPlacement (${schema}) moved by the drawn vector`, appliedOf(n) === 1 && near(centre(b), want, 1e-3) && Math.abs(b.hi[1] - b.lo[1] - 3.6) < 1e-3, { n, got: r6(centre(b)), want, ins });
    await S.close();
  }
  // (c) linear placement (fixture + bars), straight and arc segments.
  {
    const probe = linearModel(path.join(REPO, 'test/fixtures/authored/linear-placement.ifc'));
    const arcC = centre(nodeBounds(nodeModel(probe.text), probe.products.lp420));
    const m = linearModel(path.join(REPO, 'test/fixtures/authored/linear-placement.ifc'), arcC);
    const S = await open();
    const axis = landXml('EJE_LP', 0, [0, 0], [500, 0]);
    const { ids, cards, zip } = await bridgeFlow(S, dir, [W(dir, 'linear.ifc', m.text)], { axis });
    check('linear: 2 clashes', cards.length === 2, cards);
    const signY = await rightSign(open, dir);
    const m0 = nodeModel(m.text);
    for (const key of ['lp45', 'lp420']) {
      const c0 = centre(nodeBounds(m0, m.products[key]));
      const name = [...zip.keys()].filter((k) => k.startsWith('individual/')).find((k) => inserts(dec(zip.get(k))).some((i) => blockOf(m.products[key])(i.block)));
      if (!name) {
        check(`linear ${key}: section found`, false, [...zip.keys()]);
        continue;
      }
      const d = dec(zip.get(name));
      const n = await S.reimport(W(dir, `lin_${key}.dxf`, moveInsert(d, blockOf(m.products[key]), 0.2, 0.3).text));
      const b = await S.liveBounds(ids[0], m.products[key]);
      const want = [c0[0], c0[1] + signY * 0.2, c0[2] + 0.3];
      check(`IfcLinearPlacement ${key} moved by the drawn vector`, appliedOf(n) === 1 && near(centre(b), want, 2e-3), { n, got: r6(centre(b)), want: r6(want), section: name });
    }
    await S.close();
  }
});


/** right = sign * world +Y for an axis along +X, measured once on the bridge kerb. */
async function rightSign(open, dir) {
  if (cachedSign !== null) return cachedSign;
  const m = bridge({ guidPrefix: '0Sgn' });
  const S = await open();
  const { zip } = await bridgeFlow(S, dir, [W(dir, 'sign_bridge.ifc', m.text)]);
  const d = sectionDxf(zip, '0+105');
  const ins = inserts(d).find((i) => blockOf(m.products.bar5)(i.block));
  cachedSign = frameFromKerb(d, ins, m.centres.bar5).sign;
  await S.close();
  return cachedSign;
}

// =============================================================================================
await scenario('idempotence', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1IdmRev' });
  const S = await open();
  const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)]);
  const bid = ids[0];
  const d110 = sectionDxf(zip, '0+110');
  const z = async () => centre(await S.liveBounds(bid, m.products.bar10))[2];
  const n1 = await S.reimport(W(dir, 'm030.dxf', moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text));
  check('move +0.30 applied', appliedOf(n1) === 1 && Math.abs((await z()) - 0.8) < 1e-4, n1);
  const n2 = await S.reimport(W(dir, 'm030.dxf', moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text));
  check('same DXF again: "ya aplicado", no change', /ya aplicado/.test(n2.join(' ')) && Math.abs((await z()) - 0.8) < 1e-4, n2);
  check('[ux] patched-IFC download still offered after a "ya aplicado" import', await S.hasButton('Descargar IFC parcheado'), {});
  const n3 = await S.reimport(W(dir, 'm050.dxf', moveInsert(d110, blockOf(m.products.bar10), 0, 0.5).text));
  check('further edit to +0.50: only +0.20 applied (total +0.50)', appliedOf(n3) === 1 && Math.abs((await z()) - 1.0) < 1e-4, { n3, z: await z() });
  const n4 = await S.reimport(W(dir, 'm000.dxf', d110));
  check('edit reverted to the exported position: bar back at +0.00', Math.abs((await z()) - 0.5) < 1e-4, { n4, z: await z() });
  // Rebend then revert.
  const d105 = sectionDxf(zip, '0+105');
  const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
  const V = [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]];
  const n5 = await S.reimport(W(dir, 'V.dxf', setEditPolyline(d105, m.products.bar5, V)));
  const zl = async () => (await S.liveBounds(bid, m.products.bar5)).lo[2];
  check('rebend V applied', appliedOf(n5) === 1 && Math.abs((await zl()) - 0.19) < 0.006, n5);
  const n6 = await S.reimport(W(dir, 'V.dxf', setEditPolyline(d105, m.products.bar5, V)));
  check('same rebend again: "ya aplicado"', /ya aplicado/.test(n6.join(' ')), n6);
  const n7 = await S.reimport(W(dir, 'straight.dxf', d105));
  check('rebend reverted to the exported polyline: bar straight again', Math.abs((await zl()) - 0.49) < 0.002, { n7, zlo: await zl() });
  await S.shot(path.join(dir, '01-idempotence.png'));
});

// =============================================================================================
await scenario('move-rebend', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1MvRbRev' });
  const files = [];
  // (a) one import: INSERT +0.3 and V drawn at the original place -> bar = V at original place.
  let single;
  {
    const S = await open();
    const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)]);
    const d105 = sectionDxf(zip, '0+105');
    writeFileSync(path.join(dir, 'PK105.dxf'), d105);
    const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
    const V = [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]];
    const both = setEditPolyline(moveInsert(d105, blockOf(m.products.bar5), 0, 0.3).text, m.products.bar5, V);
    files.push(W(dir, 'both.dxf', both), W(dir, 'V_only.dxf', setEditPolyline(d105, m.products.bar5, V)));
    const n = await S.reimport(files[0]);
    single = await S.liveBounds(ids[0], m.products.bar5);
    check('one import of move+rebend: bar ends on the drawn V (not shifted twice)', Math.abs(single.lo[2] - 0.19) < 0.006 && Math.abs(single.hi[2] - 0.51) < 0.006, { n, lo: r6(single.lo), hi: r6(single.hi) });
    // Polyline drawn at the moved place.
    const V2 = V.map(([x, y]) => [x, y + 0.3]);
    await S.close();
    const S2 = await open();
    const r2 = await bridgeFlow(S2, dir, [W(dir, 'bridge2.ifc', m.text)]);
    const d2 = sectionDxf(r2.zip, '0+105');
    const both2 = setEditPolyline(moveInsert(d2, blockOf(m.products.bar5), 0, 0.3).text, m.products.bar5, V2);
    const n2 = await S2.reimport(W(dir, 'both_shiftedV.dxf', both2));
    const b2 = await S2.liveBounds(r2.ids[0], m.products.bar5);
    check('move+rebend with the V drawn at the moved place: bar on that V', Math.abs(b2.lo[2] - 0.49) < 0.006 && Math.abs(b2.hi[2] - 0.81) < 0.006, { n2, lo: r6(b2.lo), hi: r6(b2.hi) });
  }
  // (b) two imports of the same edits: V first, then V + INSERT moved.
  {
    const S = await open();
    const { ids } = await bridgeFlow(S, dir, [W(dir, 'bridge3.ifc', m.text)]);
    const n1 = await S.reimport(files[1]);
    const n2 = await S.reimport(files[0]);
    const b = await S.liveBounds(ids[0], m.products.bar5);
    check('V then V+move (same final DXF) ends where the single import ends', near(b.lo, single.lo, 2e-3) && near(b.hi, single.hi, 2e-3), { n1, n2, lo: r6(b.lo), hi: r6(b.hi), singleLo: r6(single.lo), singleHi: r6(single.hi) });
  }
});

// =============================================================================================
await scenario('legacy-v1', async ({ dir, check, open }) => {
  const m = bridge({ origin: [540000, 4700000, 0], guidPrefix: '2LegRev' });
  const S = await open();
  const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'utm.ifc', m.text)], { axis: AXIS_UTM });
  const d110 = sectionDxf(zip, '0+110');
  // Rebuild the v1 layout of commit 4455862: one GUBIM_META line in the v1 key order, clipped
  // at 255 chars by the R12 writer, GUBIM_ORIG without guid=, GUBIM_POLY without part=.
  const kv = {};
  for (const t of metaTexts(d110).filter((t) => t.startsWith('GUBIM_META'))) for (const tok of t.split(/\s+/).slice(1)) { const [k, v] = tok.split('='); kv[k] = v; }
  const order = ['model', 'pk', 'sMin', 'sMax', 'ox', 'oy', 'oz', 'rx', 'ry', 'rz', 'ux', 'uy', 'uz', 'lx', 'ly', 'lz', 'nx', 'ny'];
  const v1full = 'GUBIM_META v=1 ' + order.map((k) => `${k}=${kv[k]}`).join(' ');
  const v1 = v1full.slice(0, 255);
  const lines = dxfLines(d110);
  const out = [];
  let metaDone999 = false;
  let metaDoneText = false;
  for (let i = 0; i < lines.length; i++) {
    const L = lines[i];
    if (/^GUBIM_META /.test(L)) {
      // 999 comment value or TEXT value: keep only the first occurrence of each kind.
      const isComment = lines[i - 1]?.trim() === '999';
      if (isComment) {
        if (metaDone999) { out.pop(); continue; }
        metaDone999 = true;
        out.push(v1);
      } else {
        if (metaDoneText) { out.push('GUBIM_NOTE removed'); continue; }
        metaDoneText = true;
        out.push(v1);
      }
      continue;
    }
    if (/^GUBIM_ORIG /.test(L)) { out.push(L.replace(/ guid=\S+/, '')); continue; }
    if (/^GUBIM_POLY /.test(L)) { out.push(L.replace(/ part=\d+/, '')); continue; }
    out.push(L);
  }
  const v1dxf = out.join('\r\n');
  writeFileSync(path.join(dir, 'PK110_v1.dxf'), v1dxf);
  check('v1 META line is really clipped (>=255 chars before clipping)', v1full.length > 255, { full: v1full.length, lost: v1full.slice(255) });
  const ins = inserts(v1dxf).find((i) => blockOf(m.products.bar10)(i.block));
  const V = [[ins.x - 1.8, ins.y], [ins.x, ins.y - 0.3], [ins.x + 1.8, ins.y]];
  const edited = setEditPolyline(moveInsert(v1dxf, blockOf(m.products.bar10), 0, 0.2).text, m.products.bar10, V);
  const n = await S.reimport(W(dir, 'PK110_v1_edited.dxf', edited));
  const b = await S.liveBounds(ids[0], m.products.bar10);
  check('v1: move applied, rebend refused with the re-export reason', appliedOf(n) === 1 && /metadatos del DXF incompletos, vuelva a exportar el ZIP/.test(n.join(' ')) && Math.abs(centre(b)[2] - 0.7) < 1e-4 && Math.abs(b.hi[2] - b.lo[2] - 0.02) < 1e-3, { n, c: r6(centre(b)) });
  // Same v1 DXF in a later session (model id unknown), decoy with the same expressIds open first.
  await S.close();
  const S2 = await open();
  const decoy = bridge({ origin: [540000, 4700000, 0], guidPrefix: '0DecLeg' });
  await S2.loadIfc(W(dir, 'decoy.ifc', decoy.text), 1);
  await S2.loadIfc(W(dir, 'utm2.ifc', m.text), 2);
  const [decoyId, bid2] = await S2.modelIds();
  await S2.openClashTab();
  await S2.configure({ modelA: decoyId, modelB: decoyId, classA: 'IfcReinforcingBar', classB: 'IfcTendon' });
  await S2.runClash();
  await S2.loadLandXml(W(dir, 'axis2.xml', AXIS_UTM));
  const moveOnly = moveInsert(v1dxf, blockOf(m.products.bar10), 0, 0.2).text;
  const n2 = await S2.reimport(W(dir, 'PK110_v1_move_stale.dxf', moveOnly));
  const bd = await S2.liveBounds(decoyId, m.products.bar10);
  const bb = await S2.liveBounds(bid2, m.products.bar10);
  check('v1 stale-session DXF: bridge moved by block-name GlobalId prefix, decoy untouched', appliedOf(n2) === 1 && Math.abs(centre(bd)[2] - 0.5) < 1e-6 && Math.abs(centre(bb)[2] - 0.7) < 1e-4, { n2, decoy: r6(centre(bd)), bridge: r6(centre(bb)) });
});

// =============================================================================================
await scenario('guid-safety', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '3GsfRev' });
  const decoy = bridge({ guidPrefix: '0GsfDec' });
  let d110;
  let d115;
  {
    const S = await open();
    const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'decoy.ifc', decoy.text), W(dir, 'bridge.ifc', m.text)]);
    const [decoyId, bid] = ids;
    d110 = sectionDxf(zip, '0+110');
    d115 = sectionDxf(zip, '0+115');
    // (a) tampered GlobalId in GUBIM_ORIG and block name -> decoy's GUID, same session.
    const tam = retargetGuid(moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text, m.products.bar10, m.guids.bar10, decoy.guids.bar10);
    const n = await S.reimport(W(dir, 'tampered.dxf', tam));
    const bb = await S.liveBounds(bid, m.products.bar10);
    const bd = await S.liveBounds(decoyId, m.products.bar10);
    check('GlobalId of another model: refused "GlobalId distinto", nothing moves', /GlobalId distinto/.test(n.join(' ')) && Math.abs(centre(bb)[2] - 0.5) < 1e-6 && Math.abs(centre(bd)[2] - 0.5) < 1e-6, { n, bridge: r6(centre(bb)), decoy: r6(centre(bd)) });
    await S.close();
  }
  const stale = W(dir, 'stale_PK110.dxf', moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text);
  // (b) real page reload: decoy primary, DXF from the previous session.
  {
    const S = await open();
    await S.loadIfc(W(dir, 'decoy2.ifc', decoy.text), 1);
    await S.loadIfc(W(dir, 'bridge2.ifc', m.text), 2);
    const [decoyId, bid] = await S.modelIds();
    await S.openClashTab();
    await S.configure({ modelA: decoyId, modelB: decoyId, classA: 'IfcReinforcingBar', classB: 'IfcTendon' });
    await S.runClash();
    await S.loadLandXml(W(dir, 'axis.xml', AXIS));
    const n = await S.reimport(stale);
    const bb = await S.liveBounds(bid, m.products.bar10);
    const bd = await S.liveBounds(decoyId, m.products.bar10);
    check('earlier-session DXF with the decoy as primary: bridge bar moved, decoy untouched', appliedOf(n) === 1 && Math.abs(centre(bb)[2] - 0.8) < 1e-4 && Math.abs(centre(bd)[2] - 0.5) < 1e-6, { n, bridge: r6(centre(bb)), decoy: r6(centre(bd)) });
    // Same stale DXF again: bridge already patched.
    const n2 = await S.reimport(stale);
    check('earlier-session DXF again: reported "ya aplicado" (decoy primary)', /ya aplicado/.test(n2.join(' ')), n2);
    await S.close();
  }
  // (c) only the decoy open.
  {
    const S = await open();
    await S.loadIfc(W(dir, 'decoy3.ifc', decoy.text), 1);
    const [decoyId] = await S.modelIds();
    await S.openClashTab();
    await S.configure({ modelA: decoyId, modelB: decoyId, classA: 'IfcReinforcingBar', classB: 'IfcTendon' });
    await S.runClash();
    await S.loadLandXml(W(dir, 'axis.xml', AXIS));
    const n = await S.reimport(stale);
    const bd = await S.liveBounds(decoyId, m.products.bar10);
    check('earlier-session DXF with only the decoy open: all refused, decoy untouched', appliedOf(n) === 0 && /GlobalId distinto/.test(n.join(' ')) && Math.abs(centre(bd)[2] - 0.5) < 1e-6, { n });
    // (d) no GlobalId anywhere (block GUBIM_A_<id>, ORIG without guid=), stale.
    const noGuid = moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text
      .replace(/ guid=\S+/g, '')
      .split(new RegExp(`GUBIM_A_${m.products.bar15}_[0-9A-Za-z_$]+`)).join(`GUBIM_A_${m.products.bar15}`);
    const n3 = await S.reimport(W(dir, 'noguid_stale.dxf', noGuid));
    const bd15 = await S.liveBounds(decoyId, m.products.bar15);
    check('earlier-session DXF without any GlobalId: refused "sin GlobalId", decoy untouched', appliedOf(n3) === 0 && /sin GlobalId/.test(n3.join(' ')) && Math.abs(centre(bd15)[2] - 0.5) < 1e-6, { n3 });
    await S.close();
  }
});

// =============================================================================================
await scenario('reasons-csv', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1RsnRev' });
  const decoy = bridge({ guidPrefix: '0RsnDec' });
  const S = await open();
  const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)]);
  // Merge three edits into the PK110 DXF is impossible (one bar per section), so use the
  // master-less individual: PK 0+110 bar10 move (applied) and a bad GlobalId copy for bar10?
  // Use PK 0+125 (L1 longitudinal) with a tampered GlobalId plus PK 0+105 bar5 NaN rebend in
  // separate files, then one file with an applied + skipped pair: PK 0+110 move + NaN polyline.
  const d110 = sectionDxf(zip, '0+110');
  const ins = inserts(d110).find((i) => blockOf(m.products.bar10)(i.block));
  const mixed = setEditPolyline(moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text, m.products.bar10, [[ins.x - 1.8, ins.y], [ins.x, NaN], [ins.x + 1.8, ins.y]], { lw: true });
  const n = await S.reimport(W(dir, 'mixed.dxf', mixed));
  check('mixed import: 1 applied, NaN rebend skipped "geometría no válida" (Spanish)', appliedOf(n) === 1 && /1 omitidos: 1 geometría no válida/.test(n.join(' ')), n);
  const csvPath = await S.downloadCsv();
  const csv = readFileSync(csvPath, 'utf8');
  writeFileSync(path.join(dir, 'audit.csv'), csv);
  const [hdr, ...rows] = csv.trim().split(/\r?\n/);
  const cols = hdr.split(',');
  const st = cols.indexOf('status');
  const rs = cols.indexOf('reason');
  const gi = cols.indexOf('global_id');
  const parsed = rows.map((r) => r.split(','));
  check('CSV has status, reason, global_id; rows match', st >= 0 && rs >= 0 && gi >= 0 && parsed.some((r) => r[2] === 'move' && r[st] === 'applied' && r[gi] === m.guids.bar10) && parsed.some((r) => r[2] === 'rebend' && r[st] === 'skipped' && r[rs] === 'invalid-points'), { hdr, rows });
  // All skipped: tampered GlobalId on L1 (PK 0+125).
  const d125 = sectionDxf(zip, '0+125');
  const tam = retargetGuid(moveInsert(d125, blockOf(m.products.L1), 0, 0.1).text, m.products.L1, m.guids.L1, decoy.guids.L1);
  const n2 = await S.reimport(W(dir, 'tampered_L1.dxf', tam));
  check('all-skipped import shows "No se aplicó ningún cambio ... GlobalId distinto"', /No se aplicó ningún cambio del DXF: 1 omitidos \(1 GlobalId distinto\)/.test(n2.join(' ')), n2);
  check('[ux] CSV of the all-skipped import is downloadable', await S.hasButton('Registro CSV'), {});
  // No edits at all.
  const n3 = await S.reimport(W(dir, 'unchanged.dxf', sectionDxf(zip, '0+115')));
  check('unchanged DXF: "no contiene desplazamientos"', /no contiene desplazamientos/.test(n3.join(' ')), n3);
  await S.shot(path.join(dir, '01-reasons.png'));
});

// =============================================================================================
await scenario('federated', async ({ dir, check, open }) => {
  // Left and right decks from the same template: same expressIds, other GlobalIds, R 6 m to +Y.
  const Lm = bridge({ guidPrefix: '1LftDeck' });
  const Rm = bridge({ guidPrefix: '2RgtDeck', yOffset: 6 });
  const lp = W(dir, 'left.ifc', Lm.text);
  const rp = W(dir, 'right.ifc', Rm.text);
  const S = await open();
  const { ids, cards, zip } = await bridgeFlow(S, dir, [lp, rp], { models: () => ({ a: '*', b: '*' }) });
  const [lid, rid] = ids;
  check('8 clashes over both decks', cards.length === 8, cards.length);
  const d105 = sectionDxf(zip, '0+105');
  writeFileSync(path.join(dir, 'PK105.dxf'), d105);
  const ins = inserts(d105).filter((i) => blockOf(Lm.products.bar5)(i.block));
  const meta = metaTexts(d105);
  const isL = (b) => b.includes('_' + Lm.guids.bar5.slice(0, 6));
  const isR = (b) => b.includes('_' + Rm.guids.bar5.slice(0, 6));
  check('PK 0+105 holds both decks\' bar5 (same expressId, two INSERTs)', ins.length === 2 && ins.some((i) => isL(i.block)) && ins.some((i) => isR(i.block)), { ins, orig: meta.filter((t) => t.startsWith('GUBIM_ORIG')) });
  const modelInMeta = meta.find((t) => t.includes(' model='))?.match(/model=(\S+)/)?.[1];
  check('[info] section metadata names one model', true, { modelInMeta, lid, rid });
  const z = async (mid) => centre(await S.liveBounds(mid, Lm.products.bar5))[2];
  // (a) move only the left deck's INSERT.
  const a = moveInsert(d105, isL, 0, 0.3);
  const na = await S.reimport(W(dir, 'a_moveL.dxf', a.text));
  const za = [await z(lid), await z(rid)];
  check('(a) move of the left INSERT: applied to the left bar or skipped with a reason, never silently dropped, right untouched', a.moved === 1 && Math.abs(za[1] - 0.5) < 1e-6 && ((appliedOf(na) === 1 && Math.abs(za[0] - 0.8) < 1e-4) || /omitidos/.test(na.join(' '))), { na, z: za });
  // (b) move only the right deck's INSERT.
  const b = moveInsert(d105, isR, 0, 0.2);
  const nb = await S.reimport(W(dir, 'b_moveR.dxf', b.text));
  const zb = [await z(lid), await z(rid)];
  check('(b) move of the right INSERT: never lands on the left bar', Math.abs(zb[0] - za[0]) < 1e-6 && (Math.abs(zb[1] - 0.7) < 1e-4 || /omitidos/.test(nb.join(' '))), { nb, z: zb });
  // (c) move the left INSERT and delete the right one (CAD user removes what they do not edit).
  const lines = dxfLines(a.text);
  const { ents, secStart, secEnd } = entities(lines);
  const us = units(ents).filter((u) => !(u.type === 'INSERT' && isR(u.parts[0].pairs.find((p) => p[0] === '2')[1].trim())));
  const c = rebuild(lines, us, secStart, secEnd).join('\r\n');
  const cPath = W(dir, 'c_moveL_deleteR.dxf', c);
  const nc = await S.reimport(cPath);
  const bl = await S.liveBounds(lid, Lm.products.bar5);
  const br = await S.liveBounds(rid, Lm.products.bar5);
  check('(c) left moved + right INSERT deleted: right bar not moved, left bar not moved by a bogus vector', Math.abs(centre(br)[1] - 2006) < 1e-4 && Math.abs(centre(bl)[1] - 2000) < 1e-4, { nc, left: r6(centre(bl)), right: r6(centre(br)) });
  await S.shot(path.join(dir, '01-federated.png'));
  await S.close();
  // (d) the same DXF (c) in a later session: both decks re-opened (new model ids).
  const S2 = await open();
  await S2.loadIfc(lp, 1);
  await S2.loadIfc(rp, 2);
  const [lid2, rid2] = await S2.modelIds();
  await S2.openClashTab();
  await S2.configure({ modelA: '*', modelB: '*', classA: 'IfcReinforcingBar', classB: 'IfcTendon' });
  await S2.runClash();
  await S2.loadLandXml(W(dir, 'axis2.xml', AXIS));
  const nd = await S2.reimport(cPath);
  const bl2 = await S2.liveBounds(lid2, Lm.products.bar5);
  const br2 = await S2.liveBounds(rid2, Lm.products.bar5);
  check('(d) later session, DXF (c): right bar not moved by the left bar\'s edit', near(centre(br2), [1005, 2006, 0.5], 1e-4), { nd, left: r6(centre(bl2)), right: r6(centre(br2)) });
  await S2.shot(path.join(dir, '02-federated-stale.png'));
});

// =============================================================================================
await scenario('cad-resave', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1CadRev' });
  const S = await open();
  const { ids, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)]);
  const bid = ids[0];
  const d110 = sectionDxf(zip, '0+110');
  const base = moveInsert(d110, blockOf(m.products.bar10), 0, 0.3).text;
  const zc = async (key = 'bar10') => centre(await S.liveBounds(bid, m.products[key]));
  // v0: LF line ends + right-aligned group codes ("  0", " 10").
  const v0 = dxfLines(base).map((l, i) => (i % 2 === 0 && /^\d+$/.test(l) ? l.padStart(3, ' ') : l)).join('\n');
  const n0 = await S.reimport(W(dir, 'v0_lf_spaces.dxf', v0));
  check('LF + padded group codes: move applied', appliedOf(n0) === 1 && Math.abs((await zc())[2] - 0.8) < 1e-4, n0);
  // v1: entities reversed (POLYLINE runs kept together).
  {
    const lines = dxfLines(base);
    const { ents, secStart, secEnd } = entities(lines);
    const v1 = rebuild(lines, units(ents).reverse(), secStart, secEnd).join('\r\n');
    const n = await S.reimport(W(dir, 'v1_reversed.dxf', v1));
    check('reversed entity order: same edit -> "ya aplicado"', /ya aplicado/.test(n.join(' ')) && Math.abs((await zc())[2] - 0.8) < 1e-4, n);
  }
  // v2: every coordinate in exponent notation.
  {
    const lines = dxfLines(base);
    for (let i = 0; i + 1 < lines.length; i += 2) if (/^\s*(10|20|30|11|21|31|40)$/.test(lines[i])) lines[i + 1] = Number(lines[i + 1]).toExponential(12).toUpperCase();
    const n = await S.reimport(W(dir, 'v2_exponent.dxf', lines.join('\r\n')));
    check('exponent number format: "ya aplicado"', /ya aplicado/.test(n.join(' ')) && Math.abs((await zc())[2] - 0.8) < 1e-4, n);
  }
  // v3: 999 comments stripped.
  {
    const lines = dxfLines(base);
    const out = [];
    for (let i = 0; i < lines.length; i += 2) if (lines[i].trim() !== '999') out.push(lines[i], lines[i + 1]);
    const n = await S.reimport(W(dir, 'v3_no999.dxf', out.join('\r\n')));
    check('999 comments stripped: "ya aplicado"', /ya aplicado/.test(n.join(' ')), n);
  }
  // v4: every LINE/CIRCLE deleted, EDIT polyline converted to LWPOLYLINE (unchanged points).
  {
    const lines = dxfLines(base);
    const { ents, secStart, secEnd } = entities(lines);
    const kept = units(ents).filter((u) => !['LINE', 'CIRCLE'].includes(u.type));
    let v4 = rebuild(lines, kept, secStart, secEnd).join('\r\n');
    v4 = setEditPolyline(v4, m.products.bar10, editPolyline(base, m.products.bar10), { lw: true });
    const n = await S.reimport(W(dir, 'v4_deleted_lw.dxf', v4));
    check('geometry deleted + LWPOLYLINE: "ya aplicado", no rebend', /ya aplicado/.test(n.join(' ')) && Math.abs((await S.liveBounds(bid, m.products.bar10)).hi[2] - 0.81) < 1e-3, n);
  }
  // v5: comma decimal in a moved INSERT x (locale-broken writer), bar15.
  {
    const d115 = sectionDxf(zip, '0+115');
    const lines = dxfLines(moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text);
    const { ents } = entities(lines);
    const ins = ents.find((e) => e.type === 'INSERT' && blockOf(m.products.bar15)(e.pairs.find((p) => p[0] === '2')[1].trim()));
    for (let p = ins.start + 2; p < ins.end; p += 2) if (lines[p].trim() === '10') lines[p + 1] = lines[p + 1].replace('.', ',');
    const n = await S.reimport(W(dir, 'v5_comma.dxf', lines.join('\r\n')));
    const c = await zc('bar15');
    check('unparseable INSERT x ("1,5"): refused, bar15 not moved sideways', Math.abs(c[1] - 2000) < 1e-3, { n, c: r6(c) });
  }
  // v6: duplicated INSERT (CAD COPY) of bar5, copy 2 m to the right, original untouched.
  {
    const d105 = sectionDxf(zip, '0+105');
    const lines = dxfLines(d105);
    const { ents, secStart, secEnd } = entities(lines);
    const us = units(ents);
    const orig = us.find((u) => u.type === 'INSERT' && blockOf(m.products.bar5)(u.parts[0].pairs.find((p) => p[0] === '2')[1].trim()));
    const copyLines = lines.slice(orig.parts[0].start, orig.parts[0].end).map((l, i, arr) => (i > 0 && arr[i - 1].trim() === '10' ? (Number(l) + 2).toFixed(6) : l));
    const fakeLines = [...lines.slice(0, secEnd), ...copyLines, ...lines.slice(secEnd)];
    const n = await S.reimport(W(dir, 'v6_copied_insert.dxf', fakeLines.join('\r\n')));
    const c = await zc('bar5');
    check('copied INSERT (original left in place): bar5 not moved, or refused as ambiguous', Math.abs(c[1] - 2000) < 1e-3, { n, c: r6(c) });
  }
  // v7: zero-length edit polyline (both vertices snapped to one point) on bar15.
  {
    const d115 = sectionDxf(zip, '0+115');
    const i15 = inserts(d115).find((i) => blockOf(m.products.bar15)(i.block));
    const n = await S.reimport(W(dir, 'v7_zero_length.dxf', setEditPolyline(d115, m.products.bar15, [[i15.x, i15.y], [i15.x, i15.y]])));
    const b = await S.liveBounds(bid, m.products.bar15);
    check('zero-length edit polyline: refused, bar15 keeps a mesh', !!b && b.n > 0, { n, mesh: b ? b.n : null });
  }
  await S.shot(path.join(dir, '01-cad.png'));
});

// =============================================================================================
await scenario('tendon-two-sections', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1TndRev' });
  const S = await open();
  const { ids, cards, zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)], { classA: 'IfcTendon', classB: 'IfcReinforcingBar' });
  const bid = ids[0];
  check('tendons vs bars: 4 clashes', cards.length === 4, cards.length);
  const secs = [...zip.keys()].filter((k) => k.startsWith('individual/'));
  // T2 (transverse tendon) rebend at PK 0+125.
  const d125 = sectionDxf(zip, '0+125');
  const it2 = inserts(d125).find((i) => blockOf(m.products.t2)(i.block));
  const poly = editPolyline(d125, m.products.t2);
  check('transverse tendon T2 has an EDIT_A polyline', !!poly, { poly, secs });
  if (poly) {
    const V = [[it2.x - 1.8, it2.y], [it2.x, it2.y + 0.15], [it2.x + 1.8, it2.y]];
    const n = await S.reimport(W(dir, 'T2_V.dxf', setEditPolyline(d125, m.products.t2, V)));
    const b = await S.liveBounds(bid, m.products.t2);
    check('tendon rebend applied, radius 0.04 kept', appliedOf(n) === 1 && b && Math.abs(b.hi[0] - b.lo[0] - 0.08) < 0.003 && Math.abs(b.hi[2] - (0.45 + 0.04)) < 0.008, { n, lo: r6(b?.lo), hi: r6(b?.hi) });
  }
  // T1 (longitudinal) is CLASH_A in three sections; edit it differently in two of them.
  const inT1 = secs.filter((k) => inserts(dec(zip.get(k))).some((i) => blockOf(m.products.tendon)(i.block)));
  check('T1 appears as CLASH_A in 3 sections', inT1.length === 3, inT1);
  const d105 = sectionDxf(zip, '0+105');
  const d110 = sectionDxf(zip, '0+110');
  const n1 = await S.reimport(W(dir, 'T1_105_up010.dxf', moveInsert(d105, blockOf(m.products.tendon), 0, 0.1).text));
  const n2 = await S.reimport(W(dir, 'T1_110_up020.dxf', moveInsert(d110, blockOf(m.products.tendon), 0, 0.2).text));
  const c = centre(await S.liveBounds(bid, m.products.tendon));
  check('[observation] T1 edited +0.10 in PK105 and +0.20 in PK110', true, { n1, n2, z: +c[2].toFixed(6), note: 'conflicting edits of one element from two sections' });
  check('two sections editing T1 differently: not silently summed (+0.30)', Math.abs(c[2] - 0.8) > 1e-3, { z: c[2] });
});

// =============================================================================================
await scenario('patched-fresh', async ({ dir, check, open }) => {
  const m = bridge({ guidPrefix: '1FrsRev' });
  let patched;
  let edited;
  {
    const S = await open();
    const { zip } = await bridgeFlow(S, dir, [W(dir, 'bridge.ifc', m.text)]);
    edited = W(dir, 'PK110_m030.dxf', moveInsert(sectionDxf(zip, '0+110'), blockOf(m.products.bar10), 0, 0.3).text);
    const d105 = sectionDxf(zip, '0+105');
    const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
    await S.reimport(edited);
    await S.reimport(W(dir, 'PK105_V.dxf', setEditPolyline(d105, m.products.bar5, [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]])));
    patched = readFileSync(await S.downloadPatched(), 'utf8');
    writeFileSync(path.join(dir, 'review_bridge_patched.ifc'), patched);
    await S.close();
  }
  // STEP diff.
  const origLines = m.text.split('\r\n');
  const patLines = patched.split(/\r?\n/);
  const pl = new Set(patLines);
  const removed = origLines.filter((l) => !pl.has(l));
  const ol = new Set(origLines);
  const addedL = patLines.filter((l) => !ol.has(l));
  writeFileSync(path.join(dir, 'step.diff'), ['--- removed', ...removed, '+++ added', ...addedL].join('\n'));
  const changedIds = removed.filter((l) => l.startsWith('#')).map((l) => Number(l.match(/^#(\d+)=/)[1])).sort((a, b) => a - b);
  const strip = (l, i) => l.split(',').filter((_, k) => k !== i).join(',');
  const onlyAttr = removed.filter((l) => l.startsWith('#')).every((l) => {
    const id = l.match(/^#(\d+)=/)[1];
    const nl = patLines.find((x) => x.startsWith(`#${id}=`));
    return nl && (strip(l, 5) === strip(nl, 5) || strip(l, 6) === strip(nl, 6));
  });
  const maxOrig = Math.max(...origLines.filter((l) => l.startsWith('#')).map((l) => Number(l.match(/^#(\d+)=/)[1])));
  const newOk = addedL.filter((l) => l.startsWith('#')).every((l) => Number(l.match(/^#(\d+)=/)[1]) > maxOrig || changedIds.includes(Number(l.match(/^#(\d+)=/)[1])));
  check('STEP diff: only bar5 and bar10 records changed (one attribute each), new ids appended', JSON.stringify(changedIds) === JSON.stringify([m.products.bar5, m.products.bar10].sort((a, b) => a - b)) && onlyAttr && newOk && removed.every((l) => l.startsWith('#')), { changedIds, removedNonEntity: removed.filter((l) => !l.startsWith('#')) });
  // Fresh session: open the patched file.
  const S = await open();
  const pp = W(dir, 'reopen_patched.ifc', patched);
  await S.loadIfc(pp, 1);
  const [pid] = await S.modelIds();
  const c10 = centre(await S.liveBounds(pid, m.products.bar10));
  const b5 = await S.liveBounds(pid, m.products.bar5);
  check('reopened patched IFC: bar10 +0.30, bar5 bent', Math.abs(c10[2] - 0.8) < 1e-4 && Math.abs(b5.lo[2] - 0.19) < 0.006, { c10: r6(c10), b5lo: r6(b5.lo) });
  await S.openClashTab();
  await S.configure({ modelA: pid, modelB: pid, classA: 'IfcReinforcingBar', classB: 'IfcTendon' });
  const cards = await S.runClash();
  check('reopened patched IFC: 2 clashes (bar15, L1)', cards.length === 2, cards);
  await S.loadLandXml(W(dir, 'axis.xml', AXIS));
  const n = await S.reimport(edited);
  check('old DXF into the reopened patched file: "ya aplicado"', /ya aplicado/.test(n.join(' ')) && Math.abs(centre(await S.liveBounds(pid, m.products.bar10))[2] - 0.8) < 1e-4, n);
  // Fresh export from the patched model; move bar15 and re-import.
  const { zip } = await S.exportZip();
  const d115 = sectionDxf(zip, '0+115');
  const n2 = await S.reimport(W(dir, 'fresh_PK115.dxf', moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text));
  const c15 = centre(await S.liveBounds(pid, m.products.bar15));
  check('new export from the patched file, bar15 moved: applied, 1 clash left', appliedOf(n2) === 1 && remainingOf(n2) === 1 && Math.abs(c15[2] - 0.8) < 1e-4, { n2, c15: r6(c15) });
  await S.shot(path.join(dir, '01-patched-fresh.png'));
});

// =============================================================================================
writeFileSync(path.join(root, 'results.json'), JSON.stringify(results, null, 1));
console.log(`\n=== ${mode} summary`);
for (const r of results) console.log(`${r.ok ? 'PASS' : 'FAIL'} ${r.name} (${r.checks.filter((c) => c.ok).length}/${r.checks.length}, ${r.secs}s)`);
process.exit(results.every((r) => r.ok) ? 0 : 1);
