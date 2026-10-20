//! Contact sheet of the editable basic shapes added after the original set.
use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/yydraw-basic-shapes.png".into());
    let mut d = Document {
        grid: 1.,
        font_size: 11.,
        ..Default::default()
    };
    for (i, &(shape, name)) in SHAPES.iter().filter(|(s, _)| s.is_extended()).enumerate() {
        let (w, h) = shapes::default_size(shape);
        let scale = (106. / w).min(72. / h);
        let x = 12. + (i % 10) as f64 * 140.;
        let y = 12. + (i / 10) as f64 * 116.;
        d.add_node(shape, Point::default(), None);
        let node = d.nodes.last_mut().unwrap();
        node.rect = Rect {
            x: x + (128. - w * scale) / 2.,
            y: y + (76. - h * scale) / 2.,
            w: w * scale,
            h: h * scale,
        };
        d.add_node(Shape::Text, Point::default(), None);
        let label = d.nodes.last_mut().unwrap();
        label.rect = Rect {
            x,
            y: y + 79.,
            w: 128.,
            h: 28.,
        };
        label.label = name.into();
    }
    let renderer = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    std::fs::write(
        &path,
        renderer
            .png(
                &d,
                &Export {
                    bounds: Rect {
                        x: 0.,
                        y: 0.,
                        w: 1424.,
                        h: 604.,
                    },
                    background: Some(0xffffff),
                    grid: false,
                    text: false,
                    selected: vec![],
                },
                1.,
            )
            .unwrap(),
    )
    .unwrap();
    println!("{path}");
}
