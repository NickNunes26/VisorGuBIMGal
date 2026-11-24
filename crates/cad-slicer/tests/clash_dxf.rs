use cad_slicer::{
    Cut, DrawingMesh, GridMergeOptions, build_clash_section_sheet, draw_slab_unoccluded,
    merge_drawings_grid, to_clash_dxf,
};
use glam::DVec3;
use ifc_geometry::Mesh;
use std::time::Instant;

fn box_drawing_mesh(id: u32, layer: &str, min: [f64; 3], max: [f64; 3]) -> DrawingMesh {
    DrawingMesh::new(
        0,
        id,
        layer,
        Mesh::cuboid(DVec3::from_array(min), DVec3::from_array(max)),
    )
}

#[test]
fn unoccluded_slab_keeps_elements_behind_concrete_and_zero_depth_emits_cut_only() {
    // Concrete deck slab spanning X = [0, 10], Y = [-3, 3], Z = [0, 1.5].
    let concrete = box_drawing_mesh(1, "IfcBridgePart", [0.0, -3.0, 0.0], [10.0, 3.0, 1.5]);
    // Rebar sitting completely behind the cut plane x = 5.0 (at X = [5.01, 5.03]),
    // completely inside the concrete deck -> hidden if occlusion were on!
    let rebar = box_drawing_mesh(2, "IfcReinforcingBar", [5.01, -1.0, 0.4], [5.03, 1.0, 0.45]);

    let cut = Cut::Plane {
        origin: DVec3::new(5.0, 0.0, 0.0),
        normal: DVec3::X,
    };

    // Depth = 0 -> cut loops only, rebar behind cut plane is not cut.
    let zero_draw = draw_slab_unoccluded(&[concrete.clone(), rebar.clone()], cut, 0.0);
    let rebar_layer_zero = zero_draw
        .layers
        .iter()
        .find(|l| l.name == "IfcReinforcingBar")
        .unwrap();
    assert!(rebar_layer_zero.loops.is_empty());
    assert!(rebar_layer_zero.visible.is_empty());

    // Depth = 0.05 -> rebar inside [5.00, 5.05] IS visible (no occlusion!).
    let slab_draw = draw_slab_unoccluded(&[concrete, rebar], cut, 0.05);
    let rebar_layer_slab = slab_draw
        .layers
        .iter()
        .find(|l| l.name == "IfcReinforcingBar")
        .unwrap();
    assert!(
        !rebar_layer_slab.visible.is_empty(),
        "rebar inside slab behind concrete must be visible in no-occlusion mode"
    );
    assert!(rebar_layer_slab.hidden.is_empty());
}

#[test]
fn clash_dxf_normalizes_to_origin_in_metres_and_writes_layers_blocks_and_metadata() {
    // Coordinates shifted far away from origin (UTM-like local offset) to prove exact (0,0) normalization.
    let concrete = box_drawing_mesh(10, "IfcSlab", [100.0, 50.0, 20.0], [110.0, 56.0, 22.0]);
    let bar_a = box_drawing_mesh(101, "IfcReinforcingBar", [104.99, 51.0, 20.5], [105.01, 55.0, 20.54]);
    let tendon_b = box_drawing_mesh(202, "IfcTendon", [100.0, 52.8, 20.48], [110.0, 53.0, 20.60]);

    let cut = Cut::Plane {
        origin: DVec3::new(104.99, 0.0, 0.0),
        normal: DVec3::X,
    };

    let sheet = build_clash_section_sheet(
        &[concrete, bar_a, tendon_b],
        cut,
        0.02,
        &[(0, 101, "2O2Fr$t4X7Zf8NOew3FLOH".to_string())],
        &[(0, 202)],
        &[DVec3::new(105.0, 52.9, 20.52)],
        Some(DVec3::new(105.0, 53.0, 21.0)),
        "bridge-model",
        124.5,
        "0+124.50",
        104.99,
        105.01,
        vec!["PK 0+124.50 | depth 20 mm | pen 18.0 mm".to_string()],
    );

    // Exact (0, 0) bounding box: width = 6.0 m (Y: 50..56), height = 2.0 m (Z: 20..22).
    assert!((sheet.width_m - 6.0).abs() < 1e-6);
    assert!((sheet.height_m - 2.0).abs() < 1e-6);
    assert_eq!(sheet.blocks_a.len(), 1);
    assert!(sheet.blocks_a[0].block_name.starts_with("GUBIM_A_101_"));
    assert!(sheet.blocks_a[0].block_name.len() <= 31);

    let bytes = to_clash_dxf(&sheet);
    let text = String::from_utf8_lossy(&bytes);

    // Valid R12 header in metres ($INSUNITS = 6) with $EXTMIN = (0, 0).
    assert!(text.contains("$ACADVER\r\n1\r\nAC1009\r\n"));
    assert!(text.contains("$INSUNITS\r\n70\r\n6\r\n"));
    assert!(text.contains("$EXTMIN\r\n10\r\n0.000000\r\n20\r\n0.000000\r\n"));
    assert!(text.contains("999\r\nGUBIM_META v=2 model=bridge-model pk=124.500000"));
    // Every value fits the DXF R12 limit of 255 characters (metadata split over records).
    assert!(text.split("\r\n").all(|line| line.len() <= 255));
    assert!(text.contains("CLASH_A"));
    assert!(text.contains("CLASH_B"));
    assert!(text.contains("CLASH_MARKER"));
    assert!(text.contains("ALIGNMENT_AXIS"));
    assert!(text.contains("ANNOTATION"));
    assert!(text.contains("GUBIM_META"));
    assert!(text.contains("0\r\nBLOCK\r\n"));
    assert!(text.contains("0\r\nINSERT\r\n"));
    assert!(text.ends_with("0\r\nEOF\r\n"));
}

#[test]
fn merge_drawings_grid_packs_50_sections_with_zero_overlap_and_under_50ms_per_section() {
    let mut sheets = Vec::with_capacity(50);
    let t0 = Instant::now();

    for i in 0..50usize {
        let pk = (50 - i) as f64 * 2.5; // intentionally reverse order to verify PK sorting
        let w = 4.0 + (i % 5) as f64 * 1.2;
        let h = 1.5 + (i % 4) as f64 * 0.6;
        let deck = box_drawing_mesh(1, "IfcSlab", [pk - 1.0, 0.0, 0.0], [pk + 1.0, w, h]);
        let bar_a = box_drawing_mesh(100 + i as u32, "IfcReinforcingBar", [pk, 0.5, 0.3], [pk + 0.02, w - 0.5, 0.34]);
        let tendon_b = box_drawing_mesh(200 + i as u32, "IfcTendon", [pk - 1.0, 1.0, 0.31], [pk + 1.0, 1.1, 0.40]);

        let cut = Cut::Plane {
            origin: DVec3::new(pk, 0.0, 0.0),
            normal: DVec3::X,
        };
        sheets.push(build_clash_section_sheet(
            &[deck, bar_a, tendon_b],
            cut,
            0.02,
            &[(0, 100 + i as u32, format!("GUID_{i}"))],
            &[(0, 200 + i as u32)],
            &[DVec3::new(pk + 0.01, 1.05, 0.33)],
            Some(DVec3::new(pk + 0.01, w * 0.5, h * 0.5)),
            "bridge",
            pk,
            format!("0+{pk:06.2}"),
            pk,
            pk + 0.02,
            vec![format!("PK 0+{pk:06.2}")],
        ));
    }

    let slice_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert!(
        slice_ms / 50.0 < 50.0,
        "expected <50 ms/section, got {:.2} ms/section",
        slice_ms / 50.0
    );

    let merged = merge_drawings_grid(&sheets, &GridMergeOptions::default());
    assert_eq!(merged.cells.len(), 50);

    // Strictly ascending PK order.
    for pair in merged.cells.windows(2) {
        assert!(pair[0].pk_m <= pair[1].pk_m);
    }

    // Non-overlap invariant across all 50 * 49 / 2 = 1225 pairs of cell frames!
    for i in 0..merged.cells.len() {
        for j in (i + 1)..merged.cells.len() {
            let a = &merged.cells[i];
            let b = &merged.cells[j];
            let disjoint = a.frame_max.x < b.frame_min.x
                || b.frame_max.x < a.frame_min.x
                || a.frame_max.y < b.frame_min.y
                || b.frame_max.y < a.frame_min.y;
            assert!(
                disjoint,
                "cells {i} ({:?}..{:?}) and {j} ({:?}..{:?}) overlap!",
                a.frame_min, a.frame_max, b.frame_min, b.frame_max
            );
        }
    }

    let text = String::from_utf8_lossy(&merged.dxf_bytes);
    assert!(text.contains("SHEET_FRAME"));
    assert!(text.contains("GUBIM_MASTER_GRID sections=50 units=m"));
    assert!(text.ends_with("0\r\nEOF\r\n"));
}

#[test]
fn federated_models_with_colliding_express_ids_classify_clash_a_and_b_by_model_id() {
    // Model 0 (#100) is CLASH_A; Model 1 (#100) is CLASH_B; Model 2 (#100) is background concrete.
    let bar_a = DrawingMesh::new(
        0,
        100,
        "IfcReinforcingBar",
        Mesh::cuboid(DVec3::new(4.99, 0.0, 0.5), DVec3::new(5.01, 1.0, 0.54)),
    );
    let tendon_b = DrawingMesh::new(
        1,
        100,
        "IfcTendon",
        Mesh::cuboid(DVec3::new(4.0, 0.4, 0.48), DVec3::new(6.0, 0.6, 0.58)),
    );
    let deck_bg = DrawingMesh::new(
        2,
        100,
        "IfcSlab",
        Mesh::cuboid(DVec3::new(0.0, -2.0, 0.0), DVec3::new(10.0, 3.0, 2.0)),
    );

    let cut = Cut::Plane {
        origin: DVec3::new(4.99, 0.0, 0.0),
        normal: DVec3::X,
    };

    let sheet = build_clash_section_sheet(
        &[bar_a, tendon_b, deck_bg],
        cut,
        0.02,
        &[(0, 100, "GUID_MODEL0_100".to_string())],
        &[(1, 100)],
        &[DVec3::new(5.0, 0.5, 0.52)],
        None,
        "model-0",
        5.0,
        "0+005.00",
        4.99,
        5.01,
        vec!["Federated #100 collision".to_string()],
    );

    // Exactly one CLASH_A block from model 0, sized 1.0 x 0.04 m (NOT merged with model 1 or 2).
    assert_eq!(sheet.blocks_a.len(), 1);
    assert_eq!(sheet.blocks_a[0].model_id, 0);
    assert_eq!(sheet.blocks_a[0].express_id, 100);
    // CLASH_B layer exists (from model 1 #100), and IfcSlab background layer exists (from model 2 #100).
    assert!(sheet.drawing.layers.iter().any(|l| l.name == "CLASH_B"));
    assert!(sheet.drawing.layers.iter().any(|l| l.name == "IfcSlab"));
}
