//! Office の文書（`docx`・`xlsx`・`pptx`）の中の文字列（18 章 8.3）。
//!
//! Office の文書は ZIP の中の XML なので、ZIP の目次を読んで必要な XML を展開し（`miniz_oxide`）、
//! 文字列の要素（`<w:t>`・`<a:t>`・`<t>`）の中身を取り出す。段落・行の終わりで改行する。

use std::io::{self, Read, Seek, SeekFrom};

/// 展開する XML の大きさの上限（壊れた・悪意のある ZIP で膨らまないように）。
const MAX_XML: usize = 256 << 20;

/// 対象の拡張子か。
pub fn is_office(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".docx") || n.ends_with(".xlsx") || n.ends_with(".xlsm") || n.ends_with(".pptx")
}

struct ZipEntry {
    name: String,
    method: u16,
    comp_size: u64,
    offset: u64,
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// ZIP の目次（中央ディレクトリ）を読む。
fn entries<R: Read + Seek>(f: &mut R) -> io::Result<Vec<ZipEntry>> {
    let len = f.seek(SeekFrom::End(0))?;
    // 終わりのレコード（22 バイト + コメント最大 64 KiB）
    let tail_len = len.min(22 + 65_535);
    f.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| bad("ZIP の終わりが見つかりません"))?;
    let count = u16le(&tail, eocd + 10) as usize;
    let cd_size = u32le(&tail, eocd + 12) as u64;
    let cd_off = u32le(&tail, eocd + 16) as u64;
    if cd_off == u32::MAX as u64 || cd_off + cd_size > len {
        return Err(bad("ZIP64 には対応していません"));
    }
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd)?;
    let mut out = Vec::with_capacity(count);
    let mut p = 0;
    while p + 46 <= cd.len() && cd[p..p + 4] == [0x50, 0x4b, 0x01, 0x02] {
        let method = u16le(&cd, p + 10);
        let comp_size = u32le(&cd, p + 20) as u64;
        let name_len = u16le(&cd, p + 28) as usize;
        let extra_len = u16le(&cd, p + 30) as usize;
        let comment_len = u16le(&cd, p + 32) as usize;
        let offset = u32le(&cd, p + 42) as u64;
        let name =
            String::from_utf8_lossy(cd.get(p + 46..p + 46 + name_len).unwrap_or(&[])).into_owned();
        out.push(ZipEntry {
            name,
            method,
            comp_size,
            offset,
        });
        p += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// ZIP の 1 項目の中身。
fn read_entry<R: Read + Seek>(f: &mut R, e: &ZipEntry) -> io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(e.offset))?;
    let mut h = [0u8; 30];
    f.read_exact(&mut h)?;
    if h[..4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err(bad("ZIP の項目の頭が違います"));
    }
    let skip = u16le(&h, 26) as i64 + u16le(&h, 28) as i64;
    f.seek(SeekFrom::Current(skip))?;
    if e.comp_size as usize > MAX_XML {
        return Err(bad("大きすぎます"));
    }
    let mut data = vec![0u8; e.comp_size as usize];
    f.read_exact(&mut data)?;
    match e.method {
        0 => Ok(data),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(&data, MAX_XML)
            .map_err(|_| bad("展開できません")),
        _ => Err(bad("対応していない圧縮です")),
    }
}

/// 文書の種類ごとの、文字列のある XML か。
fn wanted(name: &str) -> bool {
    name == "word/document.xml"
        || (name.starts_with("word/")
            && (name.contains("header") || name.contains("footer") || name.contains("footnotes"))
            && name.ends_with(".xml"))
        || name == "xl/sharedStrings.xml"
        || (name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
        || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        || (name.starts_with("ppt/notesSlides/") && name.ends_with(".xml"))
}

/// 文字の参照（`&amp;`・`&#x3042;` など）を戻す。
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let ent = &after[..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// XML から文字列を取り出す（文字列の要素の中身。段落・行・セルの終わりで区切る）。
pub fn xml_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let mut in_text = false;
    while let Some(lt) = rest.find('<') {
        if in_text {
            out.push_str(&unescape(&rest[..lt]));
        }
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if local == "t" {
            in_text = !closing && !tag.ends_with('/');
            continue;
        }
        let newline =
            (closing && matches!(local, "p" | "si" | "row")) || (!closing && local == "br");
        let tab = (closing && matches!(local, "c" | "tc")) || (!closing && local == "tab");
        if newline {
            out.push('\n');
        } else if tab {
            out.push('\t');
        }
    }
    out
}

/// 並べる順（`slide10.xml` は `slide2.xml` の後）。
fn order_key(name: &str) -> (String, u64) {
    let stem = name.strip_suffix(".xml").unwrap_or(name);
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (prefix, num) = stem.split_at(stem.len() - digits);
    (prefix.to_owned(), num.parse().unwrap_or(0))
}

/// Office の文書の中の文字列。
pub fn extract_text<R: Read + Seek>(f: &mut R) -> io::Result<String> {
    let mut list = entries(f)?;
    list.retain(|e| wanted(&e.name));
    // スライド・シートは番号の順に
    list.sort_by_cached_key(|e| order_key(&e.name));
    let mut out = String::new();
    for e in &list {
        let xml = read_entry(f, e)?;
        out.push_str(&xml_text(&String::from_utf8_lossy(&xml)));
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 試験用の小さな ZIP（無圧縮と deflate）。
    pub(crate) fn zip(files: &[(&str, &str, bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, body, deflate) in files {
            let data = if *deflate {
                miniz_oxide::deflate::compress_to_vec(body.as_bytes(), 6)
            } else {
                body.as_bytes().to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let off = out.len() as u32;
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // 時刻・CRC（読まない）
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);
            cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0]);
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&[0; 8]);
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn extracts_word_excel_and_powerpoint_text() {
        let docx = zip(&[
            ("[Content_Types].xml", "<Types/>", false),
            (
                "word/document.xml",
                r#"<w:document><w:body><w:p><w:r><w:t>見積の</w:t></w:r><w:r><w:t xml:space="preserve">税込 &amp; 合計</w:t></w:r></w:p><w:p><w:r><w:t>2 行目</w:t><w:tab/><w:t>&#x3042;</w:t></w:r></w:p></w:body></w:document>"#,
                true,
            ),
        ]);
        let t = extract_text(&mut io::Cursor::new(docx)).unwrap();
        assert!(t.contains("見積の税込 & 合計\n2 行目\tあ"), "{t:?}");
        let xlsx = zip(&[
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>売上</t></si><si><r><t>東</t></r><r><t>京</t></r></si></sst>"#,
                true,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>直接</t></is></c></row></sheetData></worksheet>"#,
                false,
            ),
        ]);
        let t = extract_text(&mut io::Cursor::new(xlsx)).unwrap();
        assert!(t.contains("売上\n東京\n"), "{t:?}");
        assert!(t.contains("直接"), "{t:?}");
        let pptx = zip(&[
            (
                "ppt/slides/slide10.xml",
                "<p:sld><a:p><a:r><a:t>十枚目</a:t></a:r></a:p></p:sld>",
                true,
            ),
            (
                "ppt/slides/slide2.xml",
                "<p:sld><a:p><a:r><a:t>二枚目</a:t></a:r></a:p></p:sld>",
                true,
            ),
        ]);
        let t = extract_text(&mut io::Cursor::new(pptx)).unwrap();
        assert!(
            t.find("二枚目").unwrap() < t.find("十枚目").unwrap(),
            "{t:?}"
        );
        assert!(extract_text(&mut io::Cursor::new(b"not a zip".to_vec())).is_err());
        assert!(is_office("a.XLSX") && !is_office("a.xls"));
        assert_eq!(unescape("a &lt;b&gt; &bogus; &#65;"), "a <b> &bogus; A");
    }
}
