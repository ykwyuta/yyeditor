//! Render a labeled contact sheet to visually inspect all original business/IT icons.
use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/yydraw-business-icons.png".into());
    let mut doc = Document {
        font_size: 11.,
        ..Default::default()
    };
    for (cell, (index, entry)) in catalog::all()
        .iter()
        .enumerate()
        .filter(|(_, e)| e.vendor == "業務・IT")
        .enumerate()
    {
        let asset = catalog::place(&mut doc, index).unwrap();
        let id = doc.add_node(
            Shape::Icon,
            Point::new(
                12. + (cell % 10) as f64 * 124.,
                12. + (cell / 10) as f64 * 110.,
            ),
            Some(asset),
        );
        let node = doc.nodes.iter_mut().find(|n| n.id == id).unwrap();
        node.rect.w = 112.;
        node.rect.h = 96.;
        node.label = entry.name.clone();
    }
    let renderer = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    let bytes = renderer
        .png(
            &doc,
            &Export {
                bounds: Rect {
                    x: 0.,
                    y: 0.,
                    w: 1252.,
                    h: 1112.,
                },
                background: Some(0xffffff),
                grid: false,
                text: false,
                selected: vec![],
            },
            1.,
        )
        .unwrap();
    std::fs::write(&path, bytes).unwrap();
    println!("{path}");
}
