//! Embeddable editor using yyeditor's actual document, layout, IME and renderer.
use crate::render::{Composition, Frame, Renderer};
use windows::{Win32::Foundation::HWND, core::Result};
use yy_core::{Document, Selection, SelectionSet, motion};
use yy_layout::{RowConfig, Viewport};

pub struct Editor {
    pub document: Document,
    pub renderer: Renderer,
    pub viewport: Viewport,
    pub composition: Option<Composition>,
    pub dpi: u32,
    /// Optional marked byte ranges (for embedders such as yymerge).
    pub marks: Vec<std::ops::Range<u64>>,
    pub follow_caret: bool,
    pub scroll_x: f32,
    high_surrogate: Option<u16>,
    page: usize,
    syntax: Option<yy_syntax::SyntaxIndex>,
    syntax_version: u64,
}
impl Editor {
    pub fn new(text: &str, dpi: u32) -> Result<Self> {
        Ok(Self {
            document: Document::from_text(text),
            renderer: Renderer::new(
                crate::font::BUNDLED_FAMILY,
                11.,
                4,
                yy_config::Colors::default(),
                dpi,
            )?,
            viewport: Viewport::default(),
            composition: None,
            dpi,
            marks: vec![],
            follow_caret: true,
            scroll_x: 0.,
            high_surrogate: None,
            page: 20,
            syntax: yy_syntax::Registry::builtin()
                .get("sql")
                .ok()
                .map(yy_syntax::SyntaxIndex::new),
            syntax_version: 0,
        })
    }
    /// Select syntax explicitly; None renders plain text. Existing SQL default is preserved.
    pub fn set_language(&mut self, language: Option<&str>) {
        self.syntax = language
            .and_then(|name| yy_syntax::Registry::builtin().get(name).ok())
            .map(yy_syntax::SyntaxIndex::new);
        self.syntax_version = 0;
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .document
                .snapshot()
                .read(0..self.document.snapshot().len()),
        )
        .into_owned()
    }
    pub fn execution_text(&self) -> String {
        self.document
            .selected_text(16 * 1024 * 1024)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.text())
    }
    pub fn paint(&mut self, hwnd: HWND, w: u32, h: u32) -> Result<()> {
        self.render(Some(hwnd), w, h).map(|_| ())
    }
    pub fn offscreen(&mut self, w: u32, h: u32) -> Result<Vec<u8>> {
        self.render(None, w, h).map(|v| v.unwrap_or_default())
    }
    fn render(&mut self, hwnd: Option<HWND>, w: u32, h: u32) -> Result<Option<Vec<u8>>> {
        let metrics = self.renderer.metrics();
        let cfg = RowConfig::default();
        self.page = ((h as f32 * 96. / self.dpi as f32) / metrics.line_height).max(1.) as usize;
        if self.follow_caret {
            self.viewport.ensure_visible(
                self.document.snapshot(),
                &cfg,
                self.document.selections().primary().head,
                self.page,
            );
        }
        let rows = yy_layout::rows_from(
            self.document.snapshot(),
            &cfg,
            self.viewport.top,
            self.page + 1,
        );
        let selections = self
            .document
            .selections()
            .iter()
            .map(|s| s.range())
            .collect::<Vec<_>>();
        let carets = self
            .document
            .selections()
            .iter()
            .map(|s| s.head)
            .collect::<Vec<_>>();
        let tokens = if let Some(index) = &mut self.syntax {
            if self.syntax_version != self.document.version() {
                index.truncate(0);
                self.syntax_version = self.document.version();
            }
            index.extend(self.document.snapshot(), 64 * 1024);
            let (lines, _) = index.highlight_lines(
                self.document.snapshot(),
                self.viewport.top,
                rows.last().map(|r| r.next).unwrap_or(0),
            );
            let palette =
                crate::highlight::token_palette(index.syntax(), &yy_config::Colors::default());
            crate::highlight::row_tokens(&rows, &lines, &palette)
        } else {
            vec![]
        };
        let f = Frame {
            version: self.document.version(),
            rows: &rows,
            first_line: yy_layout::line_number(self.document.snapshot(), self.viewport.top).line,
            line_digits: 4,
            line_exact: true,
            show_line_numbers: true,
            scroll_x: self.scroll_x,
            selections: &selections,
            matches: &self.marks,
            carets: &carets,
            caret_visible: true,
            overwrite: false,
            composition: self.composition.as_ref(),
            rect: &[],
            tokens: &tokens,
            brackets: &[],
        };
        if let Some(hwnd) = hwnd {
            self.renderer.draw(hwnd, w, h, &f)?;
            Ok(None)
        } else {
            self.renderer.render_offscreen(w, h, &f).map(Some)
        }
    }
    pub fn click(&mut self, x: i32, y: i32, extend: bool) {
        let metrics = self.renderer.metrics();
        let row = (y.max(0) as f32 * 96. / self.dpi as f32 / metrics.line_height) as usize;
        let rows = yy_layout::rows_from(
            self.document.snapshot(),
            &RowConfig::default(),
            self.viewport.top,
            row + 1,
        );
        if let Some(r) = rows.get(row) {
            let x = x as f32 * 96. / self.dpi as f32 - self.renderer.gutter_width(4, true)
                + self.scroll_x;
            let at = self.renderer.hit_test(r, x);
            let anchor = if extend {
                self.document.selections().primary().anchor
            } else {
                at
            };
            self.document
                .set_selections(SelectionSet::single(Selection::new(anchor, at)));
        }
    }
    pub fn character(&mut self, value: u16) {
        if (0xd800..=0xdbff).contains(&value) {
            self.high_surrogate = Some(value);
            return;
        }
        let units = if let Some(high) = self.high_surrogate.take() {
            vec![high, value]
        } else {
            vec![value]
        };
        let text = String::from_utf16_lossy(&units);
        if text.chars().all(|c| !c.is_control()) {
            self.document.insert_text(&text, false);
        } else if value == 13 {
            self.document.insert_newline(true);
        } else if value == 9 {
            self.document.insert_text("\t", false);
        }
    }
    pub fn key(&mut self, hwnd: HWND, key: u32, ctrl: bool, shift: bool) -> bool {
        if ctrl {
            match key {
                65 => {
                    self.document.select_all();
                }
                67 | 88 => {
                    if let Ok(Some(s)) = self.document.selected_text(16 * 1024 * 1024) {
                        crate::clipboard::set_text(hwnd, &s, false);
                        if key == 88 {
                            self.document.delete_selection(yy_core::EditKind::Delete);
                        }
                    }
                }
                86 => {
                    if let Some((s, _)) = crate::clipboard::get_text(hwnd) {
                        self.document.paste(&s);
                    }
                }
                90 => {
                    self.document.undo();
                }
                89 => {
                    self.document.redo();
                }
                _ => return false,
            }
            return true;
        }
        match key {
            8 => {
                self.document.delete_backward();
            }
            46 => {
                self.document.delete_forward();
            }
            37 | 39 | 36 | 35 => {
                self.document.move_carets(shift, |snap, s| match key {
                    37 => motion::prev_grapheme(snap, s.head),
                    39 => motion::next_grapheme(snap, s.head),
                    36 => motion::line_start(snap, s.head),
                    _ => motion::line_end(snap, s.head),
                });
            }
            38 | 40 => {
                let s = *self.document.selections().primary();
                let snap = self.document.snapshot();
                let start = motion::line_start(snap, s.head);
                let cfg = RowConfig::default();
                let current = yy_layout::row_at(snap, &cfg, start);
                let x = self.renderer.caret_x(&current, s.head);
                let target = if key == 38 {
                    yy_layout::prev_row_start(snap, &cfg, start)
                } else {
                    yy_layout::next_row_start(snap, &cfg, start)
                };
                if let Some(at) = target {
                    let row = yy_layout::row_at(snap, &cfg, at);
                    let p = self.renderer.hit_test(&row, x);
                    self.document
                        .set_selections(SelectionSet::single(Selection::new(
                            if shift { s.anchor } else { p },
                            p,
                        )));
                }
            }
            _ => return false,
        }
        true
    }
}
