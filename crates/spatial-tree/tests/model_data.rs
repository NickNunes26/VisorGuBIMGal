//! Spatial data checked against facts read from the fixtures by hand.

use spatial_tree::{AttributeValue, ModelData, PropertyValue, SpatialNode};
use std::fs;
use std::path::{Path, PathBuf};
use step_parser::index;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> Vec<u8> {
    fs::read(root().join(path)).unwrap()
}

fn transform(matrix: &[f64; 16], point: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = point;
    [
        matrix[0] * x + matrix[4] * y + matrix[8] * z + matrix[12],
        matrix[1] * x + matrix[5] * y + matrix[9] * z + matrix[13],
        matrix[2] * x + matrix[6] * y + matrix[10] * z + matrix[14],
    ]
}

#[test]
fn simple_wall_has_the_expected_hierarchy() {
    let bytes = read("public/samples/SimpleWall.ifc");
    let file = index(&bytes).unwrap();
    let data = ModelData::open(&file).unwrap();
    let tree = data.spatial_tree().unwrap();

    assert_eq!(tree.root.express_id, 121);
    assert_eq!(tree.root.type_name, "IfcProject");
    let site = &tree.root.children[0];
    assert_eq!((site.express_id, site.type_name.as_str()), (150, "IfcSite"));
    let building = &site.children[0];
    assert_eq!(building.express_id, 131);
    let storey = &building.children[0];
    assert_eq!(storey.express_id, 140);
    assert_eq!(storey.name.as_deref(), Some("Level 1"));
    assert_eq!(storey.elevation, Some(0.0));
    let elements: Vec<u32> = storey.elements.iter().map(|element| element.express_id).collect();
    assert_eq!(elements, [219, 572], "ascending, without the opening");
    assert!(tree.unassigned.is_empty());
}

#[test]
fn simple_wall_summary() {
    let bytes = read("public/samples/SimpleWall.ifc");
    let file = index(&bytes).unwrap();
    let summary = ModelData::open(&file).unwrap().summary();
    assert_eq!(summary.schema, "IFC2X3");
    assert_eq!(summary.entity_count, 548);
    assert_eq!(summary.element_count, 2);
    assert_eq!(summary.length_unit_scale, 0.001);
    assert!(summary.length_declared);
    let site = &summary.coordinate_reference;
    assert_eq!(site.source, "ifc-site");
    assert_eq!(site.confidence, "unknown");
    let [lat, lng] = site.lat_lng.unwrap();
    assert!((lat - (42.0 + 21.0 / 60.0 + 31.181945 / 3600.0)).abs() < 1e-9);
    assert!((lng - -(71.0 + 3.0 / 60.0 + 24.263305 / 3600.0)).abs() < 1e-9);
}

#[test]
fn wall_details_carry_names_properties_quantities_and_materials() {
    let bytes = read("public/samples/SimpleWall.ifc");
    let file = index(&bytes).unwrap();
    let data = ModelData::open(&file).unwrap();
    let wall = data.element_details(219).unwrap();

    assert_eq!(wall.type_name, "IfcWallStandardCase");
    assert_eq!(wall.name.as_deref(), Some("Basic Wall:Bearing Wall:346660"));
    assert_eq!(wall.global_id.as_deref(), Some("1F6umJ5H50aeL3A1As_wTm"));
    let tag = wall.attributes.iter().find(|(name, _)| name == "Tag").unwrap();
    assert_eq!(tag.1, AttributeValue::Text("346660".to_string()));

    let mut names: Vec<&str> = wall.property_sets.iter().map(|set| set.name.as_str()).collect();
    println!("{} sets: {names:?}", names.len());
    assert!(names.len() >= 14, "the wall's own 14 sets are all kept");
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    // A type's set never repeats a name; only the element's own sets may.
    assert!(total - names.len() <= 1, "{names:?}");
    let quantities = wall
        .property_sets
        .iter()
        .find(|set| set.properties.iter().any(|item| item.is_quantity))
        .unwrap();
    let height = quantities.properties.iter().find(|item| item.name == "Height").unwrap();
    assert_eq!(height.value, PropertyValue::Single(AttributeValue::Number(4000.0)));
    assert_eq!(height.unit.as_deref(), Some("mm"), "raw value in the project unit");

    let common = wall.property_sets.iter().find(|set| set.name == "Pset_WallCommon").unwrap();
    assert!(!common.properties.is_empty());

    let [material] = wall.materials.as_slice() else {
        panic!("{:?}", wall.materials)
    };
    assert_eq!(material.kind, "layer-set");
    assert!(material.from_instance);
    assert_eq!(material.layers[0].material.as_deref(), Some("Default Wall"));
    assert_eq!(material.layers[0].thickness, Some(0.2));
    assert!(wall.type_object.is_some());
}

#[test]
fn galicia_fixture_georeferences_to_utm_29n() {
    let bytes = read("test/fixtures/georef-galicia-utm29n.ifc");
    let file = index(&bytes).unwrap();
    let reference = ModelData::open(&file).unwrap().summary().coordinate_reference;

    assert_eq!(reference.source, "ifc-map-conversion");
    assert_eq!(reference.crs_id.as_deref(), Some("EPSG:25829"));
    assert_eq!(reference.confidence, "verified");
    assert_eq!(reference.map_zone.as_deref(), Some("29N"));

    let matrix = reference.local_to_map.unwrap();
    let origin = transform(&matrix, [0.0, 0.0, 0.0]);
    assert_eq!(origin, [536000.0, 4747000.0, 260.0]);
    let [lat, lng] = reference.lat_lng.unwrap();
    assert!((lat - (42.0 + 52.0 / 60.0 + 50.0 / 3600.0)).abs() < 1e-9);
    // The file writes (-8, 32, 40, 0), and Santiago is at 8.54 degrees west, not 7.46.
    assert!((lng - -(8.0 + 32.0 / 60.0 + 40.0 / 3600.0)).abs() < 1e-9, "{lng}");
}

#[test]
fn rotation_and_absent_scale_follow_the_contract() {
    let bytes = read("test/fixtures/geographic-referencing.ifc");
    let file = index(&bytes).unwrap();
    let summary = ModelData::open(&file).unwrap().summary();
    let reference = &summary.coordinate_reference;
    assert_eq!(reference.crs_id.as_deref(), Some("EPSG:25832"));
    assert!(summary.diagnostics.iter().any(|item| item.code == "map-scale-assumed"));

    let matrix = reference.local_to_map.unwrap();
    assert_eq!(transform(&matrix, [0.0, 0.0, 0.0]), [458658.78, 5438232.79, 113.7]);

    // A metre along model X moves along (XAxisAbscissa, XAxisOrdinate), and stays a metre.
    let moved = transform(&matrix, [1.0, 0.0, 0.0]);
    let (dx, dy) = (moved[0] - 458658.78, moved[1] - 5438232.79);
    let length = (0.270600445976_f64.powi(2) + 0.962691746426_f64.powi(2)).sqrt();
    assert!((dx - 0.270600445976 / length).abs() < 1e-6, "{dx}");
    assert!((dy - 0.962691746426 / length).abs() < 1e-6, "{dy}");
    assert!((dx.hypot(dy) - 1.0).abs() < 1e-6);
}

fn count_elements(node: &SpatialNode) -> usize {
    node.elements.len() + node.children.iter().map(count_elements).sum::<usize>()
}

#[test]
fn every_file_yields_summary_tree_and_details() {
    let mut files: Vec<PathBuf> = Vec::new();
    for folder in ["public/samples", "test/fixtures"] {
        for entry in fs::read_dir(root().join(folder)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "ifc") {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let bytes = fs::read(&path).unwrap();
        let file = index(&bytes).unwrap();
        let data = ModelData::open(&file).unwrap();
        let summary = data.summary();
        let tree = data.spatial_tree().expect("every fixture has an IfcProject");
        let placed = count_elements(&tree.root);
        for id in file.ids() {
            let _ = data.element_details(id);
        }
        println!(
            "{:<34} {:<7} {:>4} elements, {:>3} placed, {:>2} unassigned, {} {:?}",
            path.file_name().unwrap().to_string_lossy(),
            summary.schema,
            summary.element_count,
            placed,
            tree.unassigned.len(),
            summary.coordinate_reference.source,
            summary.coordinate_reference.crs_id,
        );
    }
}

/// Forty levels where each entity links twice to the next: 2^40 paths from 80 entities.
#[test]
fn shared_links_do_not_multiply_the_work() {
    let mut body = String::from("#1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n");
    for level in 0..40u32 {
        let (from, a, b) = (2 + level * 2, 4 + level * 2, 5 + level * 2);
        body.push_str(&format!("#{from}=IFCWALL('a',$,$,$,$,$,$,$,$);\n"));
        body.push_str(&format!("#{}=IFCWALL('b',$,$,$,$,$,$,$,$);\n", from + 1));
        body.push_str(&format!("#{}=IFCRELAGGREGATES('x',$,$,$,#{from},(#{a},#{b}));\n", 1000 + level * 2));
        body.push_str(&format!("#{}=IFCRELAGGREGATES('y',$,$,$,#{},(#{a},#{b}));\n", 1001 + level * 2, from + 1));
        body.push_str(&format!(
            "#{}=IFCCOMPLEXPROPERTY('c',$,'u',(#{},#{}));\n",
            3000 + level,
            3001 + level,
            3001 + level
        ));
    }
    body.push_str("#900=IFCRELAGGREGATES('r',$,$,$,#1,(#2));\n");
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n{body}ENDSEC;END-ISO-10303-21;"
    );
    let file = index(text.as_bytes()).unwrap();
    let data = ModelData::open(&file).unwrap();
    assert!(data.spatial_tree().is_some());
    let _ = data.summary();
}

#[test]
fn cyclic_hierarchy_does_not_hang_or_crash() {
    let text = "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n\
        #1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
        #2=IFCSITE('s',$,$,$,$,$,$,$,.ELEMENT.,$,$,$,$,$);\n\
        #3=IFCRELAGGREGATES('a',$,$,$,#1,(#2));\n\
        #4=IFCRELAGGREGATES('b',$,$,$,#2,(#1,#2));\n\
        #5=IFCCOMPLEXPROPERTY('c',$,'u',(#5));\n\
        ENDSEC;END-ISO-10303-21;";
    let file = index(text.as_bytes()).unwrap();
    let data = ModelData::open(&file).unwrap();
    let tree = data.spatial_tree().unwrap();
    assert_eq!(tree.root.express_id, 1);
    let _ = data.element_details(5);
}

/// A wasm stack is far smaller than a native one, so these chains must not recurse with length.
#[test]
fn chains_as_long_as_the_file_do_not_exhaust_the_stack() {
    const LENGTH: u32 = 200_000;
    let mut body = String::from(
        "#1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
         #2=IFCBUILDINGSTOREY('s',$,$,$,$,$,$,$,.ELEMENT.,0.);\n\
         #3=IFCRELAGGREGATES('a',$,$,$,#1,(#2));\n\
         #4=IFCWALL('w',$,$,$,$,$,$,$);\n\
         #5=IFCRELCONTAINEDINSPATIALSTRUCTURE('c',$,$,$,(#4),#2);\n\
         #6=IFCPROPERTYSET('g',$,'Pset',$,(#1000000));\n\
         #7=IFCRELDEFINESBYPROPERTIES('r',$,$,$,(#4),#6);\n",
    );
    for level in 0..LENGTH {
        let (part, relation, property) = (100_000 + level, 400_000 + level, 1_000_000 + level);
        let parent = if level == 0 { 4 } else { part - 1 };
        body.push_str(&format!("#{part}=IFCWALL('w',$,$,$,$,$,$,$);\n"));
        body.push_str(&format!("#{relation}=IFCRELAGGREGATES('a',$,$,$,#{parent},(#{part}));\n"));
        body.push_str(&format!("#{property}=IFCCOMPLEXPROPERTY('c',$,'u',(#{}));\n", property + 1));
    }
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n{body}ENDSEC;END-ISO-10303-21;"
    );
    let file = index(text.as_bytes()).unwrap();
    let data = ModelData::open(&file).unwrap();

    // The stack a wasm build gets.
    let handle = std::thread::Builder::new().stack_size(1024 * 1024);
    std::thread::scope(|scope| {
        handle
            .spawn_scoped(scope, || {
                let tree = data.spatial_tree().unwrap();
                assert_eq!(tree.root.children[0].elements.len(), LENGTH as usize + 1);
                let wall = data.element_details(4).unwrap();
                assert_eq!(wall.property_sets[0].name, "Pset");
            })
            .unwrap()
            .join()
            .unwrap();
    });
}
