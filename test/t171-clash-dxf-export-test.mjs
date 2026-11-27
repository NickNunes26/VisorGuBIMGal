// T-171 (F4): End-to-end clash alignment DXF batch planner, WASM ClashBatchSlicer + 3D slab
// culling, ZIP packaging (`individual/` + `clashes_all.dxf` + `summary.csv`), and cancellation test.
// Usage: node --experimental-strip-types test/t171-clash-dxf-export-test.mjs
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

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

const { bindAlignmentToModel, parseLandXml } = await import('../src/domain/alignment/landXml.ts');
const { buildClashSummaryCsv, computeElementASlabTs, planClashAlignmentSections } = await import(
  '../src/domain/alignment/clashDxfPlan.ts'
);
const { exportClashAlignmentZip } = await import('../src/ui/viewer/clashAlignmentExport.ts');
const {
  buildRoundTripAuditCsv,
  extractMetaModelIdFromDxf,
  packMoveEditsForStepPatch
} = await import('../src/ui/viewer/clashRoundTrip.ts');
const { relativeModelTransform, transformSlabQueryToSource } = await import(
  '../src/infrastructure/kernel/drawingRequest.ts'
);
const { WasmKernelAdapter } = await import('../src/infrastructure/kernel/WasmKernelAdapter.ts');
const { readZip } = await import('../src/ui/viewer/zipArchive.ts');
const wasm = await import('../src/infrastructure/kernel/pkg/wasm_api.js');

const wasmBytes = readFileSync(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src/infrastructure/kernel/pkg/wasm_api_bg.wasm')
);
await wasm.default({ module_or_path: wasmBytes });

let passed = 0;
async function test(name, fn) {
  await fn();
  console.log(`PASS ${name}`);
  passed++;
}

function boxVerts(x0, y0, z0, x1, y1, z1) {
  return new Float64Array([
    x0, y0, z0, x1, y0, z0, x1, y1, z0, x0, y1, z0,
    x0, y0, z1, x1, y0, z1, x1, y1, z1, x0, y1, z1
  ]);
}

const ALIGNMENT_XML = `<?xml version="1.0" encoding="UTF-8"?>
<LandXML version="1.2">
  <Units>
    <Metric linearUnit="meter" angularUnit="grads" directionUnit="grads"/>
  </Units>
  <Alignments>
    <Alignment name="PUENTE_TEST" staStart="100.0">
      <CoordGeom>
        <Line length="50.0">
          <Start>0.0 0.0</Start>
          <End>0.0 50.0</End>
        </Line>
      </CoordGeom>
    </Alignment>
  </Alignments>
</LandXML>`;

await test('plans transverse vs longitudinal slabs and merges clashes within 20 mm PK', () => {
  const doc = parseLandXml(ALIGNMENT_XML);
  const bound = bindAlignmentToModel(doc.alignments[0]);

  const transverseVerts = boxVerts(10.0, -2.0, 0.0, 10.02, 2.0, 1.0);
  const sFront = computeElementASlabTs(transverseVerts, [1, 0], [10.0, 0, 0.5]);
  const sRear = computeElementASlabTs(transverseVerts, [1, 0], [10.02, 0, 0.5]);
  assert.equal(sFront.orientation, 'transverse');
  assert.equal(sRear.orientation, 'transverse');
  assert.ok(Math.abs(sFront.sMin - 10.0) < 1e-9 && Math.abs(sFront.sMax - 10.02) < 1e-9);
  assert.ok(Math.abs(sRear.sMin - 10.0) < 1e-9 && Math.abs(sRear.sMax - 10.02) < 1e-9);

  const longVerts = boxVerts(0.0, -0.015, 0.0, 12.0, 0.015, 0.03);
  const sLong = computeElementASlabTs(longVerts, [1, 0], [8.5, 0, 0.015]);
  assert.equal(sLong.orientation, 'longitudinal');
  assert.equal(sLong.depthM, 0);
  assert.ok(Math.abs(sLong.sMin - 8.5) < 1e-9);

  const sections = planClashAlignmentSections(
    [
      {
        clashIndex: 0,
        modelIdA: 'm1',
        expressIdA: 101,
        globalIdA: 'GUID_A1',
        labelA: 'Stirrup 1',
        modelIdB: 'm1',
        expressIdB: 201,
        labelB: 'Tendon 1',
        kind: 'hard',
        penetrationM: 0.012,
        gapM: 0,
        point: [10.005, 0.2, 0.5],
        elementAVertices: transverseVerts
      },
      {
        clashIndex: 1,
        modelIdA: 'm1',
        expressIdA: 102,
        globalIdA: 'GUID_A2',
        labelA: 'Stirrup 2',
        modelIdB: 'm1',
        expressIdB: 201,
        labelB: 'Tendon 1',
        kind: 'hard',
        penetrationM: 0.018,
        gapM: 0,
        point: [10.018, -0.1, 0.5], // 13 mm away -> merges!
        elementAVertices: boxVerts(10.01, -2.0, 0.0, 10.03, 2.0, 1.0)
      },
      {
        clashIndex: 2,
        modelIdA: 'm1',
        expressIdA: 103,
        globalIdA: 'GUID_A3',
        labelA: 'LongBar 1',
        modelIdB: 'm1',
        expressIdB: 202,
        labelB: 'Tendon 2',
        kind: 'hard',
        penetrationM: 0.025,
        gapM: 0,
        point: [22.4, 0.0, 0.015],
        elementAVertices: longVerts
      }
    ],
    bound,
    { mergeToleranceMm: 20 }
  );

  assert.equal(sections.length, 2);
  assert.equal(sections[0].orientation, 'transverse');
  assert.deepEqual(sections[0].clashIndices, [0, 1]);
  assert.ok(Math.abs(sections[0].depthM - 0.03) < 1e-9);
  assert.equal(sections[1].orientation, 'longitudinal');
  assert.equal(sections[1].depthM, 0);

  const csv = buildClashSummaryCsv(sections);
  assert.ok(csv.includes('individual/PK_0+110.02_001.dxf'));
  assert.ok(csv.includes('individual/PK_0+122.40_002.dxf'));
});

await test('packages individual DXFs, clashes_all.dxf master grid, and summary.csv into a valid ZIP via real WASM ClashBatchSlicer', async () => {
  const doc = parseLandXml(ALIGNMENT_XML);
  const bound = bindAlignmentToModel(doc.alignments[0]);

  const ifcPath = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    '../public/samples/SimpleWall.ifc'
  );
  const model = new wasm.Model(new Uint8Array(readFileSync(ifcPath)));

  try {
    // Verify 3D slab-vs-AABB culling on a real IFC model.
    const kept = model.cullElementsBySlab(1, 0, -100, 100, 0.05);
    assert.ok(kept.length > 0, 'slab covering model must retain elements');
    const farCulled = model.cullElementsBySlab(1, 0, 5000, 5001, 0.05);
    assert.equal(farCulled.length, 0, 'slab far from model must cull 100% of elements');

    // Run full exportClashAlignmentZip using a lightweight KernelPort backed by the real WASM model + ClashBatchSlicer.
    const firstId = kept[0];
    const mockKernel = {
      async elementMesh(_modelId, expressId) {
        const view = model.elementMesh(expressId);
        if (!view) return null;
        try {
          return {
            positions: view.takePositions(),
            indices: view.takeIndices(),
            edges: view.takeEdges()
          };
        } finally {
          view.free();
        }
      },
      async getElementDetails() {
        return { globalId: '2O2Fr$t4X7Zf8NOew3FLOH' };
      },
      async exportClashAlignmentDxf(req, onProgress, signal) {
        const slicer = new wasm.ClashBatchSlicer();
        try {
          const outputs = [];
          for (let i = 0; i < req.sections.length; i++) {
            if (signal?.aborted) throw new DOMException('Cancelled', 'AbortError');
            const s = req.sections[i];
            onProgress?.('Cortando', Math.round((i / req.sections.length) * 90));
            const culled = model.cullElementsBySlab(s.normal[0], s.normal[1], s.sMin, s.sMax, 0.5);
            if (culled.length > 0) {
              const chunk = model.slabChunkForElements(culled, undefined);
              slicer.addSectionChunk(0, chunk);
            }
            const pts = new Float64Array(s.clashPoints.flat());
            const out = slicer.finishSection(
              s.origin[0], s.origin[1], s.origin[2],
              s.normal[0], s.normal[1], s.normal[2],
              s.depthM,
              new Uint32Array(s.clashAIds.map(() => 0)),
              new Uint32Array(s.clashAIds),
              [...s.clashAGuids],
              s.clashAIds.map(() => s.modelId),
              new Uint32Array(s.clashBIds.map(() => 0)),
              new Uint32Array(s.clashBIds),
              pts,
              s.alignmentPoint[0], s.alignmentPoint[1], s.alignmentPoint[2],
              s.modelId,
              s.pkM,
              s.pkLabel,
              s.sMin,
              s.sMax,
              [...s.annotations]
            );
            try {
              outputs.push({
                fileName: s.fileName,
                pkM: s.pkM,
                pkLabel: s.pkLabel,
                dxfBytes: out.dxfBytes(),
                svg: out.svg(),
                widthM: out.widthM(),
                heightM: out.heightM()
              });
            } finally {
              out.free();
            }
          }
          const masterDxfBytes = slicer.mergeMasterDxf(0, 1.0);
          return { sections: outputs, masterDxfBytes };
        } finally {
          slicer.free();
        }
      }
    };

    const entries = [
      {
        id: 'c1',
        elementA: { modelId: 'm1', expressId: firstId },
        elementB: { modelId: 'm1', expressId: firstId + 1 },
        kind: 'hard',
        penetrationM: 0.015,
        gapM: 0,
        point: [2.0, 0.0, 1.0],
        bounds: { min: { x: 0, y: 0, z: 0 }, max: { x: 5, y: 5, z: 3 } },
        metaA: { name: 'Wall A', type: 'IfcWall', modelName: 'SimpleWall.ifc' },
        metaB: { name: 'Pipe B', type: 'IfcPipeSegment', modelName: 'SimpleWall.ifc' }
      }
    ];

    const res = await exportClashAlignmentZip(entries, ['m1'], bound, mockKernel, {
      mergeToleranceMm: 20
    });

    const files = await readZip(res.zipBytes);
    assert.ok(files.has('clashes_all.dxf'), 'ZIP must contain clashes_all.dxf');
    assert.ok(files.has('summary.csv'), 'ZIP must contain summary.csv');
    assert.equal(files.size, 3, '1 individual DXF + clashes_all.dxf + summary.csv');

    const indivName = res.sections[0].fileName;
    const indivDxf = new TextDecoder('latin1').decode(files.get(indivName));
    assert.ok(indivDxf.includes('$ACADVER\r\n1\r\nAC1009\r\n'));
    assert.ok(indivDxf.includes('GUBIM_META'));
    assert.ok(indivDxf.endsWith('0\r\nEOF\r\n'));

    const masterDxf = new TextDecoder('latin1').decode(files.get('clashes_all.dxf'));
    assert.ok(masterDxf.includes('SHEET_FRAME'));
    assert.ok(masterDxf.endsWith('0\r\nEOF\r\n'));

    // Verify AbortSignal cancellation stops immediately.
    const abort = new AbortController();
    abort.abort();
    await assert.rejects(
      () =>
        exportClashAlignmentZip(entries, ['m1'], bound, mockKernel, {
          mergeToleranceMm: 20,
          signal: abort.signal
        }),
      (err) => err instanceof DOMException && err.name === 'AbortError'
    );
  } finally {
    model.free();
  }
});

await test('F5 round-trip: Model.applyClashDxf patches STEP on Level 1 MOVE and Level 2 EDIT_A_* rebend and generates audit CSV', () => {
  const clashingIfc = [
    'ISO-10303-21;',
    'HEADER;',
    "FILE_DESCRIPTION(('ViewDefinition [DesignTransferView]'),'2;1');",
    "FILE_NAME('clash_test.ifc','2026-11-23T00:00:00',(''),(''),'','','');",
    "FILE_SCHEMA(('IFC4'));",
    'ENDSEC;',
    'DATA;',
    "#1=IFCPROJECT('0000000000000000000001',$,'Bridge',$,$,$,$,(#10),#2);",
    '#2=IFCUNITASSIGNMENT((#3));',
    '#3=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);',
    "#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#11,$);",
    '#11=IFCAXIS2PLACEMENT3D(#12,$,$);',
    '#12=IFCCARTESIANPOINT((0.,0.,0.));',
    "#20=IFCSITE('0000000000000000000002',$,'Site',$,$,#21,$,$,.ELEMENT.,$,$,$,$,$);",
    '#21=IFCLOCALPLACEMENT($,#11);',
    "#22=IFCRELAGGREGATES('0000000000000000000003',$,$,$,#1,(#20));",
    '#30=IFCDIRECTION((0.,0.,1.));',
    '#31=IFCDIRECTION((1.,0.,0.));',
    '#32=IFCCARTESIANPOINT((0.02,0.02));',
    '#33=IFCAXIS2PLACEMENT2D(#32,$);',
    '#34=IFCRECTANGLEPROFILEDEF(.AREA.,$,#33,0.04,0.04);',
    '#35=IFCAXIS2PLACEMENT3D(#12,#31,#30);',
    '#36=IFCEXTRUDEDAREASOLID(#34,#35,#30,2.0);',
    "#37=IFCSHAPEREPRESENTATION(#10,'Body','SweptSolid',(#36));",
    '#38=IFCPRODUCTDEFINITIONSHAPE($,$,(#37));',
    '#101=IFCCARTESIANPOINT((0.,0.,0.));',
    '#102=IFCAXIS2PLACEMENT3D(#101,$,$);',
    '#103=IFCLOCALPLACEMENT(#21,#102);',
    "#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'Bar_A',$,$,#103,#38,$,$,0.02,$,$,$,$);",
    '#201=IFCCARTESIANPOINT((0.,0.01,0.01));',
    '#202=IFCAXIS2PLACEMENT3D(#201,$,$);',
    '#203=IFCLOCALPLACEMENT(#21,#202);',
    "#200=IFCTENDON('3P3Gs$u5Y8Ag9OPfx4GMPI',$,'Tendon_B',$,$,#203,#38,$,$,0.02,$,$,$,$,$,$,$,$);",
    "#300=IFCRELCONTAINEDINSPATIALSTRUCTURE('0000000000000000000004',$,$,$,(#100,#200),#20);",
    'ENDSEC;',
    'END-ISO-10303-21;'
  ].join('\r\n') + '\r\n';

  const encoder = new TextEncoder();
  const model = new wasm.Model(encoder.encode(clashingIfc));
  const slicer = new wasm.ClashBatchSlicer();

  try {
    // 1. Confirm initial clash in ClashSession
    const session = new wasm.ClashSession(4 * 1024 * 1024);
    try {
      const m100 = model.rawElementMesh(100);
      const m200 = model.rawElementMesh(200);
      session.setActiveMesh(0, 100, m100.takePositions(), m100.takeIndices());
      session.cacheMesh(0, 200, m200.takePositions(), m200.takeIndices());
      m100.free();
      m200.free();
      const initialHit = session.testActiveAgainstCached(0, 200, 0);
      assert.ok(initialHit, 'initial model must report a hard clash between #100 and #200');
    } finally {
      session.free();
    }

    // 2. Slice transverse section DXF at X = 1.0 m (depth = 0.02 m)
    const culled = model.cullElementsBySlab(1, 0, 0.99, 1.01, 0.05);
    const chunk = model.slabChunkForElements(culled, undefined);
    slicer.addSectionChunk(0, chunk);
    const out = slicer.finishSection(
      0.99, 0, 0,
      1, 0, 0,
      0.02,
      new Uint32Array([0]),
      new Uint32Array([100]),
      ['2O2Fr$t4X7Zf8NOew3FLOH'],
      ['bridge-m1'],
      new Uint32Array([0]),
      new Uint32Array([200]),
      new Float64Array([1.0, 0.025, 0.025]),
      1.0, 0, 0,
      'bridge-m1',
      1.0,
      '0+001.00',
      0.99,
      1.01,
      ['PK 0+001.00']
    );
    const dxfBytes = out.dxfBytes();
    out.free();

    assert.equal(extractMetaModelIdFromDxf(dxfBytes), 'bridge-m1');
    const dxfText = new TextDecoder('utf-8').decode(dxfBytes);
    assert.ok(dxfText.includes('EDIT_A_100'), 'exported DXF must include EDIT_A_100 layer');

    // 3. Level 1 MOVE edit in DXF: shift INSERT GUBIM_A_100 by (+0.05, +0.03) m
    const origMatch = /GUBIM_ORIG id=100 (?:model=\S+ )?(?:guid=\S+ )?block=([^\s]+) (?:layer=\S+ )?x=([0-9.-]+) y=([0-9.-]+)/.exec(dxfText);
    assert.ok(origMatch, 'DXF must contain GUBIM_ORIG for #100');
    const [, blockName, origXStr, origYStr] = origMatch;
    const movedX = (Number(origXStr) + 0.05).toFixed(6);
    const movedY = (Number(origYStr) + 0.03).toFixed(6);
    const needle = `0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n${blockName}\r\n10\r\n${origXStr}\r\n20\r\n${origYStr}\r\n`;
    const replacement = `0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n${blockName}\r\n10\r\n${movedX}\r\n20\r\n${movedY}\r\n`;
    const movedDxfBytes = encoder.encode(dxfText.replace(needle, replacement));

    const rtOut = model.applyClashDxf(movedDxfBytes, 'bridge-m1', ['bridge-m1'], new Uint32Array(), 'ask', 0);
    const patchedIfcBytes = rtOut.patchedIfcBytes();
    const diff = JSON.parse(rtOut.diffJson());
    rtOut.free();

    assert.equal(diff.applied, 1);
    assert.equal(diff.edits.length, 1);
    assert.equal(diff.edits[0].kind, 'move');
    assert.equal(diff.edits[0].expressId, 100);

    const packed = packMoveEditsForStepPatch(diff.edits, 0.001);
    assert.equal(packed.length, 4);
    assert.equal(packed[0], 100);

    const auditCsv = buildRoundTripAuditCsv(diff);
    assert.ok(auditCsv.includes('bridge-m1,1.000,move,100,'));

    // Reload patched IFC into a fresh wasm.Model and confirm 0 clashes!
    const patchedModel = new wasm.Model(patchedIfcBytes);
    const verifySession = new wasm.ClashSession(4 * 1024 * 1024);
    try {
      const p100 = patchedModel.rawElementMesh(100);
      const p200 = patchedModel.rawElementMesh(200);
      verifySession.setActiveMesh(0, 100, p100.takePositions(), p100.takeIndices());
      verifySession.cacheMesh(0, 200, p200.takePositions(), p200.takeIndices());
      p100.free();
      p200.free();
      const postHit = verifySession.testActiveAgainstCached(0, 200, 0);
      assert.equal(postHit, undefined, 'patched IFC must have 0 clashes after Level 1 MOVE');
    } finally {
      verifySession.free();
      patchedModel.free();
    }
  } finally {
    slicer.free();
    model.free();
  }
});

await test('federated multi-model transform aligns localToMap frames and pulls slab culling query into source space', () => {
  // Target model (Model 0): map origin at (500000, 4700000, 0), identity rotation.
  const refTarget = {
    source: 'ifc-map-conversion',
    confidence: 'verified',
    crsId: 'EPSG:25829',
    localToMap: [
      1, 0, 0, 0,
      0, 1, 0, 0,
      0, 0, 1, 0,
      500000, 4700000, 0, 1
    ]
  };
  // Source model (Model 1): map origin at (500100, 4700020, 0), rotated +90 deg around Z.
  const refSource = {
    source: 'ifc-map-conversion',
    confidence: 'verified',
    crsId: 'EPSG:25829',
    localToMap: [
      0, 1, 0, 0,
      -1, 0, 0, 0,
      0, 0, 1, 0,
      500100, 4700020, 0, 1
    ]
  };

  // Identical references yield undefined (no transform needed).
  assert.equal(relativeModelTransform(refTarget, refTarget), undefined);

  const rel = relativeModelTransform(refSource, refTarget);
  assert.ok(rel, 'different localToMap in same CRS must produce a relative 4x4 transform');
  // Translation of source origin (0,0,0) into target space must be (+100, +20, 0).
  assert.ok(Math.abs(rel[12] - 100) < 1e-9);
  assert.ok(Math.abs(rel[13] - 20) < 1e-9);
  assert.ok(Math.abs(rel[14] - 0) < 1e-9);

  // Slab in target space along +X_target at [105, 105.02]:
  // Since +Y_source maps to -X_target, +X_target corresponds to -Y_source (tx=0, ty=-1),
  // and b·t = 100, so s_source = [5, 5.02] along -Y_source (i.e. Y_source in [-5.02, -5.00]).
  const q = transformSlabQueryToSource(1, 0, 105, 105.02, 0.05, rel);
  assert.ok(Math.abs(q.tx - 0) < 1e-9);
  assert.ok(Math.abs(q.ty - -1) < 1e-9);
  assert.ok(Math.abs(q.sMin - 5.0) < 1e-9);
  assert.ok(Math.abs(q.sMax - 5.02) < 1e-9);
  assert.ok(Math.abs(q.eps - 0.05) < 1e-9);

  // Mismatched CRS returns undefined.
  assert.equal(
    relativeModelTransform({ ...refSource, crsId: 'EPSG:25830' }, refTarget),
    undefined
  );
});

await test('Rust WASM clash_detector::compute_element_a_slab matches TS computeElementASlabTs identically', () => {
  const cases = [
    { verts: boxVerts(10.0, -2.0, 0.0, 10.02, 2.0, 1.0), t: [1, 0], pt: [10.01, 0.2, 0.5] },
    { verts: boxVerts(0.0, -0.015, 0.0, 12.0, 0.015, 0.03), t: [1, 0], pt: [8.5, 0, 0.015] },
    { verts: boxVerts(10.0, -0.5, 0.0, 10.5, 0.5, 0.5), t: [0.8, 0.6], pt: [10.4, 0.1, 0.2] }
  ];
  for (const c of cases) {
    const ts = computeElementASlabTs(c.verts, c.t, c.pt);
    const rs = wasm.computeElementASlab(c.verts, c.t[0], c.t[1], c.pt[0], c.pt[1], c.pt[2], 0.15, 2.5, 0.25);
    assert.equal(rs[0] === 1 ? 'longitudinal' : 'transverse', ts.orientation);
    assert.ok(Math.abs(rs[1] - ts.sMin) < 1e-12);
    assert.ok(Math.abs(rs[2] - ts.sMax) < 1e-12);
    assert.ok(Math.abs(rs[3] - ts.sClash) < 1e-12);
    assert.ok(Math.abs(rs[4] - ts.deltaSA) < 1e-12);
    assert.ok(Math.abs(rs[5] - ts.widthPerpM) < 1e-12);
    assert.ok(Math.abs(rs[6] - ts.heightPerpM) < 1e-12);
  }
});

await test('WasmKernelAdapter orchestrates elementSlabSpan, exportClashAlignmentDxf, and applyClashDxfRoundTrip across Workers', async () => {
  const prevWorker = globalThis.Worker;
  let activeWorkers = 0;

  // Lightweight in-process Worker shim backed by the real compiled WASM Model + ClashBatchSlicer.
  class FakeKernelWorker {
    constructor() {
      this.model = null;
      this.batch = null;
      this.terminated = false;
      this.onmessage = null;
      this.onerror = null;
      activeWorkers++;
    }
    postMessage({ id, request }) {
      queueMicrotask(() => {
        if (this.terminated) return;
        try {
          let value;
          switch (request.op) {
            case 'open': {
              this.model = new wasm.Model(new Uint8Array(request.bytes));
              const summary = JSON.parse(this.model.summaryJson());
              value = { ...summary, diagnostics: [] };
              break;
            }
            case 'clash-element-slab': {
              value =
                this.model.elementSlabSpan(
                  request.expressId,
                  request.tx,
                  request.ty,
                  request.px,
                  request.py,
                  request.pz,
                  request.longMinExtentM,
                  request.longAspectRatio,
                  request.maxTransverseSlabM
                ) ?? null;
              break;
            }
            case 'clash-cull-slab': {
              value = this.model.cullElementsBySlab(
                request.tx,
                request.ty,
                request.sMin,
                request.sMax,
                request.eps
              );
              break;
            }
            case 'clash-slab-chunk': {
              const xf = request.transform ? new Float64Array(request.transform) : undefined;
              const chunk = this.model.slabChunkForElements(request.expressIds, xf);
              value = chunk.buffer;
              break;
            }
            case 'clash-batch-new': {
              this.batch?.free();
              this.batch = new wasm.ClashBatchSlicer();
              this.batch.setExportTime(request.exportTimeMs);
              value = undefined;
              break;
            }
            case 'clash-batch-add': {
              value = this.batch.addSectionChunk(request.modelIndex, new Uint8Array(request.chunk));
              break;
            }
            case 'clash-batch-finish': {
              const out = this.batch.finishSection(
                request.origin[0], request.origin[1], request.origin[2],
                request.normal[0], request.normal[1], request.normal[2],
                request.depthM,
                request.clashAModelIndices,
                request.clashAIds,
                request.clashAGuids,
                request.clashAOwners,
                request.clashBModelIndices,
                request.clashBIds,
                request.clashPointsFlat,
                request.alignmentPoint[0], request.alignmentPoint[1], request.alignmentPoint[2],
                request.modelId,
                request.pkM,
                request.pkLabel,
                request.sMin,
                request.sMax,
                request.annotations
              );
              try {
                value = {
                  dxfBytes: out.dxfBytes(),
                  svg: out.svg(),
                  widthM: out.widthM(),
                  heightM: out.heightM()
                };
              } finally {
                out.free();
              }
              break;
            }
            case 'clash-batch-merge': {
              value = this.batch.mergeMasterDxf(request.columns, request.marginM);
              break;
            }
            case 'clash-roundtrip-apply': {
              const out = this.model.applyClashDxf(
                request.dxfBytes,
                request.selfModelId,
                request.openModelIds,
                request.exclude,
                request.conflict,
                request.nowMs
              );
              try {
                value = {
                  patchedIfcBytes: out.patchedIfcBytes(),
                  diffJson: out.diffJson()
                };
              } finally {
                out.free();
              }
              break;
            }
            default:
              throw new Error(`Unhandled op in FakeKernelWorker: ${request.op}`);
          }
          this.onmessage?.({ data: { id, ok: true, value } });
        } catch (err) {
          this.onmessage?.({
            data: { id, ok: false, code: 'internal', message: err instanceof Error ? err.message : String(err) }
          });
        }
      });
    }
    terminate() {
      if (this.terminated) return;
      this.terminated = true;
      this.batch?.free();
      this.batch = null;
      this.model?.free();
      this.model = null;
      activeWorkers--;
    }
  }

  globalThis.Worker = FakeKernelWorker;
  const adapter = new WasmKernelAdapter();
  try {
    const clashingIfc = [
      'ISO-10303-21;',
      'HEADER;',
      "FILE_DESCRIPTION(('ViewDefinition [DesignTransferView]'),'2;1');",
      "FILE_NAME('adapter_test.ifc','2026-11-23T00:00:00',(''),(''),'','','');",
      "FILE_SCHEMA(('IFC4'));",
      'ENDSEC;',
      'DATA;',
      "#1=IFCPROJECT('0000000000000000000001',$,'Bridge',$,$,$,$,(#10),#2);",
      '#2=IFCUNITASSIGNMENT((#3));',
      '#3=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);',
      "#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#11,$);",
      '#11=IFCAXIS2PLACEMENT3D(#12,$,$);',
      '#12=IFCCARTESIANPOINT((0.,0.,0.));',
      "#20=IFCSITE('0000000000000000000002',$,'Site',$,$,#21,$,$,.ELEMENT.,$,$,$,$,$);",
      '#21=IFCLOCALPLACEMENT($,#11);',
      "#22=IFCRELAGGREGATES('0000000000000000000003',$,$,$,#1,(#20));",
      '#30=IFCDIRECTION((0.,0.,1.));',
      '#31=IFCDIRECTION((1.,0.,0.));',
      '#32=IFCCARTESIANPOINT((0.02,0.02));',
      '#33=IFCAXIS2PLACEMENT2D(#32,$);',
      '#34=IFCRECTANGLEPROFILEDEF(.AREA.,$,#33,0.04,0.04);',
      '#35=IFCAXIS2PLACEMENT3D(#12,#31,#30);',
      '#36=IFCEXTRUDEDAREASOLID(#34,#35,#30,2.0);',
      "#37=IFCSHAPEREPRESENTATION(#10,'Body','SweptSolid',(#36));",
      '#38=IFCPRODUCTDEFINITIONSHAPE($,$,(#37));',
      '#101=IFCCARTESIANPOINT((0.,0.,0.));',
      '#102=IFCAXIS2PLACEMENT3D(#101,$,$);',
      '#103=IFCLOCALPLACEMENT(#21,#102);',
      "#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'Bar_A',$,$,#103,#38,$,$,0.02,$,$,$,$);",
      '#201=IFCCARTESIANPOINT((0.,0.01,0.01));',
      '#202=IFCAXIS2PLACEMENT3D(#201,$,$);',
      '#203=IFCLOCALPLACEMENT(#21,#202);',
      "#200=IFCTENDON('3P3Gs$u5Y8Ag9OPfx4GMPI',$,'Tendon_B',$,$,#203,#38,$,$,0.02,$,$,$,$,$,$,$,$);",
      "#300=IFCRELCONTAINEDINSPATIALSTRUCTURE('0000000000000000000004',$,$,$,(#100,#200),#20);",
      'ENDSEC;',
      'END-ISO-10303-21;'
    ].join('\r\n') + '\r\n';

    const encoder = new TextEncoder();
    const buf = encoder.encode(clashingIfc).buffer;
    await adapter.openModel('m-adapter', buf);
    assert.equal(activeWorkers, 1);

    const slab = await adapter.elementSlabSpan('m-adapter', 100, [1, 0], [1.0, 0.02, 0.02]);
    assert.ok(slab);
    assert.equal(slab.orientation, 'longitudinal');
    assert.equal(slab.depthM, 0);

    const batch = await adapter.exportClashAlignmentDxf({
      modelIds: ['m-adapter'],
      sections: [
        {
          fileName: 'individual/PK_0+001.00_001.dxf',
          pkM: 1.0,
          pkLabel: '0+001.00',
          origin: [0.99, 0, 0],
          normal: [1, 0, 0],
          alignmentPoint: [1.0, 0, 0],
          sMin: 0.99,
          sMax: 1.01,
          depthM: 0.02,
          modelId: 'm-adapter',
          clashAModelIds: ['m-adapter'],
          clashAIds: [100],
          clashAGuids: ['2O2Fr$t4X7Zf8NOew3FLOH'],
          clashBModelIds: ['m-adapter'],
          clashBIds: [200],
          clashPoints: [[1.0, 0.025, 0.025]],
          annotations: ['PK 0+001.00']
        }
      ],
      includeMasterGrid: true
    });
    assert.equal(batch.sections.length, 1);
    assert.ok(batch.masterDxfBytes && batch.masterDxfBytes.byteLength > 0);
    // Slicer worker must be terminated after batch finishes, leaving only the 1 model worker.
    assert.equal(activeWorkers, 1);

    // Edit INSERT in exported DXF and round-trip through adapter.applyClashDxfRoundTrip!
    const dxfText = new TextDecoder('utf-8').decode(batch.sections[0].dxfBytes);
    // F7: the export time travels in the metadata, so a later import can tell which edits of
    // other sections this drawing did not show.
    const stamp = /GUBIM_META [^\r\n]*\bt=(\d+)/.exec(dxfText);
    assert.ok(stamp && Math.abs(Number(stamp[1]) - Date.now()) < 600000, 'GUBIM_META t= is the export time');
    const origMatch = /GUBIM_ORIG id=100 (?:model=\S+ )?(?:guid=\S+ )?block=([^\s]+) (?:layer=\S+ )?x=([0-9.-]+) y=([0-9.-]+)/.exec(dxfText);
    assert.ok(origMatch);
    const [, blockName, origXStr, origYStr] = origMatch;
    const movedX = (Number(origXStr) + 0.05).toFixed(6);
    const movedY = (Number(origYStr) + 0.03).toFixed(6);
    const editedDxf = encoder.encode(
      dxfText.replace(
        `0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n${blockName}\r\n10\r\n${origXStr}\r\n20\r\n${origYStr}\r\n`,
        `0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n${blockName}\r\n10\r\n${movedX}\r\n20\r\n${movedY}\r\n`
      )
    );

    const rt = await adapter.applyClashDxfRoundTrip('fallback-id', editedDxf);
    assert.equal(rt.modelId, 'm-adapter', 'GUBIM_META modelId must route to the right open Worker');
    assert.equal(rt.applied, 1);
    assert.ok(rt.auditCsv.includes('m-adapter,1.000,move,100,'));
    assert.ok(rt.patchedIfcBytes.byteLength > clashingIfc.length);
    assert.ok(
      /\/\* GUBIM_F5 key=\S+ id=100 kind=move v=\S+ w=\S+ t=\d+ \*\//.test(new TextDecoder().decode(rt.patchedIfcBytes)),
      'the note carries the world move and the import time'
    );

    await adapter.closeModel('m-adapter');
    assert.equal(activeWorkers, 0);
  } finally {
    globalThis.Worker = prevWorker;
  }
});

await test('F7 conflict action bar wraps so all four choice buttons fit inside sidebar', () => {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const css = readFileSync(path.join(repoRoot, 'src/ui/styles/index.css'), 'utf8');
  const rule = /\.clash-alignment-actions\s*\{([^}]*)\}/.exec(css);
  assert.ok(rule, '.clash-alignment-actions rule must exist in index.css');
  assert.match(rule[1], /\bflex-wrap\s*:\s*wrap\b/, 'clash-alignment-actions must wrap across lines');
});

await test('multi-part IfcReinforcingBar sets classify individual clashing bar orientation and prune out-of-slab sub-meshes', () => {
  // #100: 6 longitudinal bars (each 10 m long along X, spaced across 5 m in Y so the combined box is 10x5 m).
  // #200: 5 transverse bars (each 4 m wide in Y, 0.04 m thick in X, spaced every 2 m along X).
  const multiPartIfc = [
    'ISO-10303-21;',
    'HEADER;',
    "FILE_DESCRIPTION(('ViewDefinition [DesignTransferView]'),'2;1');",
    "FILE_NAME('multipart_rebar.ifc','2026-11-27T00:00:00',(''),(''),'','','');",
    "FILE_SCHEMA(('IFC4'));",
    'ENDSEC;',
    'DATA;',
    "#1=IFCPROJECT('0000000000000000000001',$,'Bridge',$,$,$,$,(#10),#2);",
    '#2=IFCUNITASSIGNMENT((#3));',
    '#3=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);',
    "#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#11,$);",
    '#11=IFCAXIS2PLACEMENT3D(#12,$,$);',
    '#12=IFCCARTESIANPOINT((0.,0.,0.));',
    "#20=IFCSITE('0000000000000000000002',$,'Site',$,$,#21,$,$,.ELEMENT.,$,$,$,$,$);",
    '#21=IFCLOCALPLACEMENT($,#11);',
    "#22=IFCRELAGGREGATES('0000000000000000000003',$,$,$,#1,(#20));",
    '#40=IFCCARTESIANPOINT((0.,0.,0.));',
    '#41=IFCCARTESIANPOINT((10.,0.,0.));',
    '#42=IFCPOLYLINE((#40,#41));',
    '#43=IFCSWEPTDISKSOLID(#42,0.01,$,$,$);',
    '#44=IFCCARTESIANPOINT((0.,5.,0.));',
    '#45=IFCCARTESIANPOINT((10.,5.,0.));',
    '#46=IFCPOLYLINE((#44,#45));',
    '#47=IFCSWEPTDISKSOLID(#46,0.01,$,$,$);',
    "#48=IFCSHAPEREPRESENTATION(#10,'Body','AdvancedSweptSolid',(#43,#47));",
    '#49=IFCPRODUCTDEFINITIONSHAPE($,$,(#48));',
    "#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'LongSet',$,$,#21,#49,$,$,0.02,$,$,$,$);",
    '#50=IFCCARTESIANPOINT((1.,-2.,0.));',
    '#51=IFCCARTESIANPOINT((1.,2.,0.));',
    '#52=IFCPOLYLINE((#50,#51));',
    '#53=IFCSWEPTDISKSOLID(#52,0.01,$,$,$);',
    '#54=IFCCARTESIANPOINT((8.,-2.,0.));',
    '#55=IFCCARTESIANPOINT((8.,2.,0.));',
    '#56=IFCPOLYLINE((#54,#55));',
    '#57=IFCSWEPTDISKSOLID(#56,0.01,$,$,$);',
    "#58=IFCSHAPEREPRESENTATION(#10,'Body','AdvancedSweptSolid',(#53,#57));",
    '#59=IFCPRODUCTDEFINITIONSHAPE($,$,(#58));',
    "#200=IFCREINFORCINGBAR('3P3Gs$u5Y8Ag9OPfx4GMPI',$,'TransSet',$,$,#21,#59,$,$,0.02,$,$,$,$);",
    "#300=IFCRELCONTAINEDINSPATIALSTRUCTURE('0000000000000000000004',$,$,$,(#100,#200),#20);",
    'ENDSEC;',
    'END-ISO-10303-21;'
  ].join('\r\n') + '\r\n';

  const model = new wasm.Model(new TextEncoder().encode(multiPartIfc));
  try {
    // #100 has 2 longitudinal bars 5 m apart in Y; the bar at Y=0 must classify as longitudinal (1).
    const spanLong = model.elementSlabSpan(100, 1, 0, 1.0, 0.0, 0.0, 0.15, 2.5, 0.25);
    assert.ok(spanLong);
    assert.equal(spanLong[0], 1, 'multi-bar longitudinal set must classify clashing bar as longitudinal');
    assert.equal(spanLong[1], spanLong[2], 'longitudinal bar slab must have zero depth');

    // #200 has 2 transverse bars at X=1 and X=8; clash at X=1 must only span the bar at X=1 (~0.02 m, not 0.25 m).
    const spanTrans = model.elementSlabSpan(200, 1, 0, 1.0, 0.0, 0.0, 0.15, 2.5, 0.25);
    assert.ok(spanTrans);
    assert.equal(spanTrans[0], 0, 'multi-bar transverse set must classify clashing bar as transverse');
    assert.ok(spanTrans[2] - spanTrans[1] < 0.05, 'transverse bar slab must match single bar thickness (~20 mm)');

    // Culling at X in [0.98, 1.02] must prune the far sub-mesh at X=8 from slabChunkForElements.
    const culled = model.cullElementsBySlab(1, 0, 0.98, 1.02, 0.05);
    assert.ok(culled.includes(200));
    const chunkFiltered = model.slabChunkForElements(new Uint32Array([200]), undefined);
    // Culling covering both X=1 and X=8 includes both sub-meshes (larger chunk).
    model.cullElementsBySlab(1, 0, 0.0, 10.0, 0.05);
    const chunkAll = model.slabChunkForElements(new Uint32Array([200]), undefined);
    assert.ok(chunkFiltered.byteLength < chunkAll.byteLength, 'out-of-slab sub-mesh must be pruned from chunk');
  } finally {
    model.free();
  }
});

console.log(`\nt171-clash-dxf-export-test: ${passed}/${passed} pass`);
