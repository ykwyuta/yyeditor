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
    change_path: PathBuf,
    pub rows: u64,
    pub changes: u64,
    pub max_line_bytes: u64,
}

impl Drop for Index {
    fn drop(&mut self) {
        let _ = remove_file(&self.path);
        let _ = remove_file(&self.change_path);
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
    trim: &'a dyn Fn(u64),
}

impl<'a> Lines<'a> {
    fn new(snapshot: &'a Snapshot, cancel: &'a AtomicBool, trim: &'a dyn Fn(u64)) -> Self {
        Self {
            slices: snapshot.slices(0..snapshot.len()),
            slice: 0,
            byte: 0,
            offset: 0,
            number: 0,
            total: snapshot.len(),
            pending: VecDeque::new(),
            cancel,
            trim,
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

        let mut append = |b: u8| {
            hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
            hash2 = hash2.rotate_left(5) ^ (b as u64).wrapping_mul(0x517cc1b727220a95);
            len += 1;
        };
        while let Some(b) = self.next_byte() {
            if self.offset & 0x3f_ffff == 0 {
                (self.trim)(self.offset);
            }
            if self.offset & 0xffff == 0 && self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            append(b);
            if b == b'\n' {
                break;
            }
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
    compare_with_trim(left, right, cancel, progress, &|_| {}, &|_| {})
}

pub fn compare_inputs(
    left: &Input,
    right: &Input,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> io::Result<Option<Index>> {
    compare_with_trim(
        &left.snapshot,
        &right.snapshot,
        cancel,
        progress,
        &|n| left.release_until(n),
        &|n| right.release_until(n),
    )
}

fn compare_with_trim(
    left: &Snapshot,
    right: &Snapshot,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    trim_left: &dyn Fn(u64),
    trim_right: &dyn Fn(u64),
) -> io::Result<Option<Index>> {
    let same = |a: Line, b: Line| -> bool {
        if !same(a, b) {
            return false;
        }
        let ls = left.slices(a.start..a.start + a.len);
        let rs = right.slices(b.start..b.start + b.len);
        if let ([l], [r]) = (ls.as_slice(), rs.as_slice())
            && a.len <= 64 * 1024
        {
            return l.bytes == r.bytes;
        }
        let mut offset = 0;
        while offset < a.len {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let count = (a.len - offset).min(64 * 1024);
            if left.read(a.start + offset..a.start + offset + count)
                != right.read(b.start + offset..b.start + offset + count)
            {
                return false;
            }
            offset += count;
            trim_left(a.start + offset);
            trim_right(b.start + offset);
        }
        true
    };
    let path = yy_io::temp_path("yymerge-diff");
    let change_path = path.with_extension("changes");
    let result = (|| {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut writer = BufWriter::new(file);
        let mut changed = BufWriter::new(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&change_path)?,
        );
        let mut l = Lines::new(left, cancel, trim_left);
        let mut r = Lines::new(right, cancel, trim_right);
        let mut rows = 0u64;
        let mut changes = 0u64;
        let mut max_line_bytes = 0u64;
        let mut steps = 0u64;
        let mut emit = |row: Row| -> io::Result<()> {
            writer.write_all(&row.bytes())?;
            if row.kind != Kind::Equal {
                changed.write_all(&rows.to_le_bytes())?;
            }
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
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        progress.store(left.len().saturating_add(right.len()), Ordering::Relaxed);
        writer.flush()?;
        changed.flush()?;
        drop(changed);
        drop(writer);
        Ok(Some(Index {
            path: path.clone(),
            change_path: change_path.clone(),
            rows,
            changes,
            max_line_bytes,
        }))
    })();
    if !matches!(result, Ok(Some(_))) {
        let _ = remove_file(&path);
        let _ = remove_file(&change_path);
    }
    result
}

/// A disk-backed, decoded snapshot. Heap use does not grow with input size.
pub struct Input {
    pub snapshot: Snapshot,
    source: Option<std::sync::Arc<yy_io::MmapSource>>,
    trimmed: std::cell::Cell<u64>,
    pub encoding: yy_encoding::Encoding,
    pub bom: Vec<u8>,
    _temp: tempfile::TempDir,
}
impl Input {
    /// Discard only clean pages of our immutable, file-backed decoded snapshot.
    /// The mapping remains valid and faults pages back in when the user scrolls.
    fn release_until(&self, offset: u64) {
        use yy_buffer::ByteSource;
        let Some(source) = &self.source else {
            return;
        };
        let bytes = source.bytes();
        #[cfg(windows)]
        let page = 4096u64;
        #[cfg(unix)]
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        #[cfg(not(any(windows, unix)))]
        let page = 4096u64;
        let end = offset.saturating_sub(1024 * 1024).min(bytes.len() as u64) / page * page;
        let start = self.trimmed.get();
        if end < start {
            self.trimmed.set(end);
            return;
        }
        if end - start < 4 * 1024 * 1024 {
            return;
        }
        let range = &bytes[start as usize..end as usize];
        // SAFETY: these pages belong to a read-only file mapping we own. No
        // anonymous/heap pages or dirty private data can be discarded here.
        #[cfg(windows)]
        unsafe {
            let _ =
                windows::Win32::System::Memory::VirtualUnlock(range.as_ptr().cast(), range.len());
        }
        #[cfg(unix)]
        unsafe {
            let _ = libc::madvise(
                range.as_ptr().cast_mut().cast(),
                range.len(),
                libc::MADV_DONTNEED,
            );
        }
        self.trimmed.set(end);
    }
    pub fn open(
        path: &std::path::Path,
        charset: Option<&str>,
        cancel: &AtomicBool,
    ) -> io::Result<Self> {
        use yy_encoding::Encoding;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1 | 4);
        }
        let mut source = BufReader::with_capacity(256 * 1024, options.open(path)?);
        let before = source.get_ref().metadata()?;
        if !before.is_file() {
            return Err(io::Error::other("regular files only"));
        }
        let mut head = [0; 4];
        let size = source.read(&mut head)?;
        let head = &head[..size];
        let (auto, skip) = if head.starts_with(&[0xff, 0xfe, 0, 0]) {
            (Encoding::Utf32Le, 4)
        } else if head.starts_with(&[0, 0, 0xfe, 0xff]) {
            (Encoding::Utf32Be, 4)
        } else if head.starts_with(&[0xef, 0xbb, 0xbf]) {
            (Encoding::Utf8, 3)
        } else if head.starts_with(&[0xff, 0xfe]) {
            (Encoding::Utf16Le, 2)
        } else if head.starts_with(&[0xfe, 0xff]) {
            (Encoding::Utf16Be, 2)
        } else {
            (Encoding::Utf8, 0)
        };
        let encoding = charset
            .map(|s| Encoding::from_name(s).ok_or_else(|| io::Error::other("unknown encoding")))
            .transpose()?
            .unwrap_or(auto);
        if skip > 0 && auto != encoding {
            return Err(io::Error::other("encoding conflicts with BOM"));
        }
        let temp = tempfile::Builder::new()
            .prefix("yymerge-large-")
            .tempdir()?;
        let target = temp.path().join("decoded.txt");
        let mut output = BufWriter::with_capacity(256 * 1024, File::create(&target)?);
        source.seek(SeekFrom::Start(skip as u64))?;
        let mut decoder = encoding.new_decoder(false);
        let mut block = vec![0; 256 * 1024];
        let mut decoded = Vec::new();
        let mut read = skip as u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("cancelled"));
            }
            let count = source.read(&mut block)?;
            read += count as u64;
            decoded.clear();
            decoder.decode(&block[..count], &mut decoded, count == 0);
            output.write_all(&decoded)?;
            if count == 0 {
                break;
            }
        }
        let after = source.get_ref().metadata()?;
        if before.len() != after.len()
            || read != before.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(io::Error::other("input changed while capturing snapshot"));
        }
        if decoder.stats().invalid != 0 {
            return Err(io::Error::other(
                "invalid text encoding; select the correct input encoding",
            ));
        }
        output.flush()?;
        drop(output);
        let mapped = yy_io::open_file(&target)?;
        let input = Self {
            snapshot: mapped.snapshot(0..mapped.file_len),
            source: mapped.source.clone(),
            trimmed: std::cell::Cell::new(0),
            encoding,
            bom: head[..skip].to_vec(),
            _temp: temp,
        };
        let bytes = mapped.bytes();
        let mut start = 0;
        while start < bytes.len() {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("cancelled"));
            }
            let mut end = (start + 256 * 1024).min(bytes.len());
            for _ in 0..3 {
                if end < bytes.len() && bytes[end] & 0xc0 == 0x80 {
                    end += 1;
                } else {
                    break;
                }
            }
            std::str::from_utf8(&bytes[start..end]).map_err(io::Error::other)?;
            start = end;
            input.release_until(start as u64);
        }
        Ok(input)
    }
}

impl Index {
    /// Locate a changed row without loading the complete index into memory.
    pub fn next_change(
        &self,
        _reader: &mut BufReader<File>,
        start: u64,
        backwards: bool,
    ) -> io::Result<Option<u64>> {
        let mut file = File::open(&self.change_path)?;
        let mut low = 0;
        let mut high = self.changes;
        let mut value = [0u8; 8];
        while low < high {
            let mid = low + (high - low) / 2;
            file.seek(SeekFrom::Start(mid * 8))?;
            file.read_exact(&mut value)?;
            if u64::from_le_bytes(value) < start {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let position = if backwards {
            if low == 0 {
                return Ok(None);
            }
            low - 1
        } else {
            if low == self.changes {
                return Ok(None);
            }
            low
        };
        file.seek(SeekFrom::Start(position * 8))?;
        file.read_exact(&mut value)?;
        Ok(Some(u64::from_le_bytes(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compare_bytes(l: &[u8], r: &[u8]) -> Index {
        compare(
            &Snapshot::from_bytes(l),
            &Snapshot::from_bytes(r),
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap()
        .unwrap()
    }
    #[test]
    fn exact_endings_insertions_and_final_newline() {
        for (l, r) in [
            (b"a\r\n".as_slice(), b"a\n".as_slice()),
            (b"a\n", b"a"),
            (b"\r", b""),
        ] {
            assert!(compare_bytes(l, r).changes > 0);
        }
        let i = compare_bytes(b"a\nb\n", b"a\nx\nb\n");
        let rows = i.read_rows(&mut i.open().unwrap(), 0, 9).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [Kind::Equal, Kind::RightOnly, Kind::Equal]
        );
        assert_eq!(
            i.next_change(&mut i.open().unwrap(), 0, false).unwrap(),
            Some(1)
        );
    }
    #[test]
    fn large_and_single_line_do_not_have_the_old_limit() {
        let data = vec![b'x'; 33 * 1024 * 1024];
        let i = compare_bytes(&data, &data);
        assert_eq!((i.rows, i.changes), (1, 0));
        assert_eq!(i.max_line_bytes, data.len() as u64);
    }
    #[test]
    fn decoding_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input");
        std::fs::write(&path, [0xff, 0xfe, b'a', 0, 13, 0, 10, 0]).unwrap();
        let input = Input::open(&path, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(input.snapshot.read(0..input.snapshot.len()), b"a\r\n");
        assert!(Input::open(&path, None, &AtomicBool::new(true)).is_err());
        assert!(
            compare(
                &Snapshot::empty(),
                &Snapshot::empty(),
                &AtomicBool::new(true),
                &AtomicU64::new(0)
            )
            .unwrap()
            .is_none()
        );
    }
}
