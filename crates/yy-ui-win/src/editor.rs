//! Embeddable editor using yyeditor's actual document, layout, IME and renderer.
use crate::render::{Composition, Frame, Renderer};
use crate::{aligned::AlignedView, render::RowDecoration};
use windows::{Win32::Foundation::HWND, core::Result};
use yy_core::{Document, Selection, SelectionSet, motion};
use yy_layout::{RowConfig, Viewport};

pub struct Editor {
    pub document: Document,
    pub renderer: Renderer,
    pub viewport: Viewport,
    pub composition: Option<Composition>,
    pub dpi: u32,
    /// Optional sorted, non-overlapping marked byte ranges (e.g. yymerge).
    pub marks: Vec<std::ops::Range<u64>>,
    pub follow_caret: bool,
    pub scroll_x: f32,
    /// Display-only mapping. Source documents remain untouched.
    pub aligned: Option<AlignedView>,
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
            aligned: None,
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
    pub fn reveal_caret(&mut self) {
        if let Some(view) = &mut self.aligned {
            view.reveal(self.document.selections().primary().head, self.page);
        }
    }
    /// Detach an obsolete display map without changing text, selections or undo.
    pub fn clear_alignment(&mut self) {
        if let Some(view) = self.aligned.take() {
            let at = view
                .source_near_row(view.top)
                .unwrap_or(0)
                .min(self.document.snapshot().len());
            self.viewport.top =
                yy_layout::row_containing(self.document.snapshot(), &RowConfig::default(), at);
        }
    }
    pub fn scroll_rows(&mut self, delta: i64) {
        self.follow_caret = false;
        if let Some(view) = &mut self.aligned {
            view.scroll(delta, self.page);
        } else {
            self.viewport.scroll_rows(
                self.document.snapshot(),
                &RowConfig::default(),
                delta,
                self.page,
            );
        }
    }
    fn line_digits(&self) -> usize {
        let count = self
            .aligned
            .as_ref()
            .map(|v| v.rows.len() as u64)
            .unwrap_or_else(|| self.document.snapshot().estimated_line_count())
            .max(1);
        ((count.ilog10() + 1) as usize).max(4)
    }
    fn visible_rows(&self, count: usize) -> (Vec<yy_layout::Row>, Vec<RowDecoration>) {
        let Some(view) = &self.aligned else {
            return (
                yy_layout::rows_from(
                    self.document.snapshot(),
                    &RowConfig::default(),
                    self.viewport.top,
                    count,
                ),
                vec![],
            );
        };
        let mut rows = Vec::new();
        let mut styles = Vec::new();
        for slot in view.rows.iter().skip(view.top).take(count) {
            rows.push(if let Some(start) = slot.source {
                yy_layout::row_at(self.document.snapshot(), &RowConfig::default(), start)
            } else {
                yy_layout::Row {
                    start: u64::MAX,
                    end: u64::MAX,
                    next: u64::MAX,
                    line_start: false,
                    ends_line: false,
                    text: String::new(),
                    spans: vec![],
                }
            });
            styles.push(slot.decoration);
        }
        (rows, styles)
    }
    fn render(&mut self, hwnd: Option<HWND>, w: u32, h: u32) -> Result<Option<Vec<u8>>> {
        let metrics = self.renderer.metrics();
        let cfg = RowConfig::default();
        self.page = ((h as f32 * 96. / self.dpi as f32) / metrics.line_height).max(1.) as usize;
        if self.follow_caret && self.aligned.is_some() {
            self.reveal_caret();
        } else if self.follow_caret {
            self.viewport.ensure_visible(
                self.document.snapshot(),
                &cfg,
                self.document.selections().primary().head,
                self.page,
            );
        }
        if let Some(view) = &mut self.aligned {
            view.top = view.top.min(view.last_top(self.page));
        }
        let (rows, decorations) = self.visible_rows(self.page + 1);
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
                rows.iter()
                    .find(|r| r.start != u64::MAX)
                    .map_or(0, |r| r.start),
                rows.iter()
                    .rev()
                    .find(|r| r.start != u64::MAX)
                    .map_or(0, |r| r.next),
            );
            let palette =
                crate::highlight::token_palette(index.syntax(), &yy_config::Colors::default());
            crate::highlight::row_tokens(&rows, &lines, &palette)
        } else {
            vec![]
        };
        let marks = if self.aligned.is_some() {
            let first = rows.iter().find(|r| r.start != u64::MAX).map(|r| r.start);
            let last = rows
                .iter()
                .rev()
                .find(|r| r.start != u64::MAX)
                .map(|r| r.end);
            if let (Some(first), Some(last)) = (first, last) {
                let start = self.marks.partition_point(|r| r.end <= first);
                let end = start + self.marks[start..].partition_point(|r| r.start < last);
                &self.marks[start..end]
            } else {
                &[]
            }
        } else {
            &self.marks[..]
        };
        let f = Frame {
            version: self.document.version(),
            rows: &rows,
            first_line: yy_layout::line_number(self.document.snapshot(), self.viewport.top).line,
            line_digits: self.line_digits(),
            line_exact: true,
            show_line_numbers: true,
            scroll_x: self.scroll_x,
            selections: &selections,
            matches: marks,
            carets: &carets,
            caret_visible: true,
            overwrite: false,
            composition: self.composition.as_ref(),
            rect: &[],
            tokens: &tokens,
            brackets: &[],
        };
        if let Some(hwnd) = hwnd {
            self.renderer.draw_decorated(hwnd, w, h, &f, &decorations)?;
            Ok(None)
        } else {
            self.renderer
                .render_offscreen_decorated(w, h, &f, &decorations)
                .map(Some)
        }
    }
    pub fn click(&mut self, x: i32, y: i32, extend: bool) {
        let metrics = self.renderer.metrics();
        let row = (y.max(0) as f32 * 96. / self.dpi as f32 / metrics.line_height) as usize;
        let r = if let Some(view) = &self.aligned {
            view.source_near_row(view.top + row)
                .map(|at| yy_layout::row_at(self.document.snapshot(), &RowConfig::default(), at))
        } else {
            self.visible_rows(row + 1).0.into_iter().nth(row)
        };
        if let Some(r) = r {
            let x = x as f32 * 96. / self.dpi as f32
                - self.renderer.text_origin_x(self.line_digits(), true)
                + self.scroll_x;
            let at = self.renderer.hit_test(&r, x);
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
                35 | 36 => {
                    let at = if key == 36 {
                        0
                    } else {
                        self.document.snapshot().len()
                    };
                    let anchor = if shift {
                        self.document.selections().primary().anchor
                    } else {
                        at
                    };
                    self.document
                        .set_selections(SelectionSet::single(Selection::new(anchor, at)));
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
            38 | 40 | 33 | 34 => {
                let s = *self.document.selections().primary();
                let snap = self.document.snapshot();
                let start = motion::line_start(snap, s.head);
                let cfg = RowConfig::default();
                let current = yy_layout::row_at(snap, &cfg, start);
                let x = self.renderer.caret_x(&current, s.head);
                let up = matches!(key, 33 | 38);
                let count = if matches!(key, 33 | 34) {
                    self.page.max(1)
                } else {
                    1
                };
                let target = if let Some(view) = &self.aligned {
                    let row = view.row_for_offset(s.head);
                    let target = if up {
                        row.saturating_sub(count)
                    } else {
                        row.saturating_add(count)
                            .min(view.rows.len().saturating_sub(1))
                    };
                    // Arrow keys skip placeholders; page keys move in display rows.
                    if count == 1 {
                        if up {
                            yy_layout::prev_row_start(snap, &cfg, start)
                        } else {
                            yy_layout::next_row_start(snap, &cfg, start)
                        }
                    } else {
                        view.source_near_row(target)
                    }
                } else {
                    let mut at = Some(start);
                    for _ in 0..count {
                        at = at
                            .and_then(|at| {
                                if up {
                                    yy_layout::prev_row_start(snap, &cfg, at)
                                } else {
                                    yy_layout::next_row_start(snap, &cfg, at)
                                }
                            })
                            .or(at);
                    }
                    at
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aligned::DisplayRow;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    #[test]
    fn aligned_render_hit_testing_and_selection_keep_original_text() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let original = "alpha\n日本語\nlast\n";
        let mut e = Editor::new(original, 96).unwrap();
        e.set_language(None);
        let second = original.find('日').unwrap() as u64;
        let third = original.find("last").unwrap() as u64;
        let bg = yy_config::Color::rgb(238, 240, 243);
        let row = |source, number| DisplayRow {
            source,
            decoration: RowDecoration {
                line_number: number,
                background: source.is_none().then_some(bg),
            },
        };
        e.aligned = Some(AlignedView::new(vec![
            row(Some(0), Some(1)),
            row(None, None),
            row(Some(second), Some(2)),
            row(Some(third), Some(3)),
        ]));
        let before = e.document.version();
        let image = e.offscreen(400, 200).unwrap();
        let lh = e.renderer.metrics().line_height;
        let x = (e.renderer.text_origin_x(4, true) + 1.) as i32;
        e.click(x, (lh * 2.5) as i32, false);
        assert_eq!(e.document.selections().primary().head, second);
        e.click(x, (lh * 0.5) as i32, false);
        e.click(x, (lh * 2.5) as i32, true);
        assert_eq!(e.document.selected_text(100).unwrap().unwrap(), "alpha\n");
        let at = (((lh as u32 + 2) * 400 + 200) * 4) as usize;
        assert_eq!(&image[at..at + 3], &[bg.b, bg.g, bg.r]);
        assert_eq!(e.text(), original);
        assert_eq!(e.document.version(), before);
        e.document.select_all();
        assert_eq!(e.document.selected_text(100).unwrap().unwrap(), original);
        e.document.insert_text("replacement", false);
        e.clear_alignment();
        assert!(e.aligned.is_none());
        assert_eq!(e.text(), "replacement");
        e.document.undo();
        assert_eq!(e.text(), original);
    }
}
