//! Turns evaluated elements into wire-format geometry batches. See `docs/05` section 3.

use std::collections::{BTreeMap, VecDeque};

use glam::{DVec3, Vec3};
use ifc_geometry::hash::KeyedMap;
use ifc_geometry::{Diagnostic, ElementGeometry, ModelEvaluator};

/// Furthest a vertex may lie from its batch origin, in metres. Mirrors `MAX_BATCH_RADIUS`.
pub const MAX_BATCH_RADIUS: f64 = 4096.0;
/// Coordinates beyond this are corrupt: they would not survive the trip to `f32`. Metres.
const MAX_COORDINATE: f64 = 1e9;
/// An element wider than the radius is split: its triangles are halved until no side is longer
/// than `SPLIT_SIDE`, then grouped by the cell of `SPLIT_CELL` holding each centroid. A corner
/// lies within 2/3 of a side of its centroid, so a piece spans at most 4096 + 4/3 x 512 m on
/// each axis, and no corner is more than 3843 m, plus 0.9 m of rounding, from its centre.
const SPLIT_SIDE: f64 = 512.0;
const SPLIT_CELL: f64 = 4096.0;
/// Most triangles splitting may make of one element; past that it stays whole, flagged.
const MAX_SPLIT_TRIANGLES: u64 = 1 << 20;

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

    /// Whether it fits a batch of its own. The box answers for nearly every element; rounding
    /// moves the origin by at most half a metre on each axis.
    fn fits_alone(&self) -> bool {
        let reach = (self.max - self.min).length() / 2.0 + 0.5 * 3f64.sqrt();
        reach <= MAX_BATCH_RADIUS || self.fits(self.origin())
    }

    /// Pieces of an element too wide for one batch, in a fixed order, each with the element's
    /// id and flags, or `None` when they would hold more than `MAX_SPLIT_TRIANGLES`.
    fn split(&self) -> Option<Vec<Prepared>> {
        // Halving every side `levels` times leaves 4^levels triangles: count before making any.
        let levels_of = |corners: &[DVec3; 3]| {
            let mut side = (0..3).map(|at| corners[at].distance(corners[(at + 1) % 3])).fold(0.0, f64::max);
            let mut levels = 0u32;
            while side > SPLIT_SIDE && levels <= 31 {
                side /= 2.0;
                levels += 1;
            }
            levels
        };
        let mut total = 0u64;
        for part in &self.parts {
            for triangle in part.indices.as_chunks::<3>().0 {
                let levels = levels_of(&triangle.map(|index| part.positions[index as usize]));
                if levels > 31 {
                    return None;
                }
                total = total.saturating_add(1 << (2 * levels));
            }
        }
        if total > MAX_SPLIT_TRIANGLES {
            return None;
        }

        // Per cell, one part for each part of the element, sharing corners of equal position and
        // normal. A BTreeMap keeps the pieces in the same order on every run.
        type Shared = KeyedMap<([u64; 3], [u32; 3]), u32>;
        let mut cells: BTreeMap<[i64; 3], Vec<(Part, Shared)>> = BTreeMap::new();
        for (index, part) in self.parts.iter().enumerate() {
            for triangle in part.indices.as_chunks::<3>().0 {
                let corners = triangle.map(|index| part.positions[index as usize]);
                // Every corner of a triangle carries its facing: see `prepare`.
                let normal = part.normals[triangle[0] as usize];
                // Depth first, so at most 3 triangles per level wait here, never all of them.
                let mut stack = vec![(corners, levels_of(&corners))];
                while let Some((piece, levels)) = stack.pop() {
                    if levels > 0 {
                        stack.extend(quarters(piece).map(|quarter| (quarter, levels - 1)));
                        continue;
                    }
                    let centroid = (piece[0] + piece[1] + piece[2]) / 3.0;
                    let cell = (centroid / SPLIT_CELL).floor().as_i64vec3().to_array();
                    let parts = cells.entry(cell).or_insert_with(|| {
                        let empty = |part: &Part| Part { positions: Vec::new(), normals: Vec::new(), indices: Vec::new(), rgba: part.rgba };
                        self.parts.iter().map(|part| (empty(part), Shared::default())).collect()
                    });
                    let (out, shared) = &mut parts[index];
                    for corner in piece {
                        let key = (corner.to_array().map(f64::to_bits), normal.to_array().map(f32::to_bits));
                        let next = out.positions.len() as u32;
                        let at = *shared.entry(key).or_insert(next);
                        if at == next {
                            out.positions.push(corner);
                            out.normals.push(normal);
                        }
                        out.indices.push(at);
                    }
                }
            }
        }

        let pieces = cells.into_values().map(|parts| {
            let parts: Vec<Part> = parts.into_iter().map(|(part, _)| part).filter(|part| !part.indices.is_empty()).collect();
            let corners = parts.iter().flat_map(|part| &part.positions);
            let min = corners.clone().fold(DVec3::INFINITY, |min, corner| min.min(*corner));
            let max = corners.fold(DVec3::NEG_INFINITY, |max, corner| max.max(*corner));
            Prepared { express_id: self.express_id, parts, flags: self.flags, min, max }
        });
        Some(pieces.collect())
    }
}

/// The four triangles between a triangle's corners and the middles of its sides, each facing
/// the same way as the whole.
fn quarters([a, b, c]: [DVec3; 3]) -> [[DVec3; 3]; 4] {
    let (ab, bc, ca) = ((a + b) / 2.0, (b + c) / 2.0, (c + a) / 2.0);
    [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
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
    /// An element that did not fit the last batch, or the pieces of a split one still to go.
    pending: VecDeque<Prepared>,
    mesh_count: u32,
    bounds: Option<(DVec3, DVec3)>,
    diagnostics: Vec<Diagnostic>,
}

impl GeometryStream {
    pub fn new(ids: Vec<u32>) -> Self {
        Self {
            ids,
            cursor: 0,
            pending: VecDeque::new(),
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
                if prepared.fits_alone() {
                    return Some(prepared);
                }
                // Too wide: its pieces go in turn. One that cannot split stays whole, and
                // `next_batch` flags it.
                let Some(pieces) = prepared.split() else { return Some(prepared) };
                self.pending.extend(pieces);
                if let Some(first) = self.pending.pop_front() {
                    return Some(first);
                }
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
            let element = match self.pending.pop_front() {
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
                        self.pending.push_front(element);
                        return batch;
                    }
                    open.push(&element);
                    self.record(&element);
                }
                None => {
                    let mut element = element;
                    let origin = element.origin();
                    let oversized = !element.fits_alone();
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
