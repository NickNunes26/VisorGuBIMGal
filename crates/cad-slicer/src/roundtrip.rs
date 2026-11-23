//! DXF round-trip reader and diff engine for Phase F5 (`docs/12_clash_alignment_dxf.md` §3.5).
//!
//! Reads an edited clash DXF file, recovers `GUBIM_META` (frame + normalization offset) and
//! `GUBIM_ORIG` (original `INSERT` coordinates), compares every `INSERT` `GUBIM_A_<expressId>`
//! and `EDIT_A_<expressId>` polyline (`POLYLINE`/`VERTEX` or `LWPOLYLINE`), and emits exact 3D
//! Level 1 (`Move`) and Level 2 (`Rebend`) edits in model-local metres.

use crate::Frame;
use glam::{DVec2, DVec3};
use std::collections::BTreeMap;

/// Minimum 2D displacement (metres = 0.01 mm) considered an intentional CAD edit.
const MIN_MOVE_EPS_M: f64 = 1e-5;

/// Parsed `GUBIM_META` header from an exported clash DXF.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedClashMeta {
    pub model_id: String,
    pub pk_m: f64,
    pub s_min: f64,
    pub s_max: f64,
    pub frame: Frame,
    pub norm_offset: DVec2,
}

/// One detected CAD edit ready for surgical STEP IFC patching.
#[derive(Debug, Clone, PartialEq)]
pub enum DxfRoundTripEdit {
    /// Level 1 (`MOVE` on `INSERT` `GUBIM_A_<expressId>`): world-space 3D translation in metres.
    Move {
        express_id: u32,
        block_name: String,
        delta_2d: DVec2,
        delta_3d: DVec3,
    },
    /// Level 2 (edited `POLYLINE`/`LWPOLYLINE` on `EDIT_A_<expressId>`): lifted 3D vertices in metres.
    Rebend {
        express_id: u32,
        points_3d: Vec<DVec3>,
    },
}

/// Result of inspecting an edited clash DXF.
#[derive(Debug, Clone, PartialEq)]
pub struct DxfRoundTripDiff {
    pub meta: ParsedClashMeta,
    pub edits: Vec<DxfRoundTripEdit>,
}

/// Parses an edited clash DXF and extracts all Level 1 (`Move`) and Level 2 (`Rebend`) edits.
pub fn parse_clash_dxf_edits(dxf_bytes: &[u8]) -> Result<DxfRoundTripDiff, String> {
    let text = String::from_utf8_lossy(dxf_bytes);
    let pairs = read_group_pairs(&text);

    let mut meta: Option<ParsedClashMeta> = None;
    let mut orig_positions: BTreeMap<u32, (String, DVec2)> = BTreeMap::new();
    let mut orig_polylines: BTreeMap<u32, Vec<DVec2>> = BTreeMap::new();
    let mut current_inserts: BTreeMap<u32, (String, DVec2)> = BTreeMap::new();
    let mut edit_polylines: BTreeMap<u32, Vec<DVec2>> = BTreeMap::new();

    // First scan top-level 999 comments for GUBIM_META.
    for &(code, val) in &pairs {
        if code == 999 && val.starts_with("GUBIM_META ") && meta.is_none() {
            meta = parse_meta_kv(val);
        }
    }

    // Walk ENTITIES section.
    let mut i = 0;
    let mut in_entities = false;
    while i < pairs.len() {
        let (code, val) = pairs[i];
        if code == 0 && val == "SECTION" {
            if let Some(&(2, sec_name)) = pairs.get(i + 1) {
                in_entities = sec_name == "ENTITIES";
                i += 2;
                continue;
            }
        } else if code == 0 && val == "ENDSEC" {
            in_entities = false;
            i += 1;
            continue;
        }

        if !in_entities || code != 0 {
            i += 1;
            continue;
        }

        let entity_type = val;
        i += 1;
        let start = i;
        while i < pairs.len() && pairs[i].0 != 0 {
            i += 1;
        }
        let body = &pairs[start..i];

        match entity_type {
            "TEXT" | "MTEXT" => {
                let mut layer = "";
                let mut content = "";
                for &(c, v) in body {
                    if c == 8 {
                        layer = v;
                    } else if c == 1 {
                        content = v;
                    }
                }
                if layer.eq_ignore_ascii_case("GUBIM_META") || content.starts_with("GUBIM_") {
                    if content.starts_with("GUBIM_META ") && meta.is_none() {
                        meta = parse_meta_kv(content);
                    } else if content.starts_with("GUBIM_ORIG ")
                        && let Some((id, block, pos)) = parse_orig_kv(content)
                    {
                        orig_positions.insert(id, (block, pos));
                    } else if content.starts_with("GUBIM_POLY ")
                        && let Some((id, pts)) = parse_poly_kv(content)
                    {
                        orig_polylines.insert(id, pts);
                    }
                }
            }
            "INSERT" => {
                let mut block = "";
                let mut x = 0.0;
                let mut y = 0.0;
                for &(c, v) in body {
                    match c {
                        2 => block = v,
                        10 => x = v.parse().unwrap_or(0.0),
                        20 => y = v.parse().unwrap_or(0.0),
                        _ => {}
                    }
                }
                if let Some(id) = parse_gubim_a_id(block) {
                    current_inserts.insert(id, (block.to_string(), DVec2::new(x, y)));
                }
            }
            "LWPOLYLINE" => {
                let mut layer = "";
                let mut pts: Vec<DVec2> = Vec::new();
                let mut cur_x: Option<f64> = None;
                for &(c, v) in body {
                    match c {
                        8 => layer = v,
                        10 => cur_x = v.parse().ok(),
                        20 => {
                            if let (Some(x), Ok(y)) = (cur_x.take(), v.parse::<f64>()) {
                                pts.push(DVec2::new(x, y));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(id) = parse_edit_a_id(layer)
                    && pts.len() >= 2
                {
                    edit_polylines.insert(id, pts);
                }
            }
            "POLYLINE" => {
                let mut layer = "";
                for &(c, v) in body {
                    if c == 8 {
                        layer = v;
                    }
                }
                let edit_id = parse_edit_a_id(layer);
                let mut pts: Vec<DVec2> = Vec::new();
                // Consume following VERTEX entities until SEQEND.
                while i < pairs.len() {
                    if pairs[i].0 == 0 && pairs[i].1 == "VERTEX" {
                        i += 1;
                        let mut vx = 0.0;
                        let mut vy = 0.0;
                        while i < pairs.len() && pairs[i].0 != 0 {
                            match pairs[i].0 {
                                10 => vx = pairs[i].1.parse().unwrap_or(0.0),
                                20 => vy = pairs[i].1.parse().unwrap_or(0.0),
                                _ => {}
                            }
                            i += 1;
                        }
                        pts.push(DVec2::new(vx, vy));
                    } else if pairs[i].0 == 0 && pairs[i].1 == "SEQEND" {
                        i += 1;
                        while i < pairs.len() && pairs[i].0 != 0 {
                            i += 1;
                        }
                        break;
                    } else {
                        break;
                    }
                }
                if let Some(id) = edit_id
                    && pts.len() >= 2
                {
                    edit_polylines.insert(id, pts);
                }
            }
            _ => {}
        }
    }

    let meta = meta.ok_or_else(|| "DXF does not contain GUBIM_META metadata".to_string())?;
    let mut edits: Vec<DxfRoundTripEdit> = Vec::new();

    // Level 1: compare edited INSERT positions against GUBIM_ORIG.
    for (express_id, (block_name, new_pos)) in &current_inserts {
        if let Some((_, orig_pos)) = orig_positions.get(express_id) {
            let delta_2d = *new_pos - *orig_pos;
            if delta_2d.length() > MIN_MOVE_EPS_M {
                let delta_3d = meta.frame.right * delta_2d.x + meta.frame.up * delta_2d.y;
                edits.push(DxfRoundTripEdit::Move {
                    express_id: *express_id,
                    block_name: block_name.clone(),
                    delta_2d,
                    delta_3d,
                });
            }
        }
    }

    // Level 2: lift any modified EDIT_A_<expressId> polylines to 3D on the slab mid-plane.
    let s_mid_shift = 0.5 * (meta.s_min + meta.s_max) - meta.frame.origin.dot(meta.frame.look);
    for (express_id, pts_2d) in edit_polylines {
        let is_unchanged_or_same_move = if let Some(orig_pts) = orig_polylines.get(&express_id) {
            if orig_pts.len() == pts_2d.len() {
                let insert_delta = current_inserts
                    .get(&express_id)
                    .and_then(|(_, new_pos)| {
                        orig_positions
                            .get(&express_id)
                            .map(|(_, orig_pos)| *new_pos - *orig_pos)
                    })
                    .unwrap_or(DVec2::ZERO);
                let matches_orig = orig_pts
                    .iter()
                    .zip(&pts_2d)
                    .all(|(a, b)| a.distance(*b) <= MIN_MOVE_EPS_M);
                let matches_insert_move = insert_delta.length() > MIN_MOVE_EPS_M
                    && orig_pts
                        .iter()
                        .zip(&pts_2d)
                        .all(|(a, b)| (*a + insert_delta).distance(*b) <= MIN_MOVE_EPS_M);
                matches_orig || matches_insert_move
            } else {
                false
            }
        } else {
            false
        };
        if is_unchanged_or_same_move {
            continue;
        }
        let points_3d: Vec<DVec3> = pts_2d
            .into_iter()
            .map(|p| meta.frame.unproject(p + meta.norm_offset) + meta.frame.look * s_mid_shift)
            .collect();
        edits.push(DxfRoundTripEdit::Rebend {
            express_id,
            points_3d,
        });
    }

    Ok(DxfRoundTripDiff { meta, edits })
}

fn read_group_pairs(text: &str) -> Vec<(i32, &str)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out = Vec::with_capacity(lines.len() / 2);
    let mut i = 0;
    while i + 1 < lines.len() {
        if let Ok(code) = lines[i].parse::<i32>() {
            out.push((code, lines[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn parse_gubim_a_id(block_name: &str) -> Option<u32> {
    let rest = block_name
        .strip_prefix("GUBIM_A_")
        .or_else(|| block_name.strip_prefix("gubim_a_"))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn parse_edit_a_id(layer_name: &str) -> Option<u32> {
    let rest = layer_name
        .strip_prefix("EDIT_A_")
        .or_else(|| layer_name.strip_prefix("edit_a_"))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn parse_meta_kv(line: &str) -> Option<ParsedClashMeta> {
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    for token in line.split_whitespace().skip(1) {
        if let Some((k, v)) = token.split_once('=') {
            kv.insert(k, v);
        }
    }
    let f = |k: &str| -> Option<f64> { kv.get(k)?.parse::<f64>().ok().filter(|v| v.is_finite()) };
    let model_id = kv.get("model")?.to_string();
    let frame = Frame {
        origin: DVec3::new(f("ox")?, f("oy")?, f("oz")?),
        right: DVec3::new(f("rx")?, f("ry")?, f("rz")?),
        up: DVec3::new(f("ux")?, f("uy")?, f("uz")?),
        look: DVec3::new(f("lx")?, f("ly")?, f("lz")?),
    };
    Some(ParsedClashMeta {
        model_id,
        pk_m: f("pk")?,
        s_min: f("sMin")?,
        s_max: f("sMax")?,
        frame,
        norm_offset: DVec2::new(f("nx")?, f("ny")?),
    })
}

fn parse_orig_kv(line: &str) -> Option<(u32, String, DVec2)> {
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    for token in line.split_whitespace().skip(1) {
        if let Some((k, v)) = token.split_once('=') {
            kv.insert(k, v);
        }
    }
    let id: u32 = kv.get("id")?.parse().ok()?;
    let block = kv.get("block")?.to_string();
    let x: f64 = kv.get("x")?.parse().ok()?;
    let y: f64 = kv.get("y")?.parse().ok()?;
    Some((id, block, DVec2::new(x, y)))
}

fn parse_poly_kv(line: &str) -> Option<(u32, Vec<DVec2>)> {
    let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
    for token in line.split_whitespace().skip(1) {
        if let Some((k, v)) = token.split_once('=') {
            kv.insert(k, v);
        }
    }
    let id: u32 = kv.get("id")?.parse().ok()?;
    let pts_raw = kv.get("pts")?;
    let mut pts = Vec::new();
    for pair in pts_raw.split(';') {
        let (xs, ys) = pair.split_once(',')?;
        let x: f64 = xs.parse().ok()?;
        let y: f64 = ys.parse().ok()?;
        if x.is_finite() && y.is_finite() {
            pts.push(DVec2::new(x, y));
        }
    }
    (pts.len() >= 2).then_some((id, pts))
}
