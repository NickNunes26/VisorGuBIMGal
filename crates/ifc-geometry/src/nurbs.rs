//! B-spline curves and surfaces, rational or not, and meshing of trimmed surfaces.
//!
//! Evaluation is the Cox-de Boor recurrence. A trimmed face is meshed in parameter space:
//! its boundary is mapped onto the surface by Newton iteration, then clipped against a grid
//! of cells, so the result follows both the curvature and the trim.

use crate::mesh::Mesh;
use earcut::Earcut;
use glam::{DVec2, DVec3, DVec4};

/// Hostile files can claim any size, and evaluation cost grows with the square of the degree.
const MAX_DEGREE: usize = 32;
const STEPS_PER_SPAN: usize = 8;
const MAX_CURVE_POINTS: usize = 4096;
const MAX_CELLS_PER_DIRECTION: usize = 64;
const NEWTON_STEPS: usize = 12;
/// Grid resolution used to seed Newton iteration.
const SEED_GRID: usize = 16;

/// Expands `Knots` and `KnotMultiplicities`. `None` unless they suit `count` control points.
pub(crate) fn expand_knots(
    knots: &[f64],
    multiplicities: &[usize],
    degree: usize,
    count: usize,
) -> Option<Vec<f64>> {
    if knots.len() != multiplicities.len() || degree == 0 || degree > MAX_DEGREE || count <= degree {
        return None;
    }
    if multiplicities.iter().sum::<usize>() != count + degree + 1 {
        return None;
    }
    let expanded: Vec<f64> = knots
        .iter()
        .zip(multiplicities)
        .flat_map(|(knot, times)| std::iter::repeat_n(*knot, *times))
        .collect();
    let valid = expanded.iter().all(|knot| knot.is_finite())
        && expanded.windows(2).all(|pair| pair[0] <= pair[1])
        && expanded[degree] < expanded[count];
    valid.then_some(expanded)
}

/// The span holding `t` and its `degree + 1` non-zero basis functions.
fn basis(knots: &[f64], degree: usize, count: usize, t: f64) -> (usize, Vec<f64>) {
    let t = t.clamp(knots[degree], knots[count]);
    let mut span = degree;
    while span + 1 < count && t >= knots[span + 1] {
        span += 1;
    }
    let mut values = vec![0.0; degree + 1];
    values[0] = 1.0;
    let (mut left, mut right) = (vec![0.0; degree + 1], vec![0.0; degree + 1]);
    for j in 1..=degree {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let denominator = right[r + 1] + left[j - r];
            let share = if denominator == 0.0 { 0.0 } else { values[r] / denominator };
            values[r] = saved + right[r + 1] * share;
            saved = left[j - r] * share;
        }
        values[j] = saved;
    }
    (span, values)
}

fn project(point: DVec4) -> Option<DVec3> {
    let result = point.truncate() / point.w;
    result.is_finite().then_some(result)
}

fn distinct_spans(knots: &[f64], degree: usize, count: usize) -> usize {
    knots[degree..=count].windows(2).filter(|pair| pair[0] < pair[1]).count()
}

pub(crate) struct Curve {
    pub degree: usize,
    pub points: Vec<DVec3>,
    pub weights: Vec<f64>,
    pub knots: Vec<f64>,
}

impl Curve {
    fn at(&self, t: f64) -> Option<DVec3> {
        let (span, values) = basis(&self.knots, self.degree, self.points.len(), t);
        let mut sum = DVec4::ZERO;
        for (offset, value) in values.iter().enumerate() {
            let index = span - self.degree + offset;
            let weight = self.weights[index] * value;
            sum += (self.points[index] * weight).extend(weight);
        }
        project(sum)
    }

    /// A polyline through the curve, with more points on higher degrees and more spans.
    pub fn sample(&self) -> Option<Vec<DVec3>> {
        let count = self.points.len();
        let (start, end) = (self.knots[self.degree], self.knots[count]);
        let spans = distinct_spans(&self.knots, self.degree, count);
        let steps = (spans * self.degree * STEPS_PER_SPAN / 2).clamp(1, MAX_CURVE_POINTS);
        (0..=steps)
            .map(|step| self.at(start + (end - start) * step as f64 / steps as f64))
            .collect()
    }
}

pub(crate) struct Surface {
    pub u_degree: usize,
    pub v_degree: usize,
    /// One row per U index.
    pub points: Vec<Vec<DVec3>>,
    pub weights: Vec<Vec<f64>>,
    pub u_knots: Vec<f64>,
    pub v_knots: Vec<f64>,
}

impl Surface {
    fn u_count(&self) -> usize {
        self.points.len()
    }

    fn v_count(&self) -> usize {
        self.points[0].len()
    }

    fn u_domain(&self) -> (f64, f64) {
        (self.u_knots[self.u_degree], self.u_knots[self.u_count()])
    }

    fn v_domain(&self) -> (f64, f64) {
        (self.v_knots[self.v_degree], self.v_knots[self.v_count()])
    }

    fn at(&self, u: f64, v: f64) -> Option<DVec3> {
        let (u_span, u_values) = basis(&self.u_knots, self.u_degree, self.u_count(), u);
        let (v_span, v_values) = basis(&self.v_knots, self.v_degree, self.v_count(), v);
        let mut sum = DVec4::ZERO;
        for (i, u_value) in u_values.iter().enumerate() {
            let row = u_span - self.u_degree + i;
            for (j, v_value) in v_values.iter().enumerate() {
                let column = v_span - self.v_degree + j;
                let weight = self.weights[row][column] * u_value * v_value;
                sum += (self.points[row][column] * weight).extend(weight);
            }
        }
        project(sum)
    }

    /// The closest parameters to `target`, from the best of a coarse grid of starts.
    fn invert(&self, target: DVec3, grid: &[(DVec2, DVec3)]) -> Option<DVec2> {
        let (u_start, u_end) = self.u_domain();
        let (v_start, v_end) = self.v_domain();
        let closest = |a: &&(DVec2, DVec3), b: &&(DVec2, DVec3)| {
            a.1.distance_squared(target).total_cmp(&b.1.distance_squared(target))
        };
        let mut at = grid.iter().min_by(closest)?.0;
        let (du, dv) = ((u_end - u_start) * 1e-5, (v_end - v_start) * 1e-5);

        for _ in 0..NEWTON_STEPS {
            let here = self.at(at.x, at.y)?;
            let along_u = (self.at(at.x + du, at.y)? - self.at(at.x - du, at.y)?) / (2.0 * du);
            let along_v = (self.at(at.x, at.y + dv)? - self.at(at.x, at.y - dv)?) / (2.0 * dv);
            let miss = target - here;
            let (uu, uv, vv) = (along_u.dot(along_u), along_u.dot(along_v), along_v.dot(along_v));
            let determinant = uu * vv - uv * uv;
            if determinant.abs() < 1e-24 {
                break;
            }
            let (a, b) = (miss.dot(along_u), miss.dot(along_v));
            let step = DVec2::new(vv * a - uv * b, uu * b - uv * a) / determinant;
            at = DVec2::new(
                (at.x + step.x).clamp(u_start, u_end),
                (at.y + step.y).clamp(v_start, v_end),
            );
            if step.length_squared() < 1e-20 {
                break;
            }
        }
        Some(at)
    }

    fn seed_grid(&self) -> Vec<(DVec2, DVec3)> {
        let (u_start, u_end) = self.u_domain();
        let (v_start, v_end) = self.v_domain();
        let mut grid = Vec::with_capacity((SEED_GRID + 1) * (SEED_GRID + 1));
        for i in 0..=SEED_GRID {
            for j in 0..=SEED_GRID {
                let uv = DVec2::new(
                    u_start + (u_end - u_start) * i as f64 / SEED_GRID as f64,
                    v_start + (v_end - v_start) * j as f64 / SEED_GRID as f64,
                );
                if let Some(point) = self.at(uv.x, uv.y) {
                    grid.push((uv, point));
                }
            }
        }
        grid
    }
}

/// Keeps the part of a closed ring on one side of an axis-aligned line.
fn clip_ring(ring: &[DVec2], axis: usize, limit: f64, keep_above: bool) -> Vec<DVec2> {
    let inside = |point: DVec2| {
        let value = point[axis];
        if keep_above { value >= limit } else { value <= limit }
    };
    let mut result = Vec::with_capacity(ring.len());
    for (index, current) in ring.iter().enumerate() {
        let next = ring[(index + 1) % ring.len()];
        if inside(*current) {
            result.push(*current);
        }
        if inside(*current) != inside(next) {
            let share = (limit - current[axis]) / (next[axis] - current[axis]);
            result.push(current.lerp(next, share));
        }
    }
    result
}

fn clip_to_cell(ring: &[DVec2], min: DVec2, max: DVec2) -> Vec<DVec2> {
    let ring = clip_ring(ring, 0, min.x, true);
    let ring = clip_ring(&ring, 0, max.x, false);
    let ring = clip_ring(&ring, 1, min.y, true);
    clip_ring(&ring, 1, max.y, false)
}

fn contains(ring: &[DVec2], point: DVec2) -> bool {
    let mut inside = false;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        if (a.y > point.y) != (b.y > point.y)
            && point.x < a.x + (point.y - a.y) * (b.x - a.x) / (b.y - a.y)
        {
            inside = !inside;
        }
    }
    inside
}

fn touches_cell(ring: &[DVec2], min: DVec2, max: DVec2) -> bool {
    !clip_to_cell(ring, min, max).is_empty()
}

/// Splits `[start, end]` so the cells follow the surface's own spans.
fn grid_lines(knots: &[f64], degree: usize, count: usize, start: f64, end: f64) -> Vec<f64> {
    let spans = distinct_spans(knots, degree, count).max(1);
    let cells = (spans * degree * 2).clamp(4, MAX_CELLS_PER_DIRECTION);
    (0..=cells).map(|cell| start + (end - start) * cell as f64 / cells as f64).collect()
}

/// Meshes a surface between trimming loops, given as points in space.
///
/// Returns triangles wound by parameter direction. A loop that wraps around a closed
/// surface has no interior in parameter space, so such faces keep the band the loops span.
pub(crate) fn mesh_surface(surface: &Surface, loops: &[Vec<DVec3>]) -> Option<Mesh> {
    let seeds = surface.seed_grid();
    let (u_start, u_end) = surface.u_domain();
    let (v_start, v_end) = surface.v_domain();

    let mut rings: Vec<Vec<DVec2>> = Vec::with_capacity(loops.len());
    for boundary in loops {
        let ring: Option<Vec<DVec2>> =
            boundary.iter().map(|point| surface.invert(*point, &seeds)).collect();
        rings.push(ring?);
    }

    let extent = |values: &mut dyn Iterator<Item = f64>| {
        values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| (low.min(value), high.max(value)))
    };
    let (u_low, u_high) = extent(&mut rings.iter().flatten().map(|uv| uv.x));
    let (v_low, v_high) = extent(&mut rings.iter().flatten().map(|uv| uv.y));

    // A jump across most of the domain between neighbours means the loop crossed a seam.
    let crossing = |ring: &[DVec2], axis: usize, span: f64| {
        (0..ring.len()).any(|i| (ring[(i + 1) % ring.len()][axis] - ring[i][axis]).abs() > span * 0.5)
    };
    let u_wraps = rings.iter().any(|ring| crossing(ring, 0, u_end - u_start));
    let v_wraps = rings.iter().any(|ring| crossing(ring, 1, v_end - v_start));
    let trimmed = !(u_wraps || v_wraps);

    let (u_from, u_to) = if u_wraps { (u_start, u_end) } else { (u_low, u_high) };
    let (v_from, v_to) = if v_wraps { (v_start, v_end) } else { (v_low, v_high) };
    if !(u_to > u_from && v_to > v_from) {
        return None;
    }
    let us = grid_lines(&surface.u_knots, surface.u_degree, surface.u_count(), u_from, u_to);
    let vs = grid_lines(&surface.v_knots, surface.v_degree, surface.v_count(), v_from, v_to);

    let mut mesh = Mesh::default();
    let mut emit = |corners: &[DVec2], triangles: &[u32]| -> Option<()> {
        let first = mesh.positions.len() as u32;
        for corner in corners {
            mesh.positions.push(surface.at(corner.x, corner.y)?);
        }
        for triangle in triangles.chunks_exact(3) {
            mesh.triangles.push([first + triangle[0], first + triangle[1], first + triangle[2]]);
        }
        Some(())
    };

    for i in 0..us.len() - 1 {
        for j in 0..vs.len() - 1 {
            let (min, max) = (DVec2::new(us[i], vs[j]), DVec2::new(us[i + 1], vs[j + 1]));
            let square = [min, DVec2::new(max.x, min.y), max, DVec2::new(min.x, max.y)];
            if !trimmed || !rings.iter().any(|ring| touches_cell(ring, min, max)) {
                let centre = (min + max) / 2.0;
                let outside = trimmed
                    && !rings.is_empty()
                    && (!contains(&rings[0], centre) || rings[1..].iter().any(|hole| contains(hole, centre)));
                if !outside {
                    emit(&square, &[0, 1, 2, 0, 2, 3])?;
                }
                continue;
            }

            let outer = clip_to_cell(&rings[0], min, max);
            if outer.len() < 3 {
                continue;
            }
            let mut points: Vec<DVec2> = outer;
            let mut hole_starts = Vec::new();
            for hole in &rings[1..] {
                let hole = clip_to_cell(hole, min, max);
                if hole.len() >= 3 {
                    hole_starts.push(points.len() as u32);
                    points.extend(hole);
                }
            }
            let mut indices: Vec<u32> = Vec::new();
            Earcut::new().earcut(points.iter().map(|point| [point.x, point.y]), &hole_starts, &mut indices);
            for triangle in indices.chunks_exact_mut(3) {
                let [a, b, c] = [0, 1, 2].map(|corner| points[triangle[corner] as usize]);
                if (b - a).perp_dot(c - a) < 0.0 {
                    triangle.swap(1, 2);
                }
            }
            emit(&points, &indices)?;
        }
    }
    (!mesh.is_empty()).then_some(mesh)
}
