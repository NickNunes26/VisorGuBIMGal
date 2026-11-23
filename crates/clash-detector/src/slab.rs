//! Element A slab classification and PK clash grouping along a horizontal alignment (F2,
//! `docs/12_clash_alignment_dxf.md` §3.3).
//!
//! 100% generic (no hardcoded IFC classes):
//! - Projects all `f64` vertices of Element A onto the horizontal unit alignment tangent
//!   `t_hat = (tx, ty, 0)` and the transverse plane `(r_hat, z_hat)`.
//! - **Longitudinal Element A** (`delta_s > 0.15 m` and `delta_s > 2.5 * max(w_perp, h_perp)`):
//!   produces a zero-depth cut (`depth = 0`) at `s_cut = clash_point · t_hat` (maximum penetration).
//! - **Transverse / compact Element A**: produces the exact slab `[s_min, s_max]` covering 100% of
//!   Element A (with an optional safety cap for highly oblique elements), independent of whether
//!   the clash occurred on its front, center, or rear face.
//! - **PK Grouping**: sorts clashes by station (`pk_m`) and merges consecutive clashes separated by
//!   `<= merge_tolerance_m` (default `0.020 m` = `20 mm`; `0.0` disables merging).

use glam::DVec3;

/// Default minimum longitudinal extent (metres) to classify Element A as longitudinal.
pub const DEFAULT_LONGITUDINAL_MIN_EXTENT_M: f64 = 0.15;
/// Default aspect ratio `delta_s / max(w_perp, h_perp)` above which Element A is longitudinal.
pub const DEFAULT_LONGITUDINAL_ASPECT_RATIO: f64 = 2.5;
/// Default safety cap (metres) on a single transverse slab for strongly oblique elements.
pub const DEFAULT_MAX_TRANSVERSE_SLAB_M: f64 = 0.25;
/// Default PK merge tolerance (metres = 20 mm).
pub const DEFAULT_MERGE_TOLERANCE_M: f64 = 0.020;

/// Generic orientation of Element A relative to the horizontal alignment tangent at a clash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementOrientation {
    Transverse,
    Longitudinal,
}

/// Configuration for Element A slab classification and PK grouping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabConfig {
    pub longitudinal_min_extent_m: f64,
    pub longitudinal_aspect_ratio: f64,
    /// Upper cap on a transverse slab width (metres). Set `<= 0.0` or `f64::INFINITY` to disable.
    pub max_transverse_slab_m: f64,
    /// Consecutive clashes within this PK distance (metres) merge into one section. `0.0` disables.
    pub merge_tolerance_m: f64,
}

impl Default for SlabConfig {
    fn default() -> Self {
        Self {
            longitudinal_min_extent_m: DEFAULT_LONGITUDINAL_MIN_EXTENT_M,
            longitudinal_aspect_ratio: DEFAULT_LONGITUDINAL_ASPECT_RATIO,
            max_transverse_slab_m: DEFAULT_MAX_TRANSVERSE_SLAB_M,
            merge_tolerance_m: DEFAULT_MERGE_TOLERANCE_M,
        }
    }
}

/// Computed slab span along the horizontal unit tangent `t_hat = (tx, ty, 0)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabSpan {
    pub orientation: ElementOrientation,
    /// Lower bound of the cut slab along `t_hat` (`p · t_hat`), in metres.
    pub s_min: f64,
    /// Upper bound of the cut slab along `t_hat` (`p · t_hat`), in metres.
    pub s_max: f64,
    /// Projection of the clash point onto `t_hat`.
    pub s_clash: f64,
    /// Full longitudinal extent of Element A along `t_hat` (`s_a_max - s_a_min`).
    pub delta_s_a: f64,
    /// Transverse horizontal extent of Element A along `r_hat = (ty, -tx, 0)`.
    pub width_perp_m: f64,
    /// Vertical extent of Element A along `z_hat = (0, 0, 1)`.
    pub height_perp_m: f64,
}

impl SlabSpan {
    /// Slab depth in metres (`0.0` for longitudinal elements).
    #[inline]
    pub fn depth_m(&self) -> f64 {
        (self.s_max - self.s_min).max(0.0)
    }
}

/// Normalizes a 2D horizontal tangent into a unit 3D vector `(tx, ty, 0)`.
#[inline]
pub fn normalize_horizontal_tangent(tangent_xy: [f64; 2]) -> DVec3 {
    let len = tangent_xy[0].hypot(tangent_xy[1]);
    if len > 1e-12 && len.is_finite() {
        DVec3::new(tangent_xy[0] / len, tangent_xy[1] / len, 0.0)
    } else {
        DVec3::X
    }
}

/// Classifies Element A (given its `f64` vertices) relative to the alignment's horizontal tangent
/// `tangent_xy` and computes the exact slab `[s_min, s_max]` (or `depth = 0` at `clash_point`).
pub fn compute_element_a_slab(
    element_a_vertices: &[DVec3],
    tangent_xy: [f64; 2],
    clash_point: DVec3,
    config: &SlabConfig,
) -> SlabSpan {
    let t_hat = normalize_horizontal_tangent(tangent_xy);
    let r_hat = DVec3::new(t_hat.y, -t_hat.x, 0.0);
    let s_clash = clash_point.dot(t_hat);

    let mut s_min_a = f64::INFINITY;
    let mut s_max_a = f64::NEG_INFINITY;
    let mut r_min_a = f64::INFINITY;
    let mut r_max_a = f64::NEG_INFINITY;
    let mut z_min_a = f64::INFINITY;
    let mut z_max_a = f64::NEG_INFINITY;

    for &v in element_a_vertices {
        if !v.is_finite() {
            continue;
        }
        let s = v.dot(t_hat);
        let r = v.dot(r_hat);
        s_min_a = s_min_a.min(s);
        s_max_a = s_max_a.max(s);
        r_min_a = r_min_a.min(r);
        r_max_a = r_max_a.max(r);
        z_min_a = z_min_a.min(v.z);
        z_max_a = z_max_a.max(v.z);
    }

    if !s_min_a.is_finite() || !s_max_a.is_finite() {
        return SlabSpan {
            orientation: ElementOrientation::Longitudinal,
            s_min: s_clash,
            s_max: s_clash,
            s_clash,
            delta_s_a: 0.0,
            width_perp_m: 0.0,
            height_perp_m: 0.0,
        };
    }

    let delta_s_a = (s_max_a - s_min_a).max(0.0);
    let width_perp_m = (r_max_a - r_min_a).max(0.0);
    let height_perp_m = (z_max_a - z_min_a).max(0.0);
    let max_perp = width_perp_m.max(height_perp_m);

    let is_longitudinal = delta_s_a > config.longitudinal_min_extent_m
        && delta_s_a > config.longitudinal_aspect_ratio * max_perp;

    if is_longitudinal {
        SlabSpan {
            orientation: ElementOrientation::Longitudinal,
            s_min: s_clash,
            s_max: s_clash,
            s_clash,
            delta_s_a,
            width_perp_m,
            height_perp_m,
        }
    } else {
        let (s_min, s_max) =
            if config.max_transverse_slab_m.is_finite()
                && config.max_transverse_slab_m > 0.0
                && delta_s_a > config.max_transverse_slab_m
            {
                let half = config.max_transverse_slab_m * 0.5;
                let mut lo = (s_clash - half).max(s_min_a);
                let mut hi = lo + config.max_transverse_slab_m;
                if hi > s_max_a {
                    hi = s_max_a;
                    lo = (hi - config.max_transverse_slab_m).max(s_min_a);
                }
                (lo, hi)
            } else {
                (s_min_a, s_max_a)
            };

        SlabSpan {
            orientation: ElementOrientation::Transverse,
            s_min,
            s_max,
            s_clash,
            delta_s_a,
            width_perp_m,
            height_perp_m,
        }
    }
}

/// One projected clash ready to be grouped by PK into cross-section DXF sheets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClashSlabInput {
    /// Caller-owned index into the clash report array.
    pub clash_index: usize,
    /// Projected station (PK) along the alignment in metres.
    pub pk_m: f64,
    /// Penetration depth in metres (`> 0` for hard clashes, `0` for soft clashes).
    pub penetration_m: f64,
    /// Minimum surface gap in metres (`0` for hard clashes, `> 0` for soft clashes).
    pub gap_m: f64,
    /// Representative 3D clash point in model-local metres (maximum penetration for hard clashes).
    pub point: DVec3,
    /// Unit horizontal tangent `(tx, ty)` of the alignment at `pk_m` in model-local space.
    pub tangent_xy: [f64; 2],
    /// Precomputed Element A slab along `tangent_xy`.
    pub slab: SlabSpan,
}

/// One merged cross-section group along the alignment (one individual DXF sheet).
#[derive(Debug, Clone, PartialEq)]
pub struct ClashSlabGroup {
    /// Representative PK of the group (metres).
    pub pk_m: f64,
    /// Representative horizontal unit tangent `(tx, ty, 0)` of the cut plane.
    pub tangent: DVec3,
    /// Representative 3D anchor point on the section plane (from the primary clash).
    pub anchor_point: DVec3,
    /// `Transverse` if any clash in the group is transverse; `Longitudinal` if all are longitudinal.
    pub orientation: ElementOrientation,
    /// Combined slab start along `tangent` (`p · tangent`), in metres.
    pub s_min: f64,
    /// Combined slab end along `tangent` (`p · tangent`), in metres (`== s_min` when longitudinal).
    pub s_max: f64,
    /// Indices of all clashes merged into this section, ordered by PK.
    pub clash_indices: Vec<usize>,
}

impl ClashSlabGroup {
    #[inline]
    pub fn depth_m(&self) -> f64 {
        (self.s_max - self.s_min).max(0.0)
    }
}

/// Sorts clashes by `pk_m` and merges consecutive clashes separated by `<= merge_tolerance_m`
/// (default `0.020 m` = 20 mm; `0.0` disables merging).
///
/// - If any clash in a merged group has a **transverse** Element A, the group's slab is the union
///   of all transverse slabs in the group.
/// - If **all** clashes in a merged group are **longitudinal**, the group's slab has `depth = 0`
///   at the clash with maximum penetration (tie-broken by smallest gap, then lowest PK).
pub fn group_clash_slabs(
    inputs: &[ClashSlabInput],
    merge_tolerance_m: f64,
) -> Vec<ClashSlabGroup> {
    if inputs.is_empty() {
        return Vec::new();
    }

    let tol = if merge_tolerance_m.is_finite() && merge_tolerance_m > 0.0 {
        merge_tolerance_m
    } else {
        0.0
    };

    let mut sorted: Vec<ClashSlabInput> = inputs.to_vec();
    sorted.sort_by(|a, b| {
        a.pk_m
            .total_cmp(&b.pk_m)
            .then_with(|| a.clash_index.cmp(&b.clash_index))
    });

    let mut groups: Vec<ClashSlabGroup> = Vec::new();
    let mut start = 0usize;

    while start < sorted.len() {
        let mut end = start + 1;
        if tol > 0.0 {
            while end < sorted.len() && (sorted[end].pk_m - sorted[end - 1].pk_m) <= tol {
                end += 1;
            }
        }
        groups.push(build_group(&sorted[start..end]));
        start = end;
    }

    groups
}

fn build_group(slice: &[ClashSlabInput]) -> ClashSlabGroup {
    // Primary clash: highest penetration, then smallest gap (for soft), then lowest PK.
    let primary = slice
        .iter()
        .max_by(|a, b| {
            a.penetration_m
                .total_cmp(&b.penetration_m)
                .then_with(|| b.gap_m.total_cmp(&a.gap_m))
                .then_with(|| b.pk_m.total_cmp(&a.pk_m))
        })
        .expect("non-empty group slice");

    let tangent = normalize_horizontal_tangent(primary.tangent_xy);
    let any_transverse = slice
        .iter()
        .any(|c| c.slab.orientation == ElementOrientation::Transverse);

    let (orientation, s_min, s_max, anchor_point, pk_m) = if any_transverse {
        let mut u_min = f64::INFINITY;
        let mut u_max = f64::NEG_INFINITY;
        for c in slice {
            if c.slab.orientation == ElementOrientation::Transverse {
                // Re-anchor c's slab interval onto the group's representative tangent.
                let s_c_on_rep = c.point.dot(tangent);
                let back = c.slab.s_clash - c.slab.s_min;
                let fwd = c.slab.s_max - c.slab.s_clash;
                u_min = u_min.min(s_c_on_rep - back);
                u_max = u_max.max(s_c_on_rep + fwd);
            }
        }
        (
            ElementOrientation::Transverse,
            u_min,
            u_max,
            primary.point,
            primary.pk_m,
        )
    } else {
        let s_cut = primary.point.dot(tangent);
        (
            ElementOrientation::Longitudinal,
            s_cut,
            s_cut,
            primary.point,
            primary.pk_m,
        )
    };

    ClashSlabGroup {
        pk_m,
        tangent,
        anchor_point,
        orientation,
        s_min,
        s_max,
        clash_indices: slice.iter().map(|c| c.clash_index).collect(),
    }
}
