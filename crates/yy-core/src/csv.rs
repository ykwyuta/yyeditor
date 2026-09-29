//! 区切り文字モード（04 章）: レコードインデックスの維持とレコード単位の書き直し。

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yy_buffer::Snapshot;
use yy_delimited::{Dialect, RecordIndex, RecordReader, common_prefix, unquote, write_record};
use yy_jobs::{JobHandle, JobPool, Notifier};
use yy_search::ReplaceError;

/// この大きさまでの残りはその場で読む
const SYNC_INDEX_BYTES: u64 = 16 << 20;
/// バックグラウンドで 1 回にロックを持って読む量
const STEP_BYTES: u64 = 4 << 20;

/// 文書の区切り文字モードの状態（方言とレコードインデックス）。
pub struct CsvView {
    pub dialect: Dialect,
    index: Arc<Mutex<RecordIndex>>,
    /// インデックスが対応している内容
    indexed: Snapshot,
    job: Option<JobHandle>,
}

impl CsvView {
    pub fn new(dialect: Dialect) -> CsvView {
        CsvView {
            dialect,
            index: Arc::new(Mutex::new(RecordIndex::new(dialect))),
            indexed: Snapshot::empty(),
            job: None,
        }
    }

    pub fn index(&self) -> Arc<Mutex<RecordIndex>> {
        self.index.clone()
    }

    /// インデックスを作り終えたか。
    pub fn is_complete(&self) -> bool {
        self.index.lock().unwrap().is_complete()
    }

    /// 読み終えた割合（0.0〜1.0）。
    pub fn progress(&self) -> f64 {
        let len = self.indexed.len().max(1);
        self.index.lock().unwrap().scanned() as f64 / len as f64
    }

    /// 文書の内容 `snap` に合わせる。変わっていれば、変わっていない先頭部分から読み直す
    /// （残りが少なければその場で、多ければバックグラウンドで）。
    pub fn sync(&mut self, snap: &Snapshot, pool: &JobPool, notify: Notifier) {
        let same =
            snap.len() == self.indexed.len() && common_prefix(snap, &self.indexed) == snap.len();
        if same && (self.job.is_some() || self.is_complete()) {
            return;
        }
        if let Some(j) = self.job.take() {
            j.cancel();
        }
        let prefix = common_prefix(snap, &self.indexed);
        let mut idx = self.index.lock().unwrap();
        idx.truncate(prefix);
        self.indexed = snap.clone();
        if snap.len() - idx.scanned().min(snap.len()) <= SYNC_INDEX_BYTES {
            idx.extend(snap, u64::MAX);
            return;
        }
        drop(idx);
        let index = self.index.clone();
        let snap = snap.clone();
        self.job = Some(pool.spawn(move |ctx| {
            ctx.progress.set_total(snap.len());
            let mut last = Instant::now();
            loop {
                let (done, pos) = {
                    let mut idx = index.lock().unwrap();
                    if ctx.cancel.is_cancelled() {
                        return;
                    }
                    let done = idx.extend(&snap, STEP_BYTES);
                    (done, idx.scanned())
                };
                ctx.progress.set_done(pos);
                if done || last.elapsed() >= Duration::from_millis(200) {
                    notify();
                    last = Instant::now();
                }
                if done {
                    return;
                }
            }
        }));
    }

    /// バックグラウンドの作成が終わっていれば片付けて `true`。
    pub fn poll(&mut self) -> bool {
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.job = None;
            return true;
        }
        false
    }
}

impl Drop for CsvView {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel();
        }
    }
}

/// レコード単位の書き直し。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordOp {
    /// 列（フィールド番号）を削除する
    DeleteField(u32),
    /// 列の前に空の列を挿入する（フィールドがそれより少ないレコードはそのまま）
    InsertField(u32),
    /// 別の方言（区切り文字）に変換する
    Convert(Dialect),
}

/// 文書全体をレコードごとに書き直して `w` に書き出す。書き直したレコード数を返す。
pub(crate) fn rewrite_records(
    snap: &Snapshot,
    d: Dialect,
    op: RecordOp,
    w: &mut dyn Write,
    step: &mut dyn FnMut(u64) -> bool,
) -> Result<u64, ReplaceError> {
    let mut n = 0u64;
    let mut out = Vec::with_capacity(1 << 16);
    let mut last_step = 0u64;
    let mut reader = RecordReader::new(snap, d, 0, snap.len());
    for rec in reader.by_ref() {
        let blank = rec.fields.len() == 1 && rec.fields[0].is_empty();
        if blank {
            // 空行はそのまま
            out.extend_from_slice(&rec.terminator);
        } else {
            match op {
                RecordOp::Convert(to) => {
                    let values: Vec<Vec<u8>> = rec.fields.iter().map(|f| unquote(f, &d)).collect();
                    write_record(&values, &to, &rec.terminator, &mut out);
                }
                RecordOp::DeleteField(k) | RecordOp::InsertField(k) => {
                    let mut fields = rec.fields;
                    let k = k as usize;
                    match op {
                        RecordOp::DeleteField(_) if k < fields.len() && fields.len() > 1 => {
                            fields.remove(k);
                        }
                        RecordOp::InsertField(_) if k <= fields.len() => {
                            fields.insert(k, Vec::new());
                        }
                        _ => {}
                    }
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            out.extend_from_slice(d.delimiter());
                        }
                        out.extend_from_slice(f);
                    }
                    out.extend_from_slice(&rec.terminator);
                }
            }
            n += 1;
        }
        if out.len() >= 1 << 16 {
            w.write_all(&out)?;
            out.clear();
        }
        if rec.range.end - last_step >= 4 << 20 {
            last_step = rec.range.end;
            if !step(last_step) {
                return Err(ReplaceError::Cancelled);
            }
        }
    }
    w.write_all(&out)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, d: Dialect, op: RecordOp) -> String {
        let s = Snapshot::from_bytes(text.as_bytes().to_vec());
        let mut out = Vec::new();
        rewrite_records(&s, d, op, &mut out, &mut |_| true).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn converts_between_dialects() {
        let csv = "id,text\r\n1,\"a,b\"\r\n\r\n2,\"tab\there\"\r\n3,\"x\ny\"";
        assert_eq!(
            run(csv, Dialect::csv(), RecordOp::Convert(Dialect::tsv())),
            "id\ttext\r\n1\ta,b\r\n\r\n2\t\"tab\there\"\r\n3\t\"x\ny\""
        );
        let back = run(
            &run(csv, Dialect::csv(), RecordOp::Convert(Dialect::tsv())),
            Dialect::tsv(),
            RecordOp::Convert(Dialect::csv()),
        );
        // 引用符は必要なものだけになる
        assert_eq!(back, csv.replace("\"tab\there\"", "tab\there"));
    }

    #[test]
    fn inserts_and_deletes_columns() {
        let csv = "a,b,c\n1,\"x\ny\",3\nonly\n";
        assert_eq!(
            run(csv, Dialect::csv(), RecordOp::DeleteField(1)),
            "a,c\n1,3\nonly\n"
        );
        assert_eq!(
            run(csv, Dialect::csv(), RecordOp::InsertField(1)),
            "a,,b,c\n1,,\"x\ny\",3\nonly,\n"
        );
    }
}
