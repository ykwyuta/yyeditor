use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}
fn crossing() -> Document {
    let mut d = Document::default();
    d.connect(
        Endpoint::Free(p(0., 50.)),
        Endpoint::Free(p(100., 50.)),
        false,
    );
    d.connect(
        Endpoint::Free(p(50., 0.)),
        Endpoint::Free(p(50., 100.)),
        false,
    );
    d
}
#[test]
fn crossings_have_one_stable_bridge_and_never_join() {
    let mut d = crossing();
    assert!(!geometry::connector_path(&d, 0).contains(" C "));
    assert!(geometry::connector_path(&d, 1).contains(" C "));
    let original = d.clone();
    d.connectors[0].jump = Jump::Always;
    assert!(geometry::connector_path(&d, 0).contains(" C "));
    assert!(!geometry::connector_path(&d, 1).contains(" C "));
    d.connectors[0].jump = Jump::Never;
    d.connectors[1].jump = Jump::Never;
    assert!(!geometry::connector_path(&d, 1).contains(" C "));
    let restored = format::decode(&format::encode(&original).unwrap()).unwrap();
    assert_eq!(
        geometry::connector_path(&restored, 1),
        geometry::connector_path(&original, 1)
    );
    assert!(geometry::intersection(p(0., 0.), p(50., 0.), p(50., 0.), p(50., 50.)).is_none());
    assert!(geometry::intersection(p(0., 0.), p(50., 0.), p(10., 0.), p(100., 0.)).is_none());
}
#[test]
fn triple_crossing_has_only_one_bridge() {
    let mut d = crossing();
    d.connect(
        Endpoint::Free(p(0., 0.)),
        Endpoint::Free(p(100., 100.)),
        false,
    );
    assert_eq!(
        (0..3)
            .filter(|&i| geometry::connector_path(&d, i).contains(" C "))
            .count(),
        1
    );
}
#[test]
fn attached_connections_follow_move_duplicate_and_delete() {
    let mut d = Document::default();
    let a = d.add_node(Shape::Rectangle, p(0., 0.), None);
    let b = d.add_node(Shape::Cylinder, p(320., 120.), None);
    d.connect(
        Endpoint::Attached { node: a, side: 1 },
        Endpoint::Attached { node: b, side: 3 },
        true,
    );
    let original = d.clone();
    let group = d.id();
    for n in &mut d.nodes {
        n.group = Some(group);
    }
    d.translate(&[a], 32., 16.);
    assert_eq!(d.endpoint(d.connectors[0].start), p(176., 56.));
    let ids = d.duplicate(&[a, b]);
    let copied_group = d.node(ids[0]).unwrap().group.unwrap();
    assert_ne!(copied_group, group);
    assert_eq!(d.node(ids[1]).unwrap().group, Some(copied_group));
    assert_eq!(ids.len(), 3);
    let c = d.connectors.last().unwrap();
    assert!(matches!(c.start,Endpoint::Attached{node,..} if node!=a&&ids.contains(&node)));
    assert!(
        geometry::route(&d, c)
            .windows(2)
            .all(|s| (s[0].x - s[1].x).abs() < 0.001 || (s[0].y - s[1].y).abs() < 0.001)
    );
    let before = d.endpoint(d.connectors[0].start);
    d.delete(&[a]);
    assert_eq!(d.connectors[0].start, Endpoint::Free(before));
    d.validate().unwrap();
    let mut h = History::default();
    h.commit(original.clone(), &d);
    h.undo(&mut d);
    assert_eq!(d, original);
    h.redo(&mut d);
    assert_ne!(d, original);
}
#[test]
fn binary_storage_detects_corruption_and_replaces_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("日本語.yyd");
    let d = crossing();
    format::save(&path, &d).unwrap();
    assert_eq!(format::load(&path).unwrap(), d);
    let mut next = d.clone();
    assets::add_starter(&mut next);
    format::save(&path, &next).unwrap();
    assert_eq!(format::load(&path).unwrap(), next);
    let mut bytes = format::encode(&next).unwrap();
    bytes[55] ^= 1;
    assert!(format::decode(&bytes).is_err());
    assert!(format::decode(&bytes[..30]).is_err());
    bytes[8] = 2;
    assert!(format::decode(&bytes).is_err());
    let mut invalid = next.clone();
    invalid.grid = f64::NAN;
    assert!(format::save(&path, &invalid).is_err());
    assert_eq!(format::load(&path).unwrap(), next);
}
#[test]
fn assets_are_embedded_static_and_render_in_both_formats() {
    let renderer = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    let mut d = crossing();
    assets::add_starter(&mut d);
    for (i, a) in d.assets.clone().iter().enumerate() {
        render::validate_svg(&a.svg).unwrap();
        let a = renderer
            .normalize_asset(a.svg.as_bytes(), a.name.clone(), a.source.clone(), a.id)
            .unwrap();
        assert!(!a.svg.contains("<script"));
        d.add_node(Shape::Icon, p(i as f64 * 100., 160.), Some(a.id));
    }
    let n = d.add_node(Shape::Rounded, p(180., 20.), None);
    d.nodes.iter_mut().find(|v| v.id == n).unwrap().label = "日本語と英語\n<構成図> & test".into();
    d.validate().unwrap();
    let opts = Export::document(&d);
    let svg = renderer.svg(&d, &opts).unwrap();
    assert!(!svg.contains("<text"));
    assert!(!svg.contains("<script"));
    assert!(svg.contains("<path"));
    assert!(svg.contains("data:"));
    let png = renderer.png(&d, &opts, 1.).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(png.len() > 1000);
    let from_svg = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let native = renderer.raster(&d, &opts, 1.).unwrap();
    let mut svg_image = resvg::tiny_skia::Pixmap::new(native.width(), native.height()).unwrap();
    resvg::render(
        &from_svg,
        resvg::tiny_skia::Transform::default(),
        &mut svg_image.as_mut(),
    );
    let difference: u64 = native
        .data()
        .iter()
        .zip(svg_image.data())
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    assert!(
        (difference as f64) / (native.data().len() as f64) < 1.,
        "SVG and PNG differ"
    );
    let dir = tempfile::tempdir().unwrap();
    let icon = renderer
        .normalize_asset(&png, "snapshot".into(), "test".into(), 99)
        .unwrap();
    assert!(icon.svg.contains("data:image/png"));
    std::fs::write(dir.path().join("out.png"), png).unwrap();
    for bad in [
        "<svg><script>alert(1)</script></svg>",
        "<svg><image href='file:///secret.png'/></svg>",
        "<svg><rect onload='bad()'/></svg>",
    ] {
        assert!(render::validate_svg(bad).is_err());
    }
    assert!(renderer.png(&d, &opts, 1e6).is_err());
}
