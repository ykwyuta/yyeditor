//! Source-order edit timeline, in Media Foundation's 100 ns units.
use crate::{Params, Result, value};
pub const SECOND: i64 = 10_000_000;

#[derive(Clone, Debug)]
pub struct Edit {
    remove: bool,
    ranges: Vec<(i64, i64)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: i64,
    pub end: i64,
    pub output: i64,
}
impl Edit {
    pub fn parse(params: &Params) -> Result<Self> {
        let remove = match value(params, "edit_mode", "クリップ（残す）") {
            "クリップ（残す）" => false,
            "カット（削除）" => true,
            _ => return Err("編集の操作を選んでください".into()),
        };
        let raw = value(params, "ranges", "0-10");
        if raw.len() > 16_384 {
            return Err("時間範囲が長すぎます".into());
        }
        let mut ranges = Vec::new();
        for entry in raw.split([',', '、', '\n']) {
            let (a, b) = entry
                .trim()
                .split_once('-')
                .ok_or("範囲は開始-終了で指定してください（例: 5-10, 20-30）")?;
            let (a, b) = (time(a)?, time(b)?);
            if a >= b {
                return Err("終了時刻は開始時刻より後にしてください".into());
            }
            ranges.push((a, b));
            if ranges.len() > 100 {
                return Err("時間範囲は100個までです".into());
            }
        }
        ranges.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (a, b) in ranges {
            if let Some(last) = merged.last_mut().filter(|last| a <= last.1) {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        Ok(Self {
            remove,
            ranges: merged,
        })
    }
    pub fn segments(&self, duration: i64) -> Result<Vec<Segment>> {
        if duration <= 0 || self.ranges.iter().any(|(_, b)| *b > duration) {
            return Err(format!(
                "時間範囲は動画の長さ（{:.3}秒）以内にしてください",
                duration as f64 / SECOND as f64
            ));
        }
        let mut keep = Vec::new();
        if self.remove {
            let mut position = 0;
            for &(a, b) in &self.ranges {
                if position < a {
                    keep.push((position, a));
                }
                position = b;
            }
            if position < duration {
                keep.push((position, duration));
            }
        } else {
            keep.clone_from(&self.ranges);
        }
        if keep.is_empty() {
            return Err("すべてのシーンが削除されるため保存できません".into());
        }
        let mut output = 0;
        Ok(keep
            .into_iter()
            .map(|(start, end)| {
                let segment = Segment { start, end, output };
                output += end - start;
                segment
            })
            .collect())
    }
}

fn time(raw: &str) -> Result<i64> {
    let parts: Vec<_> = raw.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err("時刻は秒、分:秒、時:分:秒で指定してください".into());
    }
    let mut ticks = 0i64;
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let (whole, fraction) = part.split_once('.').unwrap_or((part, ""));
        if whole.is_empty()
            || !whole.bytes().all(|c| c.is_ascii_digit())
            || (!last && part.contains('.'))
            || fraction.len() > 7
            || !fraction.bytes().all(|c| c.is_ascii_digit())
        {
            return Err("時刻は非負の数値で、小数は秒に7桁まで指定してください".into());
        }
        let seconds: i64 = whole.parse().map_err(|_| "時刻が大きすぎます")?;
        if i > 0 && seconds >= 60 {
            return Err("分・秒は60未満で指定してください".into());
        }
        ticks = ticks
            .checked_mul(60)
            .and_then(|v| seconds.checked_mul(SECOND).and_then(|s| v.checked_add(s)))
            .ok_or("時刻が大きすぎます")?;
        if !fraction.is_empty() {
            ticks = ticks
                .checked_add(
                    fraction.parse::<i64>().map_err(|_| "時刻が不正です")?
                        * 10i64.pow(7 - fraction.len() as u32),
                )
                .ok_or("時刻が大きすぎます")?;
        }
    }
    Ok(ticks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clips_merge_and_cuts_close_gaps_in_source_order() {
        let mut p = Params::from([("ranges".into(), "00:00:05-10, 8-12, 1.25-2".into())]);
        let clips = Edit::parse(&p).unwrap().segments(20 * SECOND).unwrap();
        assert_eq!(
            clips,
            vec![
                Segment {
                    start: 12_500_000,
                    end: 2 * SECOND,
                    output: 0
                },
                Segment {
                    start: 5 * SECOND,
                    end: 12 * SECOND,
                    output: 7_500_000
                }
            ]
        );
        p.insert("edit_mode".into(), "カット（削除）".into());
        let cuts = Edit::parse(&p).unwrap().segments(20 * SECOND).unwrap();
        assert_eq!(cuts.len(), 3);
        assert_eq!(
            cuts[2],
            Segment {
                start: 12 * SECOND,
                end: 20 * SECOND,
                output: 42_500_000
            }
        );
        assert_eq!(time("01:02:03.0000001").unwrap(), 3723 * SECOND + 1);
    }
    #[test]
    fn rejects_invalid_or_empty_edits_and_out_of_range_times() {
        for ranges in [
            "",
            "2-1",
            "-1-2",
            "NaN-2",
            "0-1:60",
            "0-1.00000000",
            "0-9223372036854775807",
            "1.5:01-90",
        ] {
            assert!(
                Edit::parse(&Params::from([("ranges".into(), ranges.into())])).is_err(),
                "{ranges}"
            );
        }
        let p = Params::from([
            ("ranges".into(), "0-2".into()),
            ("edit_mode".into(), "カット（削除）".into()),
        ]);
        let edit = Edit::parse(&p).unwrap();
        assert!(edit.segments(2 * SECOND).is_err());
        assert!(edit.segments(SECOND).is_err());
    }
}
