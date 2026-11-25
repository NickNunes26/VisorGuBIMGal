// Direct kernel probes (NOT the UI): Model.applyClashDxf on DXFs the UI exported.
// Usage: node probe-node.mjs <scenarioDir with bridge.ifc + downloads/*.zip>
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { bridge } from './gen.mjs';
import { nodeModel, nodeBounds, centre, r6, readZip, dxfLines, entities, units, rebuild, inserts, moveInsert, blockOf, editPolyline, setEditPolyline } from './lib.mjs';

const dir = process.argv[2];
const m = bridge({ guidPrefix: '1CadRev' });
const ifc = readFileSync(path.join(dir, 'bridge.ifc'), 'utf8');
if (ifc !== m.text) throw new Error('bridge.ifc differs from generator (guidPrefix)');
const dlDir = readdirSync(dir).find((n) => n.startsWith('dl-') || n === 'downloads');
const zipName = readdirSync(path.join(dir, dlDir)).find((n) => n.endsWith('.zip'));
const zip = readZip(readFileSync(path.join(dir, dlDir, zipName)));
const sec = (pk) => new TextDecoder().decode(zip.get([...zip.keys()].find((k) => k.startsWith('individual/') && k.includes(pk))));
const out = [];
function apply(label, dxf, requireGuid = false) {
  const model = nodeModel(ifc);
  let res;
  try {
    // requireGuid=false: apply as the model the DXF was exported from; true: as an unrelated
    // open model, so only GlobalId matches land.
    const owner = /GUBIM_(?:ORIG|META) [^\r\n]*?\bmodel=(\S+)/.exec(dxf)?.[1] ?? '';
    const self = requireGuid ? '__probe_other__' : owner;
    res = model.applyClashDxf(new TextEncoder().encode(dxf), self, [self], new Uint32Array(), 'add', Date.now());
  } catch (e) {
    out.push({ label, error: String(e) });
    console.log(label, 'ERROR', String(e));
    return null;
  }
  const json = JSON.parse(res.diffJson());
  const patched = res.patchedIfcBytes();
  const pm = nodeModel(patched);
  const c = {};
  for (const k of ['bar5', 'bar10', 'bar15']) {
    const b = nodeBounds(pm, m.products[k]);
    c[k] = b ? { c: r6(centre(b)), nan: b.nan, size: r6(b.hi.map((h, i) => h - b.lo[i])) } : null;
  }
  const summary = { label, applied: json.applied, skipped: json.skipped, edits: json.edits.map((e) => ({ kind: e.kind, id: e.expressId, status: e.status, reason: e.reason, d2: e.delta2d, n: e.points3d?.length })), geom: c };
  out.push(summary);
  console.log(JSON.stringify(summary));
  return { json, patched: new TextDecoder().decode(patched) };
}
const d105 = sec('0+105');
const d115 = sec('0+115');
const i5 = inserts(d105).find((i) => blockOf(m.products.bar5)(i.block));
const setInsertField = (text, id, code, value) => {
  const lines = dxfLines(text);
  const { ents } = entities(lines);
  const e = ents.find((x) => x.type === 'INSERT' && blockOf(id)(x.pairs.find((p) => p[0] === '2')[1].trim()));
  let found = false;
  for (let p = e.start + 2; p < e.end; p += 2) if (lines[p].trim() === code) { lines[p + 1] = value; found = true; }
  if (!found) lines.splice(e.end, 0, code, value);
  return lines.join('\r\n');
};
// 1. Comma decimal on a moved INSERT x.
apply('comma-decimal x', setInsertField(moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text, m.products.bar15, '10', inserts(d115).find((i) => blockOf(m.products.bar15)(i.block)).x.toFixed(6).replace('.', ',')));
// 2. Empty y value.
apply('empty INSERT y', setInsertField(d115, m.products.bar15, '20', ''));
// 3. Huge move.
apply('huge move 1e7', moveInsert(d115, blockOf(m.products.bar15), 1e7, 0).text);
apply('huge move 1e300', moveInsert(d115, blockOf(m.products.bar15), 0, 1e300).text);
// 4. inf / NaN in INSERT.
apply('INSERT y=inf', setInsertField(d115, m.products.bar15, '20', 'inf'));
apply('INSERT y=NaN', setInsertField(d115, m.products.bar15, '20', 'NaN'));
// 5. Rotation / mirror of the INSERT about another point (INSERT also translated).
apply('INSERT rotated 90 (50=90) + moved', setInsertField(moveInsert(d115, blockOf(m.products.bar15), 0.5, 0).text, m.products.bar15, '50', '90.0'));
apply('INSERT mirrored (41=-1)', setInsertField(d115, m.products.bar15, '41', '-1.0'));
// 6. Edit polylines: 1 point, repeated vertex, inf, zigzag back on itself.
const lw = { lw: true };
apply('polyline 1 point (LW)', setEditPolyline(d105, m.products.bar5, [[i5.x, i5.y - 0.3]], lw));
apply('polyline 1 point (R12)', setEditPolyline(d105, m.products.bar5, [[i5.x, i5.y - 0.3]]));
apply('polyline repeated vertex', setEditPolyline(d105, m.products.bar5, [[i5.x - 1.8, i5.y], [i5.x, i5.y - 0.3], [i5.x, i5.y - 0.3], [i5.x + 1.8, i5.y]]));
apply('polyline inf', setEditPolyline(d105, m.products.bar5, [[i5.x - 1.8, i5.y], [i5.x, Infinity], [i5.x + 1.8, i5.y]], { lw: true, fmt: (v) => (Number.isFinite(v) ? String(v) : 'inf') }));
apply('polyline 180deg fold back', setEditPolyline(d105, m.products.bar5, [[i5.x - 1.8, i5.y], [i5.x + 1.8, i5.y], [i5.x - 1.0, i5.y]]));
apply('polyline coincident endpoints only', setEditPolyline(d105, m.products.bar5, [[i5.x, i5.y], [i5.x, i5.y]]));
// 7. Copied INSERT appended (CAD COPY), original unchanged.
{
  const lines = dxfLines(d105);
  const { ents, secEnd } = entities(lines);
  const e = ents.find((x) => x.type === 'INSERT' && blockOf(m.products.bar5)(x.pairs.find((p) => p[0] === '2')[1].trim()));
  const copy = lines.slice(e.start, e.end).map((l, i, a) => (i > 0 && a[i - 1].trim() === '10' ? (Number(l) + 2).toFixed(6) : l));
  apply('copied INSERT appended (+2 m)', [...lines.slice(0, secEnd), ...copy, ...lines.slice(secEnd)].join('\r\n'));
  apply('copied INSERT prepended (+2 m)', (() => { const first = ents[0].start; return [...lines.slice(0, first), ...copy, ...lines.slice(first)].join('\r\n'); })());
}
// 8. GUBIM_ORIG text of the moved element deleted (user cleaned the locked layer).
{
  const lines = dxfLines(moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text);
  const { ents, secStart, secEnd } = entities(lines);
  const kept = units(ents).filter((u) => !(u.type === 'TEXT' && /^GUBIM_ORIG /.test(u.parts[0].pairs.find((p) => p[0] === '1')?.[1] ?? '')));
  apply('GUBIM_ORIG deleted', rebuild(lines, kept, secStart, secEnd).join('\r\n'));
}
// 9. Mac CR-only line endings.
apply('CR-only line ends', moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text.replace(/\r\n/g, '\r'));
// 10. UTF-16 / BOM.
apply('UTF-8 BOM', '﻿' + moveInsert(d115, blockOf(m.products.bar15), 0, 0.3).text);
writeFileSync(path.join(dir, 'probe-node.json'), JSON.stringify(out, null, 1));
