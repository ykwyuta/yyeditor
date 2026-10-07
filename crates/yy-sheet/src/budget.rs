//! メモリの予算（15 章 3.6）。
//!
//! アプリが使うメモリを `limit`（既定 8 GB）に抑えるため、メモリを使う部品ごとに予算を配る。
//! 部品は使った量を数え、予算を超えたら自分のキャッシュを捨てるか、作業を区切る。

use std::sync::atomic::{AtomicU64, Ordering};

/// 既定の上限（8 GB）。
pub const DEFAULT_LIMIT: u64 = 8 << 30;

/// 予算の部品。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// 固定（目次・統計・数式・書式・差分）
    Fixed,
    /// チャンクのキャッシュ（読んで展開したチャンク）
    Cache,
    /// 表示の状態（行番号の並び・絞り込みのビット列）
    View,
    /// 作業領域（並べ替え・集計・取り込み・書き出し）
    Work,
    /// 索引
    Index,
    /// 余裕
    Spare,
}

impl Part {
    pub const ALL: [Part; 6] = [
        Part::Fixed,
        Part::Cache,
        Part::View,
        Part::Work,
        Part::Index,
        Part::Spare,
    ];

    /// 全体に対する割合（8 GB で 0.5・2.5・0.5・2.5・1.5・0.5 GB）。
    fn share16(self) -> u64 {
        match self {
            Part::Fixed | Part::View | Part::Spare => 1,
            Part::Cache | Part::Work => 5,
            Part::Index => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Part::Fixed => "固定",
            Part::Cache => "チャンクのキャッシュ",
            Part::View => "表示の状態",
            Part::Work => "作業領域",
            Part::Index => "索引",
            Part::Spare => "余裕",
        }
    }
}

/// 予算と使用量。
#[derive(Debug)]
pub struct Budget {
    limit: AtomicU64,
    used: [AtomicU64; 6],
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(DEFAULT_LIMIT)
    }
}

impl Budget {
    pub fn new(limit: u64) -> Budget {
        Budget {
            limit: AtomicU64::new(limit),
            used: Default::default(),
        }
    }

    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }

    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
    }

    /// 部品の予算。
    pub fn of(&self, part: Part) -> u64 {
        self.limit() / 16 * part.share16()
    }

    pub fn used(&self, part: Part) -> u64 {
        self.used[part as usize].load(Ordering::Relaxed)
    }

    pub fn total_used(&self) -> u64 {
        Part::ALL.iter().map(|&p| self.used(p)).sum()
    }

    pub fn add(&self, part: Part, bytes: u64) {
        self.used[part as usize].fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn sub(&self, part: Part, bytes: u64) {
        let _ = self.used[part as usize].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |u| {
            Some(u.saturating_sub(bytes))
        });
    }

    /// `bytes` を足しても部品の予算に収まるか。
    pub fn fits(&self, part: Part, bytes: u64) -> bool {
        self.used(part) + bytes <= self.of(part)
    }

    /// 使用量を数える札を作る（落とすと戻す）。
    pub fn lease(&self, part: Part, bytes: u64) -> Lease<'_> {
        self.add(part, bytes);
        Lease {
            budget: self,
            part,
            bytes,
        }
    }
}

/// 使用量の札。
#[derive(Debug)]
pub struct Lease<'a> {
    budget: &'a Budget,
    part: Part,
    bytes: u64,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.budget.sub(self.part, self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_add_up() {
        let b = Budget::default();
        let total: u64 = Part::ALL.iter().map(|&p| b.of(p)).sum();
        assert_eq!(total, DEFAULT_LIMIT);
        assert_eq!(b.of(Part::Cache), 2_684_354_560);
        {
            let _l = b.lease(Part::Work, 100);
            assert_eq!(b.used(Part::Work), 100);
        }
        assert_eq!(b.used(Part::Work), 0);
    }
}
