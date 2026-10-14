//! フォルダを zip にする（ワークスペースの「zip にしてダウンロード」。11 章）。
//!
//! 接続先の一時フォルダに zip を作り、端末はそれを [`Request::Open`](yy_proto::Request) で取り寄せる。
//! エージェントは要求を 1 つずつ処理するので、作業は [`ZipJob::step`] で少しずつ進め（目安 0.2 秒）、
//! そのたびに進み具合を返す（端末は表示し、Esc で中止できる）。
//!
//! 圧縮は deflate（miniz_oxide）。ZIP64 は使わない（4 GB 以上のファイル・65,535 個を超える項目は
//! 上限を超えたものとして扱う。端末は別の方法でダウンロードする）。シンボリックリンクはたどらない。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use miniz_oxide::deflate::core::{
    CompressorOxide, TDEFLFlush, TDEFLStatus, compress, create_comp_flags_from_zip_params,
};
use yy_proto::{ZipProgress, ZipState};

/// 1 回の [`ZipJob::step`] で使う時間の目安
const STEP: Duration = Duration::from_millis(200);
/// ZIP64 なしの上限
const MAX_ENTRIES: usize = 0xFFFF;
const MAX_SIZE: u64 = 0xFFFF_FFFE;
/// 読み出しの単位
const CHUNK: usize = 256 << 10;

/// zip に入れる 1 項目。
struct Entry {
    path: PathBuf,
    /// zip の中の名前（フォルダは `/` で終わる）
    name: Vec<u8>,
    dir: bool,
    size: u64,
    mtime: u64,
    mode: u32,
}

/// 書いている途中のファイル。
struct Current {
    file: File,
    /// ローカルヘッダーの位置
    header_at: u64,
    crc: u32,
    read: u64,
    /// 圧縮したデータの大きさ
    packed: u64,
    eof: bool,
}

pub(crate) struct ZipJob {
    id: u32,
    /// 端末の時差（秒）。zip の日時（MS-DOS 形式）は時差を持たないので端末の時刻で書く
    /// （UT の拡張は UTC のまま）
    utc_offset: i64,
    temp: PathBuf,
    out: Option<BufWriter<File>>,
    limit: u64,
    entries: Vec<Entry>,
    next: usize,
    current: Option<Current>,
    comp: Box<CompressorOxide>,
    central: Vec<u8>,
    written: u64,
    files: u64,
    total_files: u64,
    read: u64,
    total_bytes: u64,
    skipped: u64,
    state: ZipState,
    inbuf: Vec<u8>,
    outbuf: Vec<u8>,
}

impl Drop for ZipJob {
    fn drop(&mut self) {
        self.out = None;
        let _ = fs::remove_file(&self.temp);
    }
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

impl ZipJob {
    /// `dir` を zip にし始める（中身を調べて一時ファイルを作るだけ。圧縮は [`step`](Self::step)）。
    pub(crate) fn begin(id: u32, dir: &Path, limit: u64, utc_offset: i32) -> io::Result<ZipJob> {
        let meta = fs::metadata(dir)?;
        if !meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "フォルダではありません",
            ));
        }
        let base = dir
            .file_name()
            .map_or_else(|| b"root".to_vec(), crate::name_bytes);
        let mut entries = Vec::new();
        let mut skipped = 0;
        walk(dir, &base, &mut entries, &mut skipped);

        let temp = std::env::temp_dir().join(format!(
            "yy-zip-{}-{}.zip",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&temp)?;
        let total_files = entries.iter().filter(|e| !e.dir).count() as u64;
        let total_bytes = entries.iter().map(|e| e.size).sum();
        let mut job = ZipJob {
            id,
            utc_offset: i64::from(utc_offset),
            temp,
            out: Some(BufWriter::with_capacity(1 << 20, file)),
            limit,
            entries,
            next: 0,
            current: None,
            comp: Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(
                6, -15, 0,
            ))),
            central: Vec::new(),
            written: 0,
            files: 0,
            total_files,
            read: 0,
            total_bytes,
            skipped,
            state: ZipState::Running,
            inbuf: vec![0; CHUNK],
            outbuf: vec![0; CHUNK + 1024],
        };
        if job.entries.len() > MAX_ENTRIES {
            job.give_up(format!(
                "項目が多すぎます（{} 個。zip にできるのは {MAX_ENTRIES} 個まで）",
                job.entries.len()
            ));
        } else if let Some(big) = job.entries.iter().find(|e| e.size > MAX_SIZE) {
            job.give_up(format!(
                "4 GB 以上のファイルがあります（{}）",
                String::from_utf8_lossy(&big.name)
            ));
        }
        Ok(job)
    }

    pub(crate) fn progress(&self) -> ZipProgress {
        ZipProgress {
            job: self.id,
            temp: crate::path_bytes(&self.temp),
            files: self.files,
            total_files: self.total_files,
            read: self.read,
            total_bytes: self.total_bytes,
            written: self.written,
            skipped: self.skipped,
            state: self.state.clone(),
        }
    }

    /// 上限を超えたのでやめる（一時ファイルは消す）。
    fn give_up(&mut self, why: String) {
        self.out = None;
        self.current = None;
        let _ = fs::remove_file(&self.temp);
        self.state = ZipState::TooLarge(why);
    }

    /// 作業を少し進める。
    pub(crate) fn step(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + STEP;
        while self.state == ZipState::Running && Instant::now() < deadline {
            if let Err(e) = self.advance() {
                // 書けない（容量不足など）ときは作りかけを残さない
                self.out = None;
                self.current = None;
                let _ = fs::remove_file(&self.temp);
                return Err(e);
            }
            if self.state == ZipState::Running && self.written > self.limit {
                self.give_up(format!(
                    "圧縮後の大きさが上限（{} バイト）を超えました",
                    self.limit
                ));
            }
        }
        Ok(())
    }

    fn out(&mut self) -> &mut BufWriter<File> {
        self.out.as_mut().expect("zip の書き出し先")
    }

    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.out().write_all(b)?;
        self.written += b.len() as u64;
        Ok(())
    }

    /// 次の一区切り（ファイルの一部・フォルダ 1 つ・最後の目次）を書く。
    fn advance(&mut self) -> io::Result<()> {
        if self.current.is_some() {
            return self.pump();
        }
        let Some(i) = (self.next < self.entries.len()).then_some(self.next) else {
            return self.finish();
        };
        self.next += 1;
        if self.entries[i].dir {
            let header_at = self.written;
            let header = local_header(&self.entries[i], self.dos(self.entries[i].mtime), 0, 0);
            self.put(&header)?;
            self.add_central(i, 0, 0, 0, 0, header_at);
            return Ok(());
        }
        let file = match File::open(&self.entries[i].path) {
            Ok(f) => f,
            Err(_) => {
                self.skipped += 1;
                self.read += self.entries[i].size;
                return Ok(());
            }
        };
        let header_at = self.written;
        let header = local_header(&self.entries[i], self.dos(self.entries[i].mtime), 8, 0);
        self.put(&header)?;
        self.comp.reset();
        self.current = Some(Current {
            file,
            header_at,
            crc: 0,
            read: 0,
            packed: 0,
            eof: false,
        });
        Ok(())
    }

    /// 書いている途中のファイルを少し圧縮する。
    fn pump(&mut self) -> io::Result<()> {
        let mut cur = self.current.take().expect("書いている途中のファイル");
        let n = if cur.eof {
            0
        } else {
            match cur.file.read(&mut self.inbuf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return self.keep(cur),
                Err(e) => {
                    // 読んでいる途中で読めなくなった: ここまでで終える（中身は短くなる）
                    let _ = e;
                    self.skipped += 1;
                    0
                }
            }
        };
        if n == 0 {
            cur.eof = true;
        }
        if cur.read + n as u64 > MAX_SIZE {
            self.give_up("4 GB 以上のファイルがあります".into());
            return Ok(());
        }
        cur.crc = crc32(cur.crc, &self.inbuf[..n]);
        cur.read += n as u64;
        self.read += n as u64;
        let flush = if cur.eof {
            TDEFLFlush::Finish
        } else {
            TDEFLFlush::None
        };
        let inbuf = std::mem::take(&mut self.inbuf);
        let mut outbuf = std::mem::take(&mut self.outbuf);
        let r = self.deflate(&inbuf[..n], &mut outbuf, flush, &mut cur.packed);
        self.inbuf = inbuf;
        self.outbuf = outbuf;
        r?;
        if !cur.eof {
            return self.keep(cur);
        }
        // ローカルヘッダーの CRC・大きさを書き直す
        let i = self.next - 1;
        let end = self.written;
        let out = self.out();
        out.seek(SeekFrom::Start(cur.header_at + 14))?;
        out.write_all(&cur.crc.to_le_bytes())?;
        out.write_all(&(cur.packed as u32).to_le_bytes())?;
        out.write_all(&(cur.read as u32).to_le_bytes())?;
        out.seek(SeekFrom::Start(end))?;
        self.add_central(i, 8, cur.crc, cur.packed, cur.read, cur.header_at);
        self.files += 1;
        // 調べたときより大きさが変わっていたら、全体の大きさを合わせる
        let listed = self.entries[i].size;
        self.total_bytes = self.total_bytes - listed + cur.read;
        Ok(())
    }

    /// `input` を圧縮して書く（`Finish` なら圧縮の終わりまで）。
    fn deflate(
        &mut self,
        mut input: &[u8],
        outbuf: &mut [u8],
        flush: TDEFLFlush,
        packed: &mut u64,
    ) -> io::Result<()> {
        loop {
            let (status, used, made) = compress(&mut self.comp, input, outbuf, flush);
            input = &input[used..];
            if made > 0 {
                *packed += made as u64;
                self.put(&outbuf[..made])?;
            }
            match status {
                TDEFLStatus::Done => return Ok(()),
                TDEFLStatus::Okay if input.is_empty() && flush == TDEFLFlush::None => {
                    return Ok(());
                }
                TDEFLStatus::Okay => {}
                _ => return Err(io::Error::other("圧縮できませんでした")),
            }
        }
    }

    /// 更新日時の MS-DOS 形式（端末の時刻）。
    fn dos(&self, mtime: u64) -> (u16, u16) {
        dos_time(mtime.saturating_add_signed(self.utc_offset))
    }

    fn keep(&mut self, cur: Current) -> io::Result<()> {
        self.current = Some(cur);
        Ok(())
    }

    fn add_central(&mut self, i: usize, method: u16, crc: u32, packed: u64, size: u64, at: u64) {
        let (time, date) = self.dos(self.entries[i].mtime);
        let e = &self.entries[i];
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&0x0314u16.to_le_bytes()); // UNIX・2.0
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&flags(&e.name).to_le_bytes());
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&time.to_le_bytes());
        c.extend_from_slice(&date.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(packed as u32).to_le_bytes());
        c.extend_from_slice(&(size as u32).to_le_bytes());
        c.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        c.extend_from_slice(&9u16.to_le_bytes()); // 拡張: 更新日時（UT）
        c.extend_from_slice(&0u16.to_le_bytes()); // コメント
        c.extend_from_slice(&0u16.to_le_bytes()); // ディスク
        c.extend_from_slice(&0u16.to_le_bytes()); // 内部の属性
        let dos = if e.dir { 0x10 } else { 0 };
        c.extend_from_slice(&((e.mode << 16) | dos).to_le_bytes());
        c.extend_from_slice(&(at as u32).to_le_bytes());
        c.extend_from_slice(&e.name);
        c.extend_from_slice(&ut_extra(e.mtime));
    }

    /// 目次（セントラルディレクトリ）と終わりの記録を書く。
    fn finish(&mut self) -> io::Result<()> {
        let at = self.written;
        let central = std::mem::take(&mut self.central);
        self.put(&central)?;
        let count = self.entries.len() as u16;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0, 0, 0, 0]);
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&(central.len() as u32).to_le_bytes());
        end.extend_from_slice(&(at as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.put(&end)?;
        self.out().flush()?;
        self.out().get_ref().sync_all()?;
        if self.written > self.limit {
            self.give_up(format!(
                "圧縮後の大きさが上限（{} バイト）を超えました",
                self.limit
            ));
        } else {
            self.state = ZipState::Done;
        }
        Ok(())
    }
}

/// フォルダの中身を名前順に集める（シンボリックリンクはたどらない。読めないフォルダは飛ばす）。
fn walk(dir: &Path, name: &[u8], out: &mut Vec<Entry>, skipped: &mut u64) {
    let meta = match fs::symlink_metadata(dir) {
        Ok(m) => m,
        Err(_) => {
            *skipped += 1;
            return;
        }
    };
    let mut dname = name.to_vec();
    dname.push(b'/');
    out.push(Entry {
        path: dir.to_path_buf(),
        name: dname,
        dir: true,
        size: 0,
        mtime: mtime(&meta),
        mode: mode(&meta, 0o40755),
    });
    let mut children: Vec<(Vec<u8>, PathBuf)> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| (crate::name_bytes(&e.file_name()), e.path()))
            .collect(),
        Err(_) => {
            *skipped += 1;
            return;
        }
    };
    children.sort();
    for (child, path) in children {
        let mut cname = name.to_vec();
        cname.push(b'/');
        cname.extend_from_slice(&child);
        let Ok(m) = fs::symlink_metadata(&path) else {
            *skipped += 1;
            continue;
        };
        if m.is_dir() {
            walk(&path, &cname, out, skipped);
        } else if m.is_file() {
            out.push(Entry {
                path,
                name: cname,
                dir: false,
                size: m.len(),
                mtime: mtime(&m),
                mode: mode(&m, 0o100644),
            });
        }
        // シンボリックリンク・デバイスなどは入れない
    }
}

fn mtime(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

#[cfg(unix)]
fn mode(m: &fs::Metadata, _default: u32) -> u32 {
    use std::os::unix::fs::MetadataExt;
    m.mode()
}

#[cfg(not(unix))]
fn mode(_m: &fs::Metadata, default: u32) -> u32 {
    default
}

/// 名前が UTF-8 なら、その印（汎用フラグのビット 11）。
fn flags(name: &[u8]) -> u16 {
    if std::str::from_utf8(name).is_ok() {
        0x0800
    } else {
        0
    }
}

/// ローカルヘッダー（CRC・大きさはファイルを書き終えてから書き直す）。
fn local_header(e: &Entry, (time, date): (u16, u16), method: u16, crc: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(30 + e.name.len() + 9);
    h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    h.extend_from_slice(&20u16.to_le_bytes());
    h.extend_from_slice(&flags(&e.name).to_le_bytes());
    h.extend_from_slice(&method.to_le_bytes());
    h.extend_from_slice(&time.to_le_bytes());
    h.extend_from_slice(&date.to_le_bytes());
    h.extend_from_slice(&crc.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
    h.extend_from_slice(&9u16.to_le_bytes());
    h.extend_from_slice(&e.name);
    h.extend_from_slice(&ut_extra(e.mtime));
    h
}

/// 拡張フィールド UT（UNIX 時間の更新日時。7-Zip などはこれを使う）。
fn ut_extra(mtime: u64) -> [u8; 9] {
    let t = (mtime.min(u64::from(u32::MAX)) as u32).to_le_bytes();
    [0x55, 0x54, 5, 0, 1, t[0], t[1], t[2], t[3]]
}

/// UNIX 時間（秒）を MS-DOS の時刻・日付にする（UTC。1980 年より前は 1980-01-01）。
pub(crate) fn dos_time(secs: u64) -> (u16, u16) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // 日数から年月日（Howard Hinnant の civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if year < 1980 {
        return (0, (1 << 5) | 1);
    }
    let year = year.min(2107);
    let time = ((rem / 3600) << 11) | (((rem % 3600) / 60) << 5) | ((rem % 60) / 2);
    let date = ((year - 1980) << 9) | (month << 5) | day;
    (time as u16, date as u16)
}

/// CRC-32（zip・gzip と同じ多項式）。
pub(crate) fn crc32(crc: u32, data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *v = c;
        }
        t
    });
    let mut c = !crc;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}
