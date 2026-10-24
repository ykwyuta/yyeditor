use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use resvg::{tiny_skia, usvg};
use std::sync::{Arc, Mutex};

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
/// Windows DIBV5 with straight-alpha BGRA, for image paste into other applications.
pub fn png_dib(png: &[u8]) -> Result<Vec<u8>> {
    let image = tiny_skia::Pixmap::decode_png(png).map_err(|e| e.to_string())?;
    let w = image.width();
    let h = image.height();
    check_size(w, h)?;
    let mut data = vec![0u8; 124];
    data[0..4].copy_from_slice(&124u32.to_le_bytes());
    data[4..8].copy_from_slice(&(w as i32).to_le_bytes());
    data[8..12].copy_from_slice(&(-(h as i32)).to_le_bytes());
    data[12..14].copy_from_slice(&1u16.to_le_bytes());
    data[14..16].copy_from_slice(&32u16.to_le_bytes());
    data[16..20].copy_from_slice(&3u32.to_le_bytes());
    data[20..24].copy_from_slice(&(w * h * 4).to_le_bytes());
    for (i, mask) in [
        0x00ff0000u32,
        0x0000ff00,
        0x000000ff,
        0xff000000,
        0x73524742,
    ]
    .into_iter()
    .enumerate()
    {
        data[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
    }
    for px in image.data().chunks_exact(4) {
        let a = u32::from(px[3]);
        let straight = |v: u8| {
            (u32::from(v) * 255 + a / 2)
                .checked_div(a)
                .unwrap_or(0)
                .min(255) as u8
        };
        data.extend_from_slice(&[straight(px[2]), straight(px[1]), straight(px[0]), px[3]]);
    }
    Ok(data)
}
fn color(c: u32) -> String {
    format!("#{:06x}", c & 0xffffff)
}
pub fn validate_svg(svg: &str) -> Result<()> {
    if svg.len() > 8 * 1024 * 1024 {
        return Err("SVG は 8 MiB までです".into());
    }
    let xml = roxmltree::Document::parse(svg).map_err(|e| e.to_string())?;
    if xml.root_element().tag_name().name() != "svg" {
        return Err("SVG ではありません".into());
    }
    if xml.descendants().count() > 100_000 {
        return Err("SVG の要素数が多すぎます".into());
    }
    for n in xml.descendants().filter(|n| n.is_element()) {
        if matches!(
            n.tag_name().name(),
            "script" | "foreignObject" | "animate" | "animateTransform" | "set"
        ) {
            return Err("動的な SVG は使用できません".into());
        }
        for a in n.attributes() {
            let v = a.value().trim();
            if a.name().starts_with("on")
                || a.name() == "href"
                    && !(v.starts_with('#')
                        || v.starts_with("data:image/png;base64,")
                        || v.starts_with("data:image/jpeg;base64,"))
            {
                return Err("SVG の外部参照・イベントは使用できません".into());
            }
        }
        let text = n.text().unwrap_or("");
        if text.contains("@import") || text.contains("url(http") || text.contains("url(file:") {
            return Err("SVG の外部スタイルは使用できません".into());
        }
    }
    Ok(())
}
#[derive(Clone)]
pub struct Renderer {
    fonts: Arc<usvg::fontdb::Database>,
    cache: Arc<Mutex<Option<Prepared>>>,
    symbols: Arc<Mutex<std::collections::HashMap<Id, CachedSymbol>>>,
}
struct CachedSymbol {
    source: String,
    tree: Arc<usvg::Tree>,
    raster: Option<((u64, u32, u32), tiny_skia::Pixmap)>,
}
struct Prepared {
    doc: Document,
    tree: Arc<usvg::Tree>,
    bounds: Rect,
}
impl Renderer {
    pub fn new(bundled_font: Option<&[u8]>) -> Self {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        if let Some(bytes) = bundled_font {
            fonts.load_font_data(bytes.to_vec());
        }
        Self {
            fonts: Arc::new(fonts),
            cache: Arc::new(Mutex::new(None)),
            symbols: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
    fn options(&self) -> usvg::Options<'static> {
        let mut o = usvg::Options {
            fontdb: self.fonts.clone(),
            ..Default::default()
        };
        o.image_href_resolver.resolve_string = Box::new(|_, _| None);
        o
    }
    pub fn thumbnail(&self, svg: &str, size: u32) -> Result<tiny_skia::Pixmap> {
        check_size(size, size)?;
        let tree = usvg::Tree::from_str(svg, &self.options()).map_err(|e| e.to_string())?;
        let mut image = tiny_skia::Pixmap::new(size, size).ok_or("画像のメモリを確保できません")?;
        let scale = (size as f32 / tree.size().width()).min(size as f32 / tree.size().height());
        let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(
            (size as f32 - tree.size().width() * scale) / 2.,
            (size as f32 - tree.size().height() * scale) / 2.,
        );
        resvg::render(&tree, transform, &mut image.as_mut());
        Ok(image)
    }
    pub fn normalize_asset(
        &self,
        bytes: &[u8],
        name: String,
        source: String,
        id: Id,
    ) -> Result<Asset> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("素材は 8 MiB までです".into());
        }
        let svg = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            if bytes.len() < 24 {
                return Err("不正な PNG です".into());
            }
            let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
            check_size(w, h)?;
            tiny_skia::Pixmap::decode_png(bytes).map_err(|e| e.to_string())?;
            format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'><image width='{w}' height='{h}' href='data:image/png;base64,{}'/></svg>",
                STANDARD.encode(bytes)
            )
        } else {
            let s = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            validate_svg(s)?;
            s.to_owned()
        };
        let tree = usvg::Tree::from_str(&svg, &self.options()).map_err(|e| e.to_string())?;
        let svg = tree.to_string(&usvg::WriteOptions::default());
        Ok(Asset {
            id,
            name,
            source,
            svg,
        })
    }
    pub fn svg(&self, doc: &Document, options: &Export) -> Result<String> {
        let src = scene(doc, options);
        let tree = usvg::Tree::from_str(&src, &self.options()).map_err(|e| e.to_string())?;
        Ok(tree.to_string(&usvg::WriteOptions {
            preserve_text: options.text,
            ..Default::default()
        }))
    }
    pub fn raster(
        &self,
        doc: &Document,
        options: &Export,
        scale: f64,
    ) -> Result<tiny_skia::Pixmap> {
        if !scale.is_finite() || scale <= 0. {
            return Err("出力倍率が不正です".into());
        }
        let w = (options.bounds.w * scale).ceil();
        let h = (options.bounds.h * scale).ceil();
        if w > 32768. || h > 32768. {
            return Err("画像の最大辺は 32768 px です".into());
        }
        check_size(w as u32, h as u32)?;
        let mut pixmap =
            tiny_skia::Pixmap::new(w as u32, h as u32).ok_or("画像のメモリを確保できません")?;
        let cached = {
            let cache = self.cache.lock().unwrap();
            cache
                .as_ref()
                .filter(|p| p.doc == *doc)
                .map(|p| (p.tree.clone(), p.bounds))
        };
        let (tree, bounds) = if let Some(v) = cached {
            v
        } else {
            let bounds = doc.bounds();
            let options = Export {
                bounds,
                background: None,
                grid: false,
                text: false,
                selected: vec![],
            };
            let src = scene(doc, &options);
            let tree =
                Arc::new(usvg::Tree::from_str(&src, &self.options()).map_err(|e| e.to_string())?);
            *self.cache.lock().unwrap() = Some(Prepared {
                doc: doc.clone(),
                tree: tree.clone(),
                bounds,
            });
            (tree, bounds)
        };
        let scale_transform = tiny_skia::Transform::from_scale(scale as f32, scale as f32);
        if options.grid || options.background.is_some() {
            let empty = Document {
                grid: doc.grid,
                ..Default::default()
            };
            let src = scene(
                &empty,
                &Export {
                    selected: vec![],
                    ..options.clone()
                },
            );
            let bg = usvg::Tree::from_str(&src, &self.options()).map_err(|e| e.to_string())?;
            resvg::render(&bg, scale_transform, &mut pixmap.as_mut());
        }
        let transform = scale_transform.post_translate(
            ((bounds.x - options.bounds.x) * scale) as f32,
            ((bounds.y - options.bounds.y) * scale) as f32,
        );
        render_visible(
            tree.root(),
            transform,
            tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        if !options.selected.is_empty() {
            let overlay = selection_svg(doc, options);
            let overlay =
                usvg::Tree::from_str(&overlay, &self.options()).map_err(|e| e.to_string())?;
            resvg::render(&overlay, scale_transform, &mut pixmap.as_mut());
        }
        Ok(pixmap)
    }
    pub fn png(&self, doc: &Document, options: &Export, scale: f64) -> Result<Vec<u8>> {
        self.raster(doc, options, scale)?
            .encode_png()
            .map_err(|e| e.to_string())
    }
    /// Flat diagram viewport: reuse independently parsed icon SVGs while geometry moves.
    /// Other shapes, routed/rounded/arrows/dashed lines and grid use the general renderer.
    /// This preserves the full SVG/PNG export path and its document-level semantics.
    pub fn raster_flat_icons(
        &self,
        doc: &Document,
        options: &Export,
        scale: f64,
    ) -> Result<tiny_skia::Pixmap> {
        if options.grid
            || doc
                .nodes
                .iter()
                .any(|n| n.shape != Shape::Icon || !n.label.is_empty())
            || doc.connectors.iter().any(|c| {
                c.orthogonal
                    || c.arrow_start
                    || c.arrow_end
                    || c.dashed
                    || c.corner_radius != 0.
                    || c.jump != Jump::Never
                    || !c.label.is_empty()
                    || !matches!(c.start, Endpoint::Free(_))
                    || !matches!(c.end, Endpoint::Free(_))
            })
        {
            return self.raster(doc, options, scale);
        }
        if !scale.is_finite() || scale <= 0. {
            return Err("出力倍率が不正です".into());
        }
        let w = (options.bounds.w * scale).ceil();
        let h = (options.bounds.h * scale).ceil();
        if w > 32768. || h > 32768. {
            return Err("画像の最大辺は 32768 px です".into());
        }
        check_size(w as u32, h as u32)?;
        let mut pixmap =
            tiny_skia::Pixmap::new(w as u32, h as u32).ok_or("画像のメモリを確保できません")?;
        if let Some(bg) = options.background {
            pixmap.fill(tiny_skia::Color::from_rgba8(
                (bg >> 16) as u8,
                (bg >> 8) as u8,
                bg as u8,
                255,
            ));
        }
        let transform = tiny_skia::Transform::from_scale(scale as f32, scale as f32)
            .post_translate(
                (-options.bounds.x * scale) as f32,
                (-options.bounds.y * scale) as f32,
            );
        let mut paint = tiny_skia::Paint::default();
        let clip = options.bounds.expand(8. / scale);
        for c in &doc.connectors {
            let (Endpoint::Free(start), Endpoint::Free(end)) = (c.start, c.end) else {
                unreachable!()
            };
            let points = std::iter::once(start)
                .chain(c.waypoints.iter().copied())
                .chain(std::iter::once(end));
            let mut builder = tiny_skia::PathBuilder::new();
            builder.move_to(start.x as f32, start.y as f32);
            let mut visible = false;
            let mut last = start;
            for p in points.skip(1) {
                visible |= router::overlap(
                    Rect {
                        x: last.x.min(p.x),
                        y: last.y.min(p.y),
                        w: (last.x - p.x).abs().max(1.),
                        h: (last.y - p.y).abs().max(1.),
                    },
                    clip,
                );
                builder.line_to(p.x as f32, p.y as f32);
                last = p;
            }
            if !visible {
                continue;
            }
            if let Some(path) = builder.finish() {
                paint.set_color_rgba8(
                    (c.color >> 16) as u8,
                    (c.color >> 8) as u8,
                    c.color as u8,
                    255,
                );
                let stroke = tiny_skia::Stroke {
                    width: c.width as f32,
                    ..Default::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, transform, None);
            }
        }
        let assets = doc
            .assets
            .iter()
            .map(|a| (a.id, a))
            .collect::<std::collections::HashMap<_, _>>();
        let mut cache = self.symbols.lock().unwrap();
        cache.retain(|id, _| assets.contains_key(id));
        for n in doc.nodes.iter().filter(|n| router::overlap(n.rect, clip)) {
            let Some(asset) = n.asset.and_then(|id| assets.get(&id)) else {
                continue;
            };
            if cache
                .get(&asset.id)
                .is_none_or(|entry| entry.source != asset.svg)
            {
                let tree =
                    usvg::Tree::from_str(&asset.svg, &self.options()).map_err(|e| e.to_string())?;
                cache.insert(
                    asset.id,
                    CachedSymbol {
                        source: asset.svg.clone(),
                        tree: Arc::new(tree),
                        raster: None,
                    },
                );
            }
            let tree = cache[&asset.id].tree.clone();
            let tw = tree.size().width() as f64;
            let th = tree.size().height() as f64;
            let height = (n.rect.h - 24.).max(8.);
            let fit = (n.rect.w / tw).min(height / th);
            let x = (n.rect.x - options.bounds.x + (n.rect.w - tw * fit) / 2.) * scale;
            let y = (n.rect.y - options.bounds.y + (height - th * fit) / 2.) * scale;
            let t = tiny_skia::Transform::from_scale((fit * scale) as f32, (fit * scale) as f32)
                .post_translate(x as f32, y as f32);
            let raster_scale = fit * scale;
            // Tiny-skia snaps translated bitmap patterns to integer pixels. Rasterize
            // the fractional phase into the cached symbol, then blit at integer pixels.
            let phase_x = (x - x.floor()) as f32;
            let phase_y = (y - y.floor()) as f32;
            let raster_key = (raster_scale.to_bits(), phase_x.to_bits(), phase_y.to_bits());
            let rw = (tw * raster_scale).ceil();
            let rh = (th * raster_scale).ceil();
            // Bound the bitmap cache to 64 MiB. Oversized table cards use vectors.
            let bytes = (rw + 2.) * (rh + 2.) * 4.;
            let cached = cache[&asset.id]
                .raster
                .as_ref()
                .is_some_and(|(key, _)| *key == raster_key);
            if !cached
                && rw > 0.
                && rh > 0.
                && rw <= 8192.
                && rh <= 8192.
                && bytes <= 16. * 1024. * 1024.
            {
                let used = cache
                    .values()
                    .filter_map(|v| v.raster.as_ref())
                    .map(|(_, p)| p.data().len())
                    .sum::<usize>();
                if used + bytes as usize > 64 * 1024 * 1024 {
                    for v in cache.values_mut() {
                        v.raster = None;
                    }
                }
                if let Some(mut symbol) = tiny_skia::Pixmap::new(rw as u32 + 2, rh as u32 + 2) {
                    resvg::render(
                        &tree,
                        tiny_skia::Transform::from_scale(raster_scale as f32, raster_scale as f32)
                            .post_translate(1. + phase_x, 1. + phase_y),
                        &mut symbol.as_mut(),
                    );
                    cache.get_mut(&asset.id).unwrap().raster = Some((raster_key, symbol));
                }
            }
            if let Some((_, image)) = cache[&asset.id]
                .raster
                .as_ref()
                .filter(|(key, _)| *key == raster_key)
            {
                let paint = tiny_skia::PixmapPaint {
                    quality: tiny_skia::FilterQuality::Nearest,
                    ..Default::default()
                };
                pixmap.draw_pixmap(
                    x.floor() as i32 - 1,
                    y.floor() as i32 - 1,
                    image.as_ref(),
                    &paint,
                    tiny_skia::Transform::identity(),
                    None,
                );
            } else {
                resvg::render(&tree, t, &mut pixmap.as_mut());
            }
        }
        if !options.selected.is_empty() {
            let overlay = usvg::Tree::from_str(&selection_svg(doc, options), &self.options())
                .map_err(|e| e.to_string())?;
            resvg::render(
                &overlay,
                tiny_skia::Transform::from_scale(scale as f32, scale as f32),
                &mut pixmap.as_mut(),
            );
        }
        Ok(pixmap)
    }
}
// Resvg visits every path, including offscreen paths. Keep compositing groups intact,
// but skip invisible leaves before rasterizing large documents.
fn render_visible(
    group: &usvg::Group,
    root: tiny_skia::Transform,
    parent: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut<'_>,
) {
    for node in group.children() {
        if let usvg::Node::Image(image) = node
            && let usvg::ImageKind::SVG(tree) = image.kind()
        {
            // Resvg normally allocates a canvas-sized intermediate for every SVG
            // image. Rasterize only the transformed icon viewport instead.
            if !image.is_visible() {
                continue;
            }
            let transform = root.pre_concat(parent);
            if let Some(r) =
                tiny_skia::Rect::from_xywh(0., 0., tree.size().width(), tree.size().height())
                    .and_then(|r| r.transform(transform))
            {
                let left = (r.left().floor() - 2.).max(-2.) as i32;
                let top = (r.top().floor() - 2.).max(-2.) as i32;
                let right = (r.right().ceil() + 2.).min(pixmap.width() as f32 + 2.) as i32;
                let bottom = (r.bottom().ceil() + 2.).min(pixmap.height() as f32 + 2.) as i32;
                if right > left && bottom > top {
                    if let Some(mut icon) =
                        tiny_skia::Pixmap::new((right - left) as u32, (bottom - top) as u32)
                    {
                        resvg::render(
                            tree,
                            transform.post_translate(-left as f32, -top as f32),
                            &mut icon.as_mut(),
                        );
                        pixmap.draw_pixmap(
                            left,
                            top,
                            icon.as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            continue;
        }
        let Some(bounds) = node.abs_layer_bounding_box() else {
            continue;
        };
        let cull_bounds = if matches!(node, usvg::Node::Path(_) | usvg::Node::Text(_)) {
            node.abs_stroke_bounding_box()
                .to_non_zero_rect()
                .unwrap_or(bounds)
        } else {
            bounds
        };
        let Some(screen) = cull_bounds.transform(root) else {
            continue;
        };
        if matches!(node, usvg::Node::Path(_) | usvg::Node::Text(_))
            && (screen.right() < -2.
                || screen.bottom() < -2.
                || screen.x() > pixmap.width() as f32 + 2.
                || screen.y() > pixmap.height() as f32 + 2.)
        {
            continue;
        }
        if let usvg::Node::Group(child) = node {
            if child
                .children()
                .iter()
                .all(|n| matches!(n, usvg::Node::Path(_) | usvg::Node::Text(_)))
                && (screen.right() < -2.
                    || screen.bottom() < -2.
                    || screen.x() > pixmap.width() as f32 + 2.
                    || screen.y() > pixmap.height() as f32 + 2.)
            {
                continue;
            }
            if !child.should_isolate()
                && child
                    .children()
                    .iter()
                    .all(|n| n.abs_layer_bounding_box().is_some())
            {
                render_visible(child, root, parent.pre_concat(child.transform()), pixmap);
                continue;
            }
        }
        // render_node normally relocates the node to its bounding-box origin.
        // Cancel that relocation, preserving the document and ancestor transforms.
        resvg::render_node(
            node,
            root.pre_concat(parent)
                .pre_translate(bounds.x(), bounds.y()),
            pixmap,
        );
    }
}
fn selection_svg(doc: &Document, options: &Export) -> String {
    let r = options.bounds;
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}' viewBox='{} {} {} {}'>",
        r.w, r.h, r.x, r.y, r.w, r.h
    );
    s.push_str(&selection_elements(doc, &options.selected, options.bounds));
    s.push_str("</svg>");
    s
}
fn selection_elements(doc: &Document, ids: &[Id], clip: Rect) -> String {
    let mut s = String::new();
    let selected: HashSet<_> = ids.iter().copied().collect();
    let mut groups = std::collections::HashMap::<Id, (usize, usize)>::new();
    for (id, group) in doc
        .nodes
        .iter()
        .map(|n| (n.id, n.group))
        .chain(doc.connectors.iter().map(|c| (c.id, c.group)))
    {
        if let Some(group) = group {
            let count = groups.entry(group).or_default();
            count.0 += 1;
            count.1 += usize::from(selected.contains(&id));
        }
    }
    let complete: HashSet<_> = groups
        .into_iter()
        .filter_map(|(g, (all, count))| (all == count && count > 1).then_some(g))
        .collect();
    if !complete.is_empty() {
        for u in doc
            .layout_units(ids)
            .into_iter()
            .filter(|u| u.group.is_some_and(|g| complete.contains(&g)))
        {
            let b = u.bounds.expand(5.);
            if !router::overlap(b, clip) {
                continue;
            }
            s.push_str(&format!("<rect x='{}' y='{}' width='{}' height='{}' fill='none' stroke='#3974d7' stroke-width='1.5' stroke-dasharray='6 3'/>",b.x,b.y,b.w,b.h));
        }
    }
    for n in doc
        .nodes
        .iter()
        .filter(|n| selected.contains(&n.id) && router::overlap(n.rect.expand(6.), clip))
    {
        let b = n.rect.expand(3.);
        s.push_str(&format!("<rect x='{}' y='{}' width='{}' height='{}' fill='none' stroke='#3974d7' stroke-dasharray='4 3'/>",b.x,b.y,b.w,b.h));
        if !n.group.is_some_and(|g| complete.contains(&g)) {
            s.push_str(&format!(
                "<rect x='{}' y='{}' width='7' height='7' fill='white' stroke='#3974d7'/>",
                n.rect.x + n.rect.w - 3.5,
                n.rect.y + n.rect.h - 3.5
            ));
        }
    }
    let nodes: std::collections::HashMap<_, _> = doc.nodes.iter().map(|n| (n.id, n)).collect();
    let endpoint = |e| match e {
        Endpoint::Free(p) => p,
        Endpoint::Attached { node, side } => nodes
            .get(&node)
            .map_or(Point::default(), |n| ports::point(n, side)),
    };
    for c in doc.connectors.iter().filter(|c| selected.contains(&c.id)) {
        for p in [endpoint(c.start), endpoint(c.end)]
            .into_iter()
            .chain(c.waypoints.iter().copied())
        {
            if !clip.expand(5.).contains(p) {
                continue;
            }
            s.push_str(&format!(
                "<circle cx='{}' cy='{}' r='4' fill='white' stroke='#3974d7'/>",
                p.x, p.y
            ));
        }
    }
    s
}
fn check_size(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > 32768 || h > 32768 || u64::from(w) * u64::from(h) > 64 * 1024 * 1024
    {
        Err("画像サイズの上限を超えています".into())
    } else {
        Ok(())
    }
}
#[derive(Clone)]
pub struct Export {
    pub bounds: Rect,
    pub background: Option<u32>,
    pub grid: bool,
    pub text: bool,
    pub selected: Vec<Id>,
}
impl Export {
    pub fn document(doc: &Document) -> Self {
        Self {
            bounds: doc.bounds(),
            background: None,
            grid: false,
            text: false,
            selected: vec![],
        }
    }
}
pub fn scene(doc: &Document, options: &Export) -> String {
    let r = options.bounds;
    let mut s = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='{}' height='{}' viewBox='{} {} {} {}'>",
        r.w, r.h, r.x, r.y, r.w, r.h
    );
    if let Some(bg) = options.background {
        s.push_str(&format!(
            "<rect x='{}' y='{}' width='{}' height='{}' fill='{}'/>",
            r.x,
            r.y,
            r.w,
            r.h,
            color(bg)
        ));
    }
    if options.grid {
        let mut step = doc.grid;
        while r.w / step + r.h / step > 1000. {
            step *= 5.;
        }
        let major = step * 5.;
        for (spacing, color, width) in [(step, "#e4e9ef", 0.5), (major, "#cdd8e4", 0.7)] {
            let mut path = String::new();
            let first = (r.x / spacing).floor() as i64;
            let last = ((r.x + r.w) / spacing).ceil() as i64;
            for i in first..=last {
                let x = i as f64 * spacing;
                path.push_str(&format!("M {x} {} V {} ", r.y, r.y + r.h));
            }
            let first = (r.y / spacing).floor() as i64;
            let last = ((r.y + r.h) / spacing).ceil() as i64;
            for i in first..=last {
                let y = i as f64 * spacing;
                path.push_str(&format!("M {} {y} H {} ", r.x, r.x + r.w));
            }
            s.push_str(&format!(
                "<path d='{path}' fill='none' stroke='{color}' stroke-width='{width}'/>"
            ));
        }
    }
    let routes = router::Router::new(doc).all();
    let points = routes.iter().map(|r| r.points.clone()).collect::<Vec<_>>();
    let paths = geometry::connector_paths_for(doc, &points);
    for (i, c) in doc.connectors.iter().enumerate() {
        s.push_str(&format!("<g id='line{}'>", c.id));
        let path = &paths[i];
        let dash = if c.dashed {
            " stroke-dasharray='6 4'"
        } else {
            ""
        };
        s.push_str(&format!(
            "<path d='{path}' fill='none' stroke='{}' stroke-width='{}'{dash}/>",
            color(if routes[i].resolved {
                c.color
            } else {
                0xc53232
            }),
            c.width
        ));
        let points = &points[i];
        if points.len() > 1 {
            if c.arrow_end {
                arrow(
                    &mut s,
                    points[points.len() - 2],
                    points[points.len() - 1],
                    c.color,
                    c.width,
                );
            }
            if c.arrow_start {
                arrow(&mut s, points[1], points[0], c.color, c.width);
            }
            let p = points[points.len() / 2];
            label(
                &mut s,
                &doc.font_family,
                14.,
                Rect {
                    x: p.x - 80.,
                    y: p.y - 30.,
                    w: 160.,
                    h: 24.,
                },
                &c.label,
                0x243044,
            );
        }
        s.push_str("</g>");
    }
    for n in &doc.nodes {
        let r = n.rect;
        let x = r.x;
        let y = r.y;
        let w = r.w;
        let h = r.h;
        let attrs = format!(
            "fill='{}' stroke='{}' stroke-width='{}'",
            if matches!(n.shape, Shape::Text | Shape::Frame) {
                "none".into()
            } else {
                color(n.fill)
            },
            color(n.stroke),
            n.line_width
        );
        match n.shape{
            Shape::Rectangle|Shape::Rounded|Shape::Frame=>s.push_str(&format!("<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='{}' {attrs}/>",if n.shape==Shape::Rounded{12.}else{0.})),
            Shape::Ellipse=>s.push_str(&format!("<ellipse cx='{}' cy='{}' rx='{}' ry='{}' {attrs}/>",x+w/2.,y+h/2.,w/2.,h/2.)),
            Shape::Diamond=>s.push_str(&format!("<path d='M {} {y} L {} {} L {} {} L {x} {} Z' {attrs}/>",x+w/2.,x+w,y+h/2.,x+w/2.,y+h,y+h/2.)),
            Shape::Triangle=>s.push_str(&format!("<path d='M {} {y} L {} {} H {x} Z' {attrs}/>",x+w/2.,x+w,y+h)),
            Shape::Cylinder=>{
                let ry=(h/6.).min(w/4.);
                s.push_str(&format!("<path d='M {x} {} V {} A {} {ry} 0 0 0 {} {} V {} Z' {attrs}/><ellipse cx='{}' cy='{}' rx='{}' ry='{ry}' {attrs}/>",y+ry,y+h-ry,w/2.,x+w,y+h-ry,y+ry,x+w/2.,y+ry,w/2.));
            },
            Shape::Cloud=>s.push_str(&format!("<path transform='translate({x} {y}) scale({} {})' d='M 18 65 C -5 65 -5 30 18 30 C 12 0 55 -8 62 15 C 88 0 105 25 90 38 C 115 55 96 80 78 70 H 20 Z' {attrs}/>",w/100.,h/80.)),
            Shape::Icon=>{if let Some(a)=doc.assets.iter().find(|a|Some(a.id)==n.asset){s.push_str(&format!("<image x='{x}' y='{y}' width='{w}' height='{}' preserveAspectRatio='xMidYMid meet' href='data:image/svg+xml;base64,{}'/>",(h-24.).max(8.),STANDARD.encode(a.svg.as_bytes())));}},
            Shape::Text=>{}
            Shape::Junction=>s.push_str(&format!("<circle cx='{}' cy='{}' r='{}' fill='#243044'/>",x+w/2.,y+h/2.,w.min(h)/2.)),
            shape=>{
                let geometry=shapes::geometry(shape).expect("extended shape geometry");
                s.push_str(&format!("<path d='{}' fill-rule='evenodd' {attrs}/>",geometry.outline.svg(r)));
                s.push_str(&format!("<path d='{}' fill='none' stroke='{}' stroke-width='{}'/>",geometry.detail.svg(r),color(n.stroke),n.line_width));
            },
        }
        let text_rect = if n.shape == Shape::Icon {
            Rect {
                x,
                y: y + h - 24.,
                w,
                h: 24.,
            }
        } else if let Some(geometry) = shapes::geometry(n.shape) {
            geometry.text_rect(r)
        } else {
            r
        };
        label(
            &mut s,
            &doc.font_family,
            n.font_size,
            text_rect,
            &n.label,
            0x243044,
        );
    }
    if !options.selected.is_empty() {
        s.push_str(&selection_elements(doc, &options.selected, options.bounds));
    }
    s.push_str("</svg>");
    s
}
fn arrow(s: &mut String, a: Point, b: Point, c: u32, width: f64) {
    let l = a.distance(b);
    if l < 0.01 {
        return;
    }
    let ux = (b.x - a.x) / l;
    let uy = (b.y - a.y) / l;
    let size = 8. + width;
    s.push_str(&format!(
        "<path d='M {} {} L {} {} L {} {} Z' fill='{}'/>",
        b.x,
        b.y,
        b.x - ux * size - uy * size / 2.,
        b.y - uy * size + ux * size / 2.,
        b.x - ux * size + uy * size / 2.,
        b.y - uy * size - ux * size / 2.,
        color(c)
    ));
}
fn label(s: &mut String, family: &str, size: f64, r: Rect, text: &str, c: u32) {
    if text.is_empty() {
        return;
    }
    let lines: Vec<_> = text.lines().collect();
    let top = r.y + r.h / 2. - (lines.len().saturating_sub(1) as f64) * size * 0.65 + size * 0.35;
    for (i, line) in lines.iter().enumerate() {
        s.push_str(&format!("<text x='{}' y='{}' font-family='{}' font-size='{size}' text-anchor='middle' fill='{}'>{}</text>",r.x+r.w/2.,top+i as f64*size*1.3,escape(family),color(c),escape(line)));
    }
}
#[cfg(test)]
mod flat_tests {
    use super::*;
    #[test]
    fn icon_cache_handles_motion_source_changes_zoom_and_offscreen_assets() {
        let renderer = Renderer::new(None);
        let mut doc = Document {
            snap: false,
            jumps: false,
            ..Default::default()
        };
        doc.assets.push(Asset{id:1,name:"square".into(),source:String::new(),svg:"<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'><rect width='20' height='20' fill='red'/></svg>".into()});
        doc.next_id = 2;
        doc.add_node(Shape::Icon, Point::new(10., 10.), Some(1));
        doc.nodes[0].label.clear();
        doc.nodes[0].rect.w = 20.;
        doc.nodes[0].rect.h = 44.;
        let options = Export {
            bounds: Rect {
                x: 0.,
                y: 0.,
                w: 120.,
                h: 80.,
            },
            background: Some(0xffffff),
            grid: false,
            text: false,
            selected: vec![],
        };
        let image = renderer.raster_flat_icons(&doc, &options, 1.).unwrap();
        assert_eq!(image.pixel(15, 15).unwrap().red(), 255);
        assert_eq!(image.pixel(15, 15).unwrap().green(), 0);
        doc.nodes[0].rect.x = 80.;
        doc.assets[0].svg = doc.assets[0].svg.replace("red", "blue");
        let moved = renderer.raster_flat_icons(&doc, &options, 1.).unwrap();
        assert_eq!(moved.pixel(15, 15).unwrap().green(), 255);
        assert_eq!(moved.pixel(85, 15).unwrap().blue(), 255);
        assert_eq!(moved.pixel(85, 15).unwrap().red(), 0);
        for scale in [0.5, 1.25, 2.] {
            let flat = renderer.raster_flat_icons(&doc, &options, scale).unwrap();
            let full = renderer.raster(&doc, &options, scale).unwrap();
            // Fractional transforms can round an antialiased edge by one channel level.
            let difference = flat
                .data()
                .iter()
                .zip(full.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            assert!(
                difference <= 1,
                "scale {scale}: maximum channel difference {difference}; {:?}",
                flat.data()
                    .chunks_exact(4)
                    .zip(full.data().chunks_exact(4))
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .take(6)
                    .map(|(i, (a, b))| (i % flat.width() as usize, i / flat.width() as usize, a, b))
                    .collect::<Vec<_>>()
            );
        }
        doc.assets.push(Asset {
            id: 3,
            name: "unseen".into(),
            source: String::new(),
            svg: "invalid SVG".into(),
        });
        doc.next_id = 4;
        doc.add_node(Shape::Icon, Point::new(10000., 10000.), Some(3));
        doc.nodes[1].label.clear();
        assert!(renderer.raster_flat_icons(&doc, &options, 1.).is_ok());
        let pan = Export {
            bounds: Rect {
                x: 9990.,
                y: 9990.,
                w: 120.,
                h: 80.,
            },
            ..options.clone()
        };
        assert!(renderer.raster_flat_icons(&doc, &pan, 1.).is_err());
        doc.nodes.pop();
        doc.nodes[0].shape = Shape::Rectangle;
        let fallback = renderer.raster_flat_icons(&doc, &options, 1.).unwrap();
        assert_eq!(
            fallback.data(),
            renderer.raster(&doc, &options, 1.).unwrap().data()
        );
    }
}
