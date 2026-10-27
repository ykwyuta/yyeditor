//! Compare the cached canvas renderer with resvg's full-canvas SVG-image path.
use std::time::Instant;
use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let mut d = Document::default();
    for i in 0..200 {
        let index = (i * 7) % catalog::all().len();
        let asset = catalog::place(&mut d, index).unwrap();
        d.add_node(
            Shape::Icon,
            Point::new((i % 20) as f64 * 100., (i / 20) as f64 * 100.),
            Some(asset),
        );
        let node = d.nodes.last_mut().unwrap();
        node.rect.w = 64.;
        node.rect.h = 80.;
        node.label.clear();
    }
    let renderer = Renderer::new(None);
    let mut o = Export {
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
    renderer.raster(&d, &o, 1.).unwrap();
    let now = Instant::now();
    for i in 0..30 {
        o.bounds.x = i as f64 * 4.;
        renderer.raster(&d, &o, 1.).unwrap();
    }
    let optimized = now.elapsed().as_secs_f64() * 1000. / 30.;
    o.bounds.x = 0.;
    let svg = render::scene(&d, &o);
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let mut image = resvg::tiny_skia::Pixmap::new(1280, 720).unwrap();
    let now = Instant::now();
    for _ in 0..10 {
        image.fill(resvg::tiny_skia::Color::TRANSPARENT);
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut image.as_mut(),
        );
    }
    let reference = now.elapsed().as_secs_f64() * 1000. / 10.;
    println!(
        "icons=200 viewport=1280x720 cached_pan_ms={optimized:.2} full_canvas_images_ms={reference:.2} speedup={:.1}x",
        reference / optimized
    );
}
