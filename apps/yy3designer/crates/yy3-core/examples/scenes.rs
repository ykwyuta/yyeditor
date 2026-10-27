//! Procedural, original sample models. Coordinates are X right, Y up, Z back.
use std::{io, path::Path};
use yy3_core::{image, mesh, *};

struct Model {
    size: [u32; 3],
    cells: Vec<Color>,
}
impl Model {
    fn new(size: [u32; 3]) -> Self {
        Self {
            size,
            cells: vec![CLEAR; size.iter().product::<u32>() as usize],
        }
    }
    fn box_at(&mut self, low: [u32; 3], high: [u32; 3], color: Color) {
        for z in low[2]..high[2] {
            for y in low[1]..high[1] {
                for x in low[0]..high[0] {
                    let i = ((z * self.size[1] + y) * self.size[0] + x) as usize;
                    self.cells[i] = color;
                }
            }
        }
    }
    fn facade_pixel(&mut self, side: usize, u: u32, y: u32, color: Color) {
        let p = match side {
            0 => [u, y, 6],
            1 => [41, y, u],
            2 => [47 - u, y, 41],
            _ => [6, y, 47 - u],
        };
        self.box_at(p, p.map(|v| v + 1), color);
    }
    fn facade_line(&mut self, side: usize, a: [u32; 2], b: [u32; 2], color: Color) {
        let steps = a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = std::array::from_fn::<_, 2, _>(|j| {
                (a[j] as f32 * (1. - t) + b[j] as f32 * t).round() as u32
            });
            self.facade_pixel(side, p[0], p[1], color);
        }
    }
    fn object(self, name: &str, roundness: f32) -> io::Result<Object> {
        let mut object = Object::new(name, self.size)?;
        object.roundness = roundness;
        for direction in Direction::ALL {
            let [w, h] = direction.size(self.size);
            let mut view = View::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    for t in 0..direction.ray_length(self.size) {
                        let [vx, vy, vz] = direction.ray_point([x, y], t, self.size);
                        let color =
                            self.cells[((vz * self.size[1] + vy) * self.size[0] + vx) as usize];
                        if color[3] != 0 {
                            view.layers[0].pixels[(y * w + x) as usize] = color;
                            break;
                        }
                    }
                }
            }
            object.views[direction.index()] = Some(view);
        }
        // Silhouettes cannot describe every recess. Preserve the exact model
        // through the same editable depth-cut mask used by the application's brush.
        let hull = mesh::reconstruct(&object)?;
        object.carved = hull
            .cells
            .iter()
            .zip(&self.cells)
            .enumerate()
            .filter(|(_, (h, c))| h[3] != 0 && c[3] == 0)
            .map(|(i, _)| i as u32)
            .collect();
        Ok(object)
    }
}
const WOOD: Color = [155, 91, 47, 255];
const LIGHT_WOOD: Color = [210, 155, 87, 255];
const DARK: Color = [40, 47, 62, 255];
const SKIN: Color = [249, 189, 139, 255];

fn chair() -> io::Result<Object> {
    let mut m = Model::new([24, 28, 24]);
    for x in [4, 17] {
        for z in [4, 17] {
            m.box_at([x, 0, z], [x + 3, 10, z + 3], WOOD);
        }
    }
    m.box_at([3, 10, 3], [21, 13, 21], LIGHT_WOOD);
    for x in [4, 17] {
        m.box_at([x, 13, 18], [x + 3, 27, 21], WOOD);
    }
    for y in [17, 23] {
        m.box_at([4, y, 18], [20, y + 3, 21], LIGHT_WOOD);
    }
    m.object("椅子", 0.45)
}
fn table() -> io::Result<Object> {
    let mut m = Model::new([32, 22, 24]);
    for x in [3, 26] {
        for z in [3, 18] {
            m.box_at([x, 0, z], [x + 3, 17, z + 3], WOOD);
        }
    }
    m.box_at([2, 14, 3], [30, 17, 6], WOOD);
    m.box_at([2, 14, 18], [30, 17, 21], WOOD);
    m.box_at([1, 17, 1], [31, 20, 23], LIGHT_WOOD);
    m.object("テーブル", 0.35)
}
fn child() -> io::Result<Object> {
    let mut m = Model::new([24, 34, 16]);
    for x in [6, 13] {
        m.box_at([x, 1, 5], [x + 5, 11, 11], [48, 91, 158, 255]);
        m.box_at([x, 0, 2], [x + 5, 3, 11], DARK);
    }
    m.box_at([5, 11, 4], [19, 21, 12], [243, 170, 43, 255]);
    for x in [2, 19] {
        m.box_at([x, 14, 5], [x + 3, 20, 11], [243, 170, 43, 255]);
        m.box_at([x, 10, 5], [x + 3, 15, 11], SKIN);
    }
    m.box_at([10, 21, 6], [14, 24, 10], SKIN);
    m.box_at([6, 23, 3], [18, 32, 13], SKIN);
    m.box_at([5, 30, 3], [19, 34, 14], [83, 53, 39, 255]);
    m.box_at([5, 26, 10], [7, 32, 14], [83, 53, 39, 255]);
    m.box_at([17, 26, 10], [19, 32, 14], [83, 53, 39, 255]);
    for x in [8, 14] {
        m.box_at([x, 27, 2], [x + 2, 29, 4], DARK);
    }
    m.box_at([10, 25, 2], [14, 26, 4], [187, 83, 76, 255]);
    m.object("子供", 0.65)
}
fn car() -> io::Result<Object> {
    let mut m = Model::new([22, 18, 36]);
    let red = [210, 55, 61, 255];
    m.box_at([3, 4, 2], [19, 10, 34], red);
    m.box_at([5, 10, 10], [17, 15, 26], red);
    m.box_at([6, 15, 12], [16, 17, 24], red);
    let glass = [105, 195, 223, 255];
    m.box_at([6, 11, 9], [16, 15, 11], glass);
    m.box_at([6, 11, 25], [16, 15, 27], glass);
    for x in [4, 17] {
        m.box_at([x, 11, 12], [x + 1, 15, 24], glass);
        m.box_at([x, 10, 17], [x + 1, 16, 19], red);
    }
    for x in [1, 18] {
        for z in [6, 25] {
            m.box_at([x, 2, z], [x + 3, 7, z + 5], DARK);
            m.box_at([x, 3, z + 1], [x + 3, 6, z + 4], [170, 182, 195, 255]);
        }
    }
    for x in [4, 14] {
        m.box_at([x, 6, 1], [x + 4, 9, 3], [255, 239, 173, 255]);
        m.box_at([x, 6, 33], [x + 4, 9, 35], [151, 24, 44, 255]);
    }
    m.box_at([4, 4, 1], [18, 6, 3], [170, 182, 195, 255]);
    m.object("車", 0.6)
}
fn cat() -> io::Result<Object> {
    let mut m = Model::new([24, 27, 30]);
    let fur = [231, 149, 64, 255];
    let cream = [255, 222, 167, 255];
    m.box_at([6, 6, 8], [18, 16, 24], fur);
    for x in [6, 14] {
        for z in [7, 19] {
            m.box_at([x, 0, z], [x + 4, 9, z + 4], fur);
            m.box_at([x, 0, z], [x + 4, 2, z + 4], cream);
        }
    }
    m.box_at([5, 13, 3], [19, 23, 13], fur);
    for x in [5, 15] {
        m.box_at([x, 23, 6], [x + 4, 25, 11], fur);
        m.box_at([x + 1, 25, 7], [x + 3, 27, 10], fur);
    }
    m.box_at([7, 14, 2], [17, 18, 4], cream);
    for x in [7, 15] {
        m.box_at([x, 19, 2], [x + 2, 21, 4], [52, 106, 71, 255]);
    }
    m.box_at([11, 17, 1], [13, 19, 3], [202, 107, 112, 255]);
    m.box_at([10, 10, 23], [14, 14, 28], fur);
    m.box_at([10, 13, 25], [14, 21, 29], fur);
    m.box_at([10, 20, 24], [14, 23, 28], cream);
    m.object("猫", 0.7)
}

fn london_clock_tower() -> io::Result<Object> {
    let mut m = Model::new([48, 176, 48]);
    let stone = [199, 169, 113, 255];
    let light = [239, 215, 165, 255];
    let gold = [206, 155, 53, 255];
    let roof = [48, 76, 72, 255];
    let recess = [65, 74, 78, 255];
    let dial = [255, 247, 220, 255];
    let ink = [31, 43, 47, 255];
    // Stepped plinth, tall masonry shaft and the wider clock chamber.
    m.box_at([2, 0, 2], [46, 3, 46], light);
    m.box_at([4, 3, 4], [44, 7, 44], stone);
    m.box_at([7, 7, 7], [41, 108, 41], stone);
    m.box_at([6, 108, 6], [42, 139, 42], stone);
    for x in [5, 38] {
        for z in [5, 38] {
            m.box_at([x, 7, z], [x + 5, 145, z + 5], light);
            m.box_at([x - 1, 139, z - 1], [x + 6, 144, z + 6], gold);
            for y in 145..161 {
                let inset = ((y - 145) / 5).min(2);
                m.box_at(
                    [x + inset, y, z + inset],
                    [x + 5 - inset, y + 1, z + 5 - inset],
                    if y < 150 { light } else { roof },
                );
            }
            m.box_at([x + 2, 161, z + 2], [x + 3, 164, z + 3], gold);
        }
    }
    for side in 0..4 {
        for y in 8..108 {
            for u in 10..38 {
                let band = [16, 36, 56, 76, 96, 105].contains(&y);
                let pilaster = [11, 19, 27, 35].contains(&u);
                m.facade_pixel(side, u, y, if band || pilaster { light } else { stone });
            }
        }
        // Pointed lancet windows, with a stone mullion between the openings.
        for base in [20, 40, 60, 80] {
            for u in [14, 22, 30] {
                for y in base..base + 12 {
                    for x in u..u + 3 {
                        if y < base + 10 || x == u + 1 {
                            m.facade_pixel(side, x, y, recess);
                        }
                    }
                }
            }
        }
        // Each of the four faces has its own editable dial and hands at 10:10.
        for y in 109..138 {
            for u in 10..39 {
                let radius = (u as f32 - 24.).hypot(y as f32 - 123.);
                if radius <= 14. {
                    let color = if radius > 13. {
                        gold
                    } else if radius > 12. {
                        ink
                    } else {
                        dial
                    };
                    m.facade_pixel(side, u, y, color);
                }
            }
        }
        for hour in 0..12 {
            let angle = hour as f32 * std::f32::consts::TAU / 12.;
            let point = |r: f32| {
                [
                    (24. + r * angle.sin()).round() as u32,
                    (123. + r * angle.cos()).round() as u32,
                ]
            };
            m.facade_line(side, point(10.), point(11.), ink);
        }
        m.facade_line(side, [24, 123], [18, 127], ink);
        m.facade_line(side, [24, 124], [18, 128], ink);
        m.facade_line(side, [24, 123], [33, 128], ink);
        m.facade_pixel(side, 24, 123, gold);
    }
    // Cornices, steep slate roof, gilded ridges and central finial.
    m.box_at([4, 138, 4], [44, 141, 44], light);
    m.box_at([5, 141, 5], [43, 144, 43], gold);
    for y in 144..169 {
        let half = 13 - (y - 144) / 2;
        let low = 24 - half;
        let high = 24 + half;
        m.box_at([low, y, low], [high, y + 1, high], roof);
        for x in [low, high - 1] {
            for z in [low, high - 1] {
                m.box_at([x, y, z], [x + 1, y + 1, z + 1], gold);
            }
        }
    }
    m.box_at([22, 169, 22], [26, 173, 26], roof);
    m.box_at([23, 173, 23], [25, 176, 25], gold);
    m.object("ロンドンの時計塔（エリザベス・タワー）", 0.)
}

pub fn write(path: &Path) -> io::Result<()> {
    let models = [
        ("chair", chair()?),
        ("table", table()?),
        ("child", child()?),
        ("car", car()?),
        ("cat", cat()?),
        ("london-clock-tower", london_clock_tower()?),
    ];
    let mut collection = Project::default();
    collection.objects.clear();
    let mut sheet = image::Raster {
        width: 960,
        height: 640,
        pixels: vec![[232, 237, 245, 255]; 960 * 640],
    };
    for (i, (file, object)) in models.into_iter().enumerate() {
        let project = Project {
            objects: vec![object.clone()],
            ..Project::default()
        };
        save(&path.join(format!("{file}.yy3d")), &project)?;
        assert_eq!(load(&path.join(format!("{file}.yy3d")))?, project);
        let mesh = mesh::build(&object)?;
        let raster = mesh::render(&mesh, 320, 320, -0.65, -0.4, 1.1)?;
        image::write_png(&path.join(format!("{file}.png")), &raster)?;
        if file == "london-clock-tower" {
            let portrait = mesh::render(&mesh, 640, 960, -0.65, -0.15, 1.85)?;
            image::write_png(&path.join(format!("{file}.png")), &portrait)?;
        }
        for y in 0..320usize {
            let start = ((i / 3) * 320 + y) * 960 + (i % 3) * 320;
            sheet.pixels[start..start + 320]
                .copy_from_slice(&raster.pixels[y * 320..(y + 1) * 320]);
        }
        collection.objects.push(object);
    }
    save(&path.join("examples.yy3d"), &collection)?;
    image::write_png(&path.join("examples.png"), &sheet)?;
    Ok(())
}

// This helper is also compiled as a Cargo example target.
fn main() -> io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "demo/yy3designer".into());
    std::fs::create_dir_all(&path)?;
    write(&path)
}
