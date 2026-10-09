//! 裏の処理（中身の索引の作成・それを使う中身の検索・索引のバキューム）が、利用者が許した CPU とメモリの
//! 上限をなるべく守るための部品。
//!
//! * CPU: スレッドの数を「CPU の数 × 許した割合」に絞り、Windows ではスレッドをバックグラウンド モード
//!   （`THREAD_MODE_BACKGROUND_BEGIN`。CPU・ディスク・メモリの優先度を下げる）にする。
//! * メモリ: 同時に読むファイルの大きさの見積もりの合計を予算に収める（[`MemoryBudget`]）。予算より大きな
//!   ファイルは、ほかに読んでいるものがないときに 1 つだけ読む（上限を超えるが、読めないよりはよい）。

use std::sync::{Condvar, Mutex};

/// 裏の処理の上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// 使ってよい CPU の割合（1〜100）
    pub cpu_percent: u32,
    /// 使ってよいメモリ（バイト。0 は上限なし）
    pub memory: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            cpu_percent: 25,
            memory: 512 << 20,
        }
    }
}

impl Limits {
    /// 使ってよいスレッドの数（CPU の数 × 割合。少なくとも 1）。`max` を超えない。
    pub fn threads(&self, max: usize) -> usize {
        let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
        threads_for(cpus, self.cpu_percent).min(max.max(1))
    }
}

fn threads_for(cpus: usize, percent: u32) -> usize {
    let p = percent.clamp(1, 100) as usize;
    (cpus * p / 100).max(1)
}

/// 今のスレッドの優先度を下げる（Windows のバックグラウンド モード。ほかの OS では何もしない）。
pub fn enter_background() {
    #[cfg(windows)]
    background_windows();
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 のスレッドの優先度（このクレートで unsafe を使うのは ACL・ここ・試験だけ）
fn background_windows() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };
    // 失敗しても（既にバックグラウンドなど）続ける
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// メモリの予算（バイト）。
pub struct MemoryBudget {
    limit: u64,
    used: Mutex<u64>,
    freed: Condvar,
}

/// 予算から借りた分（手放すと返す）。
pub struct Lease<'a> {
    budget: &'a MemoryBudget,
    n: u64,
}

impl MemoryBudget {
    /// `limit` が 0 なら上限なし。
    pub fn new(limit: u64) -> MemoryBudget {
        MemoryBudget {
            limit,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// `n` バイトを借りる（空くまで待つ）。予算より大きければ、ほかに借りているものがなくなるまで待って
    /// 1 つだけ借りる。`stop` が `true` を返したら待つのをやめて `None`。
    pub fn acquire(&self, n: u64, stop: &dyn Fn() -> bool) -> Option<Lease<'_>> {
        if self.limit == 0 {
            return Some(Lease { budget: self, n: 0 });
        }
        let mut used = self.used.lock().unwrap();
        loop {
            if stop() {
                return None;
            }
            if *used == 0 || *used + n <= self.limit {
                *used += n;
                return Some(Lease { budget: self, n });
            }
            used = self
                .freed
                .wait_timeout(used, std::time::Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }

    /// 今借りている大きさ。
    pub fn used(&self) -> u64 {
        *self.used.lock().unwrap()
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if self.n > 0 {
            let mut used = self.budget.used.lock().unwrap();
            *used -= self.n;
            self.budget.freed.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn threads_follow_the_cpu_share() {
        assert_eq!(threads_for(8, 25), 2);
        assert_eq!(threads_for(8, 100), 8);
        assert_eq!(threads_for(4, 10), 1);
        assert_eq!(threads_for(4, 0), 1);
        assert_eq!(threads_for(16, 250), 16);
        let l = Limits {
            cpu_percent: 100,
            memory: 0,
        };
        assert!(l.threads(2) <= 2);
        enter_background(); // 落ちない
    }

    #[test]
    fn budget_keeps_the_total_under_the_limit() {
        let b = MemoryBudget::new(100);
        let peak = AtomicU64::new(0);
        std::thread::scope(|s| {
            for i in 0..16u64 {
                let (b, peak) = (&b, &peak);
                s.spawn(move || {
                    let n = 10 + (i % 4) * 15; // 10〜55
                    let _l = b.acquire(n, &|| false).unwrap();
                    peak.fetch_max(b.used(), Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                });
            }
        });
        assert!(peak.load(Ordering::Relaxed) <= 100);
        assert_eq!(b.used(), 0);
        // 予算より大きいものは 1 つだけ
        let big = b.acquire(500, &|| false).unwrap();
        assert_eq!(b.used(), 500);
        assert!(b.acquire(1, &|| true).is_none()); // 待っている間に止めた
        drop(big);
        assert_eq!(b.used(), 0);
        // 上限なし
        let free = MemoryBudget::new(0);
        let _a = free.acquire(1 << 40, &|| false).unwrap();
        assert_eq!(free.used(), 0);
    }
}
