//! Interchange through a bounded string-keyed tree. Structural envelopes are explicit.
use crate::{Output, Params, Result, checkpoint, value};
use serde_json::{Map, Value};
use std::collections::HashSet;

type Row = [String; 3];
const MARKER: &str = "yytoolbox-tree-v1";

pub(crate) fn convert(
    input: &str,
    params: &Params,
    cancelled: &impl Fn() -> bool,
) -> Result<Output> {
    let from = value(params, "from", "YAML");
    let to = value(params, "to", "XML");
    let tree = read(input.trim_start_matches('\u{feff}'), from)?;
    validate(&tree, 0, &mut 0)?;
    checkpoint(cancelled)?;
    let structural = match value(params, "structure", "構造保持") {
        "構造保持" => true,
        "標準形式" => false,
        _ => return Err("構造の扱いが不正です".into()),
    };
    let text = write(&tree, to, structural)?;
    let filename = match to {
        "XML" => "converted.xml",
        "YAML" => "converted.yaml",
        "CSV" => "converted.csv",
        "INI" => "converted.ini",
        _ => return Err("出力形式が不正です".into()),
    };
    Ok(Output::text(
        text,
        filename,
        format!(
            "{from} → {to} / {}。元のコメント・整形は保持しません。CSV・INIの通常入力値は文字列です",
            if structural {
                "型と入れ子を保持（XML/CSV/INIは専用表現）"
            } else {
                "標準形式（表現できない構造はエラー）"
            }
        ),
    ))
}
fn validate(value: &Value, depth: usize, count: &mut usize) -> Result<()> {
    *count += 1;
    if depth > 64 || *count > 100000 {
        return Err("変換は深さ64・100000要素までです".into());
    }
    match value {
        Value::Object(map) => {
            for value in map.values() {
                validate(value, depth + 1, count)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate(value, depth + 1, count)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn yaml_value(value: serde_yaml_ng::Value, depth: usize) -> Result<Value> {
    use serde_yaml_ng::Value as Y;
    if depth > 64 {
        return Err("YAMLの深さは64までです".into());
    }
    Ok(match value {
        Y::Null => Value::Null,
        Y::Bool(v) => Value::Bool(v),
        Y::String(v) => Value::String(v),
        Y::Number(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
        Y::Sequence(values) => Value::Array(
            values
                .into_iter()
                .map(|v| yaml_value(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Y::Mapping(values) => {
            let mut map = Map::new();
            for (key, v) in values {
                let Y::String(key) = key else {
                    return Err("YAMLのキーは文字列にしてください".into());
                };
                map.insert(key, yaml_value(v, depth + 1)?);
            }
            Value::Object(map)
        }
        Y::Tagged(_) => return Err("相互変換ではYAMLの独自タグを扱えません".into()),
    })
}
fn read(input: &str, format: &str) -> Result<Value> {
    match format {
        "YAML" => {
            let tree: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(input).map_err(|e| format!("YAML: {e}"))?;
            // YAML nonfinite numbers cannot be represented faithfully by the common tree.
            fn finite(v: &serde_yaml_ng::Value) -> bool {
                match v {
                    serde_yaml_ng::Value::Number(n) => n.as_f64().is_none_or(f64::is_finite),
                    serde_yaml_ng::Value::Sequence(v) => v.iter().all(finite),
                    serde_yaml_ng::Value::Mapping(v) => v.values().all(finite),
                    _ => true,
                }
            }
            if !finite(&tree) {
                return Err("NaN・無限大は相互変換できません".into());
            }
            yaml_value(tree, 0)
        }
        "CSV" => {
            let mut reader = csv::Reader::from_reader(input.as_bytes());
            let headers = reader.headers().map_err(|e| format!("CSV: {e}"))?.clone();
            if headers.iter().eq([MARKER, "type", "value"]) {
                let rows = reader
                    .records()
                    .map(|r| {
                        let r = r.map_err(|e| e.to_string())?;
                        Ok([r[0].into(), r[1].into(), r[2].into()])
                    })
                    .collect::<Result<Vec<Row>>>()?;
                return unflatten(rows);
            }
            if headers.is_empty()
                || headers.iter().any(str::is_empty)
                || headers.iter().collect::<HashSet<_>>().len() != headers.len()
            {
                return Err("CSVには空欄・重複のない見出し行が必要です".into());
            }
            let mut rows = Vec::new();
            for row in reader.records() {
                let row = row.map_err(|e| format!("CSV: {e}"))?;
                rows.push(Value::Object(
                    headers
                        .iter()
                        .zip(row.iter())
                        .map(|(h, v)| (h.into(), Value::String(v.into())))
                        .collect(),
                ));
                if rows.len() > 100000 {
                    return Err("CSVは100000行までです".into());
                }
            }
            Ok(Value::Array(rows))
        }
        "INI" => {
            let tree = read_ini(input)?;
            if let Some(envelope) = tree
                .as_object()
                .and_then(|v| v.get(MARKER))
                .and_then(Value::as_object)
            {
                if tree.as_object().is_none_or(|v| v.len() != 1) {
                    return Err("構造保持INIに別のセクションが混在しています".into());
                }
                let mut rows = Vec::new();
                for i in 0..envelope.len() {
                    let text = envelope
                        .get(&format!("n{i:06}"))
                        .and_then(Value::as_str)
                        .ok_or("構造保持INIの連番が不正です")?;
                    rows.push(
                        serde_json::from_str::<Row>(text)
                            .map_err(|e| format!("構造保持INI: {e}"))?,
                    );
                }
                return unflatten(rows);
            }
            Ok(tree)
        }
        "XML" => {
            let doc = roxmltree::Document::parse_with_options(
                input,
                roxmltree::ParsingOptions {
                    allow_dtd: false,
                    nodes_limit: 100000,
                },
            )
            .map_err(|e| format!("XML: {e}"))?;
            let root = doc.root_element();
            if root.tag_name().name() == MARKER && root.tag_name().namespace().is_none() {
                let mut rows = Vec::new();
                for node in root.children().filter(|n| n.is_element()) {
                    if node.tag_name().name() != "node" {
                        return Err("構造保持XMLの要素が不正です".into());
                    }
                    rows.push([
                        node.attribute("path")
                            .ok_or("XMLにpathがありません")?
                            .into(),
                        node.attribute("type")
                            .ok_or("XMLにtypeがありません")?
                            .into(),
                        node.text().unwrap_or("").into(),
                    ]);
                }
                return unflatten(rows);
            }
            Ok(Value::Object(
                [(
                    qualified(root, root.tag_name().namespace(), root.tag_name().name()),
                    xml_value(root, 0)?,
                )]
                .into_iter()
                .collect(),
            ))
        }
        _ => Err("入力形式はXML/YAML/CSV/INIです".into()),
    }
}
fn qualified(node: roxmltree::Node<'_, '_>, uri: Option<&str>, name: &str) -> String {
    uri.and_then(|u| node.lookup_prefix(u))
        .map_or_else(|| name.to_owned(), |p| format!("{p}:{name}"))
}
fn xml_value(node: roxmltree::Node<'_, '_>, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err("XMLの深さは64までです".into());
    }
    let children: Vec<_> = node.children().filter(|n| n.is_element()).collect();
    let text: String = node
        .children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect();
    if !children.is_empty() && !text.trim().is_empty() {
        return Err("文字列と子要素が交互に現れる混合内容のXMLは相互変換できません".into());
    }
    let mut out = Map::new();
    for ns in node.namespaces() {
        out.insert(
            ns.name()
                .map_or_else(|| "@xmlns".into(), |p| format!("@xmlns:{p}")),
            Value::String(ns.uri().into()),
        );
    }
    for attr in node.attributes() {
        out.insert(
            format!("@{}", qualified(node, attr.namespace(), attr.name())),
            Value::String(attr.value().into()),
        );
    }
    if children.is_empty() && out.is_empty() {
        return Ok(Value::String(text));
    }
    if children.is_empty() && !text.is_empty() {
        out.insert("#text".into(), Value::String(text));
    }
    for child in children {
        let name = qualified(child, child.tag_name().namespace(), child.tag_name().name());
        let value = xml_value(child, depth + 1)?;
        match out.entry(name) {
            serde_json::map::Entry::Vacant(e) => {
                e.insert(value);
            }
            serde_json::map::Entry::Occupied(mut e) => {
                if let Value::Array(values) = e.get_mut() {
                    values.push(value);
                } else {
                    let old = e.insert(Value::Null);
                    e.insert(Value::Array(vec![old, value]));
                }
            }
        }
    }
    Ok(Value::Object(out))
}
fn read_ini(input: &str) -> Result<Value> {
    let mut root = Map::new();
    let mut section: Option<String> = None;
    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let key = line[1..line.len() - 1].trim();
            if key.is_empty() || root.contains_key(key) {
                return Err(format!("INI {}行目: 空欄または重複のセクション", index + 1));
            }
            root.insert(key.into(), Value::Object(Map::new()));
            section = Some(key.into());
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("INI {}行目: key=valueで指定してください", index + 1))?;
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("INI {}行目: キーが空です", index + 1));
        }
        let map = match &section {
            Some(name) => root
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .ok_or("INIセクションが不正です")?,
            None => &mut root,
        };
        // Structural envelope values are JSON rows, not ordinary quoted INI strings.
        let raw = value.trim();
        let value = if section.as_deref() != Some(MARKER) && raw.starts_with('"') {
            serde_json::from_str::<String>(raw)
                .map_err(|e| format!("INI {}行目: {e}", index + 1))?
        } else {
            raw.into()
        };
        if map.insert(key.into(), Value::String(value)).is_some() {
            return Err(format!("INI {}行目: キーが重複しています", index + 1));
        }
    }
    Ok(Value::Object(root))
}
fn scalar(value: &Value) -> Result<String> {
    match value { Value::String(v) => Ok(v.clone()), Value::Bool(v) => Ok(v.to_string()), Value::Number(v) => Ok(v.to_string()), _ => Err("標準CSV/INI/XMLの値には文字列・数値・真偽値を指定してください。nullや入れ子には構造保持を選択してください".into()) }
}
fn write(tree: &Value, format: &str, structural: bool) -> Result<String> {
    if format == "YAML" {
        return serde_yaml_ng::to_string(tree).map_err(|e| e.to_string());
    }
    if structural {
        let mut rows = Vec::new();
        flatten(tree, "", &mut rows, &mut 0)?;
        return match format {
            "CSV" => {
                let mut writer = csv::WriterBuilder::new()
                    .terminator(csv::Terminator::CRLF)
                    .from_writer(Vec::new());
                writer
                    .write_record([MARKER, "type", "value"])
                    .map_err(|e| e.to_string())?;
                for row in rows {
                    writer.write_record(row).map_err(|e| e.to_string())?;
                }
                String::from_utf8(writer.into_inner().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            }
            "INI" => {
                let mut out = format!("[{MARKER}]\n");
                for (i, row) in rows.iter().enumerate() {
                    out.push_str(&format!(
                        "n{i:06}={}\n",
                        serde_json::to_string(row).map_err(|e| e.to_string())?
                    ));
                }
                Ok(out)
            }
            "XML" => {
                let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{MARKER}>\n");
                for [path, kind, value] in rows {
                    out.push_str(&format!(
                        "  <node path=\"{}\" type=\"{}\">{}</node>\n",
                        xml_escape(&path),
                        kind,
                        xml_escape(&value)
                    ));
                }
                out.push_str(&format!("</{MARKER}>\n"));
                roxmltree::Document::parse(&out)
                    .map_err(|e| format!("XMLで表せない文字があります: {e}"))?;
                Ok(out)
            }
            _ => Err("出力形式が不正です".into()),
        };
    }
    match format {
        "CSV" => {
            let records = tree
                .as_array()
                .ok_or("標準CSVには文字列のオブジェクト配列が必要です。構造保持も利用できます")?;
            let first = records
                .first()
                .and_then(Value::as_object)
                .ok_or("標準CSVには1件以上のオブジェクトが必要です")?;
            if first.is_empty() {
                return Err("CSVの列がありません".into());
            }
            let headers: Vec<_> = first.keys().cloned().collect();
            let mut writer = csv::WriterBuilder::new()
                .terminator(csv::Terminator::CRLF)
                .from_writer(Vec::new());
            writer.write_record(&headers).map_err(|e| e.to_string())?;
            for record in records {
                let map = record
                    .as_object()
                    .ok_or("CSV行はオブジェクトにしてください")?;
                if map.len() != headers.len() || !headers.iter().all(|h| map.contains_key(h)) {
                    return Err("標準CSVは全行のキーを揃えてください".into());
                }
                writer
                    .write_record(
                        headers
                            .iter()
                            .map(|h| scalar(&map[h]))
                            .collect::<Result<Vec<_>>>()?,
                    )
                    .map_err(|e| e.to_string())?;
            }
            String::from_utf8(writer.into_inner().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "INI" => {
            let root = tree
                .as_object()
                .ok_or("標準INIはオブジェクトにしてください")?;
            let mut out = String::new();
            fn pair(out: &mut String, key: &str, value: &Value) -> Result<()> {
                if key.trim() != key
                    || key.is_empty()
                    || key.contains(['=', '\r', '\n'])
                    || key.starts_with([';', '#', '['])
                {
                    return Err("INIのキーとして表せない名前です".into());
                }
                out.push_str(key);
                out.push('=');
                out.push_str(&serde_json::to_string(&scalar(value)?).map_err(|e| e.to_string())?);
                out.push('\n');
                Ok(())
            }
            for (key, v) in root {
                if !v.is_object() {
                    pair(&mut out, key, v)?;
                }
            }
            for (section, v) in root {
                if let Some(map) = v.as_object() {
                    if section.is_empty()
                        || section.trim() != section
                        || section.contains(['[', ']', '\r', '\n'])
                        || section == MARKER
                    {
                        return Err("INIのセクション名が不正です".into());
                    }
                    out.push_str(&format!("\n[{section}]\n"));
                    for (key, v) in map {
                        pair(&mut out, key, v)?;
                    }
                }
            }
            Ok(out)
        }
        "XML" => {
            let root = tree
                .as_object()
                .filter(|m| m.len() == 1)
                .ok_or("標準XMLにはルート要素1個のオブジェクトが必要です")?;
            let (name, value) = root.iter().next().unwrap();
            let mut out = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n".to_owned();
            xml_element(name, value, &mut out, 0)?;
            roxmltree::Document::parse(&out)
                .map_err(|e| format!("XMLで表せない名前・文字があります: {e}"))?;
            Ok(out)
        }
        _ => Err("出力形式が不正です".into()),
    }
}
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
        .replace('\t', "&#9;")
}
fn xml_element(name: &str, value: &Value, out: &mut String, depth: usize) -> Result<()> {
    if depth > 64
        || name.is_empty()
        || name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '/' | '=' | '\'' | '"'))
    {
        return Err("XML要素名・深さが不正です".into());
    }
    out.push_str(&"  ".repeat(depth));
    out.push('<');
    out.push_str(name);
    if let Some(map) = value.as_object() {
        for (key, v) in map {
            if let Some(attr) = key.strip_prefix('@') {
                if attr.is_empty()
                    || !attr.chars().enumerate().all(|(i, c)| {
                        c.is_alphabetic()
                            || matches!(c, '_' | ':')
                            || (i > 0 && (c.is_numeric() || matches!(c, '-' | '.')))
                    })
                {
                    return Err("XML属性名が不正です".into());
                }
                out.push_str(&format!(" {attr}=\"{}\"", xml_escape(&scalar(v)?)));
            }
        }
        let children: Vec<_> = map
            .iter()
            .filter(|(k, _)| !k.starts_with('@') && *k != "#text")
            .collect();
        if children.is_empty() {
            out.push('>');
            if let Some(text) = map.get("#text") {
                out.push_str(&xml_escape(&scalar(text)?));
            }
        } else {
            if map.contains_key("#text") {
                return Err("標準XMLの混合内容はサポートしていません".into());
            }
            out.push_str(">\n");
            for (key, value) in children {
                if let Some(values) = value.as_array() {
                    for value in values {
                        xml_element(key, value, out, depth + 1)?;
                    }
                } else {
                    xml_element(key, value, out, depth + 1)?;
                }
            }
            out.push_str(&"  ".repeat(depth));
        }
    } else {
        out.push('>');
        out.push_str(&xml_escape(&scalar(value)?));
    }
    out.push_str(&format!("</{name}>\n"));
    Ok(())
}
fn flatten(tree: &Value, path: &str, rows: &mut Vec<Row>, budget: &mut usize) -> Result<()> {
    let (kind, text) = match tree {
        Value::Null => ("null", String::new()),
        Value::Bool(v) => ("bool", v.to_string()),
        Value::Number(v) => ("number", v.to_string()),
        Value::String(v) => ("string", v.clone()),
        Value::Array(_) => ("array", String::new()),
        Value::Object(_) => ("object", String::new()),
    };
    *budget = budget.saturating_add(path.len() + kind.len() + text.len() + 64);
    if *budget > crate::MAX_OUTPUT {
        return Err("構造保持の中間データが16 MiBを超えました".into());
    }
    rows.push([path.into(), kind.into(), text]);
    match tree {
        Value::Object(map) => {
            for (key, value) in map {
                flatten(
                    value,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    rows,
                    budget,
                )?;
            }
        }
        Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                flatten(value, &format!("{path}/{i}"), rows, budget)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn unflatten(rows: Vec<Row>) -> Result<Value> {
    if rows.is_empty() || rows.len() > 100000 {
        return Err("構造保持データの要素数が不正です".into());
    }
    let mut root = None;
    let mut paths = HashSet::new();
    for [path, kind, text] in rows {
        if !paths.insert(path.clone()) || path.split('/').count() > 65 {
            return Err("構造保持データのパスが重複、または深すぎます".into());
        }
        let value = match kind.as_str() {
            "object" if text.is_empty() => Value::Object(Map::new()),
            "array" if text.is_empty() => Value::Array(Vec::new()),
            "null" if text.is_empty() => Value::Null,
            "string" => Value::String(text),
            "bool" => Value::Bool(text.parse().map_err(|_| "boolが不正です")?),
            "number" => Value::Number(text.parse().map_err(|_| "数値が不正です")?),
            _ => return Err("構造保持データの型が不正です".into()),
        };
        if path.is_empty() {
            if root.is_some() {
                return Err("ルートが重複しています".into());
            }
            root = Some(value);
            continue;
        }
        let (parent, key) = path
            .rsplit_once('/')
            .ok_or("パスはJSON Pointerで指定してください")?;
        let key = key.replace("~1", "/").replace("~0", "~");
        let parent = root
            .as_mut()
            .and_then(|v| v.pointer_mut(parent))
            .ok_or("構造保持データの親が見つかりません")?;
        match parent {
            Value::Object(map) => {
                map.insert(key, value);
            }
            Value::Array(values) if key == values.len().to_string() => values.push(value),
            _ => return Err("構造保持データの親・配列添字が不正です".into()),
        }
    }
    root.ok_or_else(|| "構造保持データのルートがありません".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_pair_preserves_nested_types_and_special_strings() {
        let tree = serde_json::json!({"日本語/鍵~": [1, true, null, "001", "a\r\nb\t&<\""], "empty": {}, "array": [], "": "x"});
        for from in ["XML", "YAML", "CSV", "INI"] {
            for to in ["XML", "YAML", "CSV", "INI"] {
                let first = write(&tree, from, true).unwrap();
                let input = read(&first, from).unwrap();
                let second = write(&input, to, true).unwrap();
                assert_eq!(read(&second, to).unwrap(), tree, "{from} → {to}");
            }
        }
    }
    #[test]
    fn native_inputs_and_outputs_are_explicit_and_reject_loss() {
        let csv = read("name,value\n日本語,001\n", "CSV").unwrap();
        assert_eq!(
            read(&write(&csv, "CSV", false).unwrap(), "CSV").unwrap(),
            csv
        );
        let ini = read("key=001\n[s]\nvalue=a=b;comment\n", "INI").unwrap();
        assert_eq!(
            read(&write(&ini, "INI", false).unwrap(), "INI").unwrap(),
            ini
        );
        let xml = read("<root id=\"001\"><v>a&amp;b</v><v>日本語</v></root>", "XML").unwrap();
        assert_eq!(
            read(&write(&xml, "XML", false).unwrap(), "XML").unwrap(),
            xml
        );
        assert!(read("<r>before<x/>after</r>", "XML").is_err());
        assert!(
            read(
                "<!DOCTYPE r [<!ENTITY e SYSTEM 'file:///secret'>]><r>&e;</r>",
                "XML"
            )
            .is_err()
        );
        assert!(read("[s]\nx=1\nx=2", "INI").is_err());
        assert!(read("a,a\n1,2", "CSV").is_err());
        assert!(write(&serde_json::json!({"count": [1]}), "INI", false).is_err());
        assert!(read("value: .nan", "YAML").is_err());
    }
}
