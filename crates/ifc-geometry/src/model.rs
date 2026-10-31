//! Turns IFC products into world-space meshes, in metres.
//!
//! Every failure is local to its element (R8): what cannot be evaluated becomes a
//! [`Diagnostic`], and the rest of the model carries on.

use crate::csg::subtract_all;
use crate::mesh::Mesh;
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
const DEFAULT_RGBA: u32 = 0xB8B8_B8FF;
const CIRCLE_SEGMENTS: usize = 32;
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
        let color = self.item_colors.get(&id).copied().unwrap_or(inherited);

        if def.is_a("IfcMappedItem") {
            self.mapped_item(&entity, &def, transform, color, depth, out);
            return;
        }
        let local = if def.is_a("IfcExtrudedAreaSolid") {
            self.extrusion(&entity, &def)
        } else if def.is_a("IfcFacetedBrep") {
            attribute(&entity, &def, "Outer")
                .and_then(reference)
                .and_then(|shell| self.shell(&[shell]))
        } else if def.is_a("IfcShellBasedSurfaceModel") {
            let shells: Vec<u32> = list(attribute(&entity, &def, "SbsmBoundary"))
                .iter()
                .filter_map(reference)
                .collect();
            self.shell(&shells)
        } else if def.is_a("IfcFaceBasedSurfaceModel") {
            let sets: Vec<u32> = list(attribute(&entity, &def, "FbsmFaces"))
                .iter()
                .filter_map(reference)
                .collect();
            self.shell(&sets)
        } else if def.is_a("IfcTriangulatedFaceSet") {
            self.triangulated_face_set(&entity, &def)
        } else if def.is_a("IfcPolygonalFaceSet") {
            self.polygonal_face_set(&entity, &def)
        } else {
            self.warn("unsupported-geometry", format!("#{id} {}", def.name()));
            None
        };

        if let Some(mesh) = local {
            out.push(ElementMesh {
                mesh: transformed(mesh, transform),
                rgba: color,
            });
        }
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
        let (entity, def) = self.lookup(id)?;
        if def.is_a("IfcPolyline") {
            return list(attribute(&entity, &def, "Points"))
                .iter()
                .map(|point| {
                    let point = self.point(reference(point)?)?;
                    Some(point.truncate())
                })
                .collect();
        }
        if def.is_a("IfcIndexedPolyCurve") {
            if attribute(&entity, &def, "Segments").is_some_and(|segments| {
                list(Some(segments))
                    .iter()
                    .any(|segment| matches!(segment, Value::Typed { name, .. } if name == "IFCARCINDEX"))
            }) {
                self.warn("arc-approximated", format!("#{id} arcs treated as chords"));
            }
            let points = attribute(&entity, &def, "Points").and_then(reference)?;
            let (points, points_def) = self.lookup(points)?;
            return list(attribute(&points, &points_def, "CoordList"))
                .iter()
                .map(|pair| {
                    let pair = list(Some(pair));
                    let value = |index: usize| pair.get(index).and_then(number);
                    Some(DVec2::new(value(0)?, value(1)?) * self.units.length)
                })
                .collect();
        }
        self.warn("unsupported-curve", format!("#{id} {}", def.name()));
        None
    }

    fn profile(&mut self, id: u32) -> Option<Profile> {
        let (entity, def) = self.lookup(id)?;
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
        } else if def.name() == "IfcRectangleProfileDef" || def.is_a("IfcRoundedRectangleProfileDef")
        {
            if def.is_a("IfcRoundedRectangleProfileDef") {
                self.warn("rounding-ignored", format!("#{id} corners are left square"));
            }
            let size = |name: &str| {
                Some(attribute(&entity, &def, name).and_then(number)? * self.units.length / 2.0)
            };
            let (x, y) = (size("XDim")?, size("YDim")?);
            let outer = vec![
                DVec2::new(-x, -y),
                DVec2::new(x, -y),
                DVec2::new(x, y),
                DVec2::new(-x, y),
            ];
            (self.placed_profile(&entity, &def, outer), Vec::new())
        } else if def.name() == "IfcCircleProfileDef" {
            let radius = attribute(&entity, &def, "Radius").and_then(number)? * self.units.length;
            let outer = (0..CIRCLE_SEGMENTS)
                .map(|i| {
                    let angle = i as f64 / CIRCLE_SEGMENTS as f64 * std::f64::consts::TAU;
                    DVec2::new(angle.cos(), angle.sin()) * radius
                })
                .collect();
            (self.placed_profile(&entity, &def, outer), Vec::new())
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
        let Profile { mut outer, mut holes } = self.profile(profile)?;
        let depth = attribute(entity, def, "Depth").and_then(number)? * self.units.length;
        let direction = attribute(entity, def, "ExtrudedDirection")
            .and_then(reference)
            .and_then(|id| self.direction(id))?
            .try_normalize()?;
        let position = attribute(entity, def, "Position")
            .and_then(reference)
            .map_or(DMat4::IDENTITY, |id| self.placement_matrix(id));

        if ring_area(&outer) < 0.0 {
            outer.reverse();
        }
        for hole in &mut holes {
            if ring_area(hole) > 0.0 {
                hole.reverse();
            }
        }

        let mut rings: Vec<&[DVec2]> = vec![&outer];
        rings.extend(holes.iter().map(Vec::as_slice));
        let caps = triangulate_rings(&rings);

        let count = rings.iter().map(|ring| ring.len()).sum::<usize>() as u32;
        let mut positions = Vec::with_capacity(count as usize * 2);
        for offset in [DVec3::ZERO, direction * depth] {
            for ring in &rings {
                positions.extend(ring.iter().map(|point| point.extend(0.0) + offset));
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
        if direction.z * depth < 0.0 {
            for triangle in &mut triangles {
                triangle.swap(1, 2);
            }
        }
        Some(transformed(
            Mesh {
                positions,
                triangles,
            },
            position,
        ))
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
        let mut outer: Option<Vec<u32>> = None;
        let mut holes: Vec<Vec<u32>> = Vec::new();
        for bound in list(attribute(&entity, &def, "Bounds")) {
            let Some((bound, bound_def)) = reference(bound).and_then(|id| self.lookup(id)) else {
                continue;
            };
            let forward = !matches!(
                attribute(&bound, &bound_def, "Orientation"),
                Some(Value::Enum(flag)) if flag == "F"
            );
            let Some((poly, poly_def)) = attribute(&bound, &bound_def, "Bound")
                .and_then(reference)
                .and_then(|id| self.lookup(id))
            else {
                continue;
            };
            if !poly_def.is_a("IfcPolyLoop") {
                self.warn("unsupported-face-bound", poly_def.name().to_string());
                continue;
            }
            let mut ring: Vec<u32> = list(attribute(&poly, &poly_def, "Polygon"))
                .iter()
                .filter_map(reference)
                .filter_map(|point| {
                    let position = self.point(point)?;
                    Some(builder.vertex(position))
                })
                .collect();
            if !forward {
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
        if let Some(outer) = outer {
            builder.face(&outer, &holes);
        }
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
