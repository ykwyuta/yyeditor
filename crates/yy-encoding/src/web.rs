//! `encoding_rs` を使う文字コード（ISO-2022-JP、欧州・中国語・韓国語など）。

use std::ops::Range;

use encoding_rs::{DecoderResult, EncoderResult};

use crate::{Sink, is_escape_prefix};

pub(crate) struct WebDecoder {
    dec: encoding_rs::Decoder,
    /// これまでに読んだ入力の末尾（不正なバイトが前回の入力にまたがる場合に使う）
    hist: Vec<u8>,
}

const HIST: usize = 8;

impl WebDecoder {
    pub fn new(enc: &'static encoding_rs::Encoding) -> WebDecoder {
        WebDecoder {
            dec: enc.new_decoder_without_bom_handling(),
            hist: Vec::new(),
        }
    }

    pub fn decode(&mut self, src: &[u8], sink: &mut Sink<'_>, last: bool) {
        let mut read_total = 0;
        loop {
            let need = self
                .dec
                .max_utf8_buffer_length_without_replacement(src.len() - read_total)
                .unwrap_or(1 << 20)
                .max(16);
            let start = sink.dst.len();
            sink.dst.resize(start + need, 0);
            let (res, read, written) = self.dec.decode_to_utf8_without_replacement(
                &src[read_total..],
                &mut sink.dst[start..],
                last,
            );
            sink.dst.truncate(start + written);
            sink.stats.literal_escapes += count_literal_escapes(&sink.dst[start..]);
            read_total += read;
            match res {
                DecoderResult::InputEmpty => break,
                DecoderResult::OutputFull => continue,
                DecoderResult::Malformed(bad, after) => {
                    // 不正なバイトは read_total - after の直前 bad バイト（前回の入力にまたがることがある）
                    let end = read_total as isize - after as isize;
                    let begin = end - bad as isize;
                    for p in begin..end {
                        let b = if p >= 0 {
                            src[p as usize]
                        } else {
                            let h = self.hist.len() as isize + p;
                            self.hist.get(h.max(0) as usize).copied().unwrap_or(0)
                        };
                        sink.invalid(b);
                    }
                }
            }
        }
        // 履歴を更新
        if src.len() >= HIST {
            self.hist.clear();
            self.hist.extend_from_slice(&src[src.len() - HIST..]);
        } else {
            self.hist.extend_from_slice(src);
            let n = self.hist.len();
            if n > HIST {
                self.hist.drain(..n - HIST);
            }
        }
    }
}

/// 出力にエスケープ文字と同じ文字があれば数える（GB18030 などは全 Unicode を表せる）。
fn count_literal_escapes(out: &[u8]) -> u64 {
    let mut n = 0;
    let mut i = 0;
    while let Some(k) = out[i..].iter().position(|&b| b == 0xF4) {
        let p = i + k;
        if is_escape_prefix(&out[p..]) {
            n += 1;
        }
        i = p + 1;
    }
    n
}

pub(crate) struct WebEncoder {
    enc: encoding_rs::Encoder,
}

impl WebEncoder {
    pub fn new(enc: &'static encoding_rs::Encoding) -> WebEncoder {
        WebEncoder {
            enc: enc.new_encoder(),
        }
    }

    pub fn encode(
        &mut self,
        s: &str,
        dst: &mut Vec<u8>,
        last: bool,
        bad: &mut dyn FnMut(Range<usize>),
    ) {
        let mut read_total = 0;
        loop {
            let need = self
                .enc
                .max_buffer_length_from_utf8_without_replacement(s.len() - read_total)
                .unwrap_or(1 << 20)
                .max(16);
            let start = dst.len();
            dst.resize(start + need, 0);
            let (res, read, written) = self.enc.encode_from_utf8_without_replacement(
                &s[read_total..],
                &mut dst[start..],
                last,
            );
            dst.truncate(start + written);
            read_total += read;
            match res {
                EncoderResult::InputEmpty => break,
                EncoderResult::OutputFull => continue,
                EncoderResult::Unmappable(c) => {
                    bad(read_total - c.len_utf8()..read_total);
                }
            }
        }
    }
}
