use super::*;
pub const CLOSE_SAVED: u32 = WM_APP + 91;
pub const ERROR_READY: u32 = WM_APP + 92;
pub struct Frame {
    pub camera: interaction::Camera,
    pub pixels: Vec<u8>,
    pub size: (u32, u32),
    pub preview: bool,
    pub sprite: Option<Sprite>,
    pub preview_ids: Vec<Id>,
}
pub struct Sprite {
    pub pixels: Vec<u8>,
    pub size: (u32, u32),
    pub bounds: Rect,
}
pub fn draw_frame(
    renderer: &Renderer,
    mut doc: Document,
    options: Export,
    scale: f64,
    moving: Option<Vec<Id>>,
    control: yy_draw::jobs::Control,
) -> std::result::Result<Frame, String> {
    control.checkpoint(5, "描画・経路計算中")?;
    let preview = moving.is_some();
    let camera = interaction::Camera {
        origin: Point::new(options.bounds.x, options.bounds.y),
        scale,
    };
    let preview_ids = moving.clone().unwrap_or_default();
    let sprite = if let Some(ids) = moving {
        let selected: std::collections::HashSet<_> = ids.iter().copied().collect();
        let mut fragment = doc.fragment(&ids);
        fragment.connectors.clear();
        doc.nodes.retain(|n| !selected.contains(&n.id));
        doc.connectors.retain(|c| {
            !selected.contains(&c.id)
                && !matches!(c.start,Endpoint::Attached{node,..} if selected.contains(&node))
                && !matches!(c.end,Endpoint::Attached{node,..} if selected.contains(&node))
        });
        if fragment.nodes.is_empty() {
            None
        } else {
            let bounds = fragment.bounds();
            let sprite_scale = scale.min(2048. / bounds.w).min(2048. / bounds.h);
            let image = renderer.raster(
                &fragment,
                &Export {
                    bounds,
                    background: None,
                    grid: false,
                    text: false,
                    selected: ids,
                },
                sprite_scale,
            )?;
            let size = (image.width(), image.height());
            let mut pixels = image.take();
            for p in pixels.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            Some(Sprite {
                pixels,
                size,
                bounds,
            })
        }
    } else {
        None
    };
    control.checkpoint(40, "背景を描画中")?;
    let bitmap = renderer.raster(
        &doc,
        &Export {
            selected: if preview {
                vec![]
            } else {
                options.selected.clone()
            },
            ..options
        },
        scale,
    )?;
    control.checkpoint(95, "描画を反映")?;
    let size = (bitmap.width(), bitmap.height());
    let mut pixels = bitmap.take();
    for p in pixels.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    Ok(Frame {
        camera,
        pixels,
        size,
        preview,
        sprite,
        preview_ids,
    })
}
pub enum Outcome {
    Saved {
        uid: u64,
        path: PathBuf,
        snapshot: Document,
    },
    Opened {
        path: PathBuf,
        doc: Document,
    },
    Exported,
    Imported {
        bundle: Document,
        library: Document,
        uid: u64,
    },
    LibrarySaved(Document),
    Image {
        formats: Vec<(u32, Vec<u8>)>,
    },
}
impl App {
    pub(super) fn begin_import(
        &mut self,
        load: impl FnOnce(&Renderer, &yy_draw::jobs::Control) -> std::result::Result<Document, String>
        + Send
        + 'static,
    ) {
        if self.busy() {
            return;
        }
        let renderer = self.renderer.clone();
        let mut library = self.library.clone();
        let uid = self.tabs[self.tab].uid;
        let path = features::library_path();
        self.work = Some(yy_draw::jobs::Task::spawn(move |c| {
            c.checkpoint(5, "素材を読み込み中")?;
            let bundle = load(&renderer, &c)?;
            c.checkpoint(60, "素材パックを検証中")?;
            yy_draw::library::install(&mut library, &bundle, true)?;
            let path = path?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            format::save_control(&path, &library, &c)?;
            Ok(Outcome::Imported {
                bundle,
                library,
                uid,
            })
        }));
    }
    pub(super) fn busy(&self) -> bool {
        if self.work.is_some() {
            self.status("処理中です。取消または完了後に操作してください");
            true
        } else {
            false
        }
    }
    pub(super) fn begin_save(&mut self, path: PathBuf) -> bool {
        if self.busy() {
            return false;
        }
        let snapshot = self.tabs[self.tab].doc.clone();
        let uid = self.tabs[self.tab].uid;
        self.work = Some(yy_draw::jobs::Task::spawn(move |control| {
            format::save_control(&path, &snapshot, &control)?;
            Ok(Outcome::Saved {
                uid,
                path,
                snapshot,
            })
        }));
        self.status("保存中 — 取消メニューで取り消せます");
        true
    }
    pub(super) fn poll_work(&mut self) -> Option<u32> {
        if let Some((revision, task)) = &self.frame
            && let Some(result) = task.poll()
        {
            let revision = *revision;
            self.frame = None;
            match result {
                Ok(frame) if revision == self.revision => {
                    self.pixels = frame.pixels;
                    self.image_camera = Some(frame.camera);
                    self.image_uid = self.tabs[self.tab].uid;
                    self.pending_connector = None;
                    self.pixsize = frame.size;
                    self.pixel_revision = revision;
                    self.preview_visible = frame.preview;
                    self.sprite = frame.sprite;
                    self.preview_ids = frame.preview_ids;
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                Err(e) if revision == self.revision => self.status(&e),
                _ => {}
            }
        }
        if self.frame_ready() && self.frame.is_none() {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
        let task = self.work.as_ref()?;
        let Some(result) = task.poll() else {
            let (p, s) = task.control.status();
            self.status(&format!("{s} {p}% — 処理を取消で中止"));
            return None;
        };
        let result = if task.control.cancelled() {
            Err("処理を取り消しました".into())
        } else {
            result
        };
        self.work = None;
        match result {
            Ok(Outcome::Saved {
                uid,
                path,
                mut snapshot,
            }) => {
                let mut close = false;
                let mut found = None;
                for (i, tab) in self.tabs.iter_mut().enumerate() {
                    if tab.uid == uid {
                        if tab.doc.active_page < snapshot.pages.len() {
                            let _ = snapshot.switch_page(tab.doc.active_page);
                        }
                        close = tab.doc == snapshot;
                        tab.saved = snapshot;
                        tab.path = Some(path);
                        found = Some(i);
                        break;
                    }
                }
                self.refresh_tabs();
                self.status("保存しました");
                if self.close_after.take() == Some(uid) && close {
                    if let Some(i) = found {
                        self.tab = i;
                    }
                    return Some(if self.closing_all {
                        WM_CLOSE
                    } else {
                        CLOSE_SAVED
                    });
                }
                self.closing_all = false;
            }
            Ok(Outcome::Opened { path, doc }) => {
                if self.tabs.len() == 1
                    && self.tabs[0].path.is_none()
                    && !self.tabs[0].dirty()
                    && self.tabs[0].doc.pages.len() == 1
                    && self.tabs[0].doc.nodes.is_empty()
                    && self.tabs[0].doc.connectors.is_empty()
                {
                    self.tabs.clear();
                }
                self.tabs.push(Tab::new(doc, Some(path)));
                self.tab = self.tabs.len() - 1;
                self.properties();
                self.rebuild_palette();
                self.refresh_tabs();
                self.invalidate();
            }
            Ok(Outcome::Imported {
                bundle,
                library,
                uid,
            }) => {
                self.library = library;
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.uid == uid) {
                    let before = tab.doc.clone();
                    if let Err(e) = yy_draw::library::install(&mut tab.doc, &bundle, true) {
                        self.pending_error = Some(e);
                        return Some(ERROR_READY);
                    }
                    tab.history.commit(before, &tab.doc);
                }
                self.rebuild_palette();
                self.refresh_tabs();
                self.invalidate();
                self.status("素材パックを追加しました");
            }
            Ok(Outcome::Image { formats }) => {
                if !crate::clipboard::set_formats(self.hwnd, &formats) {
                    self.pending_error = Some("クリップボードへ書き込めませんでした".into());
                    return Some(ERROR_READY);
                }
                self.status("画像をコピーしました");
            }
            Ok(Outcome::Exported) => self.status("エクスポートしました"),
            Ok(Outcome::LibrarySaved(library)) => {
                self.library = library;
                self.rebuild_palette();
                self.status("素材パックを保存しました");
            }
            Err(e) => {
                self.close_after = None;
                self.closing_all = false;
                if e.contains("取り消") {
                    self.status(&e);
                } else {
                    self.pending_error = Some(e);
                    return Some(ERROR_READY);
                }
            }
        }
        None
    }
}
