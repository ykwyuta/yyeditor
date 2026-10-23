use yy3_core::{mesh, sculpt, *};
fn solid(size: [u32; 3]) -> Object {
    let mut o = Object::new("厚み試験", size).unwrap();
    o.views[0].as_mut().unwrap().layers[0]
        .pixels
        .fill([80, 140, 220, 255]);
    o
}

#[test]
fn all_six_rays_project_to_the_requested_pixel_and_cover_the_volume() {
    let size = [3, 4, 5];
    for dir in Direction::ALL {
        let [w, h] = dir.size(size);
        let mut positions = std::collections::BTreeSet::new();
        for y in 0..h {
            for x in 0..w {
                for t in 0..dir.ray_length(size) {
                    let p = dir.ray_point([x, y], t, size);
                    assert_eq!(dir.project(p, size), [x, y]);
                    assert!(positions.insert(p));
                }
            }
        }
        assert_eq!(positions.len(), 60);
    }
}
#[test]
fn thinning_is_local_and_one_step_per_stroke_without_changing_the_pixel_art() {
    let mut o = solid([2, 2, 4]);
    let original = o.views.clone();
    let mut stroke = sculpt::Stroke::new(&o, Direction::Front, false).unwrap();
    stroke.apply(&mut o, [0, 0], [0, 0], 1, None);
    stroke.apply(&mut o, [0, 0], [0, 0], 1, None);
    assert_eq!(o.carved, vec![2]);
    assert_eq!(stroke.depths, vec![3, 4, 4, 4]);
    assert_eq!(mesh::reconstruct(&o).unwrap().count(), 15);
    let mut stroke = sculpt::Stroke::new(&o, Direction::Front, false).unwrap();
    stroke.apply(&mut o, [0, 0], [0, 0], 1, None);
    assert_eq!(o.carved, vec![2, 6]);
    assert_eq!(stroke.depths, vec![2, 4, 4, 4]);
    assert_eq!(o.views, original);
}
#[test]
fn each_direction_carves_its_near_surface_and_restoration_is_lossless() {
    for dir in Direction::ALL {
        let mut o = solid([3, 4, 5]);
        let original = mesh::reconstruct(&o).unwrap().cells;
        for _ in 0..dir.ray_length(o.size) + 2 {
            let mut stroke = sculpt::Stroke::new(&o, dir, false).unwrap();
            stroke.apply(&mut o, [0, 0], [0, 0], 1, None);
        }
        assert_eq!(o.carved.len(), dir.ray_length(o.size) as usize);
        assert_eq!(sculpt::depths(&o, dir).unwrap()[0], 0);
        let near = dir.ray_point([0, 0], 0, o.size);
        assert_eq!(mesh::reconstruct(&o).unwrap().get(near), CLEAR);
        let mut restore = sculpt::Stroke::new(&o, dir, true).unwrap();
        restore.apply(&mut o, [0, 0], [0, 0], 1, None);
        assert!(o.carved.is_empty());
        assert_eq!(mesh::reconstruct(&o).unwrap().cells, original);
    }
}
#[test]
fn overlapping_brush_dabs_respect_the_selection_and_never_carve_twice() {
    let mut o = solid([5, 5, 4]);
    let mut stroke = sculpt::Stroke::new(&o, Direction::Front, false).unwrap();
    let clip = Some(Rect {
        x: 1,
        y: 1,
        w: 2,
        h: 2,
    });
    stroke.apply(&mut o, [0, 0], [4, 4], 3, clip);
    stroke.apply(&mut o, [4, 4], [0, 0], 3, clip);
    assert_eq!(o.carved.len(), 4);
    assert_eq!(mesh::reconstruct(&o).unwrap().count(), 96);
    for y in 0..5 {
        for x in 0..5 {
            assert_eq!(
                stroke.depths[y * 5 + x],
                if (1..3).contains(&x) && (1..3).contains(&y) {
                    3
                } else {
                    4
                }
            );
        }
    }
}
#[test]
fn carving_roundtrips_with_history_and_old_projects_still_load() {
    let mut editor = Editor::new(Project {
        objects: vec![solid([3, 3, 4])],
        ..Project::default()
    });
    let before = editor.project.clone();
    editor
        .change(|p| {
            let mut stroke = sculpt::Stroke::new(p.object(), Direction::Front, false)?;
            stroke.apply(p.object_mut(), [1, 1], [1, 1], 1, None);
            Ok(())
        })
        .unwrap();
    let after = editor.project.clone();
    assert!(editor.undo());
    assert_eq!(editor.project, before);
    assert!(editor.redo());
    assert_eq!(editor.project, after);
    assert_eq!(
        storage::decode(&storage::encode(&after).unwrap()).unwrap(),
        after
    );
    let old = serde_json::to_value(&before).unwrap();
    assert!(old["objects"][0].get("carved").is_none());
    let loaded: Project = serde_json::from_value(old).unwrap();
    loaded.validate().unwrap();
    assert!(loaded.object().carved.is_empty());
    let mut bad = after.clone();
    bad.object_mut().carved = vec![u32::MAX];
    assert!(bad.validate().is_err());
    bad.object_mut().carved = vec![1, 1];
    assert!(bad.validate().is_err());
}
#[test]
fn resizing_scales_or_crops_carving_along_with_the_pixels() {
    let mut o = solid([2, 2, 2]);
    let mut stroke = sculpt::Stroke::new(&o, Direction::Front, false).unwrap();
    stroke.apply(&mut o, [0, 0], [0, 0], 1, None);
    let mut scaled = o.clone();
    scaled.resize([4, 4, 4], true).unwrap();
    assert_eq!(scaled.carved.len(), 8);
    assert_eq!(mesh::reconstruct(&scaled).unwrap().count(), 56);
    o.resize([2, 3, 2], false).unwrap();
    assert_eq!(o.carved, vec![4]);
    assert_eq!(mesh::reconstruct(&o).unwrap().count(), 7);
}
