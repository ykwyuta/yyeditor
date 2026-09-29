//! 性能試験用の巨大テキストファイルを生成する（08 章 2.3）。
//!
//! ```text
//! gen-bigfile <出力パス> <サイズ> [種類] [文字コード]
//!   サイズ:   1024, 512K, 100M, 10G など（UTF-8 での大きさ）
//!   種類:     log（既定, 短い行）| japanese（日本語）| long（長い行）| single（改行なし 1 行）| csv
//!   文字コード: 既定は UTF-8。cp932, euc-jp, utf-16le など
//! ```

use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        'T' => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    num.parse::<f64>().ok().map(|n| (n * mul as f64) as u64)
}

/// 再現可能な擬似乱数（xorshift64）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
];
const JA: &[&str] = &[
    "吾輩は猫である。",
    "名前はまだ無い。",
    "どこで生れたかとんと見当がつかぬ。",
    "何でも薄暗いじめじめした所で",
    "ニャーニャー泣いていた事だけは記憶している。",
    "①②③　ＡＢＣ　ｱｲｳ",
];

fn make_line(kind: &str, rng: &mut Rng, i: u64, out: &mut Vec<u8>) {
    match kind {
        "japanese" => {
            write!(out, "{i:>10}: ").unwrap();
            for _ in 0..1 + rng.below(4) {
                out.extend_from_slice(JA[rng.below(JA.len() as u64) as usize].as_bytes());
            }
            out.push(b'\n');
        }
        "long" => {
            for _ in 0..200 + rng.below(4000) {
                out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
                out.push(b' ');
            }
            out.push(b'\n');
        }
        "single" => {
            out.extend_from_slice(WORDS[rng.below(WORDS.len() as u64) as usize].as_bytes());
            out.push(b' ');
        }
        "csv" => {
            write!(
                out,
                "{i},\"{}\",{},\"multi\nline, \"\"quoted\"\" field\",{}\r\n",
                WORDS[rng.below(WORDS.len() as u64) as usize],
                rng.below(1_000_000),
                JA[rng.below(JA.len() as u64) as usize],
            )
            .unwrap();
        }
        _ => {
            writeln!(
                out,
                "2026-09-28 12:{:02}:{:02}.{:03} {} [{}] request {} took {} ms",
                (i / 60) % 60,
                i % 60,
                rng.below(1000),
                ["INFO ", "WARN ", "ERROR", "DEBUG"][rng.below(4) as usize],
                WORDS[rng.below(WORDS.len() as u64) as usize],
                i,
                rng.below(5000),
            )
            .unwrap();
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: gen-bigfile <output> <size> [log|japanese|long|single|csv] [encoding]");
        return ExitCode::FAILURE;
    }
    let Some(size) = parse_size(&args[2]) else {
        eprintln!("invalid size: {}", args[2]);
        return ExitCode::FAILURE;
    };
    let kind = args.get(3).map(String::as_str).unwrap_or("log");
    let encoding = match args.get(4) {
        Some(name) => match yy_encoding::Encoding::from_name(name) {
            Some(e) => e,
            None => {
                eprintln!("unknown encoding: {name}");
                return ExitCode::FAILURE;
            }
        },
        None => yy_encoding::Encoding::Utf8,
    };
    let mut encoder = encoding.new_encoder(yy_encoding::EscapeMode::Literal);
    let mut encoded = Vec::new();
    let file = match File::create(&args[1]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}: {e}", args[1]);
            return ExitCode::FAILURE;
        }
    };
    let start = Instant::now();
    let mut w = BufWriter::with_capacity(8 << 20, file);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut buf = Vec::with_capacity(64 << 10);
    let mut written = 0u64;
    let mut i = 0u64;
    while written < size {
        buf.clear();
        while buf.len() < 60 << 10 {
            make_line(kind, &mut rng, i, &mut buf);
            i += 1;
        }
        let mut n = buf.len().min((size - written) as usize);
        // 途中で切る場合も UTF-8 の文字境界に合わせる
        while n > 0 && n < buf.len() && buf[n] & 0xC0 == 0x80 {
            n -= 1;
        }
        encoded.clear();
        encoder.encode(&buf[..n], &mut encoded, false, &mut |_| {});
        if let Err(e) = w.write_all(&encoded) {
            eprintln!("write error: {e}");
            return ExitCode::FAILURE;
        }
        written += n as u64;
        if n == 0 {
            break;
        }
    }
    encoded.clear();
    encoder.encode(&[], &mut encoded, true, &mut |_| {});
    if let Err(e) = w.write_all(&encoded).and_then(|_| w.flush()) {
        eprintln!("write error: {e}");
        return ExitCode::FAILURE;
    }
    let secs = start.elapsed().as_secs_f64();
    eprintln!(
        "wrote {} bytes of UTF-8 text ({kind}, saved as {encoding}) in {secs:.2}s ({:.0} MB/s)",
        written,
        written as f64 / secs / 1e6
    );
    ExitCode::SUCCESS
}
