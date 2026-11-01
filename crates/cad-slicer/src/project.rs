//! Projection and hidden lines.
//!
//! Lines come from the mesh's own features, not from every triangle edge: an edge on the
//! boundary of an open mesh, an edge where two faces meet at a crease, or an edge where a
//! curved surface turns away from the viewer. That is what a drawing shows, and it keeps the
//! output small.
//!
//! Visibility is exact: each projected edge is clipped against the projected triangles nearer
//! the viewer, found through a uniform grid, and the parts they cover are its hidden runs.
//! Each cell lists its triangles nearest first, so an edge stops at the first ones that
//! cannot hide it, and a fully covered edge stops at once.

use crate::cut::Frame;
use crate::geom::{Segment, SEGMENT_EPSILON};
use glam::{DVec2, DVec3};
use ifc_geometry::Mesh;
use std::collections::BTreeMap;

/// Triangles one drawing may occlude with.
const MAX_OCCLUDERS: usize = 4_000_000;
/// Triangles one element may offer before its projection is skipped.
const MAX_ELEMENT_TRIANGLES: usize = 150_000;
/// Feature edges one element may contribute.
const MAX_FEATURE_EDGES: usize = 100_000;
/// Edge against triangle tests one drawing may run before the projection stops and says so.
const MAX_OCCLUSION_TESTS: usize = 200_000_000;
/// Cosines above this are the same plane: a tessellated curve, not a crease.
const CREASE_COSINE: f64 = 0.94;
/// A point this far behind a surface is not hidden by the surface itself. Metres.
const OCCLUSION_EPSILON: f64 = 1e-6;
/// Cells one triangle may occupy; wider ones go to a list every edge checks.
const MAX_CELLS_PER_TRIANGLE: usize = 1024;
/// Grid cells along each side of the drawing.
const GRID_RANGE: (f64, f64) = (64.0, 1024.0);
/// How far a point may lie outside a triangle and still count as on it, in the units of a
/// cross product: without this, an edge along a triangle's own border would flicker.
const COVER_SLACK: f64 = 1e-9;
/// Gaps between hidden spans narrower than this are rounding between neighbours. Metres.
const JOIN_GAP: f64 = 1e-7;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectReport {
    pub non_finite: usize,
    pub too_large: bool,
    pub truncated: bool,
}

/// A projected triangle: two sheet coordinates and the depth at each corner.
#[derive(Debug, Clone, Copy)]
struct Flat {
    points: [DVec2; 3],
    depths: [f64; 3],
    area: f64,
    nearest: f64,
}

/// The projected triangles of every mesh, in a grid so an edge only meets what is near it.
pub(crate) struct Occluders {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
    wide: Vec<usize>,
    triangles: Vec<Flat>,
    /// Which edge last tested each triangle, so one crossing many cells counts once.
    seen: Vec<usize>,
    edge: usize,
    tests_left: usize,
}

impl Occluders {
    /// `reach` is the drawing's longest side and `triangles` a count of what will be added.
    pub(crate) fn new(reach: f64, triangles: usize) -> Self {
        let side = ((triangles as f64).sqrt() / 2.0).clamp(GRID_RANGE.0, GRID_RANGE.1);
        Self {
            cell: (reach.max(0.0) / side).max(1e-3),
            cells: BTreeMap::new(),
            wide: Vec::new(),
            triangles: Vec::new(),
            seen: Vec::new(),
            edge: 0,
            tests_left: MAX_OCCLUSION_TESTS,
        }
    }

    /// Call once everything is in: nearest first, with the id as a stable tie-break.
    pub(crate) fn finish(&mut self) {
        let triangles = &self.triangles;
        let order = |a: &usize, b: &usize| {
            triangles[*a].nearest.total_cmp(&triangles[*b].nearest).then(a.cmp(b))
        };
        self.wide.sort_by(order);
        for ids in self.cells.values_mut() {
            ids.sort_by(order);
        }
        self.seen = vec![0; self.triangles.len()];
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
            nearest: depths[0].min(depths[1]).min(depths[2]),
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

    /// Clips the edge against one triangle: the span of it that lies under the triangle and
    /// behind it. Everything here is linear along the edge, so each test is one clip.
    fn covered(flat: &Flat, edge: &Edge) -> Option<(f64, f64)> {
        let (mut start, mut end) = (0.0_f64, 1.0_f64);
        let sign = flat.area.signum();
        let [p, q, r] = flat.points;
        let mut clip = |at_start: f64, at_end: f64| -> bool {
            if at_start < 0.0 && at_end < 0.0 {
                return false;
            }
            if at_start < 0.0 {
                start = start.max(at_start / (at_start - at_end));
            } else if at_end < 0.0 {
                end = end.min(at_start / (at_start - at_end));
            }
            start < end
        };
        for (from, to) in [(p, q), (q, r), (r, p)] {
            let side = to - from;
            let at_start = sign * side.perp_dot(edge.a - from) + COVER_SLACK;
            let at_end = sign * side.perp_dot(edge.b - from) + COVER_SLACK;
            if !clip(at_start, at_end) {
                return None;
            }
        }
        // The triangle's depth under each end of the edge, by barycentric weights.
        let depth_under = |point: DVec2| {
            let wp = (q - point).perp_dot(r - point) / flat.area;
            let wq = (r - point).perp_dot(p - point) / flat.area;
            let wr = (p - point).perp_dot(q - point) / flat.area;
            wp * flat.depths[0] + wq * flat.depths[1] + wr * flat.depths[2]
        };
        let (under_start, under_end) = (depth_under(edge.a), depth_under(edge.b));
        let behind_start = edge.depth_a - under_start - OCCLUSION_EPSILON;
        let behind_end = edge.depth_b - under_end - OCCLUSION_EPSILON;
        if !behind_start.is_finite() || !behind_end.is_finite() || !clip(behind_start, behind_end) {
            return None;
        }
        // The part of a sloped triangle in front of the plane has been cut away.
        if !clip(under_start, under_end) {
            return None;
        }
        Some((start, end))
    }

    /// Tests the edge against `ids`, nearest first. `false` when the budget has run out.
    fn test_all(ids: &[usize], data: &mut Tests<'_>, edge: &Edge) -> bool {
        for id in ids {
            let flat = &data.triangles[*id];
            if flat.nearest + OCCLUSION_EPSILON >= edge.deepest {
                break;
            }
            if data.seen[*id] == data.edge {
                continue;
            }
            data.seen[*id] = data.edge;
            if *data.tests_left == 0 {
                return false;
            }
            *data.tests_left -= 1;
            if let Some(span) = Self::covered(flat, edge) {
                data.spans.push(span);
            }
        }
        true
    }

    /// The hidden spans of an edge, as sorted parameters along it. `None` when the budget ran
    /// out, because a partial answer would draw hidden lines as visible.
    fn hidden_spans(&mut self, edge: &Edge) -> Option<Vec<(f64, f64)>> {
        self.edge += 1;
        let join = JOIN_GAP / edge.a.distance(edge.b).max(JOIN_GAP);
        let mut data = Tests {
            triangles: &self.triangles,
            seen: &mut self.seen,
            edge: self.edge,
            tests_left: &mut self.tests_left,
            spans: Vec::new(),
        };
        if !Self::test_all(&self.wide, &mut data, edge) {
            return None;
        }
        let mut merged = merge_spans(&mut data.spans, join);
        let mut walk = GridWalk::new(edge.a, edge.b, self.cell);
        while let Some(key) = walk.next() {
            if merged.len() == 1 && merged[0].0 <= 0.0 && merged[0].1 >= 1.0 {
                break;
            }
            let Some(ids) = self.cells.get(&key) else { continue };
            let before = data.spans.len();
            if !Self::test_all(ids, &mut data, edge) {
                return None;
            }
            if data.spans.len() != before {
                merged = merge_spans(&mut data.spans, join);
            }
        }
        Some(merged)
    }
}

/// An edge on the sheet, with the depth at each end and the deepest of the two.
struct Edge {
    a: DVec2,
    b: DVec2,
    depth_a: f64,
    depth_b: f64,
    deepest: f64,
}

/// What one edge's tests share, kept apart so the grid can stay borrowed.
struct Tests<'a> {
    triangles: &'a [Flat],
    seen: &'a mut [usize],
    edge: usize,
    tests_left: &'a mut usize,
    spans: Vec<(f64, f64)>,
}

/// Sorts and joins spans in place, snapping to the ends of the edge, and returns a copy.
fn merge_spans(spans: &mut Vec<(f64, f64)>, join: f64) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans.iter().copied() {
        match out.last_mut() {
            Some(last) if start <= last.1 + join => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    if let Some(first) = out.first_mut().filter(|first| first.0 <= join) {
        first.0 = 0.0;
    }
    if let Some(last) = out.last_mut().filter(|last| last.1 >= 1.0 - join) {
        last.1 = 1.0;
    }
    spans.clone_from(&out);
    out
}

/// The grid cells a segment passes through, in order.
struct GridWalk {
    cell: (i64, i64),
    last: (i64, i64),
    step: (i64, i64),
    next_at: (f64, f64),
    each: (f64, f64),
    left: u64,
}

impl GridWalk {
    fn new(a: DVec2, b: DVec2, size: f64) -> Self {
        let cell = ((a.x / size).floor() as i64, (a.y / size).floor() as i64);
        let last = ((b.x / size).floor() as i64, (b.y / size).floor() as i64);
        let axis = |from: f64, to: f64, index: i64| -> (i64, f64, f64) {
            let run = to - from;
            if run.abs() <= f64::MIN_POSITIVE {
                return (0, f64::INFINITY, f64::INFINITY);
            }
            let step = if run > 0.0 { 1 } else { -1 };
            let border = (index + i64::from(run > 0.0)) as f64 * size;
            (step, (border - from) / run, size / run.abs())
        };
        let (step_x, next_x, each_x) = axis(a.x, b.x, cell.0);
        let (step_y, next_y, each_y) = axis(a.y, b.y, cell.1);
        Self {
            cell,
            last,
            step: (step_x, step_y),
            next_at: (next_x, next_y),
            each: (each_x, each_y),
            left: (last.0 - cell.0).unsigned_abs() + (last.1 - cell.1).unsigned_abs() + 1,
        }
    }

    fn next(&mut self) -> Option<(i64, i64)> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let current = self.cell;
        if self.left == 0 {
            return Some(self.last);
        }
        if self.next_at.0 < self.next_at.1 {
            self.cell.0 += self.step.0;
            self.next_at.0 += self.each.0;
        } else {
            self.cell.1 += self.step.1;
            self.next_at.1 += self.each.1;
        }
        Some(current)
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
    // One only partly beyond it still hides what lies behind its near part.
    let ceiling = limit.unwrap_or(f64::INFINITY);
    for [a, b, c] in mesh.triangle_corners() {
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            report.non_finite += 1;
            continue;
        }
        let depths = [frame.depth(a), frame.depth(b), frame.depth(c)];
        if depths.iter().any(|depth| !depth.is_finite()) {
            continue;
        }
        if depths[0].min(depths[1]).min(depths[2]) > ceiling {
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
    visible: &mut Vec<Segment>,
    hidden: &mut Vec<Segment>,
) -> ProjectReport {
    let mut report = ProjectReport::default();
    if mesh.triangles.len() > MAX_ELEMENT_TRIANGLES {
        report.too_large = true;
        return report;
    }
    let (edges, skipped, too_many) = feature_edges(mesh, frame.look);
    report.non_finite += skipped;
    report.truncated = too_many;

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

        let edge = Edge {
            a,
            b,
            depth_a,
            depth_b,
            deepest: depth_a.max(depth_b),
        };
        let Some(spans) = occluders.hidden_spans(&edge) else {
            report.truncated = true;
            break;
        };
        let mut cursor = 0.0;
        for (start, end) in spans {
            push_run(a, b, cursor, start, true, visible, hidden);
            push_run(a, b, start, end, false, visible, hidden);
            cursor = end;
        }
        push_run(a, b, cursor, 1.0, true, visible, hidden);
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
/// The edges worth drawing, welded so two triangles sharing an edge count once, the triangles
/// skipped, and whether the element had more edges than it may contribute.
fn feature_edges(mesh: &Mesh, look: DVec3) -> (Vec<(DVec3, DVec3)>, usize, bool) {
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
    let mut too_many = false;
    for ((from, to), edge) in faces {
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
            if edges.len() >= MAX_FEATURE_EDGES {
                too_many = true;
                break;
            }
            edges.push((a, b));
        }
    }
    (edges, skipped, too_many)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(z: f64) -> Mesh {
        Mesh {
            positions: vec![
                DVec3::new(0.0, 0.0, z),
                DVec3::new(4.0, 0.0, z),
                DVec3::new(4.0, 4.0, z),
                DVec3::new(0.0, 4.0, z),
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        }
    }

    #[test]
    fn a_spent_budget_is_reported_and_draws_nothing_unchecked() {
        let frame = Frame::of(crate::cut::Cut::Plan { z: 10.0 }).unwrap();
        let (roof, floor) = (square(5.0), square(1.0));
        let mut grid = Occluders::new(4.0, 4);
        add_occluders(&roof, &frame, None, &mut grid);
        grid.finish();

        let (mut visible, mut hidden) = (Vec::new(), Vec::new());
        grid.tests_left = 0;
        let report = project_mesh(&floor, &frame, None, &mut grid, &mut visible, &mut hidden);
        assert!(report.truncated);
        assert!(visible.is_empty(), "an unchecked edge would look visible: {visible:?}");

        grid.tests_left = 1000;
        let report = project_mesh(&floor, &frame, None, &mut grid, &mut visible, &mut hidden);
        assert!(!report.truncated);
        assert!(visible.is_empty());
        assert!(!hidden.is_empty());
    }

    #[test]
    fn a_walk_visits_every_cell_a_segment_crosses() {
        let cells = |a: DVec2, b: DVec2| {
            let mut walk = GridWalk::new(a, b, 1.0);
            let mut out = Vec::new();
            while let Some(cell) = walk.next() {
                out.push(cell);
            }
            out
        };
        assert_eq!(cells(DVec2::new(0.5, 0.5), DVec2::new(0.6, 0.7)), [(0, 0)]);
        assert_eq!(
            cells(DVec2::new(0.5, 0.5), DVec2::new(2.5, 0.5)),
            [(0, 0), (1, 0), (2, 0)]
        );
        assert_eq!(
            cells(DVec2::new(2.5, 1.5), DVec2::new(0.5, 1.5)),
            [(2, 1), (1, 1), (0, 1)]
        );
        // Up and to the right, crossing X first: y reaches 1 at about x = 2.17.
        assert_eq!(
            cells(DVec2::new(0.5, 0.25), DVec2::new(2.5, 1.15)),
            [(0, 0), (1, 0), (2, 0), (2, 1)]
        );
    }
}
