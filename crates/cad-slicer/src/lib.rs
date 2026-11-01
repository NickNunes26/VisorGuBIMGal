//! Plan and section cuts, hidden lines, hatching, DXF and SVG export. Owner: DeepSeek (T-050).
//!
//! The crate takes a list of indexed meshes in metres, each tagged with an element, a model
//! and a layer, and never parses IFC. Every failure stays local to its element: it becomes a
//! [`Diagnostic`] and the rest of the drawing carries on.

mod cut;
mod dxf;
mod geom;
mod hatch;
mod project;
mod svg;
mod text;

pub use cut::{Axis, Cut, Frame, Look};
pub use dxf::{DxfOptions, to_dxf, to_dxf_with};
pub use geom::{Loop, Segment};
pub use hatch::HatchStyle;
pub use svg::{SvgOptions, to_svg, to_svg_with};

use ifc_geometry::Mesh;
use glam::DVec2;

/// Something skipped or degraded on one element of one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub model_id: u32,
    pub element_id: u32,
    pub code: &'static str,
    pub message: String,
}

/// One mesh to draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawingMesh {
    pub model_id: u32,
    pub element_id: u32,
    /// Layer or material name, straight from the file: untrusted text.
    pub layer: String,
    pub mesh: Mesh,
    /// The caller's `IfcFillAreaStyleHatching`, when it has one.
    pub hatch: Option<HatchStyle>,
}

impl DrawingMesh {
    pub fn new(model_id: u32, element_id: u32, layer: impl Into<String>, mesh: Mesh) -> Self {
        Self {
            model_id,
            element_id,
            layer: layer.into(),
            mesh,
            hatch: None,
        }
    }
}

/// What to compute. Exporters only serialise what this asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingOptions {
    pub cut: Cut,
    /// How far behind the cut plane the projection reaches, in metres. `None` reaches the
    /// end of the geometry.
    pub depth: Option<f64>,
    /// Visible and hidden lines. Off by default: hidden lines are the expensive part.
    pub include_projection: bool,
    pub include_hatching: bool,
}

impl DrawingOptions {
    pub fn cut_only(cut: Cut) -> Self {
        Self {
            cut,
            depth: None,
            include_projection: false,
            include_hatching: false,
        }
    }
}

/// Everything one layer contributes to the sheet, in drawing coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Cut loops: counter-clockwise around material, clockwise around a hole.
    pub loops: Vec<Loop>,
    pub visible: Vec<Segment>,
    pub hidden: Vec<Segment>,
    pub hatch: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drawing {
    pub frame: Frame,
    /// The projection depth the drawing was generated with, after validation.
    pub depth: Option<f64>,
    pub layers: Vec<Layer>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Collects one diagnostic per element and code: a mesh with a million bad triangles must not
/// fill memory with a million identical lines.
pub(crate) struct Diagnostics {
    entries: Vec<Diagnostic>,
    seen: std::collections::BTreeSet<(u32, u32, &'static str)>,
}

/// Diagnostics one drawing may hold before the rest are dropped.
const MAX_DIAGNOSTICS: usize = 512;

impl Diagnostics {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: std::collections::BTreeSet::new(),
        }
    }

    fn add(&mut self, model_id: u32, element_id: u32, code: &'static str, message: String) {
        if self.entries.len() >= MAX_DIAGNOSTICS {
            return;
        }
        if self.seen.insert((model_id, element_id, code)) {
            self.entries.push(Diagnostic {
                model_id,
                element_id,
                code,
                message,
            });
        }
    }
}

/// Records what the projection had to skip.
fn note_projection(
    diagnostics: &mut Diagnostics,
    item: &DrawingMesh,
    report: &project::ProjectReport,
) {
    if report.non_finite > 0 {
        diagnostics.add(
            item.model_id,
            item.element_id,
            "non-finite-coordinate",
            "the projection skipped triangles with a coordinate that is not finite".into(),
        );
    }
    if report.too_large {
        diagnostics.add(
            item.model_id,
            item.element_id,
            "projection-skipped",
            "the element has too many triangles and is not projected".into(),
        );
    }
    if report.truncated {
        diagnostics.add(
            item.model_id,
            item.element_id,
            "projection-truncated",
            "the projection reached its work limit and stopped".into(),
        );
    }
}

pub fn draw(meshes: &[DrawingMesh], options: &DrawingOptions) -> Drawing {
    let mut diagnostics = Diagnostics::new();
    let Some(frame) = Frame::of(options.cut) else {
        diagnostics.add(0, 0, "bad-cut", "the cut position is not finite".into());
        return Drawing {
            frame: Frame::of(Cut::Plan { z: 0.0 }).expect("an identity frame is finite"),
            depth: None,
            layers: Vec::new(),
            diagnostics: diagnostics.entries,
        };
    };
    let depth = match options.depth {
        Some(depth) if !depth.is_finite() || depth < 0.0 => {
            diagnostics.add(0, 0, "bad-depth", format!("depth {depth} is not a length"));
            None
        }
        depth => depth,
    };

    // Sorted by name through a tree, so the sheet never depends on hash order.
    let mut layers: std::collections::BTreeMap<String, Layer> = std::collections::BTreeMap::new();

    let mut grid = options.include_projection.then(|| {
        let mut low = DVec2::splat(f64::MAX);
        let mut high = DVec2::splat(f64::MIN);
        for item in meshes {
            for point in &item.mesh.positions {
                if !point.is_finite() {
                    continue;
                }
                let flat = frame.project(*point);
                low = low.min(flat);
                high = high.max(flat);
            }
        }
        let span = if low.x <= high.x { (high - low).max_element() } else { 0.0 };
        project::Occluders::new(span)
    });
    if let Some(grid) = grid.as_mut() {
        for item in meshes {
            let report = project::add_occluders(&item.mesh, &frame, depth, grid);
            note_projection(&mut diagnostics, item, &report);
        }
    }
    let mut budget = project::Budget::new();

    for item in meshes {
        let layer = layers
            .entry(item.layer.clone())
            .or_insert_with(|| Layer {
                name: item.layer.clone(),
                ..Layer::default()
            });
        let mut loops = Vec::new();
        let report = cut::cut_mesh(&item.mesh, &frame, &mut loops);
        if report.non_finite > 0 {
            diagnostics.add(
                item.model_id,
                item.element_id,
                "non-finite-coordinate",
                "the cut skipped triangles with a coordinate that is not finite".into(),
            );
        }
        if report.truncated {
            diagnostics.add(
                item.model_id,
                item.element_id,
                "cut-truncated",
                "the cut reached its segment limit and stopped".into(),
            );
        }
        if report.open_loops > 0 {
            diagnostics.add(
                item.model_id,
                item.element_id,
                "open-cut-loop",
                "the cut could not close a loop, which a watertight mesh would".into(),
            );
        }
        if options.include_hatching {
            let report = hatch::hatch_loops(&loops, item.hatch, &item.layer, &mut layer.hatch);
            if report.bad_style {
                diagnostics.add(
                    item.model_id,
                    item.element_id,
                    "bad-hatch-style",
                    "the given hatch has no usable angle or spacing".into(),
                );
            }
            if report.truncated {
                diagnostics.add(
                    item.model_id,
                    item.element_id,
                    "hatch-truncated",
                    "the hatch reached its line limit and stopped".into(),
                );
            }
        }
        layer.loops.extend(loops);

        if let Some(grid) = grid.as_mut() {
            let report = project::project_mesh(
                &item.mesh,
                &frame,
                depth,
                grid,
                &mut budget,
                &mut layer.visible,
                &mut layer.hidden,
            );
            note_projection(&mut diagnostics, item, &report);
        }
    }

    if options.include_projection {
        for layer in layers.values_mut() {
            project::merge_lines(&mut layer.visible, &mut layer.hidden);
        }
    }

    // A layer nothing reached is still a layer: the caller asked for it.
    Drawing {
        frame,
        depth,
        layers: layers.into_values().collect(),
        diagnostics: diagnostics.entries,
    }
}

/// Shared by the exporters: nothing is written for a mesh whose layer has no geometry.
pub(crate) fn used_layers(drawing: &Drawing) -> impl Iterator<Item = &Layer> {
    drawing.layers.iter().filter(|layer| {
        !layer.loops.is_empty()
            || !layer.visible.is_empty()
            || !layer.hidden.is_empty()
            || !layer.hatch.is_empty()
    })
}
