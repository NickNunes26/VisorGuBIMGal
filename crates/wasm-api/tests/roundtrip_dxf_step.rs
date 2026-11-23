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
