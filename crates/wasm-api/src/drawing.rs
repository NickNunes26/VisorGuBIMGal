//! Drawings across per-model Workers: `docs/05` section 6.
//!
//! Each model turns its elements into frame-space chunks. One `Sheet`, in a Worker of its own,
//! gathers them and runs `cad_slicer::draw` once, so hidden lines work across models.

use crate::encode::severity;
use crate::json::{Object, ToJson, to_json};
use cad_slicer::{
    Axis, Cut, Drawing, DrawingMesh, DrawingOptions, DxfOptions, Frame, Look, SvgOptions, draw,
    to_dxf_with, to_svg_with,
};
use glam::{DMat4, DVec3};
use ifc_geometry::{ElementGeometry, Mesh, ModelEvaluator};
use spatial_tree::ModelData;

/// Words of a request before the optional transform.
const REQUEST_WORDS: usize = 7;

/// What a drawing asks for, as the flat numbers the Worker sends. See `drawingRequest.ts`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingRequest {
    pub options: DrawingOptions,
    pub include_hidden: bool,
    /// Frame coordinates from model metres, for a `map` frame.
    pub transform: Option<DMat4>,
}

impl DrawingRequest {
    /// `[kind, position, look, depth, projection, hatching, hidden, ...transform]`. Kind is 0 for
    /// a plan, 1 and 2 for sections on X and Y. A NaN depth means no limit.
    pub fn decode(words: &[f64]) -> Option<Self> {
        if words.len() != REQUEST_WORDS && words.len() != REQUEST_WORDS + 16 {
            return None;
        }
        let look = if words[2] > 0.0 { Look::Positive } else { Look::Negative };
        let cut = match words[0] as u32 {
            0 => Cut::Plan { z: words[1] },
            1 => Cut::Section { axis: Axis::X, position: words[1], look },
            2 => Cut::Section { axis: Axis::Y, position: words[1], look },
            _ => return None,
        };
        let options = DrawingOptions {
            cut,
            depth: (!words[3].is_nan()).then_some(words[3]),
            include_projection: words[4] != 0.0,
            include_hatching: words[5] != 0.0,
        };
        let transform = (words.len() > REQUEST_WORDS)
            .then(|| <[f64; 16]>::try_from(&words[REQUEST_WORDS..]).ok())
            .flatten()
            .map(|matrix| DMat4::from_cols_array(&matrix));
        Some(Self { options, include_hidden: words[6] != 0.0, transform })
    }
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_element(out: &mut Vec<u8>, express_id: u32, part: u32, layer: &str, mesh: &Mesh) {
    put_u32(out, express_id);
    put_u32(out, part);
    put_u32(out, layer.len() as u32);
    out.extend_from_slice(layer.as_bytes());
    put_u32(out, mesh.positions.len() as u32);
    put_u32(out, mesh.triangles.len() as u32);
    for position in &mesh.positions {
        for value in position.to_array() {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    for triangle in &mesh.triangles {
        for index in triangle {
            put_u32(out, *index);
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if count > self.bytes.len() {
            return None;
        }
        let (head, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }

    fn f64(&mut self) -> Option<f64> {
        self.take(8)?.try_into().ok().map(f64::from_le_bytes)
    }

    /// Checks the size first: a bad count must not reserve gigabytes.
    fn element(&mut self, model_index: u32) -> Option<Item> {
        let express_id = self.u32()?;
        let part = self.u32()?;
        let layer_length = self.u32()? as usize;
        let layer = String::from_utf8_lossy(self.take(layer_length)?).into_owned();
        let vertices = self.u32()? as usize;
        let triangles = self.u32()? as usize;
        if vertices.checked_mul(24)?.checked_add(triangles.checked_mul(12)?)? > self.bytes.len() {
            return None;
        }
        let mut mesh = Mesh {
            positions: Vec::with_capacity(vertices),
            triangles: Vec::with_capacity(triangles),
        };
        for _ in 0..vertices {
            let (x, y, z) = (self.f64()?, self.f64()?, self.f64()?);
            mesh.positions.push(DVec3::new(x, y, z));
        }
        for _ in 0..triangles {
            mesh.triangles.push([self.u32()?, self.u32()?, self.u32()?]);
        }
        let mesh = DrawingMesh::new(model_index, express_id, layer, mesh);
        Some(Item { part, mesh })
    }
}

/// Whether anything of an element can reach the sheet: behind the plane, within `depth`, and on
/// the plane itself when nothing is projected. Each corner is in frame coordinates.
fn reaches_sheet(mesh: &Mesh, frame: &Frame, options: &DrawingOptions) -> bool {
    let (mut near, mut far) = (f64::INFINITY, f64::NEG_INFINITY);
    for position in &mesh.positions {
        let depth = frame.depth(*position);
        near = near.min(depth);
        far = far.max(depth);
    }
    if far < 0.0 {
        return false;
    }
    if options.depth.is_some_and(|limit| near > limit) {
        return false;
    }
    options.include_projection || near <= 0.0
}

/// One model's way through a drawing, on an evaluator of its own.
pub struct DrawingStream {
    evaluator: ModelEvaluator<'static>,
    ids: Vec<u32>,
    cursor: usize,
    frame: Option<Frame>,
    request: DrawingRequest,
}

impl DrawingStream {
    pub fn new(evaluator: ModelEvaluator<'static>, request: DrawingRequest) -> Self {
        let ids = evaluator.element_ids();
        let frame = Frame::of(request.options.cut);
        Self { evaluator, ids, cursor: 0, frame, request }
    }

    fn write(&self, geometry: ElementGeometry, data: &ModelData<'_>, out: &mut Vec<u8>) -> usize {
        let Some(frame) = &self.frame else { return 0 };
        let layer = data.layer_name(geometry.express_id, &geometry.type_name);
        let mut written = 0;
        for (part, element_mesh) in geometry.meshes.into_iter().enumerate() {
            let mut mesh = element_mesh.mesh;
            if let Some(transform) = &self.request.transform {
                for position in &mut mesh.positions {
                    *position = transform.transform_point3(*position);
                }
            }
            if reaches_sheet(&mesh, frame, &self.request.options) {
                written += mesh.triangles.len();
                write_element(out, geometry.express_id, part as u32, &layer, &mesh);
            }
        }
        written
    }

    /// `None` when every element has been through. `max_triangles` is soft, as for batches: an
    /// element is never split.
    pub fn next_chunk(&mut self, data: &ModelData<'_>, max_triangles: usize) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut triangles = 0;
        while self.cursor < self.ids.len() && (triangles < max_triangles || out.is_empty()) {
            let id = self.ids[self.cursor];
            self.cursor += 1;
            let Some(geometry) = self.evaluator.evaluate(id) else { continue };
            if !geometry.hidden_by_default {
                triangles += self.write(geometry, data, &mut out);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    pub fn evaluator(&self) -> &ModelEvaluator<'static> {
        &self.evaluator
    }
}

struct Item {
    part: u32,
    mesh: DrawingMesh,
}

/// A problem with the sheet as a whole or one of its elements.
struct SheetDiagnostic {
    model_index: Option<u32>,
    express_id: Option<u32>,
    code: &'static str,
    message: String,
}

impl ToJson for SheetDiagnostic {
    fn write_json(&self, out: &mut String) {
        Object::new(out)
            .field("severity", severity(self.code))
            .field("code", self.code)
            .field("message", &self.message)
            .some("modelIndex", &self.model_index)
            .some("expressId", &self.express_id)
            .end();
    }
}

/// How `Sheet::export` should serialise.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOptions {
    pub svg: bool,
    pub scale: f64,
    pub show_hidden: bool,
    pub show_hatching: bool,
    pub comment: Option<String>,
}

/// One drawing: its meshes until `generate`, its 2D data afterwards.
pub struct SheetStore {
    request: DrawingRequest,
    items: Vec<Item>,
    bad_chunks: Vec<u32>,
    drawing: Option<Drawing>,
}

impl SheetStore {
    pub fn new(request: DrawingRequest) -> Self {
        Self { request, items: Vec::new(), bad_chunks: Vec::new(), drawing: None }
    }

    /// A malformed chunk loses what it still held and is reported. It never panics.
    pub fn add_chunk(&mut self, model_index: u32, chunk: &[u8]) {
        let mut reader = Reader { bytes: chunk };
        while !reader.bytes.is_empty() {
            match reader.element(model_index) {
                Some(item) => self.items.push(item),
                None => {
                    self.bad_chunks.push(model_index);
                    return;
                }
            }
        }
    }

    pub fn drop_model(&mut self, model_index: u32) {
        self.items.retain(|item| item.mesh.model_id != model_index);
        self.bad_chunks.retain(|index| *index != model_index);
    }

    /// Cuts everything once and frees the meshes. Returns the diagnostics as JSON.
    pub fn generate(&mut self) -> String {
        let mut items = std::mem::take(&mut self.items);
        items.sort_by_key(|item| (item.mesh.model_id, item.mesh.element_id, item.part));
        let meshes: Vec<DrawingMesh> = items.into_iter().map(|item| item.mesh).collect();
        let mut drawing = draw(&meshes, &self.request.options);
        drop(meshes);
        if !self.request.include_hidden {
            for layer in &mut drawing.layers {
                layer.hidden = Vec::new();
            }
        }
        let mut diagnostics: Vec<SheetDiagnostic> = self
            .bad_chunks
            .iter()
            .map(|index| SheetDiagnostic {
                model_index: Some(*index),
                express_id: None,
                code: "bad-drawing-chunk",
                message: "part of this model's geometry could not be read".to_string(),
            })
            .collect();
        diagnostics.extend(drawing.diagnostics.iter().map(|item| SheetDiagnostic {
            model_index: (item.element_id != 0).then_some(item.model_id),
            express_id: (item.element_id != 0).then_some(item.element_id),
            code: item.code,
            message: item.message.clone(),
        }));
        self.drawing = Some(drawing);
        to_json(&diagnostics)
    }

    /// `None` before `generate`.
    pub fn export(&self, options: &ExportOptions) -> Option<Vec<u8>> {
        let drawing = self.drawing.as_ref()?;
        let scaled;
        let drawing = if options.scale == 1.0 {
            drawing
        } else {
            scaled = drawing.scaled(options.scale);
            &scaled
        };
        Some(if options.svg {
            let svg = SvgOptions {
                include_hidden: options.show_hidden,
                include_hatch: options.show_hatching,
                millimetres_per_unit: 10.0 / options.scale,
                ..SvgOptions::default()
            };
            to_svg_with(drawing, &svg).into_bytes()
        } else {
            let dxf = DxfOptions {
                include_hidden: options.show_hidden,
                include_hatch: options.show_hatching,
                comment: options.comment.clone(),
            };
            to_dxf_with(drawing, &dxf)
        })
    }
}
