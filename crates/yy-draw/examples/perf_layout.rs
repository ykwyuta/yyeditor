//! Grouping/alignment performance, including the prepared selection renderer.
use std::{hint::black_box, time::Instant};
use yy_draw::{
    layout::{Align, Reference},
    render::{Export, Renderer},
    *,
};
fn measure(n: usize, mut f: impl FnMut()) -> f64 {
    let start = Instant::now();
    for _ in 0..n {
        f();
    }
    start.elapsed().as_secs_f64() * 1000. / n as f64
}
fn main() {
    let mut doc = Document::default();
    for i in 0..2000 {
        doc.add_node(
            Shape::Rectangle,
            Point::new((i % 40) as f64 * 120., (i / 40) as f64 * 80.),
            None,
        );
        let n = doc.nodes.last_mut().unwrap();
        n.rect.w = 64.;
        n.rect.h = 40.;
    }
    let ids: Vec<_> = doc.nodes.iter().map(|n| n.id).collect();
    for i in 0..5000 {
        let a = i % 1960;
        let vertical = i % 3 == 0;
        let b = if vertical { a + 40 } else { a + 1 };
        doc.connect(
            Endpoint::Attached {
                node: ids[a],
                side: if vertical { 2 } else { 1 },
            },
            Endpoint::Attached {
                node: ids[b],
                side: if vertical { 0 } else { 3 },
            },
            true,
        );
    }
    for pair in (0..2000).step_by(2) {
        let group = doc.id();
        doc.nodes[pair].group = Some(group);
        doc.nodes[pair + 1].group = Some(group);
    }
    let groups: std::collections::HashMap<_, _> =
        doc.nodes.iter().map(|n| (n.id, n.group)).collect();
    for c in &mut doc.connectors {
        if let (Endpoint::Attached { node: a, .. }, Endpoint::Attached { node: b, .. }) =
            (c.start, c.end)
            && groups[&a] == groups[&b]
        {
            c.group = groups[&a];
        }
    }
    let select_ms = measure(100, || {
        black_box(doc.expanded_selection(black_box(&ids[..1000])));
    });
    let units_ms = measure(100, || {
        black_box(doc.layout_units(black_box(&ids)));
    });
    let center_ms = measure(50, || {
        doc.arrange(&ids, Align::Center, Reference::Page).unwrap();
    });
    // Restore a spatially spread layout before measuring distribution and drawing.
    for (i, n) in doc.nodes.iter_mut().enumerate() {
        n.rect.x = (i % 40) as f64 * 120.;
    }
    let distribute_ms = measure(50, || {
        doc.arrange(&ids, Align::Horizontal, Reference::Selection)
            .unwrap();
    });
    for (i, n) in doc.nodes.iter_mut().enumerate() {
        n.rect.x = (i % 40) as f64 * 120.;
    }
    let renderer = Renderer::new(None);
    let mut options = Export {
        bounds: Rect {
            x: 0.,
            y: 0.,
            w: 1280.,
            h: 720.,
        },
        background: Some(0xffffff),
        grid: true,
        text: false,
        selected: doc.expanded_selection(&ids),
    };
    renderer.raster(&doc, &options, 1.).unwrap();
    let selected_pan_ms = measure(30, || {
        options.bounds.x += 8.;
        black_box(renderer.raster(&doc, &options, 1.).unwrap());
    });
    println!(
        "nodes=2000 connectors=5000 groups=1000 expand_1000_ms={select_ms:.3} collect_units_ms={units_ms:.3} page_center_ms={center_ms:.3} distribute_ms={distribute_ms:.3} selected_pan_ms={selected_pan_ms:.2}"
    );
}
