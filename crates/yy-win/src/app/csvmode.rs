//! 区切り文字モード（CSV / TSV の列揃え表示とセル操作。04 章）。

use std::sync::Arc;

use yy_core::csv::{CsvView, RecordOp};
use yy_core::edit::Change;
use yy_core::{EditKind, Selection, SelectionSet, motion};
use yy_delimited::{Dialect, quote_field, split_line};
use yy_layout::CellLayout;
use yy_layout::cells::measure_widths;

use super::*;

/// 最初に幅を測るレコード数（04 章 4.1）
const INITIAL_LINES: usize = 1000;

/// カーソルのあるフィールド。
struct CaretCell {
    /// 論理行の先頭
    line_start: u64,
    /// 行の内容（改行を除く）
    line: Vec<u8>,
    /// 行内のフィールドの範囲（引用符を含む）
    range: std::ops::Range<usize>,
    field: u32,
    /// 直後の区切り文字の範囲
    delim: Option<std::ops::Range<usize>>,
}

/// 区切り文字モードの状態。
pub(crate) struct CsvState {
    pub view: CsvView,
    pub widths: Vec<u32>,
}

// CSV メニューの項目
pub(crate) const ID_CSV_OFF: u16 = 700;
pub(crate) const ID_CSV_AUTO: u16 = 701;
pub(crate) const ID_CSV_COMMA: u16 = 702;
pub(crate) const ID_CSV_TAB: u16 = 703;
pub(crate) const ID_CSV_SEMICOLON: u16 = 704;
pub(crate) const ID_CSV_PIPE: u16 = 705;
pub(crate) const ID_CSV_TO_COMMA: u16 = 710;
pub(crate) const ID_CSV_TO_TAB: u16 = 711;
pub(crate) const ID_CSV_INSERT_COL: u16 = 712;
pub(crate) const ID_CSV_DELETE_COL: u16 = 713;
pub(crate) const ID_CSV_NEXT_CELL: u16 = 714;
pub(crate) const ID_CSV_PREV_CELL: u16 = 715;

pub(crate) fn create_csv_menu() -> Result<HMENU> {
    unsafe {
        let m = CreatePopupMenu()?;
        let item =
            |id: u16, text: windows::core::PCWSTR| AppendMenuW(m, MF_STRING, id as usize, text);
        item(ID_CSV_OFF, w!("区切り文字モードを使わない(&O)"))?;
        item(ID_CSV_AUTO, w!("区切り文字を自動判別(&A)"))?;
        item(ID_CSV_COMMA, w!("カンマ区切り (CSV)(&C)"))?;
        item(ID_CSV_TAB, w!("タブ区切り (TSV)(&T)"))?;
        item(ID_CSV_SEMICOLON, w!("セミコロン区切り(&S)"))?;
        item(ID_CSV_PIPE, w!("パイプ区切り(&P)"))?;
        AppendMenuW(m, MF_SEPARATOR, 0, None)?;
        item(ID_CSV_NEXT_CELL, w!("次のセル\tTab"))?;
        item(ID_CSV_PREV_CELL, w!("前のセル\tShift+Tab"))?;
        item(ID_CSV_INSERT_COL, w!("列を挿入（カーソルの列の左）(&I)"))?;
        item(ID_CSV_DELETE_COL, w!("列を削除（カーソルの列）(&D)"))?;
        AppendMenuW(m, MF_SEPARATOR, 0, None)?;
        item(ID_CSV_TO_COMMA, w!("カンマ区切りに変換"))?;
        item(ID_CSV_TO_TAB, w!("タブ区切りに変換"))?;
        Ok(m)
    }
}

impl App {
    /// 区切り文字モードを切り替える（`None` なら通常の表示）。
    pub(crate) fn set_csv_mode(&mut self, dialect: Option<Dialect>) {
        self.csv = dialect.map(|d| CsvState {
            view: CsvView::new(d),
            widths: Vec::new(),
        });
        if self.csv.is_some() {
            let n = self.notifier();
            let snap = self.doc.snapshot().clone();
            if let Some(c) = &mut self.csv {
                c.view.sync(&snap, &self.pool, n);
            }
            self.rebuild_cells();
            self.measure_widths(0, INITIAL_LINES);
        } else {
            self.rebuild_cells();
        }
        self.update_csv_menu();
        self.renderer.clear_cache();
        let page = self.page_rows();
        self.vp.clamp(self.doc.snapshot(), &self.rows_cfg, page);
        self.after_move();
    }

    /// ファイル名の拡張子から区切り文字モードを決める。
    pub(crate) fn csv_mode_for_path(&mut self) {
        let d = self
            .doc
            .path()
            .and_then(|p| p.extension())
            .and_then(|e| Dialect::for_extension(&e.to_string_lossy()));
        if d.is_some() || self.csv.is_some() {
            self.set_csv_mode(d);
        }
    }

    /// 先頭を調べて区切り文字を推定する。
    pub(crate) fn csv_auto(&mut self) -> Option<String> {
        let snap = self.doc.snapshot();
        let sample = snap.read(0..snap.len().min(64 << 10));
        match yy_delimited::sniff(&sample) {
            Some(d) => {
                self.set_csv_mode(Some(d));
                self.status_msg = format!("区切り文字: {}", d.name());
                self.update_status();
                None
            }
            None => Some("区切り文字を判別できませんでした。".into()),
        }
    }

    pub(crate) fn update_csv_menu(&self) {
        let current = self
            .csv
            .as_ref()
            .map(|c| c.view.dialect.delimiter().to_vec());
        let items: [(u16, Option<&[u8]>); 5] = [
            (ID_CSV_OFF, None),
            (ID_CSV_COMMA, Some(b",")),
            (ID_CSV_TAB, Some(b"\t")),
            (ID_CSV_SEMICOLON, Some(b";")),
            (ID_CSV_PIPE, Some(b"|")),
        ];
        for (id, d) in items {
            let on = current.as_deref() == d;
            let flag = if on { MF_CHECKED } else { MF_UNCHECKED };
            unsafe {
                CheckMenuItem(self.menu_csv, id as u32, (MF_BYCOMMAND | flag).0);
            }
        }
    }

    /// 表示の設定（列幅・インデックス）を作り直す。内容や列幅が変わったときに呼ぶ。
    pub(crate) fn rebuild_cells(&mut self) {
        // 区切り文字モードでは長い行も分割せずに列を揃える（テキストの表示では巨大な 1 行の
        // ファイルを速く表示するため、既定の単位で分割する）
        let v = &self.config.view;
        let text_limit = v.max_row_bytes.max(256) as u64;
        let limit = if self.csv.is_some() {
            text_limit.max(v.csv_max_row_bytes as u64)
        } else {
            text_limit
        };
        if limit != self.rows_cfg.max_row_bytes {
            self.rows_cfg.max_row_bytes = limit;
            // 表示位置を新しい単位の表示行の先頭に合わせる
            let snap = self.doc.snapshot();
            self.vp.top = yy_layout::row_containing(snap, &self.rows_cfg, self.vp.top);
        }
        let Some(c) = &self.csv else {
            self.rows_cfg.cells = None;
            return;
        };
        let cl = CellLayout::new(c.view.dialect, c.widths.clone(), self.ccfg, c.view.index());
        self.rows_cfg.cells = Some(Arc::new(cl));
    }

    /// 内容の変更に合わせてインデックスを更新する。
    pub(crate) fn sync_csv(&mut self) {
        if self.csv.is_none() {
            return;
        }
        let n = self.notifier();
        let snap = self.doc.snapshot().clone();
        if let Some(c) = &mut self.csv {
            c.view.sync(&snap, &self.pool, n);
        }
        self.rebuild_cells();
    }

    /// バックグラウンドのインデックス作成の進み具合を反映する。
    pub(crate) fn poll_csv(&mut self) {
        let Some(c) = &mut self.csv else { return };
        c.view.poll();
        // インデックスが進むと、行の先頭の状態（引用符の内外）が確定する
        self.rebuild_cells();
        self.renderer.clear_cache();
        self.update_status();
        self.invalidate();
    }

    /// 論理行 `line_start` から `lines` 行の幅を測り、広がったら表示を作り直す。
    pub(crate) fn measure_widths(&mut self, line_start: u64, lines: usize) {
        let Some(cl) = self.rows_cfg.cells.clone() else {
            return;
        };
        let Some(c) = &mut self.csv else { return };
        let snap = self.doc.snapshot();
        if measure_widths(
            snap,
            &cl,
            line_start,
            lines,
            self.rows_cfg.max_row_bytes,
            self.config.view.csv_max_column_width.max(1),
            &mut c.widths,
        ) {
            self.rebuild_cells();
            self.renderer.clear_cache();
        }
    }

    /// 表示中の行の幅を測る（描画の前に呼ぶ。列幅は広がるだけで縮まない）。
    pub(crate) fn measure_visible(&mut self) {
        if self.csv.is_none() {
            return;
        }
        let snap = self.doc.snapshot();
        let start = motion::line_start(snap, self.vp.top);
        let page = self.page_rows() + 1;
        self.measure_widths(start, page);
    }

    /// カーソルのある行の先頭と、行の内容（改行を除く）。列を揃えない長い行（数 GB の行など。
    /// 毎回読むと UI が止まる）なら `None`。
    fn caret_line(&self) -> Option<(u64, Vec<u8>)> {
        let snap = self.doc.snapshot();
        let head = self.doc.selections().primary().head;
        let start = motion::line_start(snap, head);
        let end = motion::line_end(snap, start);
        (end - start <= self.rows_cfg.max_row_bytes).then(|| (start, snap.read(start..end)))
    }

    /// カーソルのあるフィールド。
    fn caret_cell(&self) -> Option<CaretCell> {
        let cl = self.rows_cfg.cells.clone()?;
        let (line_start, line) = self.caret_line()?;
        let head = self.doc.selections().primary().head;
        let (range, field, delim) =
            yy_layout::cells::cell_at(self.doc.snapshot(), &cl, line_start, &line, head)?;
        Some(CaretCell {
            line_start,
            line,
            range,
            field,
            delim,
        })
    }

    /// カーソルのあるフィールドの番号。
    pub(crate) fn caret_field(&self) -> Option<u32> {
        self.caret_cell().map(|c| c.field)
    }

    /// 次（`forward`）・前のセルの先頭へ移動する。
    pub(crate) fn move_cell(&mut self, forward: bool) {
        let Some(CaretCell {
            line_start: start,
            line,
            range,
            delim,
            ..
        }) = self.caret_cell()
        else {
            return;
        };
        let snap = self.doc.snapshot().clone();
        let head = self.doc.selections().primary().head;
        let target = if forward {
            match delim {
                Some(d) => start + d.end as u64,
                // 行の最後のフィールドなら次の行の先頭
                None => {
                    let end = motion::line_end(&snap, start);
                    snap.find_next(end..snap.len(), b'\n')
                        .map_or(end, |n| n + 1)
                }
            }
        } else {
            let cell_start = start + range.start as u64;
            if head > cell_start {
                cell_start
            } else {
                let Some(cl) = self.rows_cfg.cells.clone() else {
                    return;
                };
                let (cells, _) = split_line(&line, &cl.dialect, cl.line_state(&snap, start));
                match cells.iter().rposition(|c| c.range.end < range.start) {
                    Some(i) => start + cells[i].range.start as u64,
                    // 行の最初のフィールドなら前の行の最後のフィールド
                    None if start > 0 => {
                        let prev = motion::line_start(&snap, start - 1);
                        let prev_end = motion::line_end(&snap, prev);
                        if prev_end - prev > self.rows_cfg.max_row_bytes {
                            // 列を揃えない長い行は読まずに行頭へ
                            prev
                        } else {
                            let pline = snap.read(prev..prev_end);
                            let (pc, _) =
                                split_line(&pline, &cl.dialect, cl.line_state(&snap, prev));
                            prev + pc.last().map_or(0, |c| c.range.start as u64)
                        }
                    }
                    None => 0,
                }
            }
        };
        self.rect = None;
        self.doc
            .set_selections(SelectionSet::single(Selection::caret(target)));
        self.after_move();
    }

    /// 入力した文字列を挿入する。区切り文字モードで区切り文字・引用符・改行を含む文字列を
    /// 引用符のないフィールドに入力した場合は、フィールドを引用符で囲む（04 章 5）。
    pub(crate) fn insert_typed(&mut self, text: &str) -> bool {
        if let Some(changed) = self.insert_quoted(text) {
            return changed;
        }
        self.doc.insert_text(text, self.overwrite)
    }

    fn insert_quoted(&mut self, text: &str) -> Option<bool> {
        let d = self.csv.as_ref()?.view.dialect;
        let q = d.quote?;
        let sels = self.doc.selections();
        if sels.len() != 1 || !sels.primary().is_empty() || self.overwrite {
            return None;
        }
        let bytes = text.as_bytes();
        let special = bytes
            .windows(d.delimiter().len())
            .any(|w| w == d.delimiter())
            || bytes.iter().any(|&b| b == q || b == b'\n' || b == b'\r');
        if !special {
            return None;
        }
        let CaretCell {
            line_start: start,
            line,
            range,
            ..
        } = self.caret_cell()?;
        let head = self.doc.selections().primary().head;
        let rel = (head - start) as usize;
        let raw = &line[range.clone()];
        let cell_start = start + range.start as u64;
        let (change, caret) = if raw.first() == Some(&q) {
            // すでに引用符付き: 引用符だけ重ねる
            let mut ins = Vec::new();
            for &b in bytes {
                if b == q {
                    ins.push(q);
                }
                ins.push(b);
            }
            let len = ins.len() as u64;
            (Change::replace_bytes(head..head, ins), head + len)
        } else {
            let before = &line[range.start..rel.max(range.start)];
            let after = &line[rel.max(range.start)..range.end];
            let mut value = before.to_vec();
            value.extend_from_slice(bytes);
            let caret_in = quote_field(&value, &d).len() as u64 - 1;
            value.extend_from_slice(after);
            let quoted = quote_field(&value, &d);
            let cell_end = start + range.end as u64;
            (
                Change::replace_bytes(cell_start..cell_end, quoted),
                cell_start + caret_in,
            )
        };
        let ok = self.doc.apply_changes(vec![change], EditKind::Typing, |_| {
            SelectionSet::single(Selection::caret(caret))
        });
        Some(ok)
    }

    /// 列の挿入・削除・区切り文字の変換を実行する。
    pub(crate) fn csv_record_op(&mut self, op: RecordOp) {
        let Some(d) = self.csv.as_ref().map(|c| c.view.dialect) else {
            self.status_msg = "区切り文字モードではありません".into();
            self.update_status();
            return;
        };
        self.rect = None;
        let n = self.notifier();
        let r = unsafe {
            let old = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
            let r = self.doc.transform_records(d, op, &self.pool, n);
            SetCursor(Some(old));
            r
        };
        match r {
            Ok(Some(count)) => {
                self.status_msg = format!("{} レコードを書き換えました", group_digits(count));
                self.after_record_op(op);
            }
            Ok(None) => {
                self.pending_record_op = Some(op);
                self.update_status();
            }
            Err(e) => {
                self.status_msg = format!("書き換えられませんでした: {e}");
                self.update_status();
            }
        }
    }

    /// レコードの書き直しの後処理（変換したら区切り文字モードも新しい区切り文字にする）。
    pub(crate) fn after_record_op(&mut self, op: RecordOp) {
        if let RecordOp::Convert(to) = op {
            self.set_csv_mode(Some(to));
        } else {
            if let Some(c) = &mut self.csv {
                c.widths.clear();
            }
            self.sync_csv();
            self.measure_widths(0, INITIAL_LINES);
        }
        self.renderer.clear_cache();
        self.after_edit();
    }

    /// ステータスバーの位置表示に加えるレコード番号・列番号。
    pub(crate) fn csv_position(&self) -> Option<String> {
        let c = self.csv.as_ref()?;
        let snap = self.doc.snapshot();
        let head = self.doc.selections().primary().head;
        let start = motion::line_start(snap, head);
        let record = c.view.index().lock().unwrap().record_at(snap, start);
        let field = self.caret_field();
        let mut s = String::from("  [");
        s += &match record {
            Some(r) => format!("レコード {}", group_digits(r + 1)),
            None => "レコード ?".into(),
        };
        if let Some(f) = field {
            s += &format!(", 列 {}", f + 1);
        }
        s += "]";
        Some(s)
    }
}

impl App {
    /// 列見出し（A, B, …）の高さ（ピクセル）。区切り文字モードでなければ 0。
    pub(crate) fn column_header_height(&self) -> i32 {
        if self.csv.is_none() || self.hex.is_some() || self.code.is_some() {
            return 0;
        }
        let lh = self.renderer.metrics().line_height;
        (lh / self.renderer.px_to_dip(1.0)).ceil() as i32 + 2
    }

    /// 列見出しの高さが変わったら（区切り文字モードの切り替え・拡大など）配置し直す。
    pub(crate) fn sync_column_header(&mut self) {
        if self.column_header_height() != self.colhead_h {
            self.layout_children();
        }
    }

    /// 列見出しを描く。各列の上に Excel と同じ規則の列名を、列の幅の中央に表示する。
    /// 表示中の行の区切り文字（" │ "）の中央の x 座標（本文の左端からの DIP。左の列から順に）。
    /// 区切り文字のいちばん多い行を、描画と同じレイアウトで測る。
    fn measured_delimiters(&mut self) -> Vec<f32> {
        use yy_layout::SpanKind;
        let rows = self.cached_rows(self.vp.top, self.page_rows() + 1);
        // 前の行から続くフィールドの行（列揃えの空白で始まる）は、途中の列から始まるので使わない
        let best = rows
            .iter()
            .filter(|r| r.spans.first().is_none_or(|s| s.kind != SpanKind::Pad))
            .max_by_key(|r| r.spans.iter().filter(|s| s.kind == SpanKind::Delim).count());
        let Some(row) = best.cloned() else {
            return Vec::new();
        };
        let delims: Vec<_> = row
            .spans
            .iter()
            .filter(|s| s.kind == SpanKind::Delim)
            .map(|s| s.range.clone())
            .collect();
        delims
            .into_iter()
            .map(|r| {
                (self.renderer.text_x(&row, r.start) + self.renderer.text_x(&row, r.end)) / 2.0
            })
            .collect()
    }

    fn paint_column_header(&mut self, hdc: windows::Win32::Graphics::Gdi::HDC, rc: RECT) {
        use windows::Win32::Graphics::Gdi::*;
        let cref = |c: yy_config::Color| {
            windows::Win32::Foundation::COLORREF(
                c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16,
            )
        };
        let measured = if self.rows_cfg.cells.is_some() {
            self.measured_delimiters()
        } else {
            Vec::new()
        };
        let colors = &self.config.colors;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        unsafe {
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(mem, bmp.into());
            let old_font = SelectObject(mem, self.ui_font.into());
            let fill = |r: &RECT, c: yy_config::Color| {
                let b = CreateSolidBrush(cref(c));
                FillRect(mem, r, b);
                let _ = DeleteObject(b.into());
            };
            fill(&rc, colors.gutter_background);
            SetBkMode(mem, TRANSPARENT);
            SetTextColor(mem, cref(colors.line_number));
            if let Some(cl) = self.rows_cfg.cells.clone() {
                let scale = 1.0 / self.renderer.px_to_dip(1.0);
                let cw = self.renderer.metrics().char_width;
                let origin = self.text_origin_x();
                let x_of = |dip: f32| ((origin + dip - self.scroll_x) * scale).round() as i32;
                let left_edge = (origin * scale).round() as i32;
                // 列の境界（区切り文字 " │ " の中央）は、表示中の行を描画と同じレイアウトで測る。
                // 測った行より右の列は、列の幅と区切り文字の幅から求める
                let delim = self.renderer.text_width(yy_layout::cells::DELIM_TEXT);
                let delim = if delim > 0.0 {
                    delim
                } else {
                    cl.delim_cols() as f32 * cw
                };
                let current = self.caret_field();
                // 最後の列（後ろに区切り文字がない）の幅は測っていないので、名前の分だけにする
                let n = cl.widths.len().max(measured.len()) as u32 + 1;
                let mut prev_mid: Option<f32> = None; // 前の列の右の境界（DIP）
                for f in 0..n {
                    let x0 = prev_mid.map_or(0.0, |m| m + delim / 2.0); // 列の左端
                    let mid = match (measured.get(f as usize), cl.widths.get(f as usize)) {
                        (Some(&m), _) => m,
                        (None, Some(&w)) => x0 + w as f32 * cw + delim / 2.0,
                        (None, None) => x0 + (yy_core::csv::column_name(f).len() + 2) as f32 * cw,
                    };
                    let left = match prev_mid {
                        None => x_of(0.0) - 2,
                        Some(m) => x_of(m),
                    };
                    let right = x_of(mid);
                    prev_mid = Some(mid);
                    if right < left_edge {
                        continue;
                    }
                    if left > w {
                        break;
                    }
                    let mut cell = RECT {
                        left: left.max(left_edge),
                        top: 0,
                        right,
                        bottom: h - 1,
                    };
                    if current == Some(f) {
                        fill(&cell, colors.selection);
                    }
                    let mut name: Vec<u16> = yy_core::csv::column_name(f).encode_utf16().collect();
                    DrawTextW(
                        mem,
                        &mut name,
                        &mut cell,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    // 列の境界の縦線
                    fill(
                        &RECT {
                            left: right,
                            top: 2,
                            right: right + 1,
                            bottom: h - 3,
                        },
                        colors.line_number,
                    );
                }
            }
            // 下の境界線
            fill(
                &RECT {
                    left: 0,
                    top: h - 1,
                    right: w,
                    bottom: h,
                },
                colors.line_number,
            );
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            SelectObject(mem, old_font);
            SelectObject(mem, old_bmp);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
        }
    }
}

/// 列見出しのウィンドウプロシージャ。
pub(crate) extern "system" fn colhead_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::Graphics::Gdi::*;
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if with_app(|a| a.paint_column_header(hdc, rc)).is_none() {
                FillRect(hdc, &rc, GetSysColorBrush(COLOR_BTNFACE));
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_ERASEBKGND => LRESULT(1),
        _ => default_proc(hwnd, msg, wparam, lparam),
    }
}
