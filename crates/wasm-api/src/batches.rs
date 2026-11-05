//! Turns evaluated elements into wire-format geometry batches. See `docs/05` section 3.

use glam::{DVec3, Vec3};
use ifc_geometry::hash::KeyedMap;
use ifc_geometry::{Diagnostic, ElementGeometry, ModelEvaluator};

/// Furthest a vertex may lie from its batch origin, in metres. Mirrors `MAX_BATCH_RADIUS`.
pub const MAX_BATCH_RADIUS: f64 = 4096.0;
/// Coordinates beyond this are corrupt: they would not survive the trip to `f32`. Metres.
const MAX_COORDINATE: f64 = 1e9;

const FLAG_TRANSPARENT: u32 = 1 << 0;
const FLAG_DEGRADED: u32 = 1 << 1;
const FLAG_HIDDEN_BY_DEFAULT: u32 = 1 << 2;

/// One block of triangles, laid out as `GeometryBatch` in `GeometryBatch.ts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Batch {
    pub origin: [f64; 3],
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
    pub mesh_table: Vec<u32>,
}

impl Batch {
    fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn push(&mut self, element: &Prepared) {
        let origin = DVec3::from_array(self.origin);
        for part in &element.parts {
            let first_vertex = (self.positions.len() / 3) as u32;
            let first_index = self.indices.len() as u32;
            for position in &part.positions {
                let local = (*position - origin).as_vec3();
                self.positions.extend_from_slice(&local.to_array());
            }
            for normal in &part.normals {
                self.normals.extend_from_slice(&normal.to_array());
            }
            self.indices.extend(part.indices.iter().map(|index| first_vertex + index));

            let mut flags = element.flags;
            if part.rgba & 0xFF != 0xFF {
                flags |= FLAG_TRANSPARENT;
            }
            self.mesh_table.extend_from_slice(&[
                element.express_id,
                first_vertex,
                part.positions.len() as u32,
                first_index,
                part.indices.len() as u32,
                part.rgba,
                flags,
            ]);
        }
    }
}

/// One mesh of an element, with its sharp edges split so each vertex has one normal.
struct Part {
    positions: Vec<DVec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    rgba: u32,
}

struct Prepared {
    express_id: u32,
    parts: Vec<Part>,
    flags: u32,
    min: DVec3,
    max: DVec3,
}

impl Prepared {
    fn triangle_count(&self) -> usize {
        self.parts.iter().map(|part| part.indices.len() / 3).sum()
    }

    /// The centre of the box, rounded to the metre, half away from zero.
    fn origin(&self) -> [f64; 3] {
        ((self.min + self.max) / 2.0).round().to_array()
    }

    fn fits(&self, origin: [f64; 3]) -> bool {
        let origin = DVec3::from_array(origin);
        let limit = MAX_BATCH_RADIUS * MAX_BATCH_RADIUS;
        self.parts
            .iter()
            .flat_map(|part| &part.positions)
            .all(|position| position.distance_squared(origin) <= limit)
    }
}

/// `None` when a corner is corrupt or the triangle has no area, so it has no normal.
pub(crate) fn usable_normal(corners: [DVec3; 3]) -> Option<Vec3> {
    if !corners.iter().all(|corner| corner.abs().max_element() <= MAX_COORDINATE) {
        return None;
    }
    let cross = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
    let length = cross.length();
    (length > 0.0 && length.is_finite()).then(|| (cross / length).as_vec3())
}

/// Vertices are shared between triangles of the same facing. Curved surfaces stay faceted.
fn prepare(geometry: ElementGeometry) -> (Option<Prepared>, usize) {
    let mut parts = Vec::new();
    let mut dropped = 0;
    let (mut min, mut max) = (DVec3::INFINITY, DVec3::NEG_INFINITY);

    for element_mesh in &geometry.meshes {
        let mesh = &element_mesh.mesh;
        let mut part = Part {
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
            rgba: element_mesh.rgba,
        };
        // A mesh normally uses each of its positions at least once, so reserving that many
        // saves the map from growing step by step.
        let mut shared: KeyedMap<(u32, u64), u32> =
            KeyedMap::with_capacity_and_hasher(mesh.positions.len(), Default::default());
        for triangle in &mesh.triangles {
            let corners = triangle.map(|id| mesh.positions[id as usize]);
            let Some(normal) = usable_normal(corners) else {
                dropped += 1;
                continue;
            };
            let facing = (normal * 32767.0).round().as_i16vec3().to_array();
            // One word instead of three to hash.
            let packed = facing.iter().fold(0, |packed, part| packed << 16 | u64::from(*part as u16));
            for (id, corner) in triangle.iter().zip(corners) {
                let next = part.positions.len() as u32;
                let index = *shared.entry((*id, packed)).or_insert(next);
                if index == next {
                    part.positions.push(corner);
                    part.normals.push(normal);
                    min = min.min(corner);
                    max = max.max(corner);
                }
                part.indices.push(index);
            }
        }
        if !part.indices.is_empty() {
            parts.push(part);
        }
    }

    if parts.is_empty() {
        return (None, dropped);
    }
    let mut flags = 0;
    if geometry.degraded {
        flags |= FLAG_DEGRADED;
    }
    if geometry.hidden_by_default {
        flags |= FLAG_HIDDEN_BY_DEFAULT;
    }
    let prepared = Prepared { express_id: geometry.express_id, parts, flags, min, max };
    (Some(prepared), dropped)
}

/// Pulls batches out of an evaluator, element by element, so the Worker stays responsive.
pub struct GeometryStream {
    ids: Vec<u32>,
    cursor: usize,
    /// An element that did not fit the last batch.
    pending: Option<Prepared>,
    mesh_count: u32,
    bounds: Option<(DVec3, DVec3)>,
    diagnostics: Vec<Diagnostic>,
}

impl GeometryStream {
    pub fn new(ids: Vec<u32>) -> Self {
        Self {
            ids,
            cursor: 0,
            pending: None,
            mesh_count: 0,
            bounds: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn processed(&self) -> usize {
        self.cursor
    }

    pub fn total(&self) -> usize {
        self.ids.len()
    }

    pub fn mesh_count(&self) -> u32 {
        self.mesh_count
    }

    /// Absolute model coordinates of everything visible by default.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        self.bounds.map(|(min, max)| (min.to_array(), max.to_array()))
    }

    /// Problems found here. The evaluator keeps its own.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn warn(&mut self, express_id: u32, code: &'static str, message: String) {
        self.diagnostics.push(Diagnostic { express_id: Some(express_id), code, message });
    }

    fn advance(&mut self, evaluator: &mut ModelEvaluator<'_>) -> Option<Prepared> {
        while self.cursor < self.ids.len() {
            let id = self.ids[self.cursor];
            self.cursor += 1;
            // Expressid 0 means "no element" on the wire.
            if id == 0 {
                self.warn(id, "invalid-express-id", "element #0 skipped".into());
                continue;
            }
            let Some(geometry) = evaluator.evaluate(id) else { continue };
            let (prepared, dropped) = prepare(geometry);
            if dropped > 0 {
                let message = format!("{dropped} degenerate or corrupt triangles dropped");
                self.warn(id, "invalid-triangles", message);
            }
            if let Some(mut prepared) = prepared {
                if dropped > 0 {
                    prepared.flags |= FLAG_DEGRADED;
                }
                return Some(prepared);
            }
        }
        None
    }

    fn record(&mut self, element: &Prepared) {
        self.mesh_count += element.parts.len() as u32;
        if element.flags & FLAG_HIDDEN_BY_DEFAULT == 0 {
            let (min, max) = self.bounds.unwrap_or((element.min, element.max));
            self.bounds = Some((min.min(element.min), max.max(element.max)));
        }
    }

    /// The next batch, or `None` when every element has been processed.
    pub fn next_batch(
        &mut self,
        evaluator: &mut ModelEvaluator<'_>,
        max_triangles: usize,
    ) -> Option<Batch> {
        let mut batch: Option<Batch> = None;
        loop {
            let element = match self.pending.take() {
                Some(element) => element,
                None => match self.advance(evaluator) {
                    Some(element) => element,
                    None => return batch,
                },
            };
            match &mut batch {
                Some(open) => {
                    let room = open.triangle_count() + element.triangle_count() <= max_triangles;
                    if !room || !element.fits(open.origin) {
                        self.pending = Some(element);
                        return batch;
                    }
                    open.push(&element);
                    self.record(&element);
                }
                None => {
                    let mut element = element;
                    let origin = element.origin();
                    let oversized = !element.fits(origin);
                    if oversized {
                        element.flags |= FLAG_DEGRADED;
                        let message = format!("element exceeds {MAX_BATCH_RADIUS} m from its origin");
                        self.warn(element.express_id, "batch-radius-exceeded", message);
                    }
                    let mut open = Batch { origin, ..Batch::default() };
                    open.push(&element);
                    self.record(&element);
                    if oversized {
                        return Some(open);
                    }
                    batch = Some(open);
                }
            }
        }
    }
}
