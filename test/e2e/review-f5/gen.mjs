// Independent IFC + LandXML generators for the F5 review. Every expected value used by the
// scenarios is derived from the numbers written here, never from the app's output.
import { readFileSync } from 'node:fs';

const B64 = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$';

/** Builder for STEP DATA lines with sequential ids. */
class Step {
  constructor(start = 1) {
    this.id = start - 1;
    this.lines = [];
  }
  e(s) {
    this.id++;
    this.lines.push(`#${this.id}=${s};`);
    return this.id;
  }
}

function guidMaker(prefix) {
  let g = 0;
  return () => {
    g++;
    let s = prefix;
    let n = g;
    while (s.length < 22) {
      s += B64[n % 64];
      n = Math.floor(n / 64);
    }
    return s.slice(0, 22);
  };
}

const num = (v) => {
  const s = Number(v).toFixed(6);
  return s.includes('.') ? s : s + '.';
};

/**
 * Bridge deck model.
 * opts.schema: IFC2X3 | IFC4 | IFC4X3_ADD2; opts.origin: site location in metres;
 * opts.unit: 'm' | 'mm'; opts.guidPrefix; opts.yOffset (whole deck shifted laterally, metres).
 *
 * World geometry (metres, before site origin; storey at site):
 *   deck slab x 0..30, y -2..2, z 0..1          kerb proxy x 0..30, y 1.2..1.6, z 1.0..1.4
 *   tendon T1 along X at y=0, z=0.5, r=0.04     bar5  at x=5, along Y -1.8..1.8, z=0.5, r=0.01
 *   bar10 same at x=10 (parent tilted)          bar15 same at x=15 (parent rotated about Z)
 *   bar20 at x=20, z=0.8 (clear)                L1 longitudinal x 22..28, y=0, z=0.3, r=0.01
 *   T2 transverse tendon at x=25, along Y, z=0.3, r=0.04 (clashes L1)
 */
export function bridge(opts = {}) {
  const schema = opts.schema ?? 'IFC4';
  const origin = opts.origin ?? [1000, 2000, 0];
  const k = opts.unit === 'mm' ? 1000 : 1;
  const yo = opts.yOffset ?? 0;
  const guid = guidMaker(opts.guidPrefix ?? '3RevF5');
  const S = new Step();
  const e = (s) => S.e(s);
  const L = (v) => num(v * k);
  const pt = (x, y, z) => e(`IFCCARTESIANPOINT((${L(x)},${L(y)},${L(z)}))`);
  const pt2 = (x, y) => e(`IFCCARTESIANPOINT((${L(x)},${L(y)}))`);
  const dir = (x, y, z) => e(`IFCDIRECTION((${num(x)},${num(y)},${num(z)}))`);
  const ax = (p, z, x) => e(`IFCAXIS2PLACEMENT3D(#${p},${z ? '#' + z : '$'},${x ? '#' + x : '$'})`);
  const lp = (rel, a) => e(`IFCLOCALPLACEMENT(${rel ? '#' + rel : '$'},#${a})`);
  const is2x3 = schema === 'IFC2X3';

  const lenUnit = opts.unit === 'mm' ? 'IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)' : 'IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)';
  const ua = e(`IFCUNITASSIGNMENT((#${e(lenUnit)},#${e('IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)')}))`);
  const ctx = e(`IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#${ax(pt(0, 0, 0), null, null)},$)`);
  const axisCtx = e(`IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Axis','Model',*,*,*,*,#${ctx},$,.GRAPH_VIEW.,$)`);
  const body = e(`IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#${ctx},$,.MODEL_VIEW.,$)`);
  const project = e(`IFCPROJECT('${guid()}',$,'F5 review',$,$,$,$,(#${ctx}),#${ua})`);
  const sitePl = lp(null, ax(pt(origin[0], origin[1] + yo, origin[2]), null, null));
  const site = e(`IFCSITE('${guid()}',$,'Site',$,$,#${sitePl},$,$,.ELEMENT.,$,$,$,$,$)`);
  const bldPl = lp(sitePl, ax(pt(0, 0, 0), null, null));
  const bld = e(`IFCBUILDING('${guid()}',$,'Bridge',$,$,#${bldPl},$,$,.ELEMENT.,$,$,$)`);
  const stPl = lp(bldPl, ax(pt(0, 0, 0), null, null));
  const storey = e(`IFCBUILDINGSTOREY('${guid()}',$,'Deck',$,$,#${stPl},$,$,.ELEMENT.,0.)`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${project},(#${site}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${site},(#${bld}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${bld},(#${storey}))`);

  const shape = (item, kind, axisItem) => {
    const reps = [e(`IFCSHAPEREPRESENTATION(#${body},'Body','${kind}',(#${item}))`)];
    if (axisItem) reps.push(e(`IFCSHAPEREPRESENTATION(#${axisCtx},'Axis','Curve3D',(#${axisItem}))`));
    return e(`IFCPRODUCTDEFINITIONSHAPE($,$,(${reps.map((r) => '#' + r).join(',')}))`);
  };
  const params = is2x3 ? '0.,1.' : '$,$';
  const polyline = (a, b) => e(`IFCPOLYLINE((#${pt(...a)},#${pt(...b)}))`);
  const disk = (a, b, r) => {
    const pl = polyline(a, b);
    return { solid: e(`IFCSWEPTDISKSOLID(#${pl},${L(r)},$,${params})`), curve: pl };
  };
  const box = (cx, cy, dx, dy, z0, h) => {
    const prof = e(`IFCRECTANGLEPROFILEDEF(.AREA.,$,#${e(`IFCAXIS2PLACEMENT2D(#${pt2(cx, cy)},$)`)},${L(dx)},${L(dy)})`);
    return e(`IFCEXTRUDEDAREASOLID(#${prof},#${ax(pt(0, 0, z0), null, null)},#${dir(0, 0, 1)},${L(h)})`);
  };
  const products = {};
  const guids = {};
  const g = (key) => (guids[key] = guid());
  products.deck = e(`IFCSLAB('${g('deck')}',$,'Deck slab',$,$,#${lp(stPl, ax(pt(0, 0, 0), null, null))},#${shape(box(15, 0, 30, 4, 0, 1), 'SweptSolid')},$,.FLOOR.)`);
  products.kerb = e(`IFCBUILDINGELEMENTPROXY('${g('kerb')}',$,'Kerb',$,$,#${lp(stPl, ax(pt(0, 0, 0), null, null))},#${shape(box(15, 1.4, 30, 0.4, 1, 0.4), 'SweptSolid')},$,$)`);
  const t1 = disk([0, 0, 0], [30, 0, 0], 0.04);
  products.tendon = e(`IFCTENDON('${g('tendon')}',$,'Tendon T1',$,$,#${lp(stPl, ax(pt(0, 0, 0.5), null, null))},#${shape(t1.solid, 'AdvancedSweptSolid')},$,$,.STRAND.,${L(0.08)},$,$,$,$,$,$)`);
  const t2 = disk([0, -1.8, 0], [0, 1.8, 0], 0.04);
  products.t2 = e(`IFCTENDON('${g('t2')}',$,'Tendon T2 transverse',$,$,#${lp(stPl, ax(pt(25, 0, 0.3), null, null))},#${shape(t2.solid, 'AdvancedSweptSolid')},$,$,.STRAND.,${L(0.08)},$,$,$,$,$,$)`);
  const role = is2x3 ? '.LIGATURE.' : '.LIGATURE.';
  const bar = (key, name, placement, a, b, withAxis) => {
    const d = disk(a, b, 0.01);
    const axisItem = withAxis ? polyline(a, b) : null;
    products[key] = e(`IFCREINFORCINGBAR('${g(key)}',$,'${name}',$,$,#${placement},#${shape(d.solid, 'AdvancedSweptSolid', axisItem)},$,$,${L(0.02)},${L(0.000314)},$,${role},$)`);
  };
  bar('bar5', 'Stirrup x5', lp(stPl, ax(pt(5, 0, 0.5), null, null)), [0, -1.8, 0], [0, 1.8, 0], true);
  const tilt = lp(stPl, ax(pt(10, 0, 0.5), dir(1, 0, 0), dir(0, 1, 0)));
  bar('bar10', 'Stirrup x10 tilted', lp(tilt, ax(pt(0, 0, 0), null, null)), [-1.8, 0, 0], [1.8, 0, 0]);
  const rot = lp(stPl, ax(pt(15, 0, 0), dir(0, 0, 1), dir(0, 1, 0)));
  bar('bar15', 'Stirrup x15 rotZ', lp(rot, ax(pt(0, 0, 0.5), null, null)), [-1.8, 0, 0], [1.8, 0, 0]);
  bar('bar20', 'Stirrup x20 clear', lp(stPl, ax(pt(20, 0, 0.8), null, null)), [0, -1.8, 0], [0, 1.8, 0]);
  bar('L1', 'Longitudinal L1', lp(stPl, ax(pt(22, 0, 0.3), null, null)), [0, 0, 0], [6, 0, 0]);
  e(`IFCRELCONTAINEDINSPATIALSTRUCTURE('${guid()}',$,$,$,(${Object.values(products).map((p) => '#' + p).join(',')}),#${storey})`);

  const text = wrap(schema, 'review_bridge.ifc', S.lines);
  // World centres (metres) of the bars' directrix bounding boxes.
  const ox = origin[0];
  const oy = origin[1] + yo;
  const oz = origin[2];
  const centres = {
    bar5: [ox + 5, oy, oz + 0.5],
    bar10: [ox + 10, oy, oz + 0.5],
    bar15: [ox + 15, oy, oz + 0.5],
    bar20: [ox + 20, oy, oz + 0.8],
    L1: [ox + 25, oy, oz + 0.3],
    t2: [ox + 25, oy, oz + 0.3],
    tendon: [ox + 15, oy, oz + 0.5]
  };
  return { text, products, guids, centres, ids: { body, axisCtx, ctx }, schema, k };
}

export function wrap(schema, name, lines) {
  return [
    'ISO-10303-21;',
    'HEADER;',
    "FILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');",
    `FILE_NAME('${name}','2026-11-24T00:00:00',(''),(''),'gen','gen','');`,
    `FILE_SCHEMA(('${schema}'));`,
    'ENDSEC;',
    'DATA;',
    ...lines,
    'ENDSEC;',
    'END-ISO-10303-21;',
    ''
  ].join('\r\n');
}

/**
 * Grid model: deck + tendon T1 along X + one stirrup placed with IfcGridPlacement on a grid
 * rotated 30 deg about Z at (x=8,y=0). The stirrup runs along world Y at x=8, z=0.5.
 */
export function gridModel(opts = {}) {
  const schema = opts.schema ?? 'IFC4X3_ADD2';
  const origin = opts.origin ?? [1000, 2000, 0];
  const guid = guidMaker(opts.guidPrefix ?? '1GridF5');
  const S = new Step();
  const e = (s) => S.e(s);
  const pt = (x, y, z) => e(`IFCCARTESIANPOINT((${num(x)},${num(y)},${num(z)}))`);
  const pt2 = (x, y) => e(`IFCCARTESIANPOINT((${num(x)},${num(y)}))`);
  const dir = (x, y, z) => e(`IFCDIRECTION((${num(x)},${num(y)},${num(z)}))`);
  const ax = (p, z, x) => e(`IFCAXIS2PLACEMENT3D(#${p},${z ? '#' + z : '$'},${x ? '#' + x : '$'})`);
  const lp = (rel, a) => e(`IFCLOCALPLACEMENT(${rel ? '#' + rel : '$'},#${a})`);
  const ua = e(`IFCUNITASSIGNMENT((#${e('IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)')}))`);
  const ctx = e(`IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#${ax(pt(0, 0, 0), null, null)},$)`);
  const body = e(`IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#${ctx},$,.MODEL_VIEW.,$)`);
  const project = e(`IFCPROJECT('${guid()}',$,'F5 grid',$,$,$,$,(#${ctx}),#${ua})`);
  const sitePl = lp(null, ax(pt(...origin), null, null));
  const site = e(`IFCSITE('${guid()}',$,'Site',$,$,#${sitePl},$,$,.ELEMENT.,$,$,$,$,$)`);
  const bldPl = lp(sitePl, ax(pt(0, 0, 0), null, null));
  const bld = e(`IFCBUILDING('${guid()}',$,'Bridge',$,$,#${bldPl},$,$,.ELEMENT.,$,$,$)`);
  const stPl = lp(bldPl, ax(pt(0, 0, 0), null, null));
  const storey = e(`IFCBUILDINGSTOREY('${guid()}',$,'Deck',$,$,#${stPl},$,$,.ELEMENT.,0.)`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${project},(#${site}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${site},(#${bld}))`);
  e(`IFCRELAGGREGATES('${guid()}',$,$,$,#${bld},(#${storey}))`);
  const shape = (item, kind) => e(`IFCPRODUCTDEFINITIONSHAPE($,$,(#${e(`IFCSHAPEREPRESENTATION(#${body},'Body','${kind}',(#${item}))`)}))`);
  const disk = (a, b, r) => e(`IFCSWEPTDISKSOLID(#${e(`IFCPOLYLINE((#${pt(...a)},#${pt(...b)}))`)},${num(r)},$,$,$)`);
  const products = {};
  const guids = {};
  const g = (key) => (guids[key] = guid());
  const prof = e(`IFCRECTANGLEPROFILEDEF(.AREA.,$,#${e(`IFCAXIS2PLACEMENT2D(#${pt2(15, 0)},$)`)},30.,4.)`);
  const ext = e(`IFCEXTRUDEDAREASOLID(#${prof},#${ax(pt(0, 0, 0), null, null)},#${dir(0, 0, 1)},1.)`);
  products.deck = e(`IFCSLAB('${g('deck')}',$,'Deck slab',$,$,#${lp(stPl, ax(pt(0, 0, 0), null, null))},#${shape(ext, 'SweptSolid')},$,.FLOOR.)`);
  products.tendon = e(`IFCTENDON('${g('tendon')}',$,'Tendon T1',$,$,#${lp(stPl, ax(pt(0, 0, 0.5), null, null))},#${shape(disk([0, 0, 0], [30, 0, 0], 0.04), 'AdvancedSweptSolid')},$,$,.STRAND.,0.08,$,$,$,$,$,$)`);
  // Grid at (8,0,0) rotated 30 deg about Z. U axis '1' along grid y at grid x=0, V axis 'A' along grid x.
  const c = Math.cos(Math.PI / 6);
  const s = Math.sin(Math.PI / 6);
  const gridPl = lp(stPl, ax(pt(8, 0, 0), dir(0, 0, 1), dir(c, s, 0)));
  const u1 = e(`IFCGRIDAXIS('1',#${e(`IFCPOLYLINE((#${pt2(0, -10)},#${pt2(0, 10)}))`)},.T.)`);
  const va = e(`IFCGRIDAXIS('A',#${e(`IFCPOLYLINE((#${pt2(-10, 0)},#${pt2(10, 0)}))`)},.T.)`);
  const grid = e(`IFCGRID('${g('grid')}',$,'Grid',$,$,#${gridPl},$,(#${u1}),(#${va}),$,$)`);
  const vgi = e(`IFCVIRTUALGRIDINTERSECTION((#${u1},#${va}),(0.,0.,0.5))`);
  const gp = schema.startsWith('IFC4X3') ? e(`IFCGRIDPLACEMENT(#${gridPl},#${vgi},$)`) : e(`IFCGRIDPLACEMENT(#${vgi},$)`);
  // Placement x runs along U axis '1' = grid +y = world heading 120 deg. Local (cos30,-sin30)
  // is world heading 90 deg (+Y).
  const a = [-1.8 * c, 1.8 * s, 0];
  const b = [1.8 * c, -1.8 * s, 0];
  products.gbar = e(`IFCREINFORCINGBAR('${g('gbar')}',$,'Stirrup on grid',$,$,#${gp},#${shape(disk(a, b, 0.01), 'AdvancedSweptSolid')},$,$,0.02,$,$,.LIGATURE.,$)`);
  e(`IFCRELCONTAINEDINSPATIALSTRUCTURE('${guid()}',$,$,$,(${Object.values(products).map((p) => '#' + p).join(',')},#${grid}),#${storey})`);
  return {
    text: wrap(schema, 'review_grid.ifc', S.lines),
    products,
    guids,
    centres: { gbar: [origin[0] + 8, origin[1], origin[2] + 0.5] }
  };
}

/**
 * Linear-placement model: the repo fixture authored/linear-placement.ifc (read only, copied in
 * memory) plus a tendon along X at z=0.5 and two stirrups with IfcLinearPlacement on its basis
 * curve: s=45 (straight, heading 0) and s=420 (circular arc, heading 0.12 + 100/500 = 0.32 rad).
 * The arc stirrup's world centre is read back from the kernel before the tendon is placed
 * through it (`extraTendonAt`).
 */
export function linearModel(fixturePath, extraTendonAt) {
  const src = readFileSync(fixturePath, 'utf8');
  const guid = guidMaker('2LinF5');
  const S = new Step(5000);
  const e = (s) => S.e(s);
  const pt = (x, y, z) => e(`IFCCARTESIANPOINT((${num(x)},${num(y)},${num(z)}))`);
  const shape = (item) => e(`IFCPRODUCTDEFINITIONSHAPE($,$,(#${e(`IFCSHAPEREPRESENTATION(#11,'Body','AdvancedSweptSolid',(#${item}))`)}))`);
  const disk = (a, b, r) => e(`IFCSWEPTDISKSOLID(#${e(`IFCPOLYLINE((#${pt(...a)},#${pt(...b)}))`)},${num(r)},$,$,$)`);
  const products = {};
  const guids = {};
  const g = (key) => (guids[key] = guid());
  const linPl = (sDist) => e(`IFCLINEARPLACEMENT($,#${e(`IFCAXIS2PLACEMENTLINEAR(#${e(`IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(${num(sDist)}),0.,0.5,$,#29)`)},$,$)`)},$)`);
  const origin = e(`IFCLOCALPLACEMENT($,#${e(`IFCAXIS2PLACEMENT3D(#${pt(0, 0, 0)},$,$)`)})`);
  products.tendon = e(`IFCTENDON('${g('tendon')}',$,'Tendon LP',$,$,#${origin},#${shape(disk([30, 0, 0.5], [60, 0, 0.5], 0.04))},$,$,.STRAND.,0.08,$,$,$,$,$,$)`);
  products.lp45 = e(`IFCREINFORCINGBAR('${g('lp45')}',$,'Stirrup LP s=45',$,$,#${linPl(45)},#${shape(disk([0, -1.8, 0], [0, 1.8, 0], 0.01))},$,$,0.02,$,$,.LIGATURE.,$)`);
  products.lp420 = e(`IFCREINFORCINGBAR('${g('lp420')}',$,'Stirrup LP s=420',$,$,#${linPl(420)},#${shape(disk([0, -1.8, 0], [0, 1.8, 0], 0.01))},$,$,0.02,$,$,.LIGATURE.,$)`);
  if (extraTendonAt) {
    const [x, y, z] = extraTendonAt;
    products.tendon2 = e(`IFCTENDON('${g('tendon2')}',$,'Tendon LP arc',$,$,#${origin},#${shape(disk([x - 10, y, z], [x + 10, y, z], 0.04))},$,$,.STRAND.,0.08,$,$,$,$,$,$)`);
  }
  e(`IFCRELCONTAINEDINSPATIALSTRUCTURE('${guid()}',$,$,$,(${Object.values(products).map((p) => '#' + p).join(',')}),#13)`);
  const at = src.lastIndexOf('ENDSEC;');
  const text = src.slice(0, at) + S.lines.join('\r\n') + '\r\n' + src.slice(at);
  return { text, products, guids };
}

/** LandXML 1.2 with one straight alignment from (E0,N0) to (E1,N1), Northing Easting order. */
export function landXml(name, staStart, [e0, n0], [e1, n1]) {
  const len = Math.hypot(e1 - e0, n1 - n0);
  return `<?xml version="1.0" encoding="UTF-8"?>
<LandXML version="1.2">
  <Units><Metric linearUnit="meter" angularUnit="grads" directionUnit="grads"/></Units>
  <Alignments>
    <Alignment name="${name}" staStart="${staStart}">
      <CoordGeom>
        <Line length="${len}"><Start>${n0} ${e0}</Start><End>${n1} ${e1}</End></Line>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>
`;
}
