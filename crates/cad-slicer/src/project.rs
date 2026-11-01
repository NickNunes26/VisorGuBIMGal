//! Projection and hidden lines.
//!
//! Lines come from the mesh's own features, not from every triangle edge: an edge on the
//! boundary of an open mesh, an edge where two faces meet at a crease, or an edge where a
//! curved surface turns away from the viewer. That is what a drawing shows, and it keeps the
//! output small.
//!
//! Visibility is decided by sampling each projected edge against the projected triangles,
//! through a uniform grid. A sample is hidden when a triangle nearer the viewer covers it.
//! The resolution is fixed, so the same input always gives the same runs.

use crate::cut::Frame;
use crate::geom::{Segment, SEGMENT_EPSILON};
use glam::{DVec2, DVec3};
use ifc_geometry::Mesh;
use std::collections::BTreeMap;

/// Distance between visibility samples. Metres.
const SAMPLE_STEP: f64 = 0.02;
/// Samples one edge may take, however long it is.
const MAX_STEPS_PER_EDGE: usize = 512;
/// Samples one drawing may take before the projection stops and says so.
const MAX_SAMPLES: usize = 400_000;
/// Triangles one drawing may occlude with.
const MAX_OCCLUDERS: usize = 400_000;
/// Triangles one element may offer before its projection is skipped.
const MAX_ELEMENT_TRIANGLES: usize = 150_000;
/// Feature edges one element may contribute.
const MAX_FEATURE_EDGES: usize = 100_000;
/// Occlusion tests one drawing may run.
const MAX_OCCLUSION_TESTS: usize = 40_000_000;
/// Cosines above this are the same plane: a tessellated curve, not a crease.
const CREASE_COSINE: f64 = 0.94;
/// A sample this far behind a surface is not hidden by the surface itself. Metres.
const OCCLUSION_EPSILON: f64 = 1e-6;
/// Cells one triangle may occupy; wider ones go to a list every sample checks.
const MAX_CELLS_PER_TRIANGLE: usize = 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectReport {
    pub non_finite: usize,
    pub too_large: bool,
    pub truncated: bool,
}

/// Samples the whole drawing may still take, shared between its elements.
pub(crate) struct Budget {
    samples: usize,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            samples: MAX_SAMPLES,
        }
    }
}

/// A projected triangle: two sheet coordinates and the depth at each corner.
#[derive(Debug, Clone, Copy)]
struct Flat {
    points: [DVec2; 3],
    depths: [f64; 3],
    area: f64,
}

/// The projected triangles of every mesh, in a grid so a sample only tests what is near it.
pub(crate) struct Occluders {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
    wide: Vec<usize>,
    triangles: Vec<Flat>,
    tests_left: std::cell::Cell<usize>,
}

impl Occluders {
    pub(crate) fn new(reach: f64) -> Self {
        Self {
            cell: (reach.max(0.0) / 64.0).max(1e-3),
            cells: BTreeMap::new(),
            wide: Vec::new(),
            triangles: Vec::new(),
            tests_left: std::cell::Cell::new(MAX_OCCLUSION_TESTS),
        }
    }

    /// Adds a triangle, already projected.
    fn add(&mut self, points: [DVec2; 3], depths: [f64; 3]) -> bool {
        if self.triangles.len() >= MAX_OCCLUDERS {
            return false;
        }
        let area = (points[1] - points[0]).perp_dot(points[2] - points[0]);
        if !area.is_finite() || area.abs() <= 1e-12 {
            // Seen edge on: it covers no area and hides nothing.
            return true;
        }
        let min = DVec2::new(
            points[0].x.min(points[1].x).min(points[2].x),
            points[0].y.min(points[1].y).min(points[2].y),
        );
        let max = DVec2::new(
            points[0].x.max(points[1].x).max(points[2].x),
            points[0].y.max(points[1].y).max(points[2].y),
        );
        let id = self.triangles.len();
        self.triangles.push(Flat {
            points,
            depths,
            area,
        });
        let mut cells = Vec::new();
        let low = (
            (min.x / self.cell).floor() as i64,
            (min.y / self.cell).floor() as i64,
        );
        let high = (
            (max.x / self.cell).floor() as i64,
            (max.y / self.cell).floor() as i64,
        );
        for x in low.0..=high.0 {
            for y in low.1..=high.1 {
                if cells.len() >= MAX_CELLS_PER_TRIANGLE {
                    break;
                }
                cells.push((x, y));
            }
        }
        if (high.0 - low.0 + 1) * (high.1 - low.1 + 1) > MAX_CELLS_PER_TRIANGLE as i64 {
            self.wide.push(id);
        } else {
            for key in cells {
                self.cells.entry(key).or_default().push(id);
            }
        }
        true
    }

    /// Whether something nearer the viewer covers `point`, which sits at `depth`.
    fn hidden(&self, point: DVec2, depth: f64) -> bool {
        let key = (
            (point.x / self.cell).floor() as i64,
            (point.y / self.cell).floor() as i64,
        );
        let mut hidden = false;
        if let Some(ids) = self.cells.get(&key) {
            for id in ids {
                if self.covers(*id, point, depth) {
                    hidden = true;
                    break;
                }
            }
        }
        if !hidden {
            for id in &self.wide {
                if self.covers(*id, point, depth) {
                    hidden = true;
                    break;
                }
            }
        }
        hidden
    }

    fn covers(&self, id: usize, point: DVec2, depth: f64) -> bool {
        let left = self.tests_left.get();
        if left == 0 {
            return false;
        }
        self.tests_left.set(left - 1);
        let flat = &self.triangles[id];
        // Barycentric weights, tolerant of a sample exactly on an edge.
        let [a, b, c] = flat.points;
        let wa = (b - point).perp_dot(c - point) / flat.area;
        let wb = (c - point).perp_dot(a - point) / flat.area;
        let wc = (a - point).perp_dot(b - point) / flat.area;
        let slack = 1e-9 / flat.area.abs().max(1e-12);
        if wa < -slack || wb < -slack || wc < -slack {
            return false;
        }
        let at = wa * flat.depths[0] + wb * flat.depths[1] + wc * flat.depths[2];
        at.is_finite() && at < depth - OCCLUSION_EPSILON
    }
}

/// Projects one mesh's triangles into the occluder grid.
pub(crate) fn add_occluders(
    mesh: &Mesh,
    frame: &Frame,
    limit: Option<f64>,
    occluders: &mut Occluders,
) -> ProjectReport {
    let mut report = ProjectReport::default();
    if mesh.triangles.len() > MAX_ELEMENT_TRIANGLES {
        report.too_large = true;
        return report;
    }
    // A triangle entirely beyond the reach of the projection draws nothing and hides nothing.
    let ceiling = limit.unwrap_or(f64::INFINITY);
    for [a, b, c] in mesh.triangle_corners() {
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            report.non_finite += 1;
            continue;
        }
        let depths = [frame.depth(a), frame.depth(b), frame.depth(c)];
        if depths.iter().any(|depth| !depth.is_finite() || *depth > ceiling) {
            continue;
        }
        if depths[0].max(depths[1]).max(depths[2]) < 0.0 {
            continue;
        }
        if !occluders.add([frame.project(a), frame.project(b), frame.project(c)], depths) {
            report.truncated = true;
            break;
        }
    }
    report
}

/// One undirected edge of the welded mesh and the faces that meet along it.
#[derive(Default)]
struct EdgeFaces {
    count: u32,
    normals: [DVec3; 2],
}

/// Projects one mesh's feature edges, splitting them into visible and hidden runs.
pub(crate) fn project_mesh(
    mesh: &Mesh,
    frame: &Frame,
    limit: Option<f64>,
    occluders: &mut Occluders,
    budget: &mut Budget,
    visible: &mut Vec<Segment>,
    hidden: &mut Vec<Segment>,
) -> ProjectReport {
    let mut report = ProjectReport::default();
    if mesh.triangles.len() > MAX_ELEMENT_TRIANGLES {
        report.too_large = true;
        return report;
    }
    let (edges, skipped) = feature_edges(mesh, frame.look);
    report.non_finite += skipped;

    for (from, to) in edges {
        // Depth runs linearly along the edge: clip by solving once.
        let (depth_from, depth_to) = (frame.depth(from), frame.depth(to));
        if !depth_from.is_finite() || !depth_to.is_finite() {
            report.non_finite += 1;
            continue;
        }
        let run = depth_to - depth_from;
        let (mut start, mut end) = (0.0_f64, 1.0_f64);
        if run.abs() <= f64::MIN_POSITIVE {
            if depth_from < 0.0 || limit.is_some_and(|limit| depth_from > limit) {
                continue;
            }
        } else {
            // Between the eye and the plane there is no line.
            let zero = -depth_from / run;
            if run > 0.0 {
                start = start.max(zero);
            } else {
                end = end.min(zero);
            }
            if let Some(limit) = limit {
                let reach = (limit - depth_from) / run;
                if run > 0.0 {
                    end = end.min(reach);
                } else {
                    start = start.max(reach);
                }
            }
        }
        if end - start <= 1e-9 {
            continue;
        }
        let a = frame.project(from.lerp(to, start));
        let b = frame.project(from.lerp(to, end));
        let depth_a = frame.depth(from.lerp(to, start));
        let depth_b = frame.depth(from.lerp(to, end));
        if !(a.is_finite() && b.is_finite() && depth_a.is_finite() && depth_b.is_finite()) {
            report.non_finite += 1;
            continue;
        }
        let length = a.distance(b);
        if length <= SEGMENT_EPSILON {
            continue;
        }

        let wanted = ((length / SAMPLE_STEP).ceil() as usize).clamp(1, MAX_STEPS_PER_EDGE);
        if budget.samples < 2 {
            report.truncated = true;
            break;
        }
        let steps = wanted.min(budget.samples - 1);
        budget.samples -= steps + 1;

        let mut state = !occluders.hidden(a, depth_a);
        let mut run_start = 0.0;
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let point = a.lerp(b, t);
            let depth = depth_a + (depth_b - depth_a) * t;
            let now = !occluders.hidden(point, depth);
            if now != state {
                let boundary = (step as f64 - 0.5) / steps as f64;
                push_run(a, b, run_start, boundary, state, visible, hidden);
                run_start = boundary;
                state = now;
            }
        }
        push_run(a, b, run_start, 1.0, state, visible, hidden);
        if budget.samples == 0 {
            report.truncated = true;
            break;
        }
    }
    report
}

fn push_run(
    a: DVec2,
    b: DVec2,
    start: f64,
    end: f64,
    visible: bool,
    visible_out: &mut Vec<Segment>,
    hidden_out: &mut Vec<Segment>,
) {
    if end - start <= 1e-9 {
        return;
    }
    let from = a.lerp(b, start);
    let to = a.lerp(b, end);
    if from.distance(to) <= SEGMENT_EPSILON {
        return;
    }
    let segment = Segment::new(from, to);
    if visible {
        visible_out.push(segment);
    } else {
        hidden_out.push(segment);
    }
}

/// Joins collinear segments that overlap, so a box's top and bottom rims do not draw the same
/// line twice. A visible run wins over a hidden one: what can be seen is not dashed.
pub(crate) fn merge_lines(visible: &mut Vec<Segment>, hidden: &mut Vec<Segment>) {
    struct Bucket {
        direction: DVec2,
        normal: DVec2,
        offset: f64,
        visible: Vec<(f64, f64)>,
        hidden: Vec<(f64, f64)>,
    }

    let mut buckets: BTreeMap<(i64, i64), Bucket> = BTreeMap::new();
    for (segment, is_visible) in visible
        .iter()
        .map(|segment| (segment, true))
        .chain(hidden.iter().map(|segment| (segment, false)))
    {
        let Some(mut direction) = (segment.b - segment.a).try_normalize() else {
            continue;
        };
        // One direction per line, so the two triangles of a quad agree.
        if direction.x < -1e-12 || (direction.x.abs() <= 1e-12 && direction.y < 0.0) {
            direction = -direction;
        }
        let normal = DVec2::new(-direction.y, direction.x);
        let offset = normal.dot(segment.a);
        let key = (
            (direction.y.atan2(direction.x) / 1e-6).round() as i64,
            (offset / 1e-6).round() as i64,
        );
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
            direction,
            normal,
            offset,
            visible: Vec::new(),
            hidden: Vec::new(),
        });
        let (from, to) = (direction.dot(segment.a), direction.dot(segment.b));
        let span = (from.min(to), from.max(to));
        if is_visible {
            bucket.visible.push(span);
        } else {
            bucket.hidden.push(span);
        }
    }

    let mut new_visible = Vec::new();
    let mut new_hidden = Vec::new();
    for (_, bucket) in buckets {
        let visible_spans = union(bucket.visible);
        let hidden_spans = subtract(bucket.hidden, &visible_spans);
        let place = |(from, to): (f64, f64)| {
            Segment::new(
                bucket.normal * bucket.offset + bucket.direction * from,
                bucket.normal * bucket.offset + bucket.direction * to,
            )
        };
        new_visible.extend(visible_spans.into_iter().map(place));
        new_hidden.extend(hidden_spans.into_iter().map(place));
    }
    *visible = new_visible;
    *hidden = new_hidden;
}

fn union(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (start, end) in spans {
        if end - start <= 1e-9 {
            continue;
        }
        match out.last_mut() {
            Some(last) if start <= last.1 + 1e-9 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

fn subtract(spans: Vec<(f64, f64)>, cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for (start, end) in union(spans) {
        let mut cursor = start;
        for (cut_start, cut_end) in cut {
            if *cut_end <= cursor + 1e-9 {
                continue;
            }
            if *cut_start >= end - 1e-9 {
                break;
            }
            if *cut_start > cursor + 1e-9 {
                out.push((cursor, cut_start.min(end)));
            }
            cursor = cursor.max(*cut_end);
            if cursor >= end - 1e-9 {
                break;
            }
        }
        if end > cursor + 1e-9 {
            out.push((cursor, end));
        }
    }
    out
}
/// The edges worth drawing, welded so two triangles sharing an edge count once.
fn feature_edges(mesh: &Mesh, look: DVec3) -> (Vec<(DVec3, DVec3)>, usize) {
    let mut welded: BTreeMap<[u64; 3], u32> = BTreeMap::new();
    let mut points: Vec<DVec3> = Vec::new();
    let mut faces: BTreeMap<(u32, u32), EdgeFaces> = BTreeMap::new();
    let mut skipped = 0;

    for [a, b, c] in mesh.triangle_corners() {
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            skipped += 1;
            continue;
        }
        let normal = (b - a).cross(c - a);
        if normal.length_squared() <= 1e-24 {
            continue;
        }
        let normal = normal.normalize();
        let mut id_of = |point: DVec3| -> u32 {
            let key = [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()];
            let next = points.len() as u32;
            let id = *welded.entry(key).or_insert(next);
            if id as usize == points.len() {
                points.push(point);
            }
            id
        };
        let (ia, ib, ic) = (id_of(a), id_of(b), id_of(c));
        for (from, to) in [(ia, ib), (ib, ic), (ic, ia)] {
            if from == to {
                continue;
            }
            let key = (from.min(to), from.max(to));
            let edge = faces.entry(key).or_default();
            if edge.count < 2 {
                edge.normals[edge.count as usize] = normal;
            }
            edge.count = edge.count.saturating_add(1);
        }
    }

    let mut edges = Vec::new();
    for ((from, to), edge) in faces {
        if edges.len() >= MAX_FEATURE_EDGES {
            break;
        }
        let draw = if edge.count < 2 {
            true
        } else {
            let first = edge.normals[0].dot(look);
            let second = edge.normals[1].dot(look);
            let crease = edge.normals[0].dot(edge.normals[1]) < CREASE_COSINE;
            // One face looks at the viewer and the other away: the surface turns here.
            let silhouette = (first < -1e-6 && second > 1e-6) || (second < -1e-6 && first > 1e-6);
            crease || silhouette
        };
        if !draw {
            continue;
        }
        let (a, b) = (points[from as usize], points[to as usize]);
        if a.distance(b) > SEGMENT_EPSILON {
            edges.push((a, b));
        }
    }
    (edges, skipped)
}
