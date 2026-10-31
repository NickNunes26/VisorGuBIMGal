//! Indexed triangle meshes in `f64`, in metres.

use glam::DVec3;
use std::collections::HashMap;

/// Triangles wind counter-clockwise seen from outside the solid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// An axis-aligned box.
    pub fn cuboid(min: DVec3, max: DVec3) -> Self {
        let corner = |x: bool, y: bool, z: bool| {
            DVec3::new(
                if x { max.x } else { min.x },
                if y { max.y } else { min.y },
                if z { max.z } else { min.z },
            )
        };
        let positions = vec![
            corner(false, false, false),
            corner(true, false, false),
            corner(true, true, false),
            corner(false, true, false),
            corner(false, false, true),
            corner(true, false, true),
            corner(true, true, true),
            corner(false, true, true),
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        Self {
            positions,
            triangles,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Applies `f` to every vertex. A mirroring `f` must also flip the winding.
    pub fn map_positions(&self, f: impl Fn(DVec3) -> DVec3) -> Self {
        Self {
            positions: self.positions.iter().map(|p| f(*p)).collect(),
            triangles: self.triangles.clone(),
        }
    }

    /// Yields each triangle's corners, skipping any with an out-of-range index.
    pub fn triangle_corners(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        self.triangles.iter().filter_map(|[a, b, c]| {
            Some([
                *self.positions.get(*a as usize)?,
                *self.positions.get(*b as usize)?,
                *self.positions.get(*c as usize)?,
            ])
        })
    }

    /// Enclosed volume. Meaningful for closed meshes only.
    pub fn volume(&self) -> f64 {
        // Measured from the first vertex, which keeps precision at large coordinates.
        let Some(pivot) = self.positions.first().copied() else {
            return 0.0;
        };
        let six_volume: f64 = self
            .triangle_corners()
            .map(|[a, b, c]| (a - pivot).dot((b - pivot).cross(c - pivot)))
            .sum();
        six_volume / 6.0
    }

    /// Directed edges with no opposite partner. Zero means watertight.
    pub fn open_edges(&self) -> usize {
        let mut balance: HashMap<(u32, u32), i32> = HashMap::new();
        for [a, b, c] in &self.triangles {
            for (from, to) in [(*a, *b), (*b, *c), (*c, *a)] {
                if from < to {
                    *balance.entry((from, to)).or_default() += 1;
                } else {
                    *balance.entry((to, from)).or_default() -= 1;
                }
            }
        }
        balance.values().map(|count| count.unsigned_abs() as usize).sum()
    }
}
