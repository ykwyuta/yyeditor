use crate::*;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::Path,
};

pub struct Volume {
    pub size: [u32; 3],
    pub cells: Vec<Color>,
}
impl Volume {
    fn index(&self, [x, y, z]: [u32; 3]) -> usize {
        ((z * self.size[1] + y) * self.size[0] + x) as usize
    }
    pub fn get(&self, p: [u32; 3]) -> Color {
        self.cells[self.index(p)]
    }
    fn occupied(&self, p: [i32; 3]) -> bool {
        p.iter()
            .zip(self.size)
            .all(|(&v, n)| v >= 0 && (v as u32) < n)
            && self.get(p.map(|v| v as u32))[3] != 0
    }
    pub fn count(&self) -> usize {
        self.cells.iter().filter(|c| c[3] != 0).count()
    }
}
pub fn reconstruct(object: &Object) -> io::Result<Volume> {
    validate_size(object.size)?;
    let [w, h, d] = object.size;
    let views: Vec<_> = Direction::ALL
        .iter()
        .filter_map(|&dir| {
            object.views[dir.index()]
                .as_ref()
                .map(|v| (dir, v.composite()))
        })
        .collect();
    if views.is_empty() {
        return Err(crate::invalid("少なくとも1方向の画像が必要です"));
    }
    let mut result = Volume {
        size: object.size,
        cells: vec![CLEAR; (w * h * d) as usize],
    };
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                let p = [x, y, z];
                let mut best = u32::MAX;
                let mut color = CLEAR;
                let mut keep = true;
                for (dir, pixels) in &views {
                    let [px, py] = dir.project(p, object.size);
                    let [vw, _] = dir.size(object.size);
                    let c = pixels[(py * vw + px) as usize];
                    if c[3] < object.alpha_cutoff.max(1) {
                        keep = false;
                        break;
                    }
                    let (distance, length) = match dir {
                        Direction::Front => (z, d),
                        Direction::Back => (d - 1 - z, d),
                        Direction::Left => (x, w),
                        Direction::Right => (w - 1 - x, w),
                        Direction::Top => (h - 1 - y, h),
                        Direction::Bottom => (y, h),
                    };
                    let distance = distance * 65536 / length;
                    if distance < best {
                        best = distance;
                        color = c;
                    }
                }
                if keep {
                    let i = result.index(p);
                    color[3] = 255;
                    result.cells[i] = color;
                }
            }
        }
    }
    for &i in &object.carved {
        if let Some(cell) = result.cells.get_mut(i as usize) {
            *cell = CLEAR;
        } else {
            return Err(crate::invalid("削った厚みの位置が不正です"));
        }
    }
    Ok(result)
}
pub fn projected_view(object: &Object, direction: Direction) -> io::Result<View> {
    let volume = reconstruct(object)?;
    let [w, h] = direction.size(object.size);
    let mut view = View::new(w, h);
    let mut depths = vec![u32::MAX; (w * h) as usize];
    for z in 0..volume.size[2] {
        for y in 0..volume.size[1] {
            for x in 0..volume.size[0] {
                let p = [x, y, z];
                let color = volume.get(p);
                if color[3] == 0 {
                    continue;
                }
                let [px, py] = direction.project(p, volume.size);
                let depth = match direction {
                    Direction::Front => z,
                    Direction::Back => volume.size[2] - 1 - z,
                    Direction::Left => x,
                    Direction::Right => volume.size[0] - 1 - x,
                    Direction::Top => volume.size[1] - 1 - y,
                    Direction::Bottom => y,
                };
                let i = (py * w + px) as usize;
                if depth < depths[i] {
                    depths[i] = depth;
                    view.layers[0].pixels[i] = color;
                }
            }
        }
    }
    Ok(view)
}
#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub points: [[f32; 3]; 3],
    pub color: Color,
}
pub struct Mesh {
    pub size: [u32; 3],
    pub triangles: Vec<Triangle>,
    pub voxels: usize,
}
const LIMIT: usize = 2_000_000;
fn neighbors(p: [u32; 3]) -> [[i32; 3]; 6] {
    let [x, y, z] = p.map(|v| v as i32);
    [
        [x - 1, y, z],
        [x + 1, y, z],
        [x, y - 1, z],
        [x, y + 1, z],
        [x, y, z - 1],
        [x, y, z + 1],
    ]
}
/// Move a surface point only at a locally convex exterior edge/corner.
/// All incident faces use world coordinates and the same occupancy neighborhood,
/// so shared vertices remain identical even at steps and concave junctions.
fn round_surface(volume: &Volume, q: [f32; 3], radius: f32) -> [f32; 3] {
    let mut cells = [[0; 2]; 3];
    let mut counts = [1; 3];
    let mut boundaries = [0.; 3];
    for axis in 0..3 {
        let boundary = q[axis].round();
        boundaries[axis] = boundary;
        if (q[axis] - boundary).abs() < radius + 0.00001 {
            cells[axis] = [boundary as i32 - 1, boundary as i32];
            counts[axis] = 2;
        } else {
            cells[axis][0] = q[axis].floor() as i32;
        }
    }
    let mut sides = [0u8; 3];
    let mut occupied = 0;
    for x in 0..counts[0] {
        for y in 0..counts[1] {
            for z in 0..counts[2] {
                if volume.occupied([cells[0][x], cells[1][y], cells[2][z]]) {
                    occupied += 1;
                    for (axis, side) in [x, y, z].into_iter().enumerate() {
                        sides[axis] |= 1 << side;
                    }
                }
            }
        }
    }
    // A non-Cartesian neighborhood is a concave/diagonal junction. Keep it
    // unchanged rather than pulling a bridge or an inside corner apart.
    let rectangular = sides.iter().map(|s| s.count_ones()).product::<u32>();
    if occupied == 0 || occupied != rectangular {
        return q;
    }
    let mut inner = q;
    for axis in 0..3 {
        if counts[axis] == 2 {
            match sides[axis] {
                1 => inner[axis] = q[axis].min(boundaries[axis] - radius),
                2 => inner[axis] = q[axis].max(boundaries[axis] + radius),
                _ => (), // The solid continues across this cell boundary.
            }
        }
    }
    let delta = sub(q, inner);
    let length = norm(delta);
    if length <= radius {
        q
    } else {
        std::array::from_fn(|axis| inner[axis] + delta[axis] * radius / length)
    }
}
/// A single exterior surface; shared cell faces are never emitted or rounded.
pub fn build(object: &Object) -> io::Result<Mesh> {
    let volume = reconstruct(object)?;
    let mut mesh = Mesh {
        size: volume.size,
        triangles: vec![],
        voxels: volume.count(),
    };
    let radius = (object.roundness * 0.5).clamp(0., 0.5);
    let steps: &[f32] = if radius > 0. {
        &[-0.5, -0.5 + radius, 0.5 - radius, 0.5]
    } else {
        &[-0.5, 0.5]
    };
    let [w, h, d] = volume.size;
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                let p = [x, y, z];
                let color = volume.get(p);
                if color[3] == 0 {
                    continue;
                }
                let exposed = neighbors(p).map(|p| !volume.occupied(p));
                if exposed.iter().all(|e| !*e) {
                    continue;
                }
                // Neighboring face quads reuse each point. Bound the cache to
                // one cell rather than keeping a map for the entire model.
                let mut rounded = [None; 64];
                for (axis, sign, face) in [
                    (0, -1., 0),
                    (0, 1., 1),
                    (1, -1., 2),
                    (1, 1., 3),
                    (2, -1., 4),
                    (2, 1., 5),
                ] {
                    if !exposed[face] {
                        continue;
                    }
                    let u = (axis + 1) % 3;
                    let v = (axis + 2) % 3;
                    for iu in 0..steps.len() - 1 {
                        for iv in 0..steps.len() - 1 {
                            let mut points = [[0.; 3]; 4];
                            for (i, (a, b)) in
                                [(iu, iv), (iu + 1, iv), (iu + 1, iv + 1), (iu, iv + 1)]
                                    .into_iter()
                                    .enumerate()
                            {
                                let mut q = [0.; 3];
                                q[axis] = sign * 0.5;
                                q[u] = steps[a];
                                q[v] = steps[b];
                                for j in 0..3 {
                                    q[j] += p[j] as f32 + 0.5;
                                }
                                points[i] = if radius > 0. {
                                    let mut indices = [0; 3];
                                    indices[axis] = if sign < 0. { 0 } else { 3 };
                                    indices[u] = a;
                                    indices[v] = b;
                                    let key = indices[0] + 4 * indices[1] + 16 * indices[2];
                                    *rounded[key]
                                        .get_or_insert_with(|| round_surface(&volume, q, radius))
                                } else {
                                    q
                                };
                            }
                            let mut indices = [[0, 1, 2], [0, 2, 3]];
                            if sign < 0. {
                                for t in &mut indices {
                                    t.swap(1, 2);
                                }
                            }
                            for tri in indices {
                                let points = tri.map(|i| points[i]);
                                let a = sub(points[1], points[0]);
                                let b = sub(points[2], points[0]);
                                if norm(cross(a, b)) < 0.000001 {
                                    continue;
                                }
                                if mesh.triangles.len() >= LIMIT {
                                    return Err(crate::invalid(
                                        "立体が複雑すぎます。画像の寸法または厚みを減らすか、丸みを0にしてください（上限200万三角形）",
                                    ));
                                }
                                mesh.triangles.push(Triangle { points, color });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(mesh)
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f32; 3]) -> f32 {
    a.iter().map(|v| v * v).sum::<f32>().sqrt()
}
fn rotate([x, y, z]: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (rx, rz) = (x * cy - z * sy, x * sy + z * cy);
    [rx, y * cp - rz * sp, y * sp + rz * cp]
}
/// Orthographic, depth-tested CPU renderer. Independent of Win32 and GPU drivers.
pub fn render(
    mesh: &Mesh,
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    zoom: f32,
) -> io::Result<crate::image::Raster> {
    if width == 0
        || height == 0
        || width > 2048
        || height > 2048
        || !yaw.is_finite()
        || !pitch.is_finite()
        || !zoom.is_finite()
        || zoom <= 0.
    {
        return Err(crate::invalid("プレビューの寸法・カメラが不正です"));
    }
    let mut image = crate::image::Raster {
        width,
        height,
        pixels: vec![[232, 237, 245, 255]; (width * height) as usize],
    };
    let mut depth = vec![f32::INFINITY; image.pixels.len()];
    let extent = mesh.size.iter().copied().max().unwrap_or(1) as f32;
    let scale = width.min(height) as f32 * 0.67 / extent * zoom;
    for triangle in &mesh.triangles {
        let vertices = triangle.points.map(|p| {
            let p = std::array::from_fn(|i| p[i] - mesh.size[i] as f32 / 2.);
            let [x, y, z] = rotate(p, yaw, pitch);
            [
                width as f32 / 2. + x * scale,
                height as f32 / 2. - y * scale,
                z,
            ]
        });
        let normal = rotate(
            cross(
                sub(triangle.points[1], triangle.points[0]),
                sub(triangle.points[2], triangle.points[0]),
            ),
            yaw,
            pitch,
        );
        if normal[2] >= 0. {
            continue;
        }
        let light = (0.32
            + 0.68
                * ((-0.3 * normal[0] + 0.6 * normal[1] - 0.74 * normal[2]) / norm(normal)).max(0.))
        .min(1.);
        let color = [
            (f32::from(triangle.color[0]) * light) as u8,
            (f32::from(triangle.color[1]) * light) as u8,
            (f32::from(triangle.color[2]) * light) as u8,
            255,
        ];
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
            (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
        };
        let area = edge(vertices[0], vertices[1], vertices[2][0], vertices[2][1]);
        if area.abs() < 0.0001 {
            continue;
        }
        let minx = vertices
            .iter()
            .map(|v| v[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.) as u32;
        let maxx = vertices
            .iter()
            .map(|v| v[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(width as f32) as u32;
        let miny = vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.) as u32;
        let maxy = vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(height as f32) as u32;
        for y in miny..maxy {
            for x in minx..maxx {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let a = edge(vertices[1], vertices[2], fx, fy) / area;
                let b = edge(vertices[2], vertices[0], fx, fy) / area;
                let c = 1. - a - b;
                if a < 0. || b < 0. || c < 0. {
                    continue;
                }
                let z = a * vertices[0][2] + b * vertices[1][2] + c * vertices[2][2];
                let i = (y * width + x) as usize;
                if z < depth[i] {
                    depth[i] = z;
                    image.pixels[i] = color;
                }
            }
        }
    }
    Ok(image)
}
pub fn export_obj(path: &Path, mesh: &Mesh) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let materials: BTreeMap<_, _> = mesh.triangles.iter().map(|t| (t.color, ())).collect();
    let materials: BTreeMap<_, _> = materials.keys().enumerate().map(|(i, c)| (*c, i)).collect();
    // A unique ASCII name avoids OBJ token ambiguity and never overwrites another
    // model's material library. Persist the library before the referencing OBJ.
    let mut mtl = tempfile::Builder::new()
        .prefix("yy3-")
        .suffix(".mtl")
        .tempfile_in(parent)?;
    for (color, id) in &materials {
        writeln!(
            mtl,
            "newmtl color{id}\nKd {:.6} {:.6} {:.6}\nd 1\n",
            f32::from(color[0]) / 255.,
            f32::from(color[1]) / 255.,
            f32::from(color[2]) / 255.
        )?;
    }
    mtl.as_file().sync_all()?;
    let name = mtl
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut obj = tempfile::NamedTempFile::new_in(parent)?;
    writeln!(
        obj,
        "# YY3Designer; right handed, Y up, one unit per pixel\nmtllib {name}"
    )?;
    let mut material = None;
    for (i, t) in mesh.triangles.iter().enumerate() {
        let id = materials[&t.color];
        if material != Some(id) {
            writeln!(obj, "usemtl color{id}")?;
            material = Some(id);
        }
        for p in t.points {
            writeln!(obj, "v {} {} {}", p[0], p[1], p[2])?;
        }
        writeln!(obj, "f {} {} {}", i * 3 + 1, i * 3 + 2, i * 3 + 3)?;
    }
    obj.as_file().sync_all()?;
    let (_, mtl_path) = mtl.keep().map_err(|e| e.error)?;
    if let Err(e) = obj.persist(path) {
        let _ = std::fs::remove_file(mtl_path);
        return Err(e.error);
    }
    Ok(())
}
