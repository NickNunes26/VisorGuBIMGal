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

    /// Makes a shell agree with itself: faces across a shared edge wind the same way, a closed
    /// part winds outward, and an open part keeps the way most of its area faces. Real files
    /// write faces of one shell in both directions. An edge of three or more faces joins nothing.
    pub fn orient_shell(&mut self) {
        let mut edges: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (index, [a, b, c]) in self.triangles.iter().enumerate() {
            for (from, to) in [(*a, *b), (*b, *c), (*c, *a)] {
                edges.entry((from.min(to), from.max(to))).or_default().push(index as u32);
            }
        }
        let mut flip = vec![false; self.triangles.len()];
        let mut seen = vec![false; self.triangles.len()];
        for start in 0..self.triangles.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut part = vec![start];
            let mut closed = true;
            // Grows while it is walked: an explicit list, because recursion would grow with the
            // file and the WASM stack is 1 MB.
            let mut next = 0;
            while next < part.len() {
                let current = part[next];
                next += 1;
                let [a, b, c] = self.triangles[current];
                for (from, to) in [(a, b), (b, c), (c, a)] {
                    let users = &edges[&(from.min(to), from.max(to))];
                    let [one, two] = users.as_slice() else {
                        closed = false;
                        continue;
                    };
                    let other = if *one as usize == current { *two } else { *one } as usize;
                    if seen[other] {
                        continue;
                    }
                    seen[other] = true;
                    // Neighbours that agree run along their shared edge in opposite directions.
                    let same_way = self.runs(other, from, to);
                    flip[other] = same_way != flip[current];
                    part.push(other);
                }
            }
            self.settle(&part, closed, &mut flip);
        }
        for (triangle, flip) in self.triangles.iter_mut().zip(flip) {
            if flip {
                triangle.swap(1, 2);
            }
        }
    }

    /// Whether a triangle runs along `from` to `to`, rather than the other way.
    fn runs(&self, triangle: usize, from: u32, to: u32) -> bool {
        let [a, b, c] = self.triangles[triangle];
        [(a, b), (b, c), (c, a)].contains(&(from, to))
    }

    /// Chooses the direction of one connected part, whose flips so far agree with its first.
    fn settle(&self, part: &[usize], closed: bool, flip: &mut [bool]) {
        let corners = |index: usize| {
            let [a, b, c] = self.triangles[index].map(|id| self.positions[id as usize]);
            if flip[index] { [a, c, b] } else { [a, b, c] }
        };
        let turn = if closed {
            let pivot = corners(part[0])[0];
            let six_volume: f64 = part
                .iter()
                .map(|index| {
                    let [a, b, c] = corners(*index);
                    (a - pivot).dot((b - pivot).cross(c - pivot))
                })
                .sum();
            six_volume < 0.0
        } else {
            let area = |turned: bool| -> f64 {
                part.iter()
                    .filter(|index| flip[**index] == turned)
                    .map(|index| {
                        let [a, b, c] = corners(*index);
                        (b - a).cross(c - a).length()
                    })
                    .sum()
            };
            area(true) > area(false)
        };
        if turn {
            for index in part {
                flip[*index] = !flip[*index];
            }
        }
    }
}
