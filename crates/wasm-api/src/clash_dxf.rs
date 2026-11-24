//! Sequential clash cross-section batch slicer and master DXF grid combiner for WASM (F3/F4).

use crate::drawing::decode_element_chunk;
use cad_slicer::{
    ClashSectionSheet, Cut, DrawingMesh, GridMergeOptions, Layer, SvgOptions,
    build_clash_section_sheet, merge_drawings_grid, to_clash_dxf, to_svg_with,
};
use glam::{DVec2, DVec3};
use std::collections::BTreeMap;

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
        clash_a_owners: Vec<String>,
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
        // Session id of each element's model, for routing edits back on re-import.
        let mut sheet = sheet;
        for block in &mut sheet.blocks_a {
            if let Some(i) = clash_a_refs
                .iter()
                .position(|(m, id, _)| *m == block.model_id && *id == block.express_id)
            {
                block.owner = clash_a_owners.get(i).cloned().unwrap_or_default();
            }
        }

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

/// How one DXF edit ended up.
struct EditOutcome {
    status: &'static str,
    reason: Option<&'static str>,
}

/// Largest coordinate or displacement (metres) a round-trip edit may write. Anything larger
/// is no CAD edit (the Earth is 4·10⁷ m round) and would leave the product without geometry.
const MAX_ROUNDTRIP_COORD_M: f64 = 1e8;
/// Displacements up to this (metres) count as no change.
const MOVE_EPS_M: f64 = 1e-5;

/// Which edits of a DXF one model takes. With federated models the adapter applies the same
/// DXF to every open model; each edit lands on the model it was exported from, or, when that
/// model is not open (an earlier session), on the model whose product carries its `GlobalId`.
#[derive(Debug, Clone, Default)]
pub struct ApplyScope {
    /// Session id of the model being patched. `None` treats every edit as this model's.
    pub self_model_id: Option<String>,
    /// Session ids of every open model.
    pub open_model_ids: Vec<String>,
    /// Edit indices (as in the result JSON) already applied to another model in this import.
    pub exclude: Vec<u32>,
    /// With `self_model_id: None`, refuse edits that carry no `GlobalId`.
    pub require_global_id: bool,
}

impl ApplyScope {
    /// `Ok(guid_required)` when this model may take the edit, else the reason it does not.
    fn admits(
        &self,
        index: usize,
        edit: &cad_slicer::DxfRoundTripEdit,
        section_model: &str,
    ) -> Result<bool, &'static str> {
        if self.exclude.contains(&(index as u32)) {
            return Err("other-model");
        }
        let Some(me) = self.self_model_id.as_deref() else {
            return Ok(self.require_global_id);
        };
        match edit.owner() {
            Some(owner) if owner == me => Ok(false),
            Some(owner) if self.open_model_ids.iter().any(|id| id == owner) => Err("other-model"),
            // Exported in an earlier session: only the product with its GlobalId takes it.
            Some(_) => Ok(true),
            // Files without per-element owners: the section's model is a hint.
            None => Ok(section_model != me),
        }
    }
}

/// Parses an edited clash DXF and patches `original_ifc` (F5) with the default options: edits
/// without a `GlobalId` are accepted. Returns `(patched_ifc_bytes, diff_report_json)`.
pub fn apply_clash_dxf_to_step(
    original_ifc: &[u8],
    length_unit_scale: f64,
    dxf_bytes: &[u8],
) -> Result<(Vec<u8>, String), String> {
    apply_clash_dxf_to_step_scoped(original_ifc, length_unit_scale, dxf_bytes, &ApplyScope::default())
}

/// [`apply_clash_dxf_to_step`] for a model the DXF may not name: with `require_global_id`,
/// edits without a `GlobalId` are refused too.
pub fn apply_clash_dxf_to_step_with(
    original_ifc: &[u8],
    length_unit_scale: f64,
    dxf_bytes: &[u8],
    require_global_id: bool,
) -> Result<(Vec<u8>, String), String> {
    let scope = ApplyScope {
        require_global_id,
        ..ApplyScope::default()
    };
    apply_clash_dxf_to_step_scoped(original_ifc, length_unit_scale, dxf_bytes, &scope)
}

/// Parses an edited clash DXF, converts Level 1 (`Move`) and Level 2 (`Rebend`) edits from metres
/// into the IFC file's length units (`1.0 / length_unit_scale`) and patches `original_ifc`.
///
/// - Every edit must match the product's `GlobalId` when the DXF carries one; `scope` decides
///   which edits this model takes and when a `GlobalId` is required.
/// - Placements are resolved by the kernel evaluator (local, linear and grid placements).
/// - Each applied edit leaves a `/* GUBIM_F5 ... */` note, so importing the same DXF again, or a
///   later edit of the same export, only applies what changed. An element put back where the
///   export had it reverts the earlier edit, and a rebend stays where it was drawn when its
///   element is moved by a later import.
pub fn apply_clash_dxf_to_step_scoped(
    original_ifc: &[u8],
    length_unit_scale: f64,
    dxf_bytes: &[u8],
    scope: &ApplyScope,
) -> Result<(Vec<u8>, String), String> {
    let diff = cad_slicer::parse_clash_dxf_edits(dxf_bytes)?;
    let scale = if length_unit_scale.is_finite() && length_unit_scale > 0.0 {
        length_unit_scale
    } else {
        1.0
    };
    let inv_scale = 1.0 / scale;

    // Notes from earlier imports: (key, id, kind) -> value of the last one.
    let mut previous: BTreeMap<(String, u32, String), String> = BTreeMap::new();
    for note in step_parser::find_patch_notes(original_ifc) {
        let kv: BTreeMap<&str, &str> = note
            .split_whitespace()
            .skip(1)
            .filter_map(|t| t.split_once('='))
            .collect();
        if let (Some(key), Some(id), Some(kind), Some(value)) = (
            kv.get("key"),
            kv.get("id").and_then(|v| v.parse::<u32>().ok()),
            kv.get("kind"),
            kv.get("v"),
        ) {
            previous.insert((key.to_string(), id, kind.to_string()), value.to_string());
        }
    }

    let meta = &diff.meta;
    let key_of = |edit: &cad_slicer::DxfRoundTripEdit| -> String {
        let orig = edit.orig_pos();
        let f = &meta.frame;
        fnv_hex(&format!(
            "{}|{:.6}|{:.6},{:.6},{:.6}|{:.6},{:.6},{:.6}|{:.6},{:.6}",
            edit.express_id(),
            meta.pk_m.unwrap_or(0.0),
            f.right.x,
            f.right.y,
            f.right.z,
            f.up.x,
            f.up.y,
            f.up.z,
            orig.x,
            orig.y
        ))
    };

    // Index space of the result: edits, then unchanged elements, then rejected entities.
    let all: Vec<&cad_slicer::DxfRoundTripEdit> =
        diff.edits.iter().chain(diff.unchanged.iter()).collect();
    let changed = diff.edits.len();
    let mut outcomes: Vec<Option<EditOutcome>> = (0..all.len()).map(|_| None).collect();
    // Unchanged elements are reported only when this import acts on them (a revert).
    let mut reported: Vec<bool> = (0..all.len()).map(|i| i < changed).collect();
    let mut patches = Vec::new();
    let mut patch_edit_index = Vec::new();
    // Move applied in this import, per product (metres, world), for its rebend.
    let mut moved_now: BTreeMap<u32, DVec3> = BTreeMap::new();
    let skip = |reason| {
        Some(EditOutcome {
            status: "skipped",
            reason: Some(reason),
        })
    };

    for (i, edit) in all.iter().enumerate() {
        let cad_slicer::DxfRoundTripEdit::Move {
            express_id,
            global_id,
            delta_2d,
            ..
        } = edit
        else {
            continue;
        };
        let guid_required = match scope.admits(i, edit, &meta.model_id) {
            Ok(required) => required,
            Err(reason) => {
                outcomes[i] = skip(reason);
                continue;
            }
        };
        let key = key_of(edit);
        let done = previous
            .get(&(key.clone(), *express_id, "move".to_string()))
            .and_then(|v| v.split_once(','))
            .and_then(|(x, y)| Some(DVec2::new(x.parse().ok()?, y.parse().ok()?)))
            .unwrap_or(DVec2::ZERO);
        let effective = *delta_2d - done;
        if effective.length() <= MOVE_EPS_M {
            outcomes[i] = skip("already-applied");
            continue;
        }
        reported[i] = true;
        if guid_required && global_id.is_none() {
            outcomes[i] = skip("guid-missing");
            continue;
        }
        let delta_world = meta.frame.right * effective.x + meta.frame.up * effective.y;
        if !delta_world.is_finite() || delta_world.length() > MAX_ROUNDTRIP_COORD_M {
            outcomes[i] = skip("out-of-range");
            continue;
        }
        moved_now.insert(*express_id, delta_world);
        patch_edit_index.push(i);
        patches.push(step_parser::StepPatch::MovePlacement {
            express_id: *express_id,
            delta_file_units: (delta_world * inv_scale).to_array(),
            expected_global_id: global_id.clone(),
            note: Some(format!(
                "GUBIM_F5 key={key} id={express_id} kind=move v={:.6},{:.6}",
                delta_2d.x, delta_2d.y
            )),
        });
    }
    for (i, edit) in all.iter().enumerate() {
        let cad_slicer::DxfRoundTripEdit::Rebend {
            express_id,
            global_id,
            points_3d,
            ..
        } = edit
        else {
            continue;
        };
        let guid_required = match scope.admits(i, edit, &meta.model_id) {
            Ok(required) => required,
            Err(reason) => {
                outcomes[i] = skip(reason);
                continue;
            }
        };
        let key = key_of(edit);
        let shape = fnv_hex(
            &points_3d
                .iter()
                .map(|p| format!("{:.6},{:.6},{:.6}", p.x, p.y, p.z))
                .collect::<Vec<_>>()
                .join(";"),
        );
        let prior = previous.get(&(key.clone(), *express_id, "rebend".to_string()));
        // The polyline is absolute: a move applied in this import shifts the placement the
        // geometry hangs from, so an earlier rebend is written again, minus that move.
        let shift = moved_now.get(express_id).copied().unwrap_or(DVec3::ZERO);
        let moved = shift.length() > 1e-9;
        if i >= changed {
            // Unchanged polyline: only acts on an element an earlier import rebent.
            if prior.is_none() || (prior == Some(&shape) && !moved) {
                continue;
            }
            reported[i] = true;
        } else if prior == Some(&shape) && !moved {
            outcomes[i] = skip("already-applied");
            continue;
        }
        if guid_required && global_id.is_none() {
            outcomes[i] = skip("guid-missing");
            continue;
        }
        if points_3d
            .iter()
            .any(|p| !p.is_finite() || p.abs().max_element() > MAX_ROUNDTRIP_COORD_M)
        {
            outcomes[i] = skip("out-of-range");
            continue;
        }
        let pts_file: Vec<[f64; 3]> = points_3d
            .iter()
            .map(|p| ((*p - shift) * inv_scale).to_array())
            .collect();
        patch_edit_index.push(i);
        patches.push(step_parser::StepPatch::RebendSweptDisk {
            express_id: *express_id,
            points_file_units: pts_file,
            radius_file_units: 0.0,
            expected_global_id: global_id.clone(),
            note: Some(format!(
                "GUBIM_F5 key={key} id={express_id} kind=rebend v={shape}"
            )),
        });
    }

    // Nothing for this model: skip indexing and evaluating its STEP file.
    let (patched_bytes, report) = if patches.is_empty() {
        step_parser::patch_step_file(original_ifc, &[])?
    } else {
        patch_with_kernel_placements(original_ifc, &patches, scale, false)?
    };
    for (outcome, &edit_index) in report.outcomes.iter().zip(&patch_edit_index) {
        outcomes[edit_index] = Some(EditOutcome {
            status: if outcome.applied { "applied" } else { "skipped" },
            reason: outcome.reason,
        });
    }
    let entries: Vec<(usize, &cad_slicer::DxfRoundTripEdit, Option<EditOutcome>)> = all
        .iter()
        .zip(outcomes)
        .enumerate()
        .filter(|(i, _)| reported[*i])
        .map(|(i, (edit, outcome))| (i, *edit, outcome))
        .collect();
    let json = format_roundtrip_json(&diff, all.len(), &entries);
    Ok((patched_bytes, json))
}

/// [`step_parser::patch_step_file_with`] with placements resolved by the kernel evaluator
/// (metres, converted to file units with `length_unit_scale`), so local, linear and grid
/// placements are read exactly as the viewer draws them.
pub fn patch_with_kernel_placements(
    original_ifc: &[u8],
    patches: &[step_parser::StepPatch],
    length_unit_scale: f64,
    require_global_id: bool,
) -> Result<(Vec<u8>, step_parser::StepPatchReport), String> {
    let inv_scale = if length_unit_scale.is_finite() && length_unit_scale > 0.0 {
        1.0 / length_unit_scale
    } else {
        1.0
    };
    let step = step_parser::index(original_ifc).ok();
    let mut evaluator = step
        .as_ref()
        .and_then(|s| ifc_geometry::ModelEvaluator::new(s).ok());
    let has_evaluator = evaluator.is_some();
    let mut resolver = |placement_id: u32| -> Option<step_parser::PlacementTransform> {
        let world = evaluator.as_mut()?.placement_world(placement_id)?;
        let cols = [
            world.x_axis.truncate().to_array(),
            world.y_axis.truncate().to_array(),
            world.z_axis.truncate().to_array(),
        ];
        Some((cols, (world.w_axis.truncate() * inv_scale).to_array()))
    };
    let options = step_parser::PatchOptions {
        placement_resolver: if has_evaluator {
            Some(&mut resolver)
        } else {
            None
        },
        require_global_id,
    };
    step_parser::patch_step_file_with(original_ifc, patches, options)
}

/// FNV-1a 64-bit as 16 hex digits.
fn fnv_hex(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn json_num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".to_string()
    }
}

/// The edits an edited DXF holds, without applying them (`status: "detected"`).
pub fn format_detected_json(diff: &cad_slicer::DxfRoundTripDiff) -> String {
    let entries: Vec<(usize, &cad_slicer::DxfRoundTripEdit, Option<EditOutcome>)> = diff
        .edits
        .iter()
        .enumerate()
        .map(|(i, edit)| {
            (
                i,
                edit,
                Some(EditOutcome {
                    status: "detected",
                    reason: None,
                }),
            )
        })
        .collect();
    format_roundtrip_json(diff, diff.edits.len() + diff.unchanged.len(), &entries)
}

/// Result JSON. Every entry carries `index`, stable for one DXF whatever model it is applied
/// to, so the adapter can merge the results of several models; rejected entities follow at
/// `index >= rejected_base`.
fn format_roundtrip_json(
    diff: &cad_slicer::DxfRoundTripDiff,
    rejected_base: usize,
    entries: &[(usize, &cad_slicer::DxfRoundTripEdit, Option<EditOutcome>)],
) -> String {
    let applied = entries
        .iter()
        .filter(|(_, _, o)| o.as_ref().is_some_and(|o| o.status == "applied"))
        .count();
    let skipped = entries.len() - applied + diff.rejected.len();
    let mut out = String::from("{");
    out.push_str(&format!(
        "\"modelId\":{},\"pkM\":{},\"metadataComplete\":{},\"applied\":{applied},\"skipped\":{skipped},\"edits\":[",
        crate::json::to_json(&diff.meta.model_id),
        diff.meta.pk_m.map_or_else(|| "null".to_string(), json_num),
        diff.meta.complete,
    ));
    let status_json = |o: Option<&EditOutcome>| -> String {
        let (status, reason) = o.map_or(("skipped", Some("internal")), |o| (o.status, o.reason));
        format!(
            ",\"status\":\"{status}\",\"reason\":{}",
            reason.map_or_else(|| "null".to_string(), |r| format!("\"{r}\""))
        )
    };
    let text_json = |g: Option<&str>| g.map_or_else(|| "null".to_string(), crate::json::to_json);
    let mut first = true;
    let mut sep = |out: &mut String| {
        if !first {
            out.push(',');
        }
        first = false;
    };
    for (index, edit, outcome) in entries {
        sep(&mut out);
        let common = format!(
            "\"index\":{index},\"expressId\":{},\"globalId\":{},\"owner\":{}",
            edit.express_id(),
            text_json(edit.global_id()),
            text_json(edit.owner())
        );
        match edit {
            cad_slicer::DxfRoundTripEdit::Move {
                block_name,
                delta_2d,
                delta_3d,
                ..
            } => {
                out.push_str(&format!(
                    "{{\"kind\":\"move\",{common},\"blockName\":{},\"delta2d\":[{},{}],\"delta3d\":[{},{},{}]{}}}",
                    crate::json::to_json(block_name),
                    json_num(delta_2d.x),
                    json_num(delta_2d.y),
                    json_num(delta_3d.x),
                    json_num(delta_3d.y),
                    json_num(delta_3d.z),
                    status_json(outcome.as_ref())
                ));
            }
            cad_slicer::DxfRoundTripEdit::Rebend { points_3d, .. } => {
                let pts = points_3d
                    .iter()
                    .map(|p| format!("[{},{},{}]", json_num(p.x), json_num(p.y), json_num(p.z)))
                    .collect::<Vec<_>>()
                    .join(",");
                out.push_str(&format!(
                    "{{\"kind\":\"rebend\",{common},\"points3d\":[{pts}]{}}}",
                    status_json(outcome.as_ref())
                ));
            }
        }
    }
    for (k, rejected) in diff.rejected.iter().enumerate() {
        sep(&mut out);
        let geometry = if rejected.kind == "move" {
            "\"blockName\":\"\",\"delta2d\":[0,0],\"delta3d\":[0,0,0]"
        } else {
            "\"points3d\":[]"
        };
        out.push_str(&format!(
            "{{\"kind\":\"{}\",\"index\":{},\"expressId\":{},\"globalId\":null,\"owner\":null,{geometry},\"status\":\"skipped\",\"reason\":\"{}\"}}",
            rejected.kind,
            rejected_base + k,
            rejected.express_id,
            rejected.reason
        ));
    }
    out.push_str("]}");
    out
}
