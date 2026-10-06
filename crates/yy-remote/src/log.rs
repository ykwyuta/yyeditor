//! 接続の記録（11 章 4.6）。
//!
//! 接続に失敗したときに原因を調べられるよう、接続設定の解決・TCP 接続・プロキシ・踏み台・
//! ホスト鍵の照合・認証・エージェントの配置と起動の各段階を、接続を始めてからの経過時間つきで
//! 記録する。パスワード・パスフレーズ・keyboard-interactive の答えは記録しない。

use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 記録のファイルがこれより大きくなったら、古い方を `.old.log` に移して新しく始める
const FILE_LIMIT: u64 = 1 << 20;

/// 1 回の接続の記録。接続を行うスレッドから書き、終わったあとに UI が読む。
pub struct ConnectLog {
    start: Instant,
    lines: Mutex<Vec<(Duration, String)>>,
}

impl Default for ConnectLog {
    fn default() -> Self {
        ConnectLog::new()
    }
}

impl ConnectLog {
    pub fn new() -> ConnectLog {
        ConnectLog {
            start: Instant::now(),
            lines: Mutex::new(Vec::new()),
        }
    }

    /// 1 行書く（改行を含む場合は行ごとに分ける）。
    pub fn note(&self, msg: impl AsRef<str>) {
        let at = self.start.elapsed();
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        for l in msg.as_ref().lines() {
            lines.push((at, l.trim_end().to_owned()));
        }
    }

    /// `[経過秒] 内容` の形の行。
    pub fn lines(&self) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        lines
            .iter()
            .map(|(at, l)| format!("[{:>7.3}s] {l}", at.as_secs_f64()))
            .collect()
    }

    /// 最後の `n` 行。
    pub fn tail(&self, n: usize) -> Vec<String> {
        let all = self.lines();
        all[all.len().saturating_sub(n)..].to_vec()
    }

    /// 内容（行）があるか。
    pub fn contains(&self, needle: &str) -> bool {
        let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        lines.iter().any(|(_, l)| l.contains(needle))
    }
}

/// `log` を `header`（日時・接続先など）に続けて `path` に追記する。ファイルが大きくなったら
/// 古い記録を `<名前>.old.log` に移す（1 世代だけ残す）。
pub fn append_to_file(path: &Path, header: &str, log: &ConnectLog) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > FILE_LIMIT) {
        let _ = std::fs::rename(path, path.with_extension("old.log"));
    }
    let mut text = format!("==== {header} ====\n");
    for l in log.lines() {
        text.push_str(&l);
        text.push('\n');
    }
    text.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(text.as_bytes())
}

/// 記録の行を受け取る関数。
pub type Sink = Box<dyn Fn(&str) + Send + Sync>;

/// 転送の記録（13 章 6）。1 行ごとにすぐファイルに追記し（途中で落ちても残る）、
/// 画面にも渡す（[`TransferLog::set_sink`]）。
pub struct TransferLog {
    file: Mutex<Option<std::fs::File>>,
    path: Option<std::path::PathBuf>,
    clock: fn() -> String,
    sink: Mutex<Option<Sink>>,
}

/// 転送の記録のファイルの大きさの上限（超えたら古い方を `.old.log` に移す）
const TRANSFER_LOG_LIMIT: u64 = 8 << 20;

impl TransferLog {
    /// `path` に追記する記録（`None` なら画面だけ）。`clock` は行の先頭の日時。
    pub fn new(path: Option<std::path::PathBuf>, clock: fn() -> String) -> TransferLog {
        let file = path.as_ref().and_then(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::metadata(p).is_ok_and(|m| m.len() > TRANSFER_LOG_LIMIT) {
                let _ = std::fs::rename(p, p.with_extension("old.log"));
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        TransferLog {
            file: Mutex::new(file),
            path,
            clock,
            sink: Mutex::new(None),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 書いた行を受け取る（画面のログ）。
    pub fn set_sink(&self, f: Sink) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    /// 1 行書く（`job` は転送の番号）。
    pub fn line(&self, job: Option<u64>, msg: &str) {
        let tag = job.map(|j| format!(" [#{j}]")).unwrap_or_default();
        let time = (self.clock)();
        let mut text = String::new();
        for l in msg.lines() {
            text.push_str(&format!("{time}{tag} {}\n", l.trim_end()));
        }
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.write_all(text.as_bytes());
        }
        if let Some(s) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            s(text.trim_end());
        }
    }

    /// 接続の記録（[`ConnectLog`]）を写す。
    pub fn connect_log(&self, job: Option<u64>, log: &ConnectLog) {
        for l in log.lines() {
            self.line(job, &format!("接続: {l}"));
        }
    }
}

/// 協定世界時の日時（`2026-10-06 12:34:56.789Z`。UI は手元の時刻を渡す）。
pub fn utc_clock() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // 1970-01-01 からの日数を年月日にする（Howard Hinnant の方法）
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        d.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_rotates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("remote-ssh.log");
        let log = ConnectLog::new();
        log.note("接続します");
        append_to_file(&path, "first", &log).unwrap();
        append_to_file(&path, "second", &log).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("==== first ====\n["), "{text}");
        assert!(
            text.contains("] 接続します\n\n==== second ====\n"),
            "{text}"
        );

        std::fs::write(&path, vec![b'x'; (FILE_LIMIT + 1) as usize]).unwrap();
        append_to_file(&path, "third", &log).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("==== third")
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("logs").join("remote-ssh.old.log"))
                .unwrap()
                .len(),
            FILE_LIMIT + 1
        );
    }

    #[test]
    fn transfer_log_appends_lines_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfer.log");
        let log = TransferLog::new(Some(path.clone()), || "T".into());
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        log.set_sink(Box::new(move |l| s.lock().unwrap().push(l.to_owned())));
        log.line(Some(3), "開始\n二行目");
        log.line(None, "全体");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "T [#3] 開始\nT [#3] 二行目\nT 全体\n"
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
        let c = utc_clock();
        assert_eq!(c.len(), 24, "{c}");
        assert!(c.starts_with("20") && c.ends_with('Z'), "{c}");
    }

    #[test]
    fn records_lines_with_elapsed_time() {
        let log = ConnectLog::new();
        log.note("first");
        log.note("a\nb\n");
        let lines = log.lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("[  0.0") && lines[0].ends_with("] first"));
        assert!(lines[2].ends_with("] b"));
        assert_eq!(log.tail(2).len(), 2);
        assert!(log.tail(2)[0].ends_with("] a"));
        assert!(log.contains("first") && !log.contains("zzz"));
    }
}
