//! Projection and hidden lines, with the boxes placed so the answer is known by hand.

mod common;

use cad_slicer::{Cut, DrawingMesh, DrawingOptions, Layer, Segment, draw};
use common::{cuboid, layer, v};

/// A slab whose top face is the cut plane, over a small box that it hides completely.
fn slab_over_box() -> Vec<DrawingMesh> {
    vec![
        DrawingMesh::new(1, 1, "slab", cuboid(v(0.0, 0.0, 1.5), v(4.0, 4.0, 2.0))),
        DrawingMesh::new(1, 2, "slab", cuboid(v(1.0, 1.0, 0.0), v(2.0, 2.0, 1.0))),
    ]
}

fn options(depth: Option<f64>) -> DrawingOptions {
    DrawingOptions {
        cut: Cut::Plan { z: 2.0 },
        depth,
        include_projection: true,
        include_hatching: false,
    }
}

fn bbox(segments: &[Segment]) -> (f64, f64, f64, f64) {
    let mut min = (f64::MAX, f64::MAX);
    let mut max = (f64::MIN, f64::MIN);
    for segment in segments {
        for point in [segment.a, segment.b] {
            min = (min.0.min(point.x), min.1.min(point.y));
            max = (max.0.max(point.x), max.1.max(point.y));
        }
    }
    (min.0, min.1, max.0, max.1)
}

fn drawn(depth: Option<f64>) -> Layer {
    let drawing = draw(&slab_over_box(), &options(depth));
    assert!(drawing.diagnostics.is_empty(), "{:?}", drawing.diagnostics);
    layer(&drawing, "slab").clone()
}

#[test]
fn a_box_under_a_slab_is_hidden_where_it_lies() {
    let layer = drawn(None);
    // The slab's outline: four sides, the top and bottom rims merged into one.
    assert_eq!(layer.visible.len(), 4, "{:?}", layer.visible);
    let (min_x, min_y, max_x, max_y) = bbox(&layer.visible);
    assert!((min_x).abs() < 1e-9 && (max_x - 4.0).abs() < 1e-9);
    assert!((min_y).abs() < 1e-9 && (max_y - 4.0).abs() < 1e-9);

    // The hidden box: its own square, exactly where it stands.
    assert_eq!(layer.hidden.len(), 4, "{:?}", layer.hidden);
    let (min_x, min_y, max_x, max_y) = bbox(&layer.hidden);
    assert!((min_x - 1.0).abs() < 1e-9, "{:?}", layer.hidden);
    assert!((min_y - 1.0).abs() < 1e-9);
    assert!((max_x - 2.0).abs() < 1e-9);
    assert!((max_y - 2.0).abs() < 1e-9);
    let length: f64 = layer.hidden.iter().map(Segment::length).sum();
    assert!((length - 4.0).abs() < 1e-9, "hidden perimeter {length}");
}

#[test]
fn a_visible_edge_is_not_dashed() {
    let layer = drawn(None);
    // Every visible run lies on the slab's outline, never inside it.
    for segment in &layer.visible {
        for point in [segment.a, segment.b] {
            let on_outline = point.x.abs() < 1e-9
                || (point.x - 4.0).abs() < 1e-9
                || point.y.abs() < 1e-9
                || (point.y - 4.0).abs() < 1e-9;
            assert!(on_outline, "{point:?} is not on the outline");
        }
    }
}

#[test]
fn depth_stops_the_projection_before_the_hidden_box() {
    // The box's top is 1 m behind the plane; a reach of 0.5 m must not see it.
    let layer = drawn(Some(0.5));
    assert!(layer.hidden.is_empty(), "{:?}", layer.hidden);
    assert_eq!(layer.visible.len(), 4);
}

#[test]
fn no_projection_means_no_hidden_lines() {
    let mut quiet = options(None);
    quiet.include_projection = false;
    let drawing = draw(&slab_over_box(), &quiet);
    let layer = layer(&drawing, "slab");
    assert!(layer.visible.is_empty());
    assert!(layer.hidden.is_empty());
    // The cut itself is still there.
    assert_eq!(layer.loops.len(), 1);
}

#[test]
fn a_section_projects_towards_its_look_direction() {
    // Looking along +X at x = 0, a wall at x = 1 is behind the plane and visible; the same
    // wall at x = -1 is in front of it and must not appear at all.
    let wall = |x: f64| DrawingMesh::new(1, 3, "wall", cuboid(v(x, 0.0, 0.0), v(x + 0.2, 1.0, 2.0)));
    let mut options = DrawingOptions::cut_only(Cut::Section {
        axis: cad_slicer::Axis::X,
        position: 0.0,
        look: cad_slicer::Look::Positive,
    });
    options.include_projection = true;
    let drawing = draw(&[wall(1.0), wall(-1.0)], &options);
    let layer = layer(&drawing, "wall");
    assert!(!layer.visible.is_empty());
    // Along +X the sheet's X is -Y and its Y is Z, so the far wall spans y in 0..1 and
    // z in 0..2. The near wall would sit at the same place: the cut plane hides it.
    for segment in &layer.visible {
        for point in [segment.a, segment.b] {
            assert!(point.x >= -1.0 - 1e-9 && point.x <= 0.0 + 1e-9, "{point:?}");
            assert!(point.y >= -1e-9 && point.y <= 2.0 + 1e-9, "{point:?}");
        }
    }
}
