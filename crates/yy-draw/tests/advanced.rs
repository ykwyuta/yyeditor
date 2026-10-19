use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}
#[test]
fn cylinder_cap_and_body_have_continuous_fill_at_all_sizes() {
    let renderer = Renderer::new(None);
    for (w, h) in [(80., 80.), (120., 40.), (32., 120.)] {
        let mut doc = Document::default();
        doc.add_node(Shape::Cylinder, p(10., 10.), None);
        let n = &mut doc.nodes[0];
        n.rect = Rect {
            x: 10.,
            y: 10.,
            w,
            h,
        };
        n.label.clear();
        n.fill = 0x28b976;
        n.stroke = 0x28b976;
        let options = Export {
            bounds: Rect {
                x: 0.,
                y: 0.,
                w: w + 20.,
                h: h + 20.,
            },
            background: None,
            grid: false,
            text: false,
            selected: vec![],
        };
        let image = renderer.raster(&doc, &options, 1.).unwrap();
        for y in 12..(10. + h - 2.) as u32 {
            let pixel = image.pixel((10. + w / 2.) as u32, y).unwrap();
            assert_eq!(pixel.alpha(), 255, "unfilled cylinder at {w}x{h}, y={y}");
            assert_eq!(
                (pixel.red(), pixel.green(), pixel.blue()),
                (0x28, 0xb9, 0x76)
            );
        }
    }
}
#[test]
fn routes_avoid_nodes_and_preserve_manual_points() {
    let mut d = Document::default();
    let obstacle = d.add_node(Shape::Rectangle, p(160., 0.), None);
    d.nodes.last_mut().unwrap().rect.h = 160.;
    let id = d.connect(
        Endpoint::Free(p(0., 40.)),
        Endpoint::Free(p(480., 40.)),
        true,
    );
    let route = router::Router::new(&d).route(&d.connectors[0]);
    assert!(route.resolved);
    let bounds = d.node(obstacle).unwrap().rect.expand(8.);
    assert!(
        route
            .points
            .windows(2)
            .all(|s| !router::blocked(s[0], s[1], bounds))
    );
    d.connectors[0].waypoints = vec![p(80., -80.), p(400., -80.)];
    let r = geometry::route(&d, &d.connectors[0]);
    assert!(r.iter().any(|q| q.y == -80.));
    editing::add_waypoint(&mut d, id, p(360., -80.)).unwrap();
    assert_eq!(d.connectors[0].waypoints.len(), 3);
    assert!(editing::remove_waypoint(&mut d, id, p(360., -80.), 10.));
    d.connectors[0].waypoints = vec![p(200., 40.)];
    assert!(!router::Router::new(&d).route(&d.connectors[0]).resolved);
}
#[test]
fn explicit_junction_splits_crossing_without_a_jump() {
    let mut d = Document::default();
    d.connect(
        Endpoint::Free(p(0., 80.)),
        Endpoint::Free(p(160., 80.)),
        false,
    );
    d.connect(
        Endpoint::Free(p(80., 0.)),
        Endpoint::Free(p(80., 160.)),
        false,
    );
    let id = editing::junction(&mut d, p(80., 80.), 8.);
    assert_eq!(d.connectors.len(), 4);
    assert!(
        d.connectors
            .iter()
            .all(|c| matches!(c.start,Endpoint::Attached{node,..}if node==id)
                || matches!(c.end,Endpoint::Attached{node,..}if node==id))
    );
    assert!(
        geometry::connector_paths(&d)
            .iter()
            .all(|p| !p.contains(" C "))
    );
    d.validate().unwrap();
}
#[test]
fn guides_snap_edges_and_centers() {
    let mut d = Document::default();
    let a = d.add_node(Shape::Rectangle, p(0., 0.), None);
    d.add_node(Shape::Rectangle, p(200., 160.), None);
    let (delta, guides) = editing::align(&d, &[a], p(198., 157.), 5.);
    assert_eq!(delta, p(200., 160.));
    assert_eq!(guides.len(), 2);
}
#[test]
fn pages_roundtrip_and_legacy_files_migrate() {
    let legacy = format::decode(include_bytes!("data/legacy.yyd")).unwrap();
    assert_eq!(legacy.pages.len(), 1);
    assert_eq!(legacy.nodes.len(), 10);
    let mut d = legacy.clone();
    let first = d.nodes.clone();
    d.add_page("別ページ".into()).unwrap();
    let id = d.add_node(Shape::Ellipse, p(800., 800.), None);
    d.pages[1].width = 1600.;
    d.pages[1].height = 1200.;
    let second = d.nodes.clone();
    d.switch_page(0).unwrap();
    assert_eq!(d.nodes, first);
    d.switch_page(1).unwrap();
    assert_eq!(d.nodes, second);
    assert_eq!(d.page_bounds().w, 1600.);
    d.duplicate_page().unwrap();
    assert_ne!(d.nodes[0].id, id);
    assert_eq!(d.nodes[0].rect, second[0].rect);
    d.validate().unwrap();
    assert_eq!(format::decode(&format::encode(&d).unwrap()).unwrap(), d);
    d.delete_page().unwrap();
    assert_eq!(d.pages.len(), 2);
    d.switch_page(0).unwrap();
    assert_eq!(d.nodes, first);
}
#[test]
fn optional_chunks_survive_but_required_unknown_chunks_do_not() {
    let mut d = Document::default();
    d.extra_chunks.push(format::Chunk {
        kind: *b"TEST",
        flags: 0,
        id: 7,
        data: vec![1, 2, 3, 4],
    });
    let bytes = format::encode(&d).unwrap();
    assert_eq!(format::decode(&bytes).unwrap(), d);
    let mut required = bytes;
    let flags = 52 + 4 + 2 * 64 + 4;
    required[flags..flags + 4].copy_from_slice(&1u32.to_le_bytes());
    let hash = blake3::hash(&required[52..]);
    required[20..52].copy_from_slice(hash.as_bytes());
    assert!(format::decode(&required).unwrap_err().contains("必須"));
}
#[test]
fn pack_versions_are_immutable_and_placed_icons_do_not_change() {
    let r = Renderer::new(None);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("icon.svg"),"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24'><rect width='24' height='24' fill='red'/></svg>").unwrap();
    let manifest = dir.path().join("pack.toml");
    let text = "id='custom'\nversion='1'\nsource='local'\nterms='example'\n[[icons]]\nname='Custom'\nfile='icon.svg'\n";
    std::fs::write(&manifest, text).unwrap();
    let v1 = library::import_manifest(&manifest, &r).unwrap();
    let mut lib = Document::default();
    library::install(&mut lib, &v1, true).unwrap();
    let mut diagram = lib.clone();
    diagram.add_node(Shape::Icon, p(0., 0.), Some(diagram.assets[0].id));
    let original = diagram.clone();
    std::fs::write(dir.path().join("icon.svg"),"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24'><circle cx='12' cy='12' r='12' fill='blue'/></svg>").unwrap();
    let changed = library::import_manifest(&manifest, &r).unwrap();
    assert!(library::install(&mut lib, &changed, true).is_err());
    std::fs::write(&manifest, text.replace("version='1'", "version='2'")).unwrap();
    let v2 = library::import_manifest(&manifest, &r).unwrap();
    library::install(&mut lib, &v2, true).unwrap();
    assert_eq!(lib.packs.len(), 2);
    assert!(!lib.packs[0].enabled);
    assert!(lib.packs[1].enabled);
    assert_eq!(diagram, original);
    library::activate_version(&mut lib, "custom", "1").unwrap();
    assert!(lib.packs[0].enabled);
    std::fs::write(
        &manifest,
        text.replace("file='icon.svg'", "file='../outside.svg'"),
    )
    .unwrap();
    assert!(library::import_manifest(&manifest, &r).is_err());
}
#[test]
fn cancellation_preserves_destination_and_commit_is_final() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagram.yyd");
    std::fs::write(&path, b"original").unwrap();
    let c = jobs::Control::default();
    assert!(c.cancel());
    assert!(format::save_control(&path, &Document::default(), &c).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    let c = jobs::Control::default();
    c.begin_commit().unwrap();
    assert!(!c.cancel());
}
#[test]
fn selection_text_svg_and_cached_views_match() {
    let r = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    let mut d = Document::default();
    let a = d.add_node(Shape::Rectangle, p(100., 100.), None);
    d.nodes[0].label = "編集可能な文字".into();
    let b = d.add_node(Shape::Ellipse, p(500., 500.), None);
    let f = d.fragment(&[a]);
    assert!(f.node(b).is_none());
    let svg = r
        .svg(
            &f,
            &Export {
                text: true,
                ..Export::document(&f)
            },
        )
        .unwrap();
    assert!(svg.contains("<text"));
    let options = Export {
        bounds: Rect {
            x: 72.,
            y: 64.,
            w: 400.,
            h: 300.,
        },
        background: Some(0xffffff),
        grid: true,
        selected: vec![a],
        text: false,
    };
    let first = r.raster(&d, &options, 1.).unwrap();
    let second = r.raster(&d, &options, 1.).unwrap();
    assert_eq!(first.data(), second.data());
    let png = r.png(&d, &options, 1.).unwrap();
    let dib = render::png_dib(&png).unwrap();
    assert_eq!(u32::from_le_bytes(dib[..4].try_into().unwrap()), 124);
    assert_eq!(dib.len(), 124 + 400 * 300 * 4);
}
#[test]
fn viewport_culling_preserves_icons_thick_lines_and_nested_transforms() {
    let r = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    let mut d = Document::default();
    assets::add_starter(&mut d);
    for (i, a) in d.assets.clone().iter().enumerate() {
        d.add_node(Shape::Icon, p(i as f64 * 120., 100.), Some(a.id));
    }
    let id = d.connect(
        Endpoint::Free(p(20., 70.)),
        Endpoint::Free(p(1300., 70.)),
        false,
    );
    let c = d.connectors.iter_mut().find(|c| c.id == id).unwrap();
    c.width = 32.;
    c.arrow_end = false;
    for (x, y, scale) in [(0., 78., 1.), (350., 90., 1.5), (700., 140., 0.75)] {
        let options = Export {
            bounds: Rect {
                x,
                y,
                w: 320.,
                h: 160.,
            },
            background: None,
            grid: false,
            text: false,
            selected: vec![],
        };
        let native = r.raster(&d, &options, scale).unwrap();
        let svg = r.svg(&d, &options).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut reference = resvg::tiny_skia::Pixmap::new(native.width(), native.height()).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
            &mut reference.as_mut(),
        );
        let diff: u64 = native
            .data()
            .iter()
            .zip(reference.data())
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        assert!(
            diff as f64 / (native.data().len() as f64) < 1.,
            "viewport {x},{y} differs"
        );
    }
}
