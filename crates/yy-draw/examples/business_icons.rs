//! Render a labeled contact sheet to visually inspect all original business/IT icons.
use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/yydraw-business-icons.png".into());
    // A contact sheet has 100 cells; pass 1 or 2 for subsequent batches.
    let page: usize = std::env::args()
        .nth(2)
        .map(|v| v.parse().unwrap())
        .unwrap_or(0);
    let category_prefix = std::env::args().nth(3).unwrap_or_default();
    let mut doc = Document {
        font_size: 11.,
        ..Default::default()
    };
    let mut count: usize = 0;
    for (cell, (index, entry)) in catalog::all()
        .iter()
        .enumerate()
        .filter(|(_, e)| e.vendor == "業務・IT" && e.category.starts_with(&category_prefix))
        .skip(page * 100)
        .take(100)
        .enumerate()
    {
        count = cell + 1;
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
        // Keep long Japanese names within their contact-sheet cell.
        let mut columns = 0;
        node.label.clear();
        for c in entry.name.chars() {
            let width = if c.is_ascii() { 1 } else { 2 };
            if columns + width > 20 {
                node.label.push('\n');
                columns = 0;
            }
            node.label.push(c);
            columns += width;
        }
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
                    h: 12. + count.div_ceil(10) as f64 * 110.,
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
