//! 複数カーソルでの一括編集を、カーソルごとに後ろから単独で編集する素朴な実装と比較する
//! （09 章 8）。あわせて Undo / Redo で各時点の内容に正確に戻ることを確認する。

use std::collections::BTreeSet;

use proptest::prelude::*;
use yy_core::{Document, Selection, SelectionSet};

#[derive(Debug, Clone)]
enum Op {
    Type(String),
    Backspace,
    Delete,
    Paste(String),
    MoveTo(Vec<f64>),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => "[a-z]{1,3}".prop_map(Op::Type),
        2 => Just(Op::Backspace),
        2 => Just(Op::Delete),
        1 => "[a-z\n]{0,5}".prop_map(Op::Paste),
        2 => prop::collection::vec(0.0..=1.0f64, 1..5).prop_map(Op::MoveTo),
    ]
}

/// 素朴な実装: キャレット位置の集合とテキスト。
struct Oracle {
    text: Vec<u8>,
    carets: BTreeSet<usize>,
}

impl Oracle {
    /// 各キャレットで `range(p)` を `ins` に置き換える（後ろから適用）。
    fn edit(&mut self, range: impl Fn(&[u8], usize) -> (usize, usize), ins: &[u8]) {
        let carets: Vec<usize> = self.carets.iter().copied().collect();
        let ranges: Vec<(usize, usize)> = carets.iter().map(|&p| range(&self.text, p)).collect();
        let mut new_carets = Vec::new();
        let mut shift: isize = 0;
        for &(s, e) in &ranges {
            new_carets.push((s as isize + shift) as usize + ins.len());
            shift += ins.len() as isize - (e - s) as isize;
        }
        for &(s, e) in ranges.iter().rev() {
            self.text.splice(s..e, ins.iter().copied());
        }
        self.carets = new_carets.into_iter().collect();
    }
}

fn doc_text(d: &Document) -> Vec<u8> {
    d.snapshot().read(0..d.snapshot().len())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn multi_cursor_edits_match_oracle(
        initial in "[a-z\n]{0,60}",
        start in prop::collection::vec(0.0..=1.0f64, 1..5),
        ops in prop::collection::vec(op(), 1..25),
    ) {
        let mut d = Document::from_text(&initial);
        let mut o = Oracle { text: initial.as_bytes().to_vec(), carets: BTreeSet::new() };
        let place = |d: &mut Document, o: &mut Oracle, fr: &[f64]| {
            let len = o.text.len();
            o.carets = fr.iter().map(|f| (f * len as f64) as usize).collect();
            let sels: Vec<_> = o.carets.iter().map(|&p| Selection::caret(p as u64)).collect();
            d.set_selections(SelectionSet::from_vec(sels, 0));
        };
        place(&mut d, &mut o, &start);
        let mut states = vec![doc_text(&d)];

        for op in ops {
            let changed = match &op {
                Op::Type(t) => {
                    o.edit(|_, p| (p, p), t.as_bytes());
                    d.insert_text(t, false)
                }
                // この素朴な実装は 1 バイト = 1 文字を前提とするため、CRLF（2 バイトで 1 文字）を
                // 挿入する操作は除く（CRLF の扱いは lib.rs の単体テストで確認している）
                Op::Paste(t) if t.contains('\n') && d.eol() == yy_core::Eol::CrLf => continue,
                Op::Paste(t) if o.carets.len() == 1 || t.is_empty() => {
                    // 貼り付けた改行は文書の改行コードになる（改行のない文書では OS の既定値）
                    let eol = d.eol().as_bytes();
                    let expected: Vec<u8> = t
                        .bytes()
                        .flat_map(|b| if b == b'\n' { eol.to_vec() } else { vec![b] })
                        .collect();
                    o.edit(|_, p| (p, p), &expected);
                    d.paste(t)
                }
                Op::Paste(_) => continue,
                Op::Backspace => {
                    let before = o.text.clone();
                    o.edit(|_, p| (p.saturating_sub(1), p), b"");
                    let changed = d.delete_backward();
                    prop_assert_eq!(changed, before != o.text);
                    changed
                }
                Op::Delete => {
                    let before = o.text.clone();
                    o.edit(|t, p| (p, (p + 1).min(t.len())), b"");
                    let changed = d.delete_forward();
                    prop_assert_eq!(changed, before != o.text);
                    changed
                }
                Op::MoveTo(fr) => {
                    place(&mut d, &mut o, fr);
                    false
                }
            };
            prop_assert_eq!(doc_text(&d), o.text.clone(), "after {:?}", op);
            let carets: BTreeSet<usize> = d.selections().iter().map(|s| s.head as usize).collect();
            prop_assert_eq!(&carets, &o.carets, "carets after {:?}", op);
            d.snapshot().check_invariants();
            if changed {
                states.push(doc_text(&d));
            }
        }

        // Undo で過去の状態を順に辿り、最後は初期状態に戻る
        let final_text = doc_text(&d);
        let mut seen = vec![doc_text(&d)];
        while d.undo() {
            seen.push(doc_text(&d));
        }
        prop_assert_eq!(seen.last().unwrap(), &states[0]);
        prop_assert!(!d.is_modified());
        for s in &seen {
            prop_assert!(states.contains(s), "undo produced an unknown state");
        }
        while d.redo() {}
        prop_assert_eq!(doc_text(&d), final_text);
    }
}
