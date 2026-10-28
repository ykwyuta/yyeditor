use crate::{Output, Params, QrImage, Result, Tool, integer, value};

pub(crate) fn qr(input: &str, params: &Params) -> Result<Output> {
    if input.is_empty() {
        return Err("QRコードにする文字列を入力してください".into());
    }
    let ecc = match value(params, "ecc", "M") {
        "L" => qrcode::EcLevel::L,
        "M" => qrcode::EcLevel::M,
        "Q" => qrcode::EcLevel::Q,
        "H" => qrcode::EcLevel::H,
        _ => return Err("誤り訂正はL/M/Q/Hです".into()),
    };
    if input.len() > 2953 {
        return Err(
            "QRコードの容量を超えています（UTF-8で最大2953バイト、誤り訂正により減少）".into(),
        );
    }
    let code = qrcode::QrCode::with_error_correction_level(input.as_bytes(), ecc)
        .map_err(|e| format!("QRコード: {e}"))?;
    let scale = integer(params, "scale", 4, 16)?;
    let width = code.width();
    let dark: Vec<_> = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    let side = (width as u32 + 8) * scale;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(side, side)
        .build();
    let format = value(params, "format", "SVG");
    let (bytes, filename) = match format {
        "SVG" => (svg.as_bytes().to_vec(), "qrcode.svg"),
        "PNG" => {
            let mut pixels = vec![255; side as usize * side as usize];
            for y in 0..side as usize {
                for x in 0..side as usize {
                    let (mx, my) = (x / scale as usize, y / scale as usize);
                    if mx >= 4
                        && my >= 4
                        && mx < width + 4
                        && my < width + 4
                        && dark[(my - 4) * width + mx - 4]
                    {
                        pixels[y * side as usize + x] = 0;
                    }
                }
            }
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, side, side);
                encoder.set_color(png::ColorType::Grayscale);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
                writer
                    .write_image_data(&pixels)
                    .map_err(|e| e.to_string())?;
            }
            (bytes, "qrcode.png")
        }
        _ => return Err("保存形式はSVG/PNGです".into()),
    };
    Ok(Output {
        text: svg,
        bytes,
        filename,
        qr: Some(QrImage { width, dark }),
        notice: format!(
            "QR {width}×{width}マス・余白4マス・訂正{ecc:?}。保存は{format}、コピーはSVG。密なコードは画面を広げるか保存して確認してください"
        ),
    })
}

pub(crate) fn url(input: &str, params: &Params) -> Result<Output> {
    let mode = value(params, "url_mode", "URL部品");
    if !["URL部品", "フォーム", "URI全体"].contains(&mode) {
        return Err("URL変換の方式が不正です".into());
    }
    let text = match value(params, "operation", "エンコード") {
        "エンコード" => {
            let mut out = String::new();
            for b in input.bytes() {
                let safe = b.is_ascii_alphanumeric()
                    || b"-._~".contains(&b)
                    || (mode == "フォーム" && b == b'*')
                    || (mode == "URI全体" && b":/?#[]@!$&'()*+,;=".contains(&b));
                if mode == "フォーム" && b == b' ' {
                    out.push('+');
                } else if safe && !(mode == "フォーム" && b == b'~') {
                    out.push(b as char);
                } else {
                    use std::fmt::Write;
                    let _ = write!(out, "%{b:02X}");
                }
            }
            out
        }
        "デコード" => {
            let bytes = input.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    let hi = bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16));
                    let lo = bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16));
                    let (Some(hi), Some(lo)) = (hi, lo) else {
                        return Err(format!("{}バイト目の%エスケープが不正です", i + 1));
                    };
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(if mode == "フォーム" && bytes[i] == b'+' {
                        b' '
                    } else {
                        bytes[i]
                    });
                    i += 1;
                }
            }
            String::from_utf8(out).map_err(|_| "デコード結果がUTF-8ではありません")?
        }
        _ => return Err("操作はエンコード／デコードです".into()),
    };
    Ok(Output::text(
        text,
        "url-result.txt",
        "UTF-8でURL変換しました。URLへの接続は行いません",
    ))
}

fn js(input: &str, indent: u8, width: u32) -> Result<String> {
    use biome_diagnostics::{Diagnostic, PrintDescription};
    use biome_formatter::{IndentStyle, IndentWidth, LineWidth};
    use biome_js_formatter::{context::JsFormatOptions, format_node};
    use biome_js_parser::{JsParserOptions, parse};
    use biome_js_syntax::JsFileSource;
    let source = JsFileSource::js_module();
    let parsed = parse(input, source, JsParserOptions::default());
    if parsed.has_errors() {
        return Err(parsed
            .diagnostics()
            .first()
            .map(|error| {
                let position = error
                    .location()
                    .span
                    .map(|span| format!("（UTF-8の{}バイト目）", u32::from(span.start()) + 1))
                    .unwrap_or_default();
                format!("JavaScript{position}: {}", PrintDescription(error))
            })
            .unwrap_or_else(|| "JavaScript: 構文エラーがあります".into()));
    }
    let options = JsFormatOptions::new(source)
        .with_indent_style(IndentStyle::Space)
        .with_indent_width(IndentWidth::from(indent))
        .with_line_width(LineWidth::try_from(width as u16).map_err(|e| e.to_string())?);
    let formatted =
        format_node(options, &parsed.syntax()).map_err(|e| format!("JavaScript: {e}"))?;
    let printed = formatted.print().map_err(|e| format!("JavaScript: {e}"))?;
    Ok(printed.as_code().to_owned())
}

pub(crate) fn format(input: &str, params: &Params, tool: Tool) -> Result<Output> {
    if input.len() > 256 * 1024 {
        return Err("HTML/CSS/JavaScript整形の入力は256 KiBまでです".into());
    }
    // Bound recursion before invoking third-party parsers, including hostile nesting.
    guard_nesting(input, tool)?;
    let indent = integer(params, "indent", 1, 8)? as u8;
    let width = integer(params, "width", 40, 240)?;
    let (text, filename) = match tool {
        Tool::JavaScript => (js(input, indent, width)?, "formatted.js"),
        Tool::Css => {
            let mut options = malva::config::FormatOptions::default();
            options.layout.indent_width = indent as usize;
            options.layout.print_width = width as usize;
            (
                malva::format_text(input, malva::Syntax::Css, &options)
                    .map_err(|e| format!("CSS: {e}"))?,
                "formatted.css",
            )
        }
        Tool::Html => {
            let mut options = markup_fmt::config::FormatOptions::default();
            options.layout.indent_width = indent as usize;
            options.layout.print_width = width as usize;
            // Keep embedded scripts/styles byte-for-byte; use their dedicated tools to format them.
            (
                markup_fmt::format_text(input, markup_fmt::Language::Html, &options, |code, _| {
                    Ok(code.into())
                })
                .map_err(|e| format!("HTML: {e}"))?,
                "formatted.html",
            )
        }
        _ => return Err("整形する言語が不正です".into()),
    };
    Ok(Output::text(
        text,
        filename,
        "構文を解析して整形しました。コードの実行・通信は行いません",
    ))
}

fn guard_nesting(input: &str, tool: Tool) -> Result<()> {
    let mut depth = 0usize;
    if tool == Tool::Html {
        let lower = input.to_ascii_lowercase();
        let mut at = 0;
        let mut count = 0;
        while let Some(offset) = lower[at..].find('<') {
            at += offset;
            if lower[at..].starts_with("<!--") {
                at = lower[at + 4..]
                    .find("-->")
                    .map_or(lower.len(), |n| at + 4 + n + 3);
                continue;
            }
            let Some(end) = tag_end(&lower, at + 1) else {
                break;
            };
            let inner = lower[at + 1..end].trim();
            if inner.starts_with('/') {
                depth = depth.saturating_sub(1);
            } else if inner.starts_with(|c: char| c.is_ascii_alphabetic()) {
                let tag = inner
                    .split([' ', '\t', '\r', '\n', '/'])
                    .next()
                    .unwrap_or("");
                if !inner.ends_with('/')
                    && ![
                        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
                        "param", "source", "track", "wbr",
                    ]
                    .contains(&tag)
                {
                    depth += 1;
                }
                if depth > 128 {
                    return Err("HTMLタグの深さは128までです".into());
                }
                count += 1;
                if count > 4096 {
                    return Err("HTMLのタグ数は4096までです".into());
                }
                if ["script", "style", "textarea", "title"].contains(&tag) {
                    at = lower[end + 1..]
                        .find(&format!("</{tag}"))
                        .map_or(lower.len(), |n| end + 1 + n);
                    continue;
                }
            }
            at = end + 1;
        }
    } else {
        let b = input.as_bytes();
        let mut at = 0;
        while at < b.len() {
            if matches!(b[at], b'\'' | b'"' | b'`') {
                let quote = b[at];
                at += 1;
                while at < b.len() {
                    if b[at] == b'\\' {
                        at = (at + 2).min(b.len());
                    } else if b[at] == quote {
                        at += 1;
                        break;
                    } else {
                        at += 1;
                    }
                }
                continue;
            }
            if b.get(at..at + 2) == Some(b"/*") {
                at = input[at + 2..]
                    .find("*/")
                    .map_or(b.len(), |n| at + 2 + n + 2);
                continue;
            }
            if tool == Tool::JavaScript && b.get(at..at + 2) == Some(b"//") {
                at = input[at + 2..]
                    .find('\n')
                    .map_or(b.len(), |n| at + 2 + n + 1);
                continue;
            }
            if matches!(b[at], b'{' | b'[' | b'(') {
                depth += 1;
                if depth > 128 {
                    return Err("括弧の深さは128までです".into());
                }
            }
            if matches!(b[at], b'}' | b']' | b')') {
                depth = depth.saturating_sub(1);
            }
            at += 1;
        }
    }
    Ok(())
}
fn tag_end(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, ch) in input[start..].char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(start + offset);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_unicode_form_reserved_and_invalid_bytes() {
        let mut p = Tool::Url.defaults();
        for mode in ["URL部品", "フォーム", "URI全体"] {
            p.insert("url_mode".into(), mode.into());
            p.insert("operation".into(), "エンコード".into());
            let source = "https://example.com/日本語?a=1+2 &b=%#~*😀";
            let encoded = url(source, &p).unwrap();
            p.insert("operation".into(), "デコード".into());
            assert_eq!(url(&encoded.text, &p).unwrap().text, source);
        }
        for source in ["%", "%GG", "%FF", "%E3%81"] {
            assert!(url(source, &p).is_err());
        }
        p.insert("url_mode".into(), "URL部品".into());
        assert_eq!(url("a+b", &p).unwrap().text, "a+b");
    }
    #[test]
    fn qr_png_quiet_zone_and_svg_are_real_images() {
        let mut p = Tool::QrCode.defaults();
        let svg = qr("日本語 QR & test", &p).unwrap();
        assert!(svg.text.contains("<svg"));
        assert_eq!(svg.bytes, svg.text.as_bytes());
        p.insert("format".into(), "PNG".into());
        let png = qr("日本語 QR & test", &p).unwrap();
        assert!(png.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let reader = png::Decoder::new(png.bytes.as_slice()).read_info().unwrap();
        let width = (png.qr.as_ref().unwrap().width as u32 + 8) * 8;
        assert_eq!(reader.info().width, width);
        assert!(qr("", &p).is_err());
        assert!(qr(&"x".repeat(4000), &p).is_err());
    }
    #[test]
    fn javascript_preserves_asi_regex_templates_comments_and_import_order() {
        for (input, fragments) in [
            ("function f(){return\n{x:1}}", vec!["return;", "x: 1"]),
            (
                "const ratio=(a/b)/c;const re=/[{}]a{2}/gu;",
                vec!["/[{}]a{2}/gu", "a / b"],
            ),
            (
                "// 保持するコメント\nconst text=`literal 日本語 ${1+2}`;/* block comment */",
                vec![
                    "// 保持するコメント",
                    "literal 日本語",
                    "/* block comment */",
                ],
            ),
            (
                "const text='a;{日本語}';const key=123_456n;const o={...source, [name]:text};",
                vec!["a;{日本語}", "123_456n", "...source", "[name]"],
            ),
        ] {
            let first = js(input, 4, 40).unwrap();
            for fragment in fragments {
                assert!(first.contains(fragment), "lost {fragment}: {first}");
            }
            assert_eq!(js(&first, 4, 40).unwrap(), first);
        }
        let imports = js("import z from 'z';import a from 'a';export {z,a};", 2, 80).unwrap();
        assert!(imports.find("import z").unwrap() < imports.find("import a").unwrap());
        for indent in [1, 2, 4, 8] {
            let output=js("function f(){return {longNameOne:1,longNameTwo:2,longNameThree:3,longNameFour:4};}",indent,40).unwrap();
            assert!(output.lines().any(|line|line.starts_with(&format!("{}return", " ".repeat(indent as usize)))),"indent={indent}: {output}");
        }
        let error = js("function (", 2, 80).unwrap_err();
        assert!(error.contains("JavaScript") && !error.contains("ParseDiagnostic"));
    }

    #[test]
    fn formatters_preserve_literals_and_are_idempotent() {
        for (tool, input, literal) in [
            (
                Tool::JavaScript,
                "// comment\nconst regex=/a{2}b/gi;const x=`a ${1+2} 日本語`;function f(){return /x/.test(x);}",
                "/a{2}b/gi",
            ),
            (
                Tool::Css,
                "a::before{content:'a;{日本語}';background:url('https://example.com/a;b.png')}@media(min-width:1px){a{color:red}}",
                "a;{日本語}",
            ),
            (
                Tool::Html,
                "<main><pre> a  b\n c </pre><p>A <strong>日本語</strong> B</p><script>const a={x:1};</script></main>",
                " a  b\n c ",
            ),
        ] {
            let p = tool.defaults();
            let first = format(input, &p, tool).unwrap();
            assert!(first.text.contains(literal), "{}", first.text);
            assert_eq!(format(&first.text, &p, tool).unwrap().text, first.text);
        }
        assert!(format("function (", &Tool::JavaScript.defaults(), Tool::JavaScript).is_err());
        assert!(format("a{color:", &Tool::Css.defaults(), Tool::Css).is_err());
        assert!(format("<div>", &Tool::Html.defaults(), Tool::Html).is_err());
        assert!(
            format(
                &format!("{}x{}", "<div>".repeat(129), "</div>".repeat(129)),
                &Tool::Html.defaults(),
                Tool::Html
            )
            .is_err()
        );
    }
}
