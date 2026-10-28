//! Deadline scheduling uses local calendar dates; notification intervals use UTC seconds.
use crate::businessdays::{Adjustment, Calendar};
use chrono::{Datelike, Days, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_INTERVAL: u32 = 15;
pub const MIN_INTERVAL: u32 = 5;
pub const MAX_SNOOZE: u32 = 90;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Once,
    Daily,
    Weekly,
    Monthly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    Alert,
    Windows,
    #[default]
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub due: NaiveDateTime,
    #[serde(default)]
    pub adjustment: Adjustment,
    /// Original recurrence anchor, before moving a deadline to a business day.
    #[serde(default)]
    pub nominal_due: Option<NaiveDateTime>,
    pub memo: String,
    pub interval: u32,
    pub style: Style,
    pub repeat: Repeat,
    /// Preserve the original day when February or a 30-day month is clamped.
    pub monthly_day: u32,
    pub completed: bool,
    pub last_completed: Option<NaiveDateTime>,
    pub last_notified: Option<i64>,
    /// One-shot UTC deadline. Never changes the task's regular notification interval.
    #[serde(default)]
    pub snoozed_until: Option<i64>,
}

impl Task {
    pub fn new(title: String, due: NaiveDateTime) -> Self {
        Self {
            id: 0,
            title,
            due,
            adjustment: Adjustment::None,
            nominal_due: None,
            memo: String::new(),
            interval: DEFAULT_INTERVAL,
            style: Style::Both,
            repeat: Repeat::Once,
            monthly_day: due.day(),
            completed: false,
            last_completed: None,
            last_notified: None,
            snoozed_until: None,
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.title.trim().is_empty() || self.title.chars().count() > 200 {
            return Err(invalid("タイトルは1～200文字で入力してください。"));
        }
        if self.memo.len() > 64 * 1024 {
            return Err(invalid("メモは64 KiBまでです。"));
        }
        if self.interval < MIN_INTERVAL {
            return Err(invalid("通知間隔は5分以上で指定してください。"));
        }
        if !(1..=31).contains(&self.monthly_day)
            || !(1900..=9998).contains(&self.due.year())
            || self
                .nominal_due
                .is_some_and(|d| !(1900..=9998).contains(&d.year()))
        {
            return Err(invalid("期限の年は1900～9998で指定してください。"));
        }
        Ok(())
    }

    pub fn needs_notification(&self, local: NaiveDateTime, utc: i64) -> bool {
        let morning = self.due.date().and_hms_opt(9, 0, 0).unwrap().min(self.due);
        !self.completed
            && local >= morning
            && self.snoozed_until.map_or_else(
                || {
                    self.last_notified.is_none_or(|last| {
                        utc >= last.saturating_add(i64::from(self.interval) * 60)
                    })
                },
                |until| utc >= until,
            )
    }

    fn next_due(&self, after: NaiveDateTime) -> io::Result<NaiveDateTime> {
        let base_due = self.nominal_due.unwrap_or(self.due);
        let date = match self.repeat {
            Repeat::Once => return Ok(self.due),
            Repeat::Daily | Repeat::Weekly => {
                let step = if self.repeat == Repeat::Daily { 1 } else { 7 };
                let elapsed = (after.date() - base_due.date()).num_days().max(0) as u64;
                let mut date = base_due
                    .date()
                    .checked_add_days(Days::new((elapsed / step) * step))
                    .ok_or_else(|| invalid("次回の期限を計算できません。"))?;
                if date.and_time(base_due.time()) <= after || date <= base_due.date() {
                    date = date
                        .checked_add_days(Days::new(step))
                        .ok_or_else(|| invalid("次回の期限を計算できません。"))?;
                }
                date
            }
            Repeat::Monthly => {
                let base = base_due.year() * 12 + base_due.month0() as i32;
                let mut month = (after.year() * 12 + after.month0() as i32).max(base + 1);
                loop {
                    let year = month.div_euclid(12);
                    let m = month.rem_euclid(12) as u32 + 1;
                    let date = (1..=self.monthly_day)
                        .rev()
                        .find_map(|day| NaiveDate::from_ymd_opt(year, m, day))
                        .ok_or_else(|| invalid("次回の期限を計算できません。"))?;
                    if date.and_time(base_due.time()) > after {
                        break date;
                    }
                    month += 1;
                }
            }
        };
        let due = date.and_time(base_due.time());
        if due.year() > 9998 {
            return Err(invalid("次回の期限が範囲を超えています。"));
        }
        Ok(due)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Clone, Serialize, Deserialize)]
struct Data {
    version: u32,
    tasks: Vec<Task>,
    next_id: u64,
    #[serde(default)]
    calendar: Calendar,
}

pub struct Store {
    path: PathBuf,
    data: Data,
}

impl Store {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Data>(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Data {
                version: 1,
                tasks: Vec::new(),
                next_id: 1,
                calendar: Calendar::default(),
            },
            Err(e) => return Err(e),
        };
        if data.version != 1 || data.next_id == 0 {
            return Err(invalid("対応していない期限データ形式です。"));
        }
        data.calendar.validate()?;
        let mut ids = std::collections::HashSet::new();
        for t in &data.tasks {
            t.validate()?;
            if t.id == 0 || t.id >= data.next_id || !ids.insert(t.id) {
                return Err(invalid("期限データのIDが不正です。"));
            }
        }
        Ok(Self { path, data })
    }

    pub fn tasks(&self) -> &[Task] {
        &self.data.tasks
    }
    pub fn calendar(&self) -> &Calendar {
        &self.data.calendar
    }

    /// Replace the calendar and recalculate unfinished tasks as one atomic save.
    pub fn register_calendar(&mut self, calendar: Calendar) -> io::Result<()> {
        calendar.validate()?;
        let mut data = self.data.clone();
        data.calendar = calendar;
        for task in &mut data.tasks {
            if !task.completed {
                let old_due = task.due;
                apply_calendar(task, &data.calendar)?;
                if task.due != old_due {
                    task.last_notified = None;
                    task.snoozed_until = None;
                }
            }
        }
        self.commit(data)
    }

    /// A new resident session immediately reminds about outstanding work.
    pub fn restart_notifications(&mut self) {
        for task in &mut self.data.tasks {
            if !task.completed {
                task.last_notified = None;
            }
        }
    }

    fn commit(&mut self, data: Data) -> io::Result<()> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, &data).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| e.error)?;
        self.data = data;
        Ok(())
    }

    pub fn save(&mut self, mut task: Task) -> io::Result<u64> {
        task.title = task.title.trim().to_owned();
        task.validate()?;
        apply_calendar(&mut task, &self.data.calendar)?;
        let mut data = self.data.clone();
        if task.id == 0 {
            task.id = data.next_id;
            data.next_id = data
                .next_id
                .checked_add(1)
                .ok_or_else(|| invalid("IDの上限です。"))?;
            data.tasks.push(task.clone());
        } else {
            let old = data
                .tasks
                .iter_mut()
                .find(|t| t.id == task.id)
                .ok_or_else(|| invalid("対象がありません。"))?;
            *old = task.clone();
        }
        self.commit(data)?;
        Ok(task.id)
    }

    pub fn complete(&mut self, id: u64, now: NaiveDateTime) -> io::Result<()> {
        let mut task = self
            .data
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| invalid("対象がありません。"))?;
        if task.completed {
            return Ok(());
        }
        if task.repeat == Repeat::Once {
            task.completed = true;
        } else {
            let cutoff = now.max(task.due);
            let mut cursor = now;
            let mut next = None;
            for _ in 0..3700 {
                let nominal = task.next_due(cursor)?;
                let adjusted = self.data.calendar.adjust(nominal, task.adjustment)?;
                if adjusted > cutoff {
                    next = Some(nominal);
                    break;
                }
                cursor = nominal.max(cursor);
                if task.adjustment == Adjustment::Previous {
                    // Jump past a run of closures instead of scanning every occurrence.
                    cursor = cursor.max(
                        self.data.calendar.adjust(nominal, Adjustment::Next)?
                            - chrono::Duration::minutes(1),
                    );
                }
            }
            let nominal = next.ok_or_else(|| {
                invalid("次回の営業日期限を計算できません。CSVを確認してください。")
            })?;
            task.due = nominal;
            task.nominal_due = Some(nominal);
        }
        task.last_completed = Some(now);
        task.last_notified = None;
        task.snoozed_until = None;
        self.save(task).map(|_| ())
    }

    pub fn remove(&mut self, id: u64) -> io::Result<()> {
        let mut data = self.data.clone();
        data.tasks.retain(|t| t.id != id);
        self.commit(data)
    }

    pub fn notified(&mut self, ids: &[u64], utc: i64) -> io::Result<()> {
        let mut data = self.data.clone();
        for task in &mut data.tasks {
            if ids.contains(&task.id) {
                task.last_notified = Some(utc);
                task.snoozed_until = None;
            }
        }
        self.commit(data)
    }
    pub fn snooze(&mut self, ids: &[u64], minutes: u32, utc: i64) -> io::Result<()> {
        if !(1..=MAX_SNOOZE).contains(&minutes) {
            return Err(invalid("次回通知は1～90分後で指定してください。"));
        }
        let until = utc
            .checked_add(i64::from(minutes) * 60)
            .ok_or_else(|| invalid("次回通知時刻を計算できません。"))?;
        let mut data = self.data.clone();
        let mut changed = false;
        for task in &mut data.tasks {
            if !task.completed && ids.contains(&task.id) {
                task.snoozed_until = Some(until);
                changed = true;
            }
        }
        if !changed {
            return Err(invalid("延期する未完了の作業がありません。"));
        }
        self.commit(data)
    }
}

fn apply_calendar(task: &mut Task, calendar: &Calendar) -> io::Result<()> {
    let nominal = task.nominal_due.unwrap_or(task.due);
    task.due = calendar.adjust(nominal, task.adjustment)?;
    task.nominal_due = (task.adjustment != Adjustment::None).then_some(nominal);
    task.validate()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }
    #[test]
    fn morning_interval_overdue_and_completion_are_independent() {
        let mut t = Task::new("提出".into(), date("2026-10-27 18:00"));
        assert!(!t.needs_notification(date("2026-10-27 08:59"), 1000));
        assert!(t.needs_notification(date("2026-10-27 09:00"), 1000));
        t.last_notified = Some(1000);
        assert!(!t.needs_notification(date("2026-10-28 09:00"), 1899));
        assert!(t.needs_notification(date("2026-10-28 09:00"), 1900));
        t.completed = true;
        assert!(!t.needs_notification(date("2026-10-29 09:00"), 99999));
        t.interval = 4;
        assert!(t.validate().is_err());
    }
    #[test]
    fn recurrence_preserves_calendar_and_month_end_anchor() {
        let mut t = Task::new("月末".into(), date("2028-01-31 10:30"));
        t.repeat = Repeat::Monthly;
        assert_eq!(t.next_due(t.due).unwrap(), date("2028-02-29 10:30"));
        t.due = t.next_due(t.due).unwrap();
        assert_eq!(t.next_due(t.due).unwrap(), date("2028-03-31 10:30"));
        t.repeat = Repeat::Weekly;
        assert_eq!(
            t.next_due(date("2028-03-20 12:00")).unwrap(),
            date("2028-03-21 10:30")
        );
        t.repeat = Repeat::Daily;
        assert_eq!(
            t.next_due(date("2028-03-20 12:00")).unwrap(),
            date("2028-03-21 10:30")
        );
    }
    #[test]
    fn persistence_ids_completion_and_notification_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("期限.json");
        let mut s = Store::open(path.clone()).unwrap();
        let task = Task::new("日本語😀".into(), date("2026-10-27 18:00"));
        let id = s.save(task).unwrap();
        s.notified(&[id], 100).unwrap();
        let mut s = Store::open(path.clone()).unwrap();
        assert_eq!(s.tasks()[0].last_notified, Some(100));
        s.restart_notifications();
        assert!(s.tasks()[0].needs_notification(date("2026-10-27 10:00"), 101));
        s.complete(id, date("2026-10-27 19:00")).unwrap();
        let mut s = Store::open(path).unwrap();
        assert!(s.tasks()[0].completed);
        assert!(
            s.save(Task::new("".into(), date("2026-10-27 20:00")))
                .is_err()
        );
        s.remove(id).unwrap();
        assert!(s.tasks().is_empty());
    }

    #[test]
    fn recurring_completion_skips_missed_occurrences_and_keeps_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("tasks.json")).unwrap();
        let mut task = Task::new("週次作業".into(), date("2026-10-05 08:00"));
        task.repeat = Repeat::Weekly;
        task.interval = 5;
        task.memo = "保存するメモ".into();
        let id = store.save(task).unwrap();
        store.complete(id, date("2026-10-27 10:00")).unwrap();
        let task = &store.tasks()[0];
        assert_eq!(task.due, date("2026-11-02 08:00"));
        assert!(!task.completed);
        assert_eq!(task.interval, 5);
        assert_eq!(task.memo, "保存するメモ");
        assert!(!task.needs_notification(date("2026-11-02 07:59"), 10));
        assert!(task.needs_notification(date("2026-11-02 08:00"), 10));
    }

    #[test]
    fn invalid_file_and_failed_write_never_replace_saved_or_in_memory_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        std::fs::write(&path, b"broken json").unwrap();
        assert!(Store::open(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken json");
        let target = dir.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        let mut store = Store {
            path: target,
            data: Data {
                version: 1,
                tasks: Vec::new(),
                next_id: 1,
                calendar: Calendar::default(),
            },
        };
        assert!(
            store
                .save(Task::new("保存失敗".into(), date("2026-10-27 10:00")))
                .is_err()
        );
        assert!(store.tasks().is_empty());
    }
    #[test]
    fn business_day_policies_persist_and_calendar_updates_recalculate_original_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let mut store = Store::open(path.clone()).unwrap();
        let mut task = Task::new("祝日の期限".into(), date("2026-11-03 18:37"));
        task.adjustment = Adjustment::Previous;
        let id = store.save(task).unwrap();
        assert_eq!(store.tasks()[0].due, date("2026-11-02 18:37"));
        let custom = Calendar::from_csv(b"2026-11-02\n2026-11-03\n", "company".into()).unwrap();
        store.register_calendar(custom).unwrap();
        assert_eq!(store.tasks()[0].due, date("2026-10-30 18:37"));
        let mut store = Store::open(path).unwrap();
        let mut task = store.tasks()[0].clone();
        assert_eq!(task.id, id);
        assert_eq!(task.adjustment, Adjustment::Previous);
        assert_eq!(task.nominal_due, Some(date("2026-11-03 18:37")));
        task.adjustment = Adjustment::Next;
        store.save(task).unwrap();
        assert_eq!(store.tasks()[0].due, date("2026-11-04 18:37"));
        let mut task = store.tasks()[0].clone();
        task.adjustment = Adjustment::None;
        store.save(task).unwrap();
        assert_eq!(store.tasks()[0].due, date("2026-11-03 18:37"));
    }
    #[test]
    fn business_day_recurrence_keeps_weekday_and_month_end_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path().join("tasks.json")).unwrap();
        let mut t = Task::new("週次".into(), date("2026-10-31 09:30"));
        t.repeat = Repeat::Weekly;
        t.adjustment = Adjustment::Next;
        let id = s.save(t).unwrap();
        assert_eq!(s.tasks()[0].due, date("2026-11-02 09:30"));
        s.complete(id, date("2026-11-02 10:00")).unwrap();
        assert_eq!(s.tasks()[0].due, date("2026-11-09 09:30"));
        assert_eq!(s.tasks()[0].nominal_due, Some(date("2026-11-07 09:30")));
        let mut t = Task::new("月末".into(), date("2027-01-31 18:00"));
        t.repeat = Repeat::Monthly;
        t.adjustment = Adjustment::Previous;
        let id = s.save(t).unwrap();
        assert_eq!(s.tasks()[1].due, date("2027-01-29 18:00"));
        s.complete(id, date("2027-01-29 19:00")).unwrap();
        assert_eq!(s.tasks()[1].due, date("2027-02-26 18:00"));
        s.complete(id, date("2027-02-26 19:00")).unwrap();
        assert_eq!(s.tasks()[1].due, date("2027-03-31 18:00"));
        let mut t = Task::new("日次".into(), date("2026-10-31 18:00"));
        t.repeat = Repeat::Daily;
        t.adjustment = Adjustment::Next;
        let id = s.save(t).unwrap();
        s.complete(id, date("2026-11-02 19:00")).unwrap();
        assert_eq!(s.tasks()[2].due, date("2026-11-04 18:00"));
    }
    #[test]
    fn legacy_data_and_failed_calendar_replacement_preserve_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let mut s = Store::open(path.clone()).unwrap();
        let mut t = Task::new("補正対象".into(), date("2026-11-03 10:00"));
        t.adjustment = Adjustment::Next;
        s.save(t).unwrap();
        let before = std::fs::read(&path).unwrap();
        let unavailable = Calendar::from_csv(b"2030-01-01\n", "future".into()).unwrap();
        assert!(s.register_calendar(unavailable).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(s.tasks()[0].due, date("2026-11-04 10:00"));
        let mut json: serde_json::Value = serde_json::from_slice(&before).unwrap();
        json.as_object_mut().unwrap().remove("calendar");
        for t in json["tasks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("adjustment");
            t.as_object_mut().unwrap().remove("nominal_due");
        }
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let legacy = Store::open(path).unwrap();
        assert_eq!(legacy.tasks()[0].adjustment, Adjustment::None);
        assert_eq!(legacy.calendar().holidays.len(), 108);
    }
    #[test]
    fn one_shot_snooze_survives_restart_then_resumes_original_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let mut s = Store::open(path.clone()).unwrap();
        let id = s
            .save(Task::new("延期する作業".into(), date("2026-10-27 10:00")))
            .unwrap();
        let other = s
            .save(Task::new("別の作業".into(), date("2026-10-27 10:00")))
            .unwrap();
        s.notified(&[id, other], 1000).unwrap();
        s.snooze(&[id], 90, 1100).unwrap();
        assert_eq!(s.tasks()[0].interval, 15);
        assert_eq!(s.tasks()[0].snoozed_until, Some(6500));
        assert!(!s.tasks()[0].needs_notification(date("2026-10-28 12:00"), 6499));
        assert!(s.tasks()[1].needs_notification(date("2026-10-28 12:00"), 1900));
        let mut s = Store::open(path).unwrap();
        s.restart_notifications();
        assert!(!s.tasks()[0].needs_notification(date("2026-10-28 12:00"), 6499));
        assert!(s.tasks()[0].needs_notification(date("2026-10-28 12:00"), 6500));
        s.notified(&[id], 6500).unwrap();
        assert_eq!(s.tasks()[0].snoozed_until, None);
        assert!(!s.tasks()[0].needs_notification(date("2026-10-28 12:00"), 7399));
        assert!(s.tasks()[0].needs_notification(date("2026-10-28 12:00"), 7400));
    }
    #[test]
    fn snooze_is_bounded_atomic_and_cleared_on_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        let mut s = Store::open(path.clone()).unwrap();
        let mut t = Task::new("定期作業".into(), date("2026-10-27 10:00"));
        t.interval = 120;
        t.repeat = Repeat::Daily;
        let id = s.save(t).unwrap();
        s.notified(&[id], 1000).unwrap();
        let before = std::fs::read(&path).unwrap();
        for minutes in [0, 91, u32::MAX] {
            assert!(s.snooze(&[id], minutes, 1000).is_err());
        }
        assert!(s.snooze(&[999], 30, 1000).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let broken = dir.path().join("directory");
        std::fs::create_dir(&broken).unwrap();
        s.path = broken;
        assert!(s.snooze(&[id], 90, 1000).is_err());
        assert_eq!(s.tasks()[0].snoozed_until, None);
        s.path = path;
        s.snooze(&[id], 1, 1000).unwrap();
        assert!(!s.tasks()[0].needs_notification(date("2026-10-27 12:00"), 1059));
        assert!(s.tasks()[0].needs_notification(date("2026-10-27 12:00"), 1060));
        s.complete(id, date("2026-10-27 11:00")).unwrap();
        assert_eq!(s.tasks()[0].snoozed_until, None);
        assert_eq!(s.tasks()[0].interval, 120);
    }
}
