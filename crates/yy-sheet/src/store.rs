//! チャンクを置くファイル（独自形式のファイルと作業ファイル）と、展開したチャンクのキャッシュ。
//!
//! ファイル全体をメモリに置かず、チャンクを位置を指定して読み（`pread`）、展開したものを予算の中の
//! LRU のキャッシュに置く（15 章 3.6）。新しいチャンクは作業ファイルの末尾に書き足す。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::budget::{Budget, Part};
use crate::chunk::Data;

static NEXT_STORE: AtomicU64 = AtomicU64::new(1);

/// チャンクを置くファイル。
#[derive(Debug)]
pub struct Store {
    pub id: u64,
    /// 閉じるまで `Some`
    file: Option<File>,
    writable: bool,
    /// 末尾（書き足す位置）
    end: Mutex<u64>,
    path: Mutex<PathBuf>,
    delete_on_drop: Mutex<Option<PathBuf>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        // ハンドルを閉じてから消す（Windows）
        drop(self.file.take());
        if let Some(p) = self.delete_on_drop.get_mut().unwrap().take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// ほかのプロセスに書き込ませない共有モードで開く（Windows）。名前の変更（保存時の退避）は許す。
fn options(write: bool) -> OpenOptions {
    let mut o = OpenOptions::new();
    o.read(true).write(write);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_DELETE: u32 = 0x4;
        o.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    o
}

fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = file.seek_write(&buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            done += n;
        }
        Ok(())
    }
}

impl Store {
    fn from_file(file: File, path: &Path, writable: bool) -> io::Result<Arc<Store>> {
        let end = file.metadata()?.len();
        Ok(Arc::new(Store {
            id: NEXT_STORE.fetch_add(1, Ordering::Relaxed),
            file: Some(file),
            writable,
            end: Mutex::new(end),
            path: Mutex::new(path.to_owned()),
            delete_on_drop: Mutex::new(None),
        }))
    }

    /// 既にあるファイルを開く（書けなければ読み取りだけ）。
    pub fn open(path: &Path) -> io::Result<Arc<Store>> {
        match options(true).open(path) {
            Ok(f) => Store::from_file(f, path, true),
            Err(_) => Store::from_file(options(false).open(path)?, path, false),
        }
    }

    /// 新しいファイルを作る（あれば失敗する）。
    pub fn create(path: &Path) -> io::Result<Arc<Store>> {
        let mut o = options(true);
        o.create_new(true);
        Store::from_file(o.open(path)?, path, true)
    }

    /// 作業ファイル（閉じたら消す）。
    pub fn work(dir: &Path) -> io::Result<Arc<Store>> {
        std::fs::create_dir_all(dir)?;
        static N: AtomicU64 = AtomicU64::new(0);
        let path = dir.join(format!(
            "yysheet-work-{}-{}.tmp",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let s = Store::create(&path)?;
        s.delete_when_dropped();
        Ok(s)
    }

    fn file(&self) -> &File {
        self.file.as_ref().expect("open")
    }

    pub fn path(&self) -> PathBuf {
        self.path.lock().unwrap().clone()
    }

    pub fn set_path(&self, p: &Path) {
        *self.path.lock().unwrap() = p.to_owned();
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    /// 使われなくなったらファイルを消す。
    pub fn delete_when_dropped(&self) {
        *self.delete_on_drop.lock().unwrap() = Some(self.path());
    }

    pub fn len(&self) -> u64 {
        *self.end.lock().unwrap()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `offset` から `len` バイトを読む。
    pub fn read(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len as usize];
        read_at(self.file(), &mut buf, offset)?;
        Ok(buf)
    }

    /// 末尾に書き足し（8 バイト境界に揃える）、書いた位置を返す。並列に呼んでよい。
    pub fn append(&self, bytes: &[u8]) -> io::Result<u64> {
        let offset = {
            let mut end = self.end.lock().unwrap();
            let at = end.next_multiple_of(8);
            *end = at + bytes.len() as u64;
            at
        };
        write_at(self.file(), bytes, offset)?;
        Ok(offset)
    }

    /// 書いた内容をディスクに確かにする。
    pub fn sync(&self) -> io::Result<()> {
        self.file().sync_data()
    }
}

/// ファイルの中の範囲。
#[derive(Clone, Debug)]
pub struct Region {
    pub store: Arc<Store>,
    pub offset: u64,
    pub len: u64,
}

impl Region {
    pub fn read(&self) -> io::Result<Vec<u8>> {
        self.store.read(self.offset, self.len)
    }
}

/// 展開したチャンクのキャッシュ（予算の「チャンクのキャッシュ」の中の LRU）。
#[derive(Debug, Default)]
pub struct ChunkCache {
    map: Mutex<HashMap<u64, Entry>>,
    tick: AtomicU64,
}

#[derive(Debug)]
struct Entry {
    data: Arc<Data>,
    bytes: u64,
    tick: u64,
}

impl ChunkCache {
    pub fn get(&self, id: u64) -> Option<Arc<Data>> {
        let mut m = self.map.lock().unwrap();
        let e = m.get_mut(&id)?;
        e.tick = self.tick.fetch_add(1, Ordering::Relaxed);
        Some(e.data.clone())
    }

    /// 入れて、予算を超えたら古いものから捨てる。
    pub fn put(&self, id: u64, data: Arc<Data>, budget: &Budget) {
        let bytes = data.heap_bytes() as u64 + 64;
        let mut m = self.map.lock().unwrap();
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        if let Some(old) = m.insert(id, Entry { data, bytes, tick }) {
            budget.sub(Part::Cache, old.bytes);
        }
        budget.add(Part::Cache, bytes);
        if !budget.fits(Part::Cache, 0) {
            // 予算の 3/4 まで減らす
            let target = budget.of(Part::Cache) / 4 * 3;
            let mut entries: Vec<(u64, u64, u64)> =
                m.iter().map(|(&k, e)| (e.tick, k, e.bytes)).collect();
            entries.sort_unstable();
            for (_, k, b) in entries {
                if budget.used(Part::Cache) <= target {
                    break;
                }
                if k != id {
                    m.remove(&k);
                    budget.sub(Part::Cache, b);
                }
            }
        }
    }

    pub fn remove(&self, id: u64, budget: &Budget) {
        if let Some(e) = self.map.lock().unwrap().remove(&id) {
            budget.sub(Part::Cache, e.bytes);
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
