//! Shells whose faces disagree about which way is out, as real exporters write them.

use glam::DVec3;
use ifc_geometry::{Mesh, ModelEvaluator};
use step_parser::index;

const CORNERS: &str = "#1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
    #2=IFCWALL('w',$,$,$,$,$,#3,$);\n\
    #3=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n\
    #4=IFCSHAPEREPRESENTATION($,'Body','SurfaceModel',(#5));\n\
    #10=IFCCARTESIANPOINT((0.,0.,0.));\n\
    #11=IFCCARTESIANPOINT((1.,0.,0.));\n\
    #12=IFCCARTESIANPOINT((1.,2.,0.));\n\
    #13=IFCCARTESIANPOINT((0.,2.,0.));\n\
    #14=IFCCARTESIANPOINT((0.,0.,3.));\n\
    #15=IFCCARTESIANPOINT((1.,0.,3.));\n\
    #16=IFCCARTESIANPOINT((1.,2.,3.));\n\
    #17=IFCCARTESIANPOINT((0.,2.,3.));\n";

/// A 1 by 2 by 3 box of six faces. `top` is the top face's loop, outward when `#14,#15,#16,#17`.
fn faces(top: &str) -> String {
    let loops = [
        "#10,#13,#12,#11".to_string(),
        top.to_string(),
        "#10,#11,#15,#14".to_string(),
        "#11,#12,#16,#15".to_string(),
        "#12,#13,#17,#16".to_string(),
        "#13,#10,#14,#17".to_string(),
    ];
    let mut text = String::new();
    for (index, points) in loops.iter().enumerate() {
        let at = 20 + index * 3;
        text += &format!(
            "#{at}=IFCPOLYLOOP(({points}));\n#{}=IFCFACEOUTERBOUND(#{at},.T.);\n#{}=IFCFACE((#{}));\n",
            at + 1,
            at + 2,
            at + 1
        );
    }
    text + "#40=IFCCONNECTEDFACESET((#22,#25,#28,#31,#34,#37));\n"
}

fn evaluate(body: &str) -> Mesh {
    evaluate_in("IFC2X3", body)
}

fn evaluate_in(schema: &str, body: &str) -> Mesh {
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('{schema}'));ENDSEC;DATA;\n{CORNERS}{body}ENDSEC;END-ISO-10303-21;"
    );
    let file = index(text.as_bytes()).unwrap();
    let mut evaluator = ModelEvaluator::new(&file).unwrap();
    let element = evaluator.evaluate(2).expect("the element evaluates");
    element.meshes.into_iter().next().expect("a mesh").mesh
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() <= 1e-9, "{actual} against {expected}");
}

#[test]
fn a_surface_model_with_one_face_written_backwards_closes_outward() {
    let body = faces("#17,#16,#15,#14") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let mesh = evaluate(&body);
    close(mesh.volume(), 6.0);
    assert_eq!(mesh.open_edges(), 0);
}

#[test]
fn a_shell_written_inside_out_turns_outward() {
    let body = "#41=IFCCLOSEDSHELL((#22,#25,#28,#31,#34,#37));\n#5=IFCFACETEDBREP(#41);\n";
    // Every face reversed, by the flag on each bound.
    let inverted = faces("#14,#15,#16,#17").replace(",.T.)", ",.F.)");
    close(evaluate(&(inverted + body)).volume(), 6.0);
}

#[test]
fn a_shell_written_correctly_keeps_its_triangles() {
    let body = faces("#14,#15,#16,#17") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let mesh = evaluate(&body);
    let mut repaired = mesh.clone();
    repaired.orient_shell();
    assert_eq!(repaired, mesh);
    close(mesh.volume(), 6.0);
}

fn flipped(mut mesh: Mesh, triangles: &[usize]) -> Mesh {
    for index in triangles {
        mesh.triangles[*index].swap(1, 2);
    }
    mesh
}

fn unit_box() -> Mesh {
    Mesh::cuboid(DVec3::ZERO, DVec3::splat(1.0))
}

#[test]
fn every_way_of_flipping_one_face_of_a_box_is_repaired() {
    for face in 0..6 {
        let mut mesh = flipped(unit_box(), &[face * 2, face * 2 + 1]);
        assert_ne!(mesh.open_edges(), 0);
        mesh.orient_shell();
        assert_eq!(mesh.open_edges(), 0, "face {face}");
        close(mesh.volume(), 1.0);
    }
}

#[test]
fn separate_solids_in_one_mesh_are_each_turned_outward() {
    let far = Mesh::cuboid(DVec3::splat(5.0), DVec3::splat(7.0));
    let mut mesh = unit_box();
    let first = mesh.positions.len() as u32;
    mesh.positions.extend(far.positions);
    // The second box inside out, so the two would cancel in a sum over the whole mesh.
    mesh.triangles.extend(far.triangles.iter().map(|[a, b, c]| [a + first, c + first, b + first]));
    mesh.orient_shell();
    close(mesh.volume(), 1.0 + 8.0);
}

#[test]
fn an_open_surface_keeps_the_way_most_of_it_faces() {
    // A box with no top: 10 triangles, one flipped. There is no inside, so the rest decide.
    let mut open = unit_box();
    open.triangles.drain(2..4);
    let wanted = open.clone();
    let mut mesh = flipped(open, &[3]);
    mesh.orient_shell();
    assert_eq!(mesh, wanted);
}

#[test]
fn an_open_surface_facing_inward_is_left_that_way() {
    // Volume means nothing without a closed boundary, so it must not turn this round.
    let mut open = unit_box();
    open.triangles.drain(2..4);
    open.triangles.iter_mut().for_each(|triangle| triangle.swap(1, 2));
    let wanted = open.clone();
    open.orient_shell();
    assert_eq!(open, wanted);
}

#[test]
fn an_edge_of_three_faces_does_not_panic_or_lose_triangles() {
    let mut mesh = Mesh {
        positions: vec![
            DVec3::ZERO,
            DVec3::X,
            DVec3::Y,
            DVec3::NEG_Y,
            DVec3::Z,
        ],
        triangles: vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]],
    };
    mesh.orient_shell();
    assert_eq!(mesh.triangles.len(), 3);
}

#[test]
fn a_large_shell_is_walked_without_recursion() {
    // A strip of a million triangles, every other one flipped: a recursive walk would overflow
    // the 1 MB stack of a WASM build.
    let count = 500_000u32;
    let mut mesh = Mesh::default();
    for column in 0..=count {
        mesh.positions.push(DVec3::new(f64::from(column), 0.0, 0.0));
        mesh.positions.push(DVec3::new(f64::from(column), 1.0, 0.0));
    }
    for column in 0..count {
        let (a, b, c, d) = (column * 2, column * 2 + 2, column * 2 + 3, column * 2 + 1);
        mesh.triangles.push([a, b, c]);
        mesh.triangles.push(if column % 2 == 0 { [a, c, d] } else { [a, d, c] });
    }
    mesh.orient_shell();
    let up = mesh.triangle_corners().filter(|[a, b, c]| (b - a).cross(c - a).z > 0.0).count();
    assert_eq!(up, mesh.triangles.len());
}


const CUBE_POINTS: &str = "#20=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(2.,0.,0.),(2.,2.,0.),(0.,2.,0.),(0.,0.,2.),(2.,0.,2.),(2.,2.,2.),(0.,2.,2.)));\n";

#[test]
fn a_triangulated_set_written_inward_turns_outward() {
    // The twelve triangles of a 2 m cube, every one wound towards the inside.
    let faces = "((1,2,3),(1,3,4),(5,7,6),(5,8,7),(1,6,2),(1,5,6),(2,7,3),(2,6,7),(3,8,4),(3,7,8),(4,5,1),(4,8,5))";
    let body = format!("{CUBE_POINTS}#5=IFCTRIANGULATEDFACESET(#20,$,.T.,{faces},$);\n");
    close(evaluate_in("IFC4", &body).volume(), 8.0);
}

#[test]
fn a_polygonal_set_written_inward_turns_outward() {
    let loops = ["1,2,3,4", "5,8,7,6", "1,5,6,2", "2,6,7,3", "3,7,8,4", "4,8,5,1"];
    let mut body = CUBE_POINTS.to_string();
    for (index, points) in loops.iter().enumerate() {
        body += &format!("#{}=IFCINDEXEDPOLYGONALFACE(({points}));\n", 30 + index);
    }
    body += "#5=IFCPOLYGONALFACESET(#20,.T.,(#30,#31,#32,#33,#34,#35),$);\n";
    close(evaluate_in("IFC4", &body).volume(), 8.0);
}

/// Two walls using one mapped box, the second moved 10 m along X.
const MAPPED: &str = "#2=IFCWALL('w',$,$,$,$,$,#3,$);\n\
    #3=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n\
    #4=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#50));\n\
    #6=IFCWALL('x',$,$,$,$,$,#7,$);\n\
    #7=IFCPRODUCTDEFINITIONSHAPE($,$,(#8));\n\
    #8=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#51));\n\
    #50=IFCMAPPEDITEM(#52,#53);\n\
    #51=IFCMAPPEDITEM(#52,#54);\n\
    #52=IFCREPRESENTATIONMAP(#55,#56);\n\
    #53=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,#57,$,$);\n\
    #54=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,#58,$,$);\n\
    #55=IFCAXIS2PLACEMENT3D(#57,$,$);\n\
    #56=IFCSHAPEREPRESENTATION($,'Body','Brep',(#5));\n\
    #57=IFCCARTESIANPOINT((0.,0.,0.));\n\
    #58=IFCCARTESIANPOINT((10.,0.,0.));\n";

fn mapped_file(item: &str) -> String {
    format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC2X3'));ENDSEC;DATA;\n#1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
         {}{MAPPED}{item}ENDSEC;END-ISO-10303-21;",
        CORNERS.split_once("#10=").map(|(_, rest)| format!("#10={rest}")).unwrap()
    )
}

#[test]
fn each_instance_of_a_mapped_item_keeps_its_own_place() {
    let body = faces("#14,#15,#16,#17") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let text = mapped_file(&body);
    let file = index(text.as_bytes()).unwrap();
    let mut evaluator = ModelEvaluator::new(&file).unwrap();
    for (id, x) in [(2, 0.0), (6, 10.0), (2, 0.0)] {
        let element = evaluator.evaluate(id).expect("the element evaluates");
        let mesh = &element.meshes[0].mesh;
        close(mesh.volume(), 6.0);
        let least = mesh.positions.iter().fold(f64::INFINITY, |least, point| least.min(point.x));
        close(least, x);
    }
    assert!(evaluator.diagnostics().is_empty());
}

#[test]
fn a_problem_in_a_mapped_item_is_reported_for_each_element_using_it() {
    // A face bounded by a loop the kernel cannot use is skipped, with a warning.
    let body = faces("#14,#15,#16,#17").replace("#20=IFCPOLYLOOP((#10,#13,#12,#11));", "#20=IFCVERTEXLOOP(#61);\n#61=IFCVERTEXPOINT(#10);")
        + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    assert!(body.contains("IFCVERTEXLOOP"), "{body}");
    let text = mapped_file(&body);
    let file = index(text.as_bytes()).unwrap();
    let mut evaluator = ModelEvaluator::new(&file).unwrap();
    evaluator.evaluate(2).expect("the first element evaluates");
    let first = evaluator.diagnostics().len();
    assert!(first > 0, "the item should warn");
    evaluator.evaluate(6).expect("the second element evaluates");
    assert!(evaluator.diagnostics().len() > first, "{:?}", evaluator.diagnostics());
}

#[test]
fn triangular_faces_are_kept_and_zero_area_ones_are_dropped() {
    // A tetrahedron of four faces, plus one face with all three corners on a line.
    let body = "#20=IFCPOLYLOOP((#10,#12,#11));#21=IFCFACEOUTERBOUND(#20,.T.);#22=IFCFACE((#21));\n\
        #23=IFCPOLYLOOP((#10,#11,#14));#24=IFCFACEOUTERBOUND(#23,.T.);#25=IFCFACE((#24));\n\
        #26=IFCPOLYLOOP((#11,#12,#14));#27=IFCFACEOUTERBOUND(#26,.T.);#28=IFCFACE((#27));\n\
        #29=IFCPOLYLOOP((#12,#10,#14));#30=IFCFACEOUTERBOUND(#29,.T.);#31=IFCFACE((#30));\n\
        #32=IFCPOLYLOOP((#10,#14,#60));#33=IFCFACEOUTERBOUND(#32,.T.);#34=IFCFACE((#33));\n\
        #60=IFCCARTESIANPOINT((0.,0.,6.));\n\
        #40=IFCCONNECTEDFACESET((#22,#25,#28,#31,#34));\n\
        #5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let mesh = evaluate(body);
    assert_eq!(mesh.triangles.len(), 4);
    // Base 1 by 2 over 2, height 3, over 3.
    close(mesh.volume(), 1.0 * 2.0 / 2.0 * 3.0 / 3.0);
}


#[test]
fn a_set_with_no_shared_indices_still_closes_outward() {
    // A 2 m cube as 36 corners, one per triangle corner, every triangle wound inward: nothing
    // shares an index, so adjacency has to come from where the corners are.
    let cube = Mesh::cuboid(DVec3::ZERO, DVec3::splat(2.0));
    let mut mesh = Mesh::default();
    for [a, b, c] in cube.triangle_corners() {
        let first = mesh.positions.len() as u32;
        mesh.positions.extend([a, c, b]);
        mesh.triangles.push([first, first + 1, first + 2]);
    }
    close(mesh.volume(), -8.0);
    mesh.orient_shell();
    close(mesh.volume(), 8.0);
}

#[test]
fn a_corner_at_negative_zero_is_the_same_corner() {
    // Rounding gives -0.0 as easily as 0.0, and their bits differ. Half the triangles use one.
    let cube = Mesh::cuboid(DVec3::ZERO, DVec3::splat(2.0));
    let mut mesh = Mesh::default();
    for (index, [a, b, c]) in cube.triangle_corners().enumerate() {
        let zero = |point: DVec3| {
            let axis = |value: f64| if value == 0.0 && index % 2 == 1 { -0.0 } else { value };
            DVec3::new(axis(point.x), axis(point.y), axis(point.z))
        };
        let first = mesh.positions.len() as u32;
        mesh.positions.extend([zero(a), zero(c), zero(b)]);
        mesh.triangles.push([first, first + 1, first + 2]);
    }
    mesh.orient_shell();
    close(mesh.volume(), 8.0);
}

fn diagnostics_of(body: &str) -> (Mesh, Vec<String>) {
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('IFC2X3'));ENDSEC;DATA;\n{CORNERS}{body}ENDSEC;END-ISO-10303-21;"
    );
    let file = index(text.as_bytes()).unwrap();
    let mut evaluator = ModelEvaluator::new(&file).unwrap();
    let element = evaluator.evaluate(2).expect("the element evaluates");
    let mesh = element.meshes.into_iter().next().map(|item| item.mesh).unwrap_or_default();
    (mesh, evaluator.diagnostics().iter().map(|item| item.code.to_string()).collect())
}

#[test]
fn a_corner_that_cannot_be_read_is_reported() {
    // #99 does not exist: the quad would quietly become a triangle.
    let body = faces("#14,#15,#16,#99") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let (_, codes) = diagnostics_of(&body);
    assert!(codes.iter().any(|code| code == "bad-face"), "{codes:?}");
}

#[test]
fn a_face_left_with_too_few_corners_is_reported() {
    let body = faces("#14,#15") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let (_, codes) = diagnostics_of(&body);
    assert!(codes.iter().any(|code| code == "bad-face"), "{codes:?}");
}

#[test]
fn a_clean_shell_reports_nothing() {
    let body = faces("#14,#15,#16,#17") + "#5=IFCFACEBASEDSURFACEMODEL((#40));\n";
    let (_, codes) = diagnostics_of(&body);
    assert!(codes.is_empty(), "{codes:?}");
}