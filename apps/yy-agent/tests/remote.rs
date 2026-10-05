//! 端末側（yy-remote）からエージェントを配置・起動し、ファイルを取り寄せて保存する。
//! SSH の代わりに手元の `sh -c` を使う（`LocalTransport`）。
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yy_remote::local::LocalTransport;
use yy_remote::{AgentFiles, Session, Transport, UploadOutcome};

/// テスト用の配置元（`agents/yy-agent-<arch>-linux`）と配置先。
struct Setup {
    _dir: tempfile::TempDir,
    files: AgentFiles,
    agent_dir: String,
    work: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join("agents");
    fs::create_dir(&agents).unwrap();
    let arch = std::env::consts::ARCH;
    fs::copy(
        env!("CARGO_BIN_EXE_yy-agent"),
        agents.join(format!("yy-agent-{arch}-linux")),
    )
    .unwrap();
    let work = dir.path().join("work");
    fs::create_dir(&work).unwrap();
    Setup {
        files: AgentFiles::new(&agents),
        agent_dir: dir
            .path()
            .join("remote-agent")
            .to_string_lossy()
            .into_owned(),
        work,
        _dir: dir,
    }
}

fn start(s: &Setup) -> Arc<Session> {
    let t: Arc<dyn Transport> = Arc::new(LocalTransport::new());
    Arc::new(Session::start(t, &s.files, Some(&s.agent_dir)).unwrap())
}

fn bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

fn installed(s: &Setup) -> PathBuf {
    let dirs: Vec<_> = fs::read_dir(&s.agent_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].join("yy-agent")
}

#[test]
fn installs_once_and_reinstalls_broken_agent() {
    let s = setup();
    let session = start(&s);
    assert_eq!(session.agent_version(), env!("CARGO_PKG_VERSION"));
    let exe = installed(&s);
    let first = fs::metadata(&exe).unwrap().modified().unwrap();
    drop(session);

    // 同じものが配置済みなら送り直さない
    std::thread::sleep(std::time::Duration::from_millis(20));
    drop(start(&s));
    assert_eq!(fs::metadata(&exe).unwrap().modified().unwrap(), first);

    // 壊れていたら置き直す
    // （直前のエージェントがまだ動いていることがあるので、書き換えずに置き換える）
    let broken = exe.with_file_name("broken");
    fs::write(&broken, b"#!/bin/sh\necho broken\n").unwrap();
    fs::rename(&broken, &exe).unwrap();
    drop(start(&s));
    assert_eq!(
        fs::read(&exe).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_yy-agent")).unwrap()
    );
}

#[test]
fn downloads_and_uploads() {
    let s = setup();
    let session = start(&s);
    let path = s.work.join("data.txt");
    // 読み出しの単位（1 MiB）をまたぐ大きさ
    let original: Vec<u8> = (0..3_000_000u32)
        .map(|i| b"abcdefghij\n"[(i % 11) as usize])
        .collect();
    fs::write(&path, &original).unwrap();

    let mut got = Vec::new();
    let mut calls = 0;
    let info = session
        .download(&bytes(&path), &mut got, &mut |done, total| {
            calls += 1;
            assert!(done <= total);
            true
        })
        .unwrap();
    assert_eq!(got, original);
    assert_eq!(info.len(), original.len() as u64);
    assert!(calls >= 3);

    // 開いたときのままなら置き換える
    let edited = b"edited\n".repeat(400_000);
    let mut sent = 0;
    let r = session
        .upload(&mut &edited[..], &bytes(&path), Some(info.id), &mut |n| {
            sent = n;
            true
        })
        .unwrap();
    let UploadOutcome::Saved(saved) = r else {
        panic!("conflict")
    };
    assert_eq!(sent, edited.len() as u64);
    assert_eq!(fs::read(&path).unwrap(), edited);
    assert_eq!(saved.len(), edited.len() as u64);

    // 外部で変更されていれば競合。強制すれば置き換える
    fs::write(&path, b"someone else").unwrap();
    let r = session
        .upload(
            &mut &b"mine"[..],
            &bytes(&path),
            Some(saved.id),
            &mut |_| true,
        )
        .unwrap();
    let UploadOutcome::Conflict { current, pending } = r else {
        panic!("saved")
    };
    assert_eq!(current.unwrap().len(), 12);
    assert_eq!(fs::read(&path).unwrap(), b"someone else");
    pending.force().unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"mine");

    // 競合した内容を捨てれば一時ファイルは残らない
    let r = session
        .upload(&mut &b"x"[..], &bytes(&path), None, &mut |_| true)
        .unwrap();
    assert!(matches!(r, UploadOutcome::Conflict { .. }));
    drop(r);
    // 捨てる要求が処理されるのを待つ
    session.stat(&bytes(&path)).unwrap();
    let names: Vec<_> = fs::read_dir(&s.work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["data.txt"]);
}

#[test]
fn lists_and_resolves_paths() {
    let s = setup();
    let session = start(&s);
    fs::write(s.work.join("b.txt"), b"1").unwrap();
    fs::create_dir(s.work.join("a")).unwrap();
    let dir = session
        .real_path(&bytes(&s.work.join("a").join("..")))
        .unwrap();
    assert_eq!(dir, bytes(&fs::canonicalize(&s.work).unwrap()));
    let names: Vec<_> = session
        .read_dir(&dir)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, [b"a".to_vec(), b"b.txt".to_vec()]);
    assert_eq!(session.expand_home(b"~"), session.home());
    let e = session.stat(&bytes(&s.work.join("missing"))).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn cancel_and_disconnect() {
    let s = setup();
    let session = start(&s);
    let path = s.work.join("big.bin");
    fs::write(&path, vec![7u8; 5 << 20]).unwrap();
    let e = session
        .download(&bytes(&path), &mut Vec::new(), &mut |done, _| {
            done < (2 << 20)
        })
        .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::Interrupted);
    // 中止したあとも使える
    session.stat(&bytes(&path)).unwrap();
    assert!(!session.is_closed());
}

#[test]
fn manages_files_and_folders() {
    let s = setup();
    let session = start(&s);
    let dir = s.work.join("proj");
    session.make_dir(&bytes(&dir)).unwrap();
    let file = dir.join("new.txt");
    session.create_file(&bytes(&file)).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"");
    let e = session.create_file(&bytes(&file)).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    let moved = s.work.join("moved.txt");
    session.rename(&bytes(&file), &bytes(&moved)).unwrap();
    assert!(moved.is_file() && !file.exists());
    let e = session.rename(&bytes(&moved), &bytes(&moved)).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    fs::write(dir.join("x"), b"1").unwrap();
    assert!(session.remove(&bytes(&dir), false).is_err());
    session.remove(&bytes(&dir), true).unwrap();
    session.remove(&bytes(&moved), false).unwrap();
    assert!(!dir.exists() && !moved.exists());
}
