// T-170 (F1): LandXML 1.2 horizontal alignment parser, clothoid evaluator/projector,
// and f64 localToMap^-1 binding tests (Istram 25.11 grads, negative staStart, S-curves, IfcMapConversion).
// Usage: node --experimental-strip-types test/t170-landxml-test.mjs
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Resolve extensionless relative TS imports under Node ESM.
registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (err) {
      if (specifier.startsWith('.') && context.parentURL) {
        const base = path.resolve(path.dirname(fileURLToPath(context.parentURL)), specifier);
        for (const candidate of [`${base}.ts`, path.join(base, 'index.ts')]) {
          if (existsSync(candidate)) {
            return nextResolve(pathToFileURL(candidate).href, context);
          }
        }
      }
      throw err;
    }
  }
});

const {
  bindAlignmentToModel,
  evaluateAlignment,
  extractAffine2DFromLocalToMap,
  formatStationPk,
  invertAffine2D,
  parseLandXml,
  projectPointOnAlignment
} = await import('../src/domain/alignment/landXml.ts');

let passed = 0;
function test(name, fn) {
  fn();
  console.log(`PASS ${name}`);
  passed++;
}

function approx(actual, expected, tol = 1e-6, msg = '') {
  assert.ok(
    Math.abs(actual - expected) <= tol,
    `${msg ? msg + ': ' : ''}expected ${expected} ± ${tol}, got ${actual} (diff ${Math.abs(actual - expected)})`
  );
}

/**
 * Helper: numerically integrate a clothoid (k0 -> k1 over length L, starting at (x0, y0) with
 * initial heading theta0) using fine Simpson/midpoint steps so our test fixtures have exact
 * ground-truth coordinates independent of the parser's quadrature.
 */
function buildGroundTruthClothoid(x0, y0, theta0, k0, k1, L, steps = 4000) {
  const h = L / steps;
  const dk = (k1 - k0) / L;
  let x = x0;
  let y = y0;
  for (let i = 0; i < steps; i++) {
    const s = (i + 0.5) * h;
    const th = theta0 + k0 * s + 0.5 * dk * s * s;
    x += Math.cos(th) * h;
    y += Math.sin(th) * h;
  }
  const thetaEnd = theta0 + k0 * L + 0.5 * dk * L * L;
  return { endX: x, endY: y, thetaEnd };
}

/**
 * Build a realistic Istram 25.11 LandXML 1.2 fixture in UTM 29N (Galicia) with:
 * - grads angular/direction units (400 gon = 360 deg)
 * - negative staStart (-1446.73 m)
 * - Line -> Spiral (CCW, TangentToCurve, R=200m, L=60m) -> Curve (CCW, R=200m, L=80m)
 *   -> Spiral (CCW, CurveToTangent, R=200m, L=60m) -> Spiral (CW, TangentToCurve, R=250m, L=50m) [S-curve]
 *   -> Curve (CW, R=250m, L=60m)
 * - plus a second auxiliary alignment in the same file.
 */
function buildIstramBridgeLandXml() {
  // Base UTM 29N coordinates: Easting (X) = 537000, Northing (Y) = 4747000
  const x0 = 537000.0;
  const y0 = 4747000.0;
  const theta0 = (35 * Math.PI) / 180; // 35 deg CCW from East

  // 1) Line: L = 100 m
  const L1 = 100.0;
  const x1 = x0 + L1 * Math.cos(theta0);
  const y1 = y0 + L1 * Math.sin(theta0);

  // 2) Spiral 1: CCW, INF -> R=200 m, L=60 m
  const R1 = 200.0;
  const Ls1 = 60.0;
  const sp1 = buildGroundTruthClothoid(x1, y1, theta0, 0, 1 / R1, Ls1);

  // 3) Curve 1: CCW, R=200 m, L=80 m
  const Lc1 = 80.0;
  const cx1 = sp1.endX - R1 * Math.sin(sp1.thetaEnd);
  const cy1 = sp1.endY + R1 * Math.cos(sp1.thetaEnd);
  const a1Start = Math.atan2(sp1.endY - cy1, sp1.endX - cx1);
  const a1End = a1Start + Lc1 / R1;
  const c1EndX = cx1 + R1 * Math.cos(a1End);
  const c1EndY = cy1 + R1 * Math.sin(a1End);
  const thetaAfterC1 = sp1.thetaEnd + Lc1 / R1;

  // 4) Spiral 2: CCW, R=200 m -> INF, L=60 m
  const Ls2 = 60.0;
  const sp2 = buildGroundTruthClothoid(c1EndX, c1EndY, thetaAfterC1, 1 / R1, 0, Ls2);

  // 5) Spiral 3 (S-curve reversal): CW, INF -> R=250 m, L=50 m
  const R2 = 250.0;
  const Ls3 = 50.0;
  const sp3 = buildGroundTruthClothoid(sp2.endX, sp2.endY, sp2.thetaEnd, 0, -1 / R2, Ls3);

  // 6) Curve 2: CW, R=250 m, L=60 m
  const Lc2 = 60.0;
  const cx2 = sp3.endX + R2 * Math.sin(sp3.thetaEnd);
  const cy2 = sp3.endY - R2 * Math.cos(sp3.thetaEnd);
  const a2Start = Math.atan2(sp3.endY - cy2, sp3.endX - cx2);
  const a2End = a2Start - Lc2 / R2;
  const c2EndX = cx2 + R2 * Math.cos(a2End);
  const c2EndY = cy2 + R2 * Math.sin(a2End);

  // Note LandXML coordinate order is "Northing Easting" (Y X)!
  const pt = (x, y) => `${y.toFixed(9)} ${x.toFixed(9)}`;

  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<LandXML xmlns="http://www.landxml.org/schema/LandXML-1.2" version="1.2" date="2026-11-20" time="10:00:00">
  <Units>
    <Metric areaUnit="squareMeter" linearUnit="meter" volumeUnit="cubicMeter"
            temperatureUnit="celsius" pressureUnit="milliBars"
            angularUnit="grads" directionUnit="grads"/>
  </Units>
  <Application name="Istram" version="25.11"/>
  <Alignments name="Viaducto">
    <Alignment name="EJE_PRINCIPAL" desc="Eje tronco puente" staStart="-1446.73" length="410.0">
      <CoordGeom>
        <Line length="${L1}">
          <Start>${pt(x0, y0)}</Start>
          <End>${pt(x1, y1)}</End>
        </Line>
        <Spiral spiType="clothoid" desc="TangentToCurve" rot="ccw" length="${Ls1}"
                radiusStart="INF" radiusEnd="${R1}" constant="${Math.sqrt(R1 * Ls1).toFixed(6)}">
          <Start>${pt(x1, y1)}</Start>
          <End>${pt(sp1.endX, sp1.endY)}</End>
        </Spiral>
        <Curve rot="ccw" radius="${R1}" length="${Lc1}">
          <Start>${pt(sp1.endX, sp1.endY)}</Start>
          <Center>${pt(cx1, cy1)}</Center>
          <End>${pt(c1EndX, c1EndY)}</End>
        </Curve>
        <Spiral spiType="clothoid" desc="CurveToTangent" rot="ccw" length="${Ls2}"
                radiusStart="${R1}" radiusEnd="INF" constant="${Math.sqrt(R1 * Ls2).toFixed(6)}">
          <Start>${pt(c1EndX, c1EndY)}</Start>
          <End>${pt(sp2.endX, sp2.endY)}</End>
        </Spiral>
        <Spiral spiType="clothoid" desc="TangentToCurve" rot="cw" length="${Ls3}"
                radiusStart="INF" radiusEnd="${R2}" constant="${Math.sqrt(R2 * Ls3).toFixed(6)}">
          <Start>${pt(sp2.endX, sp2.endY)}</Start>
          <End>${pt(sp3.endX, sp3.endY)}</End>
        </Spiral>
        <Curve rot="cw" radius="${R2}" length="${Lc2}">
          <Start>${pt(sp3.endX, sp3.endY)}</Start>
          <Center>${pt(cx2, cy2)}</Center>
          <End>${pt(c2EndX, c2EndY)}</End>
        </Curve>
      </CoordGeom>
    </Alignment>
    <Alignment name="RAMAL_ACCESO" staStart="0.0" length="50.0">
      <CoordGeom>
        <Line length="50.0">
          <Start>${pt(x0, y0)}</Start>
          <End>${pt(x0 + 50, y0)}</End>
        </Line>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>`;

  return { xml, x0, y0, theta0, sp1, sp2, sp3, c1EndX, c1EndY, c2EndX, c2EndY };
}

test('parses Istram 25.11 LandXML units (grads), negative staStart, multiple alignments, and Northing/Easting order', () => {
  const { xml, x0, y0 } = buildIstramBridgeLandXml();
  const doc = parseLandXml(xml);

  assert.equal(doc.warnings.length, 0);
  assert.equal(doc.units.linearUnit, 'meter');
  assert.equal(doc.units.linearToMeters, 1);
  assert.equal(doc.units.angularUnit, 'grads');
  approx(doc.units.angularToRadians, Math.PI / 200, 1e-15, '400 gon = 2*pi rad');
  approx(doc.units.directionToRadians, Math.PI / 200, 1e-15);

  assert.equal(doc.alignments.length, 2);
  const main = doc.alignments[0];
  assert.equal(main.name, 'EJE_PRINCIPAL');
  approx(main.staStart, -1446.73, 1e-12);
  approx(main.length, 410.0, 1e-6);
  approx(main.staEnd, -1446.73 + 410.0, 1e-6);
  assert.equal(main.segments.length, 6);

  // Verify Northing Easting -> (X=Easting, Y=Northing)
  approx(main.segments[0].start[0], x0, 1e-6, 'start X = Easting');
  approx(main.segments[0].start[1], y0, 1e-6, 'start Y = Northing');
  assert.equal(doc.alignments[1].name, 'RAMAL_ACCESO');
});

test('C0 position and C1 tangent continuity across Line -> Spiral -> Curve -> Spiral -> S-curve Spiral -> Curve', () => {
  const { xml, sp2 } = buildIstramBridgeLandXml();
  const main = parseLandXml(xml).alignments[0];

  for (let i = 0; i < main.segments.length - 1; i++) {
    const segA = main.segments[i];
    const segB = main.segments[i + 1];
    const endA = evaluateAlignment(main, segA.staEnd - 1e-9);
    const startB = evaluateAlignment(main, segB.staStart + 1e-9);

    approx(endA.point[0], startB.point[0], 1e-5, `C0 X junction ${i}->${i + 1}`);
    approx(endA.point[1], startB.point[1], 1e-5, `C0 Y junction ${i}->${i + 1}`);
    approx(endA.tangent[0], startB.tangent[0], 1e-5, `C1 Tx junction ${i}->${i + 1}`);
    approx(endA.tangent[1], startB.tangent[1], 1e-5, `C1 Ty junction ${i}->${i + 1}`);
  }

  // S-curve inflection point (junction 3->4 at staStart + 300m) has curvature ~ 0 and exact point
  const inflection = evaluateAlignment(main, -1446.73 + 300.0);
  approx(inflection.point[0], sp2.endX, 1e-5, 'S-curve inflection X');
  approx(inflection.point[1], sp2.endY, 1e-5, 'S-curve inflection Y');
  approx(inflection.curvature, 0, 1e-6, 'S-curve inflection curvature');
});

test('projects offset points accurately onto Line, Curve (CCW & CW), and Spiral (clothoid) segments', () => {
  const { xml } = buildIstramBridgeLandXml();
  const main = parseLandXml(xml).alignments[0];

  // Sample test stations inside every single segment (line, spiral1, curve1, spiral2, spiral3, curve2)
  const testStations = [
    { rel: 42.5, offset: 3.25 },    // Line (right)
    { rel: 128.0, offset: -4.5 },   // Spiral 1 CCW (left)
    { rel: 205.0, offset: 6.0 },    // Curve 1 CCW (right)
    { rel: 271.5, offset: -2.1 },   // Spiral 2 CCW (left)
    { rel: 325.0, offset: 5.75 },   // Spiral 3 CW S-curve (right)
    { rel: 380.0, offset: -3.4 }    // Curve 2 CW (left)
  ];

  for (const { rel, offset } of testStations) {
    const targetSta = main.staStart + rel;
    const sample = evaluateAlignment(main, targetSta);
    const query = [
      sample.point[0] + sample.rightNormal[0] * offset,
      sample.point[1] + sample.rightNormal[1] * offset
    ];

    const proj = projectPointOnAlignment(main, query);
    approx(proj.station, targetSta, 1e-5, `station at rel=${rel}`);
    approx(proj.distance, Math.abs(offset), 1e-5, `distance at rel=${rel}`);
    approx(proj.signedOffset, offset, 1e-5, `signedOffset at rel=${rel}`);
    approx(proj.tangent[0], sample.tangent[0], 1e-5, `Tx at rel=${rel}`);
    approx(proj.tangent[1], sample.tangent[1], 1e-5, `Ty at rel=${rel}`);
    assert.equal(proj.clamped, false, `not clamped at rel=${rel}`);
  }

  // Clamped before start
  const startSample = evaluateAlignment(main, main.staStart);
  const beforeStart = [
    startSample.point[0] - startSample.tangent[0] * 10,
    startSample.point[1] - startSample.tangent[1] * 10
  ];
  const projBefore = projectPointOnAlignment(main, beforeStart);
  approx(projBefore.station, main.staStart, 1e-6);
  approx(projBefore.distance, 10, 1e-6);
  assert.equal(projBefore.clamped, true);
});

test('supports millimeter linearUnit and compound (ovoid) clothoid spirals', () => {
  // Ovoid clothoid from R=400m to R=150m over L=80m, expressed in millimeters
  const L = 80;
  const sp = buildGroundTruthClothoid(100, 200, 0.2, 1 / 400, 1 / 150, L);
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<LandXML version="1.2">
  <Units>
    <Metric linearUnit="millimeter" angularUnit="decimal degrees" directionUnit="decimal degrees"/>
  </Units>
  <Alignments>
    <Alignment name="OvoidMM" staStart="100000">
      <CoordGeom>
        <Spiral spiType="clothoid" rot="ccw" length="${L * 1000}" radiusStart="400000" radiusEnd="150000">
          <Start>${200 * 1000} ${100 * 1000}</Start>
          <End>${sp.endY * 1000} ${sp.endX * 1000}</End>
        </Spiral>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>`;

  const doc = parseLandXml(xml);
  assert.equal(doc.units.linearToMeters, 0.001);
  const al = doc.alignments[0];
  approx(al.staStart, 100, 1e-9);
  approx(al.length, 80, 1e-9);

  const mid = evaluateAlignment(al, 140);
  const query = [mid.point[0] + mid.rightNormal[0] * 2.5, mid.point[1] + mid.rightNormal[1] * 2.5];
  const proj = projectPointOnAlignment(al, query);
  approx(proj.station, 140, 1e-5);
  approx(proj.signedOffset, 2.5, 1e-5);
});

test('binds map-space alignment to IFC model-local space via f64 localToMap^-1 (Allplan IfcMapConversion)', () => {
  const { xml } = buildIstramBridgeLandXml();
  const main = parseLandXml(xml).alignments[0];

  // Realistic Allplan IFC4X3_ADD2 IfcMapConversion:
  // Map origin at (E=537000, N=4747000), rotation theta=27.5 deg, grid scale lambda=0.9996
  const theta = (27.5 * Math.PI) / 180;
  const scale = 0.9996;
  const c = scale * Math.cos(theta);
  const s = scale * Math.sin(theta);
  const eastings = 537000.0;
  const northings = 4747000.0;

  // Column-major 4x4 localToMap (docs/05 section 2)
  const localToMap = [
    c, s, 0, 0,
    -s, c, 0, 0,
    0, 0, scale, 0,
    eastings, northings, 120.5, 1
  ];

  const fwd = extractAffine2DFromLocalToMap(localToMap);
  assert.ok(fwd);
  const inv = invertAffine2D(fwd);
  assert.ok(inv);

  const bound = bindAlignmentToModel(main, {
    source: 'ifc-map-conversion',
    confidence: 'verified',
    crsId: 'EPSG:25829',
    localToMap
  });
  assert.equal(bound.hasMapConversion, true);
  assert.equal(bound.warnings.length, 0);

  // Model local origin (0, 0) maps to (537000, 4747000) which is exact start of alignment!
  const startLocal = bound.evaluateLocal(main.staStart);
  approx(startLocal.point[0], 0, 1e-8, 'local start X');
  approx(startLocal.point[1], 0, 1e-8, 'local start Y');

  // Test projection of a local-space clash point on the curved bridge deck
  const targetSta = main.staStart + 210.0;
  const deckSample = bound.evaluateLocal(targetSta);
  const clashLocalX = deckSample.point[0] + deckSample.rightNormal[0] * 4.2;
  const clashLocalY = deckSample.point[1] + deckSample.rightNormal[1] * 4.2;

  const proj = bound.projectLocal(clashLocalX, clashLocalY);
  approx(proj.station, targetSta, 1e-5, 'PK preserved in map meters');
  approx(proj.distance, 4.2, 1e-5, 'local lateral distance');
  approx(proj.signedOffset, 4.2, 1e-5, 'local signed offset');
  approx(proj.tangent[0], deckSample.tangent[0], 1e-6, 'local tangent X');
  approx(proj.tangent[1], deckSample.tangent[1], 1e-6, 'local tangent Y');
  approx(Math.hypot(proj.tangent[0], proj.tangent[1]), 1, 1e-12, 'local tangent is unit');
});

test('emits warning when localToMap is missing and alignment 2D bounds do not overlap model bounds', () => {
  const { xml } = buildIstramBridgeLandXml();
  const main = parseLandXml(xml).alignments[0];

  // Disjoint local model around (0..100, 0..100) while alignment is in UTM (537000, 4747000)
  const disjoint = bindAlignmentToModel(main, undefined, {
    min: { x: 0, y: 0, z: 0 },
    max: { x: 100, y: 100, z: 20 }
  });
  assert.equal(disjoint.hasMapConversion, false);
  assert.equal(disjoint.warnings.length, 1);
  assert.equal(disjoint.warnings[0].code, 'no-map-conversion-disjoint-bounds');

  // Overlapping model bounds -> no warning
  const overlapping = bindAlignmentToModel(main, undefined, {
    min: { x: 536900, y: 4746900, z: 0 },
    max: { x: 537500, y: 4747500, z: 30 }
  });
  assert.equal(overlapping.warnings.length, 0);
});

test('formatStationPk formats positive, negative, and rollover stations cleanly', () => {
  assert.equal(formatStationPk(124.5), '0+124.50');
  assert.equal(formatStationPk(0), '0+000.00');
  assert.equal(formatStationPk(1005.25), '1+005.25');
  assert.equal(formatStationPk(-1446.73), '-1+446.73');
  assert.equal(formatStationPk(999.999), '1+000.00');
  assert.equal(formatStationPk(-42.5, 1), '-0+042.5');
});

test('honors explicit segment staStart attributes and handles station equation gaps', () => {
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<LandXML version="1.2">
  <Units><Metric linearUnit="meter" angularUnit="decimal degrees"/></Units>
  <Alignments>
    <Alignment name="STA_EQ" staStart="0.0">
      <CoordGeom>
        <Line length="50.0" staStart="0.0">
          <Start>0.0 0.0</Start>
          <End>0.0 50.0</End>
        </Line>
        <Line length="40.0" staStart="200.0">
          <Start>0.0 50.0</Start>
          <End>0.0 90.0</End>
        </Line>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>`;
  const al = parseLandXml(xml).alignments[0];
  assert.equal(al.segments.length, 2);
  approx(al.segments[0].staStart, 0, 1e-9);
  approx(al.segments[0].staEnd, 50, 1e-9);
  approx(al.segments[1].staStart, 200, 1e-9);
  approx(al.segments[1].staEnd, 240, 1e-9);
  approx(al.length, 90, 1e-9, 'geometric length excludes station gap');

  // Evaluating inside segment 2 at station 220 lands at X = 70 m
  const s2 = evaluateAlignment(al, 220);
  approx(s2.point[0], 70, 1e-9);
  approx(s2.station, 220, 1e-9);

  // Evaluating inside the gap picks the closer segment boundary without throwing
  const nearSeg0 = evaluateAlignment(al, 60);
  approx(nearSeg0.station, 50, 1e-9);
  const nearSeg1 = evaluateAlignment(al, 190);
  approx(nearSeg1.station, 200, 1e-9);
});

console.log(`\nt170-landxml-test: ${passed}/${passed} pass`);
