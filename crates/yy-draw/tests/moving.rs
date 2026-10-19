use yy_draw::*;
#[test]
fn indexed_move_matches_absolute_translation_with_locked_nodes_and_connections() {
    let mut d = Document::default();
    let a = d.add_node(Shape::Rectangle, Point::new(0., 0.), None);
    let b = d.add_node(Shape::Ellipse, Point::new(320., 160.), None);
    let locked = d.add_node(Shape::Rectangle, Point::new(480., 0.), None);
    d.nodes[2].locked = true;
    let line = d.connect(
        Endpoint::Attached { node: a, side: 9 },
        Endpoint::Free(Point::new(240., 80.)),
        false,
    );
    d.connectors[0].waypoints.push(Point::new(200., 20.));
    d.connectors[0].corner_radius = 16.;
    d.connect(
        Endpoint::Attached { node: a, side: 1 },
        Endpoint::Attached { node: b, side: 3 },
        true,
    );
    let ids = [a, locked, line];
    let movement = editing::MoveSelection::new(&d, &ids);
    let original = d.clone();
    let mut moved = d.clone();
    let allocations = (moved.nodes.as_ptr(), moved.connectors.as_ptr());
    for delta in [
        Point::new(32., 16.),
        Point::new(-80., -40.),
        Point::new(0., 0.),
    ] {
        let mut reference = original.clone();
        reference.translate(&ids, delta.x, delta.y);
        movement.apply(&mut moved, delta);
        assert_eq!(moved, reference);
        assert_eq!(
            (moved.nodes.as_ptr(), moved.connectors.as_ptr()),
            allocations
        );
    }
    // Reordering/removing elements during a gesture must never update an unrelated element.
    moved.nodes.reverse();
    movement.apply(&mut moved, Point::new(16., 8.));
    assert_eq!(moved.node(b).unwrap(), original.node(b).unwrap());
    assert_eq!(moved.node(a).unwrap().rect.x, 16.);
    moved.nodes.retain(|n| n.id != a);
    movement.apply(&mut moved, Point::new(32., 16.));
    assert_eq!(moved.node(b).unwrap(), original.node(b).unwrap());
}
