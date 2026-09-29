//! 複数ファイルの検索（Grep。05 章 3.5）。
//!
//! フォルダ内のファイルを列挙し、ファイルごとに文字コードを判別して検索する。
//! 結果は「パス(行番号): 行の内容」の形式で返し、UI はそれを文書として表示する
//! （その行からタグジャンプでファイルを開く）。

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use yy_buffer::Snapshot;
use yy_encoding::Encoding;
use yy_search::Searcher;

use crate::transcode;

/// 検索するファイルの指定。
#[derive(Clone, Debug)]
pub struct GrepOptions {
    pub dir: PathBuf,
    /// ファイル名のパターン（`*.txt;*.csv` のように `;` 区切り。`*` `?` が使える）
    pub files: String,
    /// サブフォルダも検索するか
    pub recursive: bool,
}

/// 見つかった行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrepHit {
    pub path: PathBuf,
    /// 行番号（1 始まり）
    pub line: u64,
    /// 行の内容（長い行は途中まで）
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GrepStats {
    pub files: u64,
    pub matched_files: u64,
    pub hits: u64,
    /// 読めなかった・バイナリなどで飛ばしたファイル
    pub skipped: u64,
}

/// 結果に載せる行の長さの上限（バイト）
const LINE_LIMIT: usize = 1000;
/// UTF-8 以外のファイルをメモリ上でデコードする大きさの上限
const DECODE_LIMIT: u64 = 256 << 20;

/// `*` と `?` だけのワイルドカードで `name` が `pat` に一致するか（大文字小文字は区別しない）。
pub fn wildcard_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn name_matches(files: &str, name: &str) -> bool {
    let pats: Vec<&str> = files
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    pats.is_empty() || pats.iter().any(|p| wildcard_match(p, name))
}

/// 対象のファイルを列挙する（名前順）。
fn list_files(opts: &GrepOptions, out: &mut Vec<PathBuf>, dir: &Path) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let mut subdirs = Vec::new();
    for e in entries {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            subdirs.push(e.path());
        } else if ft.is_file() && name_matches(&opts.files, &e.file_name().to_string_lossy()) {
            out.push(e.path());
        }
    }
    if opts.recursive {
        for d in subdirs {
            // 読めないフォルダは飛ばす
            let _ = list_files(opts, out, &d);
        }
    }
    Ok(())
}

/// ファイルの内容を UTF-8 の文書として読む。バイナリらしいファイルは `None`。
fn load(path: &Path) -> io::Result<Option<Snapshot>> {
    let f = yy_io::open_file(path)?;
    let bytes = f.bytes();
    let n = bytes.len().min(64 << 10);
    let det = yy_encoding::detect(&bytes[..n], n == bytes.len());
    let wide = matches!(
        det.encoding,
        Encoding::Utf16Le | Encoding::Utf16Be | Encoding::Utf32Le | Encoding::Utf32Be
    );
    if !wide && bytes[..n.min(8192)].contains(&0) {
        return Ok(None);
    }
    let body = det.bom_len..bytes.len();
    if det.encoding == Encoding::Utf8 {
        return Ok(Some(f.snapshot(body.start as u64..body.end as u64)));
    }
    if body.len() as u64 > DECODE_LIMIT {
        return Ok(None);
    }
    Ok(Some(
        transcode::decode_in_memory(det.encoding, &bytes[body]).snapshot,
    ))
}

/// 1 つの文書を検索し、一致した行を `hit` に渡す（同じ行の 2 つ目以降の一致は数えない）。
/// `hit` が `false` を返したら中止して `false` を返す。
pub fn grep_snapshot(
    searcher: &Searcher,
    snap: &Snapshot,
    path: &Path,
    cancel: &dyn Fn() -> bool,
    hit: &mut dyn FnMut(GrepHit) -> bool,
) -> bool {
    let len = snap.len();
    let mut line = 1u64;
    let mut counted_to = 0u64;
    let mut last_line_end: Option<u64> = None;
    let mut pos = 0u64;
    loop {
        let m: Option<Range<u64>> = match searcher.find_next(snap, 0..len, pos, &mut |_| !cancel())
        {
            Ok(m) => m,
            Err(_) => return false,
        };
        let Some(m) = m else { return true };
        // 一致した位置の行番号（前回の位置から改行を数える）
        for c in snap.chunks(counted_to..m.start) {
            line += memchr::memchr_iter(b'\n', c).count() as u64;
        }
        counted_to = m.start;
        let start = snap.find_prev(0..m.start, b'\n').map_or(0, |p| p + 1);
        let end = snap.find_next(m.start..len, b'\n').unwrap_or(len);
        if last_line_end != Some(end) {
            let text_end = end.min(start + LINE_LIMIT as u64);
            let mut bytes = snap.read(start..text_end);
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if text_end < end {
                text.push('…');
            }
            if !hit(GrepHit {
                path: path.to_owned(),
                line,
                text,
            }) {
                return false;
            }
            last_line_end = Some(end);
        }
        // 次の行から探す
        pos = if end < len { end + 1 } else { return true };
    }
}

/// フォルダ内のファイルを検索する。`progress(ファイル)` が `false` を返すか、`hit` が
/// `false` を返したら中止する。
pub fn grep(
    searcher: &Searcher,
    opts: &GrepOptions,
    progress: &mut dyn FnMut(&Path) -> bool,
    hit: &mut dyn FnMut(GrepHit) -> bool,
) -> io::Result<GrepStats> {
    let mut files = Vec::new();
    list_files(opts, &mut files, &opts.dir)?;
    let mut stats = GrepStats::default();
    for path in files {
        if !progress(&path) {
            break;
        }
        stats.files += 1;
        let snap = match load(&path) {
            Ok(Some(s)) => s,
            _ => {
                stats.skipped += 1;
                continue;
            }
        };
        let mut n = 0u64;
        let cont = grep_snapshot(searcher, &snap, &path, &|| false, &mut |h| {
            n += 1;
            hit(h)
        });
        stats.hits += n;
        if n > 0 {
            stats.matched_files += 1;
        }
        if !cont {
            break;
        }
    }
    Ok(stats)
}

/// 結果の行（`パス(行番号): 内容`）から、パスと行番号を取り出す（タグジャンプ用）。
pub fn parse_tag_line(line: &str) -> Option<(PathBuf, u64)> {
    let close = line
        .find("): ")
        .or_else(|| line.strip_suffix(')').map(|_| line.len() - 1))?;
    let open = line[..close].rfind('(')?;
    let n: u64 = line[open + 1..close].trim().parse().ok()?;
    let path = line[..open].trim();
    (!path.is_empty() && n > 0).then(|| (PathBuf::from(path), n))
}

/// 結果 1 行分の表記。
pub fn format_hit(h: &GrepHit) -> String {
    format!("{}({}): {}", h.path.display(), h.line, h.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yy_search::Query;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.txt", "a.TXT"));
        assert!(wildcard_match("a?c*", "abcdef"));
        assert!(!wildcard_match("*.txt", "a.txt.bak"));
        assert!(wildcard_match("*", ""));
        assert!(name_matches("*.rs; *.toml", "Cargo.toml"));
        assert!(name_matches("", "anything"));
    }

    #[test]
    fn tag_lines() {
        assert_eq!(
            parse_tag_line(r"C:\dir\a (1).txt(12): hello (x): y"),
            Some((PathBuf::from(r"C:\dir\a (1).txt"), 12))
        );
        assert_eq!(parse_tag_line("no tag here"), None);
    }

    #[test]
    fn greps_files_in_various_encodings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(
            dir.path().join("a.txt"),
            "one\nneedle two\nthree needle needle\n",
        )
        .unwrap();
        let sjis = yy_encoding::encode_all(
            Encoding::Cp932,
            "日本語\r\n針 needle\r\n".as_bytes(),
            yy_encoding::EscapeMode::Reject,
        )
        .unwrap();
        std::fs::write(dir.path().join("sub").join("b.txt"), sjis).unwrap();
        std::fs::write(dir.path().join("c.bin"), b"needle\0\0").unwrap();
        std::fs::write(dir.path().join("d.log"), "needle").unwrap();
        let s = Searcher::new(&Query {
            pattern: "needle".into(),
            ..Query::default()
        })
        .unwrap();
        let opts = GrepOptions {
            dir: dir.path().to_owned(),
            files: "*.txt;*.bin".into(),
            recursive: true,
        };
        let mut hits = Vec::new();
        let stats = grep(&s, &opts, &mut |_| true, &mut |h| {
            hits.push(h);
            true
        })
        .unwrap();
        let lines: Vec<(String, u64, String)> = hits
            .iter()
            .map(|h| {
                (
                    h.path.file_name().unwrap().to_string_lossy().into_owned(),
                    h.line,
                    h.text.clone(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                ("a.txt".into(), 2, "needle two".into()),
                ("a.txt".into(), 3, "three needle needle".into()),
                ("b.txt".into(), 2, "針 needle".into()),
            ]
        );
        assert_eq!(stats.files, 3);
        assert_eq!(stats.matched_files, 2);
        assert_eq!(stats.skipped, 1);
        // タグジャンプの行として読み戻せる
        let line = format_hit(&hits[2]);
        assert_eq!(parse_tag_line(&line), Some((hits[2].path.clone(), 2)));
    }
}
