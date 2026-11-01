//! Areas swept along directrices, checked against closed-form volumes and positions.

use glam::DVec3;
use ifc_geometry::{Diagnostic, Mesh, ModelEvaluator};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use step_parser::index;

const COMMON: &str = "#1=IFCPROJECT('p',$,$,$,$,$,$,$,$);\n\
    #2=IFCWALL('w',$,$,$,$,$,#3,$);\n\
    #3=IFCPRODUCTDEFINITIONSHAPE($,$,(#4));\n\
    #4=IFCSHAPEREPRESENTATION($,'Body','SolidModel',(#5));\n\
    #900=IFCCARTESIANPOINT((0.,0.,0.));\n\
    #901=IFCDIRECTION((0.,0.,1.));\n\
    #902=IFCDIRECTION((1.,0.,0.));\n\
    #903=IFCAXIS2PLACEMENT3D(#900,$,$);\n\
    #904=IFCCARTESIANPOINT((0.,0.));\n\
    #905=IFCAXIS2PLACEMENT2D(#904,$);\n\
    #906=IFCRECTANGLEPROFILEDEF(.AREA.,$,#905,2.,1.);\n\
    #907=IFCCARTESIANPOINT((10.,0.,0.));\n\
    #908=IFCCARTESIANPOINT((10.,10.,0.));\n\
    #909=IFCPOLYLINE((#900,#907));\n\
    #910=IFCPOLYLINE((#900,#907,#908));\n";

fn evaluated(schema: &str, body: &str) -> (Mesh, Vec<Diagnostic>) {
    let text = format!(
        "ISO-10303-21;HEADER;FILE_SCHEMA(('{schema}'));ENDSEC;DATA;\n{COMMON}{body}ENDSEC;END-ISO-10303-21;"
    );
    let file = index(text.as_bytes()).unwrap();
    let mut evaluator = ModelEvaluator::new(&file).unwrap();
    let element = evaluator.evaluate(2).expect("the element evaluates");
    let diagnostics = evaluator.diagnostics().to_vec();
    (element.meshes.into_iter().next().map(|item| item.mesh).unwrap_or_default(), diagnostics)
}

fn evaluate(body: &str) -> Mesh {
    let (mesh, diagnostics) = evaluated("IFC4", body);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    mesh
}

fn bounds(mesh: &Mesh) -> (DVec3, DVec3) {
    let start = (DVec3::INFINITY, DVec3::NEG_INFINITY);
    mesh.positions.iter().fold(start, |(min, max), point| (min.min(*point), max.max(*point)))
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!((actual - expected).abs() <= tolerance, "{actual} against {expected}");
}

fn vector(actual: DVec3, expected: DVec3, tolerance: f64) {
    assert!(actual.distance(expected) <= tolerance, "{actual} against {expected}");
}

#[test]
fn fixed_reference_sets_the_x_axis_of_the_profile() {
    let mesh = evaluate("#5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#909,$,$,#901);\n");
    close(mesh.volume(), 2.0 * 1.0 * 10.0, 1e-9);
    assert_eq!(mesh.open_edges(), 0);
    // The profile's 2 m X dimension stands along the reference, up the Z axis.
    let (min, max) = bounds(&mesh);
    vector(min, DVec3::new(0.0, -0.5, -1.0), 1e-9);
    vector(max, DVec3::new(10.0, 0.5, 1.0), 1e-9);
}

#[test]
fn profile_y_axis_completes_a_right_handed_frame_with_the_tangent() {
    // An off-centre profile. Along +X with X up, Y is the tangent cross Z: towards -Y.
    let body = "#6=IFCCARTESIANPOINT((0.5,0.25));\n#7=IFCAXIS2PLACEMENT2D(#6,$);\n\
        #8=IFCRECTANGLEPROFILEDEF(.AREA.,$,#7,0.2,0.1);\n\
        #5=IFCFIXEDREFERENCESWEPTAREASOLID(#8,#903,#909,$,$,#901);\n";
    let mesh = evaluate(body);
    let (min, max) = bounds(&mesh);
    let middle = (min + max) / 2.0;
    close(middle.z, 0.5, 1e-9);
    close(middle.y, -0.25, 1e-9);
}

#[test]
fn mitred_bend_keeps_the_volume_of_a_centred_profile() {
    let mesh = evaluate("#5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#910,$,$,#901);\n");
    close(mesh.volume(), 2.0 * 20.0, 1e-9);
    assert_eq!(mesh.open_edges(), 0);
    let (_, max) = bounds(&mesh);
    close(max.x, 10.5, 1e-9);
    close(max.y, 10.0, 1e-9);
}

#[test]
fn legs_that_climb_keep_their_own_frame_up_to_the_mitre() {
    // The reference is upright, so the frame tilts where the path starts to climb.
    let body = "#6=IFCCARTESIANPOINT((10.,8.,3.));\n#7=IFCPOLYLINE((#900,#907,#6));\n\
        #5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#7,$,$,#901);\n";
    let mesh = evaluate(body);
    assert_eq!(mesh.open_edges(), 0);
    // Each leg is a prism cut through its axis, so it keeps the volume of its length. A leg
    // that twists from one frame into the other does not.
    let climbing = (64.0_f64 + 9.0).sqrt();
    close(mesh.volume(), 2.0 * (10.0 + climbing), 1e-6);
}

#[test]
fn sweep_follows_an_arc() {
    let body = "#6=IFCCIRCLE(#905,10.);\n\
        #7=IFCTRIMMEDCURVE(#6,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(1.570796326794897)),.T.,.PARAMETER.);\n\
        #5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#7,$,$,#901);\n";
    let mesh = evaluate(body);
    assert_eq!(mesh.open_edges(), 0);
    // Pappus, along eight chords of the quarter circle.
    let chords = 8.0 * 2.0 * 10.0 * (FRAC_PI_2 / 16.0).sin();
    close(mesh.volume(), 2.0 * chords, 0.05);
    for point in &mesh.positions {
        assert!(point.truncate().length() >= 9.4 && point.truncate().length() <= 10.6, "{point}");
    }
}

#[test]
fn sweep_stops_between_its_parameters() {
    let mesh = evaluate("#5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#910,0.5,1.5,#901);\n");
    let (min, max) = bounds(&mesh);
    close(min.x, 5.0, 1e-9);
    close(max.y, 5.0, 1e-9);
    close(mesh.volume(), 2.0 * 10.0, 1e-9);
}

#[test]
fn sweep_on_a_circle_takes_its_parameters_as_angles() {
    let body = "#6=IFCCIRCLE(#905,10.);\n\
        #5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#6,0.,1.570796326794897,#901);\n";
    let mesh = evaluate(body);
    // A quarter of the circle, not all of it. End sections stand square to the last chord,
    // so they lean by half a facet.
    let (min, max) = bounds(&mesh);
    assert!(min.x > -0.1 && min.y > -0.1, "{min}");
    close(max.x, 10.5, 0.05);
    close(max.y, 10.5, 0.05);
}

#[test]
fn sweep_may_be_bounded_by_length() {
    let body = "#5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#903,#910,\
        IFCNONNEGATIVELENGTHMEASURE(4.),IFCNONNEGATIVELENGTHMEASURE(13.),#901);\n";
    let (mesh, diagnostics) = evaluated("IFC4X3", body);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (min, max) = bounds(&mesh);
    close(min.x, 4.0, 1e-9);
    close(max.y, 3.0, 1e-9);
    close(mesh.volume(), 2.0 * 9.0, 1e-9);
}

#[test]
fn position_places_the_swept_solid() {
    let body = "#6=IFCCARTESIANPOINT((1.,2.,3.));\n#7=IFCAXIS2PLACEMENT3D(#6,$,$);\n\
        #5=IFCFIXEDREFERENCESWEPTAREASOLID(#906,#7,#909,$,$,#901);\n";
    let mesh = evaluate(body);
    let (min, max) = bounds(&mesh);
    vector(min, DVec3::new(1.0, 1.5, 2.0), 1e-9);
    vector(max, DVec3::new(11.0, 2.5, 4.0), 1e-9);
}

#[test]
fn directrix_derived_reference_behaves_as_the_fixed_one_off_cant() {
    let body = "#5=IFCDIRECTRIXDERIVEDREFERENCESWEPTAREASOLID(#906,#903,#909,$,$,#901);\n";
    let (mesh, diagnostics) = evaluated("IFC4X3", body);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    close(mesh.volume(), 20.0, 1e-9);
}

#[test]
fn surface_curve_sweep_on_a_plane_uses_its_normal() {
    let body = "#6=IFCPLANE(#903);\n#5=IFCSURFACECURVESWEPTAREASOLID(#906,#903,#909,$,$,#6);\n";
    let mesh = evaluate(body);
    let (min, max) = bounds(&mesh);
    vector(min, DVec3::new(0.0, -0.5, -1.0), 1e-9);
    vector(max, DVec3::new(10.0, 0.5, 1.0), 1e-9);
}

#[test]
fn surface_curve_sweep_keeps_the_profile_on_the_surface_normal() {
    // A ring around a vertical cylinder. The 2 m X dimension lies along the radius.
    let body = "#6=IFCCYLINDRICALSURFACE(#903,5.);\n#7=IFCCIRCLE(#905,5.);\n\
        #5=IFCSURFACECURVESWEPTAREASOLID(#906,#903,#7,$,$,#6);\n";
    let mesh = evaluate(body);
    let (min, max) = bounds(&mesh);
    close(min.z, -0.5, 1e-6);
    close(max.z, 0.5, 1e-6);
    close(max.x, 6.0, 0.05);
    // Pappus, with a centroid 5 m from the axis, on a 32-sided ring.
    let perimeter = 32.0 * 2.0 * 5.0 * (TAU / 64.0).sin();
    close(mesh.volume(), 2.0 * perimeter, 0.2);
    assert_eq!(mesh.open_edges(), 0);
    let _ = PI;
}

#[test]
fn surface_curve_sweep_off_its_surface_says_so() {
    let body = "#6=IFCTOROIDALSURFACE(#903,0.,1.);\n\
        #5=IFCSURFACECURVESWEPTAREASOLID(#906,#903,#909,$,$,#6);\n";
    let (_, diagnostics) = evaluated("IFC4", body);
    assert!(diagnostics.iter().any(|item| item.code == "bad-surface"), "{diagnostics:?}");
}
