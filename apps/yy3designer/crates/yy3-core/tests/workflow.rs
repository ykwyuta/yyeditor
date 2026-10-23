use yy3_core::{image::*, mesh::*, *};
fn solid(size: [u32; 3]) -> Object {
    let mut o = Object::new("テスト", size).unwrap();
    o.views[0].as_mut().unwrap().layers[0]
        .pixels
        .fill([240, 80, 32, 255]);
    o
}
#[test]
fn one_view_extrudes_and_six_views_intersect() {
    let mut o = solid([3, 2, 4]);
    assert_eq!(reconstruct(&o).unwrap().count(), 24);
    for dir in Direction::ALL.into_iter().skip(1) {
        o.views[dir.index()] = Some(projected_view(&o, dir).unwrap());
    }
    assert_eq!(reconstruct(&o).unwrap().count(), 24);
    let right = o.views[Direction::Right.index()].as_mut().unwrap();
    right.layers[0].pixels[0] = CLEAR;
    assert_eq!(reconstruct(&o).unwrap().count(), 21);
    let v = reconstruct(&o).unwrap();
    for x in 0..3 {
        assert_eq!(v.get([x, 1, 0]), CLEAR);
    }
}
#[test]
fn opposite_directions_have_outside_camera_orientation() {
    let size = [3, 4, 5];
    assert_eq!(Direction::Front.project([0, 0, 0], size), [0, 3]);
    assert_eq!(Direction::Back.project([0, 0, 0], size), [2, 3]);
    assert_eq!(Direction::Left.project([0, 0, 0], size), [4, 3]);
    assert_eq!(Direction::Right.project([0, 0, 0], size), [0, 3]);
    assert_eq!(Direction::Top.project([0, 0, 0], size), [0, 0]);
    assert_eq!(Direction::Bottom.project([0, 0, 0], size), [0, 4]);
}
#[test]
fn layer_visibility_opacity_and_alpha_cutoff_control_geometry() {
    let mut o = solid([1, 1, 2]);
    let v = o.views[0].as_mut().unwrap();
    v.layers[0].opacity = 100;
    assert_eq!(reconstruct(&o).unwrap().count(), 0);
    o.alpha_cutoff = 80;
    assert_eq!(reconstruct(&o).unwrap().count(), 2);
    o.views[0].as_mut().unwrap().layers[0].visible = false;
    assert_eq!(reconstruct(&o).unwrap().count(), 0);
    assert_eq!(
        over([0, 0, 255, 255], [255, 0, 0, 128], 255),
        [128, 0, 127, 255]
    );
}
#[test]
fn painting_clipping_shapes_and_fill_preserve_boundaries() {
    let mut v = View::new(8, 8);
    let c = [10, 20, 30, 255];
    shape(&mut v, Tool::Rectangle, [[1, 1], [6, 6]], c, 1, false, None);
    fill(&mut v, [3, 3], [200, 200, 200, 255], None);
    assert_eq!(v.layers[0].pixels[0], CLEAR);
    assert_eq!(v.layers[0].pixels[3 * 8 + 3], [200, 200, 200, 255]);
    let clip = Rect {
        x: 2,
        y: 2,
        w: 2,
        h: 2,
    };
    line(&mut v, [0, 0], [7, 7], c, 1, Some(clip));
    assert_eq!(v.layers[0].pixels[2 * 8 + 2], c);
    assert_eq!(v.layers[0].pixels[4 * 8 + 4], [200, 200, 200, 255]);
    shape(&mut v, Tool::Ellipse, [[0, 0], [7, 7]], c, 1, true, None);
    assert_eq!(v.layers[0].pixels[0], CLEAR);
}
#[test]
fn selection_copy_paste_flip_and_rotation() {
    let mut v = View::new(3, 3);
    let red = [255, 0, 0, 255];
    put(&mut v, 0, 0, red, None);
    let r = Rect {
        x: 0,
        y: 0,
        w: 2,
        h: 2,
    };
    let clip = Clip::capture(&v, r);
    clip.paste(&mut v, [1, 1]);
    assert_eq!(v.layers[0].pixels[4], red);
    flip(&mut v, true, r);
    assert_eq!(v.layers[0].pixels[1], red);
    rotate(&mut v, r);
    assert_eq!(v.layers[0].pixels[4], red);
}
#[test]
fn history_groups_strokes_and_rolls_back_invalid_edits() {
    let mut e = Editor::default();
    let original = e.project.clone();
    e.change(|p| {
        p.object_mut().views[0].as_mut().unwrap().layers[0].pixels[0] = [1, 2, 3, 255];
        Ok(())
    })
    .unwrap();
    let changed = e.project.clone();
    assert!(e.undo());
    assert_eq!(e.project, original);
    assert!(e.redo());
    assert_eq!(e.project, changed);
    assert!(
        e.change(|p| {
            p.active = 99;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(e.project, changed);
    assert!(!e.undo() || e.project == original);
}
#[test]
fn resizing_updates_each_orthographic_plane_and_is_undoable() {
    let mut o = solid([2, 3, 4]);
    o.views[4] = Some(projected_view(&o, Direction::Top).unwrap());
    o.resize([4, 5, 6], true).unwrap();
    assert_eq!(o.views[0].as_ref().unwrap().layers[0].pixels.len(), 20);
    assert_eq!(
        [
            o.views[4].as_ref().unwrap().width,
            o.views[4].as_ref().unwrap().height
        ],
        [4, 6]
    );
    assert!(o.resize([0, 1, 1], true).is_err());
    assert!(validate_size([256, 256, 256]).is_err());
}
#[test]
fn lossless_project_roundtrip_includes_all_six_views_and_layers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("日本語 with space.yy3d");
    let mut p = Project::default();
    p.objects[0] = solid([3, 4, 5]);
    for d in Direction::ALL {
        p.object_mut().views[d.index()] = Some(projected_view(p.object(), d).unwrap());
    }
    p.object_mut().roundness = 0.7;
    let v = p.object_mut().views[0].as_mut().unwrap();
    v.layers.push(Layer {
        name: "非表示の修正".into(),
        visible: false,
        opacity: 73,
        pixels: vec![[1, 2, 3, 4]; (v.width * v.height) as usize],
    });
    v.active = 1;
    p.objects
        .push(Object::new("別のオブジェクト", [2, 3, 4]).unwrap());
    p.palette.push([1, 2, 3, 4]);
    save(&path, &p).unwrap();
    assert_eq!(load(&path).unwrap(), p);
    p.object_mut().name = "変更後".into();
    save(&path, &p).unwrap();
    assert_eq!(load(&path).unwrap(), p);
    let mut bytes = storage::encode(&p).unwrap();
    bytes[0] ^= 1;
    assert!(storage::decode(&bytes).is_err());
    assert!(storage::decode(b"short").is_err());
    let mut bytes = storage::encode(&p).unwrap();
    bytes.truncate(bytes.len() / 2);
    assert!(storage::decode(&bytes).is_err());
}
#[test]
fn rounded_cells_have_no_open_edges_and_faces_use_the_nearest_view_color() {
    use std::collections::BTreeMap;
    let mut o = solid([1, 1, 1]);
    o.roundness = 0.7;
    let mesh = build(&o).unwrap();
    let mut edges = BTreeMap::new();
    for triangle in &mesh.triangles {
        let p = triangle
            .points
            .map(|p| p.map(|f| (f * 1_000_000.).round() as i32));
        for i in 0..3 {
            let mut edge = [p[i], p[(i + 1) % 3]];
            edge.sort();
            *edges.entry(edge).or_insert(0) += 1;
        }
    }
    assert!(edges.values().all(|&n| n == 2));
    o.resize([1, 1, 3], true).unwrap();
    let mut back = View::new(1, 1);
    back.layers[0].pixels[0] = [20, 70, 230, 255];
    o.views[1] = Some(back);
    let volume = reconstruct(&o).unwrap();
    assert_eq!(volume.get([0, 0, 0]), [240, 80, 32, 255]);
    assert_eq!(volume.get([0, 0, 2]), [20, 70, 230, 255]);
}
#[test]
fn area_sampling_respects_alpha_and_palette_and_dither_are_deterministic() {
    let raster = Raster {
        width: 2,
        height: 1,
        pixels: vec![[255, 0, 0, 255], [0, 0, 255, 0]],
    };
    let (p, _) = convert(
        &raster,
        1,
        1,
        &Convert {
            sampling: Sampling::Area,
            colors: 0,
            dither: false,
        },
    )
    .unwrap();
    assert_eq!(p[0], [255, 0, 0, 128]);
    let source = Raster {
        width: 32,
        height: 1,
        pixels: (0..32).map(|n| [n * 8, n * 8, n * 8, 255]).collect(),
    };
    let options = Convert {
        sampling: Sampling::Area,
        colors: 4,
        dither: true,
    };
    let (a, palette) = convert(&source, 32, 1, &options).unwrap();
    assert!(palette.len() <= 4);
    assert!(a.iter().all(|p| palette.contains(p)));
    assert_eq!(a, convert(&source, 32, 1, &options).unwrap().0);
}
#[test]
fn png_roundtrip_preserves_transparency_and_rounded_export_preserves_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.png");
    let o = solid([2, 2, 1]);
    let v = o.views[0].as_ref().unwrap();
    let raw = export_view(v, 8, 0.).unwrap();
    let rounded = export_view(v, 8, 1.).unwrap();
    assert_eq!(raw.pixels[0][3], 255);
    assert_eq!(rounded.pixels[0][3], 0);
    write_png(&path, &rounded).unwrap();
    assert_eq!(read_png(&path).unwrap().pixels, rounded.pixels);
    assert_eq!(v.layers[0].pixels[0][3], 255);
}
#[test]
fn cube_mesh_has_outward_normals_and_internal_faces_are_removed() {
    let mut o = solid([1, 1, 1]);
    let cube = build(&o).unwrap();
    assert_eq!(cube.triangles.len(), 12);
    for t in &cube.triangles {
        let [a, b, c] = t.points;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        assert!((0..3).map(|i| n[i] * (a[i] - 0.5)).sum::<f32>() > 0.);
    }
    o.resize([2, 1, 1], true).unwrap();
    assert_eq!(build(&o).unwrap().triangles.len(), 20);
    o.roundness = 0.7;
    let round = build(&o).unwrap();
    assert!(round.triangles.len() > 20);
    assert!(
        round
            .triangles
            .iter()
            .flat_map(|t| t.points)
            .all(|p| p.iter().all(|n| n.is_finite()))
    );
    let preview = render(&round, 160, 120, 0.6, 0.4, 1.).unwrap();
    assert!(preview.pixels.iter().any(|p| *p != [232, 237, 245, 255]));
}
#[test]
fn obj_export_references_existing_materials_and_valid_faces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("日本語 test.obj");
    let m = build(&solid([2, 2, 2])).unwrap();
    export_obj(&path, &m).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let lib = text
        .lines()
        .find_map(|l| l.strip_prefix("mtllib "))
        .unwrap();
    assert!(dir.path().join(lib).is_file());
    assert_eq!(
        text.lines().filter(|l| l.starts_with("v ")).count(),
        m.triangles.len() * 3
    );
    assert_eq!(
        text.lines().filter(|l| l.starts_with("f ")).count(),
        m.triangles.len()
    );
}
