//! 切断を模擬した転送のテスト（手元の sftp-server・scp を使う）。

use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;
use std::sync::atomic::AtomicI64;

use super::*;
use crate::local::{LocalTransport, sftp_server};
use crate::uri::Target;
use crate::{Process, Transport};

/// 決めた量を送受信したら切れる接続。
struct Faulty {
    inner: LocalTransport,
    budget: Arc<AtomicI64>,
    closed: Arc<AtomicBool>,
}

struct LimitedWriter {
    inner: Box<dyn Write + Send>,
    budget: Arc<AtomicI64>,
    closed: Arc<AtomicBool>,
}

impl Write for LimitedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::Relaxed)
            || self.budget.fetch_sub(buf.len() as i64, Ordering::Relaxed) < buf.len() as i64
        {
            self.closed.store(true, Ordering::Relaxed);
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "（模擬）回線が切れました",
            ));
        }
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct LimitedReader {
    inner: Box<dyn Read + Send>,
    budget: Arc<AtomicI64>,
    closed: Arc<AtomicBool>,
}

impl Read for LimitedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "（模擬）回線が切れました",
            ));
        }
        let n = self.inner.read(buf)?;
        if self.budget.fetch_sub(n as i64, Ordering::Relaxed) < n as i64 {
            self.closed.store(true, Ordering::Relaxed);
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "（模擬）回線が切れました",
            ));
        }
        Ok(n)
    }
}

impl Faulty {
    fn new(budget: i64) -> Faulty {
        Faulty {
            inner: LocalTransport::new(),
            budget: Arc::new(AtomicI64::new(budget)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn wrap(&self, p: io::Result<Process>) -> io::Result<Process> {
        if self.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "（模擬）切れています",
            ));
        }
        let (stdin, stdout, finish) = p?.into_parts();
        Ok(Process::new(
            Box::new(LimitedWriter {
                inner: stdin,
                budget: self.budget.clone(),
                closed: self.closed.clone(),
            }),
            Box::new(LimitedReader {
                inner: stdout,
                budget: self.budget.clone(),
                closed: self.closed.clone(),
            }),
            finish,
        ))
    }
}

impl Transport for Faulty {
    fn exec(&self, command: &[u8]) -> io::Result<Process> {
        self.wrap(self.inner.exec(command))
    }
    fn subsystem(&self, name: &str) -> io::Result<Process> {
        self.wrap(self.inner.subsystem(name))
    }
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

fn tools(protocol: Protocol) -> bool {
    let ok = match protocol {
        Protocol::Sftp => sftp_server().is_some(),
        Protocol::Scp => std::process::Command::new("sh")
            .args(["-c", "command -v scp"])
            .output()
            .is_ok_and(|o| o.status.success()),
    };
    if !ok {
        eprintln!("{} の道具がないため飛ばします", protocol.name());
    }
    ok
}

fn data(n: usize) -> Vec<u8> {
    let mut x = 0x1234_5678u32;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn uri(path: &Path) -> RemoteUri {
    RemoteUri {
        user: None,
        host: "test".into(),
        port: None,
        path: path.as_os_str().as_bytes().to_vec(),
    }
}

/// 転送を動かす（接続ごとに `budgets` の量で切れる。使い切ったら切れない）。
struct Harness {
    log: TransferLog,
    lines: Arc<Mutex<Vec<String>>>,
    budgets: Mutex<Vec<i64>>,
    connections: AtomicI64,
    journal: Journal,
}

impl Harness {
    fn new(dir: &Path, budgets: &[i64]) -> Harness {
        let log = TransferLog::new(Some(dir.join("transfer.log")), || "T".into());
        let lines = Arc::new(Mutex::new(Vec::new()));
        let l = lines.clone();
        log.set_sink(Box::new(move |s| l.lock().unwrap().push(s.to_owned())));
        Harness {
            log,
            lines,
            budgets: Mutex::new(budgets.to_vec()),
            connections: AtomicI64::new(0),
            journal: Journal::new(dir.join("journal")),
        }
    }

    fn run(&self, job: &mut Job, attempts: u32, scp_chunk: u64) {
        self.run_with(job, attempts, scp_chunk, &mut |_| {});
    }

    fn run_with(
        &self,
        job: &mut Job,
        attempts: u32,
        scp_chunk: u64,
        progress: &mut dyn FnMut(&Job),
    ) {
        let connect = |_: &TransferLog, _: u64| -> io::Result<Arc<dyn Transport>> {
            self.connections.fetch_add(1, Ordering::Relaxed);
            let mut b = self.budgets.lock().unwrap();
            let budget = if b.is_empty() { i64::MAX } else { b.remove(0) };
            Ok(Arc::new(Faulty::new(budget)) as Arc<dyn Transport>)
        };
        let cancel = AtomicBool::new(false);
        let mut cx = Context {
            connect: &connect,
            log: &self.log,
            journal: Some(&self.journal),
            cancel: &cancel,
            progress,
            retry: Retry {
                attempts,
                delays: vec![Duration::ZERO],
            },
            scp_chunk,
            transport: None,
        };
        run(job, &mut cx);
    }

    fn logged(&self, needle: &str) -> bool {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains(needle))
    }

    fn dump(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

fn upload(protocol: Protocol, size: usize, budgets: &[i64], chunk: u64) {
    if !tools(protocol) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    let content = data(size);
    std::fs::write(&src, &content).unwrap();
    let dst = dir.path().join("remote/sub/big.bin");
    let h = Harness::new(dir.path(), budgets);
    let mut job = upload_job(7, protocol, &src, uri(&dst), false).unwrap();
    h.run(&mut job, 5, chunk);
    assert_eq!(job.state, State::Done, "{}", h.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(!dir.path().join("remote/sub/big.bin.yypart").exists());
    assert!(h.logged("接続が切れました"), "{}", h.dump());
    assert!(h.logged("レジューム"), "{}", h.dump());
    assert!(h.logged(&format!("確認: 接続先の大きさ {size} バイトが一致しました")));
    assert!(h.connections.load(Ordering::Relaxed) as usize > budgets.len());
    // 完了したらジャーナルから消える
    assert!(h.journal.load().is_empty());
    // 記録はファイルにも残る
    let text = std::fs::read_to_string(dir.path().join("transfer.log")).unwrap();
    assert!(text.contains("[#7] 完了しました"), "{text}");
    // 更新日時も合わせる
    let m = |p: &Path| mtime_of(&std::fs::metadata(p).unwrap());
    assert_eq!(m(&dst), m(&src));
}

#[test]
fn sftp_upload_resumes_after_disconnects() {
    // SFTP は 2 MiB まで応答を待たずに送るので、それより後で切る（切れる前の応答で位置が確定する）
    upload(Protocol::Sftp, 9_000_000, &[3_000_000, 3_000_000], 0);
}

#[test]
fn scp_upload_resumes_after_disconnects() {
    upload(Protocol::Scp, 3_000_000, &[700_000, 900_000], 256 << 10);
}

fn download(protocol: Protocol) {
    if !tools(protocol) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("remote.bin");
    let content = data(2_500_000);
    std::fs::write(&src, &content).unwrap();
    let meta = std::fs::metadata(&src).unwrap();
    let dst = dir.path().join("local/remote.bin");
    let h = Harness::new(dir.path(), &[600_000, 800_000]);
    let mut job = download_job(
        3,
        protocol,
        uri(&src),
        meta.len(),
        mtime_of(&meta),
        &dst,
        false,
    );
    h.run(&mut job, 5, 0);
    assert_eq!(job.state, State::Done, "{}", h.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(!job.local_part().exists());
    assert!(
        h.logged("接続が切れました") && h.logged("レジューム"),
        "{}",
        h.dump()
    );
}

#[test]
fn sftp_download_resumes_after_disconnects() {
    download(Protocol::Sftp);
}

#[test]
fn scp_download_resumes_after_disconnects() {
    download(Protocol::Scp);
}

#[test]
fn resumes_from_the_journal_after_a_restart_and_checks_the_tail() {
    if !tools(Protocol::Sftp) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.bin");
    // 並べて送る量（32 KiB × 64 = 2 MiB）より後で切る: 切れたときには、それより前の分の応答が
    // 必ず届いている（確定した位置が 0 にならない）
    let content = data(8_000_000);
    std::fs::write(&src, &content).unwrap();
    let dst = dir.path().join("a-remote.bin");
    // 再接続しない設定で切れる → 中断としてジャーナルに残る
    let h = Harness::new(dir.path(), &[6_000_000]);
    let mut job = upload_job(1, Protocol::Sftp, &src, uri(&dst), false).unwrap();
    h.run(&mut job, 0, 0);
    assert_eq!(job.state, State::Interrupted, "{}", h.dump());
    let saved = h.journal.load();
    assert_eq!(saved.len(), 1);
    assert!(
        saved[0].done > 0 && saved[0].done < 8_000_000,
        "{}",
        h.dump()
    );
    assert_eq!(saved[0].state, State::Interrupted);

    // アプリを起動し直した: ジャーナルから続ける
    let h2 = Harness::new(dir.path(), &[]);
    let mut job = h2.journal.load().remove(0);
    h2.run(&mut job, 3, 0);
    assert_eq!(job.state, State::Done, "{}", h2.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(h2.logged("照合: 続ける位置の直前の"), "{}", h2.dump());

    // 途中のファイルの中身が違えば最初から送る
    std::fs::remove_file(&dst).unwrap();
    let h3 = Harness::new(dir.path(), &[6_000_000]);
    let mut job = upload_job(2, Protocol::Sftp, &src, uri(&dst), false).unwrap();
    h3.run(&mut job, 0, 0);
    let mut job = h3.journal.load().into_iter().find(|j| j.id == 2).unwrap();
    // 確定した位置の直前（照合する範囲）を書き換える
    let part = dir.path().join("a-remote.bin.yypart");
    let mut bytes = std::fs::read(&part).unwrap();
    bytes[job.done as usize - 10] ^= 0xff;
    std::fs::write(&part, bytes).unwrap();
    h3.run(&mut job, 3, 0);
    assert_eq!(job.state, State::Done);
    assert!(h3.logged("最初から送ります"), "{}", h3.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
}

#[test]
fn refuses_when_the_source_changed_or_the_target_exists() {
    if !tools(Protocol::Sftp) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s.bin");
    std::fs::write(&src, data(500_000)).unwrap();
    let dst = dir.path().join("d.bin");
    std::fs::write(&dst, b"old").unwrap();
    let h = Harness::new(dir.path(), &[]);
    let mut job = upload_job(5, Protocol::Sftp, &src, uri(&dst), false).unwrap();
    h.run(&mut job, 3, 0);
    assert_eq!(job.state, State::Failed);
    assert!(job.message.contains("既にあります"), "{}", job.message);
    // 置き換えを選べば送る
    job.overwrite = true;
    h.run(&mut job, 3, 0);
    assert_eq!(job.state, State::Done, "{}", h.dump());
    // 送り始めた後で手元のファイルが変わった
    let mut job = upload_job(6, Protocol::Sftp, &src, uri(&dst), true).unwrap();
    std::fs::write(&src, data(10)).unwrap();
    h.run(&mut job, 3, 0);
    assert_eq!(job.state, State::Failed);
    assert!(job.message.contains("変更されました"), "{}", job.message);
}

#[test]
fn journal_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::new(dir.path());
    let job = Job {
        id: 9,
        direction: Direction::Download,
        protocol: Protocol::Scp,
        local: PathBuf::from("/tmp/a%b\nc"),
        remote: RemoteUri {
            user: Some("u".into()),
            host: "h".into(),
            port: Some(2222),
            path: b"/x y".to_vec(),
        },
        size: 10,
        mtime: 20,
        done: 5,
        current: 7,
        state: State::Running,
        message: "m".into(),
        overwrite: true,
    };
    j.save(&job).unwrap();
    let back = j.load().remove(0);
    assert_eq!(back.local, job.local);
    assert_eq!(back.remote.to_string(), job.remote.to_string());
    assert_eq!(back.remote.target(), Target::parse("u@h:2222").unwrap());
    assert_eq!((back.size, back.mtime, back.done), (10, 20, 5));
    assert_eq!(back.state, State::Interrupted);
    assert_eq!(back.protocol, Protocol::Scp);
    assert!(back.overwrite);
    assert_eq!(j.next_id(), 10);
    let mut done = job.clone();
    done.state = State::Done;
    j.save(&done).unwrap();
    assert!(j.load().is_empty());
}

/// 転送の途中でアプリが落ちた（進みの知らせの中で panic する。後片付けをせずに抜ける）。
/// 進みをゆっくりにして、ジャーナルに確かな位置が書かれてから落とす。
fn crash_midway(h: &Harness, job: &mut Job, scp_chunk: u64) {
    let start = Instant::now();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.run_with(job, 0, scp_chunk, &mut |j: &Job| {
            std::thread::sleep(Duration::from_millis(250));
            if j.state == State::Running && start.elapsed() > Duration::from_millis(2500) {
                panic!("（模擬）アプリが落ちました");
            }
        });
    }));
    assert!(r.is_err(), "落ちる前に終わりました: {}", h.dump());
}

fn resume_after_crash(protocol: Protocol, direction: Direction) {
    if !tools(protocol) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let content = data(8_000_000);
    let (src, dst) = match direction {
        Direction::Upload => (
            dir.path().join("local.bin"),
            dir.path().join("remote/x.bin"),
        ),
        Direction::Download => (
            dir.path().join("remote.bin"),
            dir.path().join("local/x.bin"),
        ),
    };
    std::fs::write(&src, &content).unwrap();
    let chunk = 256 << 10;
    let h = Harness::new(dir.path(), &[]);
    let mut job = match direction {
        Direction::Upload => upload_job(1, protocol, &src, uri(&dst), false).unwrap(),
        Direction::Download => {
            let m = std::fs::metadata(&src).unwrap();
            download_job(1, protocol, uri(&src), m.len(), mtime_of(&m), &dst, false)
        }
    };
    crash_midway(&h, &mut job, chunk);
    // 次に起動したとき: ジャーナルには中断（再開できる）として、確かな位置が残っている
    let h2 = Harness::new(dir.path(), &[]);
    let saved = h2.journal.load();
    assert_eq!(saved.len(), 1, "{}", h.dump());
    assert_eq!(saved[0].state, State::Interrupted);
    assert!(
        saved[0].done > 0 && saved[0].done < 8_000_000,
        "{}",
        h.dump()
    );
    let mut job = saved.into_iter().next().unwrap();
    h2.run(&mut job, 3, chunk);
    assert_eq!(job.state, State::Done, "{}", h2.dump());
    assert!(h2.logged("レジューム"), "{}", h2.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(h2.journal.load().is_empty());
}

#[test]
fn sftp_upload_resumes_after_the_app_crashes() {
    resume_after_crash(Protocol::Sftp, Direction::Upload);
}

#[test]
fn scp_upload_resumes_after_the_app_crashes() {
    resume_after_crash(Protocol::Scp, Direction::Upload);
}

#[test]
fn sftp_download_resumes_after_the_app_crashes() {
    resume_after_crash(Protocol::Sftp, Direction::Download);
}

#[test]
fn scp_download_resumes_after_the_app_crashes() {
    resume_after_crash(Protocol::Scp, Direction::Download);
}

/// 名前を変えた直後（ジャーナルを消す前）に落ちた転送は、送り直さずに完了とする。
fn finished_before_crash(protocol: Protocol, direction: Direction) {
    if !tools(protocol) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let content = data(300_000);
    let (src, dst) = match direction {
        Direction::Upload => (dir.path().join("local.bin"), dir.path().join("remote.bin")),
        Direction::Download => (dir.path().join("remote.bin"), dir.path().join("local.bin")),
    };
    std::fs::write(&src, &content).unwrap();
    let h = Harness::new(dir.path(), &[]);
    let mut job = match direction {
        Direction::Upload => upload_job(4, protocol, &src, uri(&dst), false).unwrap(),
        Direction::Download => {
            let m = std::fs::metadata(&src).unwrap();
            download_job(4, protocol, uri(&src), m.len(), mtime_of(&m), &dst, false)
        }
    };
    h.run(&mut job, 3, 64 << 10);
    assert_eq!(job.state, State::Done, "{}", h.dump());
    // 名前を変える前に書いたジャーナル（すべて送った・転送中）が残っている状態
    job.state = State::Running;
    h.journal.save(&job).unwrap();
    let h2 = Harness::new(dir.path(), &[]);
    let mut job = h2.journal.load().remove(0);
    assert_eq!(job.done, job.size);
    h2.run(&mut job, 3, 64 << 10);
    assert_eq!(job.state, State::Done, "{}", h2.dump());
    assert!(h2.logged("完了とします"), "{}", h2.dump());
    assert!(
        !h2.logged("区切りを送ります") && !h2.logged("を開きました"),
        "{}",
        h2.dump()
    );
    assert_eq!(std::fs::read(&dst).unwrap(), content);
    assert!(h2.journal.load().is_empty());

    // 送り先の中身が違えば完了とはしない（送り直す）
    let mut other = content.clone();
    other[299_999] ^= 0xff;
    let target = match direction {
        Direction::Upload => &dst,
        Direction::Download => &dst,
    };
    std::fs::write(target, &other).unwrap();
    if direction == Direction::Download {
        // 手元は大きさと更新日時で判断するので、更新日時をずらす
        let f = std::fs::File::options().write(true).open(target).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000))
            .unwrap();
    }
    let mut again = job.clone();
    again.state = State::Running;
    again.overwrite = true;
    h2.journal.save(&again).unwrap();
    let mut again = h2.journal.load().remove(0);
    h2.run(&mut again, 3, 64 << 10);
    assert_eq!(again.state, State::Done, "{}", h2.dump());
    assert_eq!(std::fs::read(&dst).unwrap(), content);
}

#[test]
fn sftp_upload_finished_before_a_crash_is_not_resent() {
    finished_before_crash(Protocol::Sftp, Direction::Upload);
}

#[test]
fn scp_upload_finished_before_a_crash_is_not_resent() {
    finished_before_crash(Protocol::Scp, Direction::Upload);
}

#[test]
fn sftp_download_finished_before_a_crash_is_not_resent() {
    finished_before_crash(Protocol::Sftp, Direction::Download);
}

#[test]
fn scp_download_finished_before_a_crash_is_not_resent() {
    finished_before_crash(Protocol::Scp, Direction::Download);
}

#[test]
fn journal_writes_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::new(dir.path());
    let src = dir.path().join("a");
    std::fs::write(&src, b"x").unwrap();
    let mut job = upload_job(9, Protocol::Sftp, &src, uri(Path::new("/r")), false).unwrap();
    for done in [1u64, 2, 3] {
        job.done = done;
        j.save(&job).unwrap();
    }
    // 一時ファイルは残らない
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "a")
        .collect();
    assert_eq!(names, vec!["9.job".to_owned()]);
    assert_eq!(j.load()[0].done, 3);
}
