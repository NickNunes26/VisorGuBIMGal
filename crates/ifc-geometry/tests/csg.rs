//! Booleans checked against exact volumes, watertightness and volume conservation.

use glam::{DMat3, DVec3};
use ifc_geometry::{Mesh, intersect, subtract};
use std::time::Instant;

fn v(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

/// A wall 10 m long, 0.3 m thick and 3 m high.
fn wall() -> Mesh {
    Mesh::cuboid(v(0.0, 0.0, 0.0), v(10.0, 0.3, 3.0))
}

#[track_caller]
fn assert_solid(mesh: &Mesh, volume: f64) {
    assert!((mesh.volume() - volume).abs() < 1e-6, "volume {} != {volume}", mesh.volume());
    assert_eq!(mesh.open_edges(), 0, "not watertight");
}

#[test]
fn opening_flush_with_both_wall_faces() {
    let cut = Mesh::cuboid(v(4.0, 0.0, 1.0), v(5.0, 0.3, 2.0));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0 - 0.3);
}

#[test]
fn opening_overshooting_the_wall() {
    let cut = Mesh::cuboid(v(4.0, -0.1, 1.0), v(5.0, 0.4, 2.0));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0 - 0.3);
}

#[test]
fn recess_that_does_not_pass_through() {
    let cut = Mesh::cuboid(v(4.0, 0.2, 1.0), v(5.0, 0.4, 2.0));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0 - 0.1);
}

#[test]
fn door_sharing_the_floor_plane() {
    let cut = Mesh::cuboid(v(4.0, 0.0, 0.0), v(5.0, 0.3, 2.1));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0 - 0.3 * 2.1);
}

#[test]
fn opening_at_a_corner_of_the_wall() {
    let cut = Mesh::cuboid(v(0.0, 0.0, 0.0), v(1.0, 0.3, 1.0));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0 - 0.3);
}

#[test]
fn opening_outside_the_solid_changes_nothing() {
    let cut = Mesh::cuboid(v(20.0, 0.0, 0.0), v(21.0, 1.0, 1.0));
    assert_solid(&subtract(&wall(), &cut).unwrap(), 9.0);
}

#[test]
fn opening_swallowing_the_solid_leaves_nothing() {
    let cut = Mesh::cuboid(v(-1.0, -1.0, -1.0), v(11.0, 1.0, 4.0));
    let result = subtract(&wall(), &cut).unwrap();
    assert!(result.is_empty(), "{} triangles left", result.triangles.len());
}

#[test]
fn identical_solids_cancel() {
    assert!(subtract(&wall(), &wall()).unwrap().is_empty());
}

#[test]
fn overlapping_openings_in_sequence() {
    let first = Mesh::cuboid(v(4.0, -0.1, 1.0), v(5.0, 0.4, 2.0));
    let second = Mesh::cuboid(v(4.5, -0.1, 1.5), v(5.5, 0.4, 2.5));
    let once = subtract(&wall(), &first).unwrap();
    let twice = subtract(&once, &second).unwrap();
    assert_solid(&twice, 9.0 - 0.3 - 0.3 + 0.3 * 0.25);
}

#[test]
fn opening_rotated_about_the_vertical() {
    let turn = DMat3::from_rotation_z(0.6);
    let center = v(5.0, 0.15, 1.5);
    let cut = Mesh::cuboid(v(-0.5, -1.0, -0.5), v(0.5, 1.0, 0.5))
        .map_positions(|p| center + turn * p);
    let result = subtract(&wall(), &cut).unwrap();
    assert_eq!(result.open_edges(), 0);
    // The cut is a parallelogram in plan: width / cos(0.6) by the wall thickness.
    let removed = 1.0 / 0.6_f64.cos() * 0.3 * 1.0;
    assert!((result.volume() - (9.0 - removed)).abs() < 1e-6, "{}", result.volume());
}

#[test]
fn fourteen_windows_in_one_wall() {
    let mut mesh = Mesh::cuboid(v(0.0, 0.0, 0.0), v(30.0, 0.3, 3.0));
    let mut expected = 27.0;
    for i in 0..14 {
        let x = 0.5 + i as f64 * 2.0;
        let cut = Mesh::cuboid(v(x, -0.1, 1.0), v(x + 1.2, 0.4, 2.2));
        mesh = subtract(&mesh, &cut).unwrap();
        expected -= 1.2 * 1.2 * 0.3;
    }
    assert_solid(&mesh, expected);
}

/// Deterministic, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next()
    }
}

fn random_box(rng: &mut Rng, snap: bool) -> Mesh {
    let mut size = v(rng.range(0.2, 2.0), rng.range(0.2, 2.0), rng.range(0.2, 2.0));
    let mut center = v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
    if snap {
        // Whole tenths force shared planes and edges, the hard case for tolerances.
        size = (size * 10.0).round().max(DVec3::ONE) / 10.0;
        center = (center * 10.0).round() / 10.0;
    }
    let turn = if snap {
        DMat3::IDENTITY
    } else {
        DMat3::from_axis_angle(
            v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(0.1, 1.0)).normalize(),
            rng.range(0.0, 6.0),
        )
    };
    Mesh::cuboid(-size / 2.0, size / 2.0).map_positions(|p| center + turn * p)
}

fn conservation(snap: bool, seed: u64, rounds: usize) {
    let mut rng = Rng(seed);
    let mut worst_gap: f64 = 0.0;
    for round in 0..rounds {
        let a = random_box(&mut rng, snap);
        let b = random_box(&mut rng, snap);
        let outside = subtract(&a, &b).unwrap();
        let inside = intersect(&a, &b).unwrap();
        let gap = (outside.volume() + inside.volume() - a.volume()).abs();
        worst_gap = worst_gap.max(gap);
        assert!(gap < 1e-6, "round {round}: volume gap {gap}");
        assert_eq!(outside.open_edges(), 0, "round {round}: difference not watertight");
        assert_eq!(inside.open_edges(), 0, "round {round}: intersection not watertight");
    }
    println!("{rounds} rounds, snap={snap}: worst volume gap {worst_gap:.2e} m3");
}

#[test]
fn volume_is_conserved_for_random_rotated_boxes() {
    conservation(false, 0x9E37_79B9_7F4A_7C15, 2000);
}

#[test]
fn volume_is_conserved_for_boxes_sharing_planes() {
    conservation(true, 0xD1B5_4A32_D192_ED03, 2000);
}

#[test]
#[ignore = "timing report; run with --release --ignored --nocapture"]
fn timing() {
    for windows in [1, 14, 50, 100, 300] {
        let mut mesh = Mesh::cuboid(v(0.0, 0.0, 0.0), v(windows as f64 * 2.0 + 1.0, 0.3, 3.0));
        let start = Instant::now();
        for i in 0..windows {
            let x = 0.5 + i as f64 * 2.0;
            let cut = Mesh::cuboid(v(x, -0.1, 1.0), v(x + 1.2, 0.4, 2.2));
            mesh = subtract(&mesh, &cut).unwrap();
        }
        println!(
            "{windows} openings: {:?}, {} triangles, {} open edges",
            start.elapsed(),
            mesh.triangles.len(),
            mesh.open_edges()
        );
    }
}
