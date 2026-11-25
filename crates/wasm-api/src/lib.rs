//! WebAssembly facade of the kernel. Owner: Claude (T-033). Contract: `docs/05_kernel_contract.md`.

pub mod batches;
pub mod clash;
pub mod clash_dxf;
pub mod drawing;
mod encode;
mod json;
pub mod wire;
pub mod loaded;
pub mod shape;

use drawing::{DrawingRequest, ExportOptions, SheetStore};
use loaded::{LoadedModel, OpenError};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};
use wasm_bindgen::prelude::*;

static ALLOCATION_FAILED: AtomicBool = AtomicBool::new(false);

/// Remembers a failed allocation. Rust then aborts, which in WASM is a bare `unreachable` trap
/// that JS cannot tell from any other bug.
struct TrackingAllocator;

impl TrackingAllocator {
    fn track(pointer: *mut u8) -> *mut u8 {
        if pointer.is_null() {
            ALLOCATION_FAILED.store(true, Ordering::Relaxed);
        }
        pointer
    }
}

// SAFETY: every call forwards to `System` with the caller's own arguments.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        Self::track(unsafe { System.alloc(layout) })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        Self::track(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        Self::track(unsafe { System.realloc(pointer, layout, size) })
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

/// Kernel version, so the app can detect a stale `.wasm` next to newer TypeScript.
#[wasm_bindgen(js_name = kernelVersion)]
pub fn kernel_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Whether memory ran out. Safe to call after a trap: it touches no `Model`.
#[wasm_bindgen(js_name = allocationFailed)]
pub fn allocation_failed() -> bool {
    ALLOCATION_FAILED.load(Ordering::Relaxed)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = crypto, js_name = getRandomValues, catch)]
    fn get_random_values(bytes: &mut [u8]) -> Result<(), JsValue>;
}

/// Gives the geometry's hash maps a fresh secret from the host's random source, so a file cannot
/// be written to make their keys collide. Without one they keep the secret they had.
fn renew_hash_secret() {
    #[cfg(target_arch = "wasm32")]
    {
        let mut bytes = [0u8; 32];
        if get_random_values(&mut bytes).is_ok() {
            let mut words = [0u64; 4];
            for (word, chunk) in words.iter_mut().zip(bytes.as_chunks::<8>().0) {
                *word = u64::from_le_bytes(*chunk);
            }
            ifc_geometry::hash::set_secret(words);
        }
    }
}

/// Encoded as `code|offset|message`, which the Worker turns into a `KernelError`.
fn open_error(error: OpenError) -> JsError {
    match error {
        OpenError::Parse(error) => JsError::new(&format!("parse|{}|{error}", error.offset())),
        OpenError::UnsupportedSchema(identifiers) => JsError::new(&format!(
            "unsupported-schema||schema not supported: {}",
            identifiers.join(", ")
        )),
    }
}

#[wasm_bindgen]
pub struct Model {
    inner: LoadedModel,
}

#[wasm_bindgen]
impl Model {
    #[wasm_bindgen(constructor)]
    pub fn open(bytes: Vec<u8>) -> Result<Model, JsError> {
        renew_hash_secret();
        LoadedModel::open(bytes).map(|inner| Model { inner }).map_err(open_error)
    }

    #[wasm_bindgen(js_name = summaryJson)]
    pub fn summary_json(&self) -> String {
        self.inner.summary_json()
    }

    #[wasm_bindgen(js_name = spatialTreeJson)]
    pub fn spatial_tree_json(&mut self) -> Option<String> {
        self.inner.spatial_tree_json()
    }

    #[wasm_bindgen(js_name = elementDetailsJson)]
    pub fn element_details_json(&self, express_id: u32) -> Option<String> {
        self.inner.element_details_json(express_id)
    }

    /// The products the stream draws, with their properties as the panel shows them.
    #[wasm_bindgen(js_name = elementSnapshotsJson)]
    pub fn element_snapshots_json(&self) -> String {
        self.inner.element_snapshots_json()
    }

    /// Ascending expressIds. See `KernelPort.findElements`.
    #[wasm_bindgen(js_name = findElements)]
    pub fn find_elements(
        &self,
        field: &str,
        text: &str,
        property_name: Option<String>,
    ) -> Result<Vec<u32>, JsError> {
        self.inner
            .find_elements(field, text, property_name.as_deref())
            .ok_or_else(|| JsError::new(&format!("internal||unknown search field: {field}")))
    }

    /// See `KernelPort.elementMesh`.
    #[wasm_bindgen(js_name = elementMesh)]
    pub fn element_mesh(&mut self, express_id: u32) -> Option<ShapeView> {
        self.inner.element_mesh(express_id).map(|shape| ShapeView { shape })
    }

    /// Compact `f64` AABBs `[id, minX, minY, minZ, maxX, maxY, maxZ]` for clash broad phase.
    #[wasm_bindgen(js_name = elementBounds)]
    pub fn element_bounds(&mut self, express_ids: Vec<u32>) -> Vec<f64> {
        self.inner.element_bounds(&express_ids)
    }

    /// Raw `f64` triangle mesh without feature edges, for clash narrow phase.
    #[wasm_bindgen(js_name = rawElementMesh)]
    pub fn raw_element_mesh(&mut self, express_id: u32) -> Option<RawMeshView> {
        self.inner
            .raw_element_mesh(express_id)
            .map(|mesh| RawMeshView { mesh })
    }

    /// Computes Element A's slab span `[orientation, s_min, s_max, s_clash, delta_s_a, width_perp_m,
    /// height_perp_m]` inside the Worker via `clash_detector::compute_element_a_slab` (F2/F4).
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = elementSlabSpan)]
    pub fn element_slab_span(
        &mut self,
        express_id: u32,
        tx: f64,
        ty: f64,
        px: f64,
        py: f64,
        pz: f64,
        long_min_extent_m: f64,
        long_aspect_ratio: f64,
        max_transverse_slab_m: f64,
    ) -> Option<Vec<f64>> {
        self.inner
            .element_slab_span(
                express_id,
                tx,
                ty,
                px,
                py,
                pz,
                long_min_extent_m,
                long_aspect_ratio,
                max_transverse_slab_m,
            )
            .map(|arr| arr.to_vec())
    }

    /// 3D slab-vs-AABB culling along horizontal tangent `(tx, ty, 0)` (F3).
    #[wasm_bindgen(js_name = cullElementsBySlab)]
    pub fn cull_elements_by_slab(
        &mut self,
        tx: f64,
        ty: f64,
        s_min: f64,
        s_max: f64,
        eps: f64,
    ) -> Vec<u32> {
        self.inner.cull_elements_by_slab(tx, ty, s_min, s_max, eps)
    }

    /// Evaluates only the given culled `express_ids` into a drawing chunk for slab slicing (F3).
    #[wasm_bindgen(js_name = slabChunkForElements)]
    pub fn slab_chunk_for_elements(
        &mut self,
        express_ids: Vec<u32>,
        transform: Option<Vec<f64>>,
    ) -> Vec<u8> {
        let xf = transform
            .and_then(|v| <[f64; 16]>::try_from(v.as_slice()).ok())
            .map(|arr| glam::DMat4::from_cols_array(&arr));
        self.inner.slab_chunk_for_elements(&express_ids, xf)
    }

    #[wasm_bindgen(js_name = startStream)]
    pub fn start_stream(&mut self) {
        self.inner.start_stream();
    }

    /// The Worker pulls the next batch only once it can send it.
    #[wasm_bindgen(js_name = nextBatch)]
    pub fn next_batch(&mut self, max_triangles: u32) -> Option<BatchView> {
        self.inner
            .next_batch(max_triangles as usize)
            .map(|batch| BatchView { batch: wire::pack(batch) })
    }

    #[wasm_bindgen(js_name = processedElements)]
    pub fn processed_elements(&self) -> u32 {
        self.inner.processed_elements() as u32
    }

    #[wasm_bindgen(js_name = totalElements)]
    pub fn total_elements(&self) -> u32 {
        self.inner.total_elements() as u32
    }

    #[wasm_bindgen(js_name = finishStream)]
    pub fn finish_stream(&mut self) -> String {
        self.inner.finish_stream()
    }

    /// The numbers of `DrawingRequest::decode`.
    #[wasm_bindgen(js_name = startDrawing)]
    pub fn start_drawing(&mut self, request: Vec<f64>) -> Result<(), JsError> {
        let request = DrawingRequest::decode(&request).ok_or_else(bad_request)?;
        self.inner.start_drawing(request);
        Ok(())
    }

    #[wasm_bindgen(js_name = nextDrawingChunk)]
    pub fn next_drawing_chunk(&mut self, max_triangles: u32) -> Option<Vec<u8>> {
        self.inner.next_drawing_chunk(max_triangles as usize)
    }

    #[wasm_bindgen(js_name = finishDrawing)]
    pub fn finish_drawing(&mut self) -> String {
        self.inner.finish_drawing()
    }

    /// Parses an edited clash DXF and surgically patches this open model's STEP bytes (F5).
    /// `self_model_id` is this model's session id and `open_model_ids` every open model's:
    /// an edit lands on the model it was exported from, or, when that model is not open, on
    /// the product carrying its `GlobalId`. `exclude` lists edit indices already applied to
    /// another model in this import. `conflict` (`ask`, `add`, `replace` or `skip`) handles
    /// elements another section edited after this DXF was exported, and `now_ms` is written
    /// into the notes. See `applyClashDxfRoundTrip` in `docs/05`.
    #[wasm_bindgen(js_name = applyClashDxf)]
    pub fn apply_clash_dxf(
        &self,
        dxf_bytes: Vec<u8>,
        self_model_id: String,
        open_model_ids: Vec<String>,
        exclude: Vec<u32>,
        conflict: String,
        now_ms: f64,
    ) -> Result<ClashRoundTripOutput, JsError> {
        let scope = clash_dxf::ApplyScope {
            self_model_id: Some(self_model_id),
            open_model_ids,
            exclude,
            require_global_id: true,
            conflict: clash_dxf::ConflictPolicy::parse(&conflict),
            now_ms: (now_ms.is_finite() && now_ms >= 0.0).then_some(now_ms),
        };
        let (patched_ifc, diff_json) = self
            .inner
            .apply_clash_dxf(&dxf_bytes, &scope)
            .map_err(|msg| JsError::new(&format!("parse||{msg}")))?;
        Ok(ClashRoundTripOutput {
            patched_ifc,
            diff_json,
        })
    }
}

#[wasm_bindgen]
pub struct ClashRoundTripOutput {
    patched_ifc: Vec<u8>,
    diff_json: String,
}

#[wasm_bindgen]
impl ClashRoundTripOutput {
    #[wasm_bindgen(js_name = patchedIfcBytes)]
    pub fn patched_ifc_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.patched_ifc)
    }

    #[wasm_bindgen(js_name = diffJson)]
    pub fn diff_json(&self) -> String {
        self.diff_json.clone()
    }
}

fn bad_request() -> JsError {
    JsError::new("internal||malformed drawing request")
}

/// Sequential clash cross-section batch slicer and master DXF grid combiner (F3/F4).
#[wasm_bindgen]
pub struct ClashBatchSlicer {
    inner: clash_dxf::ClashBatchStore,
}

#[wasm_bindgen]
pub struct ClashSectionOutput {
    dxf: Vec<u8>,
    svg: String,
    width_m: f64,
    height_m: f64,
}

#[wasm_bindgen]
impl ClashSectionOutput {
    #[wasm_bindgen(js_name = dxfBytes)]
    pub fn dxf_bytes(&self) -> Vec<u8> {
        self.dxf.clone()
    }

    pub fn svg(&self) -> String {
        self.svg.clone()
    }

    #[wasm_bindgen(js_name = widthM)]
    pub fn width_m(&self) -> f64 {
        self.width_m
    }

    #[wasm_bindgen(js_name = heightM)]
    pub fn height_m(&self) -> f64 {
        self.height_m
    }
}

#[wasm_bindgen]
impl ClashBatchSlicer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> ClashBatchSlicer {
        ClashBatchSlicer {
            inner: clash_dxf::ClashBatchStore::new(),
        }
    }

    #[wasm_bindgen(js_name = addSectionChunk)]
    pub fn add_section_chunk(&mut self, model_index: u32, chunk: Vec<u8>) -> bool {
        self.inner.add_section_chunk(model_index, &chunk)
    }

    /// Export time (ms since the Unix epoch) the following sections carry in `GUBIM_META t=`,
    /// so a re-import can tell which edits from other sections the drawing did not show.
    #[wasm_bindgen(js_name = setExportTime)]
    pub fn set_export_time(&mut self, ms: f64) {
        self.inner.set_export_time_ms(ms);
    }

    #[wasm_bindgen(js_name = clearCurrentSection)]
    pub fn clear_current_section(&mut self) {
        self.inner.clear_current_section();
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = finishSection)]
    pub fn finish_section(
        &mut self,
        ox: f64,
        oy: f64,
        oz: f64,
        nx: f64,
        ny: f64,
        nz: f64,
        depth_m: f64,
        clash_a_model_indices: Vec<u32>,
        clash_a_ids: Vec<u32>,
        clash_a_guids: Vec<String>,
        clash_a_owners: Vec<String>,
        clash_b_model_indices: Vec<u32>,
        clash_b_ids: Vec<u32>,
        clash_points_flat: Vec<f64>,
        align_x: f64,
        align_y: f64,
        align_z: f64,
        model_id: String,
        pk_m: f64,
        pk_label: String,
        s_min: f64,
        s_max: f64,
        annotations: Vec<String>,
    ) -> ClashSectionOutput {
        let a_refs: Vec<(u32, u32, String)> = clash_a_ids
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    clash_a_model_indices.get(i).copied().unwrap_or(0),
                    id,
                    clash_a_guids.get(i).cloned().unwrap_or_default(),
                )
            })
            .collect();
        let b_refs: Vec<(u32, u32)> = clash_b_ids
            .into_iter()
            .enumerate()
            .map(|(i, id)| (clash_b_model_indices.get(i).copied().unwrap_or(0), id))
            .collect();
        let pts: Vec<[f64; 3]> = clash_points_flat.as_chunks::<3>().0.to_vec();
        let align_pt = (align_x.is_finite() && align_y.is_finite() && align_z.is_finite())
            .then_some([align_x, align_y, align_z]);
        let res = self.inner.finish_section(
            [ox, oy, oz],
            [nx, ny, nz],
            depth_m,
            a_refs,
            clash_a_owners,
            b_refs,
            pts,
            align_pt,
            model_id,
            pk_m,
            pk_label,
            s_min,
            s_max,
            annotations,
        );
        ClashSectionOutput {
            dxf: res.dxf_bytes,
            svg: res.svg,
            width_m: res.width_m,
            height_m: res.height_m,
        }
    }

    #[wasm_bindgen(js_name = mergeMasterDxf)]
    pub fn merge_master_dxf(&self, columns: u32, margin_m: f64) -> Vec<u8> {
        let cols = (columns > 0).then_some(columns as usize);
        self.inner.merge_master_dxf(cols, margin_m)
    }

    #[wasm_bindgen(js_name = sectionCount)]
    pub fn section_count(&self) -> u32 {
        self.inner.section_count() as u32
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

impl Default for ClashBatchSlicer {
    fn default() -> Self {
        Self::new()
    }
}

/// Standalone F2 slab classification using `clash_detector::compute_element_a_slab` directly.
#[allow(clippy::too_many_arguments)]
#[wasm_bindgen(js_name = computeElementASlab)]
pub fn compute_element_a_slab_wasm(
    vertices_flat: Vec<f64>,
    tx: f64,
    ty: f64,
    px: f64,
    py: f64,
    pz: f64,
    long_min_extent_m: f64,
    long_aspect_ratio: f64,
    max_transverse_slab_m: f64,
) -> Vec<f64> {
    let verts: Vec<glam::DVec3> = vertices_flat
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| glam::DVec3::from_array(*c))
        .collect();
    let cfg = clash_detector::SlabConfig {
        longitudinal_min_extent_m: if long_min_extent_m.is_finite() && long_min_extent_m > 0.0 {
            long_min_extent_m
        } else {
            clash_detector::DEFAULT_LONGITUDINAL_MIN_EXTENT_M
        },
        longitudinal_aspect_ratio: if long_aspect_ratio.is_finite() && long_aspect_ratio > 0.0 {
            long_aspect_ratio
        } else {
            clash_detector::DEFAULT_LONGITUDINAL_ASPECT_RATIO
        },
        max_transverse_slab_m: if max_transverse_slab_m.is_finite() && max_transverse_slab_m > 0.0 {
            max_transverse_slab_m
        } else {
            clash_detector::DEFAULT_MAX_TRANSVERSE_SLAB_M
        },
        ..clash_detector::SlabConfig::default()
    };
    let span = clash_detector::compute_element_a_slab(
        &verts,
        [tx, ty],
        glam::DVec3::new(px, py, pz),
        &cfg,
    );
    let orient = match span.orientation {
        clash_detector::ElementOrientation::Transverse => 0.0,
        clash_detector::ElementOrientation::Longitudinal => 1.0,
    };
    vec![
        orient,
        span.s_min,
        span.s_max,
        span.s_clash,
        span.delta_s_a,
        span.width_perp_m,
        span.height_perp_m,
    ]
}

/// Parses an edited clash DXF and returns detected Level 1 (`Move`) and Level 2 (`Rebend`)
/// edits as JSON (F5, `docs/12_clash_alignment_dxf.md` §3.5).
#[wasm_bindgen(js_name = diffClashDxfJson)]
pub fn diff_clash_dxf_json(dxf_bytes: Vec<u8>) -> Result<String, JsError> {
    let diff = cad_slicer::parse_clash_dxf_edits(&dxf_bytes)
        .map_err(|msg| JsError::new(&format!("parse||{msg}")))?;
    Ok(clash_dxf::format_detected_json(&diff))
}

/// Applies surgical Level 1 (`MovePlacement`) STEP patches encoded as flat `[expressId, dx, dy, dz, ...]`
/// in file length units, returning the patched IFC bytes (F5).
#[wasm_bindgen(js_name = patchIfcStepMoves)]
pub fn patch_ifc_step_moves(
    original_ifc: Vec<u8>,
    moves_flat: Vec<f64>,
) -> Result<Vec<u8>, JsError> {
    let mut patches = Vec::new();
    for chunk in moves_flat.as_chunks::<4>().0 {
        patches.push(step_parser::StepPatch::MovePlacement {
            express_id: chunk[0] as u32,
            delta_file_units: [chunk[1], chunk[2], chunk[3]],
            expected_global_id: None,
            note: None,
        });
    }
    let (patched, _) = step_parser::patch_step_file(&original_ifc, &patches)
        .map_err(|msg| JsError::new(&format!("internal||{msg}")))?;
    Ok(patched)
}

/// Standalone F5 round-trip helper: diffs `dxf_bytes` and applies both Level 1 (`Move`) and
/// Level 2 (`Rebend`) surgical STEP patches to `original_ifc` given `length_unit_scale` (metres/unit).
#[wasm_bindgen(js_name = patchIfcStepFromDxf)]
pub fn patch_ifc_step_from_dxf(
    original_ifc: Vec<u8>,
    dxf_bytes: Vec<u8>,
    length_unit_scale: f64,
) -> Result<ClashRoundTripOutput, JsError> {
    let (patched_ifc, diff_json) =
        clash_dxf::apply_clash_dxf_to_step(&original_ifc, length_unit_scale, &dxf_bytes)
            .map_err(|msg| JsError::new(&format!("parse||{msg}")))?;
    Ok(ClashRoundTripOutput {
        patched_ifc,
        diff_json,
    })
}

/// One drawing, in a Worker of its own. See `docs/05` section 6.
#[wasm_bindgen]
pub struct Sheet {
    inner: SheetStore,
}

#[wasm_bindgen]
impl Sheet {
    #[wasm_bindgen(constructor)]
    pub fn new(request: Vec<f64>) -> Result<Sheet, JsError> {
        let request = DrawingRequest::decode(&request).ok_or_else(bad_request)?;
        Ok(Sheet { inner: SheetStore::new(request) })
    }

    #[wasm_bindgen(js_name = addChunk)]
    pub fn add_chunk(&mut self, model_index: u32, chunk: Vec<u8>) {
        self.inner.add_chunk(model_index, &chunk);
    }

    #[wasm_bindgen(js_name = dropModel)]
    pub fn drop_model(&mut self, model_index: u32) {
        self.inner.drop_model(model_index);
    }

    pub fn generate(&mut self) -> String {
        self.inner.generate()
    }

    /// `None` before `generate`.
    pub fn export(
        &self,
        svg: bool,
        scale: f64,
        show_hidden: bool,
        show_hatching: bool,
        comment: Option<String>,
    ) -> Option<Vec<u8>> {
        self.inner.export(&ExportOptions { svg, scale, show_hidden, show_hatching, comment })
    }
}

/// Each `take` copies one array out of WASM memory into a buffer JS owns, ready to transfer.
/// Positions, normals and indices come in one of two widths (contract v2, `docs/05` section 3):
/// the `take` of the other width returns an empty array.
#[wasm_bindgen]
pub struct BatchView {
    batch: wire::WireBatch,
}

impl BatchView {
    fn shape(&self, at: u32) -> Option<&wire::WireShape> {
        self.batch.shapes.get(at as usize)
    }

    fn shape_mut(&mut self, at: u32) -> Option<&mut wire::WireShape> {
        self.batch.shapes.get_mut(at as usize)
    }
}

#[wasm_bindgen]
impl BatchView {
    pub fn origin(&self) -> Vec<f64> {
        self.batch.origin.to_vec()
    }

    /// `box` as `[minX, minY, minZ, maxX, maxY, maxZ]`, relative to the origin.
    pub fn bounds(&self) -> Vec<f32> {
        [self.batch.min, self.batch.max].concat()
    }

    /// `quantBox` in the same layout: the grid of `q16` positions, and in `f32` the least and
    /// greatest position. Without instances it equals `bounds`; `undefined` when the batch has
    /// only instances.
    #[wasm_bindgen(js_name = quantBox)]
    pub fn quant_box(&self) -> Option<Vec<f32>> {
        self.batch.quant.map(|(min, max)| [min, max].concat())
    }

    /// Four words per instance: express id, shape id, rgba, flags (A13).
    #[wasm_bindgen(js_name = takeInstanceTable)]
    pub fn take_instance_table(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.batch.instance_table)
    }

    /// Twelve per instance: a 3x4 matrix by columns, shape to batch-local coordinates.
    #[wasm_bindgen(js_name = takeInstanceTransforms)]
    pub fn take_instance_transforms(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.batch.instance_transforms)
    }

    /// Shapes this batch sends first, read one at a time by index.
    #[wasm_bindgen(js_name = shapeCount)]
    pub fn shape_count(&self) -> u32 {
        self.batch.shapes.len() as u32
    }

    #[wasm_bindgen(js_name = shapeId)]
    pub fn shape_id(&self, at: u32) -> u32 {
        self.shape(at).map_or(0, |shape| shape.shape_id)
    }

    /// The shape's exact box, and its grid in `q16`: `[minX, minY, minZ, maxX, maxY, maxZ]`.
    #[wasm_bindgen(js_name = shapeBox)]
    pub fn shape_box(&self, at: u32) -> Vec<f32> {
        self.shape(at).map_or_else(Vec::new, |shape| [shape.min, shape.max].concat())
    }

    #[wasm_bindgen(js_name = shapeClosed)]
    pub fn shape_closed(&self, at: u32) -> bool {
        self.shape(at).is_some_and(|shape| shape.closed)
    }

    /// `"f32"` or `"q16"`.
    #[wasm_bindgen(js_name = shapeVertexFormat)]
    pub fn shape_vertex_format(&self, at: u32) -> String {
        match self.shape(at).map(|shape| &shape.vertices) {
            Some(wire::Vertices::Q16 { .. }) => "q16".into(),
            _ => "f32".into(),
        }
    }

    #[wasm_bindgen(js_name = takeShapePositions)]
    pub fn take_shape_positions(&mut self, at: u32) -> Vec<f32> {
        match self.shape_mut(at).map(|shape| &mut shape.vertices) {
            Some(wire::Vertices::F32 { positions, .. }) => std::mem::take(positions),
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeShapePositionsQ16)]
    pub fn take_shape_positions_q16(&mut self, at: u32) -> Vec<u16> {
        match self.shape_mut(at).map(|shape| &mut shape.vertices) {
            Some(wire::Vertices::Q16 { positions, .. }) => std::mem::take(positions),
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeShapeNormals)]
    pub fn take_shape_normals(&mut self, at: u32) -> Vec<f32> {
        match self.shape_mut(at).map(|shape| &mut shape.vertices) {
            Some(wire::Vertices::F32 { normals, .. }) => std::mem::take(normals),
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeShapeNormalsOct)]
    pub fn take_shape_normals_oct(&mut self, at: u32) -> Vec<i16> {
        match self.shape_mut(at).map(|shape| &mut shape.vertices) {
            Some(wire::Vertices::Q16 { normals, .. }) => std::mem::take(normals),
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = shapeIndicesAreU16)]
    pub fn shape_indices_are_u16(&self, at: u32) -> bool {
        matches!(self.shape(at).map(|shape| &shape.indices), Some(wire::Indices::U16(_)))
    }

    #[wasm_bindgen(js_name = takeShapeIndices)]
    pub fn take_shape_indices(&mut self, at: u32) -> Vec<u32> {
        match self.shape_mut(at).map(|shape| &mut shape.indices) {
            Some(wire::Indices::U32(indices)) => std::mem::take(indices),
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeShapeIndicesU16)]
    pub fn take_shape_indices_u16(&mut self, at: u32) -> Vec<u16> {
        match self.shape_mut(at).map(|shape| &mut shape.indices) {
            Some(wire::Indices::U16(indices)) => std::mem::take(indices),
            _ => Vec::new(),
        }
    }

    /// `"f32"` or `"q16"`.
    #[wasm_bindgen(js_name = vertexFormat)]
    pub fn vertex_format(&self) -> String {
        match self.batch.vertices {
            wire::Vertices::F32 { .. } => "f32".into(),
            wire::Vertices::Q16 { .. } => "q16".into(),
        }
    }

    /// `f32`: xyz per vertex.
    #[wasm_bindgen(js_name = takePositions)]
    pub fn take_positions(&mut self) -> Vec<f32> {
        match &mut self.batch.vertices {
            wire::Vertices::F32 { positions, .. } => std::mem::take(positions),
            wire::Vertices::Q16 { .. } => Vec::new(),
        }
    }

    /// `q16`: x, y, z on the grid and the record index, per vertex.
    #[wasm_bindgen(js_name = takePositionsQ16)]
    pub fn take_positions_q16(&mut self) -> Vec<u16> {
        match &mut self.batch.vertices {
            wire::Vertices::Q16 { positions, .. } => std::mem::take(positions),
            wire::Vertices::F32 { .. } => Vec::new(),
        }
    }

    /// `f32`: xyz per vertex.
    #[wasm_bindgen(js_name = takeNormals)]
    pub fn take_normals(&mut self) -> Vec<f32> {
        match &mut self.batch.vertices {
            wire::Vertices::F32 { normals, .. } => std::mem::take(normals),
            wire::Vertices::Q16 { .. } => Vec::new(),
        }
    }

    /// `q16`: two octahedral components per vertex.
    #[wasm_bindgen(js_name = takeNormalsOct)]
    pub fn take_normals_oct(&mut self) -> Vec<i16> {
        match &mut self.batch.vertices {
            wire::Vertices::Q16 { normals, .. } => std::mem::take(normals),
            wire::Vertices::F32 { .. } => Vec::new(),
        }
    }

    /// Whether the indices are 16-bit: `takeIndicesU16` then holds them.
    #[wasm_bindgen(js_name = indicesAreU16)]
    pub fn indices_are_u16(&self) -> bool {
        matches!(self.batch.indices, wire::Indices::U16(_))
    }

    #[wasm_bindgen(js_name = takeIndices)]
    pub fn take_indices(&mut self) -> Vec<u32> {
        match &mut self.batch.indices {
            wire::Indices::U32(indices) => std::mem::take(indices),
            wire::Indices::U16(_) => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeIndicesU16)]
    pub fn take_indices_u16(&mut self) -> Vec<u16> {
        match &mut self.batch.indices {
            wire::Indices::U16(indices) => std::mem::take(indices),
            wire::Indices::U32(_) => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = takeMeshTable)]
    pub fn take_mesh_table(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.batch.mesh_table)
    }
}

/// As `BatchView`, for one element's shape.
#[wasm_bindgen]
pub struct ShapeView {
    shape: shape::ElementShape,
}

#[wasm_bindgen]
impl ShapeView {
    #[wasm_bindgen(js_name = takePositions)]
    pub fn take_positions(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.shape.positions)
    }

    #[wasm_bindgen(js_name = takeIndices)]
    pub fn take_indices(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.shape.indices)
    }

    #[wasm_bindgen(js_name = takeEdges)]
    pub fn take_edges(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.shape.edges)
    }
}

/// Raw `f64` positions and `u32` triangle indices for clash narrow phase.
#[wasm_bindgen]
pub struct RawMeshView {
    mesh: clash::RawElementMesh,
}

#[wasm_bindgen]
impl RawMeshView {
    #[wasm_bindgen(js_name = takePositions)]
    pub fn take_positions(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.mesh.positions)
    }

    #[wasm_bindgen(js_name = takeIndices)]
    pub fn take_indices(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.mesh.indices)
    }
}

/// Dedicated narrow-phase clash session running in its own short-lived Worker (`docs/11_clash.md`).
#[wasm_bindgen]
pub struct ClashSession {
    inner: clash::ClashSessionStore,
}

#[wasm_bindgen]
impl ClashSession {
    #[wasm_bindgen(constructor)]
    pub fn new(max_cache_bytes: u32) -> ClashSession {
        ClashSession {
            inner: clash::ClashSessionStore::new(max_cache_bytes as usize),
        }
    }

    #[wasm_bindgen(js_name = broadPhase)]
    pub fn broad_phase(&self, a_flat: Vec<f64>, b_flat: Vec<f64>, clearance_m: f64) -> Vec<u32> {
        clash::ClashSessionStore::broad_phase(&a_flat, &b_flat, clearance_m)
    }

    #[wasm_bindgen(js_name = hasCached)]
    pub fn has_cached(&self, model_index: u32, express_id: u32) -> bool {
        self.inner.has_cached(model_index, express_id)
    }

    #[wasm_bindgen(js_name = cacheBytes)]
    pub fn cache_bytes(&self) -> u32 {
        self.inner.cache_bytes() as u32
    }

    #[wasm_bindgen(js_name = cacheMesh)]
    pub fn cache_mesh(
        &mut self,
        model_index: u32,
        express_id: u32,
        positions: Vec<f64>,
        indices: Vec<u32>,
    ) -> bool {
        self.inner.cache_mesh(model_index, express_id, &positions, &indices)
    }

    #[wasm_bindgen(js_name = setActiveMesh)]
    pub fn set_active_mesh(
        &mut self,
        model_index: u32,
        express_id: u32,
        positions: Vec<f64>,
        indices: Vec<u32>,
    ) -> bool {
        self.inner
            .set_active_mesh(model_index, express_id, &positions, &indices)
    }

    #[wasm_bindgen(js_name = testActiveAgainstCached)]
    pub fn test_active_against_cached(
        &mut self,
        cached_model: u32,
        cached_id: u32,
        clearance_m: f64,
    ) -> Option<Vec<f64>> {
        self.inner
            .test_active_against_cached(cached_model, cached_id, clearance_m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_crate_version() {
        assert_eq!(kernel_version(), env!("CARGO_PKG_VERSION"));
    }
}
