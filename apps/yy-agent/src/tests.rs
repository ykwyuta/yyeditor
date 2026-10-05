use super::*;

/// 要求を順に処理して応答を返す。
fn run(agent: &mut Agent, req: Request) -> Response {
    agent.handle(req)
}

fn bytes(p: &Path) -> Vec<u8> {
    path_bytes(p)
}

fn upload(agent: &mut Agent, path: &Path, data: &[u8]) -> u32 {
    let Response::Upload(id) = run(agent, Request::BeginUpload { path: bytes(path) }) else {
        panic!("upload");
    };
    for part in data.chunks(1000) {
        assert_eq!(
            run(
                agent,
                Request::Write {
                    upload: id,
                    data: Block::pack(part),
                },
            ),
            Response::Done
        );
    }
    id
}

fn temps_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".yytmp-"))
        .collect()
}

#[test]
fn serves_framed_requests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, b"hello\nworld\n").unwrap();
    let mut input = Vec::new();
    yy_proto::write_frame(&mut input, 1, &Request::Hello { version: VERSION }).unwrap();
    yy_proto::write_frame(&mut input, 2, &Request::Open { path: bytes(&path) }).unwrap();
    yy_proto::write_frame(
        &mut input,
        3,
        &Request::Read {
            handle: 1,
            offset: 6,
            len: 100,
        },
    )
    .unwrap();
    let mut output = Vec::new();
    serve(&input[..], &mut output).unwrap();
    let mut r = &output[..];
    let (id, hello): (u32, Response) = yy_proto::read_frame(&mut r).unwrap().unwrap();
    assert_eq!(id, 1);
    assert!(matches!(
        hello,
        Response::Hello {
            version: VERSION,
            ..
        }
    ));
    let (_, opened): (u32, Response) = yy_proto::read_frame(&mut r).unwrap().unwrap();
    let Response::Opened { handle: 1, info } = opened else {
        panic!("{opened:?}");
    };
    assert_eq!(info.len(), 12);
    let (id, data): (u32, Response) = yy_proto::read_frame(&mut r).unwrap().unwrap();
    assert_eq!(id, 3);
    let Response::Data(block) = data else {
        panic!()
    };
    assert_eq!(block.unpack().unwrap(), b"world\n");
    assert!(
        yy_proto::read_frame::<_, Response>(&mut r)
            .unwrap()
            .is_none()
    );
}

#[test]
fn errors_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::default();
    let r = run(
        &mut agent,
        Request::Open {
            path: bytes(&dir.path().join("missing")),
        },
    );
    assert!(matches!(
        r,
        Response::Error(RemoteError {
            kind: yy_proto::ErrorKind::NotFound,
            ..
        })
    ));
    let r = run(
        &mut agent,
        Request::Open {
            path: bytes(dir.path()),
        },
    );
    assert!(matches!(r, Response::Error(_)), "{r:?}");
    let r = run(
        &mut agent,
        Request::Read {
            handle: 99,
            offset: 0,
            len: 1,
        },
    );
    assert!(matches!(r, Response::Error(_)));
}

#[test]
fn lists_directories() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), b"12345").unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    let mut agent = Agent::default();
    let Response::Dir(entries) = run(
        &mut agent,
        Request::ReadDir {
            path: bytes(dir.path()),
        },
    ) else {
        panic!();
    };
    let names: Vec<_> = entries.iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(names, [&b"a"[..], b"b.txt"]);
    assert!(entries[0].info.as_ref().unwrap().is_dir());
    assert_eq!(entries[1].info.as_ref().unwrap().len(), 5);
}

#[test]
fn saves_by_replacing_and_detects_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, b"old").unwrap();
    let mut agent = Agent::default();
    let Response::Opened { info, .. } = run(&mut agent, Request::Open { path: bytes(&path) })
    else {
        panic!();
    };

    // 開いたときのままなら置き換える
    let id = upload(&mut agent, &path, b"new contents");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: Some(info.id),
            force: false,
        },
    );
    let Response::Committed(saved) = r else {
        panic!("{r:?}")
    };
    assert_eq!(fs::read(&path).unwrap(), b"new contents");
    assert_eq!(saved.len(), 12);
    assert!(temps_in(dir.path()).is_empty());

    // 外部で変更されていたら置き換えず、強制すれば置き換える
    fs::write(&path, b"changed elsewhere").unwrap();
    let id = upload(&mut agent, &path, b"mine");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: Some(saved.id),
            force: false,
        },
    );
    let Response::Conflict(Some(current)) = r else {
        panic!("{r:?}")
    };
    assert_eq!(current.len(), 17);
    assert_eq!(fs::read(&path).unwrap(), b"changed elsewhere");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: Some(saved.id),
            force: true,
        },
    );
    assert!(matches!(r, Response::Committed(_)), "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), b"mine");

    // 新しく作るつもりで既にあれば競合
    let id = upload(&mut agent, &path, b"x");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: None,
            force: false,
        },
    );
    assert!(matches!(r, Response::Conflict(Some(_))));
    assert_eq!(
        run(&mut agent, Request::Abort { upload: id }),
        Response::Done
    );
    assert!(temps_in(dir.path()).is_empty());
    assert_eq!(fs::read(&path).unwrap(), b"mine");

    // 新しいファイル
    let fresh = dir.path().join("new.txt");
    let id = upload(&mut agent, &fresh, b"fresh");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: None,
            force: false,
        },
    );
    assert!(matches!(r, Response::Committed(_)));
    assert_eq!(fs::read(&fresh).unwrap(), b"fresh");
}

#[test]
fn unfinished_uploads_leave_no_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    {
        let mut agent = Agent::default();
        upload(&mut agent, &path, b"partial");
        assert_eq!(temps_in(dir.path()).len(), 1);
    }
    assert!(temps_in(dir.path()).is_empty());
    assert!(!path.exists());
}

#[cfg(unix)]
#[test]
fn keeps_mode_symlinks_and_hard_links() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.sh");
    fs::write(&real, b"echo 1").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o750)).unwrap();
    let link = dir.path().join("link.sh");
    symlink(&real, &link).unwrap();
    let hard = dir.path().join("hard.sh");
    fs::hard_link(&real, &hard).unwrap();

    let mut agent = Agent::default();
    let id = upload(&mut agent, &link, b"echo 2");
    let r = run(
        &mut agent,
        Request::Commit {
            upload: id,
            expected: None,
            force: true,
        },
    );
    let Response::Committed(info) = r else {
        panic!("{r:?}")
    };
    assert_eq!(info.mode, 0o750);
    // リンクは残り、リンク先とハードリンクの両方が新しい内容になる
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&real).unwrap(), b"echo 2");
    assert_eq!(fs::read(&hard).unwrap(), b"echo 2");
    assert_eq!(
        fs::metadata(&real).unwrap().ino(),
        fs::metadata(&hard).unwrap().ino()
    );
    assert_eq!(fs::metadata(&real).unwrap().mode() & 0o777, 0o750);
    assert!(temps_in(dir.path()).is_empty());
}

#[test]
fn removes_old_versions_only() {
    let root = tempfile::tempdir().unwrap();
    let own = root.path().join("0.2.0-bbbb");
    let old = root.path().join("0.1.0-aaaa");
    let other = root.path().join("unrelated");
    for d in [&own, &old, &other] {
        fs::create_dir(d).unwrap();
    }
    fs::write(own.join("yy-agent"), b"").unwrap();
    fs::write(old.join("yy-agent"), b"").unwrap();
    fs::write(other.join("data"), b"").unwrap();
    // 新しいうちは残す
    clean_old_versions(&own.join("yy-agent"), Duration::from_secs(3600));
    assert!(old.exists());
    clean_old_versions(&own.join("yy-agent"), Duration::ZERO);
    assert!(!old.exists());
    assert!(own.join("yy-agent").exists() && own.join(".last-used").exists());
    assert!(other.join("data").exists());
}

#[test]
fn hashes_contents() {
    assert_eq!(
        sha256_of(&mut &b"abc"[..]).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn ok(agent: &mut Agent, req: Request) {
    let r = run(agent, req);
    assert_eq!(r, Response::Done);
}

#[test]
fn makes_renames_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::default();
    let a = dir.path().join("a");
    ok(&mut agent, Request::MakeDir { path: bytes(&a) });
    assert!(a.is_dir());
    // 既にあればエラー
    assert!(matches!(
        run(&mut agent, Request::MakeDir { path: bytes(&a) }),
        Response::Error(RemoteError {
            kind: yy_proto::ErrorKind::AlreadyExists,
            ..
        })
    ));
    fs::write(a.join("f.txt"), b"x").unwrap();
    let b = dir.path().join("b");
    ok(
        &mut agent,
        Request::Rename {
            from: bytes(&a),
            to: bytes(&b),
        },
    );
    assert!(!a.exists() && b.join("f.txt").is_file());
    // 上書きしない
    fs::write(dir.path().join("g.txt"), b"y").unwrap();
    let r = run(
        &mut agent,
        Request::Rename {
            from: bytes(&b.join("f.txt")),
            to: bytes(&dir.path().join("g.txt")),
        },
    );
    assert!(matches!(r, Response::Error(_)), "{r:?}");
    assert_eq!(fs::read(dir.path().join("g.txt")).unwrap(), b"y");
    // フォルダをその中には移動できない
    fs::create_dir(b.join("sub")).unwrap();
    let r = run(
        &mut agent,
        Request::Rename {
            from: bytes(&b),
            to: bytes(&b.join("sub").join("b")),
        },
    );
    assert!(matches!(r, Response::Error(_)), "{r:?}");
    // 空でないフォルダは recursive でなければ消さない
    let r = run(
        &mut agent,
        Request::Remove {
            path: bytes(&b),
            recursive: false,
        },
    );
    assert!(matches!(r, Response::Error(_)));
    ok(
        &mut agent,
        Request::Remove {
            path: bytes(&b),
            recursive: true,
        },
    );
    assert!(!b.exists());
    ok(
        &mut agent,
        Request::Remove {
            path: bytes(&dir.path().join("g.txt")),
            recursive: false,
        },
    );
    let r = run(
        &mut agent,
        Request::Remove {
            path: b"/".to_vec(),
            recursive: true,
        },
    );
    assert!(matches!(r, Response::Error(_)));
}

#[test]
fn copies_trees_across_file_systems() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("d")).unwrap();
    fs::write(src.join("d").join("x.sh"), b"#!/bin/sh").unwrap();
    fs::set_permissions(
        src.join("d").join("x.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("d/x.sh", src.join("link")).unwrap();
    let dst = dir.path().join("dst");
    copy_tree(&src, &dst).unwrap();
    assert_eq!(fs::read(dst.join("d").join("x.sh")).unwrap(), b"#!/bin/sh");
    assert_eq!(
        fs::metadata(dst.join("d").join("x.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read_link(dst.join("link")).unwrap(),
        Path::new("d/x.sh")
    );
}
