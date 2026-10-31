// Runs the built WASM kernel in Node over every IFC file: the real binary and bindings,
// with the batch rules from GeometryBatch.ts. Run `npm run kernel` first.
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { findBatchError } from '../src/domain/kernel/GeometryBatch.ts';
import { initSync, kernelVersion, Model } from '../src/infrastructure/kernel/pkg/wasm_api.js';

const root = fileURLToPath(new URL('..', import.meta.url));
const pkg = join(root, 'src/infrastructure/kernel/pkg');
const wasm = readFileSync(join(pkg, 'wasm_api_bg.wasm'));
initSync({ module: wasm });

console.log(`kernel ${kernelVersion()}, ${(wasm.length / 1024 / 1024).toFixed(2)} MB wasm`);

const files = ['public/samples', 'test/fixtures'].flatMap((folder) =>
  readdirSync(join(root, folder))
    .filter((name) => name.endsWith('.ifc'))
    .map((name) => join(root, folder, name))
);

function details(model, ids) {
  let count = 0;
  for (const id of ids) {
    const json = model.elementDetailsJson(id);
    if (json) {
      JSON.parse(json);
      count += 1;
    }
  }
  return count;
}

let failures = 0;
for (const file of files) {
  const name = file.split(/[\\/]/).pop();
  const started = performance.now();
  const model = new Model(readFileSync(file));
  const summary = JSON.parse(model.summaryJson());
  const tree = JSON.parse(model.spatialTreeJson());

  model.startStream();
  const seen = new Set();
  let batches = 0;
  for (;;) {
    const view = model.nextBatch(65536);
    if (!view) break;
    const [x, y, z] = view.origin();
    const batch = {
      modelId: 'check',
      sequence: ++batches,
      origin: [x, y, z],
      positions: view.takePositions(),
      normals: view.takeNormals(),
      indices: view.takeIndices(),
      meshTable: view.takeMeshTable()
    };
    view.free();
    const error = findBatchError(batch);
    if (error) {
      console.error(`${name}: ${error}`);
      failures += 1;
    }
    for (let at = 0; at < batch.meshTable.length; at += 7) seen.add(batch.meshTable[at]);
  }
  const complete = JSON.parse(model.finishStream());
  const inspected = details(model, seen);
  model.free();

  if (tree.root.type !== 'IfcProject' || inspected !== seen.size) {
    console.error(`${name}: tree ${tree.root.type}, details ${inspected} of ${seen.size}`);
    failures += 1;
  }
  const ms = (performance.now() - started).toFixed(0);
  console.log(
    `${name.padEnd(36)} ${summary.schema.padEnd(7)} ${String(seen.size).padStart(4)} elements ` +
      `${String(complete.meshCount).padStart(4)} meshes ${String(batches).padStart(2)} batches ` +
      `${String(complete.diagnostics.length).padStart(3)} diag ${ms} ms`
  );
}

for (const [bytes, expected] of [
  [new TextEncoder().encode('not an ifc file'), /^unsupported-schema\||^parse\|/],
  [
    new TextEncoder().encode(
      "ISO-10303-21;HEADER;FILE_SCHEMA(('AP214'));ENDSEC;DATA;\nENDSEC;END-ISO-10303-21;"
    ),
    /^unsupported-schema\|/
  ]
]) {
  try {
    new Model(bytes);
    console.error('a bad file opened');
    failures += 1;
  } catch (error) {
    if (!expected.test(error.message)) {
      console.error(`unexpected error: ${error.message}`);
      failures += 1;
    }
  }
}

console.log(failures === 0 ? `all ${files.length} files pass` : `${failures} failures`);
process.exit(failures === 0 ? 0 : 1);
