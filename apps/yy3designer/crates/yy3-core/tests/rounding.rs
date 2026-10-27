use std::collections::{BTreeMap, BTreeSet};
use yy3_core::{image, mesh, *};

fn shape(size: [u32; 3], cells: &[[u32; 3]]) -> Object {
    let mut object = Object::new("接続試験", size).unwrap();
    object.views[0].as_mut().unwrap().layers[0]
        .pixels
        .fill([90, 160, 220, 255]);
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                if !cells.contains(&[x, y, z]) {
                    object.carved.push((z * size[1] + y) * size[0] + x);
                }
            }
        }
    }
    object
}

fn point(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v * 100_000.).round() as i32)
}

fn assert_closed_connected(mesh: &mesh::Mesh) {
    let mut edges = BTreeMap::new();
    let mut adjacent = BTreeMap::<_, BTreeSet<_>>::new();
    for triangle in &mesh.triangles {
        let p = triangle.points.map(point);
        for (a, b) in [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])] {
            assert_ne!(a, b);
            let edge = if a < b { (a, b) } else { (b, a) };
            *edges.entry(edge).or_insert(0) += 1;
            adjacent.entry(a).or_default().insert(b);
            adjacent.entry(b).or_default().insert(a);
        }
    }
    assert!(
        edges.values().all(|&count| count == 2),
        "open/nonmanifold edges: {:?}",
        edges.iter().filter(|(_, n)| **n != 2).collect::<Vec<_>>()
    );
    let mut pending = vec![*adjacent.keys().next().unwrap()];
    let mut seen = BTreeSet::new();
    while let Some(p) = pending.pop() {
        if seen.insert(p) {
            pending.extend(adjacent[&p].iter().copied());
        }
    }
    assert_eq!(
        seen.len(),
        adjacent.len(),
        "the solid split into separate shells"
    );
}

#[test]
fn rounding_keeps_joins_closed_along_all_axes_and_at_steps() {
    let cases = [
        vec![[0, 0, 0], [1, 0, 0], [2, 0, 0]],
        vec![[0, 0, 0], [0, 1, 0], [0, 2, 0]],
        vec![[0, 0, 0], [0, 0, 1], [0, 0, 2]],
        vec![[0, 0, 0], [1, 0, 0], [1, 1, 0]],
        vec![[0, 0, 0], [1, 0, 0], [2, 0, 0], [1, 1, 0]],
        vec![[0, 0, 0], [1, 0, 0], [1, 1, 0], [1, 1, 1]],
    ];
    for cells in cases {
        for radius in [0.2, 0.7, 1.] {
            let mut object = shape([3, 3, 3], &cells);
            object.roundness = radius;
            let mesh = mesh::build(&object).unwrap();
            assert_eq!(mesh.voxels, cells.len());
            assert_closed_connected(&mesh);
        }
    }
}

#[test]
fn joins_have_no_internal_caps_or_grooves_and_keep_original_color() {
    let mut object = shape([3, 1, 1], &[[0, 0, 0], [1, 0, 0], [2, 0, 0]]);
    object.roundness = 1.;
    let mesh = mesh::build(&object).unwrap();
    for join in [1., 2.] {
        assert!(
            !mesh
                .triangles
                .iter()
                .any(|t| t.points.iter().all(|p| (p[0] - join).abs() < 0.00001)),
            "internal cap at {join}"
        );
        for t in &mesh.triangles {
            for p in t.points.iter().filter(|p| (p[0] - join).abs() < 0.00001) {
                // A rounded rectangular bar has a constant cross section here.
                let dy = p[1] - 0.5;
                let dz = p[2] - 0.5;
                assert!((dy * dy + dz * dz - 0.25).abs() < 0.00001);
            }
            assert_eq!(t.color, [90, 160, 220, 255]);
        }
    }
}

#[test]
fn rounded_png_has_no_transparent_seams_between_adjacent_pixels() {
    let object = shape([3, 2, 1], &[[0, 0, 0]]);
    let view = object.views[0].as_ref().unwrap();
    let raster = image::export_view(view, 8, 1.).unwrap();
    for x in [7, 8, 15, 16] {
        for y in 0..raster.height {
            assert_eq!(raster.pixels[(y * raster.width + x) as usize][3], 255);
        }
    }
    assert_eq!(raster.pixels[0][3], 0);
}

#[test]
fn editable_examples_keep_six_views_and_closed_joined_meshes() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../demo/yy3designer");
    let all = load(&path.join("examples.yy3d")).unwrap();
    assert_eq!(all.objects.len(), 6);
    for (i, name) in [
        "chair",
        "table",
        "child",
        "car",
        "cat",
        "london-clock-tower",
    ]
    .iter()
    .enumerate()
    {
        let individual = load(&path.join(format!("{name}.yy3d"))).unwrap();
        assert_eq!(individual.objects, vec![all.objects[i].clone()]);
        let object = individual.object();
        assert!(object.views.iter().all(Option::is_some));
        let mesh = mesh::build(object).unwrap();
        assert!(mesh.voxels > 100);
        assert_closed_connected(&mesh);
    }
}
