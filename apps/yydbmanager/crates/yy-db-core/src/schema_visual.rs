use super::*;
use unicode_width::UnicodeWidthStr;
use yy_draw::{Asset, Document, Endpoint, Jump, Point, Shape};
const HEADER: f64 = 40.;
const ROW: f64 = 26.;
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn name(physical: &str, logical: &str, mode: Names) -> String {
    match mode {
        Names::Physical => physical.into(),
        Names::Logical if !logical.is_empty() => logical.into(),
        Names::Both if !logical.is_empty() => format!("{logical} ({physical})"),
        _ => physical.into(),
    }
}
fn fields<'a>(m: &Model, e: &'a Entity) -> Vec<&'a Field> {
    let mut fields = e
        .fields
        .iter()
        .filter(|f| match m.level {
            Level::Entity => false,
            Level::Keys => e
                .keys
                .iter()
                .any(|k| k.primary && k.columns.contains(&f.name)),
            Level::Important => m.important_field(e, f),
            _ => true,
        })
        .collect::<Vec<_>>();
    fields.sort_by_key(|f| {
        !e.keys
            .iter()
            .any(|k| k.primary && k.columns.contains(&f.name))
    });
    fields
}
pub(super) fn size(m: &Model, e: &Entity) -> TableSize {
    if let Some(size) = e.size {
        return size;
    }
    let rows = fields(m, e);
    let title = name(&e.id.display(), &e.logical_name, m.names);
    let names = rows
        .iter()
        .map(|f| {
            UnicodeWidthStr::width(name(&f.name, &f.logical_name, m.names).as_str()) as f64 * 8.
                + 20.
        })
        .fold(64., f64::max);
    let types = if matches!(m.level, Level::Types | Level::Important) {
        rows.iter()
            .map(|f| UnicodeWidthStr::width(f.data_type.as_str()) as f64 * 8. + 36.)
            .fold(64., f64::max)
    } else {
        0.
    };
    TableSize {
        width: (65. + names + types + 16.)
            .max(UnicodeWidthStr::width(title.as_str()) as f64 * 8. + 32.)
            .clamp(240., 10000.),
        height: HEADER + ROW * rows.len() as f64 + 8.,
    }
}
pub(super) fn height(m: &Model, e: &Entity) -> f64 {
    size(m, e).height
}
fn text(svg: &mut String, x: f64, y: f64, value: &str, bold: bool) {
    svg.push_str(&format!("<text x='{x}' y='{y}' font-family='UDEV Gothic' font-size='13' font-weight='{}' fill='#203448'>{}</text>",if bold {"bold"}else{"normal"},escape(value)));
}
pub(super) fn drawing(m: &Model) -> Document {
    let mut doc = Document {
        show_grid: true,
        snap: false,
        jumps: false,
        ..Default::default()
    };
    let sizes = m.entities.iter().map(|e| size(m, e)).collect::<Vec<_>>();
    for (index, e) in m.entities.iter().enumerate().filter(|(_, e)| e.visible) {
        let TableSize {
            width: w,
            height: h,
        } = sizes[index];
        let inner = w - 2.;
        let edge = w - 1.;
        let columns = fields(m, e);
        let name_units = columns
            .iter()
            .map(|f| {
                UnicodeWidthStr::width(name(&f.name, &f.logical_name, m.names).as_str()) as f64
            })
            .fold(8., f64::max);
        let type_units = columns
            .iter()
            .map(|f| UnicodeWidthStr::width(f.data_type.as_str()) as f64 + 3.)
            .fold(8., f64::max);
        let typed = matches!(m.level, Level::Types | Level::Important);
        let name_width = if typed {
            (w - 81.) * (name_units / (name_units + type_units)).clamp(0.35, 0.8)
        } else {
            w - 81.
        };
        let type_x = 65. + name_width + 8.;
        let type_width = (w - type_x - 8.).max(1.);
        let title = name(&e.id.display(), &e.logical_name, m.names);
        let mut svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}' viewBox='0 0 {w} {h}'><defs><clipPath id='card'><rect x='1' y='1' width='{inner}' height='{}'/></clipPath></defs><rect x='1' y='1' width='{inner}' height='{}' fill='white' stroke='#68869f'/><path d='M1 1H{edge}V39H1Z' fill='{}'/><g clip-path='url(#card)'>",
            h - 2.,
            h - 2.,
            if e.view { "#ece7fb" } else { "#e1edf8" }
        );
        text(&mut svg, 12., 25., &title, true);
        let mut last_pk = true;
        for (row, f) in columns.iter().enumerate() {
            let pk = e
                .keys
                .iter()
                .any(|k| k.primary && k.columns.contains(&f.name));
            let fk = m
                .relations
                .iter()
                .any(|r| !r.logical && r.child == e.id && r.columns.contains(&f.name));
            let logical = m.relations.iter().any(|r| {
                r.logical
                    && ((r.child == e.id && r.columns.contains(&f.name))
                        || (r.parent == e.id && r.referenced_columns.contains(&f.name)))
            });
            let uq = e
                .keys
                .iter()
                .any(|k| !k.primary && k.columns.contains(&f.name))
                || e.indexes
                    .iter()
                    .any(|i| i.unique && i.columns.contains(&f.name));
            let y = HEADER + row as f64 * ROW;
            if last_pk && !pk && row > 0 {
                svg.push_str(&format!("<path d='M1 {y}H{edge}' stroke='#68869f'/>"));
            }
            last_pk = pk;
            text(
                &mut svg,
                8.,
                y + 19.,
                &format!(
                    "{}{}{}",
                    if pk {
                        "PK"
                    } else if uq {
                        "UQ"
                    } else {
                        ""
                    },
                    if fk {
                        " FK"
                    } else if logical {
                        " LF"
                    } else {
                        ""
                    },
                    if f.important { "★" } else { "" }
                ),
                pk,
            );
            // Separate clipped name and type columns preserve alignment at long names.
            svg.push_str(&format!(
                "<svg x='65' y='{y}' width='{}' height='26'>",
                name_width
            ));
            text(
                &mut svg,
                0.,
                19.,
                &name(&f.name, &f.logical_name, m.names),
                pk,
            );
            svg.push_str("</svg>");
            if matches!(m.level, Level::Types | Level::Important) {
                svg.push_str(&format!(
                    "<svg x='{type_x}' y='{y}' width='{type_width}' height='26'>"
                ));
                text(
                    &mut svg,
                    0.,
                    19.,
                    &format!("{}{}", f.data_type, if f.nullable { "" } else { " *" }),
                    false,
                );
                svg.push_str("</svg>");
            }
        }
        svg.push_str("</g></svg>");
        let asset = doc.next_id;
        doc.next_id += 1;
        doc.assets.push(Asset {
            id: asset,
            name: title.clone(),
            source: "yydbmanager:er".into(),
            svg,
        });
        let id = doc.add_node(Shape::Icon, e.position, Some(asset));
        let n = doc.nodes.last_mut().unwrap();
        n.id = index as u64 + 1_000_000;
        n.rect.w = w;
        n.rect.h = h + 24.;
        n.label = String::new();
        // IDs are stable across visibility changes and position edits.
        let _ = id;
    }
    // Share the eight possible cardinality symbols across all relationships.
    // Their stable IDs allow a moved endpoint to flip without reparsing SVG.
    for code in 0..8 {
        doc.assets.push(Asset {
            id: 2_000_000 + code,
            name: "cardinality".into(),
            source: "yydbmanager:er".into(),
            svg: marker_svg(code & 1 == 1, code & 2 == 2, code & 4 == 4),
        });
    }
    doc.next_id = 2_000_008;
    for (index, r) in m.relations.iter().enumerate() {
        let Some((ci, c)) = m
            .entities
            .iter()
            .enumerate()
            .find(|(_, e)| e.visible && e.id == r.child)
        else {
            continue;
        };
        let Some((pi, p)) = m
            .entities
            .iter()
            .enumerate()
            .find(|(_, e)| e.visible && e.id == r.parent)
        else {
            continue;
        };
        let cw = sizes[ci].width;
        let pw = sizes[pi].width;
        let same_column = c.position.x < p.position.x + pw && p.position.x < c.position.x + cw;
        let right = same_column || c.position.x < p.position.x;
        let cy = column_y(m, c, &r.columns);
        let py = column_y(m, p, &r.referenced_columns);
        let start = Point::new(
            c.position.x + if right { cw } else { 0. },
            c.position.y + cy,
        );
        let end = Point::new(
            p.position.x + if right && !same_column { 0. } else { pw },
            p.position.y + py,
        );
        let id = doc.connect(Endpoint::Free(start), Endpoint::Free(end), false);
        let e = doc.connectors.iter_mut().find(|e| e.id == id).unwrap();
        e.arrow_end = false;
        e.arrow_start = false;
        e.jump = Jump::Never;
        e.width = 1.4;
        e.color = 0x526c81;
        if r.logical {
            e.dashed = true;
            e.color = 0x9263b5;
        }
        let bend = if same_column {
            (c.position.x + cw).max(p.position.x + pw) + 40. + (index % 5) as f64 * 8.
        } else {
            (start.x + end.x) / 2. + (index % 5) as f64 * 8.
        };
        e.waypoints = vec![Point::new(bend, start.y), Point::new(bend, end.y)];
        let unique = c
            .keys
            .iter()
            .any(|k| k.columns.iter().all(|col| r.columns.contains(col)))
            || c.indexes
                .iter()
                .any(|i| i.unique && i.columns.iter().all(|col| r.columns.contains(col)));
        let mandatory = !r.logical
            && r.columns
                .iter()
                .all(|col| c.fields.iter().any(|f| &f.name == col && !f.nullable));
        // A child can reference exactly one parent (or zero when nullable).
        // A parent can have zero children; total participation cannot be inferred.
        marker(
            &mut doc,
            start,
            if right { 1. } else { -1. },
            false,
            !unique,
        );
        marker(
            &mut doc,
            end,
            if right && !same_column { -1. } else { 1. },
            mandatory,
            r.logical && !document::unique(p, &r.referenced_columns),
        );
    }
    doc
}
fn column_y(m: &Model, e: &Entity, columns: &[String]) -> f64 {
    let visible = fields(m, e);
    columns
        .first()
        .and_then(|c| visible.iter().position(|f| &f.name == c))
        .map(|i| (HEADER + (i as f64 + 0.5) * ROW).min(height(m, e) - 8.))
        .unwrap_or(HEADER / 2.)
}
fn marker(doc: &mut Document, p: Point, direction: f64, mandatory: bool, many: bool) {
    // Markers are tiny independent SVG nodes so SVG/PNG/yydraw exports match.
    let x = if direction > 0. { p.x } else { p.x - 28. };
    let code = u64::from(direction < 0.) | (u64::from(mandatory) * 2) | (u64::from(many) * 4);
    doc.add_node(
        Shape::Icon,
        Point::new(x, p.y - 10.),
        Some(2_000_000 + code),
    );
    let n = doc.nodes.last_mut().unwrap();
    n.rect.w = 28.;
    n.rect.h = 44.;
    n.locked = true;
    n.label.clear();
}
fn marker_svg(left: bool, mandatory: bool, many: bool) -> String {
    let transform = if !left {
        ""
    } else {
        "translate(28 0) scale(-1 1)"
    };
    let mut body = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='28' height='20'><g transform='{transform}' fill='white' stroke='#526c81' stroke-width='1.4'>"
    );
    if many {
        body.push_str("<path d='M0 2L12 10L0 18M0 10H28' fill='none'/>");
    } else {
        body.push_str("<path d='M7 3V17M0 10H28'/>");
    }
    if mandatory {
        body.push_str("<path d='M20 3V17'/>");
    } else {
        body.push_str("<circle cx='20' cy='10' r='4'/>");
    }
    body.push_str("</g></svg>");
    body
}
pub(super) fn move_entity(m: &mut Model, index: usize, position: Point, doc: &mut Document) {
    let Some(entity) = m.entities.get_mut(index) else {
        return;
    };
    entity.position = position;
    if let Some(n) = doc
        .nodes
        .iter_mut()
        .find(|n| n.id == index as u64 + 1_000_000)
    {
        n.rect.x = position.x;
        n.rect.y = position.y;
    }
    let card_count = m.entities.iter().filter(|e| e.visible).count();
    let indices = m
        .entities
        .iter()
        .enumerate()
        .filter(|(_, e)| e.visible)
        .map(|(i, e)| (&e.id, i))
        .collect::<std::collections::HashMap<_, _>>();
    let sizes = doc
        .nodes
        .iter()
        .filter(|n| n.id >= 1_000_000 && n.id < 2_000_000)
        .map(|n| ((n.id - 1_000_000) as usize, n.rect.w))
        .collect::<std::collections::HashMap<_, _>>();
    let visible_relations = m.relations.iter().enumerate().filter_map(|(i, r)| {
        let ci = *indices.get(&r.child)?;
        let pi = *indices.get(&r.parent)?;
        Some((i, r, ci, pi))
    });
    for (edge_index, ((original, r, ci, pi), edge)) in
        visible_relations.zip(doc.connectors.iter_mut()).enumerate()
    {
        if ci != index && pi != index {
            continue;
        }
        let c = &m.entities[ci];
        let p = &m.entities[pi];
        let cw = sizes[&ci];
        let pw = sizes[&pi];
        let same_column = c.position.x < p.position.x + pw && p.position.x < c.position.x + cw;
        let right = same_column || c.position.x < p.position.x;
        let start = Point::new(
            c.position.x + if right { cw } else { 0. },
            c.position.y + column_y(m, c, &r.columns),
        );
        let end = Point::new(
            p.position.x + if right && !same_column { 0. } else { pw },
            p.position.y + column_y(m, p, &r.referenced_columns),
        );
        edge.start = Endpoint::Free(start);
        edge.end = Endpoint::Free(end);
        let bend = if same_column {
            (c.position.x + cw).max(p.position.x + pw) + 40. + (original % 5) as f64 * 8.
        } else {
            (start.x + end.x) / 2. + (original % 5) as f64 * 8.
        };
        edge.waypoints = vec![Point::new(bend, start.y), Point::new(bend, end.y)];
        for (offset, point, dir) in [
            (0, start, if right { 1. } else { -1. }),
            (1, end, if right && !same_column { -1. } else { 1. }),
        ] {
            let marker = &mut doc.nodes[card_count + edge_index * 2 + offset];
            marker.rect.x = point.x - if dir > 0. { 0. } else { 28. };
            marker.rect.y = point.y - 10.;
            let code = marker.asset.unwrap() - 2_000_000;
            marker.asset = Some(2_000_000 + ((code & !1) | u64::from(dir < 0.)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_and_manual_sizes_reflow_cards_and_relationships_without_losing_columns() {
        let mut m: Model =
            serde_json::from_str(include_str!("../../../tests/fixtures/sqlite-er.yyer")).unwrap();
        m.names = Names::Both;
        m.level = Level::Types;
        let original = m.table_size(0).unwrap();
        m.entities[0].logical_name =
            "非常に長い日本語のテーブル論理名を欠けずに表示するための自動調整".into();
        let expanded = m.table_size(0).unwrap();
        assert!(expanded.width > original.width);
        let all_columns = m.entities[0].fields.len();
        m.entities[0].size = Some(TableSize {
            width: 1000.,
            height: 320.,
        });
        m.entities[1].size = Some(TableSize {
            width: 250.,
            height: 80.,
        });
        m.arrange();
        assert!(m.entities[1].position.x >= m.entities[0].position.x + 1100.);
        let mut doc = m.drawing();
        assert_eq!(
            doc.nodes.iter().find(|n| n.id == 1_000_000).unwrap().rect.w,
            1000.
        );
        m.move_entity(0, Point::new(1800., 150.), &mut doc);
        let rebuilt = m.drawing();
        assert_eq!(doc.nodes, rebuilt.nodes);
        assert_eq!(doc.connectors, rebuilt.connectors);
        doc.validate().unwrap();
        assert_eq!(m.entities[0].fields.len(), all_columns);
        m.entities[0].size = None;
        assert_eq!(m.table_size(0).unwrap(), expanded);
        m.level = Level::Entity;
        assert!(m.table_size(0).unwrap().height < expanded.height);
    }
}
