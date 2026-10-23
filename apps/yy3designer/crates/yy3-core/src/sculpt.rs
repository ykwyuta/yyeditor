//! Per-voxel carving, independent of the six silhouette images and their layers.
use crate::{
    mesh::{self, Volume},
    *,
};
use std::{
    collections::{BTreeSet, HashSet},
    io,
};

pub fn depths(object: &Object, direction: Direction) -> io::Result<Vec<u32>> {
    let volume = mesh::reconstruct(object)?;
    Ok(counts(&volume, direction))
}
fn counts(volume: &Volume, direction: Direction) -> Vec<u32> {
    let [w, h] = direction.size(volume.size);
    let mut counts = vec![0; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            for t in 0..direction.ray_length(volume.size) {
                if volume.get(direction.ray_point([x, y], t, volume.size))[3] != 0 {
                    counts[(y * w + x) as usize] += 1;
                }
            }
        }
    }
    counts
}
pub struct Stroke {
    volume: Volume,
    removed: BTreeSet<u32>,
    seen: HashSet<[u32; 2]>,
    direction: Direction,
    restore: bool,
    pub depths: Vec<u32>,
}
impl Stroke {
    pub fn new(object: &Object, direction: Direction, restore: bool) -> io::Result<Self> {
        let mut volume = mesh::reconstruct(object)?;
        let depths = counts(&volume, direction);
        if restore {
            let mut original = object.clone();
            original.carved.clear();
            volume = mesh::reconstruct(&original)?;
        }
        Ok(Self {
            volume,
            removed: object.carved.iter().copied().collect(),
            seen: HashSet::new(),
            direction,
            restore,
            depths,
        })
    }
    /// Each pixel is touched once per stroke, including overlapping brush dabs.
    pub fn apply(
        &mut self,
        object: &mut Object,
        a: [i32; 2],
        b: [i32; 2],
        size: u32,
        clip: Option<Rect>,
    ) {
        let [w, h] = self.direction.size(self.volume.size);
        let half = (size / 2) as i32;
        for [cx, cy] in line_points(a, b) {
            for dy in 0..size as i32 {
                for dx in 0..size as i32 {
                    let [x, y] = [cx + dx - half, cy + dy - half];
                    if x < 0
                        || y < 0
                        || x >= w as i32
                        || y >= h as i32
                        || clip.is_some_and(|r| !r.contains(x, y))
                    {
                        continue;
                    }
                    let p = [x as u32, y as u32];
                    if !self.seen.insert(p) {
                        continue;
                    }
                    for t in 0..self.direction.ray_length(self.volume.size) {
                        let [x, y, z] = self.direction.ray_point(p, t, self.volume.size);
                        let i = (z * self.volume.size[1] + y) * self.volume.size[0] + x;
                        if self.restore {
                            if self.removed.remove(&i) && self.volume.cells[i as usize][3] != 0 {
                                self.depths[(p[1] * w + p[0]) as usize] += 1;
                            }
                        } else if self.volume.cells[i as usize][3] != 0 {
                            self.removed.insert(i);
                            self.volume.cells[i as usize] = CLEAR;
                            self.depths[(p[1] * w + p[0]) as usize] -= 1;
                            break;
                        }
                    }
                }
            }
        }
        object.carved = self.removed.iter().copied().collect();
    }
}
