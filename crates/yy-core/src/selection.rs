//! 選択モデル（09 章 2）。
//!
//! 単一カーソルは要素数 1 のマルチ選択として扱い、すべての編集コマンドは
//! カーソル数に関係なく同じ経路で処理する。矩形選択（M2.5）は [`SelectionSet`] に
//! バリアントを追加して実装する。

use std::ops::Range;

/// 1 つの選択範囲（またはカーソル）。位置は文書内のバイトオフセット。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    /// 選択の固定端
    pub anchor: u64,
    /// カーソル位置（移動端）
    pub head: u64,
    /// 上下移動で維持する水平位置（DIP）。左右移動や編集で `None` に戻る
    pub goal_x: Option<f32>,
}

impl Selection {
    pub fn caret(at: u64) -> Selection {
        Selection {
            anchor: at,
            head: at,
            goal_x: None,
        }
    }

    pub fn new(anchor: u64, head: u64) -> Selection {
        Selection {
            anchor,
            head,
            goal_x: None,
        }
    }

    pub fn start(&self) -> u64 {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> u64 {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<u64> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// 選択範囲の向きを保ったまま位置を変換する。
    pub(crate) fn map(&self, f: impl Fn(u64) -> u64) -> Selection {
        Selection {
            anchor: f(self.anchor),
            head: f(self.head),
            goal_x: self.goal_x,
        }
    }
}

/// 文書の選択状態。
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionSet {
    /// オフセット昇順、互いに重ならない
    sels: Vec<Selection>,
    /// 主カーソル（IME・スクロール追従・ステータス表示の対象）
    primary: usize,
}

impl Default for SelectionSet {
    fn default() -> Self {
        SelectionSet::single(Selection::caret(0))
    }
}

impl SelectionSet {
    pub fn single(sel: Selection) -> SelectionSet {
        SelectionSet {
            sels: vec![sel],
            primary: 0,
        }
    }

    /// 選択の列から作る。並べ替え・重なりのマージを行う。`primary` は `sels` 内の添字。
    pub fn from_vec(sels: Vec<Selection>, primary: usize) -> SelectionSet {
        assert!(!sels.is_empty(), "selection set must not be empty");
        let primary = primary.min(sels.len() - 1);
        let mut set = SelectionSet { sels, primary };
        set.normalize();
        set
    }

    pub fn iter(&self) -> impl Iterator<Item = &Selection> {
        self.sels.iter()
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.sels
    }

    pub fn len(&self) -> usize {
        self.sels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sels.is_empty()
    }

    pub fn primary(&self) -> &Selection {
        &self.sels[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// 選択範囲を持つものが 1 つもないか。
    pub fn all_empty(&self) -> bool {
        self.sels.iter().all(Selection::is_empty)
    }

    /// 各選択を `f` で変換し、並べ替えて重なりをマージする。
    pub fn map(&self, mut f: impl FnMut(&Selection) -> Selection) -> SelectionSet {
        SelectionSet::from_vec(self.sels.iter().map(&mut f).collect(), self.primary)
    }

    /// カーソルを追加する。
    pub fn add(&mut self, sel: Selection) {
        self.sels.push(sel);
        self.primary = self.sels.len() - 1;
        self.normalize();
    }

    /// 主カーソルだけを残す。
    pub fn collapse_to_primary(&mut self) {
        let p = self.sels[self.primary];
        self.sels = vec![p];
        self.primary = 0;
    }

    /// すべての位置を `max` 以下に丸める。
    pub fn clamp(&mut self, max: u64) {
        for s in &mut self.sels {
            s.anchor = s.anchor.min(max);
            s.head = s.head.min(max);
        }
        self.normalize();
    }

    /// 昇順に並べ、重なる（または同じ位置の）選択を 1 つにまとめる。
    fn normalize(&mut self) {
        let primary_sel = self.sels[self.primary];
        let mut indexed: Vec<(usize, Selection)> = self.sels.iter().copied().enumerate().collect();
        indexed.sort_by_key(|(_, s)| (s.start(), s.end()));
        let mut out: Vec<Selection> = Vec::with_capacity(indexed.len());
        let mut primary = 0;
        for (i, s) in indexed {
            if let Some(last) = out.last_mut() {
                let overlaps = s.start() < last.end()
                    || (s.start() == last.end() && (s.is_empty() || last.is_empty()));
                if overlaps {
                    // 重なった選択は 1 つにまとめる（向きは後ろ側を優先）
                    let start = last.start().min(s.start());
                    let end = last.end().max(s.end());
                    let forward = s.head >= s.anchor;
                    *last = if forward {
                        Selection::new(start, end)
                    } else {
                        Selection::new(end, start)
                    };
                    if i == self.primary {
                        primary = out.len() - 1;
                    }
                    continue;
                }
            }
            if i == self.primary {
                primary = out.len();
            }
            out.push(s);
        }
        self.sels = out;
        self.primary = primary;
        // 主カーソルが単独で残っている場合は goal_x 等を保持する
        if self.sels[self.primary].range() == primary_sel.range() {
            self.sels[self.primary] = primary_sel;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlapping_and_duplicate_carets() {
        let set = SelectionSet::from_vec(
            vec![
                Selection::caret(10),
                Selection::new(0, 5),
                Selection::new(4, 8),
                Selection::caret(10),
                Selection::caret(20),
            ],
            4,
        );
        let ranges: Vec<_> = set.iter().map(Selection::range).collect();
        assert_eq!(ranges, vec![0..8, 10..10, 20..20]);
        assert_eq!(set.primary().head, 20);
    }

    #[test]
    fn adjacent_ranges_stay_separate() {
        let set = SelectionSet::from_vec(vec![Selection::new(0, 3), Selection::new(3, 6)], 0);
        assert_eq!(set.len(), 2);
    }
}
