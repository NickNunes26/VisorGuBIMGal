use cad_slicer::{
    Cut, DrawingMesh, DxfRoundTripEdit, build_clash_section_sheet, parse_clash_dxf_edits,
    to_clash_dxf,
};
use clash_detector::{ClashMesh, test_pair};
use glam::DVec3;
use ifc_geometry::{Mesh, ModelEvaluator};
use step_parser::{StepPatch, index, patch_step_file};

/// Minimal valid IFC4 file containing two overlapping beams (#100 Element A, #200 Element B)
/// along X:
/// - #100: X=[0.0, 2.0], Y=[0.0, 0.04], Z=[0.0, 0.04] (GUID '2O2Fr$t4X7Zf8NOew3FLOH')
/// - #200: X=[0.0, 2.0], Y=[0.01, 0.05], Z=[0.01, 0.05] (GUID '3P3Gs$u5Y8Ag9OPfx4GMPI')
///
/// They overlap by 30 mm in Y and 30 mm in Z -> hard clash!
const CLASHING_IFC: &str = "\
ISO-10303-21;\r\n\
HEADER;\r\n\
FILE_DESCRIPTION(('ViewDefinition [DesignTransferView]'),'2;1');\r\n\
FILE_NAME('clash_test.ifc','2026-11-23T00:00:00',(''),(''),'','','');\r\n\
FILE_SCHEMA(('IFC4'));\r\n\
ENDSEC;\r\n\
DATA;\r\n\
#1=IFCPROJECT('0000000000000000000001',$,'Bridge',$,$,$,$,(#10),#2);\r\n\
#2=IFCUNITASSIGNMENT((#3));\r\n\
#3=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\r\n\
#10=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#11,$);\r\n\
#11=IFCAXIS2PLACEMENT3D(#12,$,$);\r\n\
#12=IFCCARTESIANPOINT((0.,0.,0.));\r\n\
#20=IFCSITE('0000000000000000000002',$,'Site',$,$,#21,$,$,.ELEMENT.,$,$,$,$,$);\r\n\
#21=IFCLOCALPLACEMENT($,#11);\r\n\
#22=IFCRELAGGREGATES('0000000000000000000003',$,$,$,#1,(#20));\r\n\
#30=IFCDIRECTION((0.,0.,1.));\r\n\
#31=IFCDIRECTION((1.,0.,0.));\r\n\
#32=IFCCARTESIANPOINT((0.02,0.02));\r\n\
#33=IFCAXIS2PLACEMENT2D(#32,$);\r\n\
#34=IFCRECTANGLEPROFILEDEF(.AREA.,$,#33,0.04,0.04);\r\n\
#35=IFCAXIS2PLACEMENT3D(#12,#31,#30);\r\n\
#36=IFCEXTRUDEDAREASOLID(#34,#35,#30,2.0);\r\n\
#37=IFCSHAPEREPRESENTATION(#10,'Body','SweptSolid',(#36));\r\n\
#38=IFCPRODUCTDEFINITIONSHAPE($,$,(#37));\r\n\
#101=IFCCARTESIANPOINT((0.,0.,0.));\r\n\
#102=IFCAXIS2PLACEMENT3D(#101,$,$);\r\n\
#103=IFCLOCALPLACEMENT(#21,#102);\r\n\
#100=IFCREINFORCINGBAR('2O2Fr$t4X7Zf8NOew3FLOH',$,'Bar_A',$,$,#103,#38,$,$,$,$,$,$,$);\r\n\
#201=IFCCARTESIANPOINT((0.,0.01,0.01));\r\n\
#202=IFCAXIS2PLACEMENT3D(#201,$,$);\r\n\
#203=IFCLOCALPLACEMENT(#21,#202);\r\n\
#200=IFCTENDON('3P3Gs$u5Y8Ag9OPfx4GMPI',$,'Tendon_B',$,$,#203,#38,$,$,$,$,$,$,$,$,$,$,$);\r\n\
#300=IFCRELCONTAINEDINSPATIALSTRUCTURE('0000000000000000000004',$,$,$,(#100,#200),#20);\r\n\
ENDSEC;\r\n\
END-ISO-10303-21;\r\n";

fn eval_mesh(ifc_bytes: &[u8], express_id: u32) -> Mesh {
    let step = index(ifc_bytes).expect("valid STEP");
    let mut ev = ModelEvaluator::new(&step).expect("evaluator");
    let geom = ev.evaluate(express_id).expect("element geometry");
    let mut combined = Mesh::default();
    for part in geom.meshes {
        let base = combined.positions.len() as u32;
        combined.positions.extend(part.mesh.positions);
        combined.triangles.extend(
            part.mesh
                .triangles
                .into_iter()
                .map(|[a, b, c]| [a + base, b + base, c + base]),
        );
    }
    combined
}

#[test]
fn f5_roundtrip_export_dxf_move_insert_patch_step_resolves_clash_and_keeps_guids() {
    let orig_bytes = CLASHING_IFC.as_bytes();

    // 1. Confirm initial hard clash between #100 (Bar_A) and #200 (Tendon_B).
    let mesh_a = eval_mesh(orig_bytes, 100);
    let mesh_b = eval_mesh(orig_bytes, 200);
    let cm_a = ClashMesh::new(mesh_a.clone()).expect("mesh A");
    let cm_b = ClashMesh::new(mesh_b.clone()).expect("mesh B");
    assert!(
        test_pair(&cm_a, &cm_b, 0.0).is_some(),
        "initial IFC must have a hard clash between #100 and #200"
    );

    // 2. Export clash cross-section DXF at X = 1.0 perpendicular to X alignment.
    let cut = Cut::Plane {
        origin: DVec3::new(1.0, 0.0, 0.0),
        normal: DVec3::X,
    };
    let sheet = build_clash_section_sheet(
        &[
            DrawingMesh::new(0, 100, "IfcReinforcingBar", mesh_a),
            DrawingMesh::new(0, 200, "IfcTendon", mesh_b),
        ],
        cut,
        0.02,
        &[(0, 100, "2O2Fr$t4X7Zf8NOew3FLOH".to_string())],
        &[(0, 200)],
        &[DVec3::new(1.0, 0.025, 0.025)],
        Some(DVec3::new(1.0, 0.0, 0.0)),
        "bridge",
        1.0,
        "0+001.00",
        1.0,
        1.02,
        vec!["PK 0+001.00".to_string()],
    );
    let dxf_bytes = to_clash_dxf(&sheet);

    // 3. Simulate drafter moving the INSERT GUBIM_A_100_... by (0.05, 0.03) m in CAD!
    let orig_insert = sheet.blocks_a[0].insert_point;
    let moved_x = orig_insert.x + 0.05;
    let moved_y = orig_insert.y + 0.03;
    let dxf_text = String::from_utf8(dxf_bytes).expect("utf8 dxf");
    let insert_needle = format!(
        "0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n{}\r\n10\r\n{:.6}\r\n20\r\n{:.6}\r\n",
        sheet.blocks_a[0].block_name, orig_insert.x, orig_insert.y
    );
    let insert_replacement = format!(
        "0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n{}\r\n10\r\n{:.6}\r\n20\r\n{:.6}\r\n",
        sheet.blocks_a[0].block_name, moved_x, moved_y
    );
    assert!(
        dxf_text.contains(&insert_needle),
        "exported DXF must contain exact INSERT for GUBIM_A_100"
    );
    let edited_dxf = dxf_text.replace(&insert_needle, &insert_replacement);

    // 4. Reimport edited DXF and verify exact Level 1 Move delta.
    let diff = parse_clash_dxf_edits(edited_dxf.as_bytes()).expect("parse edited DXF");
    assert_eq!(diff.edits.len(), 1);
    let delta_3d = match &diff.edits[0] {
        DxfRoundTripEdit::Move {
            express_id,
            delta_2d,
            delta_3d,
            ..
        } => {
            assert_eq!(*express_id, 100);
            assert!((delta_2d.x - 0.05).abs() < 1e-6);
            assert!((delta_2d.y - 0.03).abs() < 1e-6);
            *delta_3d
        }
        other => panic!("expected Move edit, got {other:?}"),
    };

    // 5. Surgically patch original IFC STEP file.
    let (patched_bytes, report) = patch_step_file(
        orig_bytes,
        &[StepPatch::MovePlacement {
            express_id: 100,
            delta_file_units: delta_3d.to_array(),
            expected_global_id: None,
            note: None,
        }],
    )
    .expect("surgical STEP patch");
    assert_eq!(report.applied, 1);
    assert_eq!(report.skipped, 0);

    // GUIDs and untouched entities are preserved verbatim!
    let patched_text = String::from_utf8_lossy(&patched_bytes);
    assert!(patched_text.contains("2O2Fr$t4X7Zf8NOew3FLOH"));
    assert!(patched_text.contains("3P3Gs$u5Y8Ag9OPfx4GMPI"));
    assert!(patched_text.contains("#200=IFCTENDON('3P3Gs$u5Y8Ag9OPfx4GMPI'"));

    // 6. Reload patched IFC and re-run clash detection -> 0 clashes!
    let patched_a = ClashMesh::new(eval_mesh(&patched_bytes, 100)).expect("patched A");
    let patched_b = ClashMesh::new(eval_mesh(&patched_bytes, 200)).expect("patched B");
    assert_eq!(
        test_pair(&patched_a, &patched_b, 0.0),
        None,
        "patched IFC must have 0 clashes after (0.05, 0.03) m move!"
    );
}

#[test]
fn f5_level2_rebend_swept_disk_patches_representation_and_parses_cleanly() {
    let orig_bytes = CLASHING_IFC.as_bytes();
    let (patched_bytes, report) = patch_step_file(
        orig_bytes,
        &[StepPatch::RebendSweptDisk {
            express_id: 100,
            points_file_units: vec![[0.0, 0.20, 0.20], [1.0, 0.25, 0.20], [2.0, 0.20, 0.20]],
            radius_file_units: 0.01,
            expected_global_id: None,
            note: None,
        }],
    )
    .expect("Level 2 rebend patch");
    assert_eq!(report.applied, 1);
    let patched_text = String::from_utf8_lossy(&patched_bytes);
    assert!(patched_text.contains("IFCCARTESIANPOINTLIST3D("));
    assert!(patched_text.contains("IFCINDEXEDPOLYCURVE("));
    assert!(patched_text.contains("IFCSWEPTDISKSOLID("));
    assert!(patched_text.contains("2O2Fr$t4X7Zf8NOew3FLOH"));

    //Patched IFC must still be 100% valid STEP!
    let step = index(&patched_bytes).expect("patched STEP must parse cleanly");
    assert!(step.get(100).is_some());
}

#[test]
fn f5_level2_full_dxf_polyline_edit_auto_radius_and_resolves_clash() {
    let orig_bytes = CLASHING_IFC.as_bytes();
    let mesh_a = eval_mesh(orig_bytes, 100);
    let mesh_b = eval_mesh(orig_bytes, 200);

    // Export transverse section (depth = 0.02 m) so EDIT_A_100 polyline and GUBIM_POLY are emitted.
    let cut = Cut::Plane {
        origin: DVec3::new(0.99, 0.0, 0.0),
        normal: DVec3::X,
    };
    let sheet = build_clash_section_sheet(
        &[
            DrawingMesh::new(0, 100, "IfcReinforcingBar", mesh_a),
            DrawingMesh::new(0, 200, "IfcTendon", mesh_b),
        ],
        cut,
        0.02,
        &[(0, 100, "2O2Fr$t4X7Zf8NOew3FLOH".to_string())],
        &[(0, 200)],
        &[DVec3::new(1.0, 0.025, 0.025)],
        Some(DVec3::new(1.0, 0.0, 0.0)),
        "bridge",
        1.0,
        "0+001.00",
        0.99,
        1.01,
        vec!["PK 0+001.00".to_string()],
    );
    assert_eq!(sheet.blocks_a[0].edit_polyline.len(), 2);
    let dxf_bytes = to_clash_dxf(&sheet);
    let dxf_text = String::from_utf8(dxf_bytes).expect("utf8 dxf");
    assert!(
        dxf_text.contains("EDIT_A_100"),
        "transverse DXF must export EDIT_A_100 centerline polyline"
    );
    assert!(
        dxf_text.contains("GUBIM_POLY id=100"),
        "transverse DXF must record original GUBIM_POLY in GUBIM_META"
    );

    // Unedited DXF must produce 0 edits (GUBIM_POLY matches EDIT_A_100).
    let unedited_diff = parse_clash_dxf_edits(dxf_text.as_bytes()).expect("parse unedited");
    assert!(
        unedited_diff.edits.is_empty(),
        "unedited DXF must report 0 edits, got {:?}",
        unedited_diff.edits
    );

    // Simulate drafter replacing EDIT_A_100 with a shifted LWPOLYLINE clear of Tendon_B (#200 is at Y,Z in [0.01, 0.05]).
    let lwpoly_injection = "0\r\nLWPOLYLINE\r\n8\r\nEDIT_A_100\r\n10\r\n0.200000\r\n20\r\n0.200000\r\n10\r\n0.300000\r\n20\r\n0.250000\r\n10\r\n0.400000\r\n20\r\n0.200000\r\n0\r\nENDSEC\r\n0\r\nEOF\r\n";
    let edited_dxf = dxf_text.replacen("0\r\nENDSEC\r\n0\r\nEOF\r\n", lwpoly_injection, 1);

    let (patched_bytes, json) =
        wasm_api::clash_dxf::apply_clash_dxf_to_step(orig_bytes, 1.0, edited_dxf.as_bytes())
            .expect("apply Level 2 DXF rebend");
    assert!(json.contains("\"kind\":\"rebend\""));
    assert!(json.contains("\"applied\":1"));

    // Reload patched IFC -> #100 is now an IfcSweptDiskSolid clear of #200 -> 0 clashes!
    let patched_a = ClashMesh::new(eval_mesh(&patched_bytes, 100)).expect("patched rebent A");
    let patched_b = ClashMesh::new(eval_mesh(&patched_bytes, 200)).expect("patched B");
    assert_eq!(
        test_pair(&patched_a, &patched_b, 0.0),
        None,
        "Level 2 rebent bar must have 0 clashes against Tendon_B"
    );
}

#[test]
fn f5_level1_move_under_rotated_parent_placement_rotates_delta_into_parent_frame() {
    // Rotate parent site #21 by +90 deg around Z: RefDirection = (0, 1, 0) instead of (1, 0, 0).
    let rotated_ifc = CLASHING_IFC
        .replace(
            "#11=IFCAXIS2PLACEMENT3D(#12,$,$);",
            "#11=IFCAXIS2PLACEMENT3D(#12,#30,#13);\r\n#13=IFCDIRECTION((0.,1.,0.));",
        );
    let orig_bytes = rotated_ifc.as_bytes();
    let before_a = eval_mesh(orig_bytes, 100);
    let before_min = before_a
        .positions
        .iter()
        .copied()
        .reduce(|a, b| a.min(b))
        .unwrap();

    // Request a world-space move of (+0.50, -0.25, +0.10) m.
    let world_delta = [0.50, -0.25, 0.10];
    let (patched_bytes, report) = patch_step_file(
        orig_bytes,
        &[StepPatch::MovePlacement {
            express_id: 100,
            delta_file_units: world_delta,
            expected_global_id: None,
            note: None,
        }],
    )
    .expect("patch rotated parent placement");
    assert_eq!(report.applied, 1);

    let after_a = eval_mesh(&patched_bytes, 100);
    let after_min = after_a
        .positions
        .iter()
        .copied()
        .reduce(|a, b| a.min(b))
        .unwrap();
    let actual_shift = after_min - before_min;
    assert!(
        (actual_shift.x - world_delta[0]).abs() < 1e-6
            && (actual_shift.y - world_delta[1]).abs() < 1e-6
            && (actual_shift.z - world_delta[2]).abs() < 1e-6,
        "expected world shift {:?}, got {:?}",
        world_delta,
        actual_shift
    );
}


// ---------------------------------------------------------------------------------------------
// Regression tests for the 2026-11-24 F5 review (COORDINATION.md log).
// ---------------------------------------------------------------------------------------------

const LONG_MODEL_ID: &str = "model-1795544256021-8nae";
const GUID_A: &str = "2O2Fr$t4X7Zf8NOew3FLOH";

/// CLASHING_IFC moved to UTM-like coordinates (bar and tendon placements only).
fn utm_ifc() -> String {
    CLASHING_IFC
        .replace(
            "#101=IFCCARTESIANPOINT((0.,0.,0.));",
            "#101=IFCCARTESIANPOINT((540000.,4700000.,0.));",
        )
        .replace(
            "#201=IFCCARTESIANPOINT((0.,0.01,0.01));",
            "#201=IFCCARTESIANPOINT((540000.,4700000.01,0.01));",
        )
}

fn utm_sheet(ifc: &str) -> cad_slicer::ClashSectionSheet {
    let mesh_a = eval_mesh(ifc.as_bytes(), 100);
    let mesh_b = eval_mesh(ifc.as_bytes(), 200);
    build_clash_section_sheet(
        &[
            DrawingMesh::new(0, 100, "IfcReinforcingBar", mesh_a),
            DrawingMesh::new(0, 200, "IfcTendon", mesh_b),
        ],
        Cut::Plane {
            origin: DVec3::new(540_000.99, 4_700_000.0, 0.0),
            normal: DVec3::X,
        },
        0.02,
        &[(0, 100, GUID_A.to_string())],
        &[(0, 200)],
        &[DVec3::new(540_001.0, 4_700_000.025, 0.025)],
        Some(DVec3::new(540_001.0, 4_700_000.0, 0.0)),
        LONG_MODEL_ID,
        1234.5,
        "1+234.50",
        540_000.99,
        540_001.01,
        vec!["PK 1+234.50".to_string()],
    )
}

/// Moves the INSERT of a `GUBIM_A_<id>_*` block by (dx, dy) sheet metres.
fn move_insert(dxf: &str, express_id: u32, dx: f64, dy: f64) -> String {
    let mut lines: Vec<String> = dxf.split("\r\n").map(str::to_string).collect();
    let prefix = format!("GUBIM_A_{express_id}");
    let mut i = 0;
    let mut moved = 0;
    while i + 1 < lines.len() {
        if lines[i].trim() == "0" && lines[i + 1].trim() == "INSERT" {
            let mut j = i + 2;
            let (mut block, mut ix, mut iy) = (String::new(), 0, 0);
            while j + 1 < lines.len() && lines[j].trim() != "0" {
                match lines[j].trim() {
                    "2" => block = lines[j + 1].trim().to_string(),
                    "10" => ix = j + 1,
                    "20" => iy = j + 1,
                    _ => {}
                }
                j += 2;
            }
            if block.starts_with(&prefix) {
                let x: f64 = lines[ix].trim().parse().unwrap();
                let y: f64 = lines[iy].trim().parse().unwrap();
                lines[ix] = format!("{:.6}", x + dx);
                lines[iy] = format!("{:.6}", y + dy);
                moved += 1;
            }
            i = j;
        } else {
            i += 2;
        }
    }
    assert_eq!(moved, 1, "one INSERT for #{express_id}");
    lines.join("\r\n")
}

/// Appends an `EDIT_A_<id>` LWPOLYLINE (sheet metres) at the end of ENTITIES.
fn add_edit_polyline(dxf: &str, express_id: u32, pts: &[(f64, f64)]) -> String {
    let mut ent = format!("0\r\nLWPOLYLINE\r\n8\r\nEDIT_A_{express_id}\r\n");
    for (x, y) in pts {
        ent.push_str(&format!("10\r\n{x:.6}\r\n20\r\n{y:.6}\r\n"));
    }
    dxf.replacen("0\r\nENDSEC\r\n0\r\nEOF", &format!("{ent}0\r\nENDSEC\r\n0\r\nEOF"), 1)
}

fn bounds(mesh: &Mesh) -> (DVec3, DVec3) {
    let lo = mesh.positions.iter().copied().reduce(|a, b| a.min(b)).unwrap();
    let hi = mesh.positions.iter().copied().reduce(|a, b| a.max(b)).unwrap();
    (lo, hi)
}

fn json_field<'a>(json: &'a str, key: &str) -> &'a str {
    let at = json.find(&format!("\"{key}\":")).unwrap() + key.len() + 3;
    let end = json[at..].find([',', '}']).unwrap();
    &json[at..at + end]
}

#[test]
fn f5_metadata_fits_r12_limit_with_long_model_id_and_utm_coordinates() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    for line in dxf.split("\r\n") {
        assert!(line.len() <= 255, "DXF value over the R12 limit: {} chars", line.len());
    }
    assert!(dxf.contains(&format!("guid={GUID_A}")), "GUBIM_ORIG carries the full GlobalId");

    let diff = parse_clash_dxf_edits(dxf.as_bytes()).expect("parse");
    assert_eq!(diff.meta.model_id, LONG_MODEL_ID);
    assert!(diff.meta.complete);
    let norm = diff.meta.norm_offset.unwrap();
    assert!((norm - sheet.meta.norm_offset).length() < 1e-6);
    assert!(diff.edits.is_empty(), "unedited DXF has no edits: {:?}", diff.edits);

    // A move through the long-id, UTM DXF reaches the IFC and clears the clash.
    let edited = move_insert(&dxf, 100, 0.05, 0.03);
    let (patched, json) =
        wasm_api::clash_dxf::apply_clash_dxf_to_step_with(ifc.as_bytes(), 1.0, edited.as_bytes(), true)
            .expect("apply");
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let a = ClashMesh::new(eval_mesh(&patched, 100)).unwrap();
    let b = ClashMesh::new(eval_mesh(&patched, 200)).unwrap();
    assert_eq!(test_pair(&a, &b, 0.0), None);
}

#[test]
fn f5_legacy_clipped_v1_metadata_still_moves_and_rejects_rebend() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let m = &sheet.meta;
    let f = &m.frame;
    let mut v1 = format!(
        "GUBIM_META v=1 model={} pk={:.6} sMin={:.6} sMax={:.6} ox={:.6} oy={:.6} oz={:.6} rx={:.6} ry={:.6} rz={:.6} ux={:.6} uy={:.6} uz={:.6} lx={:.6} ly={:.6} lz={:.6} nx={:.6} ny={:.6}",
        m.model_id, m.pk_m, m.s_min, m.s_max, f.origin.x, f.origin.y, f.origin.z, f.right.x,
        f.right.y, f.right.z, f.up.x, f.up.y, f.up.z, f.look.x, f.look.y, f.look.z,
        m.norm_offset.x, m.norm_offset.y
    );
    assert!(v1.len() > 255);
    v1.truncate(255);
    // Rewrite every GUBIM_META record as the clipped v1 line, as 4455862 exported it.
    let dxf: String = String::from_utf8(to_clash_dxf(&sheet))
        .unwrap()
        .split("\r\n")
        .map(|l| if l.starts_with("GUBIM_META ") { v1.clone() } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\r\n");

    let edited = add_edit_polyline(
        &move_insert(&dxf, 100, 0.05, 0.03),
        100,
        &[(0.1, 0.4), (0.2, 0.45), (0.3, 0.4)],
    );
    let diff = parse_clash_dxf_edits(edited.as_bytes()).expect("legacy parse");
    assert!(!diff.meta.complete);
    assert_eq!(diff.rejected.len(), 1);
    assert_eq!(diff.rejected[0].reason, "metadata-incomplete");

    let (patched, json) =
        wasm_api::clash_dxf::apply_clash_dxf_to_step(ifc.as_bytes(), 1.0, edited.as_bytes())
            .expect("apply legacy");
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!(json.contains("\"reason\":\"metadata-incomplete\""), "{json}");
    let (before, _) = bounds(&eval_mesh(ifc.as_bytes(), 100));
    let (after, _) = bounds(&eval_mesh(&patched, 100));
    let shift = after - before;
    let expected = f.right * 0.05 + f.up * 0.03;
    assert!((shift - expected).length() < 1e-6, "shift {shift:?} expected {expected:?}");
}

#[test]
fn f5_reimport_is_idempotent_and_later_edits_apply_only_the_difference() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let first = move_insert(&dxf, 100, 0.0, 0.05);
    let apply = |ifc: &[u8], dxf: &str| {
        wasm_api::clash_dxf::apply_clash_dxf_to_step(ifc, 1.0, dxf.as_bytes()).expect("apply")
    };

    let (patched1, json1) = apply(ifc.as_bytes(), &first);
    assert_eq!(json_field(&json1, "applied"), "1");
    let (patched2, json2) = apply(&patched1, &first);
    assert_eq!(json_field(&json2, "applied"), "0", "{json2}");
    assert!(json2.contains("already-applied"));
    assert_eq!(patched2, patched1, "a repeated import changes nothing");

    // The drafter moves the same bar further in the same export: only +0.03 more is applied.
    let second = move_insert(&dxf, 100, 0.0, 0.08);
    let (patched3, json3) = apply(&patched1, &second);
    assert_eq!(json_field(&json3, "applied"), "1", "{json3}");
    let (orig_lo, _) = bounds(&eval_mesh(ifc.as_bytes(), 100));
    let (lo3, _) = bounds(&eval_mesh(&patched3, 100));
    let shift = lo3 - orig_lo;
    let expected = sheet.meta.frame.up * 0.08;
    assert!((shift - expected).length() < 1e-6, "shift {shift:?} expected {expected:?}");
}

#[test]
fn f5_move_and_rebend_of_one_bar_lands_where_the_polyline_is() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let ins = sheet.blocks_a[0].insert_point;
    // Block moved up 0.3 m and a new centreline drawn 0.3 m higher.
    let y = ins.y + 0.3;
    let edited = add_edit_polyline(
        &move_insert(&dxf, 100, 0.0, 0.3),
        100,
        &[(ins.x - 0.05, y), (ins.x + 0.05, y)],
    );
    let (patched, json) =
        wasm_api::clash_dxf::apply_clash_dxf_to_step(ifc.as_bytes(), 1.0, edited.as_bytes())
            .expect("apply");
    assert_eq!(json_field(&json, "applied"), "2", "{json}");
    let (lo, hi) = bounds(&eval_mesh(&patched, 100));
    let frame = &sheet.meta.frame;
    // Sheet y maps to world along `up`; the bar is a disk of radius 0.02 around the line.
    let target_up = y + sheet.meta.norm_offset.y;
    let centre_up = (lo + hi).dot(frame.up) * 0.5;
    assert!(
        (centre_up - target_up).abs() < 1e-4,
        "bar centre at {centre_up}, polyline at {target_up}"
    );
}

#[test]
fn f5_dxf_applied_to_another_model_changes_nothing() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let edited = move_insert(&dxf, 100, 0.0, 0.05);
    // Another file whose #100 is a different product.
    let other = ifc.replace(GUID_A, "1F6umJ5H50aeL3A1As_wTm");
    let (patched, json) =
        wasm_api::clash_dxf::apply_clash_dxf_to_step(other.as_bytes(), 1.0, edited.as_bytes())
            .expect("apply");
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"reason\":\"guid-mismatch\""), "{json}");
    assert_eq!(patched, other.as_bytes());
}

#[test]
fn f5_move_on_linear_placement_follows_the_kernel_frame() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test/fixtures/authored/linear-placement.ifc");
    let ifc = std::fs::read(path).expect("fixture");
    let delta = [1.0, -0.5, 0.3];
    let patch = || StepPatch::MovePlacement {
        express_id: 330,
        delta_file_units: delta,
        expected_global_id: Some("02WmjFQgYJQU9R_9iuhTpu".into()),
        note: None,
    };
    // Without the kernel the patcher refuses rather than guessing the frame.
    let (_, report) = patch_step_file(&ifc, &[patch()]).unwrap();
    assert_eq!(report.outcomes[0].reason, Some("unsupported-placement"));

    let (patched, report) =
        wasm_api::clash_dxf::patch_with_kernel_placements(&ifc, &[patch()], 1.0, true).unwrap();
    assert_eq!(report.applied, 1, "{:?}", report.outcomes);
    let (before, _) = bounds(&eval_mesh(&ifc, 330));
    let (after, _) = bounds(&eval_mesh(&patched, 330));
    let shift = after - before;
    assert!(
        (shift - DVec3::from_array(delta)).length() < 1e-6,
        "shift {shift:?} expected {delta:?}"
    );
}
#[test]
fn f5_rebend_meshes_and_clears_the_clash_in_every_schema() {
    for schema in ["IFC2X3", "IFC4", "IFC4X3_ADD2"] {
        let ifc = utm_ifc().replace("FILE_SCHEMA(('IFC4'))", &format!("FILE_SCHEMA(('{schema}'))"));
        let sheet = utm_sheet(&ifc);
        let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
        let ins = sheet.blocks_a[0].insert_point;
        let y = ins.y + 0.25;
        let edited = add_edit_polyline(&dxf, 100, &[(ins.x - 0.04, y), (ins.x, y + 0.02), (ins.x + 0.04, y)]);
        let (patched, json) =
            wasm_api::clash_dxf::apply_clash_dxf_to_step(ifc.as_bytes(), 1.0, edited.as_bytes())
                .expect("apply");
        assert_eq!(json_field(&json, "applied"), "1", "{schema}: {json}");
        let mesh = eval_mesh(&patched, 100);
        assert!(!mesh.triangles.is_empty(), "{schema}: rebent bar has no mesh");
        let a = ClashMesh::new(mesh).unwrap();
        let b = ClashMesh::new(eval_mesh(&patched, 200)).unwrap();
        assert_eq!(test_pair(&a, &b, 0.0), None, "{schema}: clash left");
    }
}


// ---------------------------------------------------------------------------------------------
// Regression tests for the second F5 review (`test/e2e/review-f5/REPORT.md`, F1-F10).
// ---------------------------------------------------------------------------------------------

const GUID_RIGHT: &str = "1Right0000000000000000";

fn apply_as(ifc: &[u8], dxf: &str, me: &str, open: &[&str], exclude: &[u32]) -> (Vec<u8>, String) {
    let scope = wasm_api::clash_dxf::ApplyScope {
        self_model_id: Some(me.to_string()),
        open_model_ids: open.iter().map(|s| s.to_string()).collect(),
        exclude: exclude.to_vec(),
        require_global_id: true,
        ..Default::default()
    };
    wasm_api::clash_dxf::apply_clash_dxf_to_step_scoped(ifc, 1.0, dxf.as_bytes(), &scope)
        .expect("apply")
}

fn apply_plain(ifc: &[u8], dxf: &str) -> (Vec<u8>, String) {
    wasm_api::clash_dxf::apply_clash_dxf_to_step(ifc, 1.0, dxf.as_bytes()).expect("apply")
}

/// `index` of every applied entry of a result JSON.
fn applied_indices(json: &str) -> Vec<u32> {
    json.split("{\"kind\"")
        .filter(|entry| entry.contains("\"status\":\"applied\""))
        .filter_map(|entry| json_field(entry, "index").parse().ok())
        .collect()
}

/// Moves the INSERT of exactly the named block by (dx, dy) sheet metres.
fn move_block(dxf: &str, block_name: &str, dx: f64, dy: f64) -> String {
    let needle = format!("2\r\n{block_name}\r\n10\r\n");
    let at = dxf.find(&needle).expect("INSERT of the block") + needle.len();
    let rest = &dxf[at..];
    let x_end = rest.find("\r\n").unwrap();
    let x: f64 = rest[..x_end].parse().unwrap();
    let after_x = &rest[x_end + 2..];
    assert!(after_x.starts_with("20\r\n"));
    let y_start = x_end + 2 + 4;
    let y_end = y_start + rest[y_start..].find("\r\n").unwrap();
    let y: f64 = rest[y_start..y_end].parse().unwrap();
    format!(
        "{}{:.6}\r\n20\r\n{:.6}{}",
        &dxf[..at],
        x + dx,
        y + dy,
        &rest[y_end..]
    )
}

/// The INSERT entity of the sheet's first block, exactly as exported.
fn insert_needle(sheet: &cad_slicer::ClashSectionSheet) -> String {
    let b = &sheet.blocks_a[0];
    format!(
        "0\r\nINSERT\r\n8\r\nCLASH_A\r\n2\r\n{}\r\n10\r\n{:.6}\r\n20\r\n{:.6}\r\n",
        b.block_name, b.insert_point.x, b.insert_point.y
    )
}

/// A copy of the UTM bridge 6 m to +Y whose products carry other GlobalIds, with the same
/// expressIds (two decks from one template).
fn right_ifc() -> String {
    utm_ifc()
        .replace(
            "#101=IFCCARTESIANPOINT((540000.,4700000.,0.));",
            "#101=IFCCARTESIANPOINT((540000.,4700006.,0.));",
        )
        .replace(
            "#201=IFCCARTESIANPOINT((540000.,4700000.01,0.01));",
            "#201=IFCCARTESIANPOINT((540000.,4700006.01,0.01));",
        )
        .replace(GUID_A, GUID_RIGHT)
        .replace("3P3Gs$u5Y8Ag9OPfx4GMPI", "3Right0000000000000000")
}

/// One section holding the bars of both decks, exported from the session models `left` and
/// `right`.
fn federated_sheet(left: &str, right: &str) -> cad_slicer::ClashSectionSheet {
    let mut sheet = build_clash_section_sheet(
        &[
            DrawingMesh::new(0, 100, "IfcReinforcingBar", eval_mesh(left.as_bytes(), 100)),
            DrawingMesh::new(0, 200, "IfcTendon", eval_mesh(left.as_bytes(), 200)),
            DrawingMesh::new(1, 100, "IfcReinforcingBar", eval_mesh(right.as_bytes(), 100)),
            DrawingMesh::new(1, 200, "IfcTendon", eval_mesh(right.as_bytes(), 200)),
        ],
        Cut::Plane {
            origin: DVec3::new(540_000.99, 4_700_000.0, 0.0),
            normal: DVec3::X,
        },
        0.02,
        &[(0, 100, GUID_A.to_string()), (1, 100, GUID_RIGHT.to_string())],
        &[(0, 200), (1, 200)],
        &[
            DVec3::new(540_001.0, 4_700_000.025, 0.025),
            DVec3::new(540_001.0, 4_700_006.025, 0.025),
        ],
        Some(DVec3::new(540_001.0, 4_700_003.0, 0.0)),
        "left",
        1234.5,
        "1+234.50",
        540_000.99,
        540_001.01,
        vec!["PK 1+234.50".to_string()],
    );
    for block in &mut sheet.blocks_a {
        block.owner = if block.model_id == 0 { "left" } else { "right" }.to_string();
    }
    sheet
}

#[test]
fn f5_federated_section_routes_each_edit_to_its_own_model() {
    let (left, right) = (utm_ifc(), right_ifc());
    let sheet = federated_sheet(&left, &right);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let block_of = |m: u32| {
        sheet
            .blocks_a
            .iter()
            .find(|b| b.model_id == m)
            .unwrap()
            .block_name
            .clone()
    };
    let (left_block, right_block) = (block_of(0), block_of(1));
    assert_ne!(left_block, right_block);
    let up = sheet.meta.frame.up;
    let shift = |before: &str, after: &[u8]| {
        bounds(&eval_mesh(after, 100)).0 - bounds(&eval_mesh(before.as_bytes(), 100)).0
    };
    let open = ["left", "right"];

    // Same session, only the left bar moved: it lands on the left deck, the right deck is
    // untouched.
    let edited = move_block(&dxf, &left_block, 0.0, 0.3);
    let (patched_left, json) = apply_as(left.as_bytes(), &edited, "left", &open, &[]);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((shift(&left, &patched_left) - up * 0.3).length() < 1e-6);
    let (patched_right, json) = apply_as(right.as_bytes(), &edited, "right", &open, &[]);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert_eq!(patched_right, right.as_bytes());

    // A later session (other model ids), both bars moved by different amounts: each bar
    // moves by its own amount, on its own deck, never 6 m sideways onto the other.
    let both = move_block(&move_block(&dxf, &left_block, 0.0, 0.3), &right_block, 0.0, 0.1);
    let later = ["s2-a", "s2-b"];
    let (patched_left, json_left) = apply_as(left.as_bytes(), &both, "s2-a", &later, &[]);
    assert_eq!(json_field(&json_left, "applied"), "1", "{json_left}");
    assert!(json_left.contains("guid-mismatch"), "{json_left}");
    assert!((shift(&left, &patched_left) - up * 0.3).length() < 1e-6);
    let exclude = applied_indices(&json_left);
    let (patched_right, json_right) = apply_as(right.as_bytes(), &both, "s2-b", &later, &exclude);
    assert_eq!(json_field(&json_right, "applied"), "1", "{json_right}");
    assert!((shift(&right, &patched_right) - up * 0.1).length() < 1e-6);

    // Two copies that kept the same GlobalIds: only the session's model ids tell them apart.
    let twin = right.replace(GUID_RIGHT, GUID_A);
    let edited = move_block(&dxf, &left_block, 0.0, 0.3);
    let (patched_twin, json) = apply_as(twin.as_bytes(), &edited, "right", &open, &[]);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("other-model"), "{json}");
    assert_eq!(patched_twin, twin.as_bytes());
}

#[test]
fn f5_malformed_transformed_and_copied_inserts_are_refused() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let needle = insert_needle(&sheet);
    assert!(dxf.contains(&needle));
    let b = &sheet.blocks_a[0];
    let x = format!("{:.6}", b.insert_point.x);
    let y = format!("{:.6}", b.insert_point.y);
    let with_insert = |entity: String| dxf.replacen(&needle, &entity, 1);
    let cases = [
        // Comma decimal written by a localized CAD tool.
        ("invalid-number", with_insert(needle.replace(&format!("10\r\n{x}"), &format!("10\r\n{}", x.replace('.', ","))))),
        // Blank coordinate.
        ("invalid-number", with_insert(needle.replace(&format!("20\r\n{y}"), "20\r\n"))),
        // CAD COPY: the original untouched plus a copy 2 m away.
        ("duplicate-insert", with_insert(format!("{needle}{}", move_block(&needle, &b.block_name, 2.0, 0.0)))),
        // ROTATE and MIRROR are not translations.
        ("unsupported-transform", with_insert(format!("{needle}50\r\n90.0\r\n"))),
        ("unsupported-transform", with_insert(format!("{needle}41\r\n-1.0\r\n"))),
    ];
    for (reason, edited) in cases {
        let (patched, json) = apply_plain(ifc.as_bytes(), &edited);
        assert_eq!(json_field(&json, "applied"), "0", "{reason}: {json}");
        assert!(json.contains(&format!("\"reason\":\"{reason}\"")), "{reason}: {json}");
        assert_eq!(patched, ifc.as_bytes(), "{reason}: the IFC is unchanged");
    }
}

#[test]
fn f5_zero_length_polyline_and_huge_moves_keep_the_geometry() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let ins = sheet.blocks_a[0].insert_point;

    let point = add_edit_polyline(&dxf, 100, &[(ins.x, ins.y + 0.1), (ins.x, ins.y + 0.1)]);
    let (patched, json) = apply_plain(ifc.as_bytes(), &point);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"reason\":\"invalid-points\""), "{json}");
    assert!(!eval_mesh(&patched, 100).triangles.is_empty());

    let huge = move_insert(&dxf, 100, 0.0, 1e300);
    let (patched, json) = apply_plain(ifc.as_bytes(), &huge);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"reason\":\"out-of-range\""), "{json}");
    assert_eq!(patched, ifc.as_bytes());
}

#[test]
fn f5_putting_an_element_back_reverts_the_earlier_import() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let (orig_lo, orig_hi) = bounds(&eval_mesh(ifc.as_bytes(), 100));

    // Move, then re-import the export as it was: the bar goes back.
    let (moved, json) = apply_plain(ifc.as_bytes(), &move_insert(&dxf, 100, 0.0, 0.5));
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (back, json) = apply_plain(&moved, &dxf);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (lo, hi) = bounds(&eval_mesh(&back, 100));
    assert!((lo - orig_lo).length() < 1e-6 && (hi - orig_hi).length() < 1e-6);
    let (_, json) = apply_plain(&back, &dxf);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"edits\":[]"), "nothing left to do: {json}");

    // Rebend, then re-import the export: the bar is straight on the exported centreline.
    let ins = sheet.blocks_a[0].insert_point;
    let v = add_edit_polyline(
        &dxf,
        100,
        &[(ins.x - 0.04, ins.y), (ins.x, ins.y + 0.1), (ins.x + 0.04, ins.y)],
    );
    let (bent, json) = apply_plain(ifc.as_bytes(), &v);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let up = sheet.meta.frame.up;
    let extent = |bytes: &[u8]| {
        let (lo, hi) = bounds(&eval_mesh(bytes, 100));
        ((hi - lo).dot(up), (lo + hi).dot(up) * 0.5)
    };
    assert!(extent(&bent).0 > 0.1);
    let (straight, json) = apply_plain(&bent, &dxf);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (height, centre) = extent(&straight);
    assert!(height < 0.05, "straight again, {height} m high");
    assert!((centre - (ins.y + sheet.meta.norm_offset.y)).abs() < 1e-4, "centre {centre}");
}

#[test]
fn f5_rebend_then_move_in_a_later_import_ends_where_one_import_does() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let ins = sheet.blocks_a[0].insert_point;
    let y = ins.y + 0.3;
    let v = |dxf: &str| {
        add_edit_polyline(dxf, 100, &[(ins.x - 0.05, y), (ins.x, y + 0.05), (ins.x + 0.05, y)])
    };
    let full = v(&move_insert(&dxf, 100, 0.0, 0.3));

    let (one, json) = apply_plain(ifc.as_bytes(), &full);
    assert_eq!(json_field(&json, "applied"), "2", "{json}");
    let (step1, json) = apply_plain(ifc.as_bytes(), &v(&dxf));
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (two, json) = apply_plain(&step1, &full);
    assert_eq!(json_field(&json, "applied"), "2", "move, and the rebend written again: {json}");

    let (lo1, hi1) = bounds(&eval_mesh(&one, 100));
    let (lo2, hi2) = bounds(&eval_mesh(&two, 100));
    assert!(
        (lo1 - lo2).length() < 1e-6 && (hi1 - hi2).length() < 1e-6,
        "one import {lo1:?}..{hi1:?}, two imports {lo2:?}..{hi2:?}"
    );
    let (_, json) = apply_plain(&two, &full);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
}

#[test]
fn f5_two_differing_edit_polylines_are_refused() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let ins = sheet.blocks_a[0].insert_point;
    let a = add_edit_polyline(&dxf, 100, &[(ins.x - 0.04, ins.y + 0.1), (ins.x + 0.04, ins.y + 0.1)]);
    let both = add_edit_polyline(&a, 100, &[(ins.x - 0.04, ins.y + 0.2), (ins.x + 0.04, ins.y + 0.2)]);
    let (patched, json) = apply_plain(ifc.as_bytes(), &both);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"reason\":\"duplicate-polyline\""), "{json}");
    assert_eq!(patched, ifc.as_bytes());
}

// ---------------------------------------------------------------------------------------------
// F7: one element edited in two sections of the same export.
// ---------------------------------------------------------------------------------------------

/// The UTM section of [`utm_sheet`], cut at `x` (world) and stamped with an export time.
fn utm_sheet_at(ifc: &[u8], x: f64, pk: f64, exported_at_ms: f64) -> String {
    let mesh_a = eval_mesh(ifc, 100);
    let mesh_b = eval_mesh(ifc, 200);
    let mut sheet = build_clash_section_sheet(
        &[
            DrawingMesh::new(0, 100, "IfcReinforcingBar", mesh_a),
            DrawingMesh::new(0, 200, "IfcTendon", mesh_b),
        ],
        Cut::Plane {
            origin: DVec3::new(x, 4_700_000.0, 0.0),
            normal: DVec3::X,
        },
        0.02,
        &[(0, 100, GUID_A.to_string())],
        &[(0, 200)],
        &[DVec3::new(x, 4_700_000.025, 0.025)],
        Some(DVec3::new(x, 4_700_000.0, 0.0)),
        LONG_MODEL_ID,
        pk,
        "pk",
        x - 0.01,
        x + 0.01,
        vec![],
    );
    sheet.meta.exported_at_ms = Some(exported_at_ms);
    String::from_utf8(to_clash_dxf(&sheet)).unwrap()
}

fn apply_policy(ifc: &[u8], dxf: &str, policy: &str, now_ms: f64) -> (Vec<u8>, String) {
    let scope = wasm_api::clash_dxf::ApplyScope {
        conflict: wasm_api::clash_dxf::ConflictPolicy::parse(policy),
        now_ms: Some(now_ms),
        ..Default::default()
    };
    wasm_api::clash_dxf::apply_clash_dxf_to_step_scoped(ifc, 1.0, dxf.as_bytes(), &scope)
        .expect("apply")
}

fn centre_z(ifc: &[u8]) -> f64 {
    let (lo, hi) = bounds(&eval_mesh(ifc, 100));
    (lo.z + hi.z) * 0.5
}

#[test]
fn f7_two_sections_editing_one_element_ask_before_adding_up() {
    let ifc = utm_ifc();
    let z0 = centre_z(ifc.as_bytes());
    // One export, two sections through bar #100.
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    assert!(a.contains(" t=1000"), "export time in GUBIM_META");
    let a_up = move_insert(&a, 100, 0.0, 0.1);
    let b_up = move_insert(&b, 100, 0.0, 0.2);

    let (after_a, json) = apply_policy(ifc.as_bytes(), &a_up, "ask", 2000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&after_a) - (z0 + 0.1)).abs() < 1e-6);

    // Section B did not show A's move: asked, or skipped, nothing changes.
    for policy in ["ask", "skip"] {
        let (out, json) = apply_policy(&after_a, &b_up, policy, 3000.0);
        assert_eq!(json_field(&json, "applied"), "0", "{policy}: {json}");
        assert!(json.contains("\"reason\":\"edit-conflict\""), "{policy}: {json}");
        assert!((centre_z(&out) - (z0 + 0.1)).abs() < 1e-6, "{policy}");
    }
    // Add: both edits (the behaviour before F7).
    let (added, json) = apply_policy(&after_a, &b_up, "add", 3000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&added) - (z0 + 0.3)).abs() < 1e-6);
    // Replace: B's drawing wins, A's move is undone.
    let (replaced, json) = apply_policy(&after_a, &b_up, "replace", 3000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&replaced) - (z0 + 0.2)).abs() < 1e-6, "{}", centre_z(&replaced));
    // Re-importing B changes nothing; re-importing A asks again (B's edit is newer than A's
    // export), and replacing puts A's drawing back.
    let (_, json) = apply_policy(&replaced, &b_up, "ask", 4000.0);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(!json.contains("edit-conflict"), "{json}");
    let (_, json) = apply_policy(&replaced, &a_up, "ask", 4000.0);
    assert!(json.contains("\"reason\":\"edit-conflict\""), "{json}");
    let (back_to_a, json) = apply_policy(&replaced, &a_up, "replace", 4000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&back_to_a) - (z0 + 0.1)).abs() < 1e-6, "{}", centre_z(&back_to_a));

    // A section exported after A's import shows the move: no question, it adds up.
    let b_later = utm_sheet_at(&after_a, 540_001.5, 1235.0, 2500.0);
    let (later, json) = apply_policy(&after_a, &move_insert(&b_later, 100, 0.0, 0.2), "ask", 3000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&later) - (z0 + 0.3)).abs() < 1e-6);

    // A's move put back (A re-imported as exported) is no edit to ask about.
    let (reverted, json) = apply_policy(&after_a, &a, "ask", 2500.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (b_only, json) = apply_policy(&reverted, &b_up, "ask", 3000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&b_only) - (z0 + 0.2)).abs() < 1e-6);

    // An untouched element in section B is not a conflict: B leaves A's move alone.
    let (same, json) = apply_policy(&after_a, &b, "ask", 3000.0);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"edits\":[]"), "{json}");
    assert!((centre_z(&same) - (z0 + 0.1)).abs() < 1e-6);
}

#[test]
fn f7_rebends_from_two_sections_ask_and_replace_keeps_the_later_drawing() {
    let ifc = utm_ifc();
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    let insert = |dxf: &str| {
        let diff = parse_clash_dxf_edits(dxf.as_bytes()).unwrap();
        diff.unchanged.iter().chain(&diff.edits).map(|e| e.orig_pos()).next().unwrap()
    };
    let v = |dxf: &str, rise: f64| {
        let p = insert(dxf);
        add_edit_polyline(dxf, 100, &[(p.x - 0.04, p.y), (p.x, p.y + rise), (p.x + 0.04, p.y)])
    };
    let height = |bytes: &[u8]| {
        let (lo, hi) = bounds(&eval_mesh(bytes, 100));
        hi.z - lo.z
    };
    let (bent_a, json) = apply_policy(ifc.as_bytes(), &v(&a, 0.1), "ask", 2000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    let (_, json) = apply_policy(&bent_a, &v(&b, 0.3), "ask", 3000.0);
    assert!(json.contains("\"reason\":\"edit-conflict\""), "{json}");
    let (bent_b, json) = apply_policy(&bent_a, &v(&b, 0.3), "replace", 3000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((height(&bent_b) - height(&apply_policy(ifc.as_bytes(), &v(&b, 0.3), "ask", 0.0).0)).abs() < 1e-6);
    // A's rebend was replaced (its note reset): importing B's drawing again changes nothing
    // and does not ask about A's.
    let (_, json) = apply_policy(&bent_b, &v(&b, 0.3), "ask", 4000.0);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(!json.contains("edit-conflict"), "{json}");
}

// ---------------------------------------------------------------------------------------------
// Third F5 review regression tests (`test/e2e/review-f5/REPORT.md`, T-F5R3).
// ---------------------------------------------------------------------------------------------

#[test]
fn f5_reverting_rebend_while_moving_insert_lands_at_moved_position() {
    let ifc = utm_ifc();
    let sheet = utm_sheet(&ifc);
    let dxf = String::from_utf8(to_clash_dxf(&sheet)).unwrap();
    let z0 = centre_z(ifc.as_bytes());
    let ins = sheet.blocks_a[0].insert_point;

    // 1. Rebend #100 into a V.
    let v_dxf = add_edit_polyline(
        &dxf,
        100,
        &[(ins.x - 0.04, ins.y), (ins.x, ins.y + 0.15), (ins.x + 0.04, ins.y)],
    );
    let (bent, json) = apply_plain(ifc.as_bytes(), &v_dxf);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");

    // 2. Revert EDIT_A_100 to the exported straight polyline while moving INSERT +0.30 m up.
    let straight_moved = move_insert(&dxf, 100, 0.0, 0.30);
    let (moved_straight, json) = apply_plain(&bent, &straight_moved);
    assert_eq!(json_field(&json, "applied"), "2", "{json}");
    let (lo, hi) = bounds(&eval_mesh(&moved_straight, 100));
    assert!(
        (hi.z - lo.z) < 0.05 && ((lo.z + hi.z) * 0.5 - (z0 + 0.30)).abs() < 1e-4,
        "bar must be straight at z0 + 0.30 ({}), got {lo:?}..{hi:?}",
        z0 + 0.30
    );

    // 3. Re-importing the same straight+move DXF is idempotent (`already-applied`).
    let (again, json) = apply_plain(&moved_straight, &straight_moved);
    assert_eq!(json_field(&json, "applied"), "0", "{json}");
    assert!(json.contains("\"reason\":\"already-applied\""), "{json}");
    assert_eq!(again, moved_straight);
}

#[test]
fn f7_foreign_global_id_on_patched_express_id_reports_guid_mismatch_not_conflict() {
    let ifc = utm_ifc();
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    let a_up = move_insert(&a, 100, 0.0, 0.10);
    let (after_a, json) = apply_policy(ifc.as_bytes(), &a_up, "ask", 2000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");

    // Another model's edit on #100 (different GlobalId) in section B must report `guid-mismatch`,
    // never a false F7 `edit-conflict` from this model's own note on #100.
    let b_foreign = move_insert(&b, 100, 0.0, 0.20)
        .replace(GUID_A, GUID_RIGHT)
        .replace(&GUID_A[..17], &GUID_RIGHT[..17]);
    let (out_b, json_b) = apply_policy(&after_a, &b_foreign, "ask", 3000.0);
    assert_eq!(json_field(&json_b, "applied"), "0", "{json_b}");
    assert!(json_b.contains("\"reason\":\"guid-mismatch\""), "{json_b}");
    assert!(!json_b.contains("edit-conflict"), "{json_b}");
    assert_eq!(out_b, after_a);

    // Likewise, tampering the GlobalId on an already-applied section A DXF must report
    // `guid-mismatch`, never `already-applied`.
    let a_foreign = a_up
        .replace(GUID_A, GUID_RIGHT)
        .replace(&GUID_A[..17], &GUID_RIGHT[..17]);
    let (out_a, json_a) = apply_policy(&after_a, &a_foreign, "ask", 3000.0);
    assert_eq!(json_field(&json_a, "applied"), "0", "{json_a}");
    assert!(json_a.contains("\"reason\":\"guid-mismatch\""), "{json_a}");
    assert!(!json_a.contains("already-applied"), "{json_a}");
    assert_eq!(out_a, after_a);
}

#[test]
fn f7_replace_with_equal_displacement_records_note_and_drops_earlier_section() {
    let ifc = utm_ifc();
    let z0 = centre_z(ifc.as_bytes());
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    let a_010 = move_insert(&a, 100, 0.0, 0.10);
    let b_010 = move_insert(&b, 100, 0.0, 0.10);

    let (after_a, json) = apply_policy(ifc.as_bytes(), &a_010, "ask", 2000.0);
    assert_eq!(json_field(&json, "applied"), "1", "{json}");
    assert!((centre_z(&after_a) - (z0 + 0.10)).abs() < 1e-6);

    // Section B also drew +0.10: `ask` warns (summing would land at +0.20), and `replace`
    // adopts section B (`applied == 1`, z stays z0 + 0.10, A's note is dropped).
    let (_, json_ask) = apply_policy(&after_a, &b_010, "ask", 3000.0);
    assert!(json_ask.contains("\"reason\":\"edit-conflict\""), "{json_ask}");

    let (after_b, json_rep) = apply_policy(&after_a, &b_010, "replace", 3000.0);
    assert_eq!(json_field(&json_rep, "applied"), "1", "{json_rep}");
    assert!((centre_z(&after_b) - (z0 + 0.10)).abs() < 1e-6);

    // Re-importing B now reports `already-applied` without asking again, and a further edit
    // in B (+0.20) applies only the +0.10 difference without conflict.
    let (_, json_idem) = apply_policy(&after_b, &b_010, "ask", 4000.0);
    assert_eq!(json_field(&json_idem, "applied"), "0", "{json_idem}");
    assert!(json_idem.contains("\"reason\":\"already-applied\""), "{json_idem}");
    assert!(!json_idem.contains("edit-conflict"), "{json_idem}");

    let b_020 = move_insert(&b, 100, 0.0, 0.20);
    let (after_b2, json_b2) = apply_policy(&after_b, &b_020, "ask", 5000.0);
    assert_eq!(json_field(&json_b2, "applied"), "1", "{json_b2}");
    assert!((centre_z(&after_b2) - (z0 + 0.20)).abs() < 1e-6);
}

#[test]
fn f7_reimporting_earlier_section_after_add_warns_and_replace_undoes_later_sum() {
    let ifc = utm_ifc();
    let z0 = centre_z(ifc.as_bytes());
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    let a_030 = move_insert(&a, 100, 0.0, 0.30);
    let b_010 = move_insert(&b, 100, 0.0, 0.10);

    let (after_a, _) = apply_policy(ifc.as_bytes(), &a_030, "ask", 2000.0);
    let (summed, json_add) = apply_policy(&after_a, &b_010, "add", 3000.0);
    assert_eq!(json_field(&json_add, "applied"), "1", "{json_add}");
    assert!((centre_z(&summed) - (z0 + 0.40)).abs() < 1e-6);

    // Re-importing the just-summed section B is `already-applied`.
    let (_, json_b_idem) = apply_policy(&summed, &b_010, "ask", 4000.0);
    assert!(json_b_idem.contains("\"reason\":\"already-applied\""), "{json_b_idem}");
    assert!(!json_b_idem.contains("edit-conflict"), "{json_b_idem}");

    // Re-importing the earlier section A (+0.30) must warn because B edited #100 after A's
    // import, and `replace` restores #100 to A's +0.30.
    let (_, json_a_ask) = apply_policy(&summed, &a_030, "ask", 4000.0);
    assert!(json_a_ask.contains("\"reason\":\"edit-conflict\""), "{json_a_ask}");

    let (_, json_a_add) = apply_policy(&summed, &a_030, "add", 4000.0);
    assert!(json_a_add.contains("\"reason\":\"already-applied\""), "{json_a_add}");

    let (restored, json_a_rep) = apply_policy(&summed, &a_030, "replace", 4000.0);
    assert_eq!(json_field(&json_a_rep, "applied"), "1", "{json_a_rep}");
    assert!((centre_z(&restored) - (z0 + 0.30)).abs() < 1e-6);
}

#[test]
fn f7_cross_section_move_and_rebend_commute_on_add_and_restore_on_replace() {
    let ifc = utm_ifc();
    let a = utm_sheet_at(ifc.as_bytes(), 540_000.5, 1234.0, 1000.0);
    let b = utm_sheet_at(ifc.as_bytes(), 540_001.5, 1235.0, 1000.0);
    let orig_pos = |dxf: &str| {
        let diff = parse_clash_dxf_edits(dxf.as_bytes()).unwrap();
        diff.unchanged
            .iter()
            .chain(&diff.edits)
            .map(|e| e.orig_pos())
            .next()
            .unwrap()
    };
    let v = |dxf: &str, rise: f64| {
        let p = orig_pos(dxf);
        add_edit_polyline(dxf, 100, &[(p.x - 0.04, p.y), (p.x, p.y + rise), (p.x + 0.04, p.y)])
    };

    // 1. Move in A (+0.20) then Rebend in B (V, rise +0.15) with `Add` must equal Rebend in A
    //    then Move in B (+0.20) with `Add` (`V + 0.20`).
    let a_move = move_insert(&a, 100, 0.0, 0.20);
    let b_rebend = v(&b, 0.15);
    let (m_first, _) = apply_policy(ifc.as_bytes(), &a_move, "ask", 2000.0);
    let (_, ask_1) = apply_policy(&m_first, &b_rebend, "ask", 3000.0);
    assert!(ask_1.contains("\"reason\":\"edit-conflict\""), "{ask_1}");
    let (sum_mr, json_mr) = apply_policy(&m_first, &b_rebend, "add", 3000.0);
    assert_eq!(json_field(&json_mr, "applied"), "1", "{json_mr}");

    let a_rebend = v(&a, 0.15);
    let b_move = move_insert(&b, 100, 0.0, 0.20);
    let (r_first, _) = apply_policy(ifc.as_bytes(), &a_rebend, "ask", 2000.0);
    let (sum_rm, json_rm) = apply_policy(&r_first, &b_move, "add", 3000.0);
    assert_eq!(json_field(&json_rm, "applied"), "1", "{json_rm}");

    let (v_only, _) = apply_policy(ifc.as_bytes(), &b_rebend, "ask", 2000.0);
    let (lo_v, hi_v) = bounds(&eval_mesh(&v_only, 100));
    let shift_z = glam::DVec3::new(0.0, 0.0, 0.20);
    let (lo_mr, hi_mr) = bounds(&eval_mesh(&sum_mr, 100));
    let (lo_rm, hi_rm) = bounds(&eval_mesh(&sum_rm, 100));
    assert!(
        (lo_mr.z - lo_rm.z).abs() < 1e-6 && (hi_mr.z - hi_rm.z).abs() < 1e-6,
        "Add must commute on Z: move->rebend {lo_mr:?}..{hi_mr:?} vs rebend->move {lo_rm:?}..{hi_rm:?}"
    );
    assert!(
        (lo_mr - (lo_v + shift_z)).length() < 1e-6 && (hi_mr - (hi_v + shift_z)).length() < 1e-6,
        "V + 0.20 must shift pure V ({lo_v:?}..{hi_v:?}) by +0.20 Z, got {lo_mr:?}..{hi_mr:?}"
    );

    // 2. Rebend in A (V), then Move in B (+0.20) where B carries NO EDIT_A_100 polyline
    //    (simulating a longitudinal cut or an EDIT_A polyline deleted in CAD):
    //    `Replace` must restore the original straight bar representation shifted to z0 + 0.20.
    let (orig_lo, orig_hi) = bounds(&eval_mesh(ifc.as_bytes(), 100));
    let b_move_no_poly = b_move.replace("\r\n8\r\nEDIT_A_100", "\r\n8\r\nREMOVED_POLY_100");
    let (_, ask_2) = apply_policy(&r_first, &b_move_no_poly, "ask", 3000.0);
    assert!(ask_2.contains("\"reason\":\"edit-conflict\""), "{ask_2}");
    let (rep_straight, json_rep) = apply_policy(&r_first, &b_move_no_poly, "replace", 3000.0);
    assert_eq!(json_field(&json_rep, "applied"), "1", "{json_rep}");
    let (lo_s, hi_s) = bounds(&eval_mesh(&rep_straight, 100));
    assert!(
        (lo_s - (orig_lo + shift_z)).length() < 1e-6 && (hi_s - (orig_hi + shift_z)).length() < 1e-6,
        "Replace without EDIT_A must restore original straight bar shifted by +0.20 Z ({:?}..{:?}), got {lo_s:?}..{hi_s:?}",
        orig_lo + shift_z,
        orig_hi + shift_z
    );
    // Re-importing B's move is now idempotent without conflict.
    let (_, json_idem) = apply_policy(&rep_straight, &b_move_no_poly, "ask", 4000.0);
    assert!(json_idem.contains("\"reason\":\"already-applied\""), "{json_idem}");
    assert!(!json_idem.contains("edit-conflict"), "{json_idem}");
}
