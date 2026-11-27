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

struct CachedPart {
    mesh: ifc_geometry::Mesh,
    aabb: Option<[f64; 6]>,
}

struct CachedElement {
    hidden_by_default: bool,
    layer: String,
    hatching: Vec<spatial_tree::Hatching>,
    parts: Vec<CachedPart>,
}

const MAX_GEOM_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct Query {
    evaluator: ModelEvaluator<'static>,
    /// What the stream draws, ascending.
    drawn: Vec<u32>,
    /// Per-element AABB cache populated incrementally by `element_bounds` and `cull_elements_by_slab`.
    bounds_map: std::collections::HashMap<u32, Option<[f64; 6]>>,
    /// Lazily cached `(express_id, [minX, minY, minZ, maxX, maxY, maxZ])` for O(N) 3D slab culling.
    bounds_cache: Option<Vec<(u32, [f64; 6])>>,
    /// Bounded cache of evaluated element meshes + per-part AABBs for fast repeated slab queries.
    geom_cache: std::collections::HashMap<u32, std::rc::Rc<CachedElement>>,
    geom_cache_bytes: usize,
    /// Last active horizontal slab `(ux, uy, lo_bound, hi_bound)` from `cull_elements_by_slab`
    /// used by `slab_chunk_for_elements` to prune out-of-slab sub-meshes of multi-bar rebar groups.
    last_slab: Option<(f64, f64, f64, f64)>,
}

impl Query {
    fn new(evaluator: ModelEvaluator<'static>) -> Self {
        let drawn = evaluator.element_ids();
        Self {
            evaluator,
            drawn,
            bounds_map: std::collections::HashMap::new(),
            bounds_cache: None,
            geom_cache: std::collections::HashMap::new(),
            geom_cache_bytes: 0,
            last_slab: None,
        }
    }

    fn evaluate_cached(
        &mut self,
        express_id: u32,
        data: &ModelData<'static>,
    ) -> Option<std::rc::Rc<CachedElement>> {
        if let Some(cached) = self.geom_cache.get(&express_id) {
            return Some(std::rc::Rc::clone(cached));
        }
        let geom = self.evaluator.evaluate(express_id)?;
        let layer = data.layer_name(geom.express_id, &geom.type_name);
        let hatching = data.layer_hatching(geom.express_id);
        let mut union_min = DVec3::splat(f64::INFINITY);
        let mut union_max = DVec3::splat(f64::NEG_INFINITY);
        let mut any_bounds = false;
        let mut bytes = layer.len() + hatching.len() * 24 + 64;
        let mut parts = Vec::with_capacity(geom.meshes.len());
        for part in geom.meshes {
            let aabb = if !part.mesh.triangles.is_empty() {
                mesh_aabb(&part.mesh.positions)
            } else {
                None
            };
            if let Some(b) = aabb {
                union_min = union_min.min(DVec3::new(b[0], b[1], b[2]));
                union_max = union_max.max(DVec3::new(b[3], b[4], b[5]));
                any_bounds = true;
            }
            bytes += part.mesh.positions.len() * 24 + part.mesh.triangles.len() * 12 + 48;
            parts.push(CachedPart {
                mesh: part.mesh,
                aabb,
            });
        }
        let elem_bounds = any_bounds.then_some([
            union_min.x,
            union_min.y,
            union_min.z,
            union_max.x,
            union_max.y,
            union_max.z,
        ]);
        self.bounds_map.entry(express_id).or_insert(elem_bounds);
        let rc = std::rc::Rc::new(CachedElement {
            hidden_by_default: geom.hidden_by_default,
            layer,
            hatching,
            parts,
        });
        if bytes <= MAX_GEOM_CACHE_BYTES {
            if self.geom_cache_bytes + bytes > MAX_GEOM_CACHE_BYTES {
                self.geom_cache.clear();
                self.geom_cache_bytes = 0;
            }
            self.geom_cache_bytes += bytes;
            self.geom_cache.insert(express_id, std::rc::Rc::clone(&rc));
        }
        Some(rc)
    }
}

fn mesh_aabb(positions: &[DVec3]) -> Option<[f64; 6]> {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    let mut any = false;
    for &p in positions {
        if p.is_finite() {
            min = min.min(p);
            max = max.max(p);
            any = true;
        }
    }
    (any && min.x <= max.x && min.y <= max.y && min.z <= max.z)
        .then_some([min.x, min.y, min.z, max.x, max.y, max.z])
}

fn aabb_dist_sq_to_point(b: &[f64; 6], p: DVec3) -> f64 {
    let cx = p.x.clamp(b[0], b[3]);
    let cy = p.y.clamp(b[1], b[4]);
    let cz = p.z.clamp(b[2], b[5]);
    DVec3::new(cx, cy, cz).distance_squared(p)
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
            self.query = ModelEvaluator::new(self.file).ok().map(Query::new);
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
        let mut out = Vec::with_capacity(express_ids.len() * 7);
        let mut missing = Vec::new();
        for &id in express_ids {
            if query.drawn.binary_search(&id).is_err() {
                continue;
            }
            if !query.bounds_map.contains_key(&id) {
                missing.push(id);
            }
        }
        if !missing.is_empty() {
            let flat = element_bounds_flat(&mut query.evaluator, &query.drawn, &missing);
            for &id in &missing {
                query.bounds_map.entry(id).or_insert(None);
            }
            for chunk in flat.as_chunks::<7>().0 {
                query.bounds_map.insert(
                    chunk[0] as u32,
                    Some([chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6]]),
                );
            }
        }
        for &id in express_ids {
            if let Some(Some(b)) = query.bounds_map.get(&id) {
                out.push(id as f64);
                out.extend_from_slice(b);
            }
        }
        out
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
        if self.query.is_none() {
            self.query = ModelEvaluator::new(self.file).ok().map(Query::new);
        }
        let query = self.query.as_mut()?;
        query.drawn.binary_search(&express_id).ok()?;
        let cached = query.evaluate_cached(express_id, &self.data)?;
        let clash_pt = DVec3::new(px, py, pz);
        let mut verts: Vec<DVec3> = Vec::new();
        if cached.parts.len() > 1 && clash_pt.is_finite() {
            // When an element (e.g. an IFC4X3 IfcReinforcingBar group) contains multiple discrete
            // sub-meshes, measure the sub-mesh(es) actually involved in the clash at `clash_pt`
            // so longitudinal rebar arrays are not misclassified as wide transverse blocks and
            // transverse stirrup arrays use the clashing stirrup's thickness rather than the array span.
            let mut min_d2 = f64::INFINITY;
            for part in &cached.parts {
                if let Some(b) = &part.aabb {
                    min_d2 = min_d2.min(aabb_dist_sq_to_point(b, clash_pt));
                }
            }
            let threshold_d2 = (min_d2.sqrt() + 1e-3).powi(2);
            for part in &cached.parts {
                if let Some(b) = &part.aabb
                    && aabb_dist_sq_to_point(b, clash_pt) <= threshold_d2
                {
                    verts.extend_from_slice(&part.mesh.positions);
                }
            }
        } else {
            for part in &cached.parts {
                verts.extend_from_slice(&part.mesh.positions);
            }
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
            let mut missing = Vec::new();
            for &id in &query.drawn {
                if !query.bounds_map.contains_key(&id) {
                    missing.push(id);
                }
            }
            if !missing.is_empty() {
                let flat = element_bounds_flat(&mut query.evaluator, &query.drawn, &missing);
                for &id in &missing {
                    query.bounds_map.entry(id).or_insert(None);
                }
                for chunk in flat.as_chunks::<7>().0 {
                    query.bounds_map.insert(
                        chunk[0] as u32,
                        Some([chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6]]),
                    );
                }
            }
            let mut cache = Vec::with_capacity(query.drawn.len());
            for &id in &query.drawn {
                if let Some(Some(b)) = query.bounds_map.get(&id) {
                    cache.push((id, *b));
                }
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
        query.last_slab = Some((ux, uy, lo_bound, hi_bound));
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
            self.query = ModelEvaluator::new(self.file).ok().map(Query::new);
        }
        let Some(query) = self.query.as_mut() else {
            return out;
        };
        let active_slab = query.last_slab;
        for &id in express_ids {
            if query.drawn.binary_search(&id).is_err() {
                continue;
            }
            let Some(cached) = query.evaluate_cached(id, data) else {
                continue;
            };
            if cached.hidden_by_default {
                continue;
            }
            let multi_part = cached.parts.len() > 1;
            for (part, cached_part) in cached.parts.iter().enumerate() {
                // When an element has multiple discrete sub-meshes (e.g. a rebar group of 30+ bars)
                // and `cull_elements_by_slab` set `active_slab`, skip sub-meshes strictly outside the slab.
                if multi_part
                    && let Some((ux, uy, lo_bound, hi_bound)) = active_slab
                    && let Some(b) = &cached_part.aabb
                {
                    let s_lo = if ux >= 0.0 { ux * b[0] } else { ux * b[3] }
                        + if uy >= 0.0 { uy * b[1] } else { uy * b[4] };
                    let s_hi = if ux >= 0.0 { ux * b[3] } else { ux * b[0] }
                        + if uy >= 0.0 { uy * b[4] } else { uy * b[1] };
                    if s_hi < lo_bound || s_lo > hi_bound {
                        continue;
                    }
                }
                if let Some(xf) = &transform {
                    let mut mesh = cached_part.mesh.clone();
                    for p in &mut mesh.positions {
                        *p = xf.transform_point3(*p);
                    }
                    write_element(&mut out, id, part as u32, &cached.layer, &cached.hatching, &mesh);
                } else {
                    write_element(
                        &mut out,
                        id,
                        part as u32,
                        &cached.layer,
                        &cached.hatching,
                        &cached_part.mesh,
                    );
                }
            }
        }
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
