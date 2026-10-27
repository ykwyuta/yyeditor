use yy_draw::{
    layout::{Align, Reference},
    *,
};
fn sample() -> (Document, Vec<Id>) {
    let mut d = Document::default();
    for (x, y, w, h) in [
        (10., 20., 40., 20.),
        (200., 160., 60., 30.),
        (450., 350., 80., 40.),
    ] {
        d.add_node(Shape::Rectangle, Point::new(x, y), None);
        d.nodes.last_mut().unwrap().rect = Rect { x, y, w, h };
    }
    let ids = d.nodes.iter().map(|n| n.id).collect();
    (d, ids)
}
#[test]
fn six_alignments_preserve_sizes_and_use_selection_or_page_bounds() {
    for action in [
        Align::Left,
        Align::Center,
        Align::Right,
        Align::Top,
        Align::Middle,
        Align::Bottom,
    ] {
        let (mut d, ids) = sample();
        let before = d.clone();
        d.arrange(&ids, action, Reference::Selection).unwrap();
        let values: Vec<_> = d
            .nodes
            .iter()
            .map(|n| match action {
                Align::Left => n.rect.x,
                Align::Center => n.rect.center().x,
                Align::Right => n.rect.x + n.rect.w,
                Align::Top => n.rect.y,
                Align::Middle => n.rect.center().y,
                _ => n.rect.y + n.rect.h,
            })
            .collect();
        assert!(values.windows(2).all(|s| (s[0] - s[1]).abs() < 1e-8));
        for (n, old) in d.nodes.iter().zip(&before.nodes) {
            assert_eq!((n.rect.w, n.rect.h), (old.rect.w, old.rect.h));
        }
        d.arrange(&[ids[0]], action, Reference::Page).unwrap();
        let r = d.nodes[0].rect;
        let page = d.page_bounds();
        match action {
            Align::Left => assert_eq!(r.x, 0.),
            Align::Right => assert_eq!(r.x + r.w, page.w),
            Align::Center => assert_eq!(r.center().x, page.w / 2.),
            Align::Top => assert_eq!(r.y, 0.),
            Align::Middle => assert_eq!(r.center().y, page.h / 2.),
            _ => assert_eq!(r.y + r.h, page.h),
        }
    }
}
#[test]
fn horizontal_and_vertical_distribution_use_equal_edge_gaps() {
    for action in [Align::Horizontal, Align::Vertical] {
        let (mut d, ids) = sample();
        d.arrange(&ids, action, Reference::Selection).unwrap();
        let r: Vec<_> = d.nodes.iter().map(|n| n.rect).collect();
        let gap = |a: Rect, b: Rect| {
            if action == Align::Horizontal {
                b.x - a.x - a.w
            } else {
                b.y - a.y - a.h
            }
        };
        assert!((gap(r[0], r[1]) - gap(r[1], r[2])).abs() < 1e-8);
        assert_eq!((r[0].x, r[0].y), (10., 20.));
        assert_eq!((r[2].x + r[2].w, r[2].y + r[2].h), (530., 390.));
        d.arrange(&ids, action, Reference::Page).unwrap();
        let page = d.page_bounds();
        if action == Align::Horizontal {
            assert_eq!(d.nodes[0].rect.x, 0.);
            assert_eq!(d.nodes[2].rect.x + d.nodes[2].rect.w, page.w);
        } else {
            assert_eq!(d.nodes[0].rect.y, 0.);
            assert_eq!(d.nodes[2].rect.y + d.nodes[2].rect.h, page.h);
        }
    }
}
#[test]
fn grouping_includes_internal_wires_and_keeps_offsets_during_alignment() {
    let (mut d, ids) = sample();
    d.connect(
        Endpoint::Attached {
            node: ids[0],
            side: 1,
        },
        Endpoint::Attached {
            node: ids[1],
            side: 3,
        },
        true,
    );
    d.connectors[0].waypoints.push(Point::new(120., 80.));
    let members = d.group_objects(&ids[..2]).unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(d.expanded_selection(&[members[0]]), members);
    assert_eq!(d.expanded_selection(&[members[2]]), members);
    let before = d.clone();
    d.arrange(&[ids[0], ids[2]], Align::Bottom, Reference::Selection)
        .unwrap();
    let dy = d.nodes[0].rect.y - before.nodes[0].rect.y;
    assert_eq!(d.nodes[1].rect.y - before.nodes[1].rect.y, dy);
    assert_eq!(
        d.connectors[0].waypoints[0].y - before.connectors[0].waypoints[0].y,
        dy
    );
    assert_eq!(
        d.endpoint(d.connectors[0].start).y - before.endpoint(before.connectors[0].start).y,
        dy
    );
    assert_eq!(d.layout_units(&members).len(), 1);
    let grouped = d.clone();
    assert_eq!(d.ungroup_objects(&[members[2]]).len(), 3);
    assert!(d.nodes.iter().all(|n| n.group.is_none()));
    assert!(d.connectors[0].group.is_none());
    let mut h = History::default();
    h.commit(grouped.clone(), &d);
    h.undo(&mut d);
    assert_eq!(d, grouped);
}
#[test]
fn groups_survive_copy_duplicate_pages_and_format_roundtrip() {
    let (mut d, ids) = sample();
    d.connect(
        Endpoint::Attached {
            node: ids[0],
            side: 8,
        },
        Endpoint::Attached {
            node: ids[1],
            side: 19,
        },
        true,
    );
    d.connectors[0].corner_radius = 12.;
    let members = d.group_objects(&ids[..2]).unwrap();
    let old = d.nodes[0].group.unwrap();
    let copies = d.duplicate(&members);
    let new = d.node(copies[0]).unwrap().group.unwrap();
    assert_ne!(old, new);
    assert_eq!(d.connectors[1].group, Some(new));
    let fragment = d.fragment(&members);
    let pasted = d.paste(&fragment, Point::new(16., 16.));
    let group = d.node(pasted[0]).unwrap().group;
    assert_ne!(group, Some(old));
    assert_eq!(d.connectors.last().unwrap().group, group);
    d.add_page("Other".into()).unwrap();
    let bytes = format::encode(&d).unwrap();
    assert_eq!(&bytes[10..12], &4u16.to_le_bytes());
    assert_eq!(format::decode(&bytes).unwrap(), d);
}
#[test]
fn regroup_expands_old_groups_and_locked_units_do_not_move() {
    let (mut d, ids) = sample();
    let before = d.clone();
    assert!(d.group_objects(&ids[..1]).is_err());
    assert_eq!(d, before);
    d.group_objects(&ids[..2]).unwrap();
    let all = d.group_objects(&[ids[0], ids[2]]).unwrap();
    assert_eq!(all.len(), 3);
    d.nodes[1].locked = true;
    let before = d.clone();
    assert!(d.arrange(&all, Align::Center, Reference::Page).is_err());
    assert_eq!(d, before);
    let selected = d.expanded_selection(&all);
    let moving = editing::MoveSelection::new(&d, &selected);
    moving.apply(&mut d, Point::new(16., 8.));
    assert_eq!(d, before);
    d.translate(&selected, 16., 8.);
    assert_eq!(d, before);
    d.ungroup_objects(&all);
    d.arrange(&ids, Align::Top, Reference::Page).unwrap();
    assert_eq!(d.nodes[1].rect, before.nodes[1].rect);
}
#[test]
fn grouped_selection_matches_svg_at_negative_origins_and_culls_offscreen_handles() {
    let (mut d, ids) = sample();
    d.group_objects(&ids[..2]).unwrap();
    for n in &mut d.nodes {
        n.label.clear();
    }
    let renderer = render::Renderer::new(None);
    let options = render::Export {
        bounds: Rect {
            x: -16.,
            y: -16.,
            w: 180.,
            h: 160.,
        },
        background: None,
        grid: false,
        text: false,
        selected: d.expanded_selection(&ids),
    };
    let actual = renderer.raster(&d, &options, 1.).unwrap();
    let svg = renderer.svg(&d, &options).unwrap();
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let mut expected = resvg::tiny_skia::Pixmap::new(actual.width(), actual.height()).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut expected.as_mut(),
    );
    assert_eq!(actual.data(), expected.data());
}
