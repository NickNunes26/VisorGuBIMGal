//! DXF round-trip reader and diff engine for Phase F5 (`docs/12_clash_alignment_dxf.md` §3.5).
//!
//! Reads an edited clash DXF, recovers `GUBIM_META` (frame + normalization offset) and
//! `GUBIM_ORIG` (original `INSERT` coordinates and `GlobalId`), compares every `INSERT`
//! `GUBIM_A_<expressId>` and `EDIT_A_<expressId>` polyline (`POLYLINE`/`VERTEX` or
//! `LWPOLYLINE`), and emits Level 1 (`Move`) and Level 2 (`Rebend`) edits in model metres.
//!
//! Metadata may be split over several `GUBIM_META` / `GUBIM_POLY` records (DXF R12 values stop
//! at 255 characters); the reader joins them. Files written before that split (`v=1`) carry a
//! single clipped `GUBIM_META` line: the clipped token is discarded, moves still work when the
//! frame axes survived, and rebends that need the missing values are rejected with a reason.

use crate::Frame;
use glam::{DVec2, DVec3};
use std::collections::BTreeMap;

/// Minimum 2D displacement (metres = 0.01 mm) considered an intentional CAD edit.
const MIN_MOVE_EPS_M: f64 = 1e-5;
/// DXF R12 value limit; a record this long may have been clipped by the writer.
const R12_VALUE_LIMIT: usize = 255;

/// Parsed `GUBIM_META` header from an exported clash DXF.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedClashMeta {
    /// Session model id the section was cut from (may be empty or stale).
    pub model_id: String,
    pub pk_m: Option<f64>,
    pub s_min: Option<f64>,
    pub s_max: Option<f64>,
    /// `right` and `up` are always present; `origin` and `look` are zero when missing.
    pub frame: Frame,
    pub norm_offset: Option<DVec2>,
    /// Everything a Level 2 rebend needs (origin, look, slab range, normalization) is present.
    pub complete: bool,
    /// Export time (ms since the Unix epoch), when the exporter wrote it.
    pub exported_at_ms: Option<f64>,
}

/// One element of the exported section as found in the edited DXF, ready for surgical STEP IFC
/// patching.
#[derive(Debug, Clone, PartialEq)]
pub enum DxfRoundTripEdit {
    /// Level 1 (`INSERT` `GUBIM_A_<expressId>`): world-space 3D translation in metres.
    Move {
        express_id: u32,
        /// Full `GlobalId` from `GUBIM_ORIG`, else the (possibly truncated) block-name suffix.
        global_id: Option<String>,
        /// Session id of the element's model (`GUBIM_ORIG model=`), when the export knew it.
        owner: Option<String>,
        block_name: String,
        /// Exported `INSERT` position (normalized sheet metres).
        orig_pos: DVec2,
        delta_2d: DVec2,
        delta_3d: DVec3,
    },
    /// Level 2 (`POLYLINE`/`LWPOLYLINE` on `EDIT_A_<expressId>`): lifted 3D vertices in metres.
    Rebend {
        express_id: u32,
        global_id: Option<String>,
        owner: Option<String>,
        block_name: String,
        orig_pos: DVec2,
        points_3d: Vec<DVec3>,
    },
}

impl DxfRoundTripEdit {
    pub fn express_id(&self) -> u32 {
        match self {
            DxfRoundTripEdit::Move { express_id, .. }
            | DxfRoundTripEdit::Rebend { express_id, .. } => *express_id,
        }
    }

    pub fn global_id(&self) -> Option<&str> {
        match self {
            DxfRoundTripEdit::Move { global_id, .. }
            | DxfRoundTripEdit::Rebend { global_id, .. } => global_id.as_deref(),
        }
    }

    pub fn owner(&self) -> Option<&str> {
        match self {
            DxfRoundTripEdit::Move { owner, .. } | DxfRoundTripEdit::Rebend { owner, .. } => {
                owner.as_deref()
            }
        }
    }

    pub fn orig_pos(&self) -> DVec2 {
        match self {
            DxfRoundTripEdit::Move { orig_pos, .. }
            | DxfRoundTripEdit::Rebend { orig_pos, .. } => *orig_pos,
        }
    }

    /// `move` or `rebend`.
    pub fn kind(&self) -> &'static str {
        match self {
            DxfRoundTripEdit::Move { .. } => "move",
            DxfRoundTripEdit::Rebend { .. } => "rebend",
        }
    }
}

/// An edit seen in the DXF that cannot be turned into a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedDxfEdit {
    pub express_id: u32,
    /// `move` or `rebend`.
    pub kind: &'static str,
    /// Stable reason code, e.g. `metadata-incomplete`, `invalid-number`, `duplicate-insert`.
    pub reason: &'static str,
}

/// Result of inspecting an edited clash DXF.
#[derive(Debug, Clone, PartialEq)]
pub struct DxfRoundTripDiff {
    pub meta: ParsedClashMeta,
    /// Elements whose `INSERT` or edit polyline differs from the export.
    pub edits: Vec<DxfRoundTripEdit>,
    /// Elements still where the export put them (zero `Move`, unchanged `Rebend`). They matter
    /// only to a model an earlier import already patched: there they revert that edit.
    pub unchanged: Vec<DxfRoundTripEdit>,
    pub rejected: Vec<RejectedDxfEdit>,
}

struct OrigRecord {
    id: u32,
    block: String,
    guid: Option<String>,
    owner: Option<String>,
    layer: Option<String>,
    pos: DVec2,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PolyKey {
    Block(String),
    /// Files written before `GUBIM_POLY block=` existed.
    Id(u32),
}

/// A finite number, or `None` for an empty or unparseable value (a comma decimal, a blank).
fn num(v: &str) -> Option<f64> {
    v.parse::<f64>().ok().filter(|x| x.is_finite())
}

fn upper(v: &str) -> String {
    v.to_ascii_uppercase()
}

type Found<T> = Result<T, &'static str>;

/// Parses an edited clash DXF and extracts all Level 1 (`Move`) and Level 2 (`Rebend`) edits.
///
/// Every `INSERT` and edit polyline is paired with its own `GUBIM_ORIG` record by block name
/// and edit layer (both unique in a file), never by expressId alone. An entity this reader
/// cannot turn into an exact edit is rejected with a reason instead of being guessed:
/// unparseable coordinates (`invalid-number`), a rotated, scaled or mirrored `INSERT`
/// (`unsupported-transform`), a copied `INSERT` or polyline (`duplicate-insert`,
/// `duplicate-polyline`), or an old file whose edit layer could name several elements
/// (`ambiguous-element`).
pub fn parse_clash_dxf_edits(dxf_bytes: &[u8]) -> Result<DxfRoundTripDiff, String> {
    let text = String::from_utf8_lossy(dxf_bytes);
    let pairs = read_group_pairs(&text);

    let mut meta_tokens: BTreeMap<String, String> = BTreeMap::new();
    let mut take_meta = |line: &str| {
        let mut tokens: Vec<&str> = line.split_whitespace().skip(1).collect();
        if line.len() >= R12_VALUE_LIMIT {
            // Written by a clipping writer: the last token may be cut short.
            tokens.pop();
        }
        for token in tokens {
            if let Some((k, v)) = token.split_once('=') {
                meta_tokens.entry(k.to_string()).or_insert_with(|| v.to_string());
            }
        }
    };

    for &(code, val) in &pairs {
        if code == 999 && val.starts_with("GUBIM_META ") {
            take_meta(val);
        }
    }

    // Keyed by upper-case block name: CAD tools may change the case of names on save.
    let mut orig: BTreeMap<String, OrigRecord> = BTreeMap::new();
    let mut poly_parts: BTreeMap<PolyKey, BTreeMap<u32, Vec<DVec2>>> = BTreeMap::new();
    // Every INSERT of a GUBIM_A block: (block name as written, position or reject reason).
    let mut inserts: BTreeMap<String, Vec<(String, Found<DVec2>)>> = BTreeMap::new();
    // Every polyline on an EDIT_A layer, by upper-case layer name.
    let mut edit_polylines: BTreeMap<String, Vec<Found<Vec<DVec2>>>> = BTreeMap::new();

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
                    if content.starts_with("GUBIM_META ") {
                        take_meta(content);
                    } else if content.starts_with("GUBIM_ORIG ")
                        && let Some(rec) = parse_orig_kv(content)
                    {
                        orig.insert(upper(&rec.block), rec);
                    } else if content.starts_with("GUBIM_POLY ")
                        && let Some((key, part, pts)) = parse_poly_kv(content)
                    {
                        poly_parts.entry(key).or_default().insert(part, pts);
                    }
                }
            }
            "INSERT" => {
                let mut block = "";
                let mut x = None;
                let mut y = None;
                let mut bad = false;
                let mut transformed = false;
                for &(c, v) in body {
                    match c {
                        2 => block = v,
                        10 => x = Some(num(v)),
                        20 => y = Some(num(v)),
                        41..=43 => match num(v) {
                            Some(s) if (s - 1.0).abs() <= 1e-9 => {}
                            Some(_) => transformed = true,
                            None => bad = true,
                        },
                        50 => match num(v) {
                            Some(r) => {
                                let r = r.rem_euclid(360.0);
                                if r > 1e-6 && 360.0 - r > 1e-6 {
                                    transformed = true;
                                }
                            }
                            None => bad = true,
                        },
                        _ => {}
                    }
                }
                if parse_gubim_a_id(block).is_some() {
                    let pos = match (x, y) {
                        _ if bad => Err("invalid-number"),
                        (Some(Some(x)), Some(Some(y))) if !transformed => Ok(DVec2::new(x, y)),
                        (Some(Some(_)), Some(Some(_))) => Err("unsupported-transform"),
                        _ => Err("invalid-number"),
                    };
                    inserts
                        .entry(upper(block))
                        .or_default()
                        .push((block.to_string(), pos));
                }
            }
            "LWPOLYLINE" => {
                let mut layer = "";
                let mut pts: Vec<DVec2> = Vec::new();
                let mut cur_x: Option<Option<f64>> = None;
                let mut bad = false;
                for &(c, v) in body {
                    match c {
                        8 => layer = v,
                        10 => {
                            if cur_x.is_some() {
                                bad = true;
                            }
                            cur_x = Some(num(v));
                        }
                        20 => match (cur_x.take(), num(v)) {
                            (Some(Some(x)), Some(y)) => pts.push(DVec2::new(x, y)),
                            _ => bad = true,
                        },
                        _ => {}
                    }
                }
                if cur_x.is_some() {
                    bad = true;
                }
                if parse_edit_a_id(layer).is_some() {
                    edit_polylines
                        .entry(upper(layer))
                        .or_default()
                        .push(if bad { Err("invalid-number") } else { Ok(pts) });
                }
            }
            "POLYLINE" => {
                let mut layer = "";
                for &(c, v) in body {
                    if c == 8 {
                        layer = v;
                    }
                }
                let mut pts: Vec<DVec2> = Vec::new();
                let mut bad = false;
                // Consume following VERTEX entities until SEQEND.
                while i < pairs.len() {
                    if pairs[i].0 == 0 && pairs[i].1 == "VERTEX" {
                        i += 1;
                        let mut vx = None;
                        let mut vy = None;
                        while i < pairs.len() && pairs[i].0 != 0 {
                            match pairs[i].0 {
                                10 => vx = num(pairs[i].1),
                                20 => vy = num(pairs[i].1),
                                _ => {}
                            }
                            i += 1;
                        }
                        match (vx, vy) {
                            (Some(x), Some(y)) => pts.push(DVec2::new(x, y)),
                            _ => bad = true,
                        }
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
                if parse_edit_a_id(layer).is_some() {
                    edit_polylines
                        .entry(upper(layer))
                        .or_default()
                        .push(if bad { Err("invalid-number") } else { Ok(pts) });
                }
            }
            _ => {}
        }
    }

    let meta = build_meta(&meta_tokens)
        .ok_or_else(|| "DXF does not contain GUBIM_META metadata".to_string())?;
    let orig_polylines: BTreeMap<PolyKey, Vec<DVec2>> = poly_parts
        .into_iter()
        .map(|(key, parts)| (key, parts.into_values().flatten().collect()))
        .collect();

    // How many exported elements share an expressId (federated models): records written
    // before they named their block and layer can only be paired by a unique expressId.
    let mut id_count: BTreeMap<u32, usize> = BTreeMap::new();
    for rec in orig.values() {
        *id_count.entry(rec.id).or_default() += 1;
    }
    let mut layer_to_block: BTreeMap<String, String> = BTreeMap::new();
    for (key, rec) in &orig {
        if let Some(layer) = &rec.layer {
            layer_to_block.insert(upper(layer), key.clone());
        }
    }

    let guid_of = |rec: &OrigRecord, written_block: &str| -> Option<String> {
        rec.guid
            .clone()
            .or_else(|| guid_from_block_name(written_block))
            .or_else(|| guid_from_block_name(&rec.block))
    };
    let mut edits: Vec<DxfRoundTripEdit> = Vec::new();
    let mut unchanged: Vec<DxfRoundTripEdit> = Vec::new();
    let mut rejected: Vec<RejectedDxfEdit> = Vec::new();

    // Level 1: compare edited INSERT positions against GUBIM_ORIG.
    let mut insert_delta: BTreeMap<String, DVec2> = BTreeMap::new();
    for (key, found) in &inserts {
        let Some(rec) = orig.get(key) else {
            continue;
        };
        let reject = |reason| RejectedDxfEdit {
            express_id: rec.id,
            kind: "move",
            reason,
        };
        if found.len() > 1 {
            rejected.push(reject("duplicate-insert"));
            continue;
        }
        let (written_block, pos) = &found[0];
        let new_pos = match pos {
            Ok(p) => *p,
            Err(reason) => {
                rejected.push(reject(reason));
                continue;
            }
        };
        let delta_2d = new_pos - rec.pos;
        let edit = DxfRoundTripEdit::Move {
            express_id: rec.id,
            global_id: guid_of(rec, written_block),
            owner: rec.owner.clone(),
            block_name: written_block.clone(),
            orig_pos: rec.pos,
            delta_2d,
            delta_3d: meta.frame.right * delta_2d.x + meta.frame.up * delta_2d.y,
        };
        if delta_2d.length() > MIN_MOVE_EPS_M {
            insert_delta.insert(key.clone(), delta_2d);
            edits.push(edit);
        } else {
            unchanged.push(edit);
        }
    }

    // Level 2: lift any modified EDIT_A_<expressId> polylines to 3D on the slab mid-plane.
    for (layer, found) in edit_polylines {
        let layer_id = parse_edit_a_id(&layer).unwrap_or_default();
        let block_key = if let Some(key) = layer_to_block.get(&layer) {
            key.clone()
        } else {
            match id_count.get(&layer_id).copied().unwrap_or(0) {
                0 => continue,
                1 => match orig.iter().find(|(_, r)| r.id == layer_id) {
                    Some((key, _)) => key.clone(),
                    None => continue,
                },
                _ => {
                    rejected.push(RejectedDxfEdit {
                        express_id: layer_id,
                        kind: "rebend",
                        reason: "ambiguous-element",
                    });
                    continue;
                }
            }
        };
        let Some(rec) = orig.get(&block_key) else {
            continue;
        };
        let reject = |reason| RejectedDxfEdit {
            express_id: rec.id,
            kind: "rebend",
            reason,
        };
        let orig_pts = orig_polylines
            .get(&PolyKey::Block(block_key.clone()))
            .or_else(|| {
                (id_count.get(&rec.id) == Some(&1))
                    .then(|| orig_polylines.get(&PolyKey::Id(rec.id)))
                    .flatten()
            });
        let delta = insert_delta.get(&block_key).copied().unwrap_or(DVec2::ZERO);
        let matches_orig = |pts_2d: &[DVec2]| {
            orig_pts.is_some_and(|orig_pts| {
                orig_pts.len() == pts_2d.len()
                    && orig_pts
                        .iter()
                        .zip(pts_2d)
                        .all(|(a, b)| a.distance(*b) <= MIN_MOVE_EPS_M)
            })
        };
        let matches_insert_move = |pts_2d: &[DVec2]| {
            delta.length() > MIN_MOVE_EPS_M
                && orig_pts.is_some_and(|orig_pts| {
                    orig_pts.len() == pts_2d.len()
                        && orig_pts
                            .iter()
                            .zip(pts_2d)
                            .all(|(a, b)| (*a + delta).distance(*b) <= MIN_MOVE_EPS_M)
                })
        };
        let is_unchanged_or_same_move = |pts_2d: &[DVec2]| {
            matches_orig(pts_2d) || matches_insert_move(pts_2d)
        };
        // A new polyline drawn next to the exported one, which was left in place, is the
        // edit; two that both differ from the export cannot be told apart.
        let mut found = found;
        if found.len() > 1 {
            let edited: Vec<Found<Vec<DVec2>>> = found
                .iter()
                .filter(|f| !matches!(f, Ok(pts) if is_unchanged_or_same_move(pts)))
                .cloned()
                .collect();
            match edited.len() {
                0 => found.truncate(1),
                1 => found = edited,
                _ => {
                    rejected.push(reject("duplicate-polyline"));
                    continue;
                }
            }
        }
        let pts_2d = match found.into_iter().next() {
            Some(Ok(pts)) if pts.len() >= 2 => pts,
            Some(Ok(_)) => {
                rejected.push(reject("invalid-points"));
                continue;
            }
            Some(Err(reason)) => {
                rejected.push(reject(reason));
                continue;
            }
            None => continue,
        };
        let stayed_at_orig = matches_orig(&pts_2d);
        let is_unchanged_or_same_move = stayed_at_orig || matches_insert_move(&pts_2d);
        let (Some(norm), true) = (meta.norm_offset, meta.complete) else {
            if !is_unchanged_or_same_move {
                rejected.push(reject("metadata-incomplete"));
            }
            continue;
        };
        let s_mid = 0.5 * (meta.s_min.unwrap_or(0.0) + meta.s_max.unwrap_or(0.0));
        let s_mid_shift = s_mid - meta.frame.origin.dot(meta.frame.look);
        // When the drafter moved only the INSERT and left EDIT_A untouched at its exported
        // place, the unbent bar in this DXF stands at `pts_2d + delta`.
        let lift_delta = if stayed_at_orig { delta } else { DVec2::ZERO };
        let points_3d: Vec<DVec3> = pts_2d
            .into_iter()
            .map(|p| meta.frame.unproject(p + lift_delta + norm) + meta.frame.look * s_mid_shift)
            .collect();
        let edit = DxfRoundTripEdit::Rebend {
            express_id: rec.id,
            global_id: guid_of(rec, &rec.block),
            owner: rec.owner.clone(),
            block_name: rec.block.clone(),
            orig_pos: rec.pos,
            points_3d,
        };
        if is_unchanged_or_same_move {
            unchanged.push(edit);
        } else {
            edits.push(edit);
        }
    }

    Ok(DxfRoundTripDiff {
        meta,
        edits,
        unchanged,
        rejected,
    })
}

fn build_meta(kv: &BTreeMap<String, String>) -> Option<ParsedClashMeta> {
    let f = |k: &str| -> Option<f64> { kv.get(k)?.parse::<f64>().ok().filter(|v| v.is_finite()) };
    let v3 = |a: &str, b: &str, c: &str| Some(DVec3::new(f(a)?, f(b)?, f(c)?));
    let right = v3("rx", "ry", "rz")?;
    let up = v3("ux", "uy", "uz")?;
    if right.length_squared() < 1e-12 || up.length_squared() < 1e-12 {
        return None;
    }
    let origin = v3("ox", "oy", "oz");
    let look = v3("lx", "ly", "lz");
    let norm_offset = f("nx").zip(f("ny")).map(|(x, y)| DVec2::new(x, y));
    let (s_min, s_max) = (f("sMin"), f("sMax"));
    let complete = origin.is_some()
        && look.is_some()
        && norm_offset.is_some()
        && s_min.is_some()
        && s_max.is_some();
    Some(ParsedClashMeta {
        model_id: kv.get("model").cloned().unwrap_or_default(),
        pk_m: f("pk"),
        s_min,
        s_max,
        frame: Frame {
            origin: origin.unwrap_or(DVec3::ZERO),
            right,
            up,
            look: look.unwrap_or(DVec3::ZERO),
        },
        norm_offset,
        complete,
        exported_at_ms: f("t"),
    })
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

/// The `GlobalId` (or its truncated prefix) after `GUBIM_A_<id>_` in a block name.
fn guid_from_block_name(block_name: &str) -> Option<String> {
    let rest = block_name
        .strip_prefix("GUBIM_A_")
        .or_else(|| block_name.strip_prefix("gubim_a_"))?;
    let after_id = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let guid = after_id.strip_prefix('_')?;
    (!guid.is_empty()).then(|| guid.to_string())
}

fn parse_edit_a_id(layer_name: &str) -> Option<u32> {
    let rest = layer_name
        .strip_prefix("EDIT_A_")
        .or_else(|| layer_name.strip_prefix("edit_a_"))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn kv_of(line: &str) -> BTreeMap<&str, &str> {
    line.split_whitespace()
        .skip(1)
        .filter_map(|token| token.split_once('='))
        .collect()
}

fn parse_orig_kv(line: &str) -> Option<OrigRecord> {
    let kv = kv_of(line);
    let id: u32 = kv.get("id")?.parse().ok()?;
    let block = kv.get("block")?.to_string();
    let x = num(kv.get("x")?)?;
    let y = num(kv.get("y")?)?;
    let text = |k: &str| kv.get(k).map(|g| g.to_string()).filter(|g| !g.is_empty());
    Some(OrigRecord {
        id,
        block,
        guid: text("guid"),
        owner: text("model"),
        layer: text("layer"),
        pos: DVec2::new(x, y),
    })
}

fn parse_poly_kv(line: &str) -> Option<(PolyKey, u32, Vec<DVec2>)> {
    let kv = kv_of(line);
    let id: u32 = kv.get("id")?.parse().ok()?;
    let key = match kv.get("block") {
        Some(block) if !block.is_empty() => PolyKey::Block(upper(block)),
        _ => PolyKey::Id(id),
    };
    let part: u32 = kv.get("part").and_then(|p| p.parse().ok()).unwrap_or(0);
    let pts_raw = kv.get("pts")?;
    let mut pts = Vec::new();
    for pair in pts_raw.split(';') {
        let (xs, ys) = pair.split_once(',')?;
        pts.push(DVec2::new(num(xs)?, num(ys)?));
    }
    (!pts.is_empty()).then_some((key, part, pts))
}
