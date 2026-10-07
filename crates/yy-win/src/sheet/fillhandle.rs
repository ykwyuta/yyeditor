//! フィルハンドル（15 章 12.4）: 選択範囲の右下の小さな四角をドラッグして、連続データ・コピーを
//! 入れる（離すときに Ctrl を押していると、連続データとコピーを入れ替える）。範囲の中へ戻すと外れた
//! セルを消す。ダブルクリックで、隣の列の値の終わりまで下へ広げる。中身は [`yy_sheet::autofill`]。

use yy_sheet::autofill::Filled;

use super::paint::{HANDLE, Range4};
use super::*;

impl App {
    /// フィルハンドルを出すか（編集中・列全体・行全体の選択では出さない）。
    pub(super) fn handle_shown(&self) -> bool {
        self.editor.is_none()
            && self.whole == (false, false)
            && !matches!(self.drag, Some(Drag::Point))
    }

    /// 位置（px）がフィルハンドルの上か。
    pub(super) fn on_handle(&self, x: i32, y: i32) -> bool {
        if !self.handle_shown() {
            return false;
        }
        let (_, _, b, r) = self.selection();
        let (Some(&(_, cx, cw)), Some(&(_, ry))) = (
            self.cols.iter().find(|c| c.0 == r),
            self.rows.iter().find(|w| w.0 == b),
        ) else {
            return false;
        };
        let (hx, hy) = (
            self.header_w + cx + cw,
            self.painter.row_h + ry + self.painter.row_h,
        );
        let (xd, yd) = (self.painter.to_dip(x), self.painter.to_dip(y));
        (xd - hx).abs() <= HANDLE * 0.7 && (yd - hy).abs() <= HANDLE * 0.7
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

    /// フィルハンドルを押した。ダブルクリックなら下へ広げる。扱ったら `true`。
    pub(super) fn handle_down(&mut self, x: i32, y: i32, double: bool, ctrl: bool) -> bool {
        if !self.on_handle(x, y) {
            return false;
        }
        let sel = self.selection();
        if double {
            self.drag = None;
            match self.sheet().fill_down_end(&self.ctx, sel) {
                Some(end) => self.do_fill((sel.0, sel.1, end, sel.3), ctrl),
                None => {
                    set_status("下へ広げる先が見つかりません（左か右の列に値が続いていません）")
                }
            }
            return true;
        }
        self.drag = Some(Drag::Fill(sel));
        true
    }

    /// フィルハンドルのドラッグ。
    pub(super) fn handle_drag(&mut self, x: i32, y: i32) {
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
        if self.drag.map(|d| matches!(d, Drag::Fill(t) if t == target)) != Some(true) {
            self.drag = Some(Drag::Fill(target));
            self.reveal((r, c));
            self.invalidate();
            let (t, l, b, rr) = target;
            set_status(&format!(
                "{}{}:{}{} まで（離すときに Ctrl で連続データ・コピーを入れ替え）",
                yy_sheet::col_name(l),
                t + 1,
                yy_sheet::col_name(rr),
                b + 1
            ));
        }
    }

    /// 広げる（縮める）。
    pub(super) fn do_fill(&mut self, dst: Range4, ctrl: bool) {
        let src = self.selection();
        if dst == src {
            set_status("準備完了");
            return;
        }
        let sheet = self.sheet;
        let mut filled = Filled::Nothing;
        let res = self.doc.edit(|bk, ctx| {
            filled = bk.sheets[sheet]
                .autofill(ctx, src, dst, ctrl)
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
        self.after_edit();
        set_status(match filled {
            Filled::Series => "連続データを入れました（Ctrl を押しながら離すとコピー）",
            Filled::Copy => "セルをコピーしました（数値は Ctrl を押しながら離すと連続データ）",
            Filled::Cleared => "範囲から外したセルを消しました",
            Filled::Nothing => "準備完了",
        });
    }
}
