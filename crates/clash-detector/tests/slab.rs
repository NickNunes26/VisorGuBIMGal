use clash_detector::{
    ClashMesh, ClashOutcome, ClashSlabInput, DEFAULT_MERGE_TOLERANCE_M, ElementOrientation,
    SlabConfig, compute_element_a_slab, group_clash_slabs, test_pair,
};
use glam::{DMat3, DVec3};
use ifc_geometry::Mesh;

/// Builds a closed wedge/tapered bar along X `[x0, x1]`, Y `[-0.02, 0.02]`,
/// whose bottom Z slopes linearly from `z_bot0` at `x0` down to `z_bot1` at `x1`,
/// with flat top at `z_top`.
fn tapered_bar_mesh(x0: f64, x1: f64, z_bot0: f64, z_bot1: f64, z_top: f64) -> Mesh {
    let y0 = -0.02;
    let y1 = 0.02;
    let positions = vec![
        DVec3::new(x0, y0, z_bot0), // 0
        DVec3::new(x1, y0, z_bot1), // 1
        DVec3::new(x1, y1, z_bot1), // 2
        DVec3::new(x0, y1, z_bot0), // 3
        DVec3::new(x0, y0, z_top),  // 4
        DVec3::new(x1, y0, z_top),  // 5
        DVec3::new(x1, y1, z_top),  // 6
        DVec3::new(x0, y1, z_top),  // 7
    ];
    let triangles = vec![
        // Bottom (-Z)
        [0, 3, 2],
        [0, 2, 1],
        // Top (+Z)
        [4, 5, 6],
        [4, 6, 7],
        // Front (-Y)
        [0, 1, 5],
        [0, 5, 4],
        // Back (+Y)
        [3, 7, 6],
        [3, 6, 2],
        // Left (-X)
        [0, 4, 7],
        [0, 7, 3],
        // Right (+X)
        [1, 2, 6],
        [1, 6, 5],
    ];
    Mesh {
        positions,
        triangles,
    }
}

#[test]
fn asymmetric_csg_hard_clash_reports_maximum_penetration_point() {
    // Longitudinal concrete beam / wide bar along X = [0, 10], Y = [-0.05, 0.05], Z = [0.0, 0.10].
    let rebar = ClashMesh::new(Mesh::cuboid(
        DVec3::new(0.0, -0.05, 0.0),
        DVec3::new(10.0, 0.05, 0.10),
    ))
    .unwrap();
    // Sloped tendon over X = [2, 8], Y = [-0.02, 0.02]: barely grazes at x=2 (z_bot=0.099 -> 1 mm),
    // dips deepest at x=8 (z_bot=0.070 -> 30 mm overlap).
    let tendon = ClashMesh::new(tapered_bar_mesh(2.0, 8.0, 0.099, 0.070, 0.16)).unwrap();
    assert!(rebar.closed && tendon.closed);

    let outcome = test_pair(&rebar, &tendon, 0.0).expect("asymmetric overlap must clash");
    match outcome {
        ClashOutcome::Hard {
            penetration_m,
            volume_m3,
            point,
        } => {
            assert!(volume_m3.is_some());
            // Deepest section is at x = 8.0 (NOT AABB center x = 5.0).
            assert!(
                point.x > 7.5,
                "expected max-penetration point near x=8.0, got x={}",
                point.x
            );
            assert!(
                (penetration_m - 0.030).abs() < 0.003,
                "expected ~30 mm max penetration, got {penetration_m}"
            );
        }
        other => panic!("expected Hard clash, got {other:?}"),
    }
}

#[test]
fn asymmetric_fallback_hard_clash_reports_maximum_penetration_point() {
    // Same scenario, but drop one end cap of the rebar so CSG is bypassed and the
    // triangle-triangle fallback runs.
    let mut rebar_mesh = Mesh::cuboid(
        DVec3::new(0.0, -0.05, 0.0),
        DVec3::new(10.0, 0.05, 0.10),
    );
    rebar_mesh.triangles.truncate(10); // open shell
    let rebar = ClashMesh::new(rebar_mesh).unwrap();
    assert!(!rebar.closed);

    let tendon = ClashMesh::new(tapered_bar_mesh(2.0, 8.0, 0.099, 0.070, 0.16)).unwrap();
    let outcome = test_pair(&rebar, &tendon, 0.0).expect("open fallback overlap must clash");
    match outcome {
        ClashOutcome::Hard {
            volume_m3, point, ..
        } => {
            assert!(volume_m3.is_none());
            assert!(
                point.x > 7.5,
                "fallback must also pinpoint max penetration near x=8.0, got x={}",
                point.x
            );
        }
        other => panic!("expected Hard clash, got {other:?}"),
    }
}

#[test]
fn transverse_rebar_gives_identical_slab_for_front_middle_and_rear_clashes() {
    // Alignment tangent rotated 30 deg from +X.
    let angle = 30.0_f64.to_radians();
    let rot = DMat3::from_rotation_z(angle);
    let tangent_xy = [angle.cos(), angle.sin()];

    // Transverse stirrup centered at s = 12.0 m along alignment:
    // thickness along alignment = 0.020 m ([11.99, 12.01]), width = 1.2 m, height = 0.8 m.
    let local_bar = Mesh::cuboid(
        DVec3::new(11.99, -0.60, 0.0),
        DVec3::new(12.01, 0.60, 0.80),
    )
    .map_positions(|p| rot * p);

    let cfg = SlabConfig::default();
    let front_pt = rot * DVec3::new(11.99, 0.1, 0.4);
    let mid_pt = rot * DVec3::new(12.00, -0.2, 0.5);
    let rear_pt = rot * DVec3::new(12.01, 0.3, 0.2);

    let slab_front = compute_element_a_slab(&local_bar.positions, tangent_xy, front_pt, &cfg);
    let slab_mid = compute_element_a_slab(&local_bar.positions, tangent_xy, mid_pt, &cfg);
    let slab_rear = compute_element_a_slab(&local_bar.positions, tangent_xy, rear_pt, &cfg);

    for slab in [slab_front, slab_mid, slab_rear] {
        assert_eq!(slab.orientation, ElementOrientation::Transverse);
        assert!((slab.s_min - 11.99).abs() < 1e-9);
        assert!((slab.s_max - 12.01).abs() < 1e-9);
        assert!((slab.depth_m() - 0.02).abs() < 1e-9);
    }
}

#[test]
fn longitudinal_bar_gives_zero_depth_at_max_penetration_point() {
    let tangent_xy = [1.0, 0.0];
    let long_bar = Mesh::cuboid(
        DVec3::new(0.0, -0.012, 0.0),
        DVec3::new(6.0, 0.012, 0.024),
    );
    let cfg = SlabConfig::default();
    let max_pen_pt = DVec3::new(4.75, 0.0, 0.012);

    let slab = compute_element_a_slab(&long_bar.positions, tangent_xy, max_pen_pt, &cfg);
    assert_eq!(slab.orientation, ElementOrientation::Longitudinal);
    assert_eq!(slab.depth_m(), 0.0);
    assert!((slab.s_min - 4.75).abs() < 1e-12);
    assert!((slab.s_max - 4.75).abs() < 1e-12);
}

#[test]
fn oblique_transverse_element_respects_max_slab_safety_cap() {
    let tangent_xy = [1.0, 0.0];
    // delta_s = 0.50 m, w_perp = 1.0 m -> still Transverse (aspect 0.5 <= 2.5), exceeds 0.25 m cap.
    let oblique = Mesh::cuboid(
        DVec3::new(10.0, -0.5, 0.0),
        DVec3::new(10.5, 0.5, 0.5),
    );
    let cfg = SlabConfig::default();
    let slab = compute_element_a_slab(&oblique.positions, tangent_xy, DVec3::new(10.45, 0.0, 0.2), &cfg);
    assert_eq!(slab.orientation, ElementOrientation::Transverse);
    assert!((slab.depth_m() - 0.25).abs() < 1e-12);
    assert!(slab.s_min >= 10.0 - 1e-12 && slab.s_max <= 10.5 + 1e-12);
}

#[test]
fn pk_grouping_merges_within_20mm_and_handles_transverse_longitudinal_and_zero_tolerance() {
    let cfg = SlabConfig::default();
    let t = [1.0, 0.0];

    let bar1 = Mesh::cuboid(DVec3::new(10.000, -0.5, 0.0), DVec3::new(10.016, 0.5, 0.5));
    let bar2 = Mesh::cuboid(DVec3::new(10.010, -0.5, 0.0), DVec3::new(10.026, 0.5, 0.5));
    let long_bar = Mesh::cuboid(DVec3::new(0.0, -0.01, 0.0), DVec3::new(20.0, 0.01, 0.02));

    let c0 = ClashSlabInput {
        clash_index: 0,
        pk_m: 10.008,
        penetration_m: 0.010,
        gap_m: 0.0,
        point: DVec3::new(10.008, 0.0, 0.2),
        tangent_xy: t,
        slab: compute_element_a_slab(&bar1.positions, t, DVec3::new(10.008, 0.0, 0.2), &cfg),
    };
    let c1 = ClashSlabInput {
        clash_index: 1,
        pk_m: 10.020, // 12 mm from c0 -> merges with c0!
        penetration_m: 0.015,
        gap_m: 0.0,
        point: DVec3::new(10.020, 0.1, 0.3),
        tangent_xy: t,
        slab: compute_element_a_slab(&bar2.positions, t, DVec3::new(10.020, 0.1, 0.3), &cfg),
    };
    // Two longitudinal clashes 15 mm apart far away at PK ~ 15 m.
    let c2 = ClashSlabInput {
        clash_index: 2,
        pk_m: 15.000,
        penetration_m: 0.008,
        gap_m: 0.0,
        point: DVec3::new(15.000, 0.0, 0.01),
        tangent_xy: t,
        slab: compute_element_a_slab(&long_bar.positions, t, DVec3::new(15.000, 0.0, 0.01), &cfg),
    };
    let c3 = ClashSlabInput {
        clash_index: 3,
        pk_m: 15.015, // 15 mm from c2, higher penetration -> wins anchor
        penetration_m: 0.025,
        gap_m: 0.0,
        point: DVec3::new(15.015, 0.0, 0.01),
        tangent_xy: t,
        slab: compute_element_a_slab(&long_bar.positions, t, DVec3::new(15.015, 0.0, 0.01), &cfg),
    };

    // Unsorted input order to verify PK sorting.
    let groups = group_clash_slabs(&[c3, c1, c2, c0], DEFAULT_MERGE_TOLERANCE_M);
    assert_eq!(groups.len(), 2);

    // Group 0: merged transverse bars -> union [10.000, 10.026].
    assert_eq!(groups[0].orientation, ElementOrientation::Transverse);
    assert_eq!(groups[0].clash_indices, vec![0, 1]);
    assert!((groups[0].s_min - 10.000).abs() < 1e-9);
    assert!((groups[0].s_max - 10.026).abs() < 1e-9);

    // Group 1: merged longitudinal bars -> depth = 0 at higher-penetration clash c3 (15.015).
    assert_eq!(groups[1].orientation, ElementOrientation::Longitudinal);
    assert_eq!(groups[1].clash_indices, vec![2, 3]);
    assert_eq!(groups[1].depth_m(), 0.0);
    assert!((groups[1].s_min - 15.015).abs() < 1e-9);
    assert!((groups[1].pk_m - 15.015).abs() < 1e-9);

    // Zero tolerance disables merging completely.
    let unmerged = group_clash_slabs(&[c3, c1, c2, c0], 0.0);
    assert_eq!(unmerged.len(), 4);
}
