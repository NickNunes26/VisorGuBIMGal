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
pub struct ClashBatchStore {
    current_meshes: Vec<DrawingMesh>,
    sheets: Vec<ClashSectionSheet>,
    /// Export time written into every section's `GUBIM_META t=` (ms since the Unix epoch).
    export_time_ms: Option<f64>,
    /// Whether `finish_section` should render an SVG preview (needed for single-section preview,
    /// skipped during large multi-section ZIP exports to save allocations and time).
    include_svg: bool,
}

impl Default for ClashBatchStore {
    fn default() -> Self {
        Self {
            current_meshes: Vec::new(),
            sheets: Vec::new(),
            export_time_ms: None,
            include_svg: true,
        }
    }
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

    /// Sets the export time the following sections carry (`GUBIM_META t=`).
    pub fn set_export_time_ms(&mut self, ms: f64) {
        self.export_time_ms = (ms.is_finite() && ms >= 0.0).then_some(ms);
    }

    pub fn set_include_svg(&mut self, include_svg: bool) {
        self.include_svg = include_svg;
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
        sheet.meta.exported_at_ms = self.export_time_ms;
        for block in &mut sheet.blocks_a {
            if let Some(i) = clash_a_refs
                .iter()
                .position(|(m, id, _)| *m == block.model_id && *id == block.express_id)
            {
                block.owner = clash_a_owners.get(i).cloned().unwrap_or_default();
            }
        }

        let dxf_bytes = to_clash_dxf(&sheet);
        let svg = if self.include_svg {
            preview_svg(&sheet)
        } else {
            String::new()
        };
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

    pub fn merge_master_dxf(&mut self, columns: Option<usize>, margin_m: f64) -> Vec<u8> {
        let opts = GridMergeOptions {
            columns,
            margin_m: if margin_m.is_finite() && margin_m > 0.0 {
                margin_m
            } else {
                1.0
            },
            ..GridMergeOptions::default()
        };
        let out = merge_drawings_grid(&self.sheets, &opts).dxf_bytes;
        self.sheets.clear();
        self.sheets.shrink_to_fit();
        out
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

/// What to do with an element another section edited after this DXF was exported (F7): the
/// drawing did not show that edit, so applying it as it is would add the two edits up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Leave those elements alone (`edit-conflict`), so the caller can ask the user.
    #[default]
    Ask,
    /// Apply this drawing's edit on top of the other section's.
    Add,
    /// This drawing wins: the other section's edit of the element is undone.
    Replace,
    /// Leave those elements as they are (`edit-conflict`).
    Skip,
}

impl ConflictPolicy {
    /// `"add"`, `"replace"` or `"skip"`; anything else asks.
    pub fn parse(value: &str) -> Self {
        match value {
            "add" => Self::Add,
            "replace" => Self::Replace,
            "skip" => Self::Skip,
            _ => Self::Ask,
        }
    }
}

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
    /// Elements another section edited after this DXF was exported.
    pub conflict: ConflictPolicy,
    /// Import time written into the notes (ms since the Unix epoch).
    pub now_ms: Option<f64>,
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

/// The last `GUBIM_F5` note of one (section key, element, kind).
#[derive(Debug, Clone)]
struct NoteState {
    /// `kind=move`: the drawing's own displacement (`dx,dy`, metres, in its section frame).
    /// `kind=rebend`: the hash of the polyline, or `-` once another section replaced it.
    value: String,
    /// `kind=move`: the same displacement in world metres (notes written since F7).
    world: Option<DVec3>,
    /// `kind=rebend`: original `IfcProduct.Representation` `#id` before any rebend (`rep=`).
    orig_rep: Option<u32>,
    /// Import time (ms since the Unix epoch; notes written since F7).
    at_ms: Option<f64>,
    /// Monotonic import sequence within the STEP file (`0..`), so later imports are ordered
    /// reliably even when timestamps match or are absent.
    seq: usize,
}

impl NoteState {
    fn is_active(&self, kind: &str) -> bool {
        if kind == "move" {
            match self.world {
                Some(w) => w.length() > MOVE_EPS_M,
                None => parse_pair(&self.value).is_some_and(|v| v.length() > MOVE_EPS_M),
            }
        } else {
            self.value != "-"
        }
    }
}

fn parse_pair(value: &str) -> Option<DVec2> {
    let (x, y) = value.split_once(',')?;
    Some(DVec2::new(x.parse().ok()?, y.parse().ok()?))
}

fn parse_triple(value: &str) -> Option<DVec3> {
    let mut it = value.split(',').map(|v| v.parse::<f64>().ok());
    let v = DVec3::new(it.next()??, it.next()??, it.next()??);
    v.is_finite().then_some(v)
}

/// Notes of earlier imports, in file (that is, import) order: the last one of each
/// `(key, id, kind)` wins, and `drop=` resets the keys another section's edit replaced.
fn read_notes(original_ifc: &[u8]) -> BTreeMap<(String, u32, String), NoteState> {
    let mut previous: BTreeMap<(String, u32, String), NoteState> = BTreeMap::new();
    for (seq, note) in step_parser::find_patch_notes(original_ifc).into_iter().enumerate() {
        let kv: BTreeMap<&str, &str> = note
            .split_whitespace()
            .skip(1)
            .filter_map(|t| t.split_once('='))
            .collect();
        let (Some(key), Some(id), Some(kind), Some(value)) = (
            kv.get("key"),
            kv.get("id").and_then(|v| v.parse::<u32>().ok()),
            kv.get("kind"),
            kv.get("v"),
        ) else {
            continue;
        };
        let at_ms = kv.get("t").and_then(|v| v.parse::<f64>().ok()).filter(|t| t.is_finite());
        let orig_rep = kv.get("rep").and_then(|v| v.parse::<u32>().ok()).filter(|&r| r > 0);
        for dropped in kv.get("drop").into_iter().flat_map(|d| d.split(',')) {
            let prev_rep = previous
                .get(&(dropped.to_string(), id, kind.to_string()))
                .and_then(|n| n.orig_rep)
                .or(orig_rep);
            let reset = if *kind == "move" {
                NoteState {
                    value: "0.000000,0.000000".to_string(),
                    world: Some(DVec3::ZERO),
                    orig_rep: None,
                    at_ms,
                    seq,
                }
            } else {
                NoteState {
                    value: "-".to_string(),
                    world: None,
                    orig_rep: prev_rep,
                    at_ms,
                    seq,
                }
            };
            previous.insert((dropped.to_string(), id, kind.to_string()), reset);
        }
        let prev_rep = previous
            .get(&(key.to_string(), id, kind.to_string()))
            .and_then(|n| n.orig_rep);
        previous.insert(
            (key.to_string(), id, kind.to_string()),
            NoteState {
                value: value.to_string(),
                world: kv.get("w").and_then(|v| parse_triple(v)),
                orig_rep: orig_rep.or(prev_rep),
                at_ms,
                seq,
            },
        );
    }
    previous
}

/// Active notes from other sections on `express_id` of `kind` that this DXF did not show
/// (imported after `meta.exported_at_ms`, or of unknown timestamp).
fn unseen_notes<'a>(
    previous: &'a BTreeMap<(String, u32, String), NoteState>,
    meta: &cad_slicer::ParsedClashMeta,
    express_id: u32,
    key: &str,
    kind: &str,
) -> Vec<(&'a String, &'a NoteState)> {
    previous
        .iter()
        .filter(|((k, id, kd), note)| {
            *id == express_id
                && kd == kind
                && k != key
                && note.is_active(kind)
                && match (meta.exported_at_ms, note.at_ms) {
                    (Some(exported), Some(at)) => at > exported,
                    _ => true,
                }
        })
        .map(|((k, _, _), note)| (k, note))
        .collect()
}

/// Original `IfcProduct.Representation` `#id` of `express_id` before any F5 rebend.
/// Returns `None` when a legacy rebend note without `rep=` already overwrote the shape.
fn original_rep_of(
    previous: &BTreeMap<(String, u32, String), NoteState>,
    step: Option<&step_parser::StepIndex<'_>>,
    express_id: u32,
) -> Option<u32> {
    let mut saw_rebend_note = false;
    for ((_, id, kd), note) in previous {
        if *id == express_id && kd == "rebend" {
            saw_rebend_note = true;
            if let Some(rep) = note.orig_rep {
                return Some(rep);
            }
        }
    }
    if saw_rebend_note {
        None
    } else {
        step.and_then(|s| step_parser::check_product_guid(s, express_id, None, false).ok().flatten())
    }
}

/// Whether another section imported an active edit on `express_id` *after* both this DXF's
/// export and this section's own last import (`own_seq`), meaning the element is no longer
/// where this section left it (for example, a later section chose `Add`).
fn has_newer_unseen_other(
    previous: &BTreeMap<(String, u32, String), NoteState>,
    meta: &cad_slicer::ParsedClashMeta,
    express_id: u32,
    key: &str,
    own_seq: usize,
) -> bool {
    previous.iter().any(|((k, id, kd), note)| {
        *id == express_id
            && k != key
            && note.seq > own_seq
            && note.is_active(kd)
            && match (meta.exported_at_ms, note.at_ms) {
                (Some(exported), Some(at)) => at > exported,
                _ => true,
            }
    })
}

/// Idempotence key of one element of one exported section.
fn edit_key(meta: &cad_slicer::ParsedClashMeta, edit: &cad_slicer::DxfRoundTripEdit) -> String {
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
}

/// Changes a second planning pass makes for elements in conflict (F7).
#[derive(Default)]
struct Adjust {
    /// Left alone with `edit-conflict`.
    blocked: std::collections::BTreeSet<u32>,
    /// World displacement (metres) of other sections' moves to undo.
    undo: BTreeMap<u32, DVec3>,
    /// Write the drawing's polyline even when it is unchanged.
    force_rebend: std::collections::BTreeSet<u32>,
    /// Restore original `IfcProduct.Representation` `#id` when `Replace` undoes a rebend from
    /// a section that carries no `EDIT_A` polyline.
    restore_rep: BTreeMap<u32, u32>,
    /// Other sections' keys whose move or rebend the new note resets.
    drop_moves: BTreeMap<u32, Vec<String>>,
    drop_rebends: BTreeMap<u32, Vec<String>>,
}

struct Plan {
    outcomes: Vec<Option<EditOutcome>>,
    reported: Vec<bool>,
    patches: Vec<step_parser::StepPatch>,
    /// `(edit_index, is_primary)` for each entry of `patches`.
    patch_edit_index: Vec<(usize, bool)>,
    /// Elements this import patches (or would, but for a conflict), with their section key.
    acted: BTreeMap<u32, String>,
}

struct PlanInput<'a> {
    all: &'a [&'a cad_slicer::DxfRoundTripEdit],
    changed: usize,
    meta: &'a cad_slicer::ParsedClashMeta,
    scope: &'a ApplyScope,
    previous: &'a BTreeMap<(String, u32, String), NoteState>,
    step: Option<&'a step_parser::StepIndex<'a>>,
    inv_scale: f64,
}

fn note_suffix(now_ms: Option<f64>, dropped: Option<&Vec<String>>) -> String {
    let mut out = String::new();
    if let Some(t) = now_ms.filter(|t| t.is_finite() && *t >= 0.0) {
        out.push_str(&format!(" t={t:.0}"));
    }
    if let Some(keys) = dropped.filter(|k| !k.is_empty()) {
        out.push_str(&format!(" drop={}", keys.join(",")));
    }
    out
}

/// Decides what each edit does, without touching the file.
fn plan_edits(input: &PlanInput<'_>, adjust: &Adjust) -> Plan {
    let PlanInput {
        all,
        changed,
        meta,
        scope,
        previous,
        step,
        inv_scale,
    } = *input;
    let mut outcomes: Vec<Option<EditOutcome>> = (0..all.len()).map(|_| None).collect();
    // Unchanged elements are reported only when this import acts on them (a revert).
    let mut reported: Vec<bool> = (0..all.len()).map(|i| i < changed).collect();
    let mut patches = Vec::new();
    let mut patch_edit_index = Vec::new();
    let mut acted = BTreeMap::new();
    // Move applied in this import, per product (metres, world), for its rebend.
    let mut moved_now: BTreeMap<u32, DVec3> = BTreeMap::new();
    let mut move_seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
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
        if let Some(step) = step
            && let Err(reason) = step_parser::check_product_guid(
                step,
                *express_id,
                global_id.as_deref(),
                guid_required,
            )
        {
            if i < changed {
                outcomes[i] = skip(reason);
            }
            continue;
        }
        move_seen.insert(*express_id);
        let key = edit_key(meta, edit);
        let own_note = previous.get(&(key.clone(), *express_id, "move".to_string()));
        let done = own_note
            .and_then(|n| parse_pair(&n.value))
            .unwrap_or(DVec2::ZERO);
        let own = *delta_2d - done;
        let undo = adjust.undo.get(express_id).copied().unwrap_or(DVec3::ZERO);
        let delta_world = meta.frame.right * own.x + meta.frame.up * own.y - undo;
        let dropping = adjust.drop_moves.get(express_id).is_some_and(|d| !d.is_empty());
        let restoring = adjust.restore_rep.contains_key(express_id);
        let superseded = i < changed
            && scope.conflict != ConflictPolicy::Add
            && own_note
                .filter(|n| n.is_active("move"))
                .is_some_and(|n| has_newer_unseen_other(previous, meta, *express_id, &key, n.seq));
        if delta_world.length() <= MOVE_EPS_M && !dropping && !restoring && !superseded {
            outcomes[i] = skip("already-applied");
            continue;
        }
        reported[i] = true;
        acted.insert(*express_id, key.clone());
        if adjust.blocked.contains(express_id) {
            outcomes[i] = skip("edit-conflict");
            continue;
        }
        if guid_required && global_id.is_none() {
            outcomes[i] = skip("guid-missing");
            continue;
        }
        if !delta_world.is_finite() || delta_world.length() > MAX_ROUNDTRIP_COORD_M {
            outcomes[i] = skip("out-of-range");
            continue;
        }
        let total = meta.frame.right * delta_2d.x + meta.frame.up * delta_2d.y;
        let note = format!(
            "GUBIM_F5 key={key} id={express_id} kind=move v={:.6},{:.6} w={:.6},{:.6},{:.6}{}",
            delta_2d.x,
            delta_2d.y,
            total.x,
            total.y,
            total.z,
            note_suffix(scope.now_ms, adjust.drop_moves.get(express_id)),
        );
        patch_edit_index.push((i, true));
        if delta_world.length() <= MOVE_EPS_M {
            patches.push(step_parser::StepPatch::RecordNote {
                express_id: *express_id,
                expected_global_id: global_id.clone(),
                note,
            });
        } else {
            moved_now.insert(*express_id, delta_world);
            patches.push(step_parser::StepPatch::MovePlacement {
                express_id: *express_id,
                delta_file_units: (delta_world * inv_scale).to_array(),
                expected_global_id: global_id.clone(),
                note: Some(note),
            });
        }
        if let Some(&rep_id) = adjust.restore_rep.get(express_id) {
            patch_edit_index.push((i, false));
            patches.push(step_parser::StepPatch::RestoreRepresentation {
                express_id: *express_id,
                representation_id: rep_id,
                expected_global_id: global_id.clone(),
                note: Some(format!(
                    "GUBIM_F5 key={key} id={express_id} kind=rebend v=- rep={rep_id}{}",
                    note_suffix(scope.now_ms, adjust.drop_rebends.get(express_id)),
                )),
            });
        }
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
        if let Some(step) = step
            && let Err(reason) = step_parser::check_product_guid(
                step,
                *express_id,
                global_id.as_deref(),
                guid_required,
            )
        {
            if i < changed {
                outcomes[i] = skip(reason);
            }
            continue;
        }
        let key = edit_key(meta, edit);
        let shape = fnv_hex(
            &points_3d
                .iter()
                .map(|p| format!("{:.6},{:.6},{:.6}", p.x, p.y, p.z))
                .collect::<Vec<_>>()
                .join(";"),
        );
        let own_rebend = previous
            .get(&(key.clone(), *express_id, "rebend".to_string()))
            .filter(|n| n.is_active("rebend"));
        let prior = own_rebend.map(|n| &n.value);
        // The polyline is absolute: a move applied in this import shifts the placement the
        // geometry hangs from, so an earlier rebend is written again, minus that move.
        let mut shift = moved_now.get(express_id).copied().unwrap_or(DVec3::ZERO);
        let moved = shift.length() > 1e-9;
        let superseded = i < changed
            && scope.conflict != ConflictPolicy::Add
            && own_rebend
                .is_some_and(|n| has_newer_unseen_other(previous, meta, *express_id, &key, n.seq));
        if !adjust.force_rebend.contains(express_id) {
            if i >= changed {
                // Unchanged polyline: only acts on an element an earlier import rebent.
                if prior.is_none() || (prior == Some(&shape) && !moved) {
                    continue;
                }
                reported[i] = true;
            } else if prior == Some(&shape) && !moved && !superseded {
                outcomes[i] = skip("already-applied");
                continue;
            }
        }
        reported[i] = true;
        acted.insert(*express_id, key.clone());
        if adjust.blocked.contains(express_id) {
            outcomes[i] = skip("edit-conflict");
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
        // If the drafter deleted the INSERT entity and left only EDIT_A while `Replace` must
        // undo another section's move, emit the undo move alongside the rebend.
        if !move_seen.contains(express_id) && adjust.drop_moves.contains_key(express_id) {
            let undo = adjust.undo.get(express_id).copied().unwrap_or(DVec3::ZERO);
            let delta_undo = -undo;
            let move_note = format!(
                "GUBIM_F5 key={key} id={express_id} kind=move v=0.000000,0.000000 w=0.000000,0.000000,0.000000{}",
                note_suffix(scope.now_ms, adjust.drop_moves.get(express_id)),
            );
            patch_edit_index.push((i, false));
            if delta_undo.length() <= MOVE_EPS_M {
                patches.push(step_parser::StepPatch::RecordNote {
                    express_id: *express_id,
                    expected_global_id: global_id.clone(),
                    note: move_note,
                });
            } else {
                shift = delta_undo;
                moved_now.insert(*express_id, delta_undo);
                patches.push(step_parser::StepPatch::MovePlacement {
                    express_id: *express_id,
                    delta_file_units: (delta_undo * inv_scale).to_array(),
                    expected_global_id: global_id.clone(),
                    note: Some(move_note),
                });
            }
        }
        // Active unseen moves from other sections that are kept (e.g. under `Add`) already
        // shifted `ObjectPlacement` in `original_ifc`, while this pre-move DXF's `points_3d`
        // were drawn without them: add them so the curve lands at `points_3d + kept_unseen`.
        let dropped_moves = adjust.drop_moves.get(express_id);
        let kept_unseen_move: DVec3 = unseen_notes(previous, meta, *express_id, &key, "move")
            .into_iter()
            .filter(|(k, _)| dropped_moves.is_none_or(|d| !d.contains(*k)))
            .filter_map(|(_, n)| n.world)
            .sum();
        let pts_file: Vec<[f64; 3]> = points_3d
            .iter()
            .map(|p| ((*p + kept_unseen_move - shift) * inv_scale).to_array())
            .collect();
        let rep_part = original_rep_of(previous, step, *express_id)
            .map(|r| format!(" rep={r}"))
            .unwrap_or_default();
        let add_dropped_rebends: Vec<String> = if scope.conflict == ConflictPolicy::Add {
            unseen_notes(previous, meta, *express_id, &key, "rebend")
                .into_iter()
                .map(|(k, _)| k.clone())
                .collect()
        } else {
            Vec::new()
        };
        let dropped_rebends = adjust
            .drop_rebends
            .get(express_id)
            .or((!add_dropped_rebends.is_empty()).then_some(&add_dropped_rebends));
        patch_edit_index.push((i, true));
        patches.push(step_parser::StepPatch::RebendSweptDisk {
            express_id: *express_id,
            points_file_units: pts_file,
            radius_file_units: 0.0,
            expected_global_id: global_id.clone(),
            note: Some(format!(
                "GUBIM_F5 key={key} id={express_id} kind=rebend v={shape}{rep_part}{}",
                note_suffix(scope.now_ms, dropped_rebends),
            )),
        });
    }
    Plan {
        outcomes,
        reported,
        patches,
        patch_edit_index,
        acted,
    }
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
/// - An element another section edited after this DXF was exported is handled by
///   `scope.conflict`: by default it is left alone with `edit-conflict`.
pub fn apply_clash_dxf_to_step_scoped(
    original_ifc: &[u8],
    length_unit_scale: f64,
    dxf_bytes: &[u8],
    scope: &ApplyScope,
) -> Result<(Vec<u8>, String), String> {
    apply_clash_dxf_to_step_indexed(original_ifc, None, length_unit_scale, dxf_bytes, scope)
}

/// [`apply_clash_dxf_to_step_scoped`] reusing an existing [`step_parser::StepIndex`] when the
/// caller already holds one (`LoadedModel`).
pub fn apply_clash_dxf_to_step_indexed(
    original_ifc: &[u8],
    preindexed: Option<&step_parser::StepIndex<'_>>,
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
    let previous = read_notes(original_ifc);
    let meta = &diff.meta;

    // Index space of the result: edits, then unchanged elements, then rejected entities.
    let all: Vec<&cad_slicer::DxfRoundTripEdit> =
        diff.edits.iter().chain(diff.unchanged.iter()).collect();
    let any_admitted = all
        .iter()
        .enumerate()
        .any(|(i, edit)| scope.admits(i, edit, &meta.model_id).is_ok());
    let owned_step = if preindexed.is_none() && any_admitted {
        step_parser::index(original_ifc).ok()
    } else {
        None
    };
    let input = PlanInput {
        all: &all,
        changed: diff.edits.len(),
        meta,
        scope,
        previous: &previous,
        step: preindexed.or(owned_step.as_ref()),
        inv_scale: 1.0 / scale,
    };
    let first = plan_edits(&input, &Adjust::default());

    // Other sections' edits of the same element this drawing did not show: made after the
    // export (or of unknown time).
    let unseen = |express_id: u32, key: &str, kind: &str| {
        unseen_notes(&previous, meta, express_id, key, kind)
    };
    let conflicted: Vec<(u32, &String)> = first
        .acted
        .iter()
        .filter(|(id, key)| {
            !unseen(**id, key, "move").is_empty() || !unseen(**id, key, "rebend").is_empty()
        })
        .map(|(id, key)| (*id, key))
        .collect();

    let plan = if conflicted.is_empty() || scope.conflict == ConflictPolicy::Add {
        first
    } else {
        let mut adjust = Adjust::default();
        for &(id, key) in &conflicted {
            if scope.conflict != ConflictPolicy::Replace {
                adjust.blocked.insert(id);
                continue;
            }
            let moves = unseen(id, key, "move");
            // A note written before F7 has no world displacement to undo.
            if moves.iter().any(|(_, note)| note.world.is_none()) {
                adjust.blocked.insert(id);
                continue;
            }
            let rebends = unseen(id, key, "rebend");
            let has_rebend_in_dxf = all.iter().enumerate().any(|(idx, e)| {
                e.express_id() == id
                    && matches!(e, cad_slicer::DxfRoundTripEdit::Rebend { .. })
                    && scope.admits(idx, e, &meta.model_id).is_ok()
            });
            if !rebends.is_empty() && !has_rebend_in_dxf {
                // This DXF carries no EDIT_A polyline for `id` (longitudinal cut or polyline
                // deleted in CAD): restore the product's original pre-rebend representation.
                let orig_rep = rebends
                    .iter()
                    .find_map(|(_, n)| n.orig_rep)
                    .or_else(|| {
                        previous
                            .get(&(key.clone(), id, "rebend".to_string()))
                            .and_then(|n| n.orig_rep)
                    });
                let Some(rep_id) = orig_rep else {
                    adjust.blocked.insert(id);
                    continue;
                };
                adjust.restore_rep.insert(id, rep_id);
            }
            if !moves.is_empty() {
                let undo = moves.iter().filter_map(|(_, note)| note.world).sum::<DVec3>();
                adjust.undo.insert(id, undo);
                adjust
                    .drop_moves
                    .insert(id, moves.iter().map(|(k, _)| (*k).clone()).collect());
            }
            let own_rebend = previous
                .get(&(key.clone(), id, "rebend".to_string()))
                .is_some_and(|n| n.is_active("rebend"));
            if (!rebends.is_empty() && has_rebend_in_dxf) || own_rebend {
                adjust.force_rebend.insert(id);
            }
            if !rebends.is_empty() {
                adjust
                    .drop_rebends
                    .insert(id, rebends.iter().map(|(k, _)| (*k).clone()).collect());
            }
        }
        plan_edits(&input, &adjust)
    };
    let Plan {
        mut outcomes,
        reported,
        patches,
        patch_edit_index,
        ..
    } = plan;

    // Nothing for this model: skip indexing and evaluating its STEP file.
    let (patched_bytes, report) = if patches.is_empty() {
        step_parser::patch_step_file(original_ifc, &[])?
    } else {
        patch_with_kernel_placements(original_ifc, &patches, scale, false)?
    };
    for (outcome, &(edit_index, is_primary)) in report.outcomes.iter().zip(&patch_edit_index) {
        if is_primary || !outcome.applied {
            outcomes[edit_index] = Some(EditOutcome {
                status: if outcome.applied { "applied" } else { "skipped" },
                reason: outcome.reason,
            });
        }
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
