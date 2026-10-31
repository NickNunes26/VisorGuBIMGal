//! Turns IFC products into world-space meshes, in metres.
//!
//! Every failure is local to its element (R8): what cannot be evaluated becomes a
//! [`Diagnostic`], and the rest of the model carries on.

use crate::csg::{intersect, subtract, subtract_all, union};
use crate::mesh::Mesh;
use crate::nurbs;
use crate::solids;
use earcut::Earcut;
use glam::{DMat4, DVec2, DVec3, DVec4};
use ifc_schema::units::{ProjectUnits, project_units};
use ifc_schema::{EntityDef, Schema, SchemaId, schema};
use std::collections::{HashMap, HashSet};
use step_parser::{Entity, StepIndex, Value};

/// Bounds placement chains and nested mapped items, which a hostile file could make cyclic.
const MAX_DEPTH: usize = 64;
/// Items one element may expand to. Maps sharing maps can multiply at every level, so the
/// depth limit alone leaves exponentially many expansions.
const MAX_ITEMS_PER_ELEMENT: usize = 100_000;
/// Booleans one element may evaluate. Each costs far more than a plain item.
const MAX_BOOLEANS_PER_ELEMENT: usize = 4096;
const DEFAULT_RGBA: u32 = 0xB8B8_B8FF;
/// Consecutive profile points closer than this are one point. Metres.
const POINT_TOLERANCE: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub express_id: Option<u32>,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ElementMesh {
    pub mesh: Mesh,
    /// `0xRRGGBBAA`.
    pub rgba: u32,
}

#[derive(Debug, Clone)]
pub struct ElementGeometry {
    pub express_id: u32,
    pub type_name: String,
    pub meshes: Vec<ElementMesh>,
    /// Something was skipped: a void left uncut, say. A diagnostic names it.
    pub degraded: bool,
    pub hidden_by_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedSchema;

struct Profile {
    outer: Vec<DVec2>,
    holes: Vec<Vec<DVec2>>,
}

pub struct ModelEvaluator<'a> {
    file: &'a StepIndex<'a>,
    schema: &'static Schema,
    units: ProjectUnits,
    placements: HashMap<u32, DMat4>,
    openings: HashMap<u32, Vec<u32>>,
    item_colors: HashMap<u32, u32>,
    diagnostics: Vec<Diagnostic>,
    current: Option<u32>,
    degraded: bool,
    items_used: usize,
    booleans_used: usize,
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Real(number) => Some(*number),
        Value::Integer(number) => Some(*number as f64),
        Value::Typed { value, .. } => number(value),
        _ => None,
    }
}

fn reference(value: &Value) -> Option<u32> {
    match value {
        Value::Ref(id) => Some(*id),
        _ => None,
    }
}

fn list(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

fn ring_area(ring: &[DVec2]) -> f64 {
    let count = ring.len();
    (0..count)
        .map(|i| ring[i].perp_dot(ring[(i + 1) % count]))
        .sum::<f64>()
        / 2.0
}

/// Drops repeated points, including a closing point equal to the first.
fn clean_ring(mut ring: Vec<DVec2>) -> Vec<DVec2> {
    ring.dedup_by(|a, b| a.distance_squared(*b) <= POINT_TOLERANCE * POINT_TOLERANCE);
    while ring.len() > 1
        && ring[0].distance_squared(ring[ring.len() - 1]) <= POINT_TOLERANCE * POINT_TOLERANCE
    {
        ring.pop();
    }
    ring
}

/// Triangulates a polygon with holes. Rings need at least three points each.
fn triangulate_rings(rings: &[&[DVec2]]) -> Vec<[u32; 3]> {
    let mut points: Vec<[f64; 2]> = Vec::new();
    let mut hole_starts: Vec<u32> = Vec::new();
    for (index, ring) in rings.iter().enumerate() {
        if index > 0 {
            hole_starts.push(points.len() as u32);
        }
        points.extend(ring.iter().map(|point| [point.x, point.y]));
    }
    let mut indices: Vec<u32> = Vec::new();
    Earcut::new().earcut(points.iter().copied(), &hole_starts, &mut indices);

    indices
        .chunks_exact(3)
        .map(|triangle| {
            let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
            let corner = |id: u32| {
                let [x, y] = points[id as usize];
                DVec2::new(x, y)
            };
            if (corner(b) - corner(a)).perp_dot(corner(c) - corner(a)) < 0.0 {
                [a, c, b]
            } else {
                [a, b, c]
            }
        })
        .collect()
}

/// Flips edges so each starts where the last one ended. A loop has to connect, and real
/// files carry orientation flags that say otherwise, which would fold the whole boundary.
fn chain_edges(edges: &mut [Vec<DVec3>]) {
    let ends = |edge: &[DVec3]| (edge[0], edge[edge.len() - 1]);
    let meets = |a: DVec3, b: DVec3| a.distance_squared(b) <= 1e-12;
    if edges.len() < 2 || edges.iter().any(Vec::is_empty) {
        return;
    }
    let (second_start, second_end) = ends(&edges[1]);
    let (_, first_end) = ends(&edges[0]);
    if !meets(first_end, second_start) && !meets(first_end, second_end) {
        edges[0].reverse();
    }
    for index in 1..edges.len() {
        let (_, reached) = ends(&edges[index - 1]);
        let (start, end) = ends(&edges[index]);
        if !meets(start, reached) && meets(end, reached) {
            edges[index].reverse();
        }
    }
}

/// An edge may use only part of its curve: keeps the run between its two vertices.
fn between_vertices(points: Vec<DVec3>, start: DVec3, end: DVec3) -> Vec<DVec3> {
    if start.distance_squared(end) <= POINT_TOLERANCE * POINT_TOLERANCE {
        return points;
    }
    let nearest = |to: DVec3| {
        (0..points.len()).min_by(|a, b| {
            points[*a].distance_squared(to).total_cmp(&points[*b].distance_squared(to))
        })
    };
    match (nearest(start), nearest(end)) {
        (Some(first), Some(last)) if first < last => points[first..=last].to_vec(),
        _ => points,
    }
}

fn numbers(value: Option<&Value>) -> Vec<f64> {
    list(value).iter().filter_map(number).collect()
}

fn knot_vector(
    entity: &Entity,
    def: &EntityDef,
    knots: &str,
    multiplicities: &str,
    degree: usize,
    count: usize,
) -> Option<Vec<f64>> {
    let knots = numbers(attribute(entity, def, knots));
    let times: Vec<usize> = numbers(attribute(entity, def, multiplicities))
        .into_iter()
        .map(|times| times as usize)
        .collect();
    nurbs::expand_knots(&knots, &times, degree, count)
}

/// Separate solids sharing one mesh.
fn merged(parts: impl Iterator<Item = Mesh>) -> Mesh {
    let mut all = Mesh::default();
    for mesh in parts {
        let first = all.positions.len() as u32;
        all.positions.extend(mesh.positions);
        all.triangles.extend(mesh.triangles.iter().map(|triangle| triangle.map(|id| id + first)));
    }
    all
}

/// Whether a logical is `.T.` or `.F.`. Absent or unknown gives `None`.
fn flag(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Enum(flag)) if flag == "T" => Some(true),
        Some(Value::Enum(flag)) if flag == "F" => Some(false),
        _ => None,
    }
}

/// Appends a curve segment, dropping the joint it shares with what came before.
fn join(path: &mut Vec<DVec3>, part: Vec<DVec3>) {
    let shared = match (path.last(), part.first()) {
        (Some(last), Some(first)) => last.distance_squared(*first) <= POINT_TOLERANCE * POINT_TOLERANCE,
        _ => false,
    };
    path.extend(part.into_iter().skip(shared as usize));
}

fn on_conic(frame: DMat4, radii: DVec2, start: f64, sweep: f64) -> Vec<DVec3> {
    solids::arc(DVec2::ZERO, radii, start, sweep)
        .into_iter()
        .map(|point| frame.transform_point3(point.extend(0.0)))
        .collect()
}

/// Outer ring counter-clockwise, holes clockwise.
fn wound(mut profile: Profile) -> Profile {
    if ring_area(&profile.outer) < 0.0 {
        profile.outer.reverse();
    }
    for hole in &mut profile.holes {
        if ring_area(hole) > 0.0 {
            hole.reverse();
        }
    }
    profile
}

/// Extrudes a profile along `offset`, which must not lie in the profile's plane.
fn prism(profile: Profile, offset: DVec3) -> Mesh {
    let Profile { outer, holes } = wound(profile);
    let mut rings: Vec<&[DVec2]> = vec![&outer];
    rings.extend(holes.iter().map(Vec::as_slice));
    let caps = triangulate_rings(&rings);

    let count = rings.iter().map(|ring| ring.len()).sum::<usize>() as u32;
    let mut positions = Vec::with_capacity(count as usize * 2);
    for shift in [DVec3::ZERO, offset] {
        for ring in &rings {
            positions.extend(ring.iter().map(|point| point.extend(0.0) + shift));
        }
    }

    let mut triangles = Vec::with_capacity(caps.len() * 2 + count as usize * 2);
    for [a, b, c] in caps {
        triangles.push([a, c, b]);
        triangles.push([a + count, b + count, c + count]);
    }
    let mut start = 0u32;
    for ring in &rings {
        let length = ring.len() as u32;
        for i in 0..length {
            let (a, b) = (start + i, start + (i + 1) % length);
            triangles.push([a, b, b + count]);
            triangles.push([a, b + count, a + count]);
        }
        start += length;
    }

    // Extruding downwards turns the solid inside out.
    if offset.z < 0.0 {
        for triangle in &mut triangles {
            triangle.swap(1, 2);
        }
    }
    Mesh { positions, triangles }
}

fn transformed(mesh: Mesh, transform: DMat4) -> Mesh {
    let mut result = mesh.map_positions(|point| transform.transform_point3(point));
    if transform.determinant() < 0.0 {
        for triangle in &mut result.triangles {
            triangle.swap(1, 2);
        }
    }
    result
}

/// Whether the mesh, once coincident vertices are merged, encloses a volume.
fn is_closed(mesh: &Mesh) -> bool {
    let mut ids: HashMap<[u64; 3], u32> = HashMap::new();
    let mut remap = Vec::with_capacity(mesh.positions.len());
    for position in &mesh.positions {
        let key = [position.x.to_bits(), position.y.to_bits(), position.z.to_bits()];
        let next = ids.len() as u32;
        remap.push(*ids.entry(key).or_insert(next));
    }
    let merged = Mesh {
        positions: mesh.positions.clone(),
        triangles: mesh
            .triangles
            .iter()
            .map(|triangle| triangle.map(|id| remap[id as usize]))
            .collect(),
    };
    !merged.is_empty() && merged.open_edges() == 0
}

impl<'a> ModelEvaluator<'a> {
    pub fn new(file: &'a StepIndex<'a>) -> Result<Self, UnsupportedSchema> {
        let id = SchemaId::from_identifiers(file.schema_identifiers()).ok_or(UnsupportedSchema)?;
        let tables = schema(id);
        let mut evaluator = Self {
            file,
            schema: tables,
            units: project_units(file, tables),
            placements: HashMap::new(),
            openings: HashMap::new(),
            item_colors: HashMap::new(),
            diagnostics: Vec::new(),
            current: None,
            degraded: false,
            items_used: 0,
            booleans_used: 0,
        };
        evaluator.index_openings();
        evaluator.index_colors();
        Ok(evaluator)
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn units(&self) -> ProjectUnits {
        self.units
    }

    fn warn(&mut self, code: &'static str, message: String) {
        self.degraded = true;
        self.diagnostics.push(Diagnostic {
            express_id: self.current,
            code,
            message,
        });
    }

    fn lookup(&self, id: u32) -> Option<(Entity, EntityDef)> {
        let entity = self.file.decode(id).ok()?;
        let def = self.schema.entity(&entity.type_name)?;
        Some((entity, def))
    }

    fn index_openings(&mut self) {
        let ids: Vec<u32> = self.file.ids_by_type("IFCRELVOIDSELEMENT").collect();
        for id in ids {
            let Some((entity, def)) = self.lookup(id) else { continue };
            let host = attribute(&entity, &def, "RelatingBuildingElement").and_then(reference);
            let opening = attribute(&entity, &def, "RelatedOpeningElement").and_then(reference);
            if let (Some(host), Some(opening)) = (host, opening) {
                self.openings.entry(host).or_default().push(opening);
            }
        }
    }

    fn index_colors(&mut self) {
        let ids: Vec<u32> = self.file.ids_by_type("IFCSTYLEDITEM").collect();
        for id in ids {
            let Some((entity, def)) = self.lookup(id) else { continue };
            let Some(item) = attribute(&entity, &def, "Item").and_then(reference) else {
                continue;
            };
            let color = list(attribute(&entity, &def, "Styles"))
                .iter()
                .filter_map(reference)
                .find_map(|style| self.style_color(style, 0, &mut HashSet::new()));
            if let Some(color) = color {
                self.item_colors.insert(item, color);
            }
        }
    }

    fn style_color(&self, id: u32, depth: usize, visited: &mut HashSet<u32>) -> Option<u32> {
        if depth >= MAX_DEPTH || !visited.insert(id) {
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        if def.is_a("IfcSurfaceStyleShading") {
            let colour = attribute(&entity, &def, "SurfaceColour").and_then(reference)?;
            let (rgb, rgb_def) = self.lookup(colour)?;
            let channel = |name: &str| {
                let value = attribute(&rgb, &rgb_def, name).and_then(number)?;
                Some((value.clamp(0.0, 1.0) * 255.0).round() as u32)
            };
            let transparency = attribute(&entity, &def, "Transparency")
                .and_then(number)
                .unwrap_or(0.0);
            let alpha = ((1.0 - transparency).clamp(0.0, 1.0) * 255.0).round() as u32;
            return Some(
                channel("Red")? << 24 | channel("Green")? << 16 | channel("Blue")? << 8 | alpha,
            );
        }
        if def.is_a("IfcSurfaceStyle") || def.name() == "IfcPresentationStyleAssignment" {
            return list(attribute(&entity, &def, "Styles"))
                .iter()
                .filter_map(reference)
                .find_map(|style| self.style_color(style, depth + 1, visited));
        }
        None
    }

    /// Products with a representation, ascending. Openings only exist to cut their hosts.
    pub fn element_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for type_name in self.schema.subtypes_of("IfcProduct") {
            let upper = type_name.to_ascii_uppercase();
            for id in self.file.ids_by_type(&upper) {
                let Some((entity, def)) = self.lookup(id) else { continue };
                let shown = !def.is_a("IfcFeatureElementSubtraction")
                    && attribute(&entity, &def, "Representation")
                        .and_then(reference)
                        .is_some();
                if shown {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        ids
    }

    pub fn evaluate(&mut self, id: u32) -> Option<ElementGeometry> {
        self.current = Some(id);
        self.degraded = false;
        self.items_used = 0;
        self.booleans_used = 0;
        let (entity, def) = self.lookup(id)?;
        let mut meshes = self.product_meshes(&entity, &def);

        if let Some(openings) = self.openings.get(&id).cloned() {
            let cuts: Vec<Mesh> = openings
                .iter()
                .flat_map(|opening| {
                    let Some((entity, def)) = self.lookup(*opening) else {
                        return Vec::new();
                    };
                    self.product_meshes(&entity, &def)
                })
                .map(|element| element.mesh)
                .collect();
            if !cuts.is_empty() {
                for element in &mut meshes {
                    if !is_closed(&element.mesh) {
                        self.warn("void-on-open-mesh", "void not cut: mesh is not closed".into());
                        continue;
                    }
                    match subtract_all(&element.mesh, &cuts) {
                        Ok(result) => element.mesh = result,
                        Err(error) => self.warn("void-failed", error.to_string()),
                    }
                }
            }
        }

        let geometry = ElementGeometry {
            express_id: id,
            type_name: def.name().to_string(),
            meshes,
            degraded: self.degraded,
            hidden_by_default: def.is_a("IfcSpace"),
        };
        self.current = None;
        Some(geometry)
    }

    fn product_meshes(&mut self, entity: &Entity, def: &EntityDef) -> Vec<ElementMesh> {
        let placement = match attribute(entity, def, "ObjectPlacement").and_then(reference) {
            Some(id) => self.placement(id, 0),
            None => DMat4::IDENTITY,
        };
        let Some(shape) = attribute(entity, def, "Representation").and_then(reference) else {
            return Vec::new();
        };
        let Some((shape, shape_def)) = self.lookup(shape) else {
            return Vec::new();
        };

        let representations: Vec<u32> = list(attribute(&shape, &shape_def, "Representations"))
            .iter()
            .filter_map(reference)
            .collect();
        let identifier = |evaluator: &Self, id: u32| -> Option<String> {
            let (entity, def) = evaluator.lookup(id)?;
            match attribute(&entity, &def, "RepresentationIdentifier")? {
                Value::String(text) => Some(text.clone()),
                _ => None,
            }
        };
        let chosen = ["Body", "Facetation", "Mesh"].iter().find_map(|wanted| {
            representations
                .iter()
                .copied()
                .find(|id| identifier(self, *id).as_deref() == Some(*wanted))
        });

        let mut meshes = Vec::new();
        if let Some(representation) = chosen {
            self.representation(representation, placement, DEFAULT_RGBA, 0, &mut meshes);
        }
        meshes
    }

    fn representation(
        &mut self,
        id: u32,
        transform: DMat4,
        color: u32,
        depth: usize,
        out: &mut Vec<ElementMesh>,
    ) {
        let Some((entity, def)) = self.lookup(id) else { return };
        let items: Vec<u32> = list(attribute(&entity, &def, "Items"))
            .iter()
            .filter_map(reference)
            .collect();
        for item in items {
            self.item(item, transform, color, depth, out);
        }
    }

    fn item(
        &mut self,
        id: u32,
        transform: DMat4,
        inherited: u32,
        depth: usize,
        out: &mut Vec<ElementMesh>,
    ) {
        if depth >= MAX_DEPTH {
            self.warn("nesting-too-deep", format!("#{id} is nested too deeply"));
            return;
        }
        self.items_used += 1;
        if self.items_used > MAX_ITEMS_PER_ELEMENT {
            if self.items_used == MAX_ITEMS_PER_ELEMENT + 1 {
                self.warn("too-many-items", "representation expands too far".into());
            }
            return;
        }
        let Some((entity, def)) = self.lookup(id) else {
            self.warn("missing-item", format!("#{id} cannot be read"));
            return;
        };
        let color = self.item_color(id).unwrap_or(inherited);

        if def.is_a("IfcMappedItem") {
            self.mapped_item(&entity, &def, transform, color, depth, out);
            return;
        }
        let local = self.solid(id, &entity, &def, depth);

        if let Some(mesh) = local {
            out.push(ElementMesh {
                mesh: transformed(mesh, transform),
                rgba: color,
            });
        }
    }

    /// Exporters may style a boolean result or only the solid it started from.
    fn item_color(&self, id: u32) -> Option<u32> {
        let mut current = id;
        for _ in 0..MAX_DEPTH {
            if let Some(color) = self.item_colors.get(&current) {
                return Some(*color);
            }
            let (entity, def) = self.lookup(current)?;
            let next = attribute(&entity, &def, "FirstOperand")
                .or_else(|| attribute(&entity, &def, "TreeRootExpression"));
            current = next.and_then(reference)?;
        }
        None
    }

    /// Local geometry of a representation item, before any placement.
    fn solid(&mut self, id: u32, entity: &Entity, def: &EntityDef, depth: usize) -> Option<Mesh> {
        if def.is_a("IfcExtrudedAreaSolid") {
            self.extrusion(entity, def)
        } else if def.is_a("IfcBooleanResult") {
            self.boolean(entity, def, depth)
        } else if def.is_a("IfcCsgSolid") {
            let root = attribute(entity, def, "TreeRootExpression").and_then(reference)?;
            self.operand(root, depth + 1)
        } else if def.is_a("IfcRevolvedAreaSolid") {
            self.revolved(entity, def)
        } else if def.is_a("IfcSweptDiskSolid") {
            self.swept_disk(entity, def)
        } else if def.is_a("IfcCsgPrimitive3D") {
            self.primitive(id, entity, def)
        } else if def.is_a("IfcManifoldSolidBrep") {
            attribute(entity, def, "Outer")
                .and_then(reference)
                .and_then(|shell| self.shell(&[shell]))
        } else if def.is_a("IfcShellBasedSurfaceModel") {
            let shells: Vec<u32> = list(attribute(entity, def, "SbsmBoundary"))
                .iter()
                .filter_map(reference)
                .collect();
            self.shell(&shells)
        } else if def.is_a("IfcFaceBasedSurfaceModel") {
            let sets: Vec<u32> = list(attribute(entity, def, "FbsmFaces"))
                .iter()
                .filter_map(reference)
                .collect();
            self.shell(&sets)
        } else if def.is_a("IfcTriangulatedFaceSet") {
            self.triangulated_face_set(entity, def)
        } else if def.is_a("IfcPolygonalFaceSet") {
            self.polygonal_face_set(entity, def)
        } else {
            self.warn("unsupported-geometry", format!("#{id} {}", def.name()));
            None
        }
    }

    fn operand(&mut self, id: u32, depth: usize) -> Option<Mesh> {
        if depth >= MAX_DEPTH {
            self.warn("nesting-too-deep", format!("#{id} is nested too deeply"));
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        self.solid(id, &entity, &def, depth)
    }

    /// A failed boolean leaves the first operand: too much material beats a missing wall.
    fn boolean(&mut self, entity: &Entity, def: &EntityDef, depth: usize) -> Option<Mesh> {
        let first = attribute(entity, def, "FirstOperand").and_then(reference)?;
        let second = attribute(entity, def, "SecondOperand").and_then(reference)?;
        let first = self.operand(first, depth + 1)?;

        self.booleans_used += 1;
        if self.booleans_used > MAX_BOOLEANS_PER_ELEMENT {
            if self.booleans_used == MAX_BOOLEANS_PER_ELEMENT + 1 {
                self.warn("too-many-booleans", "boolean expression expands too far".into());
            }
            return Some(first);
        }

        let second = match self.lookup(second) {
            Some((half, half_def)) if half_def.is_a("IfcHalfSpaceSolid") => {
                self.half_space(&half, &half_def, &first)
            }
            _ => self.operand(second, depth + 1),
        };
        let Some(second) = second else {
            self.warn("boolean-failed", "second operand cannot be evaluated".into());
            return Some(first);
        };
        if !is_closed(&first) || !is_closed(&second) {
            self.warn("boolean-on-open-mesh", "boolean skipped: an operand is not closed".into());
            return Some(first);
        }

        let operator = match attribute(entity, def, "Operator") {
            Some(Value::Enum(operator)) => operator.as_str(),
            _ => "DIFFERENCE",
        };
        let result = match operator {
            "UNION" => union(&first, &second),
            "INTERSECTION" => intersect(&first, &second),
            _ => subtract(&first, &second),
        };
        match result {
            Ok(mesh) => (!mesh.is_empty()).then_some(mesh),
            Err(error) => {
                self.warn("boolean-failed", error.to_string());
                Some(first)
            }
        }
    }

    /// Half spaces are unbounded, so they become a block reaching past everything in `target`.
    fn half_space(&mut self, entity: &Entity, def: &EntityDef, target: &Mesh) -> Option<Mesh> {
        let surface = attribute(entity, def, "BaseSurface").and_then(reference)?;
        let (plane, plane_def) = self.lookup(surface)?;
        if !plane_def.is_a("IfcPlane") {
            self.warn("unsupported-surface", format!("#{surface} {}", plane_def.name()));
            return None;
        }
        let frame = attribute(&plane, &plane_def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let origin = frame.w_axis.truncate();
        let normal = frame.z_axis.truncate();
        // AgreementFlag is true when the normal points away from the material.
        let agrees = matches!(attribute(entity, def, "AgreementFlag"), Some(Value::Enum(flag)) if flag == "T");
        let into = if agrees { -normal } else { normal };

        let reach = |from: DVec3| {
            let furthest = target.positions.iter().fold(0.0_f64, |most, point| most.max(point.distance(from)));
            furthest * 2.0 + 1.0
        };
        let size = reach(origin);
        let (x, y) = solids::perpendiculars(into);
        let placement = DMat4::from_cols(x.extend(0.0), y.extend(0.0), into.extend(0.0), origin.extend(1.0));
        let block = Mesh::cuboid(DVec3::new(-size, -size, 0.0), DVec3::splat(size));
        let block = transformed(block, placement);
        if !def.is_a("IfcPolygonalBoundedHalfSpace") {
            return Some(block);
        }

        let position = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let boundary = attribute(entity, def, "PolygonalBoundary").and_then(reference)?;
        let outer = clean_ring(self.curve_points(boundary)?);
        if outer.len() < 3 {
            return None;
        }
        // The plane may tilt, so the prism must also reach the far side of the block.
        let length = reach(position.w_axis.truncate()) + size * 2.0;
        let prism_mesh = prism(Profile { outer, holes: Vec::new() }, DVec3::new(0.0, 0.0, length * 2.0));
        let start = DMat4::from_translation(DVec3::new(0.0, 0.0, -length));
        let prism_mesh = transformed(prism_mesh, position * start);
        match intersect(&block, &prism_mesh) {
            Ok(mesh) => Some(mesh),
            Err(error) => {
                self.warn("boolean-failed", error.to_string());
                None
            }
        }
    }

    fn revolved(&mut self, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let profile = attribute(entity, def, "SweptArea").and_then(reference)?;
        let profiles = self.profiles(profile, 0)?;
        let position = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let axis = attribute(entity, def, "Axis").and_then(reference)?;
        let (axis, axis_def) = self.lookup(axis)?;
        let axis_point = attribute(&axis, &axis_def, "Location")
            .and_then(reference)
            .and_then(|id| self.point(id))?;
        let direction = attribute(&axis, &axis_def, "Axis")
            .and_then(reference)
            .and_then(|id| self.direction(id))
            .unwrap_or(DVec3::Z)
            .try_normalize()?;
        let angle = attribute(entity, def, "Angle").and_then(number)? * self.units.plane_angle;
        if !(angle > 0.0) {
            return None;
        }

        let angle = angle.min(std::f64::consts::TAU);
        let parts = profiles.into_iter().map(|profile| {
            let profile = wound(profile);
            let mut rings: Vec<&[DVec2]> = vec![&profile.outer];
            rings.extend(profile.holes.iter().map(Vec::as_slice));
            let caps = triangulate_rings(&rings);
            solids::revolve(&rings, &caps, axis_point, direction, angle)
        });
        Some(transformed(merged(parts), position))
    }

    fn swept_disk(&mut self, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let directrix = attribute(entity, def, "Directrix").and_then(reference)?;
        let scale = self.units.length;
        let radius = attribute(entity, def, "Radius").and_then(number)? * scale;
        let inner = attribute(entity, def, "InnerRadius")
            .and_then(number)
            .map(|inner| inner * scale)
            .filter(|inner| *inner > 0.0 && *inner < radius);
        if !(radius > 0.0) {
            return None;
        }
        let trimmed = |name: &str| attribute(entity, def, name).and_then(number).is_some();
        if trimmed("StartParam") || trimmed("EndParam") {
            self.warn("sweep-trim-ignored", "swept along the whole directrix".into());
        }
        let mut path = self.curve_path(directrix, 0)?;
        path.dedup_by(|a, b| a.distance_squared(*b) <= POINT_TOLERANCE * POINT_TOLERANCE);
        (path.len() >= 2).then(|| solids::pipe(&path, radius, inner))
    }

    fn primitive(&mut self, id: u32, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let scale = self.units.length;
        let size = |name: &str| Some(attribute(entity, def, name).and_then(number)? * scale);
        let mesh = match def.name() {
            "IfcBlock" => {
                Mesh::cuboid(DVec3::ZERO, DVec3::new(size("XLength")?, size("YLength")?, size("ZLength")?))
            }
            "IfcRectangularPyramid" => {
                solids::pyramid(size("XLength")?, size("YLength")?, size("Height")?)
            }
            "IfcRightCircularCone" => solids::cone(size("BottomRadius")?, size("Height")?),
            "IfcRightCircularCylinder" => solids::cylinder(size("Radius")?, size("Height")?),
            "IfcSphere" => solids::sphere(size("Radius")?),
            other => {
                self.warn("unsupported-geometry", format!("#{id} {other}"));
                return None;
            }
        };
        let position = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        Some(transformed(mesh, position))
    }

    /// ISO 10303-43 semantics: moves the mapping origin onto the mapping target.
    fn mapped_item(
        &mut self,
        entity: &Entity,
        def: &EntityDef,
        transform: DMat4,
        color: u32,
        depth: usize,
        out: &mut Vec<ElementMesh>,
    ) {
        let source = attribute(entity, def, "MappingSource").and_then(reference);
        let Some((map, map_def)) = source.and_then(|id| self.lookup(id)) else {
            self.warn("bad-mapped-item", "unreadable mapping source".into());
            return;
        };
        let target = attribute(entity, def, "MappingTarget")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.transformation_operator(id));
        let origin = attribute(&map, &map_def, "MappingOrigin")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let Some(representation) =
            attribute(&map, &map_def, "MappedRepresentation").and_then(reference)
        else {
            return;
        };
        let mapped = transform * target * origin.inverse();
        self.representation(representation, mapped, color, depth + 1, out);
    }

    fn transformation_operator(&mut self, id: u32) -> DMat4 {
        let Some((entity, def)) = self.lookup(id) else {
            return DMat4::IDENTITY;
        };
        let direction = |evaluator: &Self, name: &str| {
            attribute(&entity, &def, name)
                .and_then(reference)
                .and_then(|id| evaluator.direction(id))
        };
        let x_hint = direction(self, "Axis1").unwrap_or(DVec3::X);
        let y_hint = direction(self, "Axis2").unwrap_or(DVec3::Y);
        let z_hint = direction(self, "Axis3").unwrap_or(DVec3::Z);
        let origin = attribute(&entity, &def, "LocalOrigin")
            .and_then(reference)
            .and_then(|id| self.point(id))
            .unwrap_or(DVec3::ZERO);
        let scale = attribute(&entity, &def, "Scale").and_then(number).unwrap_or(1.0);
        let scale_y = attribute(&entity, &def, "Scale2").and_then(number).unwrap_or(scale);
        let scale_z = attribute(&entity, &def, "Scale3").and_then(number).unwrap_or(scale);

        let z = z_hint.try_normalize().unwrap_or(DVec3::Z);
        let x = (x_hint - z * x_hint.dot(z)).try_normalize().unwrap_or(DVec3::X);
        let mut y = z.cross(x);
        if y.dot(y_hint) < 0.0 {
            y = -y;
        }
        DMat4::from_cols(
            (x * scale).extend(0.0),
            (y * scale_y).extend(0.0),
            (z * scale_z).extend(0.0),
            DVec4::new(origin.x, origin.y, origin.z, 1.0),
        )
    }

    fn point(&self, id: u32) -> Option<DVec3> {
        let (entity, def) = self.lookup(id)?;
        let coordinates = list(attribute(&entity, &def, "Coordinates"));
        let value = |index: usize| coordinates.get(index).and_then(number);
        Some(DVec3::new(value(0)?, value(1)?, value(2).unwrap_or(0.0)) * self.units.length)
    }

    fn direction(&self, id: u32) -> Option<DVec3> {
        let (entity, def) = self.lookup(id)?;
        let ratios = list(attribute(&entity, &def, "DirectionRatios"));
        let value = |index: usize| ratios.get(index).and_then(number);
        Some(DVec3::new(value(0)?, value(1)?, value(2).unwrap_or(0.0)))
    }

    /// An `IfcAxis2Placement2D` or `IfcAxis2Placement3D`.
    fn placement_matrix(&mut self, id: u32) -> DMat4 {
        let Some((entity, def)) = self.lookup(id) else {
            self.warn("bad-placement", format!("#{id} cannot be read"));
            return DMat4::IDENTITY;
        };
        let location = attribute(&entity, &def, "Location")
            .and_then(reference)
            .and_then(|id| self.point(id))
            .unwrap_or(DVec3::ZERO);
        let axis = attribute(&entity, &def, "Axis")
            .and_then(reference)
            .and_then(|id| self.direction(id));
        let x_hint = attribute(&entity, &def, "RefDirection")
            .and_then(reference)
            .and_then(|id| self.direction(id))
            .unwrap_or(DVec3::X);

        let z = axis.and_then(DVec3::try_normalize).unwrap_or(DVec3::Z);
        let x = (x_hint - z * x_hint.dot(z))
            .try_normalize()
            .unwrap_or_else(|| z.any_orthonormal_vector());
        let y = z.cross(x);
        DMat4::from_cols(
            x.extend(0.0),
            y.extend(0.0),
            z.extend(0.0),
            location.extend(1.0),
        )
    }

    fn placement(&mut self, id: u32, depth: usize) -> DMat4 {
        if let Some(cached) = self.placements.get(&id) {
            return *cached;
        }
        if depth >= MAX_DEPTH {
            self.warn("bad-placement", "placement chain is too deep or cyclic".into());
            return DMat4::IDENTITY;
        }
        let Some((entity, def)) = self.lookup(id) else {
            return DMat4::IDENTITY;
        };
        if !def.is_a("IfcLocalPlacement") {
            self.warn("unsupported-placement", format!("#{id} {}", def.name()));
            return DMat4::IDENTITY;
        }
        let parent = match attribute(&entity, &def, "PlacementRelTo").and_then(reference) {
            Some(parent) => self.placement(parent, depth + 1),
            None => DMat4::IDENTITY,
        };
        let relative = attribute(&entity, &def, "RelativePlacement")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let world = parent * relative;
        self.placements.insert(id, world);
        world
    }

    fn curve_points(&mut self, id: u32) -> Option<Vec<DVec2>> {
        let path = self.curve_path(id, 0)?;
        Some(path.into_iter().map(DVec3::truncate).collect())
    }

    /// A curve as a polyline. Arcs are tessellated, not replaced by their chords.
    fn curve_path(&mut self, id: u32, depth: usize) -> Option<Vec<DVec3>> {
        // Composite curves can share segments, so depth alone leaves exponentially many.
        self.items_used += 1;
        if depth >= MAX_DEPTH || self.items_used > MAX_ITEMS_PER_ELEMENT {
            self.warn("nesting-too-deep", format!("#{id} is nested too deeply"));
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        if def.is_a("IfcPolyline") {
            list(attribute(&entity, &def, "Points"))
                .iter()
                .map(|point| self.point(reference(point)?))
                .collect()
        } else if def.is_a("IfcIndexedPolyCurve") {
            self.indexed_curve(&entity, &def)
        } else if def.is_a("IfcCompositeCurve") {
            let mut path = Vec::new();
            for segment in list(attribute(&entity, &def, "Segments")).iter().filter_map(reference) {
                let (segment, segment_def) = self.lookup(segment)?;
                let parent = attribute(&segment, &segment_def, "ParentCurve").and_then(reference)?;
                let mut part = self.curve_path(parent, depth + 1)?;
                if flag(attribute(&segment, &segment_def, "SameSense")) == Some(false) {
                    part.reverse();
                }
                join(&mut path, part);
            }
            Some(path)
        } else if def.is_a("IfcTrimmedCurve") {
            self.trimmed_curve(&entity, &def)
        } else if def.is_a("IfcBSplineCurveWithKnots") {
            self.bspline_curve(&entity, &def)
        } else if def.is_a("IfcSurfaceCurve") {
            let curve = attribute(&entity, &def, "Curve3D").and_then(reference)?;
            self.curve_path(curve, depth + 1)
        } else if def.is_a("IfcCircle") || def.is_a("IfcEllipse") {
            let (frame, radii) = self.conic(&entity, &def)?;
            Some(on_conic(frame, radii, 0.0, std::f64::consts::TAU))
        } else {
            self.warn("unsupported-curve", format!("#{id} {}", def.name()));
            None
        }
    }

    fn bspline_curve(&mut self, entity: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let degree = attribute(entity, def, "Degree").and_then(number)? as usize;
        let points: Vec<DVec3> = list(attribute(entity, def, "ControlPointsList"))
            .iter()
            .map(|point| self.point(reference(point)?))
            .collect::<Option<_>>()?;
        let knots = knot_vector(entity, def, "Knots", "KnotMultiplicities", degree, points.len())?;
        let weights = numbers(attribute(entity, def, "WeightsData"));
        let weights = if weights.is_empty() { vec![1.0; points.len()] } else { weights };
        if weights.len() != points.len() {
            return None;
        }
        nurbs::Curve { degree, points, weights, knots }.sample()
    }

    fn bspline_surface(&mut self, entity: &Entity, def: &EntityDef) -> Option<nurbs::Surface> {
        let u_degree = attribute(entity, def, "UDegree").and_then(number)? as usize;
        let v_degree = attribute(entity, def, "VDegree").and_then(number)? as usize;
        let points: Vec<Vec<DVec3>> = list(attribute(entity, def, "ControlPointsList"))
            .iter()
            .map(|row| {
                list(Some(row))
                    .iter()
                    .map(|point| self.point(reference(point)?))
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<Option<_>>()?;
        let (rows, columns) = (points.len(), points.first()?.len());
        if points.iter().any(|row| row.len() != columns) {
            return None;
        }
        let u_knots = knot_vector(entity, def, "UKnots", "UMultiplicities", u_degree, rows)?;
        let v_knots = knot_vector(entity, def, "VKnots", "VMultiplicities", v_degree, columns)?;
        let weights: Vec<Vec<f64>> = list(attribute(entity, def, "WeightsData"))
            .iter()
            .map(|row| numbers(Some(row)))
            .collect();
        let weights = if weights.is_empty() { vec![vec![1.0; columns]; rows] } else { weights };
        if weights.len() != rows || weights.iter().any(|row| row.len() != columns) {
            return None;
        }
        Some(nurbs::Surface { u_degree, v_degree, points, weights, u_knots, v_knots })
    }

    fn vertex_point(&self, id: u32) -> Option<DVec3> {
        let (vertex, def) = self.lookup(id)?;
        self.point(attribute(&vertex, &def, "VertexGeometry").and_then(reference)?)
    }

    /// An edge from its start vertex to its end vertex. The vertices are authoritative, so
    /// faces sharing an edge share its end points exactly.
    fn edge_points(&mut self, edge: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let start = self.vertex_point(attribute(edge, def, "EdgeStart").and_then(reference)?)?;
        let end = self.vertex_point(attribute(edge, def, "EdgeEnd").and_then(reference)?)?;
        let geometry = attribute(edge, def, "EdgeGeometry").and_then(reference)?;
        let same_sense = flag(attribute(edge, def, "SameSense")).unwrap_or(true);
        let (curve, curve_def) = self.lookup(geometry)?;

        let mut points = if curve_def.is_a("IfcLine") {
            vec![start, end]
        } else if curve_def.is_a("IfcCircle") || curve_def.is_a("IfcEllipse") {
            let (frame, radii) = self.conic(&curve, &curve_def)?;
            let inverse = frame.inverse();
            let angle = |point: DVec3| {
                let local = inverse.transform_point3(point);
                (local.y / radii.y).atan2(local.x / radii.x)
            };
            let tau = std::f64::consts::TAU;
            let turn = |amount: f64| match amount.rem_euclid(tau) {
                amount if amount < 1e-9 => tau,
                amount => amount,
            };
            let sweep = if same_sense { turn(angle(end) - angle(start)) } else { -turn(angle(start) - angle(end)) };
            on_conic(frame, radii, angle(start), sweep)
        } else {
            let mut points = self.curve_path(geometry, 0)?;
            if !same_sense {
                points.reverse();
            }
            between_vertices(points, start, end)
        };
        *points.first_mut()? = start;
        *points.last_mut()? = end;
        Some(points)
    }

    fn edge_loop(&mut self, entity: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let mut edges = Vec::new();
        for oriented in list(attribute(entity, def, "EdgeList")).iter().filter_map(reference) {
            let (oriented, oriented_def) = self.lookup(oriented)?;
            let element = attribute(&oriented, &oriented_def, "EdgeElement").and_then(reference)?;
            let (edge, edge_def) = self.lookup(element)?;
            let mut points = self.edge_points(&edge, &edge_def)?;
            if flag(attribute(&oriented, &oriented_def, "Orientation")) == Some(false) {
                points.reverse();
            }
            edges.push(points);
        }
        chain_edges(&mut edges);

        let mut path = Vec::new();
        for points in edges {
            join(&mut path, points);
        }
        // The loop closes on its first vertex, which should not appear twice.
        if path.len() > 1 && path[0].distance_squared(path[path.len() - 1]) <= POINT_TOLERANCE * POINT_TOLERANCE {
            path.pop();
        }
        (path.len() >= 3).then_some(path)
    }

    /// A face on a curved surface, meshed in that surface's parameter space.
    fn curved_face(
        &mut self,
        surface: &nurbs::Surface,
        loops: &[Vec<DVec3>],
        same_sense: bool,
        builder: &mut FaceBuilder,
    ) {
        let Some(mesh) = nurbs::mesh_surface(surface, loops) else {
            self.warn("bad-trim", "face boundary does not lie on its surface".into());
            return;
        };
        for triangle in mesh.triangles {
            let [a, b, c] = triangle.map(|id| builder.vertex(mesh.positions[id as usize]));
            builder.mesh.triangles.push(if same_sense { [a, b, c] } else { [a, c, b] });
        }
    }

    fn indexed_curve(&mut self, entity: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let points = attribute(entity, def, "Points").and_then(reference)?;
        let (points, points_def) = self.lookup(points)?;
        let scale = self.units.length;
        let coordinates: Vec<DVec3> = list(attribute(&points, &points_def, "CoordList"))
            .iter()
            .map(|point| {
                let point = list(Some(point));
                let value = |index: usize| point.get(index).and_then(number);
                Some(DVec3::new(value(0)?, value(1)?, value(2).unwrap_or(0.0)) * scale)
            })
            .collect::<Option<_>>()?;

        let segments = list(attribute(entity, def, "Segments"));
        if segments.is_empty() {
            return Some(coordinates);
        }
        let mut path = Vec::new();
        for segment in segments {
            let Value::Typed { name, value } = segment else { return None };
            let corners: Vec<DVec3> = list(Some(value))
                .iter()
                .map(|index| coordinates.get((number(index)? as usize).checked_sub(1)?).copied())
                .collect::<Option<_>>()?;
            match (name.as_str(), corners.as_slice()) {
                ("IFCARCINDEX", [a, b, c]) => join(&mut path, solids::arc_through(*a, *b, *c)),
                _ => join(&mut path, corners),
            }
        }
        Some(path)
    }

    /// The placement of a circle or ellipse, and its semi-axes.
    fn conic(&mut self, entity: &Entity, def: &EntityDef) -> Option<(DMat4, DVec2)> {
        let frame = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let scale = self.units.length;
        let size = |name: &str| Some(attribute(entity, def, name).and_then(number)? * scale);
        let radii = if def.is_a("IfcEllipse") {
            DVec2::new(size("SemiAxis1")?, size("SemiAxis2")?)
        } else {
            DVec2::splat(size("Radius")?)
        };
        Some((frame, radii))
    }

    fn trimmed_curve(&mut self, entity: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let basis = attribute(entity, def, "BasisCurve").and_then(reference)?;
        let (basis, basis_def) = self.lookup(basis)?;
        let forward = flag(attribute(entity, def, "SenseAgreement")).unwrap_or(true);
        let by_parameter = !matches!(
            attribute(entity, def, "MasterRepresentation"),
            Some(Value::Enum(master)) if master == "CARTESIAN"
        );
        let trim1 = list(attribute(entity, def, "Trim1"));
        let trim2 = list(attribute(entity, def, "Trim2"));
        let parameter = |trim: &[Value]| {
            trim.iter().find_map(|value| match value {
                Value::Typed { name, value } if name == "IFCPARAMETERVALUE" => number(value),
                _ => None,
            })
        };
        let cartesian = |evaluator: &Self, trim: &[Value]| {
            trim.iter().filter_map(reference).find_map(|point| evaluator.point(point))
        };

        if basis_def.is_a("IfcLine") {
            let start = attribute(&basis, &basis_def, "Pnt").and_then(reference).and_then(|id| self.point(id))?;
            let vector = attribute(&basis, &basis_def, "Dir").and_then(reference)?;
            let (vector, vector_def) = self.lookup(vector)?;
            let orientation = attribute(&vector, &vector_def, "Orientation")
                .and_then(reference)
                .and_then(|id| self.direction(id))?
                .try_normalize()?;
            let magnitude = attribute(&vector, &vector_def, "Magnitude").and_then(number)?;
            let step = orientation * magnitude * self.units.length;
            let end = |evaluator: &Self, trim: &[Value]| {
                let from_parameter = parameter(trim).map(|value| start + step * value);
                let from_point = cartesian(evaluator, trim);
                if by_parameter { from_parameter.or(from_point) } else { from_point.or(from_parameter) }
            };
            return Some(vec![end(self, trim1)?, end(self, trim2)?]);
        }

        if !basis_def.is_a("IfcCircle") && !basis_def.is_a("IfcEllipse") {
            self.warn("unsupported-curve", format!("trimmed {}", basis_def.name()));
            return None;
        }
        let (frame, radii) = self.conic(&basis, &basis_def)?;
        let inverse = frame.inverse();
        let angle_scale = self.units.plane_angle;
        let angle = |evaluator: &Self, trim: &[Value]| {
            let from_parameter = parameter(trim).map(|value| value * angle_scale);
            let from_point = cartesian(evaluator, trim).map(|point| {
                let local = inverse.transform_point3(point);
                (local.y / radii.y).atan2(local.x / radii.x)
            });
            if by_parameter { from_parameter.or(from_point) } else { from_point.or(from_parameter) }
        };
        let (start, end) = (angle(self, trim1)?, angle(self, trim2)?);
        let tau = std::f64::consts::TAU;
        let turn = |amount: f64| match amount.rem_euclid(tau) {
            0.0 => tau,
            amount => amount,
        };
        let sweep = if forward { turn(end - start) } else { -turn(start - end) };
        Some(on_conic(frame, radii, start, sweep))
    }

    fn profile(&mut self, id: u32) -> Option<Profile> {
        let (entity, def) = self.lookup(id)?;
        let scale = self.units.length;
        let curve = |name: &str| attribute(&entity, &def, name).and_then(reference);

        let (outer, holes) = if def.is_a("IfcArbitraryClosedProfileDef") {
            let outer = self.curve_points(curve("OuterCurve")?)?;
            let mut holes = Vec::new();
            if def.is_a("IfcArbitraryProfileDefWithVoids") {
                for inner in list(attribute(&entity, &def, "InnerCurves")) {
                    let hole = self.curve_points(reference(inner)?)?;
                    holes.push(hole);
                }
            }
            (outer, holes)
        } else if def.name() == "IfcEllipseProfileDef" {
            let size = |name: &str| Some(attribute(&entity, &def, name).and_then(number)? * scale);
            let radii = DVec2::new(size("SemiAxis1")?, size("SemiAxis2")?);
            let ring = solids::arc(DVec2::ZERO, radii, 0.0, std::f64::consts::TAU);
            (self.placed_profile(&entity, &def, ring), Vec::new())
        } else if def.name() == "IfcCircleProfileDef" || def.is_a("IfcCircleHollowProfileDef") {
            let radius = attribute(&entity, &def, "Radius").and_then(number)? * scale;
            let circle = |radius: f64| solids::arc(DVec2::ZERO, DVec2::splat(radius), 0.0, std::f64::consts::TAU);
            let wall = attribute(&entity, &def, "WallThickness").and_then(number).map(|wall| wall * scale);
            let holes = match wall.filter(|wall| *wall > 0.0 && *wall < radius) {
                Some(wall) => vec![self.placed_profile(&entity, &def, circle(radius - wall))],
                None => Vec::new(),
            };
            (self.placed_profile(&entity, &def, circle(radius)), holes)
        } else if def.is_a("IfcRectangleProfileDef") {
            let size = |name: &str| attribute(&entity, &def, name).and_then(number).map(|value| value * scale);
            let (x, y) = (size("XDim")? / 2.0, size("YDim")? / 2.0);
            let box_of = |x: f64, y: f64, radius: f64| {
                solids::rounded(&[(-x, -y, radius), (x, -y, radius), (x, y, radius), (-x, y, radius)])
            };
            let radius = size("RoundingRadius").or_else(|| size("OuterFilletRadius")).unwrap_or(0.0);
            let wall = size("WallThickness").filter(|wall| *wall > 0.0 && *wall < x.min(y));
            let holes = match wall {
                Some(wall) => {
                    let inner = box_of(x - wall, y - wall, size("InnerFilletRadius").unwrap_or(0.0));
                    vec![self.placed_profile(&entity, &def, inner)]
                }
                None => Vec::new(),
            };
            (self.placed_profile(&entity, &def, box_of(x, y, radius)), holes)
        } else if let Some(corners) = self.section(&entity, &def) {
            let ring = solids::rounded(&corners);
            (self.placed_profile(&entity, &def, ring), Vec::new())
        } else {
            self.warn("unsupported-profile", format!("#{id} {}", def.name()));
            return None;
        };

        let outer = clean_ring(outer);
        let holes: Vec<Vec<DVec2>> = holes
            .into_iter()
            .map(clean_ring)
            .filter(|hole| hole.len() >= 3)
            .collect();
        (outer.len() >= 3).then_some(Profile { outer, holes })
    }

    /// The outline of a steel section, in its own centred coordinates.
    fn section(&mut self, entity: &Entity, def: &EntityDef) -> Option<Vec<solids::Corner>> {
        let scale = self.units.length;
        let size = |name: &str| Some(attribute(entity, def, name).and_then(number)? * scale);
        let radius = |name: &str| size(name).unwrap_or(0.0);
        let sloped = ["FlangeSlope", "WebSlope", "LegSlope"]
            .iter()
            .any(|name| attribute(entity, def, name).and_then(number).is_some_and(|slope| slope != 0.0));

        let corners = match def.name() {
            "IfcIShapeProfileDef" => solids::i_section(
                size("OverallWidth")?,
                size("OverallDepth")?,
                size("WebThickness")?,
                size("FlangeThickness")?,
                radius("FilletRadius"),
                radius("FlangeEdgeRadius"),
            ),
            "IfcLShapeProfileDef" => {
                let depth = size("Depth")?;
                solids::l_section(
                    depth,
                    size("Width").unwrap_or(depth),
                    size("Thickness")?,
                    radius("FilletRadius"),
                    radius("EdgeRadius"),
                )
            }
            "IfcTShapeProfileDef" => solids::t_section(
                size("Depth")?,
                size("FlangeWidth")?,
                size("WebThickness")?,
                size("FlangeThickness")?,
                [radius("FilletRadius"), radius("FlangeEdgeRadius"), radius("WebEdgeRadius")],
            ),
            "IfcUShapeProfileDef" => solids::u_section(
                size("Depth")?,
                size("FlangeWidth")?,
                size("WebThickness")?,
                size("FlangeThickness")?,
                radius("FilletRadius"),
                radius("EdgeRadius"),
            ),
            "IfcCShapeProfileDef" => solids::c_section(
                size("Depth")?,
                size("Width")?,
                size("WallThickness")?,
                size("Girth")?,
                radius("InternalFilletRadius"),
            ),
            "IfcZShapeProfileDef" => solids::z_section(
                size("Depth")?,
                size("FlangeWidth")?,
                size("WebThickness")?,
                size("FlangeThickness")?,
                radius("FilletRadius"),
                radius("EdgeRadius"),
            ),
            _ => return None,
        };
        if sloped {
            self.warn("profile-slope-ignored", format!("{} drawn with parallel faces", def.name()));
        }
        Some(corners)
    }

    /// Every closed area of a profile. Composite profiles have several.
    fn profiles(&mut self, id: u32, depth: usize) -> Option<Vec<Profile>> {
        self.items_used += 1;
        if depth >= MAX_DEPTH || self.items_used > MAX_ITEMS_PER_ELEMENT {
            self.warn("nesting-too-deep", format!("#{id} is nested too deeply"));
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        if def.is_a("IfcCompositeProfileDef") {
            let mut all = Vec::new();
            for part in list(attribute(&entity, &def, "Profiles")).iter().filter_map(reference) {
                all.extend(self.profiles(part, depth + 1)?);
            }
            return Some(all);
        }
        if def.is_a("IfcDerivedProfileDef") {
            let parent = attribute(&entity, &def, "ParentProfile").and_then(reference)?;
            let parents = self.profiles(parent, depth + 1)?;
            // A mirrored profile flips across the Y axis, whatever its derived operator says.
            let matrix = if def.is_a("IfcMirroredProfileDef") {
                DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0))
            } else {
                attribute(&entity, &def, "Operator")
                    .and_then(reference)
                    .map_or(DMat4::IDENTITY, |id| self.transformation_operator(id))
            };
            let apply = |ring: Vec<DVec2>| -> Vec<DVec2> {
                ring.into_iter()
                    .map(|point| matrix.transform_point3(point.extend(0.0)).truncate())
                    .collect()
            };
            return Some(
                parents
                    .into_iter()
                    .map(|Profile { outer, holes }| Profile {
                        outer: apply(outer),
                        holes: holes.into_iter().map(apply).collect(),
                    })
                    .collect(),
            );
        }
        self.profile(id).map(|profile| vec![profile])
    }

    fn placed_profile(&mut self, entity: &Entity, def: &EntityDef, ring: Vec<DVec2>) -> Vec<DVec2> {
        let Some(position) = attribute(entity, def, "Position").and_then(reference) else {
            return ring;
        };
        let matrix = self.placement_matrix(position);
        ring.into_iter()
            .map(|point| matrix.transform_point3(point.extend(0.0)).truncate())
            .collect()
    }

    fn extrusion(&mut self, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let profile = attribute(entity, def, "SweptArea").and_then(reference)?;
        let profiles = self.profiles(profile, 0)?;
        let depth = attribute(entity, def, "Depth").and_then(number)? * self.units.length;
        let direction = attribute(entity, def, "ExtrudedDirection")
            .and_then(reference)
            .and_then(|id| self.direction(id))?
            .try_normalize()?;
        let position = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));
        let parts = profiles.into_iter().map(|profile| prism(profile, direction * depth));
        Some(transformed(merged(parts), position))
    }

    /// Faces from closed shells, open shells or connected face sets.
    fn shell(&mut self, shells: &[u32]) -> Option<Mesh> {
        let mut builder = FaceBuilder::default();
        for shell in shells {
            let (entity, def) = self.lookup(*shell)?;
            for face in list(attribute(&entity, &def, "CfsFaces")) {
                if let Some(face) = reference(face) {
                    self.face(face, &mut builder);
                }
            }
        }
        (!builder.mesh.is_empty()).then_some(builder.mesh)
    }

    fn face(&mut self, id: u32, builder: &mut FaceBuilder) {
        let Some((entity, def)) = self.lookup(id) else { return };
        let mut outer: Option<Vec<DVec3>> = None;
        let mut holes: Vec<Vec<DVec3>> = Vec::new();
        for bound in list(attribute(&entity, &def, "Bounds")) {
            let Some((bound, bound_def)) = reference(bound).and_then(|id| self.lookup(id)) else {
                continue;
            };
            let Some((poly, poly_def)) = attribute(&bound, &bound_def, "Bound")
                .and_then(reference)
                .and_then(|id| self.lookup(id))
            else {
                continue;
            };
            let ring = if poly_def.is_a("IfcPolyLoop") {
                Some(
                    list(attribute(&poly, &poly_def, "Polygon"))
                        .iter()
                        .filter_map(reference)
                        .filter_map(|point| self.point(point))
                        .collect::<Vec<_>>(),
                )
            } else if poly_def.is_a("IfcEdgeLoop") {
                self.edge_loop(&poly, &poly_def)
            } else {
                self.warn("unsupported-face-bound", poly_def.name().to_string());
                None
            };
            let Some(mut ring) = ring else { continue };
            if flag(attribute(&bound, &bound_def, "Orientation")) == Some(false) {
                ring.reverse();
            }
            if bound_def.is_a("IfcFaceOuterBound") && outer.is_none() {
                outer = Some(ring);
            } else {
                holes.push(ring);
            }
        }
        if outer.is_none() && !holes.is_empty() {
            outer = Some(holes.remove(0));
        }
        let Some(outer) = outer else { return };

        let same_sense = flag(attribute(&entity, &def, "SameSense")).unwrap_or(true);
        let surface = attribute(&entity, &def, "FaceSurface")
            .and_then(reference)
            .and_then(|id| self.lookup(id));
        if let Some((surface, surface_def)) = &surface {
            if surface_def.is_a("IfcBSplineSurfaceWithKnots") {
                if let Some(surface) = self.bspline_surface(surface, surface_def) {
                    let mut loops = vec![outer];
                    loops.append(&mut holes);
                    self.curved_face(&surface, &loops, same_sense, builder);
                    return;
                }
                self.warn("bad-surface", format!("#{id} has an unusable surface"));
            } else if !surface_def.is_a("IfcPlane") {
                self.warn("unsupported-surface", format!("#{id} {}", surface_def.name()));
            }
        }

        // Planar, or the nearest we can do: a flat face across the boundary.
        let mut rings = std::iter::once(outer).chain(holes).map(|ring| {
            let mut ids: Vec<u32> = ring.into_iter().map(|point| builder.vertex(point)).collect();
            if !same_sense {
                ids.reverse();
            }
            ids
        });
        let first = rings.next().unwrap_or_default();
        let others: Vec<Vec<u32>> = rings.collect();
        builder.face(&first, &others);
    }

    fn coordinate_list(&self, entity: &Entity, def: &EntityDef) -> Option<Vec<DVec3>> {
        let id = attribute(entity, def, "Coordinates").and_then(reference)?;
        let (points, points_def) = self.lookup(id)?;
        list(attribute(&points, &points_def, "CoordList"))
            .iter()
            .map(|triple| {
                let triple = list(Some(triple));
                let value = |index: usize| triple.get(index).and_then(number);
                Some(DVec3::new(value(0)?, value(1)?, value(2)?) * self.units.length)
            })
            .collect()
    }

    fn triangulated_face_set(&mut self, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let positions = self.coordinate_list(entity, def)?;
        let lookup = pn_index(entity, def);
        let triangles: Vec<[u32; 3]> = list(attribute(entity, def, "CoordIndex"))
            .iter()
            .filter_map(|triangle| {
                let triangle = list(Some(triangle));
                let corner = |index: usize| {
                    resolve_index(number(triangle.get(index)?)?, &lookup, positions.len())
                };
                Some([corner(0)?, corner(1)?, corner(2)?])
            })
            .collect();
        (!triangles.is_empty()).then_some(Mesh {
            positions,
            triangles,
        })
    }

    fn polygonal_face_set(&mut self, entity: &Entity, def: &EntityDef) -> Option<Mesh> {
        let positions = self.coordinate_list(entity, def)?;
        let lookup = pn_index(entity, def);
        let ring = |values: &[Value]| -> Vec<u32> {
            values
                .iter()
                .filter_map(|value| resolve_index(number(value)?, &lookup, positions.len()))
                .collect()
        };

        let mut builder = FaceBuilder {
            mesh: Mesh {
                positions: positions.clone(),
                triangles: Vec::new(),
            },
            ids: HashMap::new(),
        };
        for face in list(attribute(entity, def, "Faces")).iter().filter_map(reference) {
            let Some((face, face_def)) = self.lookup(face) else { continue };
            let outer = ring(list(attribute(&face, &face_def, "CoordIndex")));
            let holes: Vec<Vec<u32>> = list(attribute(&face, &face_def, "InnerCoordIndices"))
                .iter()
                .map(|hole| ring(list(Some(hole))))
                .collect();
            builder.face(&outer, &holes);
        }
        (!builder.mesh.is_empty()).then_some(builder.mesh)
    }
}

fn attribute<'e>(entity: &'e Entity, def: &EntityDef, name: &str) -> Option<&'e Value> {
    entity
        .params
        .get(def.attribute_index(name)?)
        .filter(|value| !matches!(value, Value::Null | Value::Derived))
}

fn pn_index(entity: &Entity, def: &EntityDef) -> Vec<u32> {
    list(attribute(entity, def, "PnIndex"))
        .iter()
        .filter_map(|value| Some(number(value)? as u32))
        .collect()
}

/// Converts a one-based index, through `PnIndex` if present, to a checked position.
fn resolve_index(value: f64, lookup: &[u32], count: usize) -> Option<u32> {
    let one_based = value as u32;
    let index = if lookup.is_empty() {
        one_based
    } else {
        *lookup.get(one_based.checked_sub(1)? as usize)?
    };
    let index = index.checked_sub(1)?;
    ((index as usize) < count).then_some(index)
}

/// Assembles planar faces, sharing coincident vertices.
#[derive(Default)]
struct FaceBuilder {
    mesh: Mesh,
    ids: HashMap<[u64; 3], u32>,
}

impl FaceBuilder {
    fn vertex(&mut self, position: DVec3) -> u32 {
        let key = [position.x.to_bits(), position.y.to_bits(), position.z.to_bits()];
        *self.ids.entry(key).or_insert_with(|| {
            self.mesh.positions.push(position);
            (self.mesh.positions.len() - 1) as u32
        })
    }

    fn face(&mut self, outer: &[u32], holes: &[Vec<u32>]) {
        if outer.len() < 3 {
            return;
        }
        let corner = |id: u32| self.mesh.positions[id as usize];
        // Newell's method, which tolerates collinear and slightly non-planar corners.
        let normal = outer.iter().enumerate().fold(DVec3::ZERO, |sum, (i, id)| {
            sum + corner(*id).cross(corner(outer[(i + 1) % outer.len()]))
        });
        let Some(normal) = normal.try_normalize() else {
            return;
        };
        let u = normal.any_orthonormal_vector();
        let v = normal.cross(u);
        let flatten = |id: u32| DVec2::new(corner(id).dot(u), corner(id).dot(v));

        let mut ids: Vec<u32> = outer.to_vec();
        let mut rings: Vec<Vec<DVec2>> = vec![outer.iter().map(|id| flatten(*id)).collect()];
        for hole in holes.iter().filter(|hole| hole.len() >= 3) {
            ids.extend_from_slice(hole);
            rings.push(hole.iter().map(|id| flatten(*id)).collect());
        }
        let borrowed: Vec<&[DVec2]> = rings.iter().map(Vec::as_slice).collect();
        let triangles = triangulate_rings(&borrowed);
        for [a, b, c] in triangles {
            self.mesh
                .triangles
                .push([ids[a as usize], ids[b as usize], ids[c as usize]]);
        }
    }
}
