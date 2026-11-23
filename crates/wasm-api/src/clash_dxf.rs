//! Sequential clash cross-section batch slicer and master DXF grid combiner for WASM (F3/F4).

use crate::drawing::decode_element_chunk;
use cad_slicer::{
    ClashSectionSheet, Cut, DrawingMesh, GridMergeOptions, Layer, SvgOptions,
    build_clash_section_sheet, merge_drawings_grid, to_clash_dxf, to_svg_with,
};
use glam::DVec3;

/// Accumulates culled 3D meshes for one clash section at a time, produces its individual DXF R12
/// bytes + instant SVG preview, drops the 3D meshes immediately, and retains only the compact 2D
/// `ClashSectionSheet` for zero-re-slice `merge_master_dxf`.
#[derive(Default)]
pub struct ClashBatchStore {
    current_meshes: Vec<DrawingMesh>,
    sheets: Vec<ClashSectionSheet>,
}

pub struct ClashSectionResult {
    pub dxf_bytes: Vec<u8>,
    pub svg: String,
    pub width_m: f64,
    pub height_m: f64,
}

impl ClashBatchStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_section_chunk(&mut self, model_index: u32, chunk: &[u8]) -> bool {
        let Some(meshes) = decode_element_chunk(model_index, chunk) else {
            return false;
        };
        self.current_meshes.extend(meshes);
        true
    }

    pub fn clear_current_section(&mut self) {
        self.current_meshes.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_section(
        &mut self,
        origin: [f64; 3],
        normal: [f64; 3],
        depth_m: f64,
        clash_a_refs: Vec<(u32, u32, String)>,
        clash_b_refs: Vec<(u32, u32)>,
        clash_points_3d: Vec<[f64; 3]>,
        alignment_point_3d: Option<[f64; 3]>,
        model_id: String,
        pk_m: f64,
        pk_label: String,
        s_min: f64,
        s_max: f64,
        annotations: Vec<String>,
    ) -> ClashSectionResult {
        let meshes = std::mem::take(&mut self.current_meshes);
        let cut = Cut::Plane {
            origin: DVec3::from_array(origin),
            normal: DVec3::from_array(normal),
        };
        let pts: Vec<DVec3> = clash_points_3d.into_iter().map(DVec3::from_array).collect();
        let align_pt = alignment_point_3d.map(DVec3::from_array);

        let sheet = build_clash_section_sheet(
            &meshes,
            cut,
            depth_m,
            &clash_a_refs,
            &clash_b_refs,
            &pts,
            align_pt,
            model_id,
            pk_m,
            pk_label,
            s_min,
            s_max,
            annotations,
        );
        drop(meshes);

        let dxf_bytes = to_clash_dxf(&sheet);
        let svg = preview_svg(&sheet);
        let width_m = sheet.width_m;
        let height_m = sheet.height_m;
        self.sheets.push(sheet);

        ClashSectionResult {
            dxf_bytes,
            svg,
            width_m,
            height_m,
        }
    }

    pub fn merge_master_dxf(&self, columns: Option<usize>, margin_m: f64) -> Vec<u8> {
        let opts = GridMergeOptions {
            columns,
            margin_m: if margin_m.is_finite() && margin_m > 0.0 {
                margin_m
            } else {
                1.0
            },
            ..GridMergeOptions::default()
        };
        merge_drawings_grid(&self.sheets, &opts).dxf_bytes
    }

    pub fn section_count(&self) -> usize {
        self.sheets.len()
    }

    pub fn clear(&mut self) {
        self.current_meshes.clear();
        self.sheets.clear();
    }
}

fn preview_svg(sheet: &ClashSectionSheet) -> String {
    let mut preview = sheet.drawing.clone();
    if !sheet.blocks_a.is_empty() {
        let mut layer_a = Layer {
            name: "CLASH_A".to_string(),
            ..Layer::default()
        };
        for block in &sheet.blocks_a {
            let off = block.insert_point;
            for ring in &block.loops {
                let mut shifted = ring.clone();
                for p in &mut shifted.points {
                    *p += off;
                }
                layer_a.loops.push(shifted);
            }
            for seg in &block.segments {
                layer_a.visible.push(cad_slicer::Segment::new(seg.a + off, seg.b + off));
            }
        }
        preview.layers.push(layer_a);
    }
    to_svg_with(
        &preview,
        &SvgOptions {
            include_hidden: false,
            include_hatch: false,
            ..SvgOptions::default()
        },
    )
}

/// Parses an edited clash DXF, converts all Level 1 (`Move`) and Level 2 (`Rebend`) edits from
/// metres into the IFC file's native length units (`1.0 / length_unit_scale`), applies surgical
/// STEP patches to `original_ifc`, and returns `(patched_ifc_bytes, diff_report_json)` (F5).
pub fn apply_clash_dxf_to_step(
    original_ifc: &[u8],
    length_unit_scale: f64,
    dxf_bytes: &[u8],
) -> Result<(Vec<u8>, String), String> {
    let diff = cad_slicer::parse_clash_dxf_edits(dxf_bytes)?;
    let inv_scale = if length_unit_scale.is_finite() && length_unit_scale > 0.0 {
        1.0 / length_unit_scale
    } else {
        1.0
    };

    let mut patches = Vec::with_capacity(diff.edits.len());
    for edit in &diff.edits {
        match edit {
            cad_slicer::DxfRoundTripEdit::Move {
                express_id,
                delta_3d,
                ..
            } => {
                patches.push(step_parser::StepPatch::MovePlacement {
                    express_id: *express_id,
                    delta_file_units: (*delta_3d * inv_scale).to_array(),
                });
            }
            cad_slicer::DxfRoundTripEdit::Rebend {
                express_id,
                points_3d,
            } => {
                let pts_file: Vec<[f64; 3]> = points_3d
                    .iter()
                    .map(|p| (*p * inv_scale).to_array())
                    .collect();
                patches.push(step_parser::StepPatch::RebendSweptDisk {
                    express_id: *express_id,
                    points_file_units: pts_file,
                    radius_file_units: 0.0,
                });
            }
        }
    }

    let (patched_bytes, report) = step_parser::patch_step_file(original_ifc, &patches)?;
    let json = format_roundtrip_json(&diff, report.applied, report.skipped);
    Ok((patched_bytes, json))
}

pub fn format_roundtrip_json(
    diff: &cad_slicer::DxfRoundTripDiff,
    applied: usize,
    skipped: usize,
) -> String {
    let mut out = String::from("{");
    out.push_str(&format!(
        "\"modelId\":{},\"pkM\":{},\"applied\":{applied},\"skipped\":{skipped},\"edits\":[",
        crate::json::to_json(&diff.meta.model_id),
        diff.meta.pk_m
    ));
    for (idx, edit) in diff.edits.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        match edit {
            cad_slicer::DxfRoundTripEdit::Move {
                express_id,
                block_name,
                delta_2d,
                delta_3d,
            } => {
                out.push_str(&format!(
                    "{{\"kind\":\"move\",\"expressId\":{express_id},\"blockName\":{},\"delta2d\":[{},{}],\"delta3d\":[{},{},{}]}}",
                    crate::json::to_json(block_name),
                    delta_2d.x,
                    delta_2d.y,
                    delta_3d.x,
                    delta_3d.y,
                    delta_3d.z
                ));
            }
            cad_slicer::DxfRoundTripEdit::Rebend {
                express_id,
                points_3d,
            } => {
                out.push_str(&format!(
                    "{{\"kind\":\"rebend\",\"expressId\":{express_id},\"points3d\":["
                ));
                for (pi, p) in points_3d.iter().enumerate() {
                    if pi > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!("[{},{},{}]", p.x, p.y, p.z));
                }
                out.push_str("]}");
            }
        }
    }
    out.push_str("]}");
    out
}
