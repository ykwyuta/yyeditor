use yy_draw::*;

#[test]
fn new_shapes_render_hit_and_connect_on_their_contours_at_different_sizes() {
    let renderer = render::Renderer::new(None);
    let mut names = std::collections::HashSet::new();
    assert_eq!(SHAPES.iter().filter(|(s, _)| s.is_extended()).count(), 50);
    for &(shape, name) in SHAPES.iter().filter(|(s, _)| s.is_extended()) {
        assert!(names.insert(name));
        let geometry = shapes::geometry(shape).unwrap();
        for (w, h) in [(144., 80.), (35., 150.), (230., 26.)] {
            let mut d = Document {
                grid: 1.,
                ..Default::default()
            };
            let id = d.add_node(shape, Point::new(-30., -40.), None);
            let r = Rect {
                x: -30.,
                y: -40.,
                w,
                h,
            };
            d.nodes[0].rect = r;
            d.nodes[0].fill = 0xd04030;
            d.nodes[0].stroke = 0x1050b0;
            d.nodes[0].line_width = 2.;
            let text = geometry.text_rect(r);
            assert!(
                r.contains(Point::new(text.x, text.y))
                    && r.contains(Point::new(text.x + text.w, text.y + text.h))
            );
            for side in 0..ports::COUNT {
                let p = ports::point(&d.nodes[0], side);
                assert!(p.x.is_finite() && p.y.is_finite());
                assert!(
                    geometry.near_outline(r, p, 0.00001),
                    "{shape:?} port {side}: {p:?}"
                );
                assert_eq!(
                    geometry::hit(&d, p, 0.01),
                    Some(id),
                    "{shape:?} port {side} {p:?} in {r:?}"
                );
            }
            let svg = renderer.svg(&d, &render::Export::document(&d)).unwrap();
            assert!(svg.contains("#d04030") && svg.contains("#1050b0"));
            let image = renderer
                .raster(&d, &render::Export::document(&d), 1.)
                .unwrap();
            assert!(
                image
                    .data()
                    .chunks_exact(4)
                    .any(|p| u16::from(p[0]) > u16::from(p[1]) + 10 && p[0] > p[2]),
                "{shape:?}"
            );
        }
    }
}

#[test]
fn holes_and_callout_text_are_not_treated_as_rectangular_filled_shapes() {
    for shape in [
        Shape::Donut,
        Shape::Equals,
        Shape::NotEqual,
        Shape::CalloutRounded,
    ] {
        let mut d = Document {
            grid: 1.,
            ..Default::default()
        };
        let id = d.add_node(shape, Point::default(), None);
        d.nodes[0].rect = Rect {
            x: 0.,
            y: 0.,
            w: 100.,
            h: 100.,
        };
        let outside = match shape {
            Shape::Donut | Shape::Equals => Point::new(50., 50.),
            Shape::NotEqual => Point::new(95., 95.),
            _ => Point::new(90., 95.),
        };
        assert_eq!(geometry::hit(&d, outside, 0.), None, "{shape:?}");
        let p = ports::point(&d.nodes[0], 1);
        assert_eq!(geometry::hit(&d, p, 0.01), Some(id));
        if shape == Shape::Donut {
            d.nodes[0].label = "本文".into();
            assert_eq!(geometry::hit(&d, Point::new(50., 50.), 0.), Some(id));
        }
    }
    for &(shape, _) in SHAPES
        .iter()
        .filter(|(s, _)| shapes::category(*s) == "基本図形 / 吹き出し")
    {
        let r = Rect {
            x: 0.,
            y: 0.,
            w: 100.,
            h: 100.,
        };
        assert!(
            shapes::geometry(shape).unwrap().text_rect(r).y
                + shapes::geometry(shape).unwrap().text_rect(r).h
                < 80.
        );
    }
}

#[test]
fn shapes_keep_format_history_copy_and_connections_and_old_files_keep_their_version() {
    for (shape, old_tag) in [
        Shape::Rectangle,
        Shape::Rounded,
        Shape::Ellipse,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Cylinder,
        Shape::Cloud,
        Shape::Text,
        Shape::Frame,
        Shape::Icon,
        Shape::Junction,
    ]
    .into_iter()
    .zip(0u8..)
    {
        assert_eq!(postcard::to_allocvec(&shape).unwrap(), vec![old_tag]);
    }
    let mut all = Document::default();
    for &(shape, _) in SHAPES.iter().filter(|(s, _)| s.is_extended()) {
        all.add_node(shape, Point::default(), None);
    }
    assert_eq!(format::decode(&format::encode(&all).unwrap()).unwrap(), all);
    let mut d = Document::default();
    let old = d.add_node(Shape::Rectangle, Point::default(), None);
    let bytes = format::encode(&d).unwrap();
    assert_eq!(&bytes[10..12], &1u16.to_le_bytes());
    assert_eq!(format::decode(&bytes).unwrap(), d);
    let before = d.clone();
    let new = d.add_node(Shape::CalloutRounded, Point::new(240., 160.), None);
    d.nodes.last_mut().unwrap().label = "日本語の吹き出し\n本文".into();
    d.connect(
        Endpoint::Attached { node: old, side: 1 },
        Endpoint::Attached { node: new, side: 3 },
        true,
    );
    let mut history = History::default();
    history.commit(before.clone(), &d);
    let after = d.clone();
    let bytes = format::encode(&d).unwrap();
    assert_eq!(&bytes[10..12], &5u16.to_le_bytes());
    assert_eq!(format::decode(&bytes).unwrap(), d);
    history.undo(&mut d);
    assert_eq!(d, before);
    history.redo(&mut d);
    assert_eq!(d, after);
    let start = d.endpoint(d.connectors[0].end);
    d.translate(&[new], 16., 32.);
    assert!(
        d.endpoint(d.connectors[0].end)
            .distance(Point::new(start.x + 16., start.y + 32.))
            < 1e-9
    );
    let ids = d.duplicate(&[new]);
    assert_eq!(d.node(ids[0]).unwrap().shape, Shape::CalloutRounded);
    let copy = d.fragment(&[new]);
    let mut pasted = Document::default();
    pasted.paste(&copy, Point::new(32., 16.));
    assert_eq!(pasted.nodes[0].shape, Shape::CalloutRounded);
    d.add_page("別ページ".into()).unwrap();
    assert_eq!(&format::encode(&d).unwrap()[10..12], &5u16.to_le_bytes());
    assert_eq!(format::decode(&format::encode(&d).unwrap()).unwrap(), d);
}
