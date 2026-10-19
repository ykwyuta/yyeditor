use yy_draw::*;

#[test]
fn ports_are_distinct_selectable_and_keep_legacy_midpoints() {
    for shape in [
        Shape::Rectangle,
        Shape::Rounded,
        Shape::Ellipse,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Cylinder,
        Shape::Icon,
        Shape::Cloud,
        Shape::Text,
    ] {
        let mut d = Document::default();
        let id = d.add_node(shape, Point::new(40., 80.), None);
        let n = &d.nodes[0];
        let r = n.rect;
        let expected = [
            Point::new(r.x + r.w / 2., r.y),
            Point::new(r.x + r.w, r.y + r.h / 2.),
            Point::new(r.x + r.w / 2., r.y + r.h),
            Point::new(r.x, r.y + r.h / 2.),
        ];
        for (side, p) in expected.iter().enumerate() {
            assert_eq!(ports::point(n, side as u8), *p);
        }
        let points: Vec<_> = (0..ports::COUNT).map(|s| ports::point(n, s)).collect();
        for (side, p) in points.iter().enumerate() {
            assert!(
                points[..side]
                    .iter()
                    .all(|other| p.distance(*other) > 0.001),
                "duplicate {shape:?} {side}"
            );
            assert_eq!(
                d.nearest_port(*p, 0.01),
                Some(Endpoint::Attached {
                    node: id,
                    side: side as u8
                })
            );
            let exit = ports::exit(n, side as u8);
            assert!(!r.expand(8.).contains(exit));
            assert!((exit.x - p.x).abs() < 0.001 || (exit.y - p.y).abs() < 0.001);
        }
        let top = d.add_node(shape, Point::new(40., 80.), None);
        assert_eq!(
            d.nearest_port(points[8], 0.01),
            Some(Endpoint::Attached { node: top, side: 8 })
        );
    }
}

#[test]
fn extended_connections_route_follow_edits_and_roundtrip_in_inactive_pages() {
    let mut d = Document::default();
    let a = d.add_node(Shape::Ellipse, Point::new(0., 0.), None);
    let b = d.add_node(Shape::Triangle, Point::new(320., 120.), None);
    let start = Endpoint::Attached { node: a, side: 8 };
    let end = Endpoint::Attached { node: b, side: 19 };
    d.connect(start, end, true);
    let check = |d: &Document| {
        let c = &d.connectors[0];
        let route = router::Router::new(d).route(c);
        assert!(route.resolved);
        assert_eq!(route.points.first(), Some(&d.endpoint(c.start)));
        assert_eq!(route.points.last(), Some(&d.endpoint(c.end)));
        assert!(
            route
                .points
                .windows(2)
                .all(|s| (s[0].x - s[1].x).abs() < 0.001 || (s[0].y - s[1].y).abs() < 0.001)
        );
    };
    check(&d);
    let old = d.endpoint(start);
    d.translate(&[a], 32., 16.);
    assert_eq!(d.endpoint(start), Point::new(old.x + 32., old.y + 16.));
    d.nodes[0].rect.w *= 2.;
    d.nodes[0].rect.h *= 1.5;
    assert_ne!(d.endpoint(start), old);
    check(&d);
    let copied = d.duplicate(&[a, b]);
    assert_eq!(copied.len(), 3);
    assert!(matches!(d.connectors[1].start,Endpoint::Attached{node,side:8} if node!=a));
    assert!(matches!(d.connectors[1].end,Endpoint::Attached{node,side:19} if node!=b));
    d.translate(&copied, 0., 400.);
    d.add_page("Other".into()).unwrap();
    assert!(d.connectors.is_empty());
    let bytes = format::encode(&d).unwrap();
    assert_eq!(&bytes[10..12], &2u16.to_le_bytes());
    let mut restored = format::decode(&bytes).unwrap();
    assert_eq!(restored, d);
    restored.switch_page(0).unwrap();
    assert_eq!(restored.connectors[0].start, start);
    check(&restored);
    restored.connectors.clear();
    let bytes = format::encode(&restored).unwrap();
    assert_eq!(&bytes[10..12], &1u16.to_le_bytes());
}
