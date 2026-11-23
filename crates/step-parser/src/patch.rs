//! Surgical ISO 10303-21 (STEP) IFC patcher for Phase F5 (`docs/12_clash_alignment_dxf.md` §3.5).
//!
//! Preserves >99.99% of the original IFC file byte-for-byte: all headers, GUIDs, property sets,
//! concrete, tendons, alignments, and untouched entities remain completely unchanged.
//! Only appends newly created entities immediately before `ENDSEC;` in `DATA;` and updates the
//! single `#ref` attribute (`ObjectPlacement` for Level 1 moves, `Representation` for Level 2
//! rebends) on each modified product entity.

use crate::{StepIndex, Value, index};

/// One surgical edit to apply to an IFC STEP file.
#[derive(Debug, Clone, PartialEq)]
pub enum StepPatch {
    /// Level 1 (`MOVE`): translates `IfcProduct.ObjectPlacement` by `delta_file_units` (in the
    /// IFC file's own length units, e.g. metres or millimetres).
    MovePlacement {
        express_id: u32,
        delta_file_units: [f64; 3],
    },
    /// Level 2 (`Rebend`): replaces `IfcProduct.Representation` with a new `IFCSWEPTDISKSOLID`
    /// swept along `points_file_units` with `radius_file_units`.
    RebendSweptDisk {
        express_id: u32,
        points_file_units: Vec<[f64; 3]>,
        radius_file_units: f64,
    },
}

/// Summary of applied surgical STEP patches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepPatchReport {
    pub applied: usize,
    pub skipped: usize,
    pub first_new_id: u32,
    pub last_new_id: u32,
}

/// Applies surgical STEP patches to `original_ifc` bytes, preserving untouched bytes verbatim.
pub fn patch_step_file(
    original_ifc: &[u8],
    patches: &[StepPatch],
) -> Result<(Vec<u8>, StepPatchReport), String> {
    if patches.is_empty() {
        return Ok((
            original_ifc.to_vec(),
            StepPatchReport {
                applied: 0,
                skipped: 0,
                first_new_id: 0,
                last_new_id: 0,
            },
        ));
    }

    let step = index(original_ifc).map_err(|e| format!("failed to index STEP file: {e}"))?;
    let max_id = step.ids().max().unwrap_or(0);
    let mut next_id = max_id.saturating_add(1);
    let first_new_id = next_id;

    let data_endsec_offset = find_data_endsec(original_ifc)
        .ok_or_else(|| "could not locate DATA; ... ENDSEC; in STEP file".to_string())?;

    // Find an existing geometric representation context for Level 2 rebends.
    let default_ctx_id = step
        .ids_by_type("IFCGEOMETRICREPRESENTATIONSUBCONTEXT")
        .next()
        .or_else(|| step.ids_by_type("IFCGEOMETRICREPRESENTATIONCONTEXT").next());

    // Byte-range replacements inside the original file: (byte_start, byte_end, replacement_bytes).
    let mut replacements: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let mut appended = String::new();
    let mut applied = 0usize;
    let mut skipped = 0usize;

    for patch in patches {
        match patch {
            StepPatch::MovePlacement {
                express_id,
                delta_file_units,
            } => {
                let Some(ent_ref) = step.get(*express_id) else {
                    skipped += 1;
                    continue;
                };
                let Ok(entity) = step.decode_ref(ent_ref) else {
                    skipped += 1;
                    continue;
                };
                // IfcProduct attribute index 5 is ObjectPlacement.
                let old_placement_ref = match entity.params.get(5) {
                    Some(Value::Ref(id)) => Some(*id),
                    Some(Value::Null) => None,
                    _ => {
                        skipped += 1;
                        continue;
                    }
                };
                let Some((arg_start, arg_end)) =
                    find_top_level_param_span(&original_ifc[ent_ref.start..ent_ref.end], 5)
                else {
                    skipped += 1;
                    continue;
                };

                // Compute new location: if old placement has an axis with a location, copy its
                // parent placement and axis/ref_direction while shifting location by delta!
                let (parent_ref_str, new_loc, axis_ref_str, ref_dir_str) =
                    resolve_shifted_placement(&step, old_placement_ref, *delta_file_units);

                let pt_id = next_id;
                next_id += 1;
                let axis_id = next_id;
                next_id += 1;
                let place_id = next_id;
                next_id += 1;

                appended.push_str(&format!(
                    "#{pt_id}=IFCCARTESIANPOINT(({:.6},{:.6},{:.6}));\r\n",
                    new_loc[0], new_loc[1], new_loc[2]
                ));
                appended.push_str(&format!(
                    "#{axis_id}=IFCAXIS2PLACEMENT3D(#{pt_id},{axis_ref_str},{ref_dir_str});\r\n"
                ));
                appended.push_str(&format!(
                    "#{place_id}=IFCLOCALPLACEMENT({parent_ref_str},#{axis_id});\r\n"
                ));

                replacements.push((
                    ent_ref.start + arg_start,
                    ent_ref.start + arg_end,
                    format!("#{place_id}").into_bytes(),
                ));
                applied += 1;
            }
            StepPatch::RebendSweptDisk {
                express_id,
                points_file_units,
                radius_file_units,
            } => {
                if points_file_units.len() < 2 {
                    skipped += 1;
                    continue;
                }
                let resolved_radius = if radius_file_units.is_finite() && *radius_file_units > 0.0 {
                    *radius_file_units
                } else {
                    resolve_existing_bar_radius(&step, *express_id).unwrap_or(0.01)
                };
                if !resolved_radius.is_finite() || resolved_radius <= 0.0 {
                    skipped += 1;
                    continue;
                }
                let Some(ctx_id) = default_ctx_id else {
                    skipped += 1;
                    continue;
                };
                let Some(ent_ref) = step.get(*express_id) else {
                    skipped += 1;
                    continue;
                };
                let Ok(entity) = step.decode_ref(ent_ref) else {
                    skipped += 1;
                    continue;
                };
                // IfcProduct attribute index 5 is ObjectPlacement; transform world points into
                // the product's local placement coordinates.
                let placement_ref = match entity.params.get(5) {
                    Some(Value::Ref(id)) => Some(*id),
                    _ => None,
                };
                let (r_elem, t_elem) = resolve_placement_chain_transform(&step, placement_ref);
                // IfcProduct attribute index 6 is Representation.
                let Some((arg_start, arg_end)) =
                    find_top_level_param_span(&original_ifc[ent_ref.start..ent_ref.end], 6)
                else {
                    skipped += 1;
                    continue;
                };

                let pt_list_id = next_id;
                next_id += 1;
                let curve_id = next_id;
                next_id += 1;
                let solid_id = next_id;
                next_id += 1;
                let shape_rep_id = next_id;
                next_id += 1;
                let prod_shape_id = next_id;
                next_id += 1;

                let pts_str: String = points_file_units
                    .iter()
                    .map(|p| {
                        let d = [p[0] - t_elem[0], p[1] - t_elem[1], p[2] - t_elem[2]];
                        let local = inv_rotate_vec3(r_elem, d);
                        format!("({:.6},{:.6},{:.6})", local[0], local[1], local[2])
                    })
                    .collect::<Vec<_>>()
                    .join(",");

                appended.push_str(&format!(
                    "#{pt_list_id}=IFCCARTESIANPOINTLIST3D(({pts_str}));\r\n"
                ));
                appended.push_str(&format!(
                    "#{curve_id}=IFCINDEXEDPOLYCURVE(#{pt_list_id},$,.F.);\r\n"
                ));
                appended.push_str(&format!(
                    "#{solid_id}=IFCSWEPTDISKSOLID(#{curve_id},{:.6},$,$,$);\r\n",
                    resolved_radius
                ));
                appended.push_str(&format!(
                    "#{shape_rep_id}=IFCSHAPEREPRESENTATION(#{ctx_id},'Body','SweptSolid',(#{solid_id}));\r\n"
                ));
                appended.push_str(&format!(
                    "#{prod_shape_id}=IFCPRODUCTDEFINITIONSHAPE($,$,(#{shape_rep_id}));\r\n"
                ));

                replacements.push((
                    ent_ref.start + arg_start,
                    ent_ref.start + arg_end,
                    format!("#{prod_shape_id}").into_bytes(),
                ));
                applied += 1;
            }
        }
    }

    // Insert appended entities immediately before DATA;'s ENDSEC;.
    if !appended.is_empty() {
        replacements.push((data_endsec_offset, data_endsec_offset, appended.into_bytes()));
    }

    // Apply all non-overlapping byte-range replacements in ascending byte order.
    replacements.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = Vec::with_capacity(original_ifc.len() + 512);
    let mut cursor = 0usize;
    for (start, end, bytes) in replacements {
        if start < cursor || end > original_ifc.len() {
            continue;
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
            first_new_id: if applied > 0 { first_new_id } else { 0 },
            last_new_id,
        },
    ))
}

fn resolve_shifted_placement(
    step: &StepIndex<'_>,
    old_placement_id: Option<u32>,
    delta_world: [f64; 3],
) -> (String, [f64; 3], String, String) {
    let Some(pid) = old_placement_id else {
        return ("$".to_string(), delta_world, "$".to_string(), "$".to_string());
    };
    let Ok(place_ent) = step.decode(pid) else {
        return (format!("#{pid}"), delta_world, "$".to_string(), "$".to_string());
    };
    if !place_ent.type_name.eq_ignore_ascii_case("IFCLOCALPLACEMENT") {
        return (format!("#{pid}"), delta_world, "$".to_string(), "$".to_string());
    }

    let parent_id = match place_ent.params.first() {
        Some(Value::Ref(p)) => Some(*p),
        _ => None,
    };
    let parent_str = parent_id.map_or_else(|| "$".to_string(), |p| format!("#{p}"));
    let (r_parent, _) = resolve_placement_chain_transform(step, parent_id);
    let delta_local = inv_rotate_vec3(r_parent, delta_world);

    if let Some(Value::Ref(axis_id)) = place_ent.params.get(1)
        && let Ok(axis_ent) = step.decode(*axis_id)
        && axis_ent.type_name.eq_ignore_ascii_case("IFCAXIS2PLACEMENT3D")
    {
        let mut loc = [0.0, 0.0, 0.0];
        if let Some(Value::Ref(pt_id)) = axis_ent.params.first()
            && let Some(pt_ref) = step.get(*pt_id)
            && let Some((coords, _)) = step.decode_coordinates(pt_ref)
        {
            loc = coords;
        }
        let axis_str = match axis_ent.params.get(1) {
            Some(Value::Ref(r)) => format!("#{r}"),
            _ => "$".to_string(),
        };
        let ref_dir_str = match axis_ent.params.get(2) {
            Some(Value::Ref(r)) => format!("#{r}"),
            _ => "$".to_string(),
        };
        return (
            parent_str,
            [
                loc[0] + delta_local[0],
                loc[1] + delta_local[1],
                loc[2] + delta_local[2],
            ],
            axis_str,
            ref_dir_str,
        );
    }

    let (r_self, _) = resolve_placement_chain_transform(step, Some(pid));
    let delta_self = inv_rotate_vec3(r_self, delta_world);
    (format!("#{pid}"), delta_self, "$".to_string(), "$".to_string())
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
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
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
    let num = |v: &Value| -> Option<f64> {
        match v {
            Value::Real(n) => Some(*n),
            Value::Integer(n) => Some(*n as f64),
            _ => None,
        }
    };
    Some([
        items.first().and_then(num)?,
        items.get(1).and_then(num)?,
        items.get(2).and_then(num).unwrap_or(0.0),
    ])
}

fn decode_axis2_placement_3d(step: &StepIndex<'_>, id: u32) -> ([[f64; 3]; 3], [f64; 3]) {
    let Ok(axis_ent) = step.decode(id) else {
        return (IDENTITY_ROT3, [0.0, 0.0, 0.0]);
    };
    if !axis_ent.type_name.eq_ignore_ascii_case("IFCAXIS2PLACEMENT3D") {
        return (IDENTITY_ROT3, [0.0, 0.0, 0.0]);
    }
    let mut loc = [0.0, 0.0, 0.0];
    if let Some(Value::Ref(pt_id)) = axis_ent.params.first()
        && let Some(pt_ref) = step.get(*pt_id)
        && let Some((coords, _)) = step.decode_coordinates(pt_ref)
    {
        loc = coords;
    }
    let z = axis_ent
        .params
        .get(1)
        .and_then(|v| match v {
            Value::Ref(r) => decode_direction3(step, *r),
            _ => None,
        })
        .and_then(normalize3)
        .unwrap_or([0.0, 0.0, 1.0]);
    let x_hint = axis_ent
        .params
        .get(2)
        .and_then(|v| match v {
            Value::Ref(r) => decode_direction3(step, *r),
            _ => None,
        })
        .unwrap_or([1.0, 0.0, 0.0]);
    let proj = dot3(x_hint, z);
    let x_raw = [
        x_hint[0] - z[0] * proj,
        x_hint[1] - z[1] * proj,
        x_hint[2] - z[2] * proj,
    ];
    let x = normalize3(x_raw).unwrap_or_else(|| {
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
        .unwrap_or([1.0, 0.0, 0.0])
    });
    let y = cross3(z, x);
    ([x, y, z], loc)
}

/// Evaluates the full world `(rotation_cols, translation_in_file_units)` of an `IFCLOCALPLACEMENT` chain.
fn resolve_placement_chain_transform(
    step: &StepIndex<'_>,
    placement_id: Option<u32>,
) -> ([[f64; 3]; 3], [f64; 3]) {
    let mut chain = Vec::new();
    let mut cur = placement_id;
    for _ in 0..64 {
        let Some(pid) = cur else { break };
        let Ok(ent) = step.decode(pid) else { break };
        if !ent.type_name.eq_ignore_ascii_case("IFCLOCALPLACEMENT") {
            break;
        }
        let rel = match ent.params.get(1) {
            Some(Value::Ref(axis_id)) => decode_axis2_placement_3d(step, *axis_id),
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
    (r_world, t_world)
}

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Real(n) => Some(*n),
        Value::Integer(n) => Some(*n as f64),
        Value::Typed { value, .. } => value_as_f64(value),
        _ => None,
    }
}

/// Inspects an existing product entity to determine its bar/disk radius in file units when
/// Level 2 `RebendSweptDisk` is called with `radius_file_units <= 0.0`.
fn resolve_existing_bar_radius(step: &StepIndex<'_>, express_id: u32) -> Option<f64> {
    let ent = step.decode(express_id).ok()?;
    // 1. Check NominalDiameter on IfcReinforcingBar or IfcTendon (attribute 9 -> diameter).
    if (ent.type_name.eq_ignore_ascii_case("IFCREINFORCINGBAR")
        || ent.type_name.eq_ignore_ascii_case("IFCTENDON"))
        && let Some(d) = ent.params.get(9).and_then(value_as_f64)
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

/// Locates the byte offset of `ENDSEC;` that closes the `DATA;` section.
fn find_data_endsec(bytes: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(bytes).ok().or(None);
    if let Some(s) = text {
        let upper = s.to_ascii_uppercase();
        let data_pos = upper.find("DATA;")?;
        let rel = upper[data_pos..].find("ENDSEC;")?;
        return Some(data_pos + rel);
    }
    // Fallback for non-UTF-8 (Latin-1) IFC files: ASCII case-insensitive byte scan.
    let find_ascii = |haystack: &[u8], needle: &[u8]| -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|w| w.eq_ignore_ascii_case(needle))
    };
    let data_pos = find_ascii(bytes, b"DATA;")?;
    let rel = find_ascii(&bytes[data_pos..], b"ENDSEC;")?;
    Some(data_pos + rel)
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
