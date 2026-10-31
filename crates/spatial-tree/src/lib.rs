//! IFC spatial hierarchy, property sets, materials, units and georeferencing.
//!
//! Plain data for `wasm-api`, shaped after `src/domain/kernel/KernelPort.ts`. See
//! `docs/05_kernel_contract.md`. Started as DeepSeek's T-022, implemented by Claude.

use ifc_schema::units::{ProjectUnits, named_unit, project_units};
use ifc_schema::{EntityDef, Schema, SchemaId, schema};
use std::collections::{HashMap, HashSet};
use step_parser::{Entity, StepIndex, Value};

/// Bounds spatial nesting, which a hostile file could make arbitrarily deep.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoordinateReference {
    pub crs_id: Option<String>,
    /// `"ifc-map-conversion"`, `"ifc-site"` or `"unknown"`.
    pub source: &'static str,
    /// Column-major, model metres to map metres.
    pub local_to_map: Option<[f64; 16]>,
    pub vertical_datum: Option<String>,
    /// `"verified"` or `"unknown"`. Only an exact CRS code counts as verified.
    pub confidence: &'static str,
    /// Decimal degrees.
    pub lat_lng: Option<[f64; 2]>,
    pub crs_name: Option<String>,
    pub map_projection: Option<String>,
    pub map_zone: Option<String>,
    pub map_unit: Option<String>,
    pub geodetic_datum: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelSummary {
    pub schema: &'static str,
    pub schema_identifier: String,
    pub entity_count: usize,
    pub element_count: usize,
    pub length_unit_scale: f64,
    pub length_declared: bool,
    pub coordinate_reference: CoordinateReference,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialElement {
    pub express_id: u32,
    pub type_name: String,
    pub name: Option<String>,
    /// `true` for `IfcRelReferencedInSpatialStructure`.
    pub referenced: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialNode {
    pub express_id: u32,
    pub type_name: String,
    pub name: Option<String>,
    pub long_name: Option<String>,
    /// Metres, for storeys.
    pub elevation: Option<f64>,
    pub children: Vec<SpatialNode>,
    pub elements: Vec<SpatialElement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialTree {
    pub root: SpatialNode,
    pub unassigned: Vec<SpatialElement>,
}

/// The JSON-safe encoding of one attribute, mirroring `AttributeValue`.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    Null,
    Number(f64),
    Text(String),
    Ref(u32),
    Enum(String),
    Typed { type_name: String, value: Box<AttributeValue> },
    List(Vec<AttributeValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Single(AttributeValue),
    Enumerated(Vec<AttributeValue>),
    List(Vec<AttributeValue>),
    Bounded {
        lower: Option<AttributeValue>,
        upper: Option<AttributeValue>,
        set_point: Option<AttributeValue>,
    },
    Table {
        defining: Vec<AttributeValue>,
        defined: Vec<AttributeValue>,
    },
    Complex(Vec<PropertyItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyItem {
    pub name: String,
    pub value: PropertyValue,
    /// IFC type of the value, such as `IFCLABEL`.
    pub value_type: Option<String>,
    pub is_quantity: bool,
    pub unit: Option<String>,
    pub method_of_measurement: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertySet {
    pub name: String,
    pub global_id: Option<String>,
    pub properties: Vec<PropertyItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialLayer {
    pub name: Option<String>,
    pub material: Option<String>,
    /// Metres.
    pub thickness: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialProfile {
    pub name: Option<String>,
    pub material: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialConstituent {
    pub name: Option<String>,
    pub material: Option<String>,
    pub fraction: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    /// `"material"`, `"layer-set"`, `"profile-set"`, `"constituent-set"` or `"list"`.
    pub kind: &'static str,
    pub name: Option<String>,
    pub layers: Vec<MaterialLayer>,
    pub profiles: Vec<MaterialProfile>,
    pub constituents: Vec<MaterialConstituent>,
    /// `false` when the assignment comes from the element's type.
    pub from_instance: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeObjectRef {
    pub express_id: u32,
    pub type_name: String,
    pub name: Option<String>,
    pub global_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementDetails {
    pub express_id: u32,
    pub global_id: Option<String>,
    pub type_name: String,
    pub name: Option<String>,
    /// In STEP parameter order, keyed by EXPRESS name.
    pub attributes: Vec<(String, AttributeValue)>,
    pub property_sets: Vec<PropertySet>,
    pub type_object: Option<TypeObjectRef>,
    pub materials: Vec<MaterialInfo>,
}

#[derive(Default)]
struct Relations {
    /// Whole to parts, from `IfcRelAggregates`.
    parts: HashMap<u32, Vec<u32>>,
    contained: HashMap<u32, Vec<u32>>,
    referenced: HashMap<u32, Vec<u32>>,
    property_sets: HashMap<u32, Vec<u32>>,
    types: HashMap<u32, u32>,
    materials: HashMap<u32, u32>,
}

pub struct ModelData<'a> {
    file: &'a StepIndex<'a>,
    schema_id: SchemaId,
    schema: &'static Schema,
    units: ProjectUnits,
    unit_labels: HashMap<String, String>,
    relations: Relations,
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) | Value::Enum(text) => Some(text.clone()),
        Value::Typed { value, .. } => text(Some(value)),
        _ => None,
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Real(number) => Some(*number),
        Value::Integer(number) => Some(*number as f64),
        Value::Typed { value, .. } => number(Some(value)),
        _ => None,
    }
}

fn reference(value: Option<&Value>) -> Option<u32> {
    match value? {
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

fn refs(value: Option<&Value>) -> Vec<u32> {
    match value {
        Some(Value::List(items)) => items.iter().filter_map(|item| reference(Some(item))).collect(),
        Some(Value::Ref(id)) => vec![*id],
        _ => Vec::new(),
    }
}

fn attribute<'e>(entity: &'e Entity, def: &EntityDef, name: &str) -> Option<&'e Value> {
    entity
        .params
        .get(def.attribute_index(name)?)
        .filter(|value| !matches!(value, Value::Null | Value::Derived))
}

fn encode(value: &Value) -> AttributeValue {
    match value {
        Value::Null | Value::Derived | Value::Binary(_) => AttributeValue::Null,
        Value::Integer(number) => AttributeValue::Number(*number as f64),
        Value::Real(number) => AttributeValue::Number(*number),
        Value::String(text) => AttributeValue::Text(text.clone()),
        Value::Enum(text) => AttributeValue::Enum(text.clone()),
        Value::Ref(id) => AttributeValue::Ref(*id),
        Value::Typed { name, value } => AttributeValue::Typed {
            type_name: name.clone(),
            value: Box::new(encode(value)),
        },
        Value::List(items) => AttributeValue::List(items.iter().map(encode).collect()),
    }
}

fn si_label(prefix: Option<&str>, name: &str) -> String {
    let prefix = match prefix {
        Some("MILLI") => "m",
        Some("CENTI") => "c",
        Some("DECI") => "d",
        Some("KILO") => "k",
        Some("MICRO") => "µ",
        Some(other) => other,
        None => "",
    };
    let base = match name {
        "METRE" => "m",
        "SQUARE_METRE" => "m²",
        "CUBIC_METRE" => "m³",
        "GRAM" => "g",
        "SECOND" => "s",
        "RADIAN" => "rad",
        "NEWTON" => "N",
        "PASCAL" => "Pa",
        "JOULE" => "J",
        "WATT" => "W",
        "KELVIN" => "K",
        "DEGREE_CELSIUS" => "°C",
        other => other,
    };
    format!("{prefix}{base}")
}

/// Which `UnitsInContext` entry a measure type uses.
fn measure_unit_type(measure: &str) -> Option<&'static str> {
    match measure {
        "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCNONNEGATIVELENGTHMEASURE" => {
            Some("LENGTHUNIT")
        }
        "IFCAREAMEASURE" => Some("AREAUNIT"),
        "IFCVOLUMEMEASURE" => Some("VOLUMEUNIT"),
        "IFCMASSMEASURE" => Some("MASSUNIT"),
        "IFCTIMEMEASURE" => Some("TIMEUNIT"),
        "IFCPLANEANGLEMEASURE" | "IFCPOSITIVEPLANEANGLEMEASURE" => Some("PLANEANGLEUNIT"),
        _ => None,
    }
}

/// Decimal degrees from IFC's `[degrees, minutes, seconds, millionths]`.
fn compound_angle(value: Option<&Value>) -> Option<f64> {
    let parts = list(value);
    let degrees = number(parts.first())?;
    let minutes = number(parts.get(1)).unwrap_or(0.0);
    let seconds = number(parts.get(2)).unwrap_or(0.0);
    let millionths = number(parts.get(3)).unwrap_or(0.0);
    // IFC requires one sign throughout, but files also write (-8, 32, 40, 0).
    let negative = [degrees, minutes, seconds, millionths].iter().any(|part| *part < 0.0);
    let magnitude = degrees.abs()
        + minutes.abs() / 60.0
        + (seconds.abs() + millionths.abs() * 1e-6) / 3600.0;
    Some(if negative { -magnitude } else { magnitude })
}

/// `EPSG:25829` style codes are the only names that identify a CRS exactly.
fn exact_crs_code(name: &str) -> Option<String> {
    let (authority, code) = name.trim().split_once(':')?;
    (authority.eq_ignore_ascii_case("EPSG")
        && !code.is_empty()
        && code.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| format!("EPSG:{code}"))
}

impl<'a> ModelData<'a> {
    /// `None` when the file's schema is not supported.
    pub fn open(file: &'a StepIndex<'a>) -> Option<Self> {
        let schema_id = SchemaId::from_identifiers(file.schema_identifiers())?;
        let tables = schema(schema_id);
        let mut data = Self {
            file,
            schema_id,
            schema: tables,
            units: project_units(file, tables),
            unit_labels: HashMap::new(),
            relations: Relations::default(),
        };
        data.index_relations();
        data.index_unit_labels();
        Some(data)
    }

    fn lookup(&self, id: u32) -> Option<(Entity, EntityDef)> {
        let entity = self.file.decode(id).ok()?;
        let def = self.schema.entity(&entity.type_name)?;
        Some((entity, def))
    }

    fn each(&self, type_name: &str, mut visit: impl FnMut(&Entity, &EntityDef)) {
        for id in self.file.ids_by_type(type_name) {
            if let Some((entity, def)) = self.lookup(id) {
                visit(&entity, &def);
            }
        }
    }

    fn index_relations(&mut self) {
        let mut relations = Relations::default();
        self.each("IFCRELAGGREGATES", |entity, def| {
            if let Some(whole) = reference(attribute(entity, def, "RelatingObject")) {
                relations
                    .parts
                    .entry(whole)
                    .or_default()
                    .extend(refs(attribute(entity, def, "RelatedObjects")));
            }
        });
        self.each("IFCRELCONTAINEDINSPATIALSTRUCTURE", |entity, def| {
            if let Some(space) = reference(attribute(entity, def, "RelatingStructure")) {
                relations
                    .contained
                    .entry(space)
                    .or_default()
                    .extend(refs(attribute(entity, def, "RelatedElements")));
            }
        });
        self.each("IFCRELREFERENCEDINSPATIALSTRUCTURE", |entity, def| {
            if let Some(space) = reference(attribute(entity, def, "RelatingStructure")) {
                relations
                    .referenced
                    .entry(space)
                    .or_default()
                    .extend(refs(attribute(entity, def, "RelatedElements")));
            }
        });
        self.each("IFCRELDEFINESBYPROPERTIES", |entity, def| {
            let sets = refs(attribute(entity, def, "RelatingPropertyDefinition"));
            for object in refs(attribute(entity, def, "RelatedObjects")) {
                relations
                    .property_sets
                    .entry(object)
                    .or_default()
                    .extend(sets.iter().copied());
            }
        });
        self.each("IFCRELDEFINESBYTYPE", |entity, def| {
            if let Some(type_object) = reference(attribute(entity, def, "RelatingType")) {
                for object in refs(attribute(entity, def, "RelatedObjects")) {
                    relations.types.insert(object, type_object);
                }
            }
        });
        self.each("IFCRELASSOCIATESMATERIAL", |entity, def| {
            if let Some(material) = reference(attribute(entity, def, "RelatingMaterial")) {
                for object in refs(attribute(entity, def, "RelatedObjects")) {
                    relations.materials.insert(object, material);
                }
            }
        });
        self.relations = relations;
    }

    fn index_unit_labels(&mut self) {
        let Some(project) = self.file.ids_by_type("IFCPROJECT").next() else {
            return;
        };
        let Some((project, def)) = self.lookup(project) else { return };
        let Some((assignment, assignment_def)) =
            reference(attribute(&project, &def, "UnitsInContext")).and_then(|id| self.lookup(id))
        else {
            return;
        };
        let mut labels = HashMap::new();
        for unit in refs(attribute(&assignment, &assignment_def, "Units")) {
            if let Some((unit_type, label)) = self.unit_label(unit) {
                labels.insert(unit_type, label);
            }
        }
        self.unit_labels = labels;
    }

    fn unit_label(&self, id: u32) -> Option<(String, String)> {
        let (entity, def) = self.lookup(id)?;
        let unit_type = text(attribute(&entity, &def, "UnitType"))?;
        let name = text(attribute(&entity, &def, "Name"))?;
        let label = if def.is_a("IfcSIUnit") {
            si_label(text(attribute(&entity, &def, "Prefix")).as_deref(), &name)
        } else {
            name
        };
        Some((unit_type, label))
    }

    /// The elements shown in the viewer, as in `elementCount` of the contract.
    fn product_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for type_name in self.schema.subtypes_of("IfcProduct") {
            for id in self.file.ids_by_type(&type_name.to_ascii_uppercase()) {
                let Some((entity, def)) = self.lookup(id) else { continue };
                if !def.is_a("IfcFeatureElementSubtraction")
                    && reference(attribute(&entity, &def, "Representation")).is_some()
                {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        ids
    }

    pub fn summary(&self) -> ModelSummary {
        let mut diagnostics = Vec::new();
        if !self.units.length_declared {
            diagnostics.push(Diagnostic {
                code: "length-unit-assumed",
                message: "the file declares no length unit; metres assumed".to_string(),
            });
        }
        let coordinate_reference = self.coordinate_reference(&mut diagnostics);
        ModelSummary {
            schema: self.schema_id.canonical_name(),
            schema_identifier: self.file.schema_identifiers().join(", "),
            entity_count: self.file.entity_count(),
            element_count: self.product_ids().len(),
            length_unit_scale: self.units.length,
            length_declared: self.units.length_declared,
            coordinate_reference,
            diagnostics,
        }
    }

    fn site_lat_lng(&self) -> Option<[f64; 2]> {
        let site = self.file.ids_by_type("IFCSITE").next()?;
        let (entity, def) = self.lookup(site)?;
        Some([
            compound_angle(attribute(&entity, &def, "RefLatitude"))?,
            compound_angle(attribute(&entity, &def, "RefLongitude"))?,
        ])
    }

    fn coordinate_reference(&self, diagnostics: &mut Vec<Diagnostic>) -> CoordinateReference {
        let lat_lng = self.site_lat_lng();
        let mut reference_data = CoordinateReference {
            crs_id: None,
            source: if lat_lng.is_some() { "ifc-site" } else { "unknown" },
            local_to_map: None,
            vertical_datum: None,
            confidence: "unknown",
            lat_lng,
            crs_name: None,
            map_projection: None,
            map_zone: None,
            map_unit: None,
            geodetic_datum: None,
        };

        let conversion = self
            .file
            .ids_by_type("IFCMAPCONVERSION")
            .next()
            .or_else(|| self.file.ids_by_type("IFCMAPCONVERSIONSCALED").next());
        let Some((conversion, conversion_def)) = conversion.and_then(|id| self.lookup(id)) else {
            for type_name in ["IFCRIGIDOPERATION", "IFCCOORDINATEOPERATION"] {
                if self.file.ids_by_type(type_name).next().is_some() {
                    diagnostics.push(Diagnostic {
                        code: "unsupported-coordinate-operation",
                        message: type_name.to_string(),
                    });
                }
            }
            return reference_data;
        };

        let target = reference(attribute(&conversion, &conversion_def, "TargetCRS"));
        let crs = target.and_then(|id| self.lookup(id));
        // Metres per map unit, falling back to the project's length unit.
        let mut map_scale = self.units.length;
        if let Some((crs, crs_def)) = &crs {
            let name = text(attribute(crs, crs_def, "Name"));
            reference_data.crs_id = name.as_deref().and_then(exact_crs_code);
            reference_data.confidence = if reference_data.crs_id.is_some() {
                "verified"
            } else {
                "unknown"
            };
            reference_data.crs_name = name;
            reference_data.vertical_datum = text(attribute(crs, crs_def, "VerticalDatum"));
            reference_data.geodetic_datum = text(attribute(crs, crs_def, "GeodeticDatum"));
            reference_data.map_projection = text(attribute(crs, crs_def, "MapProjection"));
            reference_data.map_zone = text(attribute(crs, crs_def, "MapZone"));
            if let Some(unit) = reference(attribute(crs, crs_def, "MapUnit")) {
                if let Some((_, scale)) = named_unit(self.file, self.schema, unit) {
                    map_scale = scale;
                }
                reference_data.map_unit = self.unit_label(unit).map(|(_, label)| label);
            }
        }

        let abscissa = number(attribute(&conversion, &conversion_def, "XAxisAbscissa"));
        let ordinate = number(attribute(&conversion, &conversion_def, "XAxisOrdinate"));
        let theta = match (abscissa, ordinate) {
            (Some(abscissa), Some(ordinate)) => ordinate.atan2(abscissa),
            _ => 0.0,
        };
        let file_scale = self.units.length;
        let scale = match number(attribute(&conversion, &conversion_def, "Scale")) {
            Some(scale) => scale,
            None => {
                diagnostics.push(Diagnostic {
                    code: "map-scale-assumed",
                    message: "IfcMapConversion.Scale is absent; derived from the units".to_string(),
                });
                file_scale / map_scale
            }
        };
        let factor = |name: &str| {
            number(attribute(&conversion, &conversion_def, name)).unwrap_or(1.0)
        };
        let (factor_x, factor_y, factor_z) = (factor("FactorX"), factor("FactorY"), factor("FactorZ"));
        let offset = |name: &str| {
            number(attribute(&conversion, &conversion_def, name)).unwrap_or(0.0) * map_scale
        };

        // Model metres become file units, then the normative formula, then metres of map.
        let per_model_metre = scale * map_scale / file_scale;
        let (sin, cos) = theta.sin_cos();
        reference_data.local_to_map = Some([
            per_model_metre * factor_x * cos,
            per_model_metre * factor_x * sin,
            0.0,
            0.0,
            -per_model_metre * factor_y * sin,
            per_model_metre * factor_y * cos,
            0.0,
            0.0,
            0.0,
            0.0,
            per_model_metre * factor_z,
            0.0,
            offset("Eastings"),
            offset("Northings"),
            offset("OrthogonalHeight"),
            1.0,
        ]);
        reference_data.source = "ifc-map-conversion";
        reference_data
    }

    fn element(&self, id: u32, referenced: bool) -> Option<SpatialElement> {
        let (entity, def) = self.lookup(id)?;
        Some(SpatialElement {
            express_id: id,
            type_name: def.name().to_string(),
            name: text(attribute(&entity, &def, "Name")),
            referenced,
        })
    }

    /// Adds an element and the parts it aggregates, which the viewer shows individually.
    fn collect_elements(
        &self,
        id: u32,
        referenced: bool,
        visited: &mut HashSet<u32>,
        seen: &mut HashSet<u32>,
        out: &mut Vec<SpatialElement>,
    ) {
        // A visited set, as a depth limit alone leaves exponentially many paths in a graph.
        if !visited.insert(id) {
            return;
        }
        let Some((_, def)) = self.lookup(id) else { return };
        if def.is_a("IfcFeatureElementSubtraction") {
            return;
        }
        if !referenced {
            seen.insert(id);
        }
        if let Some(element) = self.element(id, referenced) {
            out.push(element);
        }
        for part in self.relations.parts.get(&id).into_iter().flatten() {
            self.collect_elements(*part, referenced, visited, seen, out);
        }
    }

    fn is_spatial(def: &EntityDef) -> bool {
        def.is_a("IfcSpatialStructureElement") || def.is_a("IfcSpatialElement")
    }

    fn node(
        &self,
        id: u32,
        depth: usize,
        placed: &mut HashSet<u32>,
        seen: &mut HashSet<u32>,
    ) -> Option<SpatialNode> {
        if depth >= MAX_DEPTH || !placed.insert(id) {
            return None;
        }
        let (entity, def) = self.lookup(id)?;

        let mut children = Vec::new();
        let mut elements = Vec::new();
        for part in self.relations.parts.get(&id).into_iter().flatten() {
            match self.lookup(*part) {
                Some((_, part_def)) if Self::is_spatial(&part_def) => {
                    children.extend(self.node(*part, depth + 1, placed, seen));
                }
                Some(_) => {
                    self.collect_elements(*part, false, &mut HashSet::new(), seen, &mut elements);
                }
                None => {}
            }
        }
        for element in self.relations.contained.get(&id).into_iter().flatten() {
            self.collect_elements(*element, false, &mut HashSet::new(), seen, &mut elements);
        }
        for element in self.relations.referenced.get(&id).into_iter().flatten() {
            self.collect_elements(*element, true, &mut HashSet::new(), seen, &mut elements);
        }

        // IFC relationship sets are unordered.
        children.sort_by_key(|child: &SpatialNode| child.express_id);
        elements.sort_by_key(|element| element.express_id);
        elements.dedup_by_key(|element| (element.express_id, element.referenced));

        Some(SpatialNode {
            express_id: id,
            type_name: def.name().to_string(),
            name: text(attribute(&entity, &def, "Name")),
            long_name: text(attribute(&entity, &def, "LongName")),
            elevation: number(attribute(&entity, &def, "Elevation"))
                .map(|elevation| elevation * self.units.length),
            children,
            elements,
        })
    }

    pub fn spatial_tree(&self) -> Option<SpatialTree> {
        let project = self.file.ids_by_type("IFCPROJECT").next()?;
        let mut seen = HashSet::new();
        let root = self.node(project, 0, &mut HashSet::new(), &mut seen)?;
        let mut spatial = HashSet::new();
        collect_spatial_ids(&root, &mut spatial);
        let unassigned = self
            .product_ids()
            .into_iter()
            .filter(|id| !seen.contains(id) && !spatial.contains(id))
            .filter_map(|id| self.element(id, false))
            .collect();
        Some(SpatialTree { root, unassigned })
    }

    fn property(&self, id: u32, visited: &mut HashSet<u32>) -> Option<PropertyItem> {
        if !visited.insert(id) {
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        let name = text(attribute(&entity, &def, "Name"))?;
        let explicit_unit = reference(attribute(&entity, &def, "Unit"))
            .and_then(|unit| self.unit_label(unit))
            .map(|(_, label)| label);

        if def.is_a("IfcPhysicalSimpleQuantity") {
            let (field, unit_type) = match def.name() {
                "IfcQuantityLength" => ("LengthValue", Some("LENGTHUNIT")),
                "IfcQuantityArea" => ("AreaValue", Some("AREAUNIT")),
                "IfcQuantityVolume" => ("VolumeValue", Some("VOLUMEUNIT")),
                "IfcQuantityWeight" => ("WeightValue", Some("MASSUNIT")),
                "IfcQuantityTime" => ("TimeValue", Some("TIMEUNIT")),
                "IfcQuantityCount" => ("CountValue", None),
                _ => return None,
            };
            let value = attribute(&entity, &def, field).map_or(AttributeValue::Null, encode);
            return Some(PropertyItem {
                name,
                value: PropertyValue::Single(value),
                value_type: Some(def.name().to_string()),
                is_quantity: true,
                unit: explicit_unit.or_else(|| {
                    unit_type.and_then(|unit_type| self.unit_labels.get(unit_type).cloned())
                }),
                method_of_measurement: None,
            });
        }

        let encoded = |attribute_name: &str| {
            list(attribute(&entity, &def, attribute_name))
                .iter()
                .map(encode)
                .collect::<Vec<_>>()
        };
        let optional = |attribute_name: &str| attribute(&entity, &def, attribute_name).map(encode);
        let mut value_type = None;
        let value = if def.is_a("IfcPropertySingleValue") {
            let nominal = attribute(&entity, &def, "NominalValue");
            if let Some(Value::Typed { name, .. }) = nominal {
                value_type = Some(name.clone());
            }
            PropertyValue::Single(nominal.map_or(AttributeValue::Null, encode))
        } else if def.is_a("IfcPropertyEnumeratedValue") {
            PropertyValue::Enumerated(encoded("EnumerationValues"))
        } else if def.is_a("IfcPropertyListValue") {
            PropertyValue::List(encoded("ListValues"))
        } else if def.is_a("IfcPropertyBoundedValue") {
            PropertyValue::Bounded {
                lower: optional("LowerBoundValue"),
                upper: optional("UpperBoundValue"),
                set_point: optional("SetPointValue"),
            }
        } else if def.is_a("IfcPropertyTableValue") {
            PropertyValue::Table {
                defining: encoded("DefiningValues"),
                defined: encoded("DefinedValues"),
            }
        } else if def.is_a("IfcComplexProperty") {
            PropertyValue::Complex(
                refs(attribute(&entity, &def, "HasProperties"))
                    .into_iter()
                    .filter_map(|id| self.property(id, visited))
                    .collect(),
            )
        } else {
            return None;
        };

        let unit = explicit_unit.or_else(|| {
            let measure = measure_unit_type(value_type.as_deref()?)?;
            self.unit_labels.get(measure).cloned()
        });
        Some(PropertyItem {
            name,
            value,
            value_type,
            is_quantity: false,
            unit,
            method_of_measurement: None,
        })
    }

    fn property_set(&self, id: u32) -> Option<PropertySet> {
        let (entity, def) = self.lookup(id)?;
        let field = if def.is_a("IfcElementQuantity") {
            "Quantities"
        } else if def.is_a("IfcPropertySet") {
            "HasProperties"
        } else {
            return None;
        };
        let method = text(attribute(&entity, &def, "MethodOfMeasurement"));
        let properties = refs(attribute(&entity, &def, field))
            .into_iter()
            .filter_map(|id| self.property(id, &mut HashSet::new()))
            .map(|mut item| {
                item.method_of_measurement = method.clone();
                item
            })
            .collect();
        Some(PropertySet {
            name: text(attribute(&entity, &def, "Name")).unwrap_or_default(),
            global_id: text(attribute(&entity, &def, "GlobalId")),
            properties,
        })
    }

    fn material_name(&self, id: Option<u32>) -> Option<String> {
        let (entity, def) = self.lookup(id?)?;
        text(attribute(&entity, &def, "Name"))
    }

    fn material(&self, id: u32, from_instance: bool, depth: usize) -> Option<MaterialInfo> {
        if depth >= 4 {
            return None;
        }
        let (entity, def) = self.lookup(id)?;
        let mut info = MaterialInfo {
            kind: "material",
            name: None,
            layers: Vec::new(),
            profiles: Vec::new(),
            constituents: Vec::new(),
            from_instance,
        };
        match def.name() {
            "IfcMaterial" => info.name = text(attribute(&entity, &def, "Name")),
            "IfcMaterialLayerSetUsage" => {
                let set = reference(attribute(&entity, &def, "ForLayerSet"))?;
                return self.material(set, from_instance, depth + 1);
            }
            "IfcMaterialProfileSetUsage" | "IfcMaterialProfileSetUsageTapering" => {
                let set = reference(attribute(&entity, &def, "ForProfileSet"))?;
                return self.material(set, from_instance, depth + 1);
            }
            "IfcMaterialLayerSet" => {
                info.kind = "layer-set";
                info.name = text(attribute(&entity, &def, "LayerSetName"));
                for layer in refs(attribute(&entity, &def, "MaterialLayers")) {
                    let Some((layer, layer_def)) = self.lookup(layer) else { continue };
                    info.layers.push(MaterialLayer {
                        name: text(attribute(&layer, &layer_def, "Name")),
                        material: self
                            .material_name(reference(attribute(&layer, &layer_def, "Material"))),
                        thickness: number(attribute(&layer, &layer_def, "LayerThickness"))
                            .map(|thickness| thickness * self.units.length),
                    });
                }
            }
            "IfcMaterialProfileSet" => {
                info.kind = "profile-set";
                info.name = text(attribute(&entity, &def, "Name"));
                for profile in refs(attribute(&entity, &def, "MaterialProfiles")) {
                    let Some((profile, profile_def)) = self.lookup(profile) else { continue };
                    info.profiles.push(MaterialProfile {
                        name: text(attribute(&profile, &profile_def, "Name")),
                        material: self
                            .material_name(reference(attribute(&profile, &profile_def, "Material"))),
                    });
                }
            }
            "IfcMaterialConstituentSet" => {
                info.kind = "constituent-set";
                info.name = text(attribute(&entity, &def, "Name"));
                for part in refs(attribute(&entity, &def, "MaterialConstituents")) {
                    let Some((part, part_def)) = self.lookup(part) else { continue };
                    info.constituents.push(MaterialConstituent {
                        name: text(attribute(&part, &part_def, "Name")),
                        material: self.material_name(reference(attribute(&part, &part_def, "Material"))),
                        fraction: number(attribute(&part, &part_def, "Fraction")),
                    });
                }
            }
            "IfcMaterialList" => {
                info.kind = "list";
                info.layers = refs(attribute(&entity, &def, "Materials"))
                    .into_iter()
                    .map(|material| MaterialLayer {
                        name: None,
                        material: self.material_name(Some(material)),
                        thickness: None,
                    })
                    .collect();
            }
            _ => return None,
        }
        Some(info)
    }

    pub fn element_details(&self, id: u32) -> Option<ElementDetails> {
        let (entity, def) = self.lookup(id)?;
        let attributes = def
            .attributes()
            .iter()
            .zip(&entity.params)
            .filter(|(_, value)| !matches!(value, Value::Null | Value::Derived))
            .map(|(definition, value)| (definition.name.to_string(), encode(value)))
            .collect();

        let type_id = self.relations.types.get(&id).copied();
        let mut type_set_ids = Vec::new();
        let type_object = type_id.and_then(|type_id| {
            let (type_entity, type_def) = self.lookup(type_id)?;
            type_set_ids = refs(attribute(&type_entity, &type_def, "HasPropertySets"));
            Some(TypeObjectRef {
                express_id: type_id,
                type_name: type_def.name().to_string(),
                name: text(attribute(&type_entity, &type_def, "Name")),
                global_id: text(attribute(&type_entity, &type_def, "GlobalId")),
            })
        });
        let mut own_ids = self.relations.property_sets.get(&id).cloned().unwrap_or_default();
        own_ids.sort_unstable();
        own_ids.dedup();
        type_set_ids.sort_unstable();
        type_set_ids.dedup();

        // A set on the element overrides the type's set of the same name.
        let mut property_sets: Vec<PropertySet> = own_ids
            .into_iter()
            .filter_map(|set| self.property_set(set))
            .collect();
        for set in type_set_ids.into_iter().filter_map(|set| self.property_set(set)) {
            if !property_sets.iter().any(|own| own.name == set.name) {
                property_sets.push(set);
            }
        }

        let material = match (self.relations.materials.get(&id), type_id) {
            (Some(material), _) => self.material(*material, true, 0),
            (None, Some(type_id)) => self
                .relations
                .materials
                .get(&type_id)
                .and_then(|material| self.material(*material, false, 0)),
            (None, None) => None,
        };

        Some(ElementDetails {
            express_id: id,
            global_id: text(attribute(&entity, &def, "GlobalId")),
            type_name: def.name().to_string(),
            name: text(attribute(&entity, &def, "Name")),
            attributes,
            property_sets,
            type_object,
            materials: material.into_iter().collect(),
        })
    }
}

fn collect_spatial_ids(node: &SpatialNode, out: &mut HashSet<u32>) {
    out.insert(node.express_id);
    for child in &node.children {
        collect_spatial_ids(child, out);
    }
}
