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

#[test]
fn copies_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::default();
    let a = dir.path().join("a");
    fs::create_dir_all(a.join("sub")).unwrap();
    fs::write(a.join("sub").join("f.txt"), b"data").unwrap();
    let b = dir.path().join("b");
    ok(
        &mut agent,
        Request::Copy {
            from: bytes(&a),
            to: bytes(&b),
        },
    );
    assert_eq!(fs::read(b.join("sub").join("f.txt")).unwrap(), b"data");
    assert!(a.join("sub").join("f.txt").exists());
    // 既にあれば上書きしない
    let r = run(
        &mut agent,
        Request::Copy {
            from: bytes(&a),
            to: bytes(&b),
        },
    );
    assert!(matches!(r, Response::Error(_)), "{r:?}");
    // フォルダをその中へはコピーしない
    let r = run(
        &mut agent,
        Request::Copy {
            from: bytes(&a),
            to: bytes(&a.join("sub").join("a")),
        },
    );
    assert!(matches!(r, Response::Error(_)), "{r:?}");
    assert!(!a.join("sub").join("a").exists());
}

#[test]
fn hashes_a_prefix_of_a_file() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h.bin");
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(&path, &data).unwrap();
    let mut agent = Agent::default();
    for len in [0u64, 10, 3_000_000] {
        let Response::Hash(h) = run(
            &mut agent,
            Request::Hash {
                path: bytes(&path),
                len,
            },
        ) else {
            panic!();
        };
        assert_eq!(h, Sha256::digest(&data[..len as usize]).to_vec());
    }
    // ファイルより長くは計算しない
    let Response::Error(e) = run(
        &mut agent,
        Request::Hash {
            path: bytes(&path),
            len: 3_000_001,
        },
    ) else {
        panic!();
    };
    assert_eq!(e.kind, yy_proto::ErrorKind::InvalidInput);
}

/// zip の中身（名前 → 中身。フォルダは `None`）を、目次からたどって読む（CRC も確かめる）。
fn read_zip(data: &[u8]) -> Vec<(String, Option<Vec<u8>>)> {
    let u16le = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]) as usize;
    let u32le = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    let end = data.len() - 22;
    assert_eq!(u32le(end), 0x0605_4b50);
    let count = u16le(end + 10);
    let mut at = u32le(end + 16);
    let mut out = Vec::new();
    for _ in 0..count {
        assert_eq!(u32le(at), 0x0201_4b50);
        let method = u16le(at + 10);
        let crc = u32le(at + 16) as u32;
        let packed = u32le(at + 20);
        let size = u32le(at + 24);
        let name_len = u16le(at + 28);
        let extra = u16le(at + 30);
        let comment = u16le(at + 32);
        let local = u32le(at + 42);
        let name = String::from_utf8(data[at + 46..at + 46 + name_len].to_vec()).unwrap();
        at += 46 + name_len + extra + comment;
        assert_eq!(u32le(local), 0x0403_4b50);
        assert_eq!(u32le(local + 14) as u32, crc, "{name}");
        let start = local + 30 + u16le(local + 26) + u16le(local + 28);
        let body = &data[start..start + packed];
        if name.ends_with('/') {
            out.push((name, None));
            continue;
        }
        let content = match method {
            8 => miniz_oxide::inflate::decompress_to_vec(body).unwrap(),
            0 => body.to_vec(),
            m => panic!("method {m}"),
        };
        assert_eq!(content.len(), size, "{name}");
        assert_eq!(zip::crc32(0, &content), crc, "{name}");
        out.push((name, Some(content)));
    }
    out
}

/// 作り終えるか上限を超えるまで進める。
fn zip_until_done(agent: &mut Agent, path: &Path, limit: u64) -> yy_proto::ZipProgress {
    let Response::Zip(p) = run(
        agent,
        Request::ZipBegin {
            path: bytes(path),
            limit,
            utc_offset: 9 * 3600,
        },
    ) else {
        panic!("zip begin");
    };
    let mut p = p;
    while p.state == yy_proto::ZipState::Running {
        p = match run(agent, Request::ZipStep { job: p.job }) {
            Response::Zip(p) => p,
            r => panic!("{r:?}"),
        };
    }
    p
}

#[test]
fn zips_a_folder_with_subfolders_and_japanese_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("プロジェクト");
    fs::create_dir_all(root.join("src/空のフォルダ")).unwrap();
    fs::write(root.join("README.md"), "# 見出し\n".repeat(1000)).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(root.join("src/data.bin"), &big).unwrap();
    fs::write(root.join("src/empty.txt"), b"").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("README.md"), root.join("link")).unwrap();

    let mut agent = Agent::default();
    let p = zip_until_done(&mut agent, &root, 100 << 20);
    assert_eq!(p.state, yy_proto::ZipState::Done);
    assert_eq!((p.files, p.total_files, p.skipped), (3, 3, 0));
    assert_eq!(p.read, p.total_bytes);
    let temp = to_path(&p.temp);
    let data = fs::read(&temp).unwrap();
    assert_eq!(data.len() as u64, p.written);
    let entries = read_zip(&data);
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "プロジェクト/",
            "プロジェクト/README.md",
            "プロジェクト/src/",
            "プロジェクト/src/data.bin",
            "プロジェクト/src/empty.txt",
            "プロジェクト/src/空のフォルダ/",
        ]
    );
    assert_eq!(entries[3].1.as_deref(), Some(&big[..]));
    assert_eq!(entries[4].1.as_deref(), Some(&b""[..]));
    // 圧縮されている
    assert!(p.written < 100_000, "{}", p.written);
    // 取り寄せた後に終えると一時ファイルは消える
    assert_eq!(
        run(&mut agent, Request::ZipEnd { job: p.job }),
        Response::Done
    );
    assert!(!temp.exists());
}

#[test]
fn zip_stops_when_compressed_size_exceeds_limit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("big");
    fs::create_dir_all(&root).unwrap();
    // 圧縮できない中身
    let mut x = 0x1234_5678u32;
    let noise: Vec<u8> = (0..2_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    fs::write(root.join("noise.bin"), &noise).unwrap();
    let mut agent = Agent::default();
    let p = zip_until_done(&mut agent, &root, 1_000_000);
    assert!(matches!(p.state, yy_proto::ZipState::TooLarge(_)), "{p:?}");
    assert!(!to_path(&p.temp).exists());
    // 上限の中なら作れる
    let p = zip_until_done(&mut agent, &root, 10_000_000);
    assert_eq!(p.state, yy_proto::ZipState::Done);
    let entries = read_zip(&fs::read(to_path(&p.temp)).unwrap());
    assert_eq!(entries[1].1.as_deref(), Some(&noise[..]));
    // 終えずにエージェントが終わっても一時ファイルは残さない
    let temp = to_path(&p.temp);
    drop(agent);
    assert!(!temp.exists());
}

#[test]
fn zip_rejects_files_and_unknown_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let mut agent = Agent::default();
    assert!(matches!(
        run(
            &mut agent,
            Request::ZipBegin {
                path: bytes(&f),
                limit: 1 << 20,
                utc_offset: 0,
            }
        ),
        Response::Error(_)
    ));
    assert!(matches!(
        run(&mut agent, Request::ZipStep { job: 99 }),
        Response::Error(_)
    ));
}

#[test]
fn dos_time_is_utc_calendar() {
    // 2024-02-29 13:45:58 UTC
    let (t, d) = zip::dos_time(1_709_214_358);
    assert_eq!((d >> 9) + 1980, 2024);
    assert_eq!((d >> 5) & 0xF, 2);
    assert_eq!(d & 0x1F, 29);
    assert_eq!((t >> 11, (t >> 5) & 0x3F, (t & 0x1F) * 2), (13, 45, 58));
    assert_eq!(zip::dos_time(0), (0, (1 << 5) | 1));
    assert_eq!(zip::crc32(0, b"123456789"), 0xCBF4_3926);
}
