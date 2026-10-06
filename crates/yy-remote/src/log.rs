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
