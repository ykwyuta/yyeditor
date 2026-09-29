use yy_buffer::Snapshot;

use crate::{RowConfig, next_row_start, prev_row_start, row_containing};

/// 縦方向の表示位置。先頭に表示する表示行の開始位置で表す。
///
/// 行番号ではなくバイト位置で持つため、行数が未確定の巨大ファイルでも
/// 任意の位置へ即座に移動できる。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub top: u64,
}

impl Viewport {
    /// 最後の表示行が画面の最下段に来るときの `top`。
    pub fn last_page_top(snap: &Snapshot, cfg: &RowConfig, page_rows: usize) -> u64 {
        let mut top = row_containing(snap, cfg, snap.len());
        for _ in 1..page_rows.max(1) {
            match prev_row_start(snap, cfg, top) {
                Some(p) => top = p,
                None => break,
            }
        }
        top
    }

    /// `top` を有効な表示行の先頭に補正し、最終ページより後ろに行かないようにする。
    pub fn clamp(&mut self, snap: &Snapshot, cfg: &RowConfig, page_rows: usize) {
        self.top = row_containing(snap, cfg, self.top);
        let last = Viewport::last_page_top(snap, cfg, page_rows);
        if self.top > last {
            self.top = last;
        }
    }

    /// 表示行単位でスクロールする。位置が変わったら `true`。
    pub fn scroll_rows(
        &mut self,
        snap: &Snapshot,
        cfg: &RowConfig,
        delta: i64,
        page_rows: usize,
    ) -> bool {
        let old = self.top;
        if delta > 0 {
            let last = Viewport::last_page_top(snap, cfg, page_rows);
            for _ in 0..delta {
                if self.top >= last {
                    break;
                }
                match next_row_start(snap, cfg, self.top) {
                    Some(n) => self.top = n.min(last),
                    None => break,
                }
            }
        } else {
            for _ in 0..delta.unsigned_abs() {
                match prev_row_start(snap, cfg, self.top) {
                    Some(p) => self.top = p,
                    None => break,
                }
            }
        }
        self.top != old
    }

    /// `offset` を含む表示行が先頭に来るように移動する。
    pub fn scroll_to_offset(
        &mut self,
        snap: &Snapshot,
        cfg: &RowConfig,
        offset: u64,
        page_rows: usize,
    ) {
        self.top = offset;
        self.clamp(snap, cfg, page_rows);
    }

    /// `offset` を含む表示行が画面内（先頭から `page_rows` 行）に入るようにスクロールする。
    /// 動かしたら `true`。
    pub fn ensure_visible(
        &mut self,
        snap: &Snapshot,
        cfg: &RowConfig,
        offset: u64,
        page_rows: usize,
    ) -> bool {
        let row = row_containing(snap, cfg, offset);
        if row < self.top {
            self.top = row;
            return true;
        }
        let mut r = self.top;
        for _ in 0..page_rows.max(1) {
            if r == row {
                return false;
            }
            match next_row_start(snap, cfg, r) {
                Some(n) if n <= row => r = n,
                _ => break,
            }
        }
        let mut t = row;
        for _ in 1..page_rows.max(1) {
            match prev_row_start(snap, cfg, t) {
                Some(p) => t = p,
                None => break,
            }
        }
        let moved = t != self.top;
        self.top = t;
        moved
    }

    /// 文書全体に対する位置（0.0〜1.0、バイト位置比例）。
    pub fn fraction(&self, snap: &Snapshot) -> f64 {
        if snap.is_empty() {
            0.0
        } else {
            self.top as f64 / snap.len() as f64
        }
    }

    /// バイト位置比例の位置 `f` に移動する（スクロールバーのドラッグ用）。
    pub fn scroll_to_fraction(
        &mut self,
        snap: &Snapshot,
        cfg: &RowConfig,
        f: f64,
        page_rows: usize,
    ) {
        let off = (f.clamp(0.0, 1.0) * snap.len() as f64) as u64;
        self.scroll_to_offset(snap, cfg, off, page_rows);
    }
}
