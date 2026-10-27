use std::time::Instant;
use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let mut d = Document::default();
    for i in 0..2000 {
        let id = d.add_node(
            Shape::Rectangle,
            Point::new((i % 40) as f64 * 120., (i / 40) as f64 * 80.),
            None,
        );
        let n = d.nodes.iter_mut().find(|n| n.id == id).unwrap();
        n.rect.w = 64.;
        n.rect.h = 40.;
    }
    let ids = d.nodes.iter().map(|n| n.id).collect::<Vec<_>>();
    for i in 0..5000 {
        let a = i % 1960;
        let b = if i % 3 == 0 { a + 40 } else { a + 1 };
        d.connect(
            Endpoint::Attached {
                node: ids[a],
                side: if i % 3 == 0 { 2 } else { 1 },
            },
            Endpoint::Attached {
                node: ids[b],
                side: if i % 3 == 0 { 0 } else { 3 },
            },
            true,
        );
    }
    let now = Instant::now();
    let routes = router::Router::new(&d).all();
    let routing = now.elapsed();
    let failed = routes.iter().filter(|r| !r.resolved).count();
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
        selected: vec![],
    };
    let now = Instant::now();
    renderer.raster(&d, &options, 1.).unwrap();
    let cold = now.elapsed();
    let now = Instant::now();
    for i in 0..30 {
        options.bounds.x = i as f64 * 8.;
        renderer.raster(&d, &options, 1.).unwrap();
    }
    let pan = now.elapsed().as_secs_f64() * 1000. / 30.;
    println!(
        "nodes=2000 connectors=5000 routing_ms={:.2} unresolved={failed} cold_frame_ms={:.2} cached_pan_ms={pan:.2} fps={:.1} cpu={}",
        routing.as_secs_f64() * 1000.,
        cold.as_secs_f64() * 1000.,
        1000. / pan,
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| std::env::consts::ARCH.into())
    );
}
