//! IFC geometry evaluation: placements, solids, meshes and booleans. Owner: Claude (T-030..T-032).

pub mod csg;
pub mod mesh;
pub mod model;
mod solids;

pub use csg::{CsgError, intersect, subtract, subtract_all, union};
pub use mesh::Mesh;
pub use model::{Diagnostic, ElementGeometry, ElementMesh, ModelEvaluator, UnsupportedSchema};
