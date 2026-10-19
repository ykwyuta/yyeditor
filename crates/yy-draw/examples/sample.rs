use yy_draw::{
    render::{Export, Renderer},
    *,
};
fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/yydraw-sample".into());
    std::fs::create_dir_all(&out).unwrap();
    let mut d = Document::default();
    assets::add_starter(&mut d);
    for (i, asset) in d.assets.clone().iter().enumerate() {
        let id = d.add_node(
            Shape::Icon,
            Point::new(40. + (i % 3) as f64 * 240., 80. + (i / 3) as f64 * 200.),
            Some(asset.id),
        );
        let n = d.nodes.iter_mut().find(|n| n.id == id).unwrap();
        n.rect.w = 160.;
        n.rect.h = 120.;
    }
    let ids: Vec<_> = d.nodes.iter().map(|n| n.id).collect();
    for row in 0..3 {
        for col in 0..2 {
            d.connect(
                Endpoint::Attached {
                    node: ids[row * 3 + col],
                    side: 1,
                },
                Endpoint::Attached {
                    node: ids[row * 3 + col + 1],
                    side: 3,
                },
                false,
            );
        }
    }
    d.connect(
        Endpoint::Free(Point::new(240., 20.)),
        Endpoint::Free(Point::new(240., 680.)),
        false,
    );
    let id = d.add_node(Shape::Text, Point::new(200., -40.), None);
    let n = d.nodes.iter_mut().find(|n| n.id == id).unwrap();
    n.rect.w = 360.;
    n.label = "yydraw — クラウド構成図".into();
    n.font_size = 24.;
    let r = Renderer::new(Some(include_bytes!(
        "../../yy-win/fonts/UDEVGothic-Regular.ttf"
    )));
    let opts = Export {
        background: Some(0xffffff),
        grid: true,
        ..Export::document(&d)
    };
    let root = std::path::Path::new(&out);
    format::save(&root.join("sample.yyd"), &d).unwrap();
    format::atomic_write(
        &root.join("sample.svg"),
        r.svg(&d, &opts).unwrap().as_bytes(),
    )
    .unwrap();
    format::atomic_write(&root.join("sample.png"), &r.png(&d, &opts, 1.).unwrap()).unwrap();
    println!("{}", root.display());
}
