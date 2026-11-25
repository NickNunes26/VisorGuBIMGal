//! Surgical ISO 10303-21 (STEP) IFC patcher for Phase F5 (`docs/12_clash_alignment_dxf.md` §3.5).
//!
//! Preserves the original IFC file byte for byte: headers, GUIDs, property sets and every
//! untouched entity stay unchanged. New entities are appended just before the `ENDSEC;` that
//! closes `DATA;`, and only one `#ref` attribute is rewritten on each modified product
//! (`ObjectPlacement` for Level 1 moves, `Representation` for Level 2 rebends).
//!
//! Safety rules:
//! - An edit is applied only when the product's `GlobalId` matches the one the edit expects
//!   (a prefix is accepted, because DXF block names are cut to 31 characters). Without an
//!   expected `GlobalId` the edit is refused when [`PatchOptions::require_global_id`] is set.
//! - World transforms of placements come from [`PatchOptions::placement_resolver`] when given
//!   (the kernel's own evaluator, which understands `IfcLinearPlacement` and `IfcGridPlacement`).
//!   The built-in fallback only walks `IfcLocalPlacement` chains and refuses anything else, so a
//!   move is never applied in the wrong frame.
//! - Each product is patched at most once per call; later edits of the same kind are skipped.

use crate::{StepIndex, Value, index};
use std::collections::BTreeSet;

/// World rotation (columns: local X, Y, Z axes) and translation, in the file's length units.
pub type PlacementTransform = ([[f64; 3]; 3], [f64; 3]);

/// One surgical edit to apply to an IFC STEP file.
#[derive(Debug, Clone, PartialEq)]
pub enum StepPatch {
    /// Level 1 (`MOVE`): translates `IfcProduct.ObjectPlacement` by `delta_file_units`, a
    /// world-space vector in the IFC file's own length units.
    MovePlacement {
        express_id: u32,
        delta_file_units: [f64; 3],
        /// `GlobalId` (or a prefix of it) the product must carry.
        expected_global_id: Option<String>,
        /// Written as a `/* ... */` STEP comment before the new entities when applied.
        note: Option<String>,
    },
    /// Level 2 (`Rebend`): replaces `IfcProduct.Representation` with a new swept disk solid
    /// along `points_file_units` (world space, file units) with `radius_file_units`
    /// (`<= 0` reads the radius from the product).
    RebendSweptDisk {
        express_id: u32,
        points_file_units: Vec<[f64; 3]>,
        radius_file_units: f64,
        expected_global_id: Option<String>,
        note: Option<String>,
    },
}

impl StepPatch {
    pub fn express_id(&self) -> u32 {
        match self {
            StepPatch::MovePlacement { express_id, .. }
            | StepPatch::RebendSweptDisk { express_id, .. } => *express_id,
        }
    }

    fn expected_global_id(&self) -> Option<&str> {
        match self {
            StepPatch::MovePlacement {
                expected_global_id, ..
            }
            | StepPatch::RebendSweptDisk {
                expected_global_id, ..
            } => expected_global_id.as_deref().filter(|g| !g.is_empty()),
        }
    }

    fn note(&self) -> Option<&str> {
        match self {
            StepPatch::MovePlacement { note, .. } | StepPatch::RebendSweptDisk { note, .. } => {
                note.as_deref()
            }
        }
    }
}

/// What happened to one [`StepPatch`], in input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOutcome {
    pub express_id: u32,
    pub applied: bool,
    /// Stable reason code when skipped: `not-found`, `not-a-product`, `guid-mismatch`,
    /// `guid-missing`, `unsupported-placement`, `no-representation-context`, `invalid-points`,
    /// `invalid-radius`, `duplicate-edit`.
    pub reason: Option<&'static str>,
}

/// Summary of applied surgical STEP patches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepPatchReport {
    pub applied: usize,
    pub skipped: usize,
    pub first_new_id: u32,
    pub last_new_id: u32,
    pub outcomes: Vec<PatchOutcome>,
}

/// Options for [`patch_step_file_with`].
#[derive(Default)]
pub struct PatchOptions<'r> {
    /// World transform of a placement entity (`IfcObjectPlacement` subtype) in file units.
    /// `None` from the resolver means the placement cannot be resolved and the edit is skipped.
    pub placement_resolver: Option<&'r mut dyn FnMut(u32) -> Option<PlacementTransform>>,
    /// Refuse edits that carry no expected `GlobalId`.
    pub require_global_id: bool,
}

/// Applies surgical STEP patches with the default options.
pub fn patch_step_file(
    original_ifc: &[u8],
    patches: &[StepPatch],
) -> Result<(Vec<u8>, StepPatchReport), String> {
    patch_step_file_with(original_ifc, patches, PatchOptions::default())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Schema {
    Ifc2x3,
    Ifc4,
    Ifc4x3,
}

fn schema_of(step: &StepIndex<'_>) -> Schema {
    let ids = step.schema_identifiers();
    let has = |needle: &str| ids.iter().any(|s| s.to_ascii_uppercase().starts_with(needle));
    if has("IFC2X3") {
        Schema::Ifc2x3
    } else if has("IFC4X3") {
        Schema::Ifc4x3
    } else {
        Schema::Ifc4
    }
}

/// Applies surgical STEP patches to `original_ifc` bytes, preserving untouched bytes verbatim.
pub fn patch_step_file_with(
    original_ifc: &[u8],
    patches: &[StepPatch],
    mut options: PatchOptions<'_>,
) -> Result<(Vec<u8>, StepPatchReport), String> {
    let mut outcomes = Vec::with_capacity(patches.len());
    if patches.is_empty() {
        return Ok((
            original_ifc.to_vec(),
            StepPatchReport {
                applied: 0,
                skipped: 0,
                first_new_id: 0,
                last_new_id: 0,
                outcomes,
            },
        ));
    }

    let step = index(original_ifc).map_err(|e| format!("failed to index STEP file: {e}"))?;
    let schema = schema_of(&step);
    let max_id = step.ids().max().unwrap_or(0);
    let mut next_id = max_id.saturating_add(1);
    let first_new_id = next_id;

    let data_endsec_offset = find_data_endsec(&step, original_ifc)
        .ok_or_else(|| "could not locate DATA; ... ENDSEC; in STEP file".to_string())?;

    let mut resolve = |id: u32| -> Option<PlacementTransform> {
        match options.placement_resolver.as_mut() {
            Some(r) => r(id),
            None => resolve_local_placement_chain(&step, Some(id)),
        }
    };

    // Byte-range replacements inside the original file: (byte_start, byte_end, replacement_bytes).
    let mut replacements: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let mut appended = String::new();
    let mut applied = 0usize;
    let mut skipped = 0usize;
    let mut moved: BTreeSet<u32> = BTreeSet::new();
    let mut rebent: BTreeSet<u32> = BTreeSet::new();

    for patch in patches {
        let express_id = patch.express_id();
        let result: Result<(), &'static str> = (|| {
            let ent_ref = step.get(express_id).ok_or("not-found")?;
            let entity = step.decode_ref(ent_ref).map_err(|_| "not-found")?;
            let actual_guid = match entity.params.first() {
                Some(Value::String(g)) => g.as_str(),
                _ => return Err("not-a-product"),
            };
            match patch.expected_global_id() {
                Some(expected) if !actual_guid.starts_with(expected) => {
                    return Err("guid-mismatch");
                }
                None if options.require_global_id => return Err("guid-missing"),
                _ => {}
            }
            let record = &original_ifc[ent_ref.start..ent_ref.end];

            match patch {
                StepPatch::MovePlacement {
                    delta_file_units, ..
                } => {
                    if moved.contains(&express_id) {
                        return Err("duplicate-edit");
                    }
                    if !delta_file_units.iter().all(|v| v.is_finite()) {
                        return Err("invalid-points");
                    }
                    // IfcProduct attribute index 5 is ObjectPlacement.
                    let old_placement = match entity.params.get(5) {
                        Some(Value::Ref(id)) => Some(*id),
                        Some(Value::Null) => None,
                        _ => return Err("not-a-product"),
                    };
                    let (arg_start, arg_end) =
                        find_top_level_param_span(record, 5).ok_or("not-a-product")?;
                    let new_entities = shifted_placement_entities(
                        &step,
                        old_placement,
                        *delta_file_units,
                        &mut resolve,
                        &mut next_id,
                    )
                    .ok_or("unsupported-placement")?;
                    if let Some(note) = patch.note() {
                        push_note(&mut appended, note);
                    }
                    appended.push_str(&new_entities.text);
                    replacements.push((
                        ent_ref.start + arg_start,
                        ent_ref.start + arg_end,
                        format!("#{}", new_entities.placement_id).into_bytes(),
                    ));
                    moved.insert(express_id);
                    Ok(())
                }
                StepPatch::RebendSweptDisk {
                    points_file_units,
                    radius_file_units,
                    ..
                } => {
                    if rebent.contains(&express_id) {
                        return Err("duplicate-edit");
                    }
                    if !points_file_units.iter().flatten().all(|v| v.is_finite()) {
                        return Err("invalid-points");
                    }
                    // Repeated vertices give a sweep no direction; a polyline left with fewer
                    // than two distinct points (drawn at one spot) would delete the geometry.
                    let points_file_units = distinct_points(points_file_units);
                    if points_file_units.len() < 2 {
                        return Err("invalid-points");
                    }
                    let radius = if radius_file_units.is_finite() && *radius_file_units > 0.0 {
                        *radius_file_units
                    } else {
                        resolve_existing_bar_radius(&step, express_id).ok_or("invalid-radius")?
                    };
                    if !radius.is_finite() || radius <= 0.0 {
                        return Err("invalid-radius");
                    }
                    let ctx_id =
                        body_context_for(&step, &entity.params).ok_or("no-representation-context")?;
                    // World points into the product's own placement frame.
                    let (r_elem, t_elem) = match entity.params.get(5) {
                        Some(Value::Ref(id)) => resolve(*id).ok_or("unsupported-placement")?,
                        _ => (IDENTITY_ROT3, [0.0; 3]),
                    };
                    let (arg_start, arg_end) =
                        find_top_level_param_span(record, 6).ok_or("not-a-product")?;
                    let local: Vec<[f64; 3]> = points_file_units
                        .iter()
                        .map(|p| {
                            inv_rotate_vec3(
                                r_elem,
                                [p[0] - t_elem[0], p[1] - t_elem[1], p[2] - t_elem[2]],
                            )
                        })
                        .collect();
                    if let Some(note) = patch.note() {
                        push_note(&mut appended, note);
                    }
                    let others = other_representations(&step, &entity.params);
                    let prod_shape_id = append_swept_disk(
                        &mut appended,
                        &mut next_id,
                        schema,
                        ctx_id,
                        &local,
                        radius,
                        &others,
                    );
                    replacements.push((
                        ent_ref.start + arg_start,
                        ent_ref.start + arg_end,
                        format!("#{prod_shape_id}").into_bytes(),
                    ));
                    rebent.insert(express_id);
                    Ok(())
                }
            }
        })();
        match result {
            Ok(()) => {
                applied += 1;
                outcomes.push(PatchOutcome {
                    express_id,
                    applied: true,
                    reason: None,
                });
            }
            Err(reason) => {
                skipped += 1;
                outcomes.push(PatchOutcome {
                    express_id,
                    applied: false,
                    reason: Some(reason),
                });
            }
        }
    }

    // Insert appended entities immediately before DATA;'s ENDSEC;.
    if !appended.is_empty() {
        replacements.push((data_endsec_offset, data_endsec_offset, appended.into_bytes()));
    }

    // Apply byte-range replacements in ascending byte order. Placement and representation
    // spans of one product never overlap, and each product is patched once per kind.
    replacements.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = Vec::with_capacity(original_ifc.len() + 512);
    let mut cursor = 0usize;
    for (start, end, bytes) in replacements {
        if start < cursor || end > original_ifc.len() {
            return Err("internal: overlapping STEP patch ranges".to_string());
        }
        out.extend_from_slice(&original_ifc[cursor..start]);
        out.extend_from_slice(&bytes);
        cursor = end;
    }
    out.extend_from_slice(&original_ifc[cursor..]);

    let last_new_id = if next_id > first_new_id {
        next_id - 1
    } else {
        0
    };

    Ok((
        out,
        StepPatchReport {
            applied,
            skipped,
            first_new_id: if next_id > first_new_id { first_new_id } else { 0 },
            last_new_id,
            outcomes,
        },
    ))
}

/// Every `/* GUBIM_F5 ... */` note found in a STEP file, in file order (without the delimiters).
pub fn find_patch_notes(bytes: &[u8]) -> Vec<String> {
    const OPEN: &[u8] = b"/* GUBIM_F5 ";
    let mut notes = Vec::new();
    let mut at = 0;
    while let Some(rel) = find_bytes(&bytes[at..], OPEN) {
        let start = at + rel + 3;
        let Some(end_rel) = find_bytes(&bytes[start..], b"*/") else {
            break;
        };
        notes.push(String::from_utf8_lossy(&bytes[start..start + end_rel]).trim().to_string());
        at = start + end_rel + 2;
    }
    notes
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn push_note(out: &mut String, note: &str) {
    let clean: String = note
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '_' })
        .collect::<String>()
        .replace("*/", "*_");
    out.push_str("/* ");
    out.push_str(&clean);
    out.push_str(" */\r\n");
}

fn fmt_real(v: f64) -> String {
    // Normalise -0 so the output stays tidy; 6 decimals is a micrometre in metres.
    let v = if v.abs() < 5e-7 { 0.0 } else { v };
    format!("{v:.6}")
}

/// A representation of the product's current shape, as the rebent shape takes it over.
enum OldRepresentation {
    /// The `Body`: replaced by the new swept disk.
    Body,
    /// The `Axis` (a bar's centreline): drawn again along the new curve, in its own context.
    Axis { ctx_id: u32, rep_type: String },
    /// Any other representation, kept as it is.
    Keep(u32),
}

/// The representations of the product's current `IfcProductDefinitionShape`, in order.
fn other_representations(step: &StepIndex<'_>, product_params: &[Value]) -> Vec<OldRepresentation> {
    let Some(Value::Ref(shape_id)) = product_params.get(6) else {
        return Vec::new();
    };
    let Ok(shape) = step.decode(*shape_id) else {
        return Vec::new();
    };
    let Some(Value::List(reps)) = shape.params.get(2) else {
        return Vec::new();
    };
    reps.iter()
        .filter_map(|rep| match rep {
            Value::Ref(id) => Some(*id),
            _ => None,
        })
        .map(|id| {
            let Ok(rep) = step.decode(id) else {
                return OldRepresentation::Keep(id);
            };
            let label = match rep.params.get(1) {
                Some(Value::String(s)) => s.to_ascii_lowercase(),
                _ => String::new(),
            };
            match (label.as_str(), rep.params.first()) {
                ("body", _) => OldRepresentation::Body,
                ("axis", Some(Value::Ref(ctx_id))) => OldRepresentation::Axis {
                    ctx_id: *ctx_id,
                    rep_type: match rep.params.get(2) {
                        Some(Value::String(t)) if t.chars().all(|c| c.is_ascii_alphanumeric()) => {
                            t.clone()
                        }
                        _ => "Curve3D".to_string(),
                    },
                },
                _ => OldRepresentation::Keep(id),
            }
        })
        .collect()
}

/// Appends the new swept disk and the `IfcProductDefinitionShape` that holds it. The other
/// representations of the old shape stay: an `Axis` is drawn again along the new curve, and
/// anything else is kept as it is.
fn append_swept_disk(
    out: &mut String,
    next_id: &mut u32,
    schema: Schema,
    ctx_id: u32,
    local: &[[f64; 3]],
    radius: f64,
    old: &[OldRepresentation],
) -> u32 {
    let mut take = || {
        let id = *next_id;
        *next_id += 1;
        id
    };
    let curve_id;
    // IFC2X3 requires StartParam/EndParam (polyline parameter = segment index); later schemas
    // make them optional.
    let params = if schema == Schema::Ifc2x3 {
        format!("0.,{}.", local.len().saturating_sub(1))
    } else {
        "$,$".to_string()
    };
    if schema == Schema::Ifc2x3 {
        // IFC2X3 has neither IfcCartesianPointList3D nor IfcIndexedPolyCurve, and its
        // IfcSweptDiskSolid requires StartParam/EndParam (polyline parameter = segment index).
        let mut pt_ids = Vec::with_capacity(local.len());
        for p in local {
            let id = take();
            out.push_str(&format!(
                "#{id}=IFCCARTESIANPOINT(({},{},{}));\r\n",
                fmt_real(p[0]),
                fmt_real(p[1]),
                fmt_real(p[2])
            ));
            pt_ids.push(format!("#{id}"));
        }
        curve_id = take();
        out.push_str(&format!("#{curve_id}=IFCPOLYLINE(({}));\r\n", pt_ids.join(",")));
    } else {
        let list_id = take();
        let coords = local
            .iter()
            .map(|p| format!("({},{},{})", fmt_real(p[0]), fmt_real(p[1]), fmt_real(p[2])))
            .collect::<Vec<_>>()
            .join(",");
        // IFC4X3 adds the optional TagList attribute to IfcCartesianPointList3D.
        let tag = if schema == Schema::Ifc4x3 { ",$" } else { "" };
        out.push_str(&format!("#{list_id}=IFCCARTESIANPOINTLIST3D(({coords}){tag});\r\n"));
        curve_id = take();
        out.push_str(&format!("#{curve_id}=IFCINDEXEDPOLYCURVE(#{list_id},$,.F.);\r\n"));
    }
    let solid_id = take();
    out.push_str(&format!(
        "#{solid_id}=IFCSWEPTDISKSOLID(#{curve_id},{},$,{params});\r\n",
        fmt_real(radius)
    ));
    let rep_id = take();
    out.push_str(&format!(
        "#{rep_id}=IFCSHAPEREPRESENTATION(#{ctx_id},'Body','AdvancedSweptSolid',(#{solid_id}));\r\n"
    ));
    let mut reps = Vec::with_capacity(old.len() + 1);
    let mut body_written = false;
    for item in old {
        match item {
            OldRepresentation::Body if !body_written => {
                reps.push(rep_id);
                body_written = true;
            }
            OldRepresentation::Body => {}
            OldRepresentation::Axis { ctx_id, rep_type } => {
                let axis_id = take();
                out.push_str(&format!(
                    "#{axis_id}=IFCSHAPEREPRESENTATION(#{ctx_id},'Axis','{rep_type}',(#{curve_id}));\r\n"
                ));
                reps.push(axis_id);
            }
            OldRepresentation::Keep(id) => reps.push(*id),
        }
    }
    if !body_written {
        reps.insert(0, rep_id);
    }
    let list = reps.iter().map(|id| format!("#{id}")).collect::<Vec<_>>().join(",");
    let prod_id = take();
    out.push_str(&format!("#{prod_id}=IFCPRODUCTDEFINITIONSHAPE($,$,({list}));\r\n"));
    prod_id
}

struct NewPlacement {
    text: String,
    placement_id: u32,
}

/// Builds a new `IfcLocalPlacement` that reproduces `old_placement` shifted by `delta_world`.
fn shifted_placement_entities(
    step: &StepIndex<'_>,
    old_placement: Option<u32>,
    delta_world: [f64; 3],
    resolve: &mut dyn FnMut(u32) -> Option<PlacementTransform>,
    next_id: &mut u32,
) -> Option<NewPlacement> {
    let mut take = || {
        let id = *next_id;
        *next_id += 1;
        id
    };
    let mut text = String::new();
    let point = |text: &mut String, id: u32, p: [f64; 3]| {
        text.push_str(&format!(
            "#{id}=IFCCARTESIANPOINT(({},{},{}));\r\n",
            fmt_real(p[0]),
            fmt_real(p[1]),
            fmt_real(p[2])
        ));
    };

    // Same parent and same axes, location shifted: only when the old placement is a plain
    // IfcLocalPlacement over an IfcAxis2Placement3D.
    if let Some(pid) = old_placement
        && let Ok(place) = step.decode(pid)
        && place.type_name.eq_ignore_ascii_case("IFCLOCALPLACEMENT")
        && let Some(Value::Ref(axis_id)) = place.params.get(1)
        && let Ok(axis) = step.decode(*axis_id)
        && axis.type_name.eq_ignore_ascii_case("IFCAXIS2PLACEMENT3D")
    {
        let parent = match place.params.first() {
            Some(Value::Ref(p)) => Some(*p),
            _ => None,
        };
        let r_parent = match parent {
            Some(p) => resolve(p)?.0,
            None => IDENTITY_ROT3,
        };
        let delta_local = inv_rotate_vec3(r_parent, delta_world);
        let mut loc = [0.0; 3];
        if let Some(Value::Ref(pt)) = axis.params.first() {
            loc = step.get(*pt).and_then(|r| step.decode_coordinates(r)).map(|(c, _)| c)?;
        }
        let ref_of = |i: usize| match axis.params.get(i) {
            Some(Value::Ref(r)) => format!("#{r}"),
            _ => "$".to_string(),
        };
        let pt_id = take();
        let axis_new = take();
        let place_id = take();
        point(
            &mut text,
            pt_id,
            [loc[0] + delta_local[0], loc[1] + delta_local[1], loc[2] + delta_local[2]],
        );
        text.push_str(&format!(
            "#{axis_new}=IFCAXIS2PLACEMENT3D(#{pt_id},{},{});\r\n",
            ref_of(1),
            ref_of(2)
        ));
        let parent_str = parent.map_or_else(|| "$".to_string(), |p| format!("#{p}"));
        text.push_str(&format!("#{place_id}=IFCLOCALPLACEMENT({parent_str},#{axis_new});\r\n"));
        return Some(NewPlacement {
            text,
            placement_id: place_id,
        });
    }

    // Anything else (no placement, IfcLinearPlacement, IfcGridPlacement, 2D axes): nest a
    // pure translation under the old placement, expressed in its resolved world frame.
    let r_self = match old_placement {
        Some(pid) => resolve(pid)?.0,
        None => IDENTITY_ROT3,
    };
    let delta_self = inv_rotate_vec3(r_self, delta_world);
    let pt_id = take();
    let axis_new = take();
    let place_id = take();
    point(&mut text, pt_id, delta_self);
    text.push_str(&format!("#{axis_new}=IFCAXIS2PLACEMENT3D(#{pt_id},$,$);\r\n"));
    let rel = old_placement.map_or_else(|| "$".to_string(), |p| format!("#{p}"));
    text.push_str(&format!("#{place_id}=IFCLOCALPLACEMENT({rel},#{axis_new});\r\n"));
    Some(NewPlacement {
        text,
        placement_id: place_id,
    })
}

/// The representation context for a new Body: the context of the product's current `Body`
/// representation, else the `Body` subcontext, else a `Model` context.
fn body_context_for(step: &StepIndex<'_>, product_params: &[Value]) -> Option<u32> {
    let label = |v: Option<&Value>| match v {
        Some(Value::String(s)) => Some(s.to_ascii_lowercase()),
        _ => None,
    };
    if let Some(Value::Ref(shape_id)) = product_params.get(6)
        && let Ok(shape) = step.decode(*shape_id)
        && let Some(Value::List(reps)) = shape.params.get(2)
    {
        for rep in reps {
            if let Value::Ref(rep_id) = rep
                && let Ok(rep) = step.decode(*rep_id)
                && label(rep.params.get(1)).as_deref() == Some("body")
                && let Some(Value::Ref(ctx)) = rep.params.first()
            {
                return Some(*ctx);
            }
        }
    }
    let subs: Vec<u32> = step
        .ids_by_type("IFCGEOMETRICREPRESENTATIONSUBCONTEXT")
        .collect();
    for &id in &subs {
        if let Ok(ctx) = step.decode(id)
            && label(ctx.params.first()).as_deref() == Some("body")
        {
            return Some(id);
        }
    }
    let contexts: Vec<u32> = step.ids_by_type("IFCGEOMETRICREPRESENTATIONCONTEXT").collect();
    for &id in &contexts {
        if let Ok(ctx) = step.decode(id)
            && label(ctx.params.get(1)).as_deref() == Some("model")
        {
            return Some(id);
        }
    }
    contexts.first().copied()
}

const IDENTITY_ROT3: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[inline]
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn normalize3(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = dot3(v, v).sqrt();
    (len.is_finite() && len > 1e-12).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

#[inline]
fn rotate_vec3(cols: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        cols[0][0] * v[0] + cols[1][0] * v[1] + cols[2][0] * v[2],
        cols[0][1] * v[0] + cols[1][1] * v[1] + cols[2][1] * v[2],
        cols[0][2] * v[0] + cols[1][2] * v[1] + cols[2][2] * v[2],
    ]
}

#[inline]
/// The points without consecutive repeats (closer than a part in 10¹² of the coordinates).
fn distinct_points(points: &[[f64; 3]]) -> Vec<[f64; 3]> {
    let size = points
        .iter()
        .flatten()
        .fold(0.0_f64, |m, v| m.max(v.abs()));
    let tol = (size * 1e-12).max(1e-12);
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(points.len());
    for p in points {
        let repeat = out.last().is_some_and(|q| {
            let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= tol
        });
        if !repeat {
            out.push(*p);
        }
    }
    out
}

fn inv_rotate_vec3(cols: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [dot3(cols[0], v), dot3(cols[1], v), dot3(cols[2], v)]
}

#[inline]
fn mul_rot3(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [
        rotate_vec3(a, b[0]),
        rotate_vec3(a, b[1]),
        rotate_vec3(a, b[2]),
    ]
}

fn decode_direction3(step: &StepIndex<'_>, id: u32) -> Option<[f64; 3]> {
    let r = step.get(id)?;
    if let Some((coords, _)) = step.decode_coordinates(r) {
        return Some(coords);
    }
    let ent = step.decode_ref(r).ok()?;
    let Value::List(items) = ent.params.first()? else {
        return None;
    };
    Some([
        items.first().and_then(value_as_f64)?,
        items.get(1).and_then(value_as_f64)?,
        items.get(2).and_then(value_as_f64).unwrap_or(0.0),
    ])
}

/// `IfcAxis2Placement3D` or `IfcAxis2Placement2D` as (rotation columns, location).
fn decode_axis2_placement(step: &StepIndex<'_>, id: u32) -> Option<PlacementTransform> {
    let axis_ent = step.decode(id).ok()?;
    let is_3d = axis_ent.type_name.eq_ignore_ascii_case("IFCAXIS2PLACEMENT3D");
    let is_2d = axis_ent.type_name.eq_ignore_ascii_case("IFCAXIS2PLACEMENT2D");
    if !is_3d && !is_2d {
        return None;
    }
    let mut loc = [0.0, 0.0, 0.0];
    if let Some(Value::Ref(pt_id)) = axis_ent.params.first() {
        let (coords, count) = step.get(*pt_id).and_then(|r| step.decode_coordinates(r))?;
        loc = if count == 3 { coords } else { [coords[0], coords[1], 0.0] };
    }
    let dir_at = |i: usize| match axis_ent.params.get(i) {
        Some(Value::Ref(r)) => decode_direction3(step, *r),
        _ => None,
    };
    let (z, x_hint) = if is_3d {
        (
            dir_at(1).and_then(normalize3).unwrap_or([0.0, 0.0, 1.0]),
            dir_at(2).unwrap_or([1.0, 0.0, 0.0]),
        )
    } else {
        let d = dir_at(1).unwrap_or([1.0, 0.0, 0.0]);
        ([0.0, 0.0, 1.0], [d[0], d[1], 0.0])
    };
    let proj = dot3(x_hint, z);
    let x = normalize3([
        x_hint[0] - z[0] * proj,
        x_hint[1] - z[1] * proj,
        x_hint[2] - z[2] * proj,
    ])
    .or_else(|| {
        let fallback = if z[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let p = dot3(fallback, z);
        normalize3([
            fallback[0] - z[0] * p,
            fallback[1] - z[1] * p,
            fallback[2] - z[2] * p,
        ])
    })?;
    let y = cross3(z, x);
    Some(([x, y, z], loc))
}

/// World transform of an `IfcLocalPlacement` chain in file units. `None` when the chain holds
/// any other placement type (linear, grid), an unreadable axis, or is cyclic.
fn resolve_local_placement_chain(
    step: &StepIndex<'_>,
    placement_id: Option<u32>,
) -> Option<PlacementTransform> {
    let mut chain = Vec::new();
    let mut cur = placement_id;
    while let Some(pid) = cur {
        if chain.len() >= 64 {
            return None;
        }
        let ent = step.decode(pid).ok()?;
        if !ent.type_name.eq_ignore_ascii_case("IFCLOCALPLACEMENT") {
            return None;
        }
        let rel = match ent.params.get(1) {
            Some(Value::Ref(axis_id)) => decode_axis2_placement(step, *axis_id)?,
            _ => (IDENTITY_ROT3, [0.0, 0.0, 0.0]),
        };
        chain.push(rel);
        cur = match ent.params.first() {
            Some(Value::Ref(parent_id)) => Some(*parent_id),
            _ => None,
        };
    }
    let mut r_world = IDENTITY_ROT3;
    let mut t_world = [0.0, 0.0, 0.0];
    for (r_rel, t_rel) in chain.into_iter().rev() {
        let shifted = rotate_vec3(r_world, t_rel);
        t_world = [
            t_world[0] + shifted[0],
            t_world[1] + shifted[1],
            t_world[2] + shifted[2],
        ];
        r_world = mul_rot3(r_world, r_rel);
    }
    Some((r_world, t_world))
}

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(n) => Some(*n),
        Value::Integer(n) => Some(*n as f64),
        Value::Typed { value, .. } => value_as_f64(value),
        _ => None,
    }
}

/// Bar radius in file units for a Level 2 rebend called with `radius_file_units <= 0.0`.
fn resolve_existing_bar_radius(step: &StepIndex<'_>, express_id: u32) -> Option<f64> {
    let ent = step.decode(express_id).ok()?;
    // 1. NominalDiameter: attribute 9 on IfcReinforcingBar, 10 on IfcTendon (after PredefinedType).
    let diameter_index = if ent.type_name.eq_ignore_ascii_case("IFCREINFORCINGBAR") {
        Some(9)
    } else if ent.type_name.eq_ignore_ascii_case("IFCTENDON") {
        Some(10)
    } else {
        None
    };
    if let Some(i) = diameter_index
        && let Some(d) = ent.params.get(i).and_then(value_as_f64)
        && d.is_finite()
        && d > 0.0
    {
        return Some(d * 0.5);
    }
    // 2. Walk Representation -> Representations -> Items for IfcSweptDiskSolid or profile radii.
    let Value::Ref(prod_shape_id) = ent.params.get(6)? else {
        return None;
    };
    let prod_shape = step.decode(*prod_shape_id).ok()?;
    let Value::List(reps) = prod_shape.params.get(2)? else {
        return None;
    };
    for rep_val in reps {
        let Value::Ref(rep_id) = rep_val else { continue };
        let Ok(rep) = step.decode(*rep_id) else { continue };
        let Some(Value::List(items)) = rep.params.get(3) else {
            continue;
        };
        for item_val in items {
            let Value::Ref(item_id) = item_val else { continue };
            if let Some(r) = inspect_item_radius(step, *item_id, 0) {
                return Some(r);
            }
        }
    }
    None
}

fn inspect_item_radius(step: &StepIndex<'_>, item_id: u32, depth: usize) -> Option<f64> {
    if depth > 8 {
        return None;
    }
    let item = step.decode(item_id).ok()?;
    if item.type_name.eq_ignore_ascii_case("IFCSWEPTDISKSOLID")
        || item
            .type_name
            .eq_ignore_ascii_case("IFCSWEPTDISKSOLIDPOLYGONAL")
    {
        return item
            .params
            .get(1)
            .and_then(value_as_f64)
            .filter(|r| r.is_finite() && *r > 0.0);
    }
    if item.type_name.eq_ignore_ascii_case("IFCEXTRUDEDAREASOLID")
        && let Some(Value::Ref(prof_id)) = item.params.first()
        && let Ok(prof) = step.decode(*prof_id)
    {
        if prof.type_name.eq_ignore_ascii_case("IFCCIRCLEPROFILEDEF")
            || prof
                .type_name
                .eq_ignore_ascii_case("IFCCIRCLEHOLLOWPROFILEDEF")
        {
            return prof
                .params
                .get(3)
                .and_then(value_as_f64)
                .filter(|r| r.is_finite() && *r > 0.0);
        }
        if prof.type_name.eq_ignore_ascii_case("IFCRECTANGLEPROFILEDEF") {
            let x = prof.params.get(3).and_then(value_as_f64)?;
            let y = prof.params.get(4).and_then(value_as_f64)?;
            let r = 0.5 * x.min(y);
            if r.is_finite() && r > 0.0 {
                return Some(r);
            }
        }
    }
    if item.type_name.eq_ignore_ascii_case("IFCMAPPEDITEM")
        && let Some(Value::Ref(map_id)) = item.params.first()
        && let Ok(map) = step.decode(*map_id)
        && let Some(Value::Ref(rep_id)) = map.params.get(1)
        && let Ok(rep) = step.decode(*rep_id)
        && let Some(Value::List(sub_items)) = rep.params.get(3)
    {
        for sub in sub_items {
            if let Value::Ref(sid) = sub
                && let Some(r) = inspect_item_radius(step, *sid, depth + 1)
            {
                return Some(r);
            }
        }
    }
    None
}

/// Byte offset of the `ENDSEC` that closes `DATA;`: the first one after the last entity record,
/// so strings inside entities can never be mistaken for it.
fn find_data_endsec(step: &StepIndex<'_>, bytes: &[u8]) -> Option<usize> {
    let last_end = step
        .ids()
        .filter_map(|id| step.get(id).map(|r| r.end))
        .max()?;
    let tail = bytes.get(last_end..)?;
    let rel = tail
        .windows(6)
        .position(|w| w.eq_ignore_ascii_case(b"ENDSEC"))?;
    Some(last_end + rel)
}

/// Finds the byte span `(start, end)` of top-level argument `target_index` inside an entity body
/// slice (e.g. `IFCWALL('guid',#2,'Name',$,$,#10,#20,$)`).
fn find_top_level_param_span(record: &[u8], target_index: usize) -> Option<(usize, usize)> {
    let open = record.iter().position(|&b| b == b'(')?;
    let mut i = open + 1;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut arg_idx = 0usize;
    let mut arg_start = i;

    while i < record.len() {
        let b = record[i];
        if in_str {
            if b == b'\'' {
                if record.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => in_str = true,
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    if arg_idx == target_index {
                        return Some((arg_start, i));
                    }
                    return None;
                }
                depth -= 1;
            }
            b',' if depth == 0 => {
                if arg_idx == target_index {
                    return Some((arg_start, i));
                }
                arg_idx += 1;
                arg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "ISO-10303-21;\r\nHEADER;\r\nFILE_DESCRIPTION((''),'2;1');\r\nFILE_NAME('t','',(''),(''),'','','');\r\nFILE_SCHEMA(('@SCHEMA@'));\r\nENDSEC;\r\nDATA;\r\n\
#1=IFCPROJECT('0000000000000000000001',$,'P',$,$,$,$,(#10),$);\r\n\
#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#11,$);\r\n\
#11=IFCAXIS2PLACEMENT3D(#12,$,$);\r\n\
#12=IFCCARTESIANPOINT((0.,0.,0.));\r\n\
#13=IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Axis','Model',*,*,*,*,#10,$,.GRAPH_VIEW.,$);\r\n\
#14=IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#10,$,.MODEL_VIEW.,$);\r\n\
#20=IFCLOCALPLACEMENT($,#11);\r\n\
#30=IFCPOLYLINE((#12,#31));\r\n\
#31=IFCCARTESIANPOINT((1.,0.,0.));\r\n\
#32=IFCSWEPTDISKSOLID(#30,0.01,$,$,$);\r\n\
#33=IFCSHAPEREPRESENTATION(#14,'Body','AdvancedSweptSolid',(#32));\r\n\
#34=IFCPRODUCTDEFINITIONSHAPE($,$,(#33));\r\n\
#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'Bar ''ENDSEC;''',$,$,#20,#34,$,$,0.02,$,$,$,$);\r\n\
ENDSEC;\r\nEND-ISO-10303-21;\r\n";

    fn file(schema: &str) -> String {
        BASE.replace("@SCHEMA@", schema)
    }

    fn rebend(guid: Option<&str>) -> StepPatch {
        StepPatch::RebendSweptDisk {
            express_id: 100,
            points_file_units: vec![[0.0, 0.0, 0.3], [1.0, 0.0, 0.3]],
            radius_file_units: 0.0,
            expected_global_id: guid.map(str::to_string),
            note: Some("GUBIM_F5 test".to_string()),
        }
    }

    #[test]
    fn rebend_uses_body_context_and_schema_specific_entities() {
        for (schema, list, tail) in [
            ("IFC4", true, "));"),
            ("IFC4X3_ADD2", true, "),$);"),
            ("IFC2X3", false, ""),
        ] {
            let src = file(schema);
            let (out, report) =
                patch_step_file(src.as_bytes(), &[rebend(Some("2O2Fr$t4X7Zf8NOew3FLOH"))]).unwrap();
            assert_eq!(report.applied, 1, "{schema}");
            let text = String::from_utf8(out).unwrap();
            let added = text[src.find("ENDSEC;\r\nEND").unwrap()..].trim_start();
            assert!(added.contains("IFCSHAPEREPRESENTATION(#14,'Body','AdvancedSweptSolid'"), "{schema}: {added}");
            assert_eq!(added.contains("IFCCARTESIANPOINTLIST3D"), list, "{schema}");
            assert_eq!(added.contains("IFCINDEXEDPOLYCURVE"), list, "{schema}");
            if list {
                let line = added.lines().find(|l| l.contains("POINTLIST3D")).unwrap();
                assert!(line.ends_with(tail), "{schema}: {line}");
            } else {
                assert!(added.contains("=IFCPOLYLINE(("));
                assert!(added.contains(",0.010000,$,0.,1.);"), "{added}");
            }
            assert!(added.starts_with("/* GUBIM_F5 test */"), "{added}");
            let step = index(text.as_bytes()).expect("patched file indexes");
            assert!(step.skipped().is_empty());
            assert_eq!(find_patch_notes(text.as_bytes()), vec!["GUBIM_F5 test".to_string()]);
        }
    }

    #[test]
    fn rebend_keeps_other_representations_and_redraws_the_axis() {
        for schema in ["IFC2X3", "IFC4", "IFC4X3_ADD2"] {
            let src = file(schema)
                .replace(
                    "#34=IFCPRODUCTDEFINITIONSHAPE($,$,(#33));",
                    "#35=IFCSHAPEREPRESENTATION(#13,'Axis','Curve3D',(#30));\r\n\
#36=IFCSHAPEREPRESENTATION(#10,'Box','BoundingBox',(#37));\r\n\
#37=IFCBOUNDINGBOX(#12,1.,0.02,0.02);\r\n\
#34=IFCPRODUCTDEFINITIONSHAPE($,$,(#35,#33,#36));",
                );
            let (out, report) =
                patch_step_file(src.as_bytes(), &[rebend(Some("2O2Fr$t4X7Zf8NOew3FLOH"))]).unwrap();
            assert_eq!(report.applied, 1, "{schema}");
            let text = String::from_utf8(out).unwrap();
            let step = index(text.as_bytes()).expect("patched file indexes");
            let bar = step.decode(100).unwrap();
            let Some(Value::Ref(shape_id)) = bar.params.get(6) else { panic!("{schema}") };
            let shape = step.decode(*shape_id).unwrap();
            let Some(Value::List(reps)) = shape.params.get(2) else { panic!("{schema}") };
            let ids: Vec<u32> = reps
                .iter()
                .map(|r| match r {
                    Value::Ref(id) => *id,
                    _ => panic!("{schema}"),
                })
                .collect();
            assert_eq!(ids.len(), 3, "{schema}: {ids:?}");
            // Order kept: Axis, Body, Box.
            let axis = step.decode(ids[0]).unwrap();
            let body = step.decode(ids[1]).unwrap();
            assert_eq!(ids[2], 36, "{schema}: the Box is kept as it is");
            assert_eq!(axis.params.first(), Some(&Value::Ref(13)), "{schema}");
            assert_eq!(axis.params.get(1), Some(&Value::String("Axis".into())), "{schema}");
            assert_eq!(axis.params.get(2), Some(&Value::String("Curve3D".into())), "{schema}");
            assert_eq!(body.params.get(1), Some(&Value::String("Body".into())), "{schema}");
            // The new Axis follows the new directrix, not the old curve #30.
            let Some(Value::List(items)) = body.params.get(3) else { panic!("{schema}") };
            let Some(Value::Ref(solid_id)) = items.first() else { panic!("{schema}") };
            let solid = step.decode(*solid_id).unwrap();
            let Some(Value::List(axis_items)) = axis.params.get(3) else { panic!("{schema}") };
            assert_eq!(axis_items.first(), solid.params.first(), "{schema}");
            assert_ne!(axis_items.first(), Some(&Value::Ref(30)), "{schema}");
            assert!(step.skipped().is_empty(), "{schema}");
        }
    }

    #[test]
    fn guid_mismatch_and_missing_guid_are_skipped() {
        let src = file("IFC4");
        let (out, report) = patch_step_file(src.as_bytes(), &[rebend(Some("3F5e2eTestB"))]).unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.outcomes[0].reason, Some("guid-mismatch"));
        assert_eq!(out, src.as_bytes());

        let (_, report) = patch_step_file_with(
            src.as_bytes(),
            &[rebend(None)],
            PatchOptions {
                require_global_id: true,
                ..PatchOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.outcomes[0].reason, Some("guid-missing"));

        // A truncated block-name prefix is accepted.
        let (_, report) = patch_step_file(src.as_bytes(), &[rebend(Some("2O2Fr$t4X7Zf8"))]).unwrap();
        assert_eq!(report.applied, 1);
    }

    #[test]
    fn linear_placement_without_resolver_is_refused_and_with_resolver_is_rotated() {
        let src = file("IFC4X3_ADD2").replace(
            "#20=IFCLOCALPLACEMENT($,#11);",
            "#20=IFCLINEARPLACEMENT($,#21,$);\r\n#21=IFCAXIS2PLACEMENTLINEAR(#12,$,$);",
        );
        let mv = StepPatch::MovePlacement {
            express_id: 100,
            delta_file_units: [0.0, 0.0, 0.3],
            expected_global_id: Some("2O2Fr$t4X7Zf8NOew3FLOH".into()),
            note: None,
        };
        let (_, report) = patch_step_file(src.as_bytes(), std::slice::from_ref(&mv)).unwrap();
        assert_eq!(report.outcomes[0].reason, Some("unsupported-placement"));

        // Resolver says the linear placement is rotated 90 deg about X (local Y = world Z).
        let mut resolver = |id: u32| -> Option<PlacementTransform> {
            (id == 20).then_some((
                [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
                [5.0, 0.0, 0.0],
            ))
        };
        let (out, report) = patch_step_file_with(
            src.as_bytes(),
            &[mv],
            PatchOptions {
                placement_resolver: Some(&mut resolver),
                ..PatchOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.applied, 1);
        let text = String::from_utf8(out).unwrap();
        // World +Z is local +Y of the linear placement.
        assert!(text.contains("IFCCARTESIANPOINT((0.000000,0.300000,0.000000))"), "{text}");
        assert!(text.contains("=IFCLOCALPLACEMENT(#20,"));
    }

    #[test]
    fn duplicate_moves_of_one_product_are_skipped() {
        let src = file("IFC4");
        let mv = StepPatch::MovePlacement {
            express_id: 100,
            delta_file_units: [0.0, 0.0, 0.3],
            expected_global_id: None,
            note: None,
        };
        let (_, report) = patch_step_file(src.as_bytes(), &[mv.clone(), mv]).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.outcomes[1].reason, Some("duplicate-edit"));
    }

    #[test]
    fn tendon_diameter_is_read_after_predefined_type() {
        let src = file("IFC4").replace(
            "#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'Bar ''ENDSEC;''',$,$,#20,#34,$,$,0.02,$,$,$,$);",
            "#100=IFCTENDON('2O2Fr$t4X7Zf8NOew3FLOH',$,'T',$,$,#20,#34,$,$,.STRAND.,0.08,$,$,$,$,$,$);",
        );
        let step = index(src.as_bytes()).unwrap();
        assert_eq!(resolve_existing_bar_radius(&step, 100), Some(0.04));
    }
}
