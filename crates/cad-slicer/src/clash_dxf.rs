//! Clash cross-section DXF R12 writer, F5 round-trip metadata (`GUBIM_META` + named `BLOCK`/`INSERT`),
//! and non-overlapping master grid combiner (`merge_drawings_grid`, F3, `docs/12_clash_alignment_dxf.md` §3.4).

use crate::text::dxf_bytes;
use crate::{Drawing, DrawingMesh, Frame, Loop, Segment, draw_slab_unoccluded};
use glam::{DVec2, DVec3};
use std::collections::BTreeSet;

const MAX_VALUE: usize = 255;
const MAX_LAYER: usize = 31;

/// Round-trip metadata embedded in `999` comments and locked `GUBIM_META` layer for Phase F5.
#[derive(Debug, Clone, PartialEq)]
pub struct ClashDxfMeta {
    pub model_id: String,
    pub pk_m: f64,
    pub pk_label: String,
    pub frame: Frame,
    /// Original 2D lower-left corner subtracted when normalizing the drawing to `(0, 0)`.
    pub norm_offset: DVec2,
    pub s_min: f64,
    pub s_max: f64,
    pub annotations: Vec<String>,
    /// When the section was exported (ms since the Unix epoch, `GUBIM_META t=`). On re-import,
    /// edits made from other sections after this time are ones the drawing did not show.
    pub exported_at_ms: Option<f64>,
}

/// One highlighted Element A exported as a named `BLOCK` + `INSERT` (`GUBIM_A_<id>_<guid>`)
/// so CAD `MOVE` edits round-trip cleanly into IFC STEP patches in Phase F5.
#[derive(Debug, Clone, PartialEq)]
pub struct ClashBlockA {
    pub model_id: u32,
    pub express_id: u32,
    pub global_id: String,
    /// Session id of the model the element belongs to (`GUBIM_ORIG model=`), so a section
    /// holding elements of several federated models routes each edit to its own model. Empty
    /// when unknown; the reader then falls back to the section's `GUBIM_META model=`.
    pub owner: String,
    /// R12-safe block name (`<= 31` chars), e.g. `GUBIM_A_101_2O2Fr$t4X7Zf`.
    pub block_name: String,
    /// Insertion point in normalized `(0, 0)` sheet coordinates (metres).
    pub insert_point: DVec2,
    /// Cut loops relative to `insert_point`.
    pub loops: Vec<Loop>,
    /// Projected segments relative to `insert_point`.
    pub segments: Vec<Segment>,
    /// Optional editable 2D centerline polyline relative to `insert_point` (F5 Level 2).
    pub edit_polyline: Vec<DVec2>,
}

/// One clash marker in normalized `(0, 0)` sheet coordinates (metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClashMarker2D {
    pub position: DVec2,
    pub radius_m: f64,
}

/// Complete normalized `(0, 0)` cross-section sheet for one clash group, ready for individual
/// DXF export and zero-re-slice inclusion in `merge_drawings_grid`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClashSectionSheet {
    pub meta: ClashDxfMeta,
    /// Background model layers + `CLASH_B` layer, already normalized to `(0, 0)`.
    pub drawing: Drawing,
    /// Highlighted Element A blocks (`CLASH_A` + optional `EDIT_A_<id>`), normalized to `(0, 0)`.
    pub blocks_a: Vec<ClashBlockA>,
    /// Clash point markers (`CLASH_MARKER`), normalized to `(0, 0)`.
    pub markers: Vec<ClashMarker2D>,
    /// Optional horizontal X coordinate of the alignment axis in normalized sheet coordinates.
    pub alignment_axis_x: Option<f64>,
    /// Normalized bounding box dimensions `(width, height)` in metres (lower-left is `(0, 0)`).
    pub width_m: f64,
    pub height_m: f64,
}

/// Builds a normalized `(0, 0)` [`ClashSectionSheet`] from culled model meshes plus explicit
/// `clash_a_refs` `(model_id, express_id, global_id)` (exported as named `BLOCK`/`INSERT`s on
/// `CLASH_A`) and `clash_b_refs` `(model_id, express_id)` (placed on `CLASH_B`).
#[allow(clippy::too_many_arguments)]
pub fn build_clash_section_sheet(
    meshes: &[DrawingMesh],
    cut: crate::Cut,
    depth: f64,
    clash_a_refs: &[(u32, u32, String)],
    clash_b_refs: &[(u32, u32)],
    clash_points_3d: &[DVec3],
    alignment_point_3d: Option<DVec3>,
    model_id: impl Into<String>,
    pk_m: f64,
    pk_label: impl Into<String>,
    s_min: f64,
    s_max: f64,
    annotations: Vec<String>,
) -> ClashSectionSheet {
    let a_set: BTreeSet<(u32, u32)> = clash_a_refs.iter().map(|(m, id, _)| (*m, *id)).collect();
    let b_set: BTreeSet<(u32, u32)> = clash_b_refs.iter().copied().collect();
    let clean_depth = if depth.is_finite() && depth > 0.0 {
        depth
    } else {
        0.0
    };
    let pre_frame = Frame::of(cut);

    // Split meshes using composite (model_id, element_id) so federated models with colliding
    // expressIds never misclassify elements across models.
    let mut main_meshes: Vec<DrawingMesh> = Vec::with_capacity(meshes.len());
    let mut a_meshes_by_key: std::collections::BTreeMap<(u32, u32), Vec<DrawingMesh>> =
        std::collections::BTreeMap::new();

    for m in meshes {
        if let Some(f) = &pre_frame {
            let mut near = f64::INFINITY;
            let mut far = f64::NEG_INFINITY;
            for &pos in &m.mesh.positions {
                let d = f.depth(pos);
                if d.is_finite() {
                    near = near.min(d);
                    far = far.max(d);
                }
            }
            if near > clean_depth + 1e-6 || far < -1e-6 {
                continue;
            }
        }
        let key = (m.model_id, m.element_id);
        if a_set.contains(&key) {
            let mut copy = m.clone();
            copy.layer = "CLASH_A".to_string();
            a_meshes_by_key.entry(key).or_default().push(copy);
        } else if b_set.contains(&key) {
            let mut copy = m.clone();
            copy.layer = "CLASH_B".to_string();
            main_meshes.push(copy);
        } else {
            main_meshes.push(m.clone());
        }
    }

    let raw_drawing = draw_slab_unoccluded(&main_meshes, cut, depth);
    let frame = raw_drawing.frame;

    // Slice each Element A with the exact same cut + depth.
    let mut raw_blocks_a: Vec<(u32, u32, String, Drawing)> = Vec::new();
    for (m_id, express_id, global_id) in clash_a_refs {
        if let Some(elem_meshes) = a_meshes_by_key.get(&(*m_id, *express_id)) {
            let d = draw_slab_unoccluded(elem_meshes, cut, depth);
            raw_blocks_a.push((*m_id, *express_id, global_id.clone(), d));
        }
    }

    // Compute unified 2D bounding box across main drawing, Element A blocks, and clash markers.
    let mut low = DVec2::splat(f64::INFINITY);
    let mut high = DVec2::splat(f64::NEG_INFINITY);
    let mut take_bounds = |b: Option<(DVec2, DVec2)>| {
        if let Some((lo, hi)) = b {
            low = low.min(lo);
            high = high.max(hi);
        }
    };
    take_bounds(raw_drawing.bounds_2d());
    for (_, _, _, d) in &raw_blocks_a {
        take_bounds(d.bounds_2d());
    }
    let projected_markers: Vec<DVec2> = clash_points_3d
        .iter()
        .filter(|p| p.is_finite())
        .map(|&p| frame.project(p))
        .filter(|p| p.is_finite())
        .collect();
    for &p in &projected_markers {
        low = low.min(p);
        high = high.max(p);
    }

    let (norm_offset, width_m, height_m) = if low.x <= high.x && low.y <= high.y {
        (low, (high.x - low.x).max(0.0), (high.y - low.y).max(0.0))
    } else {
        (DVec2::ZERO, 0.0, 0.0)
    };

    let drawing = raw_drawing.translated(-norm_offset);

    let is_transverse_slab = depth.is_finite() && depth > 1e-6;
    let mut used_block_names = BTreeSet::new();
    let mut blocks_a = Vec::with_capacity(raw_blocks_a.len());
    for (m_id, express_id, global_id, d) in raw_blocks_a {
        let moved = d.translated(-norm_offset);
        let Some((blo, bhi)) = moved.bounds_2d() else {
            continue;
        };
        let insert_point = (blo + bhi) * 0.5;
        let local = moved.translated(-insert_point);
        let mut loops = Vec::new();
        let mut segments = Vec::new();
        for layer in local.layers {
            loops.extend(layer.loops);
            segments.extend(layer.visible);
        }
        let edit_polyline = if is_transverse_slab {
            let half = (bhi - blo) * 0.5;
            if half.x >= half.y && half.x > 1e-4 {
                vec![DVec2::new(-half.x, 0.0), DVec2::new(half.x, 0.0)]
            } else if half.y > 1e-4 {
                vec![DVec2::new(0.0, -half.y), DVec2::new(0.0, half.y)]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let block_name = make_block_name(express_id, &global_id, &mut used_block_names);
        blocks_a.push(ClashBlockA {
            model_id: m_id,
            express_id,
            global_id,
            owner: String::new(),
            block_name,
            insert_point,
            loops,
            segments,
            edit_polyline,
        });
    }

    let span = width_m.max(height_m).max(1.0);
    let marker_radius = (span * 0.015).clamp(0.02, 0.25);
    let markers = projected_markers
        .into_iter()
        .map(|p| ClashMarker2D {
            position: p - norm_offset,
            radius_m: marker_radius,
        })
        .collect();

    let alignment_axis_x = alignment_point_3d
        .filter(|p| p.is_finite())
        .map(|p| frame.project(p).x - norm_offset.x)
        .filter(|x| x.is_finite());

    ClashSectionSheet {
        meta: ClashDxfMeta {
            model_id: model_id.into(),
            pk_m,
            pk_label: pk_label.into(),
            frame,
            norm_offset,
            s_min,
            s_max,
            annotations,
            exported_at_ms: None,
        },
        drawing,
        blocks_a,
        markers,
        alignment_axis_x,
        width_m,
        height_m,
    }
}

/// Writes one normalized `(0, 0)` clash cross-section to valid DXF R12 bytes in metres.
pub fn to_clash_dxf(sheet: &ClashSectionSheet) -> Vec<u8> {
    let mut out = DxfWriter::default();
    let meta_lines = format_meta_lines(&sheet.meta);
    for line in &meta_lines {
        out.text(999, line);
    }

    let (ext_min, ext_max) = sheet_extents(sheet);
    out.header(ext_min, ext_max);

    // Collect layer names with ACI colors.
    let mut layer_defs: Vec<(String, u8, bool)> = Vec::new();
    let mut seen_layers = BTreeSet::new();
    let mut add_layer = |raw: &str, aci: u8, locked: bool| -> String {
        let name = clean_r12_name(raw, &mut seen_layers);
        layer_defs.push((name.clone(), aci, locked));
        name
    };

    let mut drawing_layers: Vec<(String, &crate::Layer)> = Vec::new();
    for layer in crate::used_layers(&sheet.drawing) {
        let aci = if layer.name == "CLASH_B" { 4 } else { 7 };
        let name = add_layer(&layer.name, aci, false);
        drawing_layers.push((name, layer));
    }

    let clash_a_layer = (!sheet.blocks_a.is_empty()).then(|| add_layer("CLASH_A", 1, false));
    let mut edit_layers: Vec<Option<String>> = Vec::with_capacity(sheet.blocks_a.len());
    for block in &sheet.blocks_a {
        if block.edit_polyline.len() >= 2 {
            edit_layers.push(Some(add_layer(&format!("EDIT_A_{}", block.express_id), 1, false)));
        } else {
            edit_layers.push(None);
        }
    }
    let marker_layer = (!sheet.markers.is_empty()).then(|| add_layer("CLASH_MARKER", 2, false));
    let axis_layer = sheet
        .alignment_axis_x
        .map(|_| add_layer("ALIGNMENT_AXIS", 3, false));
    let annot_layer = (!sheet.meta.annotations.is_empty()).then(|| add_layer("ANNOTATION", 7, false));
    let meta_layer = add_layer("GUBIM_META", 8, true);

    out.layer_table(&layer_defs);

    // BLOCKS section for Element A round-trip blocks.
    if let Some(layer_a) = &clash_a_layer {
        let layer_a_bytes = dxf_bytes(layer_a, MAX_VALUE);
        out.section("BLOCKS");
        for block in &sheet.blocks_a {
            out.write_block(&block.block_name, &layer_a_bytes, &block.loops, &block.segments);
        }
        out.end_section();
    }

    // ENTITIES section.
    out.section("ENTITIES");
    for (name, layer) in &drawing_layers {
        let lb = dxf_bytes(name, MAX_VALUE);
        write_layer_geometry(&mut out, &lb, layer, DVec2::ZERO);
    }

    if let Some(layer_a) = &clash_a_layer {
        let lb = dxf_bytes(layer_a, MAX_VALUE);
        for (block, edit_layer) in sheet.blocks_a.iter().zip(&edit_layers) {
            out.insert(&lb, &block.block_name, block.insert_point);
            if let Some(el_name) = edit_layer {
                let elb = dxf_bytes(el_name, MAX_VALUE);
                out.polyline_2d(&elb, &block.edit_polyline, block.insert_point);
            }
        }
    }

    if let Some(ml) = &marker_layer {
        let mlb = dxf_bytes(ml, MAX_VALUE);
        for m in &sheet.markers {
            out.marker(&mlb, m.position, m.radius_m);
        }
    }

    if let (Some(al), Some(axis_x)) = (&axis_layer, sheet.alignment_axis_x) {
        let alb = dxf_bytes(al, MAX_VALUE);
        let pad = (sheet.height_m * 0.08).clamp(0.1, 1.0);
        out.line_bytes(
            &alb,
            DVec2::new(axis_x, -pad),
            DVec2::new(axis_x, sheet.height_m + pad),
        );
    }

    if let Some(an) = &annot_layer {
        let anb = dxf_bytes(an, MAX_VALUE);
        let text_h = (sheet.width_m.max(sheet.height_m).max(1.0) * 0.025).clamp(0.05, 0.5);
        for (i, line) in sheet.meta.annotations.iter().enumerate() {
            let y = sheet.height_m + text_h * (1.5 + (sheet.meta.annotations.len() - 1 - i) as f64 * 1.4);
            out.left_text(&anb, DVec2::new(0.0, y), text_h, line);
        }
    }

    // Locked GUBIM_META text entities for CAD tools that strip 999 comments on save.
    let meta_b = dxf_bytes(&meta_layer, MAX_VALUE);
    // Every value stays within the R12 255-character limit: long records are split into
    // several entities that the reader joins again.
    let mut meta_row = 0usize;
    let mut meta_text = |out: &mut DxfWriter, line: &str| {
        meta_row += 1;
        out.left_text(&meta_b, DVec2::new(0.0, -0.001 * meta_row as f64), 0.001, line);
    };
    for line in &meta_lines {
        meta_text(&mut out, line);
    }
    for (block, edit_layer) in sheet.blocks_a.iter().zip(&edit_layers) {
        // Each record names its block (unique in the file) and its edit layer, so the reader
        // pairs an INSERT or an edit polyline with its own element even when two federated
        // models share an expressId.
        let mut tokens = vec![format!("id={}", block.express_id)];
        let guid = meta_token(&block.global_id);
        if !guid.is_empty() {
            tokens.push(format!("guid={guid}"));
        }
        tokens.push(format!("block={}", block.block_name));
        if let Some(layer) = edit_layer {
            tokens.push(format!("layer={layer}"));
        }
        tokens.push(format!("x={:.6}", block.insert_point.x));
        tokens.push(format!("y={:.6}", block.insert_point.y));
        let owner = meta_token(&block.owner);
        if !owner.is_empty() {
            let owner_token = format!("model={owner}");
            let len: usize = "GUBIM_ORIG".len() + tokens.iter().map(|t| t.len() + 1).sum::<usize>();
            if len + 1 + owner_token.len() <= META_LINE_BUDGET {
                tokens.insert(1, owner_token);
            }
        }
        let orig_line = format!("GUBIM_ORIG {}", tokens.join(" "));
        meta_text(&mut out, &orig_line);
        if block.edit_polyline.len() >= 2 {
            let points: Vec<String> = block
                .edit_polyline
                .iter()
                .map(|p| {
                    let pt = *p + block.insert_point;
                    format!("{:.6},{:.6}", pt.x, pt.y)
                })
                .collect();
            let mut part = 0usize;
            let mut current: Vec<&str> = Vec::new();
            let prefix_len = format!(
                "GUBIM_POLY id={} block={} part=9999 pts=",
                block.express_id, block.block_name
            )
            .len();
            let mut flush = |out: &mut DxfWriter, current: &mut Vec<&str>, part: &mut usize| {
                if current.is_empty() {
                    return;
                }
                let line = format!(
                    "GUBIM_POLY id={} block={} part={} pts={}",
                    block.express_id,
                    block.block_name,
                    part,
                    current.join(";")
                );
                meta_text(out, &line);
                *part += 1;
                current.clear();
            };
            let mut len = prefix_len;
            for p in &points {
                if !current.is_empty() && len + 1 + p.len() > META_LINE_BUDGET {
                    flush(&mut out, &mut current, &mut part);
                    len = prefix_len;
                }
                len += p.len() + 1;
                current.push(p);
            }
            flush(&mut out, &mut current, &mut part);
        }
    }

    out.end_section();
    out.group(0, b"EOF");
    out.bytes
}

/// Options for [`merge_drawings_grid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridMergeOptions {
    /// Fixed number of columns, or `None` for automatic `ceil(sqrt(n))`.
    pub columns: Option<usize>,
    /// Minimum horizontal/vertical gap between adjacent sheet frames (metres).
    pub margin_m: f64,
    /// Padding inside each `SHEET_FRAME` rectangle around the drawing (metres).
    pub frame_padding_m: f64,
}

impl Default for GridMergeOptions {
    fn default() -> Self {
        Self {
            columns: None,
            margin_m: 1.0,
            frame_padding_m: 0.35,
        }
    }
}

/// Placement of one section inside the combined master DXF (`clashes_all.dxf`).
#[derive(Debug, Clone, PartialEq)]
pub struct GridLayoutCell {
    pub index: usize,
    pub pk_m: f64,
    pub pk_label: String,
    /// Outer frame box `[min, max]` in master DXF metres. Guaranteed disjoint across all cells.
    pub frame_min: DVec2,
    pub frame_max: DVec2,
    /// Translation applied to the normalized `(0, 0)` sheet geometry.
    pub content_origin: DVec2,
}

/// Combined master DXF bytes plus exact non-overlapping cell coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct GridMergeResult {
    pub dxf_bytes: Vec<u8>,
    pub cells: Vec<GridLayoutCell>,
}

/// Arranges already-sliced, `(0, 0)`-normalized [`ClashSectionSheet`] items into a single
/// non-overlapping 2D grid DXF (`clashes_all.dxf`), sorted by PK, without re-slicing.
///
/// Builds the whole file in one buffer: fine for tests and small batches. Large batches (a
/// bridge with hundreds of sections reaches gigabytes) go through [`MasterGridWriter`], which
/// hands the same bytes out in bounded chunks so no single buffer outgrows wasm32's memory.
pub fn merge_drawings_grid(
    sheets: &[ClashSectionSheet],
    options: &GridMergeOptions,
) -> GridMergeResult {
    let mut writer = MasterGridWriter::new(sheets, options);
    let mut dxf_bytes = Vec::new();
    while let Some(chunk) = writer.next_chunk(sheets, usize::MAX) {
        dxf_bytes.extend_from_slice(&chunk);
    }
    GridMergeResult {
        dxf_bytes,
        cells: writer.cells,
    }
}

/// Streams the master grid DXF (`clashes_all.dxf`) in bounded chunks: the header and layer
/// table first, then whole cells in PK order until a chunk reaches its target size, then the
/// end of the file. Concatenating every chunk gives exactly [`merge_drawings_grid`]'s bytes.
///
/// Every cell keeps all of its layers, however large the file grows: the caller writes each
/// chunk out (into a ZIP entry, a file) before asking for the next, so memory holds one chunk
/// at a time rather than the whole master grid.
///
/// `sheets` must be the same slice, unchanged, on every call; once [`Self::cells_written`]
/// passes a cell, its sheet's geometry is no longer read and may be freed.
pub struct MasterGridWriter {
    cells: Vec<GridLayoutCell>,
    header: Option<Vec<u8>>,
    layer_map: std::collections::BTreeMap<String, String>,
    frame_b: Vec<u8>,
    a_b: Vec<u8>,
    b_b: Vec<u8>,
    m_b: Vec<u8>,
    ax_b: Vec<u8>,
    an_b: Vec<u8>,
    pad: f64,
    next_cell: usize,
    finished: bool,
}

fn grid_header_height(w: f64, h: f64) -> f64 {
    (w.max(h).max(1.0) * 0.06).clamp(0.15, 1.2)
}

impl MasterGridWriter {
    /// Lays out the grid and prepares the file header; writes no cell yet.
    pub fn new(sheets: &[ClashSectionSheet], options: &GridMergeOptions) -> Self {
        let pad = options.frame_padding_m.max(0.05);
        if sheets.is_empty() {
            let mut out = DxfWriter::default();
            out.header(DVec2::ZERO, DVec2::ZERO);
            out.layer_table(&[("SHEET_FRAME".to_string(), 8, false)]);
            out.section("ENTITIES");
            return Self {
                cells: Vec::new(),
                header: Some(out.bytes),
                layer_map: std::collections::BTreeMap::new(),
                frame_b: Vec::new(),
                a_b: Vec::new(),
                b_b: Vec::new(),
                m_b: Vec::new(),
                ax_b: Vec::new(),
                an_b: Vec::new(),
                pad,
                next_cell: 0,
                finished: false,
            };
        }

        let mut order: Vec<usize> = (0..sheets.len()).collect();
        order.sort_by(|&a, &b| {
            sheets[a]
                .meta
                .pk_m
                .total_cmp(&sheets[b].meta.pk_m)
                .then_with(|| a.cmp(&b))
        });

        let n = order.len();
        let cols = options
            .columns
            .filter(|&c| c > 0)
            .unwrap_or_else(|| (n as f64).sqrt().ceil() as usize)
            .max(1);
        let rows = n.div_ceil(cols);
        let margin = options.margin_m.max(0.1);

        // Determine uniform column widths and row heights so every cell frame is strictly disjoint.
        let mut col_widths = vec![1.0_f64; cols];
        let mut row_heights = vec![1.0_f64; rows];

        for (slot, &sheet_idx) in order.iter().enumerate() {
            let r = slot / cols;
            let c = slot % cols;
            let s = &sheets[sheet_idx];
            let hh = grid_header_height(s.width_m, s.height_m);
            let cell_w = s.width_m.max(0.5) + 2.0 * pad;
            let cell_h = s.height_m.max(0.5) + 2.0 * pad + hh;
            col_widths[c] = col_widths[c].max(cell_w);
            row_heights[r] = row_heights[r].max(cell_h);
        }

        let mut col_x = vec![0.0_f64; cols];
        for c in 1..cols {
            col_x[c] = col_x[c - 1] + col_widths[c - 1] + margin;
        }
        // Lay out rows top-to-bottom (row 0 at top, descending Y) so reading order matches left-to-right, top-to-bottom.
        let total_height: f64 =
            row_heights.iter().sum::<f64>() + margin * rows.saturating_sub(1) as f64;
        let mut row_y = vec![0.0_f64; rows];
        let mut cursor_y = total_height;
        for r in 0..rows {
            cursor_y -= row_heights[r];
            row_y[r] = cursor_y;
            cursor_y -= margin;
        }

        let mut cells = Vec::with_capacity(n);
        for (slot, &sheet_idx) in order.iter().enumerate() {
            let r = slot / cols;
            let c = slot % cols;
            let s = &sheets[sheet_idx];
            let fmin = DVec2::new(col_x[c], row_y[r]);
            let fmax = DVec2::new(col_x[c] + col_widths[c], row_y[r] + row_heights[r]);
            let content_origin = DVec2::new(fmin.x + pad, fmin.y + pad);
            cells.push(GridLayoutCell {
                index: sheet_idx,
                pk_m: s.meta.pk_m,
                pk_label: s.meta.pk_label.clone(),
                frame_min: fmin,
                frame_max: fmax,
                content_origin,
            });
        }

        // Collect union of all layers across all sheets.
        let mut seen_layers = BTreeSet::new();
        let mut layer_defs: Vec<(String, u8, bool)> = Vec::new();
        let mut layer_map: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let mut ensure_layer = |raw: &str, aci: u8| -> String {
            if let Some(existing) = layer_map.get(raw) {
                return existing.clone();
            }
            let assigned = clean_r12_name(raw, &mut seen_layers);
            layer_defs.push((assigned.clone(), aci, false));
            layer_map.insert(raw.to_string(), assigned.clone());
            assigned
        };

        let frame_layer = ensure_layer("SHEET_FRAME", 8);
        let clash_a_layer = ensure_layer("CLASH_A", 1);
        let clash_b_layer = ensure_layer("CLASH_B", 4);
        let marker_layer = ensure_layer("CLASH_MARKER", 2);
        let axis_layer = ensure_layer("ALIGNMENT_AXIS", 3);
        let annot_layer = ensure_layer("ANNOTATION", 7);

        for s in sheets {
            for layer in crate::used_layers(&s.drawing) {
                let aci = if layer.name == "CLASH_B" { 4 } else { 7 };
                ensure_layer(&layer.name, aci);
            }
        }

        let total_width = col_x[cols - 1] + col_widths[cols - 1];
        let mut out = DxfWriter::default();
        out.text(999, &format!("GUBIM_MASTER_GRID sections={n} units=m"));
        out.header(DVec2::ZERO, DVec2::new(total_width, total_height));
        out.layer_table(&layer_defs);
        out.section("ENTITIES");

        Self {
            cells,
            header: Some(out.bytes),
            layer_map,
            frame_b: dxf_bytes(&frame_layer, MAX_VALUE),
            a_b: dxf_bytes(&clash_a_layer, MAX_VALUE),
            b_b: dxf_bytes(&clash_b_layer, MAX_VALUE),
            m_b: dxf_bytes(&marker_layer, MAX_VALUE),
            ax_b: dxf_bytes(&axis_layer, MAX_VALUE),
            an_b: dxf_bytes(&annot_layer, MAX_VALUE),
            pad,
            next_cell: 0,
            finished: false,
        }
    }

    /// Cell placements, in PK order (the order they are written).
    pub fn cells(&self) -> &[GridLayoutCell] {
        &self.cells
    }

    /// How many cells, from the front of [`Self::cells`], are already written.
    pub fn cells_written(&self) -> usize {
        self.next_cell
    }

    /// The next piece of the file, or `None` once the end of file went out. A chunk holds whole
    /// cells and stops at the first cell boundary at or past `target_bytes`, so one very large
    /// cell makes a chunk larger than the target, never a cell split across two.
    pub fn next_chunk(&mut self, sheets: &[ClashSectionSheet], target_bytes: usize) -> Option<Vec<u8>> {
        if self.finished {
            return None;
        }
        let mut out = DxfWriter {
            bytes: self.header.take().unwrap_or_default(),
        };
        while self.next_cell < self.cells.len() && out.bytes.len() < target_bytes {
            self.write_cell(&mut out, sheets);
            self.next_cell += 1;
        }
        if self.next_cell >= self.cells.len() {
            out.end_section();
            out.group(0, b"EOF");
            self.finished = true;
        }
        Some(out.bytes)
    }

    fn write_cell(&self, out: &mut DxfWriter, sheets: &[ClashSectionSheet]) {
        let cell = &self.cells[self.next_cell];
        let Some(s) = sheets.get(cell.index) else {
            return;
        };
        let off = cell.content_origin;
        let pad = self.pad;

        // Cell frame border & PK title on SHEET_FRAME.
        out.rect(&self.frame_b, cell.frame_min, cell.frame_max);
        let hh = grid_header_height(s.width_m, s.height_m);
        let title_pos = DVec2::new(cell.frame_min.x + pad, cell.frame_max.y - hh * 0.75);
        out.left_text(
            &self.an_b,
            title_pos,
            (hh * 0.5).clamp(0.08, 0.5),
            &format!("PK {} (depth {:.3} m)", s.meta.pk_label, (s.meta.s_max - s.meta.s_min).max(0.0)),
        );

        // Background + CLASH_B layers.
        for layer in crate::used_layers(&s.drawing) {
            if layer.name == "CLASH_B" {
                write_layer_geometry(out, &self.b_b, layer, off);
            } else {
                let lb = dxf_bytes(self.layer_map.get(&layer.name).unwrap_or(&layer.name), MAX_VALUE);
                write_layer_geometry(out, &lb, layer, off);
            }
        }

        // Element A geometry on CLASH_A (inlined in master grid to avoid block name collisions).
        for block in &s.blocks_a {
            let b_off = off + block.insert_point;
            for ring in &block.loops {
                for (from, to) in ring.segments() {
                    out.line_bytes(&self.a_b, from + b_off, to + b_off);
                }
            }
            for seg in &block.segments {
                out.line_bytes(&self.a_b, seg.a + b_off, seg.b + b_off);
            }
        }

        // Markers + Alignment axis.
        for marker in &s.markers {
            out.marker(&self.m_b, marker.position + off, marker.radius_m);
        }
        if let Some(axis_x) = s.alignment_axis_x {
            out.line_bytes(
                &self.ax_b,
                DVec2::new(off.x + axis_x, cell.frame_min.y + pad * 0.5),
                DVec2::new(off.x + axis_x, off.y + s.height_m + pad * 0.5),
            );
        }
    }
}

fn make_block_name(express_id: u32, global_id: &str, used: &mut BTreeSet<String>) -> String {
    let raw = if global_id.is_empty() {
        format!("GUBIM_A_{express_id}")
    } else {
        format!("GUBIM_A_{express_id}_{global_id}")
    };
    clean_r12_name(&raw, used)
}

/// Longest metadata value written, safely under the DXF R12 limit of 255 characters.
const META_LINE_BUDGET: usize = 240;

/// A metadata value without whitespace or `=`, so `key=value` tokens split back cleanly.
fn meta_token(raw: &str) -> String {
    raw.chars()
        .take(120)
        .map(|c| if c.is_ascii_graphic() && c != '=' { c } else { '_' })
        .collect()
}

/// `GUBIM_META v=2 ...` records, each at most [`META_LINE_BUDGET`] characters. The reader merges
/// the `key=value` tokens of every `GUBIM_META` record in the file.
fn format_meta_lines(meta: &ClashDxfMeta) -> Vec<String> {
    let f = &meta.frame;
    let mut tokens = vec![
        "v=2".to_string(),
        format!("model={}", meta_token(&meta.model_id)),
        format!("pk={:.6}", meta.pk_m),
        format!("sMin={:.6}", meta.s_min),
        format!("sMax={:.6}", meta.s_max),
    ];
    if let Some(t) = meta.exported_at_ms.filter(|t| t.is_finite() && *t >= 0.0) {
        tokens.push(format!("t={t:.0}"));
    }
    for (key, value) in [
        ("ox", f.origin.x),
        ("oy", f.origin.y),
        ("oz", f.origin.z),
        ("rx", f.right.x),
        ("ry", f.right.y),
        ("rz", f.right.z),
        ("ux", f.up.x),
        ("uy", f.up.y),
        ("uz", f.up.z),
        ("lx", f.look.x),
        ("ly", f.look.y),
        ("lz", f.look.z),
        ("nx", meta.norm_offset.x),
        ("ny", meta.norm_offset.y),
    ] {
        tokens.push(format!("{key}={value:.6}"));
    }
    let mut lines = Vec::new();
    let mut line = String::from("GUBIM_META");
    for token in tokens {
        if line.len() + 1 + token.len() > META_LINE_BUDGET && line != "GUBIM_META" {
            lines.push(std::mem::replace(&mut line, String::from("GUBIM_META")));
        }
        line.push(' ');
        line.push_str(&token);
    }
    lines.push(line);
    lines
}

fn sheet_extents(sheet: &ClashSectionSheet) -> (DVec2, DVec2) {
    (DVec2::ZERO, DVec2::new(sheet.width_m, sheet.height_m))
}

fn write_layer_geometry(out: &mut DxfWriter, layer_bytes: &[u8], layer: &crate::Layer, offset: DVec2) {
    for ring in &layer.loops {
        for (from, to) in ring.segments() {
            out.line_bytes(layer_bytes, from + offset, to + offset);
        }
    }
    for seg in &layer.visible {
        out.line_bytes(layer_bytes, seg.a + offset, seg.b + offset);
    }
}

fn clean_r12_name(raw: &str, used: &mut BTreeSet<String>) -> String {
    let mut base = String::new();
    for ch in raw.chars() {
        if base.len() >= MAX_LAYER {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '$' | '_' | '-' | '.') {
            base.push(ch);
        } else {
            base.push('_');
        }
    }
    if base.is_empty() {
        base.push_str("LAYER");
    }
    if used.insert(base.to_lowercase()) {
        return base;
    }
    for suffix in 1..=9999 {
        let s = format!("_{suffix}");
        let keep = MAX_LAYER.saturating_sub(s.len());
        let mut cand: String = base.chars().take(keep).collect();
        cand.push_str(&s);
        if used.insert(cand.to_lowercase()) {
            return cand;
        }
    }
    "LAYER".to_string()
}

#[derive(Default)]
struct DxfWriter {
    bytes: Vec<u8>,
}

impl DxfWriter {
    fn group(&mut self, code: i16, value: &[u8]) {
        self.bytes.extend_from_slice(code.to_string().as_bytes());
        self.bytes.extend_from_slice(b"\r\n");
        self.bytes.extend_from_slice(value);
        self.bytes.extend_from_slice(b"\r\n");
    }

    fn text(&mut self, code: i16, value: &str) {
        let bytes = dxf_bytes(value, MAX_VALUE);
        self.group(code, &bytes);
    }

    fn number(&mut self, code: i16, value: f64) {
        self.bytes.extend_from_slice(code.to_string().as_bytes());
        self.bytes.extend_from_slice(b"\r\n");
        if value.is_finite() {
            crate::fixed::push_fixed_clean_bytes(&mut self.bytes, value, 6);
        } else {
            self.bytes.extend_from_slice(b"0.000000");
        }
        self.bytes.extend_from_slice(b"\r\n");
    }

    fn integer(&mut self, code: i16, value: i64) {
        let s = value.to_string();
        self.group(code, s.as_bytes());
    }

    fn section(&mut self, name: &str) {
        self.group(0, b"SECTION");
        self.text(2, name);
    }

    fn end_section(&mut self) {
        self.group(0, b"ENDSEC");
    }

    fn header(&mut self, low: DVec2, high: DVec2) {
        self.section("HEADER");
        self.text(9, "$ACADVER");
        self.text(1, "AC1009");
        self.text(9, "$DWGCODEPAGE");
        self.text(3, "ANSI_1252");
        // $INSUNITS = 6 (Metres)
        self.text(9, "$INSUNITS");
        self.integer(70, 6);
        self.text(9, "$EXTMIN");
        self.number(10, low.x);
        self.number(20, low.y);
        self.number(30, 0.0);
        self.text(9, "$EXTMAX");
        self.number(10, high.x);
        self.number(20, high.y);
        self.number(30, 0.0);
        self.end_section();
    }

    fn layer_table(&mut self, layers: &[(String, u8, bool)]) {
        self.section("TABLES");
        self.group(0, b"TABLE");
        self.text(2, "LTYPE");
        self.integer(70, 1);
        self.group(0, b"LTYPE");
        self.text(2, "CONTINUOUS");
        self.integer(70, 0);
        self.text(3, "Solid line");
        self.integer(72, 65);
        self.integer(73, 0);
        self.number(40, 0.0);
        self.group(0, b"ENDTAB");

        self.group(0, b"TABLE");
        self.text(2, "LAYER");
        self.integer(70, layers.len() as i64);
        for (name, aci, locked) in layers {
            self.group(0, b"LAYER");
            self.text(2, name);
            self.integer(70, if *locked { 4 } else { 0 });
            self.integer(62, *aci as i64);
            self.text(6, "CONTINUOUS");
        }
        self.group(0, b"ENDTAB");
        self.end_section();
    }

    fn write_block(&mut self, name: &str, layer: &[u8], loops: &[Loop], segments: &[Segment]) {
        self.group(0, b"BLOCK");
        self.group(8, layer);
        self.text(2, name);
        self.integer(70, 0);
        self.number(10, 0.0);
        self.number(20, 0.0);
        self.number(30, 0.0);
        self.text(3, name);
        for ring in loops {
            for (from, to) in ring.segments() {
                self.line_bytes(layer, from, to);
            }
        }
        for seg in segments {
            self.line_bytes(layer, seg.a, seg.b);
        }
        self.group(0, b"ENDBLK");
        self.group(8, layer);
    }

    fn insert(&mut self, layer: &[u8], block_name: &str, pos: DVec2) {
        self.group(0, b"INSERT");
        self.group(8, layer);
        self.text(2, block_name);
        self.number(10, pos.x);
        self.number(20, pos.y);
        self.number(30, 0.0);
    }

    fn polyline_2d(&mut self, layer: &[u8], points: &[DVec2], offset: DVec2) {
        self.group(0, b"POLYLINE");
        self.group(8, layer);
        self.integer(66, 1);
        self.number(10, 0.0);
        self.number(20, 0.0);
        self.number(30, 0.0);
        for &p in points {
            let pt = p + offset;
            self.group(0, b"VERTEX");
            self.group(8, layer);
            self.number(10, pt.x);
            self.number(20, pt.y);
            self.number(30, 0.0);
        }
        self.group(0, b"SEQEND");
        self.group(8, layer);
    }

    fn marker(&mut self, layer: &[u8], pos: DVec2, r: f64) {
        self.group(0, b"CIRCLE");
        self.group(8, layer);
        self.number(10, pos.x);
        self.number(20, pos.y);
        self.number(30, 0.0);
        self.number(40, r);
        self.line_bytes(layer, DVec2::new(pos.x - r, pos.y), DVec2::new(pos.x + r, pos.y));
        self.line_bytes(layer, DVec2::new(pos.x, pos.y - r), DVec2::new(pos.x, pos.y + r));
    }

    fn rect(&mut self, layer: &[u8], min: DVec2, max: DVec2) {
        let p0 = min;
        let p1 = DVec2::new(max.x, min.y);
        let p2 = max;
        let p3 = DVec2::new(min.x, max.y);
        self.line_bytes(layer, p0, p1);
        self.line_bytes(layer, p1, p2);
        self.line_bytes(layer, p2, p3);
        self.line_bytes(layer, p3, p0);
    }

    fn line_bytes(&mut self, layer: &[u8], from: DVec2, to: DVec2) {
        self.bytes.extend_from_slice(b"0\r\nLINE\r\n8\r\n");
        self.bytes.extend_from_slice(layer);
        self.bytes.extend_from_slice(b"\r\n10\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, from.x, 6);
        self.bytes.extend_from_slice(b"\r\n20\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, from.y, 6);
        self.bytes.extend_from_slice(b"\r\n30\r\n0.000000\r\n11\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, to.x, 6);
        self.bytes.extend_from_slice(b"\r\n21\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, to.y, 6);
        self.bytes.extend_from_slice(b"\r\n31\r\n0.000000\r\n");
    }

    fn left_text(&mut self, layer: &[u8], pos: DVec2, height: f64, value: &str) {
        self.bytes.extend_from_slice(b"0\r\nTEXT\r\n8\r\n");
        self.bytes.extend_from_slice(layer);
        self.bytes.extend_from_slice(b"\r\n10\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, pos.x, 6);
        self.bytes.extend_from_slice(b"\r\n20\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, pos.y, 6);
        self.bytes.extend_from_slice(b"\r\n30\r\n0.000000\r\n40\r\n");
        crate::fixed::push_fixed_clean_bytes(&mut self.bytes, height, 6);
        self.bytes.extend_from_slice(b"\r\n1\r\n");
        self.bytes.extend_from_slice(&dxf_bytes(value, MAX_VALUE));
        self.bytes.extend_from_slice(b"\r\n");
    }
}
