//! An open model: its bytes, its index, and everything that borrows them.

use crate::batches::{Batch, GeometryStream};
use crate::clash::{RawElementMesh, element_bounds_flat, raw_element_mesh};
use crate::drawing::{DrawingRequest, DrawingStream, write_element};
use crate::encode::{complete_json, diagnostics_json, snapshots_json};
use crate::json::to_json;
use crate::shape::element_shape;
pub use crate::shape::ElementShape;
use glam::{DMat4, DVec3};
use ifc_geometry::ModelEvaluator;
use spatial_tree::{ElementQuery, ModelData, SearchField};
use step_parser::{ParseError, StepIndex, index};

/// Owns the allocations that `LoadedModel`'s other fields borrow.
struct Allocation {
    bytes: *mut [u8],
    file: *mut StepIndex<'static>,
}

impl Allocation {
    fn new(bytes: Vec<u8>) -> Result<(Self, &'static StepIndex<'static>), ParseError> {
        let bytes: *mut [u8] = Box::into_raw(bytes.into_boxed_slice());
        // SAFETY: the slice stays allocated and unmoved until `drop` below. `LoadedModel`
        // declares `allocation` last, so every borrower has gone by then, and no `'static`
        // reference leaves this module.
        let view: &'static [u8] = unsafe { &*bytes };
        let parsed = index(view);
        match parsed {
            Ok(parsed) => {
                let file = Box::into_raw(Box::new(parsed));
                // SAFETY: as above.
                Ok((Self { bytes, file }, unsafe { &*file }))
            }
            Err(error) => {
                // SAFETY: `bytes` came from `Box::into_raw`, and the failed parse kept no borrow.
                drop(unsafe { Box::from_raw(bytes) });
                Err(error)
            }
        }
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: both came from `Box::into_raw` in `new`. The index borrows the bytes, so
        // it goes first.
        unsafe {
            drop(Box::from_raw(self.file));
            drop(Box::from_raw(self.bytes));
        }
    }
}

#[derive(Debug)]
pub enum OpenError {
    Parse(ParseError),
    UnsupportedSchema(Vec<String>),
}

struct ActiveStream {
    evaluator: ModelEvaluator<'static>,
    stream: GeometryStream,
}

struct Query {
    evaluator: ModelEvaluator<'static>,
    /// What the stream draws, ascending.
    drawn: Vec<u32>,
    /// Lazily cached `(express_id, [minX, minY, minZ, maxX, maxY, maxZ])` for O(N) 3D slab culling.
    bounds_cache: Option<Vec<(u32, [f64; 6])>>,
}

/// Storeys get the height of their placement, as their geometry is placed.
fn fill_levels(node: &mut spatial_tree::SpatialNode, evaluator: &mut ModelEvaluator<'static>) {
    if node.type_name.eq_ignore_ascii_case("IfcBuildingStorey") {
        node.level = evaluator.placement_origin(node.express_id).map(|origin| origin.z);
    }
    for child in &mut node.children {
        fill_levels(child, evaluator);
    }
}

pub struct LoadedModel {
    stream: Option<ActiveStream>,
    drawing: Option<DrawingStream>,
    query: Option<Query>,
    data: ModelData<'static>,
    file: &'static StepIndex<'static>,
    _allocation: Allocation,
}

impl LoadedModel {
    pub fn open(bytes: Vec<u8>) -> Result<Self, OpenError> {
        let (allocation, file) = Allocation::new(bytes).map_err(OpenError::Parse)?;
        let Some(data) = ModelData::open(file) else {
            let identifiers = file.schema_identifiers().iter().map(ToString::to_string).collect();
            return Err(OpenError::UnsupportedSchema(identifiers));
        };
        Ok(Self { stream: None, drawing: None, query: None, data, file, _allocation: allocation })
    }

    pub fn summary_json(&self) -> String {
        to_json(&self.data.summary())
    }

    /// Raw original IFC STEP bytes owned by this model allocation.
    pub fn raw_bytes(&self) -> &[u8] {
        // SAFETY: `_allocation.bytes` remains valid and immutable for the lifetime of `LoadedModel`.
        unsafe { &*self._allocation.bytes }
    }

    /// Project length unit scale in metres per file unit (e.g. `1.0` for m, `0.001` for mm).
    pub fn length_unit_scale(&self) -> f64 {
        let s = self.data.summary().length_unit_scale;
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    }

    /// Parses an edited clash DXF and surgically patches this model's STEP bytes (F5, Level 1 + Level 2).
    /// `scope` decides which of the DXF's edits belong to this model.
    pub fn apply_clash_dxf(
        &self,
        dxf_bytes: &[u8],
        scope: &crate::clash_dxf::ApplyScope,
    ) -> Result<(Vec<u8>, String), String> {
        crate::clash_dxf::apply_clash_dxf_to_step_indexed(
            self.raw_bytes(),
            Some(self.file),
            self.length_unit_scale(),
            dxf_bytes,
            scope,
        )
    }

    pub fn spatial_tree_json(&mut self) -> Option<String> {
        let mut tree = self.data.spatial_tree()?;
        if let Some(query) = self.query_evaluator() {
            fill_levels(&mut tree.root, &mut query.evaluator);
        }
        Some(to_json(&tree))
    }

    /// The evaluator for one-off questions, made on first use.
    fn query_evaluator(&mut self) -> Option<&mut Query> {
        if self.query.is_none() {
            self.query = ModelEvaluator::new(self.file).ok().map(|evaluator| {
                let drawn = evaluator.element_ids();
                Query { evaluator, drawn, bounds_cache: None }
            });
        }
        self.query.as_mut()
    }

    pub fn element_details_json(&self, express_id: u32) -> Option<String> {
        self.data.element_details(express_id).map(|details| to_json(&details))
    }

    /// One snapshot per product the geometry stream draws, ascending: the input of `diffModels`.
    pub fn element_snapshots_json(&self) -> String {
        let Ok(evaluator) = ModelEvaluator::new(self.file) else {
            return "[]".to_string();
        };
        let details: Vec<_> = evaluator
            .element_ids()
            .into_iter()
            .filter_map(|id| self.data.element_details(id))
            .collect();
        snapshots_json(&details)
    }

    /// `field` is an `ElementQuery['field']` of the contract; `None` for any other.
    pub fn find_elements(
        &self,
        field: &str,
        text: &str,
        property_name: Option<&str>,
    ) -> Option<Vec<u32>> {
        let field = match field {
            "any" => SearchField::Any,
            "name" => SearchField::Name,
            "class" => SearchField::Class,
            "globalId" => SearchField::GlobalId,
            "property" => SearchField::Property,
            _ => return None,
        };
        Some(self.data.find_elements(&ElementQuery {
            field,
            text: text.to_string(),
            property_name: property_name.map(str::to_string),
        }))
    }

    /// `None` for an id the stream does not draw. Its own evaluator, kept for the next query.
    pub fn element_mesh(&mut self, express_id: u32) -> Option<ElementShape> {
        let query = self.query_evaluator()?;
        query.drawn.binary_search(&express_id).ok()?;
        element_shape(&query.evaluator.evaluate(express_id)?)
    }

    /// Compact `f64` AABBs `[id, minX, minY, minZ, maxX, maxY, maxZ]` for clash broad phase.
    pub fn element_bounds(&mut self, express_ids: &[u32]) -> Vec<f64> {
        let Some(query) = self.query_evaluator() else {
            return Vec::new();
        };
        element_bounds_flat(&mut query.evaluator, &query.drawn, express_ids)
    }

    /// Raw `f64` positions and `u32` triangle indices for clash narrow phase (no feature edges).
    pub fn raw_element_mesh(&mut self, express_id: u32) -> Option<RawElementMesh> {
        let query = self.query_evaluator()?;
        query.drawn.binary_search(&express_id).ok()?;
        raw_element_mesh(&query.evaluator.evaluate(express_id)?)
    }

    /// Computes Element A's exact slab span `[orientation, s_min, s_max, s_clash, delta_s_a,
    /// width_perp_m, height_perp_m]` inside the Worker using `clash_detector::compute_element_a_slab`
    /// without transferring vertex buffers to the main thread (F2/F4).
    #[allow(clippy::too_many_arguments)]
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
    ) -> Option<[f64; 7]> {
        let query = self.query_evaluator()?;
        query.drawn.binary_search(&express_id).ok()?;
        let geom = query.evaluator.evaluate(express_id)?;
        let mut verts: Vec<DVec3> = Vec::new();
        for part in geom.meshes {
            verts.extend(part.mesh.positions);
        }
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
            max_transverse_slab_m: if max_transverse_slab_m.is_finite() && max_transverse_slab_m > 0.0
            {
                max_transverse_slab_m
            } else {
                clash_detector::DEFAULT_MAX_TRANSVERSE_SLAB_M
            },
            ..clash_detector::SlabConfig::default()
        };
        let span = clash_detector::compute_element_a_slab(
            &verts,
            [tx, ty],
            DVec3::new(px, py, pz),
            &cfg,
        );
        let orient = match span.orientation {
            clash_detector::ElementOrientation::Transverse => 0.0,
            clash_detector::ElementOrientation::Longitudinal => 1.0,
        };
        Some([
            orient,
            span.s_min,
            span.s_max,
            span.s_clash,
            span.delta_s_a,
            span.width_perp_m,
            span.height_perp_m,
        ])
    }

    /// 3D slab-vs-AABB culling along horizontal tangent `(tx, ty, 0)` (F3):
    /// projects each cached element AABB onto `t_hat` in 4 float ops and returns only the
    /// `expressId`s overlapping `[s_min - eps, s_max + eps]` (>95% culled on long bridges).
    pub fn cull_elements_by_slab(
        &mut self,
        tx: f64,
        ty: f64,
        s_min: f64,
        s_max: f64,
        eps: f64,
    ) -> Vec<u32> {
        let len = tx.hypot(ty);
        if !len.is_finite() || len <= 1e-12 || !s_min.is_finite() || !s_max.is_finite() {
            return Vec::new();
        }
        let ux = tx / len;
        let uy = ty / len;
        let pad = if eps.is_finite() && eps > 0.0 { eps } else { 0.0 };
        let lo_bound = s_min.min(s_max) - pad;
        let hi_bound = s_min.max(s_max) + pad;

        let Some(query) = self.query_evaluator() else {
            return Vec::new();
        };
        if query.bounds_cache.is_none() {
            let flat = element_bounds_flat(&mut query.evaluator, &query.drawn, &query.drawn);
            let mut cache = Vec::with_capacity(flat.len() / 7);
            for chunk in flat.as_chunks::<7>().0 {
                cache.push((
                    chunk[0] as u32,
                    [chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6]],
                ));
            }
            query.bounds_cache = Some(cache);
        }

        let cache = query.bounds_cache.as_deref().unwrap_or(&[]);
        let mut kept = Vec::new();
        for &(id, b) in cache {
            let s_lo = if ux >= 0.0 { ux * b[0] } else { ux * b[3] }
                + if uy >= 0.0 { uy * b[1] } else { uy * b[4] };
            let s_hi = if ux >= 0.0 { ux * b[3] } else { ux * b[0] }
                + if uy >= 0.0 { uy * b[4] } else { uy * b[1] };
            if s_hi >= lo_bound && s_lo <= hi_bound {
                kept.push(id);
            }
        }
        kept
    }

    /// Evaluates ONLY the given culled `express_ids` and serializes them into a drawing chunk
    /// for unoccluded slab slicing (F3).
    pub fn slab_chunk_for_elements(
        &mut self,
        express_ids: &[u32],
        transform: Option<DMat4>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let data = &self.data;
        if self.query.is_none() {
            self.query = ModelEvaluator::new(self.file).ok().map(|evaluator| {
                let drawn = evaluator.element_ids();
                Query { evaluator, drawn, bounds_cache: None }
            });
        }
        let Some(query) = self.query.as_mut() else {
            return out;
        };
        for &id in express_ids {
            if query.drawn.binary_search(&id).is_err() {
                continue;
            }
            let Some(geometry) = query.evaluator.evaluate(id) else {
                continue;
            };
            if geometry.hidden_by_default {
                continue;
            }
            let layer = data.layer_name(geometry.express_id, &geometry.type_name);
            let hatching = data.layer_hatching(geometry.express_id);
            for (part, element_mesh) in geometry.meshes.into_iter().enumerate() {
                let mut mesh = element_mesh.mesh;
                if let Some(xf) = &transform {
                    for p in &mut mesh.positions {
                        *p = xf.transform_point3(*p);
                    }
                }
                write_element(&mut out, geometry.express_id, part as u32, &layer, &hatching, &mesh);
            }
        }
        let _ = DVec3::ZERO; // keep glam::DVec3 available if extended
        out
    }

    /// Starts from the first element. Evaluator caches last only as long as the stream.
    pub fn start_stream(&mut self) {
        self.stream = ModelEvaluator::new(self.file).ok().map(|evaluator| {
            let stream = GeometryStream::new(evaluator.element_ids()).with_instances(evaluator.instance_uses());
            ActiveStream { evaluator, stream }
        });
    }

    pub fn next_batch(&mut self, max_triangles: usize) -> Option<Batch> {
        let active = self.stream.as_mut()?;
        active.stream.next_batch(&mut active.evaluator, max_triangles)
    }

    pub fn processed_elements(&self) -> usize {
        self.stream.as_ref().map_or(0, |active| active.stream.processed())
    }

    pub fn total_elements(&self) -> usize {
        self.stream.as_ref().map_or(0, |active| active.stream.total())
    }

    /// Its own evaluator, so a drawing can interleave with a geometry stream.
    pub fn start_drawing(&mut self, request: DrawingRequest) {
        self.drawing = ModelEvaluator::new(self.file)
            .ok()
            .map(|evaluator| DrawingStream::new(evaluator, request));
    }

    pub fn next_drawing_chunk(&mut self, max_triangles: usize) -> Option<Vec<u8>> {
        self.drawing.as_mut()?.next_chunk(&self.data, max_triangles)
    }

    /// Ends the drawing, frees its caches, and returns its diagnostics as JSON.
    pub fn finish_drawing(&mut self) -> String {
        let drawing = self.drawing.take();
        let diagnostics: Vec<_> = drawing
            .iter()
            .flat_map(|drawing| drawing.evaluator().diagnostics())
            .collect();
        diagnostics_json(&diagnostics)
    }

    /// Ends the stream, frees its caches, and returns the closing event.
    pub fn finish_stream(&mut self) -> String {
        let Some(active) = self.stream.take() else {
            return complete_json(0, None, &[]);
        };
        let diagnostics: Vec<_> = active
            .evaluator
            .diagnostics()
            .iter()
            .chain(active.stream.diagnostics())
            .collect();
        complete_json(active.stream.mesh_count(), active.stream.bounds(), &diagnostics)
    }
}
