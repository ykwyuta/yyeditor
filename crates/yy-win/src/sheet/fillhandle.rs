//! フィルハンドル（15 章 12.4）: 選択範囲の右下の小さな四角をドラッグして、連続データ・コピーを
//! 入れる（離すときに Ctrl を押していると、連続データとコピーを入れ替える）。範囲の中へ戻すと外れた
//! セルを消す。ダブルクリックで、隣の列の値の終わりまで下へ広げる。右ボタンでドラッグすると、離したときに
//! 仕方のメニューを出す。フィルのあとは広げた範囲の右下に「オートフィル オプション」のボタンを出し、
//! コピー・連続データ・書式のみ・書式なし・日付の単位（日・週日・月・年）でやり直せる。中身は
//! [`yy_sheet::autofill`]。

use yy_sheet::autofill::{FillMode, FillOptions, Filled};

use super::paint::{HANDLE, Range4};
use super::*;

/// 直前のフィル（オプションのボタンでやり直すため）。
#[derive(Clone, Copy, Debug)]
pub(super) struct LastFill {
    sheet: usize,
    src: Range4,
    dst: Range4,
    opts: FillOptions,
    /// フィルした直後の文書の変更の番号（ほかの編集をしたらボタンを消す）
    generation: u64,
    /// 元に日付がある（日付の単位を出す）
    dates: bool,
}

/// オプションのメニューの項目。
const MENU: [(u16, &str); 8] = [
    (1, "セルのコピー(&C)"),
    (2, "連続データ(&S)"),
    (3, "書式のみコピー (フィル)(&F)"),
    (4, "書式なしコピー (フィル)(&O)"),
    (5, "連続データ (日単位)(&D)"),
    (6, "連続データ (週日単位)(&W)"),
    (7, "連続データ (月単位)(&M)"),
    (8, "連続データ (年単位)(&Y)"),
];

fn options_of(id: u16, base: FillOptions) -> FillOptions {
    let with = |mode| FillOptions {
        mode,
        values: true,
        formats: true,
        ..base
    };
    match id {
        1 => with(FillMode::Copy),
        2 => with(FillMode::Series),
        3 => FillOptions {
            values: false,
            formats: true,
            ..base
        },
        4 => FillOptions {
            values: true,
            formats: false,
            ..base
        },
        5 => with(FillMode::Days),
        6 => with(FillMode::Weekdays),
        7 => with(FillMode::Months),
        _ => with(FillMode::Years),
    }
}

/// いまの仕方に当たる項目（チェックを付ける）。
fn checked_of(o: &FillOptions) -> u16 {
    if !o.values {
        return 3;
    }
    if !o.formats {
        return 4;
    }
    match o.mode {
        FillMode::Copy => 1,
        FillMode::Series => 2,
        FillMode::Days => 5,
        FillMode::Weekdays => 6,
        FillMode::Months => 7,
        FillMode::Years => 8,
        FillMode::Auto => 0,
    }
}

/// オプションのメニューを出す（画面の位置）。選んだ項目。状態を借りずに呼ぶこと。
pub(super) fn options_menu(hwnd: HWND, x: i32, y: i32, dates: bool, checked: u16) -> Option<u16> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for &(id, text) in &MENU {
            if id == 5 {
                if !dates {
                    break;
                }
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            }
            let flags = if id == checked {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(menu, flags, id as usize, &HSTRING::from(text));
        }
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            Some(0),
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        (cmd.0 > 0).then_some(cmd.0 as u16)
    }
}

impl App {
    /// フィルハンドルを出すか（編集中・列全体・行全体の選択・別のシートを参照中は出さない）。
    pub(super) fn handle_shown(&self) -> bool {
        self.editor.is_none()
            && self.home.is_none()
            && self.whole == (false, false)
            && !matches!(self.drag, Some(Drag::Point))
    }

    /// 位置（px）がフィルハンドルの上か。
    pub(super) fn on_handle(&self, x: i32, y: i32) -> bool {
        if !self.handle_shown() {
            return false;
        }
        let (_, _, b, r) = self.selection();
        let Some((hx, hy)) = self.corner_dip(b, r) else {
            return false;
        };
        let (xd, yd) = (self.painter.to_dip(x), self.painter.to_dip(y));
        (xd - hx).abs() <= HANDLE * 0.7 && (yd - hy).abs() <= HANDLE * 0.7
    }

    /// セルの右下の角（DIP。見えていなければ `None`）。
    fn corner_dip(&self, row: u64, col: u32) -> Option<(f32, f32)> {
        let &(_, cx, cw) = self.cols.iter().find(|c| c.0 == col)?;
        let &(_, ry) = self.rows.iter().find(|w| w.0 == row)?;
        Some((
            self.header_w + cx + cw,
            self.painter.row_h + ry + self.painter.row_h,
        ))
    }

    /// 直前のフィルがまだ有効か（ほかの編集・シートの切り替えをしていない）。
    fn last_fill_valid(&self) -> Option<LastFill> {
        self.last_fill
            .filter(|f| f.sheet == self.sheet && f.generation == self.doc.generation())
    }

    /// オプションのボタンの長方形（DIP。左・上・右・下）。
    pub(super) fn fill_button_dip(&self) -> Option<(f32, f32, f32, f32)> {
        if self.editor.is_some() || self.home.is_some() || self.drag.is_some() {
            return None;
        }
        let f = self.last_fill_valid()?;
        let (x, y) = self.corner_dip(f.dst.2, f.dst.3)?;
        let h = self.painter.row_h;
        Some((x + 2.0, y + 2.0, x + 2.0 + h * 1.1, y + 2.0 + h * 0.9))
    }

    fn on_fill_button(&self, x: i32, y: i32) -> bool {
        let Some((l, t, r, b)) = self.fill_button_dip() else {
            return false;
        };
        let (xd, yd) = (self.painter.to_dip(x), self.painter.to_dip(y));
        (l..=r).contains(&xd) && (t..=b).contains(&yd)
    }

    /// 今の仕方（日付の基準を含む）。
    fn fill_base(&self, ctrl: bool) -> FillOptions {
        FillOptions::auto(ctrl, self.sys())
    }

    /// ドラッグしている位置のセルまで広げる先（大きく動いた方向の 1 方向。範囲の中なら縮める）。
    fn fill_target(&self, row: u64, col: u32) -> Range4 {
        let (t, l, b, r) = self.selection();
        let (below, above) = (row.saturating_sub(b), t.saturating_sub(row));
        let (right, left) = (col.saturating_sub(r) as u64, l.saturating_sub(col) as u64);
        let (dv, dh) = (below.max(above), right.max(left));
        if dv == 0 && dh == 0 {
            let up = b - row.clamp(t, b);
            let lf = (r - col.clamp(l, r)) as u64;
            if up == 0 && lf == 0 {
                (t, l, b, r)
            } else if up >= lf {
                (t, l, row.clamp(t, b), r)
            } else {
                (t, l, b, col.clamp(l, r))
            }
        } else if dv >= dh {
            if below > 0 {
                (t, l, row, r)
            } else {
                (row, l, b, r)
            }
        } else if right > 0 {
            (t, l, b, col)
        } else {
            (t, col, b, r)
        }
    }

    /// フィルハンドルを押した（`right` は右ボタン）。ダブルクリックなら下へ広げる。扱ったら `true`。
    pub(super) fn handle_down(
        &mut self,
        x: i32,
        y: i32,
        double: bool,
        ctrl: bool,
        right: bool,
    ) -> bool {
        if !self.on_handle(x, y) {
            return false;
        }
        let sel = self.selection();
        if double {
            self.drag = None;
            match self.sheet().fill_down_end(&self.ctx, sel) {
                Some(end) => self.do_fill((sel.0, sel.1, end, sel.3), self.fill_base(ctrl)),
                None => {
                    set_status("下へ広げる先が見つかりません（左か右の列に値が続いていません）")
                }
            }
            return true;
        }
        self.last_fill = None;
        self.drag = Some(Drag::Fill(sel, right));
        true
    }

    /// フィルハンドルのドラッグ。
    pub(super) fn handle_drag(&mut self, x: i32, y: i32) {
        let Some(Drag::Fill(old, right)) = self.drag else {
            return;
        };
        let (row, col) = self.hit(x, y);
        let r = row.unwrap_or(self.top);
        let c = col.unwrap_or_else(|| {
            if self.painter.to_dip(x) < self.header_w {
                self.left
            } else {
                self.cols.last().map(|c| c.0).unwrap_or(0)
            }
        });
        let target = self.fill_target(r, c);
        if target != old {
            self.drag = Some(Drag::Fill(target, right));
            self.reveal((r, c));
            self.invalidate();
            let (t, l, b, rr) = target;
            set_status(&format!(
                "{}{}:{}{} まで（{}）",
                yy_sheet::col_name(l),
                t + 1,
                yy_sheet::col_name(rr),
                b + 1,
                if right {
                    "離すと仕方を選べます"
                } else {
                    "離すときに Ctrl で連続データ・コピーを入れ替え"
                }
            ));
        }
    }

    /// 左ボタンを離した: 広げる。
    pub(super) fn handle_up(&mut self, ctrl: bool) {
        if let Some(Drag::Fill(target, false)) = self.drag {
            self.drag = None;
            self.do_fill(target, self.fill_base(ctrl));
        }
    }

    /// 右ボタンでのドラッグを終える（広げる先と、日付があるか）。メニューは呼ぶ側で出す。
    pub(super) fn take_right_fill(&mut self) -> Option<(Range4, bool)> {
        let Some(Drag::Fill(target, true)) = self.drag else {
            return None;
        };
        self.drag = None;
        self.invalidate();
        // 動かさなければふつうの右クリックのメニュー
        if target == self.selection() {
            return None;
        }
        let dates = self.sheet().fill_has_dates(&self.ctx, self.selection());
        Some((target, dates))
    }

    /// 右ボタンでのドラッグのメニューで選んだ仕方で広げる。
    pub(super) fn right_fill(&mut self, target: Range4, id: u16) {
        let opts = options_of(id, self.fill_base(false));
        self.do_fill(target, opts);
    }

    /// オプションのボタンを押した: メニューの位置（画面）・日付があるか・今の項目。
    pub(super) fn fill_button_down(&mut self, x: i32, y: i32) -> Option<(POINT, bool, u16)> {
        if !self.on_fill_button(x, y) {
            return None;
        }
        let f = self.last_fill_valid()?;
        let (l, _, _, b) = self.fill_button_dip()?;
        let mut pt = POINT {
            x: self.painter.to_px(l),
            y: self.painter.to_px(b),
        };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.grid, &mut pt);
        }
        Some((pt, f.dates, checked_of(&f.opts)))
    }

    /// オプションのメニューで選んだ仕方でやり直す（直前のフィルを元に戻してから）。
    pub(super) fn refill(&mut self, id: u16) {
        let Some(f) = self.last_fill_valid() else {
            return;
        };
        let opts = options_of(id, f.opts);
        if opts == f.opts {
            return;
        }
        self.doc.undo();
        self.anchor = (f.src.0, f.src.1);
        self.cur = (f.src.2, f.src.3);
        self.do_fill(f.dst, opts);
    }

    /// 広げる（縮める）。
    pub(super) fn do_fill(&mut self, dst: Range4, opts: FillOptions) {
        let src = self.selection();
        if dst == src {
            set_status("準備完了");
            return;
        }
        let sheet = self.sheet;
        let dates = self.sheet().fill_has_dates(&self.ctx, src);
        let mut filled = Filled::Nothing;
        let res = self.doc.edit(|bk, ctx| {
            filled = bk.sheets[sheet]
                .autofill(ctx, src, dst, &opts)
                .map_err(std::io::Error::other)?;
            Ok(())
        });
        if let Err(e) = res {
            error_box(self.frame, &format!("フィルできませんでした: {e}"));
            self.after_edit();
            return;
        }
        self.anchor = (dst.0, dst.1);
        self.cur = (dst.2, dst.3);
        self.last_fill = matches!(filled, Filled::Series | Filled::Copy).then(|| LastFill {
            sheet,
            src,
            dst,
            opts,
            generation: self.doc.generation(),
            dates,
        });
        self.after_edit();
        set_status(match filled {
            Filled::Series if opts.mode == FillMode::Auto => {
                "連続データを入れました（右下のボタンでコピー・書式のみなどに変えられます）"
            }
            Filled::Copy if opts.mode == FillMode::Auto && opts.values && opts.formats => {
                "セルをコピーしました（右下のボタンで連続データなどに変えられます）"
            }
            Filled::Series | Filled::Copy => "フィルしました",
            Filled::Cleared => "範囲から外したセルを消しました",
            Filled::Nothing => "準備完了",
        });
    }
}
