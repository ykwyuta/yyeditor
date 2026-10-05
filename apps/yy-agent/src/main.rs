//! yy-agent: yyeditor が SSH 接続先に置くエージェント（11 章 6）。
//!
//! ```text
//! yy-agent serve --stdio   標準入出力で要求に答える（最初に印を出す）
//! yy-agent --version       版を表示する
//! yy-agent --sha256        自分自身の SHA-256 を表示する（配置の確認用）
//! ```

use std::io::Write;
use std::time::Duration;

/// これより長く使われていない古い版を消す
const KEEP_OLD_VERSIONS: Duration = Duration::from_secs(30 * 24 * 60 * 60);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match args.as_slice() {
        ["--version"] => {
            println!("yy-agent {}", env!("CARGO_PKG_VERSION"));
            0
        }
        ["--sha256"] => match yy_agent::self_sha256() {
            Ok(h) => {
                println!("{h}");
                0
            }
            Err(e) => {
                eprintln!("yy-agent: {e}");
                1
            }
        },
        ["serve", "--stdio"] => {
            if let Ok(exe) = std::env::current_exe() {
                yy_agent::clean_old_versions(&exe, KEEP_OLD_VERSIONS);
            }
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let started = out
                .write_all(yy_proto::MAGIC)
                .and_then(|()| out.flush())
                .and_then(|()| yy_agent::serve(std::io::stdin().lock(), out));
            match started {
                Ok(()) => 0,
                // 端末との接続が切れた
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
                Err(e) => {
                    eprintln!("yy-agent: {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: yy-agent serve --stdio | --version | --sha256");
            2
        }
    };
    std::process::exit(code);
}
