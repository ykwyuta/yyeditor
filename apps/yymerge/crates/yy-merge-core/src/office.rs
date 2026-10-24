//! Read-only OOXML review. Never invokes Office, follows external links or runs macros.
use crate::{Cancellation, MAX_BYTES, Result};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    path::Path,
};

const MAX_EXPANDED: usize = 64 * 1024 * 1024;
const MAX_XML: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Excel,
    PowerPoint,
}
impl Kind {
    pub fn detect(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "xlsx" | "xlsm" => Some(Self::Excel),
            "pptx" | "pptm" => Some(Self::PowerPoint),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Excel => "Excel",
            Self::PowerPoint => "PowerPoint",
        }
    }
}
struct Package {
    parts: BTreeMap<String, Vec<u8>>,
}
fn parse(text: &str) -> Result<Document<'_>> {
    let doc = Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Office XML: {e}"))?;
    if doc
        .descendants()
        .any(|n| n.ancestors().take(130).count() > 128)
    {
        return Err("Office XML nesting exceeds 128".into());
    }
    Ok(doc)
}
fn tag<'a, 'input>(node: Node<'a, 'input>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}
fn json(text: &str) -> String {
    serde_json::to_string(text).expect("string serialization")
}
struct Output {
    text: String,
}
impl Output {
    fn line(&mut self, text: &str, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        if text.len() + 1 > (MAX_BYTES as usize).saturating_sub(self.text.len()) {
            return Err("Office comparison incomplete: 32 MiB extracted output limit".into());
        }
        self.text.push_str(text);
        self.text.push('\n');
        Ok(())
    }
}
impl Package {
    fn read(bytes: &[u8], cancel: &Cancellation) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| format!("Office ZIP (encrypted/legacy files unsupported): {e}"))?;
        // zip 2.x deduplicates central entries by name. Validate the original names.
        let mut at = usize::try_from(zip.central_directory_start())
            .map_err(|_| "Office ZIP offset overflow")?;
        let mut names = BTreeSet::new();
        while bytes.get(at..at.saturating_add(4)) == Some(b"PK\x01\x02") {
            cancel.check()?;
            let header = bytes
                .get(at..at.saturating_add(46))
                .ok_or("truncated Office ZIP directory")?;
            let word = |i| usize::from(u16::from_le_bytes([header[i], header[i + 1]]));
            let end = at
                .checked_add(46 + word(28) + word(30) + word(32))
                .ok_or("Office ZIP directory overflow")?;
            if end > bytes.len() {
                return Err("truncated Office ZIP directory".into());
            }
            let name = std::str::from_utf8(&bytes[at + 46..at + 46 + word(28)])
                .map_err(|_| "Office part names must be UTF-8")?;
            if !names.insert(name) {
                return Err("Office ZIP has duplicate part names".into());
            }
            if names.len() > 4096 {
                return Err("Office comparison incomplete: 4096 ZIP entries limit".into());
            }
            at = end;
        }
        if names.len() != zip.len() {
            return Err("Office ZIP directory entry count mismatch".into());
        }
        if zip.len() > 4096 {
            return Err("Office comparison incomplete: 4096 ZIP entries limit".into());
        }
        let mut parts = BTreeMap::new();
        let mut total = 0;
        for i in 0..zip.len() {
            cancel.check()?;
            let mut entry = zip.by_index(i).map_err(|e| format!("Office ZIP: {e}"))?;
            let name = entry.name().to_owned();
            if entry.encrypted()
                || name.contains(['\0', '\\', ':'])
                || name.starts_with('/')
                || name.split('/').any(|s| matches!(s, "." | ".."))
            {
                return Err("Office ZIP has encrypted or unsafe entries".into());
            }
            if entry.is_dir() {
                continue;
            }
            if parts.contains_key(&name) {
                return Err("Office ZIP has duplicate part names".into());
            }
            let limit = if name.ends_with(".xml") || name.ends_with(".rels") {
                MAX_XML
            } else {
                MAX_BYTES as usize
            };
            if entry.size() > limit as u64 || entry.size() > (MAX_EXPANDED - total) as u64 {
                return Err("Office comparison incomplete: expanded ZIP size limit".into());
            }
            let mut data = Vec::new();
            let mut buffer = [0; 65536];
            loop {
                cancel.check()?;
                let n = entry
                    .read(&mut buffer)
                    .map_err(|e| format!("Office ZIP CRC/data: {e}"))?;
                if n == 0 {
                    break;
                }
                if n > limit.saturating_sub(data.len()) || n > MAX_EXPANDED - total {
                    return Err("Office comparison incomplete: expanded ZIP size limit".into());
                }
                data.extend_from_slice(&buffer[..n]);
                total += n;
            }
            parts.insert(name, data);
        }
        Ok(Self { parts })
    }
    fn xml(&self, name: &str) -> Result<String> {
        let bytes = self
            .parts
            .get(name)
            .ok_or_else(|| format!("Office part missing: {name}"))?;
        if bytes.len() > MAX_XML {
            return Err("Office XML exceeds 8 MiB".into());
        }
        let (encoding, skip) = if bytes.starts_with(&[0xff, 0xfe]) {
            (yy_encoding::Encoding::Utf16Le, 2)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            (yy_encoding::Encoding::Utf16Be, 2)
        } else {
            (
                yy_encoding::Encoding::Utf8,
                usize::from(bytes.starts_with(&[0xef, 0xbb, 0xbf])) * 3,
            )
        };
        let (decoded, _) = yy_encoding::decode_all(encoding, &bytes[skip..], false);
        if yy_encoding::encode_all(encoding, &decoded, yy_encoding::EscapeMode::Reject)
            .map_err(|_| "Office XML has invalid encoding")?
            != bytes[skip..]
        {
            return Err("Office XML has invalid encoding".into());
        }
        if decoded.len() > MAX_XML {
            return Err("Office decoded XML exceeds 8 MiB".into());
        }
        String::from_utf8(decoded).map_err(|e| e.to_string())
    }
    fn relationships(&self, part: &str) -> Result<Vec<Relation>> {
        let name = if part.is_empty() {
            "_rels/.rels".into()
        } else {
            let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
            format!(
                "{}{}/{}.rels",
                if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                },
                "_rels",
                file
            )
        };
        if !self.parts.contains_key(&name) {
            return Ok(Vec::new());
        }
        let xml = self.xml(&name)?;
        let doc = parse(&xml)?;
        let mut out = Vec::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| tag(*n, "Relationship"))
        {
            let id = n.attribute("Id").ok_or("Office relationship Id missing")?;
            if out.iter().any(|r: &Relation| r.id == id) {
                return Err("duplicate Office relationship Id".into());
            }
            out.push(Relation {
                id: id.into(),
                kind: n
                    .attribute("Type")
                    .ok_or("relationship Type missing")?
                    .into(),
                target: n
                    .attribute("Target")
                    .ok_or("relationship Target missing")?
                    .into(),
                external: n.attribute("TargetMode") == Some("External"),
            });
        }
        Ok(out)
    }
    fn resolve(&self, part: &str, rel: &Relation) -> Result<String> {
        if rel.external {
            return Err("required Office part is an external link".into());
        }
        if rel.target.contains(['\\', '\0', ':', '?', '#', '%']) {
            return Err("unsupported/unsafe Office relationship target".into());
        }
        let mut path: Vec<&str> = if rel.target.starts_with('/') {
            Vec::new()
        } else {
            part.split('/').collect()
        };
        if !rel.target.starts_with('/') {
            path.pop();
        }
        for component in rel.target.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    path.pop().ok_or("Office relationship escapes package")?;
                }
                _ => path.push(component),
            }
        }
        let name = path.join("/");
        if !self.parts.contains_key(&name) {
            return Err(format!("Office relationship part missing: {name}"));
        }
        Ok(name)
    }
    fn main_part(&self) -> Result<String> {
        let types = self.xml("[Content_Types].xml")?;
        if !tag(parse(&types)?.root_element(), "Types") {
            return Err("Office content types missing".into());
        }
        let rels = self.relationships("")?;
        let main: Vec<_> = rels
            .iter()
            .filter(|r| r.kind.ends_with("/officeDocument"))
            .collect();
        if main.len() != 1 {
            return Err("Office document relationship missing/ambiguous".into());
        }
        self.resolve("", main[0])
    }
    fn spreadsheet(&self, main: &str, out: &mut Output, cancel: &Cancellation) -> Result<()> {
        let xml = self.xml(main)?;
        let doc = parse(&xml)?;
        if !tag(doc.root_element(), "workbook") {
            return Err("Excel workbook missing".into());
        }
        let rels = self.relationships(main)?;
        let mut shared = Vec::new();
        if let Some(rel) = rels.iter().find(|r| r.kind.ends_with("/sharedStrings")) {
            let xml = self.xml(&self.resolve(main, rel)?)?;
            let doc = parse(&xml)?;
            for item in doc.root_element().children().filter(|n| tag(*n, "si")) {
                cancel.check()?;
                shared.push(rich_text(item));
            }
        }
        for (i, sheet) in doc.descendants().filter(|n| tag(*n, "sheet")).enumerate() {
            let name = sheet.attribute("name").ok_or("Excel sheet name missing")?;
            let rel = find_id(&rels, relationship_id(sheet)?)?;
            let path = self.resolve(main, rel)?;
            out.line(
                &format!(
                    "SHEET {} {} state={}",
                    i + 1,
                    json(name),
                    sheet.attribute("state").unwrap_or("visible")
                ),
                cancel,
            )?;
            if !rel.kind.ends_with("/worksheet") {
                out.line(
                    &format!("SHEET TYPE {} (see package details)", json(&rel.kind)),
                    cancel,
                )?;
                continue;
            }
            let xml = self.xml(&path)?;
            let doc = parse(&xml)?;
            if !tag(doc.root_element(), "worksheet") {
                return Err("Excel worksheet missing".into());
            }
            let mut row_index = 0u32;
            for row in doc.descendants().filter(|n| tag(*n, "row")) {
                row_index = match row.attribute("r") {
                    Some(r) => r.parse().map_err(|_| "invalid Excel row")?,
                    None => row_index + 1,
                };
                if row_index == 0 || row_index > 1048576 {
                    return Err("Excel row outside supported bounds".into());
                }
                let mut col = 0u32;
                for cell in row.children().filter(|n| tag(*n, "c")) {
                    col += 1;
                    let address = if let Some(address) = cell.attribute("r") {
                        col = column(address)?;
                        address.to_owned()
                    } else {
                        format!("{}{row_index}", column_name(col)?)
                    };
                    let kind = cell.attribute("t").unwrap_or("n");
                    let raw = cell
                        .children()
                        .find(|n| tag(*n, "v"))
                        .and_then(|n| n.text());
                    let value = match kind {
                        "s" => {
                            let index: usize = raw
                                .ok_or("Excel shared string index missing")?
                                .parse()
                                .map_err(|_| "invalid shared string index")?;
                            shared
                                .get(index)
                                .ok_or("Excel shared string index out of bounds")?
                                .clone()
                        }
                        "inlineStr" => rich_text(cell),
                        _ => raw.unwrap_or("").to_owned(),
                    };
                    let formula = cell
                        .children()
                        .find(|n| tag(*n, "f"))
                        .map(|n| n.text().unwrap_or(""));
                    out.line(
                        &format!(
                            "CELL {}!{} type={kind} value={} formula={} style={}",
                            json(name),
                            address,
                            json(&value),
                            formula.map(json).unwrap_or_else(|| "null".into()),
                            cell.attribute("s").unwrap_or("0")
                        ),
                        cancel,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn presentation(&self, main: &str, out: &mut Output, cancel: &Cancellation) -> Result<()> {
        let xml = self.xml(main)?;
        let doc = parse(&xml)?;
        if !tag(doc.root_element(), "presentation") {
            return Err("PowerPoint presentation missing".into());
        }
        let rels = self.relationships(main)?;
        for (index, slide) in doc.descendants().filter(|n| tag(*n, "sldId")).enumerate() {
            let rel = find_id(&rels, relationship_id(slide)?)?;
            if !rel.kind.ends_with("/slide") {
                return Err("PowerPoint slide relationship required".into());
            }
            let path = self.resolve(main, rel)?;
            out.line(&format!("SLIDE {} part={}", index + 1, json(&path)), cancel)?;
            self.slide_text(&path, out, cancel)?;
            for rel in self
                .relationships(&path)?
                .iter()
                .filter(|r| r.kind.ends_with("/notesSlide"))
            {
                let note = self.resolve(&path, rel)?;
                out.line("NOTES", cancel)?;
                self.slide_text(&note, out, cancel)?;
            }
        }
        Ok(())
    }
    fn slide_text(&self, path: &str, out: &mut Output, cancel: &Cancellation) -> Result<()> {
        let xml = self.xml(path)?;
        let doc = parse(&xml)?;
        for shape in doc.descendants().filter(|n| {
            n.is_element() && matches!(n.tag_name().name(), "sp" | "pic" | "graphicFrame" | "cxnSp")
        }) {
            let props = shape.descendants().find(|n| tag(*n, "cNvPr"));
            out.line(
                &format!(
                    "SHAPE {} name={}",
                    props.and_then(|n| n.attribute("id")).unwrap_or("?"),
                    json(props.and_then(|n| n.attribute("name")).unwrap_or(""))
                ),
                cancel,
            )?;
            for paragraph in shape.descendants().filter(|n| tag(*n, "p")) {
                let mut text = String::new();
                for n in paragraph.descendants().filter(Node::is_element) {
                    match n.tag_name().name() {
                        "t" => text.push_str(n.text().unwrap_or("")),
                        "br" => text.push('\n'),
                        "tab" => text.push('\t'),
                        _ => {}
                    }
                }
                out.line(&format!("TEXT {}", json(&text)), cancel)?;
            }
        }
        Ok(())
    }
    fn details(&self, out: &mut Output, cancel: &Cancellation) -> Result<()> {
        out.line(
            "PACKAGE DETAILS (XML structure / binary fingerprints)",
            cancel,
        )?;
        for (name, bytes) in &self.parts {
            out.line(&format!("PART {}", json(name)), cancel)?;
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let text = self.xml(name)?;
                let doc = parse(&text)?;
                write_xml(doc.root(), out, cancel, 0)?;
            } else {
                let mut hash = blake3::Hasher::new();
                for chunk in bytes.chunks(65536) {
                    cancel.check()?;
                    hash.update(chunk);
                }
                out.line(
                    &format!(
                        "BINARY bytes={} BLAKE3={}",
                        bytes.len(),
                        hash.finalize().to_hex()
                    ),
                    cancel,
                )?;
            }
        }
        Ok(())
    }
}
struct Relation {
    id: String,
    kind: String,
    target: String,
    external: bool,
}
fn find_id<'a>(rels: &'a [Relation], id: &str) -> Result<&'a Relation> {
    rels.iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("Office relationship missing: {id}"))
}
fn relationship_id<'a>(n: Node<'a, '_>) -> Result<&'a str> {
    n.attributes()
        .find(|a| {
            a.name() == "id"
                && a.namespace()
                    .is_some_and(|ns| ns.ends_with("/relationships"))
        })
        .map(|a| a.value())
        .ok_or("Office relationship id missing".into())
}
fn rich_text(n: Node<'_, '_>) -> String {
    n.descendants()
        .filter(|n| tag(*n, "t") && !n.ancestors().any(|a| tag(a, "rPh")))
        .filter_map(|n| n.text())
        .collect()
}
fn column(address: &str) -> Result<u32> {
    let letters = address.bytes().take_while(u8::is_ascii_uppercase).count();
    if letters == 0 || letters > 3 {
        return Err("invalid Excel cell address".into());
    }
    let col = address[..letters]
        .bytes()
        .fold(0, |n, b| n * 26 + u32::from(b - b'A' + 1));
    let row: u32 = address[letters..]
        .parse()
        .map_err(|_| "invalid Excel cell row")?;
    if col > 16384 || row == 0 || row > 1048576 {
        return Err("Excel cell outside supported bounds".into());
    }
    Ok(col)
}
fn column_name(mut col: u32) -> Result<String> {
    if col == 0 || col > 16384 {
        return Err("Excel column outside supported bounds".into());
    }
    let mut out = Vec::new();
    while col > 0 {
        col -= 1;
        out.push((b'A' + (col % 26) as u8) as char);
        col /= 26;
    }
    Ok(out.into_iter().rev().collect())
}
pub fn extract(bytes: &[u8], kind: Kind, cancel: &Cancellation) -> Result<String> {
    cancel.check()?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Office input exceeds 32 MiB".into());
    }
    let package = Package::read(bytes, cancel)?;
    let main = package.main_part()?;
    let mut out = Output {
        text: String::new(),
    };
    out.line(
        &format!(
            "{} / OOXML extracted comparison v1 (read-only)",
            kind.label()
        ),
        cancel,
    )?;
    match kind {
        Kind::Excel => package.spreadsheet(&main, &mut out, cancel)?,
        Kind::PowerPoint => package.presentation(&main, &mut out, cancel)?,
    }
    package.details(&mut out, cancel)?;
    Ok(out.text)
}

/// External rendering is opt-in and accepts inert OOXML packages only.
pub(crate) fn validate_render(bytes: &[u8], kind: Kind, cancel: &Cancellation) -> Result<()> {
    extract(bytes, kind, cancel)?;
    let package = Package::read(bytes, cancel)?;
    for (name, bytes) in &package.parts {
        cancel.check()?;
        let lower = name.to_ascii_lowercase();
        if lower.contains("vbaproject")
            || lower.contains("/activex/")
            || lower.contains("/embeddings/")
            || lower.contains("/externallinks/")
        {
            return Err("Office image rendering refuses macros, embedded objects and external links; use content comparison".into());
        }
        if lower.ends_with(".rels") {
            let text =
                std::str::from_utf8(bytes).map_err(|_| "render relationship XML requires UTF-8")?;
            if parse(text)?.descendants().any(|n| {
                n.attribute("TargetMode")
                    .is_some_and(|m| m.eq_ignore_ascii_case("external"))
            }) {
                return Err(
                    "Office image rendering refuses external relationships; use content comparison"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

/// Human-readable changes to the extracted representation, never a source patch.
pub fn changes(
    left: &str,
    right: &str,
    diff: &crate::Diff,
    cancel: &Cancellation,
) -> Result<String> {
    let a = crate::lines(left);
    let b = crate::lines(right);
    let mut out = Output {
        text: String::new(),
    };
    for row in &diff.rows {
        if row.kind == crate::Kind::Equal {
            continue;
        }
        let (marker, line) = match (row.left, row.right) {
            (Some(i), _) => ("- ", a.get(i)),
            (_, Some(i)) => ("+ ", b.get(i)),
            _ => return Err("invalid Office diff row".into()),
        };
        let line = line
            .ok_or("invalid Office diff index")?
            .trim_end_matches('\n');
        out.line(&format!("{marker}{line}"), cancel)?;
    }
    Ok(out.text)
}

fn write_xml(n: Node<'_, '_>, out: &mut Output, cancel: &Cancellation, depth: usize) -> Result<()> {
    cancel.check()?;
    if depth > 128 {
        return Err("Office XML nesting exceeds 128".into());
    }
    if n.is_element() {
        let mut attrs: Vec<_> = n
            .attributes()
            .map(|a| (a.namespace().unwrap_or(""), a.name(), a.value()))
            .collect();
        attrs.sort_unstable();
        let attrs = serde_json::to_string(&attrs).map_err(|e| e.to_string())?;
        let namespaces: Vec<_> = n
            .namespaces()
            .filter(|ns| {
                n.parent().and_then(|p| p.lookup_namespace_uri(ns.name())) != Some(ns.uri())
            })
            .map(|ns| (ns.name(), ns.uri()))
            .collect();
        let namespaces = serde_json::to_string(&namespaces).map_err(|e| e.to_string())?;
        out.line(
            &format!(
                "<{{{}}}{} attrs={attrs} xmlns={namespaces}>",
                n.tag_name().namespace().unwrap_or(""),
                n.tag_name().name()
            ),
            cancel,
        )?;
    } else if n.is_text() {
        out.line(&format!("  {}", json(n.text().unwrap_or(""))), cancel)?;
    } else if n.is_comment() {
        out.line(&format!("COMMENT {}", json(n.text().unwrap_or(""))), cancel)?;
    } else if let Some(pi) = n.pi() {
        out.line(
            &format!("PI {} {}", json(pi.target), json(pi.value.unwrap_or(""))),
            cancel,
        )?;
    }
    for child in n.children() {
        write_xml(child, out, cancel, depth + 1)?;
    }
    if n.is_element() {
        out.line(
            &format!(
                "</{{{}}}{}>",
                n.tag_name().namespace().unwrap_or(""),
                n.tag_name().name()
            ),
            cancel,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files::Snapshot, rules::Rules};
    use std::io::Write;
    const EXCEL: &[u8] = include_bytes!("../../../../../demo/yymerge/office/left.xlsx");
    const PPT: &[u8] = include_bytes!("../../../../../demo/yymerge/office/left.pptx");
    fn rewrite(bytes: &[u8], edit: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) -> Vec<u8> {
        let mut parts = Package::read(bytes, &Cancellation::new()).unwrap().parts;
        edit(&mut parts);
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.set_comment("repacked (not original bytes)");
        for (name, data) in parts.into_iter().rev() {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(&data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn image_renderer_preflight_rejects_active_and_external_parts() {
        let c = Cancellation::new();
        validate_render(EXCEL, Kind::Excel, &c).unwrap();
        validate_render(PPT, Kind::PowerPoint, &c).unwrap();
        for name in [
            "xl/vbaProject.bin",
            "xl/activeX/control.bin",
            "xl/embeddings/object.bin",
            "xl/externalLinks/link.bin",
        ] {
            let bytes = rewrite(EXCEL, |parts| {
                parts.insert(name.into(), vec![1, 2, 3]);
            });
            assert!(validate_render(&bytes, Kind::Excel, &c).is_err());
        }
        let bytes = rewrite(EXCEL, |parts| {
            parts.insert("xl/_rels/external.rels".into(), br#"<Relationships><Relationship TargetMode="External" Target="https://example.invalid/data"/></Relationships>"#.to_vec());
        });
        assert!(
            validate_render(&bytes, Kind::Excel, &c)
                .unwrap_err()
                .contains("external")
        );
    }
    #[test]
    fn independent_excel_and_powerpoint_fixtures_include_cells_order_notes_and_images() {
        let c = Cancellation::new();
        let text = extract(EXCEL, Kind::Excel, &c).unwrap();
        assert!(text.contains("CELL \"売上\"!B2 type=n value=\"3\""));
        assert!(text.contains("formula=\"B2*C2\""));
        assert!(text.contains("非表示の設定\" state=hidden"));
        assert!(text.contains("mergeCell"));
        let right = extract(
            include_bytes!("../../../../../demo/yymerge/office/right.xlsx"),
            Kind::Excel,
            &c,
        )
        .unwrap();
        assert!(right.contains("value=\"4\""));
        assert!(right.contains("追加シート"));
        assert_ne!(text, right);
        let text = extract(PPT, Kind::PowerPoint, &c).unwrap();
        assert!(text.contains("SLIDE 1 part=\"ppt/slides/slide1.xml\""));
        assert!(text.contains("TEXT \"概要\""));
        assert!(text.contains("TEXT \"発表者ノート\""));
        assert!(text.contains("BINARY bytes="));
        let right = extract(
            include_bytes!("../../../../../demo/yymerge/office/right.pptx"),
            Kind::PowerPoint,
            &c,
        )
        .unwrap();
        assert!(right.contains("SLIDE 1 part=\"ppt/slides/slide2.xml\""));
        assert!(right.contains("変更後の文章"));
        assert_ne!(text, right);
    }
    #[test]
    fn repacking_is_equivalent_but_not_raw_identity_and_sessions_reports_stay_read_only() {
        let c = Cancellation::new();
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("a.xlsx");
        let right = dir.path().join("b.xlsx");
        let repacked = rewrite(EXCEL, |_| {});
        std::fs::write(&left, EXCEL).unwrap();
        std::fs::write(&right, &repacked).unwrap();
        let a = Snapshot::open_comparison(&left, None, &c).unwrap();
        let b = Snapshot::open_comparison(&right, None, &c).unwrap();
        assert_eq!(a.text, b.text);
        assert!(!a.original_identical(&b));
        assert!(a.encode("unsafe output").is_err());
        assert!(Snapshot::open(&left, None, &c).is_err());
        let compared = crate::rules::compare(&a.text, &b.text, Rules::default(), &c).unwrap();
        let html = crate::report::html(&a, &b, Rules::default(), &compared, &c).unwrap();
        assert!(html.contains("EquivalentOfficeContent"));
        assert!(html.contains("Office抽出データ"));
        let session = crate::comparison_session::Session::capture(
            [&a, &b],
            Rules::default(),
            Default::default(),
            &c,
        )
        .unwrap();
        let path = dir.path().join("work.json");
        std::fs::write(&path, session.json(&c).unwrap()).unwrap();
        let (loaded, _) = crate::comparison_session::Session::open(&path, &c).unwrap();
        assert_eq!(loaded.inputs[0].office, Some(Kind::Excel));
        assert_eq!(loaded.inputs[0].text, a.text);
        assert!(crate::files::save(&left, b"overwrite", &[&a, &b], true, &c).is_err());
        assert_eq!(std::fs::read(&left).unwrap(), EXCEL);
        std::fs::write(&right, EXCEL).unwrap();
        assert!(b.verify(&c).is_err());
        assert!(crate::comparison_session::Session::open(&path, &c).is_err());
    }
    #[test]
    fn style_image_and_unusual_parts_are_not_silently_ignored() {
        let c = Cancellation::new();
        for (source, kind, target) in [
            (EXCEL, Kind::Excel, "xl/styles.xml"),
            (PPT, Kind::PowerPoint, "ppt/media/image1.png"),
        ] {
            let changed = rewrite(source, |p| {
                if target.ends_with(".xml") {
                    let text = String::from_utf8(p[target].clone())
                        .unwrap()
                        .replace("004080", "FF0000");
                    p.insert(target.into(), text.into_bytes());
                } else {
                    p.get_mut(target).unwrap().push(42);
                }
            });
            assert_ne!(
                extract(source, kind, &c).unwrap(),
                extract(&changed, kind, &c).unwrap()
            );
        }
        let macro_added = rewrite(EXCEL, |p| {
            p.insert("xl/vbaProject.bin".into(), b"never executed".to_vec());
        });
        assert!(
            extract(&macro_added, Kind::Excel, &c)
                .unwrap()
                .contains("xl/vbaProject.bin")
        );
    }
    #[test]
    fn shared_strings_rich_text_utf16_and_external_links_are_read_without_execution() {
        let bytes = rewrite(EXCEL, |p| {
            let path = "xl/worksheets/sheet1.xml";
            let text = String::from_utf8(p[path].clone()).unwrap().replace(
                "<c r=\"A2\" t=\"inlineStr\"><is><t>&#12426;&#12435;&#12372;</t></is></c>",
                "<c r=\"A2\" t=\"s\"><v>0</v></c>",
            );
            assert!(text.contains("t=\"s\""));
            p.insert(path.into(), text.into_bytes());
            let rel = "xl/_rels/workbook.xml.rels";
            let text = String::from_utf8(p[rel].clone()).unwrap().replace("</Relationships>", "<Relationship Id=\"shared\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/><Relationship Id=\"remote\" Type=\"link\" TargetMode=\"External\" Target=\"http://example.invalid/not-fetched\"/></Relationships>");
            p.insert(rel.into(), text.into_bytes());
            let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><si><r><t>猫</t></r><r><t> &amp; dog</t></r><rPh><t>phonetic ignored</t></rPh></si></sst>";
            let mut utf16 = vec![0xff, 0xfe];
            for unit in xml.encode_utf16() {
                utf16.extend(unit.to_le_bytes());
            }
            p.insert("xl/sharedStrings.xml".into(), utf16);
        });
        let text = extract(&bytes, Kind::Excel, &Cancellation::new()).unwrap();
        assert!(text.contains("CELL \"売上\"!A2 type=s value=\"猫 & dog\""));
        assert!(text.contains("example.invalid/not-fetched"));
    }
    #[test]
    fn malformed_unsupported_and_resource_exhausting_inputs_fail_closed() {
        let c = Cancellation::new();
        assert!(extract(b"not zip", Kind::Excel, &c).is_err());
        assert!(extract(EXCEL, Kind::PowerPoint, &c).is_err());
        for mutate in [0, 1, 2, 3, 4, 5] {
            let changed = rewrite(EXCEL, |p| match mutate {
                0 => {
                    p.remove("xl/workbook.xml");
                }
                1 => {
                    p.insert(
                        "xl/styles.xml".into(),
                        b"<!DOCTYPE a [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><a>&x;</a>"
                            .to_vec(),
                    );
                }
                2 => {
                    p.insert("../escape".into(), b"no".to_vec());
                }
                3 => {
                    p.insert("xl/styles.xml".into(), b"<bad>".to_vec());
                }
                4 => {
                    p.insert(
                        "xl/styles.xml".into(),
                        format!("{}{}", "<n>".repeat(130), "</n>".repeat(130)).into_bytes(),
                    );
                }
                _ => {
                    p.insert("xl/styles.xml".into(), vec![b' '; MAX_XML + 1]);
                }
            });
            assert!(extract(&changed, Kind::Excel, &c).is_err(), "case {mutate}");
        }
        let packed = rewrite(EXCEL, |p| {
            p.insert("custom/a.bin".into(), b"CORRUPTION_MARKER_ONLY".to_vec());
            p.insert("custom/b.bin".into(), b"second".to_vec());
        });
        let mut duplicate = packed.clone();
        let old = b"custom/b.bin";
        for pos in 0..duplicate.len().saturating_sub(old.len()) {
            if &duplicate[pos..pos + old.len()] == old {
                duplicate[pos..pos + old.len()].copy_from_slice(b"custom/a.bin");
            }
        }
        assert!(
            extract(&duplicate, Kind::Excel, &c)
                .unwrap_err()
                .contains("duplicate")
        );
        let mut corrupt = packed;
        let at = corrupt
            .windows(b"CORRUPTION_MARKER_ONLY".len())
            .position(|s| s == b"CORRUPTION_MARKER_ONLY")
            .unwrap();
        corrupt[at] ^= 1;
        assert!(
            extract(&corrupt, Kind::Excel, &c)
                .unwrap_err()
                .contains("CRC")
        );
        let mut out = Output {
            text: "x".repeat(MAX_BYTES as usize),
        };
        assert!(out.line("x", &c).is_err());
        c.cancel();
        assert!(extract(EXCEL, Kind::Excel, &c).is_err());
    }
}
