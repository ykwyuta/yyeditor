use yy_draw::{
    drawing::Draft,
    render::{Export, Renderer},
    *,
};
fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[test]
fn shift_constrains_four_directions_and_bends_allow_three_without_backtracking() {
    for (pointer, expected) in [
        (p(100., 20.), p(100., 0.)),
        (p(-100., 20.), p(-100., 0.)),
        (p(20., 100.), p(0., 100.)),
        (p(20., -100.), p(0., -100.)),
    ] {
        let mut draft = Draft::new(p(0., 0.));
        draft.update(pointer, true);
        assert_eq!(draft.current, expected);
        assert!(draft.bend());
        assert!(!draft.bend(), "key repeat must not add zero length bends");
        let pivot = draft.current;
        for direction in [p(1., 0.), p(-1., 0.), p(0., 1.), p(0., -1.)] {
            let q = p(pivot.x + direction.x * 40., pivot.y + direction.y * 40.);
            draft.update(q, true);
            if expected.x * direction.x + expected.y * direction.y < 0. {
                assert_eq!(draft.current, pivot, "reverse direction must be excluded");
            } else {
                assert_eq!(draft.current, q);
            }
        }
        draft.update(p(7., 13.), false);
        assert_eq!(draft.current, p(7., 13.));
        assert!(draft.manual);
        draft.undo_bend();
        assert!(draft.bends.is_empty());
    }
}

#[test]
fn constrained_snapping_keeps_the_segment_axis_and_follows_exact_ports() {
    let mut d = Document::default();
    let id = d.add_node(Shape::Rectangle, p(200., 0.), None);
    let target = d.endpoint(Endpoint::Attached { node: id, side: 3 });
    let mut draft = Draft::new(p(0., target.y));
    draft.update(p(target.x - 3., target.y + 4.), true);
    assert_eq!(
        draft.endpoint(&d, 10.),
        Endpoint::Attached { node: id, side: 3 }
    );
    let mut offset = Draft::new(p(0., target.y + 3.));
    offset.update(p(target.x, target.y + 4.), true);
    assert_eq!(
        offset.endpoint(&d, 10.),
        Endpoint::Free(p(target.x, target.y + 3.))
    );
    offset.update(target, false);
    assert_eq!(
        offset.endpoint(&d, 10.),
        Endpoint::Attached { node: id, side: 3 }
    );
}

fn bent() -> Document {
    let mut d = Document {
        show_grid: false,
        ..Default::default()
    };
    d.connect(
        Endpoint::Free(p(0., 0.)),
        Endpoint::Free(p(160., 80.)),
        false,
    );
    d.connectors[0].waypoints = vec![p(80., 0.), p(80., 80.)];
    d
}

#[test]
fn rounded_corners_clamp_short_segments_and_export_the_same_geometry() {
    let mut d = bent();
    let sharp = geometry::connector_path(&d, 0);
    assert!(!sharp.contains(" Q "));
    d.connectors[0].corner_radius = 12.;
    let rounded = geometry::connector_path(&d, 0);
    assert_eq!(rounded.matches(" Q ").count(), 2);
    assert!(rounded.contains("L 68 0 Q 80 0, 80 12"));
    let short = [p(0., 0.), p(4., 0.), p(4., 6.), p(10., 6.)];
    assert_eq!(
        geometry::rounded_corner(&short, 1, 1000.),
        Some((p(2., 0.), p(4., 2.)))
    );
    assert_eq!(
        geometry::rounded_corner(&short, 2, 1000.),
        Some((p(4., 3.), p(7., 6.)))
    );
    assert!(geometry::rounded_corner(&[p(0., 0.), p(10., 0.), p(20., 0.)], 1, 10.).is_none());
    let r = Renderer::new(None);
    let options = Export {
        bounds: Rect {
            x: -8.,
            y: -8.,
            w: 176.,
            h: 96.,
        },
        background: None,
        grid: false,
        text: false,
        selected: vec![],
    };
    let svg = r.svg(&d, &options).unwrap();
    assert!(svg.contains("<path") && (svg.contains('Q') || svg.contains('C')));
    let image = r.raster(&d, &options, 1.).unwrap();
    assert_eq!(
        image.pixel(88, 8).unwrap().alpha(),
        0,
        "outside rounded corner must be transparent"
    );
    assert!(
        image.pixel(85, 11).unwrap().alpha() > 0,
        "curve must be rendered"
    );
    d.connectors[0].corner_radius = 0.;
    let sharp_image = r.raster(&d, &options, 1.).unwrap();
    assert!(sharp_image.pixel(88, 8).unwrap().alpha() > 0);
    assert_ne!(image.data(), sharp_image.data());
    // Finishing immediately after Tab keeps the final arrow and preceding rounded bend.
    d.connectors[0].waypoints.push(p(160., 80.));
    d.connectors[0].corner_radius = 12.;
    let route = router::Router::new(&d).route(&d.connectors[0]);
    assert_eq!(route.points.len(), 4);
    assert_eq!(geometry::connector_path(&d, 0).matches(" Q ").count(), 2);
}

#[test]
fn rounded_styles_survive_pages_copy_undo_and_save_and_reject_invalid_radii() {
    let mut d = bent();
    let before = d.clone();
    d.connectors[0].corner_radius = 24.;
    let mut history = History::default();
    history.commit(before.clone(), &d);
    history.undo(&mut d);
    assert_eq!(d, before);
    history.redo(&mut d);
    assert_eq!(d.connectors[0].corner_radius, 24.);
    let fragment = d.fragment(&[d.connectors[0].id]);
    d.paste(&fragment, p(20., 20.));
    assert_eq!(d.connectors[1].corner_radius, 24.);
    d.add_page("Other".into()).unwrap();
    let bytes = format::encode(&d).unwrap();
    assert_eq!(&bytes[10..12], &3u16.to_le_bytes());
    assert_eq!(format::decode(&bytes).unwrap(), d);
    d.switch_page(0).unwrap();
    for radius in [f64::NAN, f64::INFINITY, -1., 10001.] {
        d.connectors[0].corner_radius = radius;
        assert!(format::encode(&d).is_err());
    }
}

#[test]
fn crossing_bridges_do_not_overlap_rounded_bends() {
    let mut d = bent();
    d.connectors[0].corner_radius = 20.;
    d.connect(
        Endpoint::Free(p(70., -20.)),
        Endpoint::Free(p(70., 20.)),
        false,
    );
    d.connectors[0].jump = Jump::Always;
    let path = geometry::connector_path(&d, 0);
    assert!(
        !path.contains(" C "),
        "crossing on trimmed corner must not draw an overlapping bridge"
    );
    d.connectors[1].start = Endpoint::Free(p(40., -20.));
    d.connectors[1].end = Endpoint::Free(p(40., 20.));
    let path = geometry::connector_path(&d, 0);
    assert!(path.contains(" C ") && path.contains(" Q "));
}
