//! What crosses the WASM boundary, checked natively over every file we have.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use wasm_api::batches::{Batch, MAX_BATCH_RADIUS};
use wasm_api::loaded::{LoadedModel, OpenError};

const RECORD_WORDS: usize = 7;
const DEGRADED: u32 = 1 << 1;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn all_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for folder in ["public/samples", "test/fixtures"] {
        for entry in fs::read_dir(root().join(folder)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "ifc") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn stream(model: &mut LoadedModel, max_triangles: usize) -> Vec<Batch> {
    model.start_stream();
    std::iter::from_fn(|| model.next_batch(max_triangles)).collect()
}

/// The same rules as `findBatchError` in `GeometryBatch.ts`, plus the radius.
fn check(batch: &Batch) {
    assert_eq!(batch.positions.len() % 3, 0);
    assert_eq!(batch.normals.len(), batch.positions.len());
    assert_eq!(batch.indices.len() % 3, 0);
    assert_eq!(batch.mesh_table.len() % RECORD_WORDS, 0);
    assert!(batch.positions.iter().all(|value| value.is_finite()));

    for normal in batch.normals.chunks_exact(3) {
        let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-3, "{normal:?}");
    }
    let vertex_total = (batch.positions.len() / 3) as u32;
    for record in batch.mesh_table.chunks_exact(RECORD_WORDS) {
        let [express_id, first_vertex, vertex_count, first_index, index_count, _, flags] =
            record.try_into().unwrap();
        assert_ne!(express_id, 0);
        assert!(first_vertex + vertex_count <= vertex_total);
        assert!((first_index + index_count) as usize <= batch.indices.len());
        assert_eq!(index_count % 3, 0);
        let range = first_vertex..first_vertex + vertex_count;
        for index in &batch.indices[first_index as usize..(first_index + index_count) as usize] {
            assert!(range.contains(index), "index {index} leaves {range:?}");
        }
        if flags & DEGRADED == 0 {
            for at in range {
                let start = at as usize * 3;
                let position = &batch.positions[start..start + 3];
                let length = position.iter().map(|value| (*value as f64).powi(2)).sum::<f64>();
                assert!(length.sqrt() <= MAX_BATCH_RADIUS + 1e-3);
            }
        }
    }
}

#[test]
fn every_file_produces_valid_batches() {
    for path in all_files() {
        let bytes = fs::read(&path).unwrap();
        let mut model = LoadedModel::open(bytes).unwrap();
        let batches = stream(&mut model, 2_000);
        batches.iter().for_each(check);
        let processed = model.processed_elements();
        assert_eq!(processed, model.total_elements(), "{}", path.display());
        let complete = model.finish_stream();
        let records: usize = batches.iter().map(|batch| batch.mesh_table.len() / RECORD_WORDS).sum();
        assert!(complete.starts_with(&format!(r#"{{"meshCount":{records}"#)), "{complete}");
        println!("{:<36} {:>3} batches {:>4} records", path.file_name().unwrap().display(), batches.len(), records);
    }
}

#[test]
fn batch_size_does_not_change_what_is_drawn() {
    let bytes = fs::read(root().join("public/samples/ManySimpleWalls.ifc")).unwrap();
    let mut model = LoadedModel::open(bytes).unwrap();

    // Origins differ between the two runs, so vertices may differ by one f32 step.
    let per_element = |batches: &[Batch]| {
        let mut elements: BTreeMap<u32, (usize, f64)> = BTreeMap::new();
        for batch in batches {
            let corner = |index: u32| -> [f64; 3] {
                let at = index as usize * 3;
                std::array::from_fn(|axis| batch.positions[at + axis] as f64 + batch.origin[axis])
            };
            for record in batch.mesh_table.chunks_exact(RECORD_WORDS) {
                let entry = elements.entry(record[0]).or_default();
                let first = record[3] as usize;
                for triangle in batch.indices[first..first + record[4] as usize].chunks_exact(3) {
                    let ([ax, ay, az], [bx, by, bz], [cx, cy, cz]) =
                        (corner(triangle[0]), corner(triangle[1]), corner(triangle[2]));
                    entry.0 += 1;
                    entry.1 += (ax * (by * cz - bz * cy) + ay * (bz * cx - bx * cz)
                        + az * (bx * cy - by * cx))
                        / 6.0;
                }
            }
        }
        elements
    };

    let whole = stream(&mut model, usize::MAX);
    let small = stream(&mut model, 50);
    assert!(small.len() > whole.len(), "{} and {}", small.len(), whole.len());
    let (whole, small) = (per_element(&whole), per_element(&small));
    assert_eq!(whole.len(), small.len());
    for ((id, a), (other, b)) in whole.iter().zip(&small) {
        assert_eq!((id, a.0), (other, b.0));
        assert!((a.1 - b.1).abs() < 1e-3, "#{id}: {} and {}", a.1, b.1);
    }
}

#[test]
fn far_apart_elements_get_separate_batches_near_real_coordinates() {
    let wall = |id: u32, x: f64| {
        format!(
            "#{id}=IFCWALL('{id}',$,$,$,$,#{},#10,$);\n\
             #{}=IFCLOCALPLACEMENT($,#{});\n\
             #{}=IFCAXIS2PLACEMENT3D(#{},$,$);\n\
             #{}=IFCCARTESIANPOINT(({x},4747000.,260.));\n",
            id + 1,
            id + 1,
            id + 2,
            id + 2,
            id + 3,
            id + 3
        )
    };
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n\
         #1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
         #10=IFCPRODUCTDEFINITIONSHAPE($,$,(#11));\n\
         #11=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#12));\n\
         #12=IFCEXTRUDEDAREASOLID(#13,#15,#16,3.);\n\
         #13=IFCRECTANGLEPROFILEDEF(.AREA.,$,#14,4.,0.2);\n\
         #14=IFCAXIS2PLACEMENT2D(#17,$);\n\
         #15=IFCAXIS2PLACEMENT3D(#17,$,$);\n\
         #16=IFCDIRECTION((0.,0.,1.));\n\
         #17=IFCCARTESIANPOINT((0.,0.,0.));\n\
         {}{}{}ENDSEC;END-ISO-10303-21;",
        wall(20, 536000.0),
        wall(30, 536010.0),
        wall(40, 590000.0)
    );
    let mut model = LoadedModel::open(text.into_bytes()).unwrap();
    let batches = stream(&mut model, usize::MAX);
    batches.iter().for_each(check);

    assert_eq!(batches.len(), 2, "{:?}", batches.iter().map(|b| b.origin).collect::<Vec<_>>());
    assert_eq!(batches[0].origin, [536000.0, 4747000.0, 262.0]);
    assert_eq!(batches[0].mesh_table.len() / RECORD_WORDS, 2);
    assert_eq!(batches[1].origin, [590000.0, 4747000.0, 262.0]);
    let complete = model.finish_stream();
    assert!(complete.contains(r#""min":{"x":535998,"#), "{complete}");
    assert!(complete.contains(r#""max":{"x":590002,"#), "{complete}");
}

#[test]
fn a_huge_element_is_flagged_instead_of_losing_precision_silently() {
    let text = "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n\
        #1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
        #2=IFCPROXY('w',$,$,$,$,$,#3,$,.PRODUCT.,$);\n\
        #3=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n\
        #4=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#5));\n\
        #5=IFCTRIANGULATEDFACESET(#6,$,$,((1,2,3)),$);\n\
        #6=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(20000.,0.,0.),(0.,20000.,0.)));\n\
        ENDSEC;END-ISO-10303-21;";
    let mut model = LoadedModel::open(text.as_bytes().to_vec()).unwrap();
    let batches = stream(&mut model, usize::MAX);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].mesh_table[6] & DEGRADED, DEGRADED);
    assert!(model.finish_stream().contains("batch-radius-exceeded"));
}

#[test]
fn degenerate_and_corrupt_triangles_are_dropped_and_reported() {
    let text = "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC4'));ENDSEC;DATA;\n\
        #1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
        #2=IFCPROXY('w',$,$,$,$,$,#3,$,.PRODUCT.,$);\n\
        #3=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n\
        #4=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#5));\n\
        #5=IFCTRIANGULATEDFACESET(#6,$,$,((1,2,3),(1,1,2),(1,2,4)),$);\n\
        #6=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.),(1.E300,1.,0.)));\n\
        ENDSEC;END-ISO-10303-21;";
    let mut model = LoadedModel::open(text.as_bytes().to_vec()).unwrap();
    let batches = stream(&mut model, usize::MAX);
    batches.iter().for_each(check);
    assert_eq!(batches[0].indices.len(), 3, "only the good triangle stays");
    assert_eq!(batches[0].mesh_table[6] & DEGRADED, DEGRADED);
    assert!(model.finish_stream().contains(r#""code":"invalid-triangles""#));
}

#[test]
fn open_reports_what_went_wrong() {
    let error = LoadedModel::open(b"not an ifc file".to_vec()).err().unwrap();
    assert!(matches!(error, OpenError::Parse(_)), "{error:?}");

    let text = "ISO-10303-21;HEADER;FILE_SCHEMA(('AP214'));ENDSEC;DATA;\nENDSEC;END-ISO-10303-21;";
    let error = LoadedModel::open(text.as_bytes().to_vec()).err().unwrap();
    assert!(matches!(error, OpenError::UnsupportedSchema(_)), "{error:?}");
}

#[test]
fn streaming_twice_gives_the_same_geometry_and_diagnostics() {
    let bytes = fs::read(root().join("public/samples/SimpleWall.ifc")).unwrap();
    let mut model = LoadedModel::open(bytes).unwrap();
    let first = stream(&mut model, usize::MAX);
    let first_end = model.finish_stream();
    let second = stream(&mut model, usize::MAX);
    assert_eq!(first, second);
    assert_eq!(first_end, model.finish_stream());
}

#[test]
fn summary_and_details_use_the_contract_field_names() {
    let bytes = fs::read(root().join("public/samples/SimpleWall.ifc")).unwrap();
    let model = LoadedModel::open(bytes).unwrap();

    let summary = model.summary_json();
    for expected in [
        r#""schema":"IFC2X3""#,
        r#""entityCount":548"#,
        r#""elementCount":2"#,
        r#""lengthUnitScale":0.001"#,
        r#""lengthUnitSource":"ifc""#,
        r#""source":"ifc-site""#,
    ] {
        assert!(summary.contains(expected), "{expected} in {summary}");
    }

    let tree = model.spatial_tree_json().unwrap();
    assert!(tree.starts_with(r#"{"root":{"expressId":121,"type":"IfcProject""#), "{tree}");
    assert!(tree.contains(r#""via":"contains""#), "{tree}");

    let wall = model.element_details_json(219).unwrap();
    for expected in [
        r#""type":"IfcWallStandardCase""#,
        r#""globalId":"1F6umJ5H50aeL3A1As_wTm""#,
        r#""Tag":"346660""#,
        r#""kind":"quantity""#,
        r#""unit":"mm""#,
        r#""kind":"layer-set""#,
        r#""source":"instance""#,
        r#""name":"Default Wall","thickness":0.2"#,
    ] {
        assert!(wall.contains(expected), "{expected} in {wall}");
    }
    assert!(model.element_details_json(999_999).is_none());
}
