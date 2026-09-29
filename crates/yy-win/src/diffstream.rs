//! Bounded-memory, line-oriented comparison and a seekable disk index.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions, remove_file};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use yy_buffer::{Slice, Snapshot};

const LOOKAHEAD: usize = 32;
pub const RECORD_SIZE: u64 = 49;
pub const NONE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Equal = 0,
    Changed = 1,
    LeftOnly = 2,
    RightOnly = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub left: u64,
    pub right: u64,
    pub left_line: u64,
    pub right_line: u64,
    pub left_len: u64,
    pub right_len: u64,
    pub kind: Kind,
}

impl Row {
    fn new(left: Option<Line>, right: Option<Line>, kind: Kind) -> Self {
        Self {
            left: left.map_or(NONE, |x| x.start),
            right: right.map_or(NONE, |x| x.start),
            left_line: left.map_or(NONE, |x| x.number),
            right_line: right.map_or(NONE, |x| x.number),
            left_len: left.map_or(0, |x| x.len),
            right_len: right.map_or(0, |x| x.len),
            kind,
        }
    }

    fn bytes(self) -> [u8; RECORD_SIZE as usize] {
        let mut b = [0; RECORD_SIZE as usize];
        for (i, n) in [
            self.left,
            self.right,
            self.left_line,
            self.right_line,
            self.left_len,
            self.right_len,
        ]
        .into_iter()
        .enumerate()
        {
            b[i * 8..i * 8 + 8].copy_from_slice(&n.to_le_bytes());
        }
        b[48] = self.kind as u8;
        b
    }

    fn from_bytes(b: &[u8; RECORD_SIZE as usize]) -> io::Result<Self> {
        let number = |i| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let kind = match b[48] {
            0 => Kind::Equal,
            1 => Kind::Changed,
            2 => Kind::LeftOnly,
            3 => Kind::RightOnly,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid diff index",
                ));
            }
        };
        Ok(Self {
            left: number(0),
            right: number(8),
            left_line: number(16),
            right_line: number(24),
            left_len: number(32),
            right_len: number(40),
            kind,
        })
    }
}

pub struct Index {
    pub path: PathBuf,
    pub rows: u64,
    pub changes: u64,
    pub max_line_bytes: u64,
}

impl Drop for Index {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
    }
}

impl Index {
    pub fn open(&self) -> io::Result<BufReader<File>> {
        Ok(BufReader::new(File::open(&self.path)?))
    }

    pub fn read_rows(
        &self,
        file: &mut BufReader<File>,
        start: u64,
        count: usize,
    ) -> io::Result<Vec<Row>> {
        file.seek(SeekFrom::Start(start * RECORD_SIZE))?;
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count.min(self.rows.saturating_sub(start) as usize) {
            let mut b = [0; RECORD_SIZE as usize];
            file.read_exact(&mut b)?;
            rows.push(Row::from_bytes(&b)?);
        }
        Ok(rows)
    }
}

#[derive(Clone, Copy)]
struct Line {
    start: u64,
    number: u64,
    len: u64,
    hash: u64,
    hash2: u64,
}

struct Lines<'a> {
    slices: Vec<Slice<'a>>,
    slice: usize,
    byte: usize,
    offset: u64,
    number: u64,
    total: u64,
    pending: VecDeque<Line>,
    cancel: &'a AtomicBool,
}

impl<'a> Lines<'a> {
    fn new(snapshot: &'a Snapshot, cancel: &'a AtomicBool) -> Self {
        Self {
            slices: snapshot.slices(0..snapshot.len()),
            slice: 0,
            byte: 0,
            offset: 0,
            number: 0,
            total: snapshot.len(),
            pending: VecDeque::new(),
            cancel,
        }
    }

    fn next_byte(&mut self) -> Option<u8> {
        while self.slice < self.slices.len() {
            let bytes = self.slices[self.slice].bytes;
            if self.byte < bytes.len() {
                let b = bytes[self.byte];
                self.byte += 1;
                self.offset += 1;
                return Some(b);
            }
            self.slice += 1;
            self.byte = 0;
        }
        None
    }

    fn scan(&mut self) -> Option<Line> {
        if self.offset >= self.total {
            return None;
        }
        let start = self.offset;
        let mut hash = 0xcbf29ce484222325u64;
        let mut hash2 = 0x9e3779b97f4a7c15u64;
        let mut len = 0;
        let mut trailing_cr = 0u64;
        let mut append = |b: u8| {
            hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
            hash2 = hash2.rotate_left(5) ^ (b as u64).wrapping_mul(0x517cc1b727220a95);
            len += 1;
        };
        while let Some(b) = self.next_byte() {
            if self.offset & 0xffff == 0 && self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            if b == b'\n' {
                break;
            }
            if b == b'\r' {
                trailing_cr += 1;
                continue;
            }
            for _ in 0..trailing_cr {
                append(b'\r');
            }
            trailing_cr = 0;
            append(b);
        }
        let line = Line {
            start,
            number: self.number,
            len,
            hash,
            hash2,
        };
        self.number += 1;
        Some(line)
    }

    fn fill(&mut self, count: usize) {
        while self.pending.len() < count {
            match self.scan() {
                Some(line) => self.pending.push_back(line),
                None => break,
            }
        }
    }

    fn first(&mut self) -> Option<Line> {
        self.fill(1);
        self.pending.front().copied()
    }

    fn pop(&mut self) -> Option<Line> {
        self.fill(1);
        self.pending.pop_front()
    }
}

fn same(a: Line, b: Line) -> bool {
    a.len == b.len && a.hash == b.hash && a.hash2 == b.hash2
}

pub fn compare(
    left: &Snapshot,
    right: &Snapshot,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> io::Result<Option<Index>> {
    let path = yy_io::temp_path("diff");
    let result = (|| {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);
        let mut l = Lines::new(left, cancel);
        let mut r = Lines::new(right, cancel);
        let mut rows = 0u64;
        let mut changes = 0u64;
        let mut max_line_bytes = 0u64;
        let mut steps = 0u64;
        let mut emit = |row: Row| -> io::Result<()> {
            writer.write_all(&row.bytes())?;
            rows += 1;
            changes += u64::from(row.kind != Kind::Equal);
            Ok(())
        };
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let a = l.first();
            let b = r.first();
            max_line_bytes = max_line_bytes
                .max(a.map_or(0, |x| x.len))
                .max(b.map_or(0, |x| x.len));
            if steps & 4095 == 0 {
                progress.store(l.offset.saturating_add(r.offset), Ordering::Relaxed);
            }
            steps += 1;
            match (a, b) {
                (None, None) => break,
                (Some(a), None) => {
                    emit(Row::new(Some(a), None, Kind::LeftOnly))?;
                    l.pop();
                }
                (None, Some(b)) => {
                    emit(Row::new(None, Some(b), Kind::RightOnly))?;
                    r.pop();
                }
                (Some(a), Some(b)) if same(a, b) => {
                    emit(Row::new(Some(a), Some(b), Kind::Equal))?;
                    l.pop();
                    r.pop();
                }
                (Some(a), Some(b)) => {
                    l.fill(LOOKAHEAD + 1);
                    r.fill(LOOKAHEAD + 1);
                    let left_anchor = l.pending.iter().skip(1).position(|x| same(*x, b));
                    let right_anchor = r.pending.iter().skip(1).position(|x| same(a, *x));
                    match (left_anchor, right_anchor) {
                        (Some(x), Some(y)) if x <= y => {
                            emit(Row::new(Some(a), None, Kind::LeftOnly))?;
                            l.pop();
                        }
                        (Some(_), None) => {
                            emit(Row::new(Some(a), None, Kind::LeftOnly))?;
                            l.pop();
                        }
                        (Some(_), Some(_)) | (None, Some(_)) => {
                            emit(Row::new(None, Some(b), Kind::RightOnly))?;
                            r.pop();
                        }
                        (None, None) => {
                            emit(Row::new(Some(a), Some(b), Kind::Changed))?;
                            l.pop();
                            r.pop();
                        }
                    }
                }
            }
        }
        progress.store(left.len().saturating_add(right.len()), Ordering::Relaxed);
        writer.flush()?;
        drop(writer);
        Ok(Some(Index {
            path: path.clone(),
            rows,
            changes,
            max_line_bytes,
        }))
    })();
    if !matches!(result, Ok(Some(_))) {
        let _ = remove_file(&path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(l: &[u8], r: &[u8]) -> Vec<Row> {
        let left = Snapshot::from_bytes(l);
        let right = Snapshot::from_bytes(r);
        let index = compare(&left, &right, &AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap()
            .unwrap();
        index.read_rows(&mut index.open().unwrap(), 0, 100).unwrap()
    }

    #[test]
    fn aligns_insertions_and_normalizes_crlf() {
        let rows = run(b"a\r\nb\n", b"a\nc\nb\n");
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [Kind::Equal, Kind::RightOnly, Kind::Equal]
        );
        assert_eq!(rows[2].left_line, 1);
        assert_eq!(rows[2].right_line, 2);
        assert_eq!(run(b"x\r\r\n", b"x\n")[0].kind, Kind::Equal);
    }

    #[test]
    fn handles_empty_and_long_lines() {
        assert!(run(b"", b"").is_empty());
        let mut long = vec![b'x'; 2_000_000];
        let mut other = long.clone();
        other.push(b'\n');
        assert_eq!(run(&long, &other)[0].kind, Kind::Equal);
        long.push(b'y');
        assert_eq!(run(&long, &other)[0].kind, Kind::Changed);
    }

    #[test]
    fn cancelled_comparison_leaves_no_index() {
        let snap = Snapshot::from_bytes(b"a\n");
        assert!(
            compare(&snap, &snap, &AtomicBool::new(true), &AtomicU64::new(0))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn compares_documents_above_previous_size_limit() {
        let mut bytes = vec![b'x'; 18 * 1024 * 1024];
        bytes.push(b'\n');
        let left = Snapshot::from_bytes(bytes);
        let right = left.clone();
        let index = compare(&left, &right, &AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap()
            .unwrap();
        assert_eq!(index.rows, 1);
        assert_eq!(index.changes, 0);
        assert_eq!(
            index.read_rows(&mut index.open().unwrap(), 0, 1).unwrap()[0].kind,
            Kind::Equal
        );
    }

    #[test]
    fn compares_more_than_twenty_thousand_lines() {
        let bytes = b"line\n".repeat(30_000);
        let left = Snapshot::from_bytes(bytes);
        let right = left.clone();
        let index = compare(&left, &right, &AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap()
            .unwrap();
        assert_eq!(index.rows, 30_000);
        assert_eq!(index.changes, 0);
        assert_eq!(
            index
                .read_rows(&mut index.open().unwrap(), 29_999, 1)
                .unwrap()[0]
                .left_line,
            29_999
        );
    }
}
