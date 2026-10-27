//! Repeatable interaction and sparse crossing workload; no timing assertions in tests.
use std::{hint::black_box, time::Instant};
use yy_draw::*;
fn measure(n: usize, mut f: impl FnMut()) -> f64 {
    let now = Instant::now();
    for _ in 0..n {
        f();
    }
    now.elapsed().as_secs_f64() * 1000. / n as f64
}
fn main() {
    let mut d = Document::default();
    for i in 0..2000 {
        d.add_node(
            Shape::Rectangle,
            Point::new((i % 40) as f64 * 200., (i / 40) as f64 * 120.),
            None,
        );
    }
    let a = d.nodes[0].id;
    let b = d.nodes[1].id;
    d.connect(
        Endpoint::Attached { node: a, side: 1 },
        Endpoint::Attached { node: b, side: 3 },
        true,
    );
    let c = d.connectors[0].clone();
    let rebuilding = measure(200, || {
        black_box(router::Router::new(&d).route(&c));
    });
    let router = router::Router::new(&d);
    let reused = measure(200, || {
        black_box(router.route(&c));
    });
    let mut cache = router::Cache::default();
    let cached = measure(200, || {
        cache.prepare(&d);
        black_box(cache.route(&c));
    });
    let mut cursor = 0;
    let mut changing = c.clone();
    let rebuild_changing = measure(200, || {
        changing.end = Endpoint::Attached {
            node: b,
            side: if cursor % 2 == 0 { 3 } else { 16 },
        };
        cursor += 1;
        black_box(router::Router::new(&d).route(&changing));
    });
    let cached_changing = measure(200, || {
        changing.end = Endpoint::Attached {
            node: b,
            side: if cursor % 2 == 0 { 3 } else { 16 },
        };
        cursor += 1;
        cache.prepare(&d);
        black_box(cache.route(&changing));
    });
    let ids: Vec<_> = d.nodes.iter().take(1000).map(|n| n.id).collect();
    let mut moving = d.clone();
    let selection = editing::MoveSelection::new(&d, &ids);
    let old_move = measure(30, || {
        for n in &mut moving.nodes {
            if ids.contains(&n.id) {
                n.rect = d.node(n.id).unwrap().rect;
            }
        }
        moving.translate(&ids, 16., 8.);
        black_box(&moving);
    });
    let move_selection = measure(200, || {
        selection.apply(&mut moving, Point::new(16., 8.));
        black_box(&moving);
    });
    let clone_update = measure(200, || {
        let mut copy = d.clone();
        copy.nodes[0].rect.w = 160.;
        black_box(copy);
    });
    let mut edit = d.clone();
    let element_update = measure(200, || {
        edit.nodes[0].rect.w = black_box(160.);
        black_box(&edit);
    });
    let mut lines = Document::default();
    let mut routes = vec![];
    for i in 0..5000 {
        let a = Point::new(0., i as f64 * 32.);
        let b = Point::new(160., a.y);
        lines.connect(Endpoint::Free(a), Endpoint::Free(b), false);
        routes.push(vec![a, b]);
    }
    let paths = measure(5, || {
        black_box(geometry::connector_paths_for(&lines, &routes));
    });
    println!(
        "nodes=2000 rebuild_route_ms={rebuilding:.4} reused_index_route_ms={reused:.4} cached_route_with_geometry_check_ms={cached:.4} rebuild_changing_endpoint_ms={rebuild_changing:.4} cached_changing_endpoint_ms={cached_changing:.4} old_move_1000_ms={old_move:.4} selected_move_1000_ms={move_selection:.4} document_clone_update_ms={clone_update:.4} element_update_ms={element_update:.6} sparse_5000_paths_ms={paths:.2}"
    );
}
