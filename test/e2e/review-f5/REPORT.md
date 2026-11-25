# E2E review of the F5 DXF → IFC round trip (`fix/f5-roundtrip`)

Independent review, 2026-11-24. Scope: the uncommitted fix on top of `4455862`
(`git diff 4455862` plus `test/e2e/f5-check.mjs`). I tried to falsify the claims in
`docs/12_clash_alignment_dxf.md` §3.6 and `docs/05_kernel_contract.md` revision 14 through the real
browser UI. My own scenarios and expected values come from the generated IFC geometry, not from the
app's output.

## Verdict: CHANGES

The happy path works on all three renderers. That covers moves under rotated parents and under
`IfcGridPlacement` and `IfcLinearPlacement`, mm files, UTM coordinates, rebends in IFC2X3, IFC4 and
IFC4X3, v1 legacy files, decoy models and re-saved DXFs.

Claim 9 is false: a DXF moved the wrong model's bar by 6 m and the panel reported success.
Malformed DXF values become large silent moves. A zero-length polyline deletes a bar's geometry.
Claims 5 and 6 fail on revert and on two-step imports.

- Build: `npm run kernel` exit 0, `npm run build` exit 0.
- The browser console was clean in every scenario on every renderer.
- The repo was not modified during the review.

## How it was run

- Vite dev server on port 5199; headless Chrome over CDP through `test/e2e/cdp.mjs`.
- Every UI scenario ran on `gpu`, `gl2` and `none`, with identical results on each.
- Raw output: [`logs/final-gpu.log`](logs/final-gpu.log), [`logs/final-gl2.log`](logs/final-gl2.log),
  [`logs/final-none.log`](logs/final-none.log).
- Direct-kernel probes: [`logs/probe-node.json`](logs/probe-node.json).
- The per-scenario evidence (`checks.json`, `console.log`, edited DXFs, patched IFCs, STEP diffs and 30
  screenshots) went to a temp folder and was deleted afterwards (R9). `scenarios.mjs` regenerates it.
- I read all 30 screenshots. Their panel messages match the logs. Bars are 2 cm thick and not visible
  at the default view, so geometry verdicts rest on numeric readback, not on images.

### Calls outside the UI
- Geometry readback: `window.__gubim_kernel__.elementMesh(modelId, id)` in the page (read only).
- Node: the kernel (`wasm.Model(...).elementMesh`) re-reads the downloaded IFCs.
- Node: `Model.applyClashDxf` directly, for the `probe-node.mjs` cases only.

## Scenarios

| Scenario | Renderers | Result |
|---|---|---|
| `r12-utm`: UTM site (540000 / 4700000). Every DXF line ≤ 255 (longest 229); `GUBIM_META` v=2 split 229 + 65 with all 19 keys; full GlobalId in `GUBIM_ORIG`; move of bar10 (0.2, 0.3) and V-rebend of bar5 land at the predicted world coordinates | all 3 | PASS |
| `rebend-IFC2X3` / `-IFC4` / `-IFC4X3_ADD2`: V rebend. Body subcontext #8 used (not Axis #7). Per-schema entities (`IfcPolyline` + `0.,2.` / `IfcCartesianPointList3D` / `…,$`). Radius 0.01 from the bar. Only the bar record changed; GlobalIds identical. Node re-read: mesh present, no diagnostics | all 3 | PASS (one observation, F11) |
| `placements`: mm file with tilted and Z-rotated parents; `IfcGridPlacement` on a 30° grid (IFC4 and IFC4X3); `IfcLinearPlacement` on a straight segment (s=45) and an arc (s=420, heading 0.32 rad). Each moved by exactly dx·right + dy·Z | all 3 | PASS |
| `legacy-v1`: v1 META of 284 chars clipped to 255 (`nx`/`ny` lost). Move applied; rebend refused with «metadatos del DXF incompletos, vuelva a exportar el ZIP». The same v1 DXF from a later session with a decoy open moves the bridge only | all 3 | PASS |
| `guid-safety`: tampered GlobalId refused; earlier-session DXF with the decoy as primary moves the bridge only; decoy-only refused; no GlobalId refused «sin GlobalId» | all 3 | PASS except the reported reason (F8) |
| `patched-fresh`: STEP diff shows only 2 product records changed (one attribute each) and new ids appended. Reopened in a new session: geometry right, 2 clashes. Old DXF gives «ya aplicado». Fresh export from the patched file, edit, applied, 1 clash left | all 3 | PASS |
| `idempotence`: same DXF again gives «ya aplicado»; +0.30 then +0.50 applies only +0.20 | all 3 | PASS |
| `idempotence`: edit reverted to the exported position (move and rebend) | all 3 | **FAIL** (F4) |
| `move-rebend`: one import of INSERT +0.3 with the V drawn at the original place ends on the V; V drawn at the moved place ends there | all 3 | PASS |
| `move-rebend`: the same final DXF imported in two steps | all 3 | **FAIL** (F5) |
| `cad-resave`: LF + padded group codes, reversed entity order, exponent numbers, 999 comments stripped, LINE/CIRCLE deleted + `LWPOLYLINE` | all 3 | PASS |
| `cad-resave`: comma decimal / copied INSERT / zero-length polyline | all 3 | **FAIL** (F2, F6, F3) |
| `reasons-csv`: Spanish reasons in the panel; CSV has `status`, `reason`, `global_id` | all 3 | PASS |
| `reasons-csv`: CSV for an import where every edit was skipped | all 3 | FAIL (F10, pre-existing) |
| `federated`: two decks from one template (same expressIds), one section holding both bars | all 3 | **FAIL** (F1) |
| `tendon-two-sections`: transverse tendon rebend keeps r = 0.04 (attribute 10) | all 3 | PASS |
| `tendon-two-sections`: one element edited in two sections | all 3 | FAIL (F7) |
| `probe-node.mjs` (direct kernel, not the UI) on a UI-exported DXF | n/a | see F2, F3, F9, F12 |

## Findings (most severe first)

### F1 (High): federated section moves the wrong model's bar and drops edits silently (claims 9 and 2)

Repro:
1. Open `left.ifc` and `right.ifc`, generated from one template so the expressIds are the same. Right is 6 m to +Y.
2. Set A = «Todos los modelos», bars vs tendons. Export the ZIP. PK 0+105 holds both bar5 INSERTs (same id 84).
3. Try three edits:
   - (a) Move only the left INSERT up 0.3. Expected: applied, or skipped with a reason. Actual: «El archivo DXF no contiene desplazamientos…», and the edit is lost.
   - (c) Move the left INSERT and delete the right one. In the same session: skipped, «GlobalId distinto».
   - (d) Import the same DXF after a page reload with both decks reopened. Actual: «Parche STEP aplicado (1 elementos actualizados)». The right deck's bar moves from (1005, 2006, 0.5) to (1005, 2000, 0.8): 6 m sideways onto the left deck, plus the left bar's 0.3.

Cause:
- `GUBIM_ORIG`, `INSERT`s and `EDIT_A_` are keyed by expressId only: `crates/cad-slicer/src/roundtrip.rs:117,119,169,190`.
- The writer adds no model key: `crates/cad-slicer/src/clash_dxf.rs:348-353`.
- The last record wins, so the left INSERT is paired with the right `GUBIM_ORIG` and the right GlobalId.
- The earlier-session fallback then finds the right model by GlobalId: `src/infrastructure/kernel/WasmKernelAdapter.ts:985-1009`.
- In the same session, the named-model path should do the same when `clashA[0]` belongs to the other model (`src/ui/viewer/clashAlignmentExport.ts:101`). I could not force that order from the UI.

### F2 (High): an unparseable or empty coordinate becomes 0 and is applied as a large move
- UI: INSERT x written as `2,000000` (comma decimal) gives «1 elementos actualizados» and bar15 moves 2 m sideways (y 2000 → 2002).
- Node probe: an empty y moves the bar −0.5 m (z 0.5 → 0.0).
- Expected: the edit is refused with a reason.
- Cause: `roundtrip.rs:184-185` (`v.parse().unwrap_or(0.0)`); the same pattern for `VERTEX` at `:232-233`.

### F3 (High): a zero-length edit polyline is «applied» and the bar loses its geometry
- UI: both `EDIT_A_` vertices of bar15 at one point give «1 elementos actualizados». The model reloads with 8 meshes instead of 9, and `elementMesh(bar15)` is null.
- Node probe: a 1e300 move is applied too and also leaves no mesh.
- Expected: `invalid-points`.
- Cause: `crates/step-parser/src/patch.rs:243` checks only the vertex count and finiteness, not length or magnitude.

### F4 (Medium): reverting an edit is ignored (claim 5)
- After +0.50 is applied, re-importing the exported DXF unchanged reports «no contiene desplazamientos»; the bar stays at +0.50.
- The same happens for a rebend reverted to the exported straight polyline.
- Cause: the reader drops zero-delta INSERTs and unchanged polylines (`roundtrip.rs:282`, `:319`) before the earlier `GUBIM_F5` notes are compared. So «applies only the difference» fails when the difference returns to zero.

### F5 (Medium): the same final DXF gives different results by import order (claim 6)
- One import of INSERT +0.3 with the V: the bar is on the V (z 0.19–0.51).
- Importing the V first, then the same V plus INSERT +0.3, leaves the bar 0.3 m higher (z 0.49–0.81).
- Cause: the second import skips the rebend as `already-applied` and applies the move on top. `crates/wasm-api/src/clash_dxf.rs:286-299` (`moved_now` covers only the same import).

### F6 (Medium): a copied INSERT moves the element
- A CAD COPY of bar5's INSERT placed 2 m away, with the original untouched, is applied as a 2 m move when the copy is last in the file. A copy placed first is ignored.
- Expected: refuse as ambiguous. `duplicate-edit` exists in the patcher but is never reached.
- Cause: `roundtrip.rs:190` (map insert, last wins).

### F7 (Medium): one element edited in two sections has its edits summed without warning
T1 moved +0.10 in PK 105 and +0.20 in PK 110 ends at +0.30, which matches neither drawing. The idempotence key includes the PK (`crates/wasm-api/src/clash_dxf.rs:213-218`), so the two edits are treated as independent.

### F8 (Low): wrong reason shown
An earlier-session DXF already applied to the bridge, re-imported with the decoy as primary model, reports «GlobalId distinto» instead of «ya aplicado». When nothing applies, the first candidate's result is kept (`WasmKernelAdapter.ts:1005`).

### F9 (Low, Node probe): INSERT rotation and mirror are silently ignored
A rotation (code 50) plus a translation applies only the translation. A mirror (41 = −1) reports no edits.

### F10 (Low, already in `4455862`): result cleared after an import that applies nothing
After an import where every edit was skipped or nothing changed, the result is cleared (`src/ui/components/ClashPanel.tsx:401`):
- the CSV of skipped edits can't be downloaded (part of claim 8);
- «Descargar IFC parcheado» disappears although the loaded model is still patched.

This is more visible now that re-importing the same DXF changes nothing.

### F11 (Low, observation): rebend drops other representations
A rebend replaces the whole `IfcProductDefinitionShape`, so the bar's other representations are lost (here an `Axis` `Curve3D`).

### F12 (Low): no bound on move size
A 1e7 m move is applied (Node probe).

Out of scope: a CR-only (classic Mac) DXF fails with «DXF does not contain GUBIM_META metadata». A UTF-8 BOM works.

## Claims not fully tested
- **Long model ids:** the app generates the ids, so I couldn't make a long one through the UI. The v2 split was exercised by UTM coordinates (229 + 65); the reader joining a 120-char id was not tested.
- **Real CAD re-save:** no CAD tool was available. Re-saves were simulated (LF, padded codes, reordering, exponent format, 999 stripping, `LWPOLYLINE` conversion, deletions).
- **Same-session federated case** where the section's first `CLASH_A` belongs to the other model: the order isn't controllable from the UI. Predicted from the code to behave like F1 (d).

## Scripts in this folder

| File | What it does |
|---|---|
| `gen.mjs` | Independent IFC generators: bridge in IFC2X3 / IFC4 / IFC4X3, m or mm, UTM origin, lateral offset; grid placement; linear placement over `test/fixtures/authored/linear-placement.ifc`. Also the LandXML generator |
| `lib.mjs` | Own ZIP reader, DXF edit helpers (move INSERT, replace `EDIT_A_` polyline, kerb-based frame sign, GlobalId retarget), CDP browser session driver |
| `scenarios.mjs` | All UI scenarios above. `node scenarios.mjs <gpu\|gl2\|none> <baseUrl> [outRoot] [scenario,…]` with a Vite server at `baseUrl`. `outRoot` defaults to `<tmp>/gubim-e2e-review-f5` |
| `probe-node.mjs` | Malformed-input probes through `Model.applyClashDxf`, on a `cad-resave` output folder |
| `probe-models.mjs` | Checks that the generated models load with the intended bounding boxes |

The scripts are not wired into `test/e2e/run.mjs`. Some scenarios fail on purpose until F1–F10 are fixed.

## Status after the fixes (2026-11-25)

Rerun of `scenarios.mjs` on `gpu`, `gl2` and `none` against the fixed tree: **111/115 checks pass on each renderer**, with a clean console everywhere. The 4 that still failed were the then-open decisions F7 and F11 (F11 three times, once per schema; F7 once). After Nick's decisions (same day) and their fixes, with the F7 warning now driven by the scenario: **119/119 checks pass on each renderer**, console clean.

| Finding | Status | Fix |
|---|---|---|
| F1 | Fixed | `GUBIM_ORIG` names its block, edit layer and session model (`model=`); `GUBIM_POLY` names its block. The reader pairs by block and layer, not by expressId. The adapter applies the DXF to every open model: each edit goes to the model it came from, or by GlobalId when that model is not open, and an edit applied in one model is excluded in the others. |
| F2 | Fixed | An empty or unparseable number is refused (`invalid-number`). |
| F3 | Fixed | Repeated vertices are dropped; fewer than two distinct points is `invalid-points`. Moves or points beyond 10⁸ m are `out-of-range`. |
| F4 | Fixed | An element put back where the export had it reverts the earlier edit (move and rebend). |
| F5 | Fixed | When a later import moves a rebent element, the rebend is written again minus the move, so one import or two give the same result. |
| F6 | Fixed | A copied INSERT is `duplicate-insert`. With two polylines on one edit layer, the one still matching the export is ignored; two that both differ are `duplicate-polyline`. |
| F7 | Fixed (Nick: warn and let the user decide) | Sections carry their export time (`GUBIM_META t=`), notes their import time and world move. An element another section edited after this DXF was exported stops the import, and the panel asks: «Sustituir por este DXF», «Sumar las dos ediciones», «Omitir estos elementos» or «Cancelar». |
| F8 | Fixed | The reported reason is the owning model's (`already-applied` before `guid-mismatch`). |
| F9 | Fixed | A rotated, scaled or mirrored INSERT is `unsupported-transform`. |
| F10 | Fixed | The CSV, and the patched IFC of an earlier import, stay downloadable after an import that applies nothing. |
| F11 | Fixed | The rebend replaces `Body`, redraws `Axis` on the new curve and keeps every other representation. |
| F12 | Closed (Nick: no limit) | The 10⁸ m bound only rejects impossible numbers. |

The expectations of two checks changed with the fixes, and the edits sit next to them in `scenarios.mjs`:
- **NaN vertex:** it is now refused while reading the DXF, so the reason is `invalid-number` rather than `invalid-points`.
- **Federated (b):** that DXF shows the left INSERT at its exported position, so the F4 revert puts the left bar back.

`probe-node.mjs` follows the new `Model.applyClashDxf(dxf, selfModelId, openModelIds, exclude)` signature.
