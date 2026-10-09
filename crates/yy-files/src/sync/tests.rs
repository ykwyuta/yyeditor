use super::*;
use crate::fs::{Faulty, Local};
use crate::scan::{ScanOptions, scan};

const NO_WAIT: &(dyn Fn(Duration) + Sync) = &|_| {};
const QUIET: &(dyn Fn(Event) + Sync) = &|_| {};

fn write(root: &Path, rel: &str, body: &[u8], mtime: i64) {
    let p = crate::join(root, rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    Local.set_mtime(&p, mtime).unwrap();
}

fn cat(root: &Path) -> Catalog {
    scan(&Local, root, &ScanOptions::default(), &|_| true).unwrap()
}

const T0: i64 = 1_700_000_000_000_000_000;
const SEC: i64 = 1_000_000_000;

fn big(n: usize, seed: u32) -> Vec<u8> {
    (0..n as u32)
        .map(|i| (i.wrapping_mul(31) ^ seed) as u8)
        .collect()
}

/// 送り元と送り先の目録が同じ（中身・日時）で、途中のファイルが残っていない。
fn assert_same(src: &Path, dst: &Path) {
    let a = cat(src);
    let b = scan(
        &Local,
        dst,
        &ScanOptions {
            exclude_files: crate::pattern::Patterns::default(),
            ..ScanOptions::default()
        },
        &|_| true,
    )
    .unwrap();
    let names = |c: &Catalog| c.files.iter().map(|f| f.rel.clone()).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
    for (x, y) in a.files.iter().zip(&b.files) {
        assert_eq!(
            std::fs::read(a.path(x)).unwrap(),
            std::fs::read(b.path(y)).unwrap(),
            "{}",
            x.rel
        );
        assert!((x.meta.mtime - y.meta.mtime).abs() < 1000, "{}", x.rel);
    }
}

#[test]
fn plans_new_update_same_touch_conflict_and_mirror() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("src"), d.path().join("dst"));
    write(&src, "new.txt", b"n", T0);
    write(&src, "same.txt", b"s", T0);
    write(&dst, "same.txt", b"s", T0 + SEC); // 誤差の内
    write(&src, "size.txt", b"longer", T0);
    write(&dst, "size.txt", b"x", T0);
    write(&src, "time.txt", b"t", T0 + 100 * SEC);
    write(&dst, "time.txt", b"t", T0);
    write(&src, "newer-dst.txt", b"a", T0);
    write(&dst, "newer-dst.txt", b"b", T0 + 100 * SEC);
    write(&src, "Case/File.TXT", b"c", T0 + 100 * SEC);
    write(&dst, "case/file.txt", b"c", T0);
    write(&dst, "only-dst.txt", b"o", T0);
    let opts = SyncOptions::default();
    let p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    let act = |rel: &str| p.items.iter().find(|i| i.rel == rel).map(|i| i.action);
    assert_eq!(act("new.txt"), Some(Action::New));
    assert_eq!(act("same.txt"), Some(Action::Same));
    assert_eq!(act("size.txt"), Some(Action::Update));
    assert_eq!(act("time.txt"), Some(Action::Update));
    assert_eq!(act("newer-dst.txt"), Some(Action::Conflict));
    // 大文字・小文字の違いは同じファイル（送り先の書き方で書く）
    let c = p.items.iter().find(|i| i.rel == "Case/File.TXT").unwrap();
    assert_eq!(
        (c.action, c.dst_rel.as_str()),
        (Action::Update, "case/file.txt")
    );
    assert_eq!(act("only-dst.txt"), None);
    // ミラーは送り先にだけあるものを消す
    let mirror = SyncOptions {
        mode: Mode::Mirror,
        ..SyncOptions::default()
    };
    let p2 = plan(&cat(&src), &cat(&dst), None, &mirror, &mut |_, _| Ok(false)).unwrap();
    assert_eq!(
        p2.items
            .iter()
            .find(|i| i.rel == "only-dst.txt")
            .map(|i| i.action),
        Some(Action::Delete)
    );
    // 中身で比べる: 同じ大きさで日時だけ違うものは日時だけ
    let content = SyncOptions {
        compare: Compare::Content,
        ..SyncOptions::default()
    };
    let p3 = plan(&cat(&src), &cat(&dst), None, &content, &mut |_, _| Ok(true)).unwrap();
    assert_eq!(
        p3.items
            .iter()
            .find(|i| i.rel == "time.txt")
            .map(|i| i.action),
        Some(Action::Touch)
    );
    // 前回の同期の記録と違う送り先は衝突（送り先が変わった）
    let mut st = SyncState::default();
    st.files.insert(crate::rel_key("size.txt"), (99, T0));
    let p4 = plan(&cat(&src), &cat(&dst), Some(&st), &opts, &mut |_, _| {
        Ok(false)
    })
    .unwrap();
    assert_eq!(
        p4.items
            .iter()
            .find(|i| i.rel == "size.txt")
            .map(|i| i.action),
        Some(Action::Conflict)
    );
    assert_eq!(p.count(Action::New), 1);
    let (n, bytes) = p.totals();
    assert_eq!(n, 4);
    assert_eq!(bytes, 1 + 6 + 1 + 1);
}

fn run_once(
    fs: &dyn Fs,
    run: &mut Run,
    journal: &Path,
    opts: &SyncOptions,
    st: &mut SyncState,
) -> io::Result<RunCounts> {
    let cancel = AtomicBool::new(false);
    execute(
        fs,
        run,
        journal,
        opts,
        st,
        &Hooks {
            event: QUIET,
            sleep: NO_WAIT,
            cancel: &cancel,
        },
    )
}

#[test]
fn executes_with_mirror_touch_readonly_and_keep_both() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst, jd) = (
        d.path().join("src"),
        d.path().join("dst"),
        d.path().join("runs"),
    );
    write(&src, "a.txt", b"hello", T0);
    write(&src, "sub/深い/b.bin", &big(3_000_000, 7), T0 + SEC * 10);
    write(&src, "ro.txt", b"ro", T0);
    std::fs::set_permissions(src.join("ro.txt"), {
        let mut p = std::fs::metadata(src.join("ro.txt")).unwrap().permissions();
        p.set_readonly(true);
        p
    })
    .unwrap();
    write(&src, "conf.txt", b"mine", T0);
    write(&dst, "conf.txt", b"theirs", T0 + 100 * SEC);
    write(&dst, "gone.txt", b"bye", T0);
    let opts = SyncOptions {
        mode: Mode::Mirror,
        threads: 3,
        verify_hash: true,
        ..SyncOptions::default()
    };
    let mut p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    // 衝突は「両方残す」にする
    for i in &mut p.items {
        if i.action == Action::Conflict {
            i.action = Action::KeepBoth;
        }
    }
    let mut run = Run::new(1, &p, opts.mode, "2026-10-08 1530");
    let journal = journal_path(&jd, 1);
    let mut st = SyncState::default();
    let c = run_once(&Local, &mut run, &journal, &opts, &mut st).unwrap();
    assert_eq!((c.done, c.failed, c.pending), (5, 0, 0));
    assert!(run.finished());
    assert_eq!(Run::load(&journal).unwrap(), run);
    assert_eq!(
        std::fs::read(dst.join("sub/深い/b.bin")).unwrap(),
        big(3_000_000, 7)
    );
    assert!(
        std::fs::metadata(dst.join("ro.txt"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(std::fs::read(dst.join("conf.txt")).unwrap(), b"mine");
    assert_eq!(
        std::fs::read(dst.join("conf (衝突 2026-10-08 1530).txt")).unwrap(),
        b"theirs"
    );
    // 消したものは隔離フォルダへ
    assert!(!dst.join("gone.txt").exists());
    assert_eq!(
        std::fs::read(dst.join(".yyfm-trash/2026-10-08-1530/gone.txt")).unwrap(),
        b"bye"
    );
    assert!(st.files.contains_key("a.txt"));
    assert!(!st.files.contains_key("gone.txt"));
    // 2 回目は送るものがない（読み取り専用の送り先も、衝突の写しも）
    let p2 = plan(
        &cat(&src),
        &cat(&dst),
        Some(&st),
        &SyncOptions::default(),
        &mut |_, _| Ok(false),
    )
    .unwrap();
    assert_eq!(p2.totals().0, 0, "{:?}", p2.items);
    // 送り元を変えると更新、読み取り専用の送り先も置き換えられる
    std::fs::set_permissions(src.join("ro.txt"), {
        let mut p = std::fs::metadata(src.join("ro.txt")).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        p
    })
    .unwrap();
    write(&src, "ro.txt", b"changed", T0 + 50 * SEC);
    let p3 = plan(
        &cat(&src),
        &cat(&dst),
        Some(&st),
        &SyncOptions::default(),
        &mut |_, _| Ok(false),
    )
    .unwrap();
    let mut run = Run::new(2, &p3, Mode::Update, "x");
    run_once(
        &Local,
        &mut run,
        &journal_path(&jd, 2),
        &SyncOptions::default(),
        &mut st,
    )
    .unwrap();
    assert_eq!(std::fs::read(dst.join("ro.txt")).unwrap(), b"changed");
}

#[test]
fn retries_dropped_connections() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("src"), d.path().join("dst"));
    write(&src, "big.bin", &big(5_000_000, 1), T0);
    write(&src, "small.txt", b"s", T0);
    std::fs::create_dir_all(&dst).unwrap();
    let opts = SyncOptions {
        checkpoint_bytes: 1 << 20,
        threads: 1,
        ..SyncOptions::default()
    };
    let p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    let mut run = Run::new(1, &p, Mode::Update, "x");
    let events = Mutex::new(Vec::new());
    let cancel = AtomicBool::new(false);
    // 3 回に 1 回の書き込みで回線が切れる
    let fs = Faulty::new(u64::MAX, 3);
    let mut st = SyncState::default();
    execute(
        &fs,
        &mut run,
        &d.path().join("1.run"),
        &opts,
        &mut st,
        &Hooks {
            event: &|e| events.lock().unwrap().push(e),
            sleep: NO_WAIT,
            cancel: &cancel,
        },
    )
    .unwrap();
    assert_same(&src, &dst);
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::Retry(..)))
    );
    // 回線が戻らなければ止まる（ジャーナルは残る）
    let d2 = tempfile::tempdir().unwrap();
    let dst2 = d2.path().join("dst");
    std::fs::create_dir_all(&dst2).unwrap();
    let p = plan(&cat(&src), &cat(&dst2), None, &opts, &mut |_, _| Ok(false)).unwrap();
    let mut run = Run::new(2, &p, Mode::Update, "x");
    let e = run_once(
        &Faulty::new(u64::MAX, 1),
        &mut run,
        &d2.path().join("2.run"),
        &opts,
        &mut st,
    )
    .unwrap_err();
    assert!(is_transient(&e));
    assert!(!run.finished());
    // 中止
    let cancel = AtomicBool::new(true);
    let e = execute(
        &Local,
        &mut run,
        &d2.path().join("2.run"),
        &opts,
        &mut st,
        &Hooks {
            event: QUIET,
            sleep: NO_WAIT,
            cancel: &cancel,
        },
    )
    .unwrap_err();
    assert!(crate::is_cancelled(&e));
}

/// どの操作の後で落ちても、ジャーナルから続ければ送り元と同じになり、途中のファイルが残らない。
#[test]
fn resumes_after_crashing_anywhere() {
    let base = tempfile::tempdir().unwrap();
    let src = base.path().join("src");
    write(&src, "a.bin", &big(2_500_000, 3), T0);
    write(&src, "dir/b.txt", b"bbb", T0 + SEC * 5);
    write(&src, "dir/c.bin", &big(1_200_000, 9), T0 + SEC * 9);
    write(&src, "keep.txt", b"new keep", T0 + SEC * 200);
    let opts = SyncOptions {
        mode: Mode::Mirror,
        threads: 2,
        checkpoint_bytes: 256 << 10,
        ..SyncOptions::default()
    };
    let mut crashed_mid_file = false;
    for crash_after in 1..60u64 {
        let d = tempfile::tempdir().unwrap();
        let dst = d.path().join("dst");
        write(&dst, "keep.txt", b"old", T0);
        write(&dst, "extra.txt", b"x", T0);
        let journal = d.path().join("1.run");
        let p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
        let mut run = Run::new(1, &p, opts.mode, "t");
        let mut st = SyncState::default();
        let r = run_once(
            &Faulty::new(crash_after, 0),
            &mut run,
            &journal,
            &opts,
            &mut st,
        );
        if r.is_ok() {
            assert_same(&src, &dst);
            continue;
        }
        // 落ちた: ジャーナルから続ける（落ちる前に書いたものだけが残っている）
        let mut run = Run::load(&journal).unwrap();
        if run
            .items
            .iter()
            .any(|i| matches!(i.state, ItemState::Partial(n) if n > 0))
        {
            crashed_mid_file = true;
        }
        let c = run_once(&Local, &mut run, &journal, &opts, &mut st).unwrap();
        assert_eq!(c.failed, 0, "crash_after {crash_after}: {:?}", run.items);
        assert_same(&src, &dst);
        assert!(!dst.join("extra.txt").exists());
    }
    assert!(crashed_mid_file, "送りかけで落ちる場合を試していない");
}

#[test]
fn source_changed_while_resuming_restarts_the_file() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("src"), d.path().join("dst"));
    write(&src, "a.bin", &big(2_000_000, 1), T0);
    std::fs::create_dir_all(&dst).unwrap();
    let opts = SyncOptions {
        checkpoint_bytes: 256 << 10,
        threads: 1,
        ..SyncOptions::default()
    };
    let p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    let mut run = Run::new(1, &p, Mode::Update, "x");
    let journal = d.path().join("1.run");
    let mut st = SyncState::default();
    assert!(run_once(&Faulty::new(5, 0), &mut run, &journal, &opts, &mut st).is_err());
    let mut run = Run::load(&journal).unwrap();
    assert!(matches!(run.items[0].state, ItemState::Partial(n) if n > 0));
    // その間に送り元が変わった
    write(&src, "a.bin", &big(2_100_000, 2), T0 + SEC * 60);
    run_once(&Local, &mut run, &journal, &opts, &mut st).unwrap();
    assert_same(&src, &dst);
    // 途中のファイルが送り元と合わない（壊れた）ときも最初から
    let mut run = Run::new(
        2,
        &plan(
            &cat(&src),
            &cat(&d.path().join("dst")),
            None,
            &opts,
            &mut |_, _| Ok(false),
        )
        .unwrap(),
        Mode::Update,
        "x",
    );
    assert!(run.items.is_empty());
    write(&src, "b.bin", &big(1_000_000, 5), T0);
    let p = plan(&cat(&src), &cat(&dst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    run = Run::new(3, &p, Mode::Update, "x");
    run.items[0].state = ItemState::Partial(500_000);
    std::fs::write(dst.join("b.bin.yypart"), vec![0u8; 500_000]).unwrap();
    run_once(&Local, &mut run, &d.path().join("3.run"), &opts, &mut st).unwrap();
    assert_same(&src, &dst);
}

#[test]
fn names_and_stamps() {
    assert_eq!(
        conflict_name("a/b/報告.docx", "2026-10-08 1530"),
        "a/b/報告 (衝突 2026-10-08 1530).docx"
    );
    assert_eq!(conflict_name("README", "x"), "README (衝突 x)");
    assert_eq!(conflict_name(".env", "x"), ".env (衝突 x)");
    assert_eq!(stamp(1_791_461_400 * SEC), "2026-10-08 1210");
    assert_eq!(stamp(0), "1970-01-01 0000");
    assert_eq!(
        part_path(Path::new("/x/a.txt")),
        Path::new("/x/a.txt.yypart")
    );
}

/// 送った量（Progress の合計）を数えながら実行する。
fn run_counting(
    fs: &dyn Fs,
    run: &mut Run,
    journal: &Path,
    opts: &SyncOptions,
    st: &mut SyncState,
) -> (io::Result<RunCounts>, u64, Vec<String>) {
    let sent = std::sync::atomic::AtomicU64::new(0);
    let logs = Mutex::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let ev = |e: Event| match e {
        Event::Progress(_, n) => {
            sent.fetch_add(n, Ordering::Relaxed);
        }
        Event::Log(m) => logs.lock().unwrap().push(m),
        _ => {}
    };
    let r = execute(
        fs,
        run,
        journal,
        opts,
        st,
        &Hooks {
            event: &ev,
            sleep: NO_WAIT,
            cancel: &cancel,
        },
    );
    (r, sent.into_inner(), logs.into_inner().unwrap())
}

#[test]
fn delta_writes_only_changed_blocks() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst) = (d.path().join("src"), d.path().join("dst"));
    let opts = SyncOptions {
        delta_min: 100_000,
        delta_block: 64 << 10,
        threads: 1,
        ..SyncOptions::default()
    };
    let mut body = big(1_000_000, 7);
    write(&src, "db.bin", &body, T0);
    write(&src, "small.txt", b"small", T0);
    std::fs::create_dir_all(&dst).unwrap();
    let mut st = SyncState::default();
    let sync_once = |st: &mut SyncState, id: u64| {
        let p = plan(&cat(&src), &cat(&dst), Some(st), &opts, &mut |_, _| {
            Ok(false)
        })
        .unwrap();
        let mut run = Run::new(id, &p, opts.mode, "t");
        let (r, sent, logs) = run_counting(
            &Local,
            &mut run,
            &d.path().join(format!("{id}.run")),
            &opts,
            st,
        );
        assert_eq!(r.unwrap().failed, 0, "{logs:?}");
        (sent, logs)
    };
    // 1 回目は全体を送り、ブロックのハッシュを覚える（小さいファイルは覚えない）
    let (sent, _) = sync_once(&mut st, 1);
    assert_eq!(sent, 1_000_005);
    assert_same(&src, &dst);
    let b = &st.blocks[&crate::rel_key("db.bin")];
    assert_eq!(b.hashes.len(), 1_000_000usize.div_ceil(64 << 10));
    assert!(!st.blocks.contains_key("small.txt"));
    assert_eq!(
        b.hashes,
        file_blocks(&Local, &dst.join("db.bin"), 64 << 10)
            .unwrap()
            .hashes
    );
    // 2 ブロックだけ書き換えて、末尾に足す
    body[10] ^= 0xff;
    body[700_000] ^= 0xff;
    body.extend_from_slice(&[1, 2, 3]);
    write(&src, "db.bin", &body, T0 + 100 * SEC);
    let (sent, logs) = sync_once(&mut st, 2);
    // 1 ブロック目・700000 を含むブロック・最後のブロック（伸びた）だけ
    let blk = 64u64 << 10;
    let last = 1_000_003 - (1_000_003 / blk) * blk;
    assert_eq!(sent, 2 * blk + last, "{logs:?}");
    assert!(logs.iter().any(|l| l.contains("差分の送り方")), "{logs:?}");
    assert_same(&src, &dst);
    assert_eq!(
        st.blocks[&crate::rel_key("db.bin")].hashes,
        file_blocks(&Local, &dst.join("db.bin"), blk)
            .unwrap()
            .hashes
    );
    // 縮んだときも合う
    body.truncate(300_000);
    body[5] ^= 1;
    write(&src, "db.bin", &body, T0 + 200 * SEC);
    let (sent, _) = sync_once(&mut st, 3);
    assert!(sent <= 2 * blk, "{sent}");
    assert_same(&src, &dst);
    // 送り先がほかで書き換わっていたら（記録の取り違えがあっても）全体を送る
    write(&dst, "db.bin", &big(300_000, 99), T0 + 200 * SEC + 3);
    st.files
        .insert(crate::rel_key("db.bin"), (300_000, T0 + 200 * SEC + 3));
    body[100] ^= 1;
    write(&src, "db.bin", &body, T0 + 300 * SEC);
    let (sent, _) = sync_once(&mut st, 4);
    assert_eq!(sent, 300_000);
    assert_same(&src, &dst);
}

#[test]
fn delta_resumes_after_crashing_anywhere() {
    let base = tempfile::tempdir().unwrap();
    let src = base.path().join("src");
    let opts = SyncOptions {
        delta_min: 100_000,
        delta_block: 64 << 10,
        threads: 1,
        checkpoint_bytes: 128 << 10,
        ..SyncOptions::default()
    };
    let mut body = big(900_000, 4);
    write(&src, "db.bin", &body, T0);
    // 前回の同期を済ませた状態を作る
    let prep = tempfile::tempdir().unwrap();
    let pdst = prep.path().join("dst");
    std::fs::create_dir_all(&pdst).unwrap();
    let mut st0 = SyncState::default();
    let p = plan(&cat(&src), &cat(&pdst), None, &opts, &mut |_, _| Ok(false)).unwrap();
    let mut run = Run::new(1, &p, opts.mode, "t");
    run_once(
        &Local,
        &mut run,
        &prep.path().join("1.run"),
        &opts,
        &mut st0,
    )
    .unwrap();
    body[200_000] ^= 0x55;
    body[850_000] ^= 0x55;
    write(&src, "db.bin", &body, T0 + 50 * SEC);
    let mut tried_delta = false;
    for crash_after in 1..20u64 {
        let d = tempfile::tempdir().unwrap();
        let dst = d.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::copy(pdst.join("db.bin"), dst.join("db.bin")).unwrap();
        let m = Local.metadata(&pdst.join("db.bin")).unwrap();
        Local.set_mtime(&dst.join("db.bin"), m.mtime).unwrap();
        let mut st = st0.clone();
        let journal = d.path().join("2.run");
        let p = plan(&cat(&src), &cat(&dst), Some(&st), &opts, &mut |_, _| {
            Ok(false)
        })
        .unwrap();
        let mut run = Run::new(2, &p, opts.mode, "t");
        let (r, _, logs) = run_counting(
            &Faulty::new(crash_after, 0),
            &mut run,
            &journal,
            &opts,
            &mut st,
        );
        tried_delta |= logs.iter().any(|l| l.contains("差分の送り方"));
        if r.is_ok() {
            assert_same(&src, &dst);
            continue;
        }
        let mut run = Run::load(&journal).unwrap();
        let c = run_once(&Local, &mut run, &journal, &opts, &mut st).unwrap();
        assert_eq!(c.failed, 0, "crash_after {crash_after}");
        assert_same(&src, &dst);
        assert_eq!(
            st.blocks[&crate::rel_key("db.bin")].hashes,
            file_blocks(&Local, &dst.join("db.bin"), 64 << 10)
                .unwrap()
                .hashes,
            "crash_after {crash_after}"
        );
    }
    assert!(tried_delta);
}

#[test]
fn state_reads_the_previous_format() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.state");
    #[derive(Serialize)]
    struct V1 {
        files: HashMap<String, (u64, i64)>,
    }
    let mut files = HashMap::new();
    files.insert("a".to_string(), (1u64, 2i64));
    std::fs::write(&p, postcard::to_allocvec(&V1 { files }).unwrap()).unwrap();
    let st = SyncState::load(&p).unwrap();
    assert_eq!(st.files["a"], (1, 2));
    let mut st2 = st.clone();
    st2.blocks.insert(
        "a".into(),
        Blocks {
            size: 1,
            mtime: 2,
            block: 4,
            hashes: vec![[1; 16]],
        },
    );
    st2.save(&p).unwrap();
    assert_eq!(SyncState::load(&p).unwrap(), st2);
}
