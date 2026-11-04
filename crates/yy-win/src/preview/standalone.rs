//! Self-contained HTML export. Parsing, resource packaging and HTTPS use Rust.
use super::*;
use html5ever::{parse_document, serialize, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use std::collections::{HashMap, HashSet};
use url::Url;

const TOTAL_LIMIT: usize = 128 << 20;

pub(crate) struct Snapshot {
    pub(super) html: String,
    pub(super) folder: Option<DocBase>,
}

pub(super) fn menu_item(
    env: &ICoreWebView2Environment9,
    frame: HWND,
) -> Result<ICoreWebView2ContextMenuItem> {
    unsafe {
        let item = env.CreateContextMenuItem(
            w!("画像を埋め込んだシングルHTMLを保存…"),
            None::<&IStream>,
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
        )?;
        let mut token = 0;
        item.add_CustomItemSelected(
            &webview2_com::CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                let _ = PostMessageW(Some(frame), WM_APP_PREVIEW_EXPORT, WPARAM(0), LPARAM(0));
                Ok(())
            })),
            &mut token,
        )?;
        Ok(item)
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) fn save(
    snapshot: &Snapshot,
    path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    let html = bundle(snapshot, cancelled)?;
    check_cancel(cancelled)?;
    // Never truncate the destination before every resource has been embedded.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(html.as_bytes())?;
    file.as_file().sync_all()?;
    check_cancel(cancelled)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn check_cancel(cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "HTML出力を中止しました",
        ))
    } else {
        Ok(())
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .unversioned_rustls_crypto_provider(Arc::new(yy_tls_crypto::provider()))
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(5)
        .build()
        .new_agent()
}

fn bundle(snapshot: &Snapshot, cancelled: &dyn Fn() -> bool) -> io::Result<String> {
    let mut resources = Resources {
        folder: snapshot.folder.clone(),
        agent: agent(),
        cache: HashMap::new(),
        active: HashSet::new(),
        total: snapshot.html.len(),
        cancelled,
    };
    let dom = parse_document(RcDom::default(), Default::default()).one(snapshot.html.as_str());
    let base = Url::parse(&format!("https://{DOC_HOST}/")).unwrap();
    resources.visit(&dom.document, &base, 0)?;
    let mut bytes = Vec::new();
    serialize(
        &mut bytes,
        &SerializableHandle::from(dom.document),
        Default::default(),
    )?;
    if bytes.len() > TOTAL_LIMIT {
        return Err(invalid("HTMLの出力上限128 MiBを超えました"));
    }
    String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))
}

struct Resources<'a> {
    folder: Option<DocBase>,
    agent: ureq::Agent,
    cache: HashMap<String, String>,
    active: HashSet<String>,
    total: usize,
    cancelled: &'a dyn Fn() -> bool,
}

impl Resources<'_> {
    fn embed(&mut self, raw: &str, base: &Url, depth: usize) -> io::Result<String> {
        check_cancel(self.cancelled)?;
        if raw.starts_with("data:") || raw.starts_with('#') {
            return Ok(raw.to_owned());
        }
        if depth > 8 {
            return Err(invalid("資源の参照が深すぎます"));
        }
        let mut url = base.join(raw).map_err(|e| invalid(format!("{raw}: {e}")))?;
        let fragment = url.fragment().map(str::to_owned);
        url.set_fragment(None);
        let key = url.as_str().to_owned();
        let data = if let Some(data) = self.cache.get(&key) {
            data.clone()
        } else {
            if self.active.len() >= 8 || !self.active.insert(key.clone()) {
                return Err(invalid(format!(
                    "資源参照が循環しているか深すぎます: {url}"
                )));
            }
            let (mut bytes, mime) = self
                .read(&url)
                .map_err(|e| invalid(format!("{raw}: {e}")))?;
            if mime.starts_with("text/css") {
                let css = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
                // Only WOFF2 is shipped. The first source already has the correct
                // format hint; replace the legacy fallbacks as well so no URL escapes.
                let css = if url.host_str() == Some(PREVIEW_HOST)
                    && url.path() == "/assets/katex.min.css"
                {
                    css.replace(".woff)", ".woff2)").replace(".ttf)", ".woff2)")
                } else {
                    css.to_owned()
                };
                bytes = self.css(&css, &url, depth + 1)?.into_bytes();
            } else if mime.starts_with("image/svg+xml") {
                let svg = std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?;
                if svg.contains("<image")
                    || svg.contains("<use")
                    || svg.contains("url(")
                    || svg.contains("<style")
                {
                    let dom = parse_document(RcDom::default(), Default::default()).one(svg);
                    let mut pending = vec![dom.document.clone()];
                    let mut root = None;
                    while let Some(node) = pending.pop() {
                        if matches!(&node.data, NodeData::Element { name, .. } if name.local.as_ref() == "svg")
                        {
                            root = Some(node);
                            break;
                        }
                        pending.extend(node.children.borrow().iter().cloned());
                    }
                    let root = root.ok_or_else(|| invalid("SVG画像が不正です"))?;
                    self.visit(&root, &url, 0)?;
                    bytes.clear();
                    serialize(
                        &mut bytes,
                        &SerializableHandle::from(root),
                        html5ever::serialize::SerializeOpts {
                            traversal_scope: html5ever::serialize::TraversalScope::IncludeNode,
                            ..Default::default()
                        },
                    )?;
                }
            } else if url.host_str() == Some(PREVIEW_HOST) && url.path() == "/assets/preview.js" {
                let d3 = self.embed("d3.min.js", &url, depth + 1)?;
                bytes = String::from_utf8(bytes)
                    .map_err(|e| invalid(e.to_string()))?
                    .replace("assets + \"d3.min.js\"", &format!("\"{d3}\""))
                    .into_bytes();
            }
            let data = format!(
                "data:{mime};base64,{}",
                data_encoding::BASE64.encode(&bytes)
            );
            self.active.remove(&key);
            self.cache.insert(key, data.clone());
            data
        };
        // Count repeated references too: each data URI is emitted into the HTML.
        self.total = self
            .total
            .checked_add(data.len())
            .ok_or_else(|| invalid("HTMLが大きすぎます"))?;
        if self.total > TOTAL_LIMIT {
            return Err(invalid("埋め込み上限128 MiBを超えました"));
        }
        Ok(match fragment {
            Some(f) => format!("{data}#{f}"),
            None => data,
        })
    }

    fn read(&self, url: &Url) -> io::Result<(Vec<u8>, String)> {
        match url.host_str() {
            Some(PREVIEW_HOST) if url.scheme() == "https" => {
                let (bytes, mime) =
                    yy_preview::asset(url.path()).ok_or_else(|| invalid("内蔵資源がありません"))?;
                Ok((bytes.into_owned(), mime.into()))
            }
            Some(DOC_HOST) if url.scheme() == "https" => {
                let path = url.path().trim_start_matches('/');
                let bytes = fetch_document_file(self.folder.clone(), path, self.cancelled)?;
                Ok((bytes, yy_preview::mime_for(url.path()).into()))
            }
            _ if matches!(url.scheme(), "http" | "https") => {
                let mut response = self
                    .agent
                    .get(url.as_str())
                    .call()
                    .map_err(|e| invalid(e.to_string()))?;
                let mime = response
                    .headers()
                    .get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or_else(|| yy_preview::mime_for(url.path()))
                    .to_owned();
                let mut bytes = Vec::new();
                response
                    .body_mut()
                    .as_reader()
                    .take(FETCH_LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > FETCH_LIMIT {
                    return Err(invalid("画像の上限32 MiBを超えました"));
                }
                check_cancel(self.cancelled)?;
                Ok((bytes, mime))
            }
            _ => Err(invalid(format!("埋め込めないURLです: {url}"))),
        }
    }

    fn css(&mut self, css: &str, base: &Url, depth: usize) -> io::Result<String> {
        // The bundled KaTeX stylesheet uses url(fonts/...). Also covers inline backgrounds.
        let lower = css.to_ascii_lowercase();
        if lower.contains("@import") {
            return Err(invalid("CSSの@importはシングルHTML出力に対応していません"));
        }
        let mut out = String::new();
        let mut start = 0;
        while let Some(i) = lower[start..].find("url(").map(|i| start + i) {
            out.push_str(&css[start..i]);
            let value_start = i + 4;
            let rest = css[value_start..].trim_start();
            let end = if rest.starts_with(['\'', '"']) {
                let quote = rest.as_bytes()[0] as char;
                let close = rest[1..]
                    .find(quote)
                    .ok_or_else(|| invalid("CSSのURLが不正です"))?
                    + 1;
                let after = value_start + (css[value_start..].len() - rest.len()) + close + 1;
                after
                    + css[after..]
                        .find(')')
                        .ok_or_else(|| invalid("CSSのURLが不正です"))?
            } else {
                value_start
                    + css[value_start..]
                        .find(')')
                        .ok_or_else(|| invalid("CSSのURLが不正です"))?
            };
            let raw = css[value_start..end].trim().trim_matches(['\'', '"']);
            out.push_str("url(\"");
            out.push_str(&self.embed(raw, base, depth)?);
            out.push_str("\")");
            start = end + 1;
        }
        out.push_str(&css[start..]);
        Ok(out)
    }

    fn visit(&mut self, node: &Handle, base: &Url, depth: usize) -> io::Result<()> {
        check_cancel(self.cancelled)?;
        if depth > 512 {
            return Err(invalid("HTMLの入れ子が深すぎます"));
        }
        if let NodeData::Element { name, attrs, .. } = &node.data {
            let tag = name.local.as_ref();
            let mut attrs = attrs.borrow_mut();
            // The preview's private virtual host must not become the saved page's base.
            if tag == "base" {
                attrs.clear();
            }
            let stylesheet = tag == "link"
                && attrs.iter().any(|a| {
                    a.name.local.as_ref() == "rel" && a.value.eq_ignore_ascii_case("stylesheet")
                });
            for attr in attrs.iter_mut() {
                let key = attr.name.local.as_ref();
                if (matches!(tag, "img" | "script" | "input") && key == "src")
                    || (matches!(tag, "image" | "use") && matches!(key, "href" | "src"))
                    || (stylesheet && key == "href")
                    || (tag == "video" && key == "poster")
                {
                    attr.value = self.embed(&attr.value, base, 0)?.into();
                } else if key == "style" {
                    attr.value = self.css(&attr.value, base, 0)?.into();
                } else if key == "srcset" {
                    attr.value = self.srcset(&attr.value, base)?.into();
                }
            }
            if tag == "style" {
                for child in node.children.borrow().iter() {
                    if let NodeData::Text { contents } = &child.data {
                        let css = self.css(&contents.borrow(), base, 0)?;
                        *contents.borrow_mut() = css.into();
                    }
                }
            }
        }
        for child in node.children.borrow().iter() {
            self.visit(child, base, depth + 1)?;
        }
        Ok(())
    }

    fn srcset(&mut self, srcset: &str, base: &Url) -> io::Result<String> {
        let mut rest = srcset;
        let mut out = Vec::new();
        while !rest
            .trim_matches(|c: char| c.is_ascii_whitespace() || c == ',')
            .is_empty()
        {
            rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
            let end = rest
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            let token = &rest[..end];
            rest = &rest[end..];
            let url = token.trim_end_matches(',');
            let descriptors = if token.ends_with(',') {
                ""
            } else {
                let end = rest.find(',').unwrap_or(rest.len());
                let value = rest[..end].trim();
                rest = &rest[end..];
                value
            };
            out.push(
                format!("{} {descriptors}", self.embed(url, base, 0)?)
                    .trim_end()
                    .to_owned(),
            );
        }
        Ok(out.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_http_image_uses_content_type_and_deduplicates_requests() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut request = [0; 4096];
                        let mut length = 0;
                        while length < request.len() && !request[..length].ends_with(b"\r\n\r\n") {
                            let got = socket.read(&mut request[length..]).unwrap();
                            assert!(got > 0, "HTTP request ended before its headers");
                            length += got;
                        }
                        assert!(request[..length].starts_with(b"GET /image?id=1 HTTP/1.1\r\n"));
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 5\r\nConnection: close\r\n\r\nIMAGE").unwrap();
                        return;
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("image server: {e}"),
                }
            }
        });
        let source = Snapshot {
            html: format!(
                "<img src='http://{address}/image?id=1'><img src='http://{address}/image?id=1'>"
            ),
            folder: None,
        };
        let result = bundle(&source, &|| false);
        server.join().unwrap();
        let html = result.unwrap();
        assert_eq!(html.matches("data:image/png;base64,SU1BR0U=").count(), 2);
        assert!(!html.contains(&address.to_string()));
    }

    #[test]
    fn standalone_offline_images_math_mermaid_and_d3_render_in_webview2() {
        use crate::preview::testing::{eval, pump_until, webview2_lock};
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        let _serial = webview2_lock();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            RegisterClassExW(&WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(preview_proc),
                hInstance: GetModuleHandleW(None).unwrap().into(),
                lpszClassName: PREVIEW_CLASS,
                ..Default::default()
            });
        }
        struct Parent(HWND);
        impl Drop for Parent {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let parent = Parent(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PREVIEW_CLASS,
                w!("standalone export test"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\"/></svg>").unwrap();
        let markdown = "# 日本語😀\n\n![画像](a.svg)\n\n$x^2$\n\n```mermaid\ngraph TD; A-->B;\n```\n\n```d3\nel.textContent = 'D3完了';\n```\n";
        let source = snapshot(
            &yy_preview::markdown_to_html(markdown, None),
            Some(DocBase::Local(dir.path().to_owned())),
        );
        let path = dir.path().join("standalone.html");
        save(&source, &path, &|| false).unwrap();
        std::fs::remove_file(dir.path().join("a.svg")).unwrap();
        let exported = std::fs::read_to_string(path).unwrap();
        let mut pane = Preview::new(parent.0, parent.0).unwrap();
        pane.set_visible(true);
        pane.set_bounds(0, 0, 620, 440);
        pane.show_html(&exported, &PageOptions::default(), None);
        assert!(
            pump_until(Duration::from_secs(45), || pane.loaded_kind()
                == Some(Kind::Html)
                || pane.is_unavailable()),
            "standalone page did not load"
        );
        if pane.is_unavailable() {
            assert!(
                std::env::var_os("YY_REQUIRE_WEBVIEW2").is_none(),
                "WebView2 required: {}",
                notice_of(pane.hwnd)
            );
            return;
        }
        let mut actual = String::new();
        assert!(
            pump_until(Duration::from_secs(30), || {
                actual = eval(&pane, "JSON.stringify({image:document.images[0]?.naturalWidth,math:!!document.querySelector('.katex'),mermaid:!!document.querySelector('.mermaid svg'),d3:document.querySelector('.yy-d3-out')?.textContent,fonts:document.fonts.status})").unwrap_or_default();
                actual.contains("\\\"image\\\":8")
                    && actual.contains("\\\"math\\\":true")
                    && actual.contains("\\\"mermaid\\\":true")
                    && actual.contains("D3完了")
                    && actual.contains("loaded")
            }),
            "offline rendering state: {actual}"
        );
        // The same native custom menu item used by ContextMenuRequested is available.
        let web = pane.web().unwrap();
        let environment = unsafe {
            web.webview
                .cast::<ICoreWebView2_2>()
                .unwrap()
                .Environment()
                .unwrap()
        };
        let item = menu_item(&environment.cast().unwrap(), parent.0).unwrap();
        assert_eq!(
            take_string(|p| unsafe { item.Label(p) }),
            "画像を埋め込んだシングルHTMLを保存…"
        );
    }

    fn snapshot(body: &str, folder: Option<DocBase>) -> Snapshot {
        Snapshot {
            html: yy_preview::markdown_page(
                body,
                &PageOptions {
                    has_folder: folder.is_some(),
                    ..Default::default()
                },
            ),
            folder,
        }
    }

    #[test]
    fn standalone_embeds_images_css_fonts_and_rendering_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\"/></svg>";
        std::fs::write(dir.path().join("画像.svg"), svg).unwrap();
        let source = snapshot(
            "<h1>日本語😀</h1><img src=\"%E7%94%BB%E5%83%8F.svg\"><img src=\"%E7%94%BB%E5%83%8F.svg\"><p class=\"math math-inline\">x^2</p>",
            Some(DocBase::Local(dir.path().to_owned())),
        );
        let html = bundle(&source, &|| false).unwrap();
        assert!(html.contains("日本語😀"));
        assert_eq!(
            html.matches(&format!(
                "data:image/svg+xml;base64,{}",
                data_encoding::BASE64.encode(svg)
            ))
            .count(),
            2
        );
        assert!(!html.contains("<base href="));
        let dom = parse_document(RcDom::default(), Default::default()).one(html);
        fn inspect(node: &Handle, scripts: &mut Vec<String>, styles: &mut Vec<String>) {
            if let NodeData::Element { name, attrs, .. } = &node.data {
                for a in attrs.borrow().iter() {
                    if name.local.as_ref() == "script" && a.name.local.as_ref() == "src" {
                        scripts.push(a.value.to_string());
                    }
                    if name.local.as_ref() == "link" && a.name.local.as_ref() == "href" {
                        styles.push(a.value.to_string());
                    }
                }
            }
            for child in node.children.borrow().iter() {
                inspect(child, scripts, styles);
            }
        }
        let (mut scripts, mut styles) = (Vec::new(), Vec::new());
        inspect(&dom.document, &mut scripts, &mut styles);
        assert_eq!(scripts.len(), 3);
        assert_eq!(styles.len(), 2);
        let decode = |uri: &str| {
            String::from_utf8(
                data_encoding::BASE64
                    .decode(uri.split_once(",").unwrap().1.as_bytes())
                    .unwrap(),
            )
            .unwrap()
        };
        let katex_css = decode(&styles[0]);
        assert!(katex_css.contains("data:font/woff2;base64,"));
        assert!(!katex_css.contains("url(fonts/"));
        let js = decode(&scripts[2]);
        assert!(!js.contains("assets + \"d3.min.js\""));
        assert!(js.contains("data:text/javascript; charset=utf-8;base64,"));
    }

    #[test]
    fn standalone_failure_and_cancellation_preserve_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.html");
        std::fs::write(&path, "existing output").unwrap();
        let missing = Snapshot {
            html: "<img src=\"missing.png\">".into(),
            folder: Some(DocBase::Local(dir.path().to_owned())),
        };
        assert!(save(&missing, &path, &|| false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing output");
        assert!(save(&missing, &path, &|| true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing output");
        let valid = Snapshot {
            html: "<h1>保存</h1>".into(),
            folder: None,
        };
        save(&valid, &path, &|| false).unwrap();
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("<h1>保存</h1>")
        );
    }

    #[test]
    fn standalone_embeds_picture_variants_and_backgrounds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"image bytes").unwrap();
        let source = Snapshot { html: "<picture><source srcset=\"a.png 1x, a.png 2x\"><img src=\"a.png\" style=\"background:url('a.png')\"></picture>".into(), folder: Some(DocBase::Local(dir.path().to_owned())) };
        let html = bundle(&source, &|| false).unwrap();
        assert_eq!(html.matches("data:image/png;base64,").count(), 4);
        assert!(!html.contains("a.png"));
    }

    #[test]
    fn standalone_svg_nested_images_are_embedded_and_cycles_fail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), b"pixels").unwrap();
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"a.png\"/></svg>";
        std::fs::write(dir.path().join("nested.svg"), svg).unwrap();
        let source = Snapshot {
            html: "<img src='nested.svg'>".into(),
            folder: Some(DocBase::Local(dir.path().to_owned())),
        };
        let output = bundle(&source, &|| false).unwrap();
        let uri = output
            .split("data:image/svg+xml;base64,")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let decoded =
            String::from_utf8(data_encoding::BASE64.decode(uri.as_bytes()).unwrap()).unwrap();
        assert!(decoded.contains("data:image/png;base64,cGl4ZWxz"));
        assert!(!decoded.contains("a.png"));
        std::fs::write(
            dir.path().join("nested.svg"),
            svg.replace("a.png", "nested.svg"),
        )
        .unwrap();
        assert!(
            bundle(&source, &|| false)
                .unwrap_err()
                .to_string()
                .contains("循環")
        );
    }
}
