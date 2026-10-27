use super::*;
pub(super) fn add_menus(root: HMENU) -> Result<()> {
    unsafe {
        for (name, items) in [
            (
                "ページ",
                vec![
                    (50, "追加…"),
                    (51, "削除"),
                    (52, "複製"),
                    (53, "名前の変更…"),
                    (54, "寸法の変更…"),
                ],
            ),
            (
                "出力・共有",
                vec![
                    (60, "出力範囲: 現在のページ"),
                    (61, "出力範囲: 図全体"),
                    (62, "出力範囲: 選択範囲"),
                    (63, "SVG の文字をテキストで保持"),
                    (65, "透明な背景"),
                    (66, "方眼紙を出力"),
                    (67, "背景色…"),
                    (68, "余白…"),
                    (58, "PNG 画像としてコピー"),
                    (59, "SVG としてコピー"),
                    (CANCEL, "処理を取消"),
                ],
            ),
            (
                "素材・設定",
                vec![
                    (55, "素材パックを追加…"),
                    (56, "素材パックの版を選択…"),
                    (70, "素材パックの版を削除…"),
                    (57, ".yyd の関連付けを登録"),
                ],
            ),
        ] {
            let menu = CreatePopupMenu()?;
            for (id, label) in items {
                AppendMenuW(menu, MF_STRING, id as usize, &HSTRING::from(label))?;
            }
            AppendMenuW(root, MF_POPUP, menu.0 as usize, &HSTRING::from(name))?;
        }
        Ok(())
    }
}
pub(super) fn copy_shapes(hwnd: HWND, doc: &Document) -> bool {
    if cfg!(test) {
        let _ = (hwnd, doc);
        true
    } else {
        let Ok(bytes) = format::encode(doc) else {
            return false;
        };
        crate::clipboard::set_formats(
            hwnd,
            &[(crate::clipboard::named_format("YYDraw.Shapes.1"), bytes)],
        )
    }
}
pub(super) fn pasted_shapes(
    hwnd: HWND,
    fallback: &Option<(Document, Vec<Id>)>,
) -> Option<Document> {
    if cfg!(test) {
        let _ = hwnd;
        fallback.as_ref().map(|(d, ids)| d.fragment(ids))
    } else {
        let _ = fallback;
        let mut bytes = crate::clipboard::get_data(
            hwnd,
            crate::clipboard::named_format("YYDraw.Shapes.1"),
            64 * 1024 * 1024 + 4096,
        )?;
        if bytes.len() < 52 {
            return None;
        }
        let len = u64::from_le_bytes(bytes[12..20].try_into().ok()?).checked_add(52)?;
        if len > bytes.len() as u64 {
            return None;
        }
        bytes.truncate(len as usize);
        format::decode(&bytes).ok()
    }
}
pub(super) fn library_path() -> std::result::Result<PathBuf, String> {
    yy_config::Config::default_path()
        .and_then(|p| p.parent().map(|p| p.join("yydraw").join("library.yyd")))
        .ok_or_else(|| "設定フォルダが見つかりません".into())
}
pub(super) fn load_library() -> Document {
    let mut doc = Document::default();
    yy_draw::assets::add_starter(&mut doc);
    #[cfg(not(test))]
    if let Ok(path) = library_path()
        && path.exists()
    {
        match format::load(&path) {
            Ok(d) => return d,
            Err(e) => eprintln!("yydraw 素材ライブラリ: {e}"),
        }
    }
    doc
}
impl App {
    pub(super) fn update_menu(&self, menu: HMENU) {
        let doc = &self.tabs[self.tab].doc;
        unsafe {
            for (id, checked) in [
                (GRID, doc.show_grid),
                (SNAP, doc.snap),
                (JUMPS, doc.jumps),
                (
                    ALIGN_SELECTION,
                    self.layout_reference == layout::Reference::Selection,
                ),
                (ALIGN_PAGE, self.layout_reference == layout::Reference::Page),
                (60, self.export_scope == 60),
                (61, self.export_scope == 61),
                (62, self.export_scope == 62),
                (63, self.export_text),
                (65, self.transparent),
                (66, self.export_grid),
            ] {
                CheckMenuItem(
                    menu,
                    u32::from(id),
                    (MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED }).0,
                );
            }
            let units = doc
                .layout_units(&self.tabs[self.tab].selected)
                .iter()
                .filter(|u| !u.locked)
                .count();
            let selection = &self.tabs[self.tab].selected;
            let grouped = doc
                .nodes
                .iter()
                .any(|n| selection.contains(&n.id) && n.group.is_some())
                || doc
                    .connectors
                    .iter()
                    .any(|c| selection.contains(&c.id) && c.group.is_some());
            for (id, enabled) in [
                (GROUP, selection.len() >= 2),
                (UNGROUP, grouped),
                (
                    ALIGN_LEFT,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    CENTER_X,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    ALIGN_RIGHT,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    ALIGN_TOP,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    CENTER_Y,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    ALIGN_BOTTOM,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            1
                        } else {
                            2
                        },
                ),
                (
                    DISTRIBUTE,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            2
                        } else {
                            3
                        },
                ),
                (
                    DISTRIBUTE_Y,
                    units
                        >= if self.layout_reference == layout::Reference::Page {
                            2
                        } else {
                            3
                        },
                ),
            ] {
                let _ = EnableMenuItem(
                    menu,
                    id as u32,
                    MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
                );
            }
            let _ = EnableMenuItem(
                menu,
                u32::from(CANCEL),
                MF_BYCOMMAND
                    | if self.work.is_some() {
                        MF_ENABLED
                    } else {
                        MF_GRAYED
                    },
            );
        }
    }
    pub(super) fn refresh_pages(&mut self) {
        if self.pages_combo.0.is_null() {
            return;
        }
        unsafe {
            SendMessageW(self.pages_combo, CB_RESETCONTENT, None, None);
            for p in &self.tabs[self.tab].doc.pages {
                let s = HSTRING::from(&p.name);
                SendMessageW(
                    self.pages_combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(s.as_ptr() as isize)),
                );
            }
            SendMessageW(
                self.pages_combo,
                CB_SETCURSEL,
                Some(WPARAM(self.tabs[self.tab].doc.active_page)),
                None,
            );
        }
    }
    pub(super) fn export_document(&self) -> std::result::Result<(Document, Export), String> {
        let tab = &self.tabs[self.tab];
        let doc = if self.export_scope == 62 {
            if tab.selected.is_empty() {
                return Err("出力する図形・線を選択してください".into());
            }
            tab.doc.fragment(&tab.selected)
        } else {
            tab.doc.clone()
        };
        let bounds = if self.export_scope == 60 {
            doc.page_bounds().expand(self.export_margin)
        } else {
            doc.bounds().expand(self.export_margin - 16.)
        };
        let options = Export {
            bounds,
            background: if self.transparent {
                None
            } else {
                Some(self.export_color)
            },
            grid: self.export_grid,
            text: self.export_text,
            selected: vec![],
        };
        Ok((doc, options))
    }
    pub(super) fn double_click(&mut self, p: Point) {
        let q = self.document_point(p);
        let id = geometry::hit(&self.tabs[self.tab].doc, q, 8. / self.tabs[self.tab].zoom);
        let tab = self.current();
        if let Some(id) = id
            && tab.doc.connectors.iter().any(|c| c.id == id)
        {
            let before = tab.doc.clone();
            if key(VK_MENU) {
                yy_draw::editing::remove_waypoint(&mut tab.doc, id, q, 12. / tab.zoom);
            } else if let Err(e) = yy_draw::editing::add_waypoint(&mut tab.doc, id, q) {
                self.status(&e);
                return;
            }
            tab.history.commit(before, &tab.doc);
            tab.selected = vec![id];
            self.refresh_tabs();
            self.invalidate();
        } else if !tab.selected.is_empty() {
            unsafe {
                let _ = SetFocus(Some(self.label));
                SendMessageW(self.label, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
    }
    fn save_library(&mut self, doc: Document) {
        let path = library_path();
        self.work = Some(yy_draw::jobs::Task::spawn(move |c| {
            let path = path?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            format::save_control(&path, &doc, &c)?;
            Ok(work::Outcome::LibrarySaved(doc))
        }));
    }
}
pub(super) fn handle(hwnd: HWND, id: u16) -> bool {
    match id {
        CANCEL => {
            with(|a| {
                a.close_after = None;
                a.closing_all = false;
                if let Some(task) = &a.work {
                    if task.control.cancel() {
                        a.status("取消を要求しました");
                    } else {
                        a.status("保存の確定処理が完了するまでお待ちください");
                    }
                }
            });
        }
        50..=54 => {
            let initial = with(|a| {
                let p = &a.tabs[a.tab].doc.pages[a.tabs[a.tab].doc.active_page];
                if id == 54 {
                    format!("{} x {}", p.width, p.height)
                } else {
                    p.name.clone()
                }
            })
            .unwrap_or_default();
            let input = if id == 50 || id == 53 || id == 54 {
                crate::goto::prompt_text(
                    hwnd,
                    "ページ",
                    if id == 54 {
                        "ページ寸法 (幅 x 高さ、DIP)"
                    } else {
                        "ページ名"
                    },
                    if id == 50 {
                        "新しいページ"
                    } else {
                        &initial
                    },
                )
            } else {
                Some(String::new())
            };
            let Some(input) = input else {
                return true;
            };
            with(|a| {
                let tab = a.current();
                let before = tab.doc.clone();
                let result = match id {
                    50 => tab.doc.add_page(input).map(|_| ()),
                    51 => tab.doc.delete_page(),
                    52 => tab.doc.duplicate_page().map(|_| ()),
                    53 => {
                        let i = tab.doc.active_page;
                        tab.doc.pages[i].name = input;
                        tab.doc.validate()
                    }
                    _ => {
                        let values = input
                            .split(['x', 'X', '×', ','])
                            .map(str::trim)
                            .filter_map(|v| v.parse::<f64>().ok())
                            .collect::<Vec<_>>();
                        if values.len() != 2 {
                            Err("幅 x 高さで指定してください".into())
                        } else {
                            let i = tab.doc.active_page;
                            tab.doc.pages[i].width = values[0];
                            tab.doc.pages[i].height = values[1];
                            tab.doc.validate()
                        }
                    }
                };
                if let Err(e) = result {
                    tab.doc = before;
                    a.status(&e);
                    return;
                }
                tab.history.commit(before, &tab.doc);
                tab.selected.clear();
                a.refresh_tabs();
                a.properties();
                a.invalidate();
            });
        }
        55 => {
            if with(|a| a.busy()).unwrap_or(true) {
                return true;
            }
            if let Some(path) = dialog(hwnd, false, "toml", false) {
                with(|a| {
                    a.begin_import(move |renderer, c| {
                        c.checkpoint(5, "素材パックを読み込み中")?;
                        yy_draw::library::import_manifest(&path, renderer)
                    });
                });
            }
        }
        56 | 70 => {
            if with(|a| a.busy()).unwrap_or(true) {
                return true;
            }
            let packs = with(|a| a.library.packs.clone()).unwrap_or_default();
            let list = packs
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    format!(
                        "{}: {} / {}{}",
                        i + 1,
                        p.id,
                        p.version,
                        if p.enabled { " (使用中)" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let Some(n) = crate::goto::prompt_text(
                hwnd,
                "素材パックの版",
                &format!(
                    "{list}\n{}する版の番号",
                    if id == 70 { "削除" } else { "有効化" }
                ),
                "1",
            )
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|i| i.checked_sub(1)) else {
                return true;
            };
            let Some(pack) = packs.get(n).cloned() else {
                error_box(hwnd, "版の番号が不正です");
                return true;
            };
            with(|a| {
                let mut library = a.library.clone();
                if id == 70 {
                    library
                        .packs
                        .retain(|p| p.id != pack.id || p.version != pack.version);
                    let used = library
                        .packs
                        .iter()
                        .flat_map(|p| p.assets.iter().copied())
                        .collect::<std::collections::HashSet<_>>();
                    library.assets.retain(|v| used.contains(&v.id));
                    a.save_library(library);
                } else {
                    let mut bundle = library;
                    bundle
                        .packs
                        .retain(|p| p.id == pack.id && p.version == pack.version);
                    bundle.assets.retain(|v| pack.assets.contains(&v.id));
                    a.begin_import(move |_, _| Ok(bundle));
                }
            });
        }
        57 => match super::association::register() {
            Ok(()) => info_box(
                hwnd,
                ".yyd を yydraw で開く登録を追加しました。既存の既定アプリは保持します。必要に応じて Windows の「プログラムから開く」で選択してください。",
            ),
            Err(e) => error_box(hwnd, &e),
        },
        58 | 59 => {
            let png = crate::clipboard::named_format("PNG");
            let svg = crate::clipboard::named_format("image/svg+xml");
            let result = with(|a| {
                if a.busy() {
                    return Err("処理中です".into());
                }
                let (doc, options) = a.export_document()?;
                let renderer = a.renderer.clone();
                let scale = a.export_scale;
                a.work = Some(yy_draw::jobs::Task::spawn(move |c| {
                    c.checkpoint(10, "画像を作成中")?;
                    let formats = if id == 58 {
                        let bytes = renderer.png(&doc, &options, scale)?;
                        let dib = yy_draw::render::png_dib(&bytes)?;
                        vec![
                            (png, bytes),
                            (windows::Win32::System::Ole::CF_DIBV5.0 as u32, dib),
                        ]
                    } else {
                        let bytes = renderer.svg(&doc, &options)?.into_bytes();
                        let text = String::from_utf8_lossy(&bytes)
                            .encode_utf16()
                            .chain(Some(0))
                            .flat_map(u16::to_le_bytes)
                            .collect();
                        vec![
                            (svg, bytes),
                            (windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32, text),
                        ]
                    };
                    c.checkpoint(95, "クリップボードへ転送")?;
                    Ok(work::Outcome::Image { formats })
                }));
                Ok::<_, String>(())
            });
            if let Some(Err(e)) = result {
                error_box(hwnd, &e);
            }
        }
        60..=63 | 65 | 66 => {
            with(|a| {
                match id {
                    60..=62 => a.export_scope = id,
                    63 => a.export_text = !a.export_text,
                    65 => a.transparent = !a.transparent,
                    _ => a.export_grid = !a.export_grid,
                }
                a.status("出力設定を変更しました");
            });
        }
        67 => colors::choose(hwnd, 67),
        68 => {
            let initial = with(|a| a.export_margin.to_string()).unwrap_or_default();
            if let Some(value) =
                crate::goto::prompt_text(hwnd, "出力設定", "出力の余白 (DIP、0～1000)", &initial)
            {
                let result = with(|a| {
                    let margin = value.parse::<f64>().map_err(|_| "余白を確認してください")?;
                    if !(0.0..=1000.).contains(&margin) {
                        return Err("余白を確認してください");
                    }
                    a.export_margin = margin;
                    Ok::<_, &str>(())
                });
                if let Some(Err(e)) = result {
                    error_box(hwnd, e);
                }
            }
        }
        _ => return false,
    }
    true
}
