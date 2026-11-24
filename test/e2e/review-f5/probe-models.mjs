import { bridge, gridModel, linearModel } from './gen.mjs';
import { nodeModel, nodeBounds, centre, r6, REPO } from './lib.mjs';
for (const [label, m] of [
  ['IFC4', bridge()], ['IFC2X3', bridge({ schema: 'IFC2X3' })], ['IFC4X3', bridge({ schema: 'IFC4X3_ADD2' })],
  ['UTM', bridge({ origin: [540000, 4700000, 0] })], ['mm', bridge({ unit: 'mm' })], ['grid43', gridModel()], ['grid4', gridModel({ schema: 'IFC4' })]
]) {
  const nm = nodeModel(m.text);
  const out = {};
  for (const [k, id] of Object.entries(m.products)) { const b = nodeBounds(nm, id); out[k] = b ? { c: r6(centre(b)), size: r6(b.hi.map((h, i) => h - b.lo[i])) } : null; }
  const sum = JSON.parse(nm.summaryJson());
  console.log(label, JSON.stringify(out));
  console.log('  diag', JSON.stringify(sum.diagnostics ?? sum.warnings ?? Object.keys(sum)).slice(0, 300));
}
const lin = linearModel(REPO + '/test/fixtures/authored/linear-placement.ifc');
const nm = nodeModel(lin.text);
for (const [k, id] of Object.entries(lin.products)) { const b = nodeBounds(nm, id); console.log('LP', k, b && r6(centre(b)), b && r6(b.hi.map((h, i) => h - b.lo[i]))); }
