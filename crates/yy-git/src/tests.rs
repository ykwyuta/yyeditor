use super::*;
use std::process::Command;

#[test]
fn parses_porcelain_v2() {
    let out = b"# branch.oid 1234567890abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs\0\
1 A. N... 000000 100644 100644 0000 cccc new file.txt\0\
1 MM N... 100644 100644 100644 aaaa bbbb both.rs\0\
2 R. N... 100644 100644 100644 aaaa bbbb R100 renamed.rs\0old name.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e/\xe3\x83\xa1\xe3\x83\xa2.txt\0\
! target/x\0";
    let st = parse_status(out).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    assert_eq!((st.ahead, st.behind), (2, 1));
    assert_eq!(st.branch_label(), "main ↑2 ↓1");
    assert_eq!(st.changes.len(), 6);
    let staged: Vec<&str> = st
        .in_group(Group::Staged)
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(staged, ["both.rs", "new file.txt", "renamed.rs"]);
    let unstaged: Vec<(String, char)> = st
        .in_group(Group::Unstaged)
        .iter()
        .map(|c| (c.path.clone(), c.letter(Group::Unstaged)))
        .collect();
    assert_eq!(
        unstaged,
        [
            ("both.rs".to_string(), 'M'),
            ("src/main.rs".to_string(), 'M'),
            ("日本語/メモ.txt".to_string(), 'U'),
        ]
    );
    let conflict = st.in_group(Group::Conflict);
    assert_eq!(conflict.len(), 1);
    assert_eq!(conflict[0].path, "conflict.rs");
    let r = st.changes.iter().find(|c| c.path == "renamed.rs").unwrap();
    assert_eq!(r.orig.as_deref(), Some("old name.rs"));
    assert_eq!(r.letter(Group::Staged), 'R');
    // まだコミットがない・HEAD が切り離されている
    let st = parse_status(b"# branch.oid (initial)\0# branch.head (detached)\0").unwrap();
    assert_eq!((st.head, st.branch), (None, None));
    assert!(parse_status(b"1 broken\0").is_err());
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// 名前・メールを決めたリポジトリを作る。
fn init(dir: &Path) -> Git {
    let g = Git::new(dir);
    g.run(["-c", "init.defaultBranch=main", "init", "-q"])
        .unwrap();
    g.run(["config", "user.name", "Test"]).unwrap();
    g.run(["config", "user.email", "test@example.com"]).unwrap();
    g.run(["config", "commit.gpgsign", "false"]).unwrap();
    g
}

#[test]
fn discovers_repositories_in_the_workspace() {
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("app/libs/core")).unwrap();
    std::fs::create_dir_all(ws.join("docs")).unwrap();
    std::fs::create_dir_all(ws.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(ws.join("plain")).unwrap();
    init(&ws.join("app"));
    init(&ws.join("app/libs/core"));
    init(&ws.join("docs"));
    // node_modules の下は見ない
    init(&ws.join("node_modules/pkg"));
    // 中身のない .git は数えない
    std::fs::create_dir_all(ws.join("plain/.git")).unwrap();
    // サブモジュール・worktree の .git はファイル
    std::fs::create_dir_all(ws.join("linked")).unwrap();
    std::fs::write(ws.join("linked/.git"), "gitdir: ../app/.git\n").unwrap();
    let found = discover(std::slice::from_ref(&ws), Limits::default());
    let labels: Vec<(&str, bool)> = found.iter().map(|f| (f.label.as_str(), f.linked)).collect();
    assert_eq!(
        labels,
        [
            ("ws/app", false),
            ("ws/app/libs/core", false),
            ("ws/docs", false),
            ("ws/linked", true),
        ]
    );
    // ワークスペースのフォルダがリポジトリの中のフォルダなら、上のリポジトリ
    let inner = ws.join("app/libs");
    let found = discover(&[inner], Limits::default());
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|f| f.root == ws.join("app") && f.label.contains("libs を含む"))
    );
    assert!(found.iter().any(|f| f.root == ws.join("app/libs/core")));
    // 深さの上限
    let found = discover(
        std::slice::from_ref(&ws),
        Limits {
            depth: 1,
            dirs: 1000,
        },
    );
    assert!(!found.iter().any(|f| f.label == "ws/app/libs/core"));
    // リモートのフォルダは見ない
    assert!(discover(&[PathBuf::from("ssh://host/x")], Limits::default()).is_empty());
}

#[test]
fn stages_commits_and_switches_branches() {
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let g = init(dir);
    // まだコミットがない
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    std::fs::write(dir.join("日本語.txt"), "こんにちは\n").unwrap();
    let st = g.status().unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.head, None);
    assert_eq!(st.in_group(Group::Unstaged).len(), 2);
    g.stage(&["a.txt".into()]).unwrap();
    assert_eq!(g.status().unwrap().in_group(Group::Staged).len(), 1);
    // 最初のコミットの前でもステージを外せる
    g.unstage(&["a.txt".into()]).unwrap();
    assert!(!g.status().unwrap().has_staged());
    g.stage_all().unwrap();
    let summary = g.commit("最初のコミット\n\n本文", false).unwrap();
    assert!(summary.ends_with("最初のコミット"), "{summary}");
    assert_eq!(g.last_message().unwrap(), "最初のコミット\n\n本文");
    let st = g.status().unwrap();
    assert!(st.changes.is_empty());
    assert!(st.head.is_some());
    // 変更・削除・追加
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    std::fs::remove_file(dir.join("日本語.txt")).unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    let st = g.status().unwrap();
    let letters: Vec<(String, char)> = st
        .in_group(Group::Unstaged)
        .iter()
        .map(|c| (c.path.clone(), c.letter(Group::Unstaged)))
        .collect();
    assert_eq!(
        letters,
        [
            ("a.txt".to_string(), 'M'),
            ("new.txt".to_string(), 'U'),
            ("日本語.txt".to_string(), 'D'),
        ]
    );
    // 版の中身（HEAD・インデックス）
    assert_eq!(g.show("HEAD", "a.txt").unwrap().unwrap(), b"one\n");
    assert_eq!(g.show("", "a.txt").unwrap().unwrap(), b"one\n");
    assert_eq!(g.show("HEAD", "new.txt").unwrap(), None);
    g.stage(&["a.txt".into(), "日本語.txt".into()]).unwrap();
    assert_eq!(g.show("", "a.txt").unwrap().unwrap(), b"two\n");
    let st = g.status().unwrap();
    assert_eq!(st.in_group(Group::Staged).len(), 2);
    g.unstage_all().unwrap();
    assert!(!g.status().unwrap().has_staged());
    // 変更を捨てる（追跡しているものは戻し、追跡していないものは消す）
    g.discard(&["a.txt".into(), "日本語.txt".into()], &["new.txt".into()])
        .unwrap();
    assert_eq!(std::fs::read(dir.join("a.txt")).unwrap(), b"one\n");
    assert!(dir.join("日本語.txt").exists());
    assert!(!dir.join("new.txt").exists());
    assert!(g.status().unwrap().changes.is_empty());
    // 直前のコミットを直す
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    g.stage_all().unwrap();
    g.commit("直した", true).unwrap();
    assert_eq!(g.last_message().unwrap(), "直した");
    assert_eq!(
        g.run(["rev-list", "--count", "HEAD"])
            .unwrap()
            .text()
            .trim(),
        "1"
    );
    // ブランチ
    assert!(!g.valid_branch_name("bad name"));
    assert!(!g.valid_branch_name("-x"));
    assert!(g.create_branch("bad..name").is_err());
    g.create_branch("feature/x").unwrap();
    assert_eq!(g.status().unwrap().branch.as_deref(), Some("feature/x"));
    let branches = g.branches().unwrap();
    let names: Vec<(&str, bool)> = branches
        .iter()
        .map(|b| (b.name.as_str(), b.current))
        .collect();
    assert_eq!(names, [("feature/x", true), ("main", false)]);
    let main = branches.iter().find(|b| b.name == "main").unwrap();
    g.switch(main).unwrap();
    assert_eq!(g.status().unwrap().branch.as_deref(), Some("main"));
}

#[test]
fn pushes_pulls_and_tracks_remote_branches() {
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    Git::new(tmp.path())
        .run([
            "-c",
            "init.defaultBranch=main",
            "init",
            "-q",
            "--bare",
            remote.to_str().unwrap(),
        ])
        .unwrap();
    let a_dir = tmp.path().join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    let a = init(&a_dir);
    a.run(["remote", "add", "origin", remote.to_str().unwrap()])
        .unwrap();
    std::fs::write(a_dir.join("x.txt"), "1\n").unwrap();
    a.stage_all().unwrap();
    a.commit("one", false).unwrap();
    // 上流がなければ origin の同じ名前へ、上流として
    let st = a.status().unwrap();
    assert_eq!(st.upstream, None);
    a.push(&st).unwrap();
    let st = a.status().unwrap();
    assert_eq!(st.upstream.as_deref(), Some("origin/main"));
    // 別の作業ツリーでコミットしてプッシュ → こちらでフェッチすると遅れ、プルで追いつく
    let b_dir = tmp.path().join("b");
    Git::new(tmp.path())
        .run([
            "clone",
            "-q",
            remote.to_str().unwrap(),
            b_dir.to_str().unwrap(),
        ])
        .unwrap();
    let b = Git::new(&b_dir);
    b.run(["config", "user.name", "B"]).unwrap();
    b.run(["config", "user.email", "b@example.com"]).unwrap();
    std::fs::write(b_dir.join("y.txt"), "2\n").unwrap();
    b.stage_all().unwrap();
    b.commit("two", false).unwrap();
    b.run(["push", "-q", "origin", "HEAD:refs/heads/feat"])
        .unwrap();
    b.push(&b.status().unwrap()).unwrap();
    a.fetch().unwrap();
    let st = a.status().unwrap();
    assert_eq!((st.ahead, st.behind), (0, 1));
    assert_eq!(st.branch_label(), "main ↑0 ↓1");
    a.pull().unwrap();
    assert!(a_dir.join("y.txt").exists());
    // リモート追跡ブランチに切り替えると、手元のブランチを作って追跡する
    let feat = a
        .branches()
        .unwrap()
        .into_iter()
        .find(|b| b.name == "origin/feat")
        .unwrap();
    assert!(feat.remote);
    a.switch(&feat).unwrap();
    let st = a.status().unwrap();
    assert_eq!(st.branch.as_deref(), Some("feat"));
    assert_eq!(st.upstream.as_deref(), Some("origin/feat"));
    // リモートがなければ知らせる
    let c_dir = tmp.path().join("c");
    std::fs::create_dir_all(&c_dir).unwrap();
    let c = init(&c_dir);
    std::fs::write(c_dir.join("z"), "z").unwrap();
    c.stage_all().unwrap();
    c.commit("z", false).unwrap();
    let e = c.push(&c.status().unwrap()).unwrap_err();
    assert!(e.message.contains("リモートがありません"), "{e}");
}

#[test]
fn reports_git_errors() {
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let g = init(tmp.path());
    // ステージしたものがなければコミットできない
    let e = g.commit("x", false).unwrap_err();
    assert!(e.command.starts_with("git commit"), "{e}");
    assert!(!e.message.is_empty());
    // git が見つからない
    let missing = Git::with_runner(Arc::new(LocalRunner::with_program(
        tmp.path(),
        Path::new("/nonexistent/git-not-here"),
    )));
    let e = missing.status().unwrap_err();
    assert!(e.message.contains("git が見つかりません"), "{e}");
}

#[test]
fn quotes_for_the_remote_shell() {
    assert_eq!(shell_quote(b"plain"), b"'plain'");
    assert_eq!(shell_quote(b"it's"), b"'it'\"'\"'s'");
    assert_eq!(shell_quote(b"a b;$(x)`y`"), b"'a b;$(x)`y`'");
}

/// SSH の接続先の代わりに、手元のシェル（`sh -c`）で同じ操作をする。
#[cfg(unix)]
#[test]
fn works_on_a_remote_repository() {
    use std::os::unix::ffi::OsStrExt;
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // 空白・引用符・日本語を含む名前（シェルの引用を確かめる）
    let ws = tmp.path().join("ws dir");
    let repo = ws.join("it's 日本語");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::create_dir_all(ws.join("node_modules/x")).unwrap();
    init(&repo);
    init(&ws.join("node_modules/x"));
    std::fs::create_dir_all(ws.join("linked")).unwrap();
    std::fs::write(ws.join("linked/.git"), "gitdir: x\n").unwrap();
    let t: Arc<dyn yy_remote::Transport> = Arc::new(yy_remote::local::LocalTransport::new());
    let folder = yy_remote::RemoteUri {
        user: Some("me".into()),
        host: "build".into(),
        port: None,
        path: ws.as_os_str().as_bytes().to_vec(),
    };
    let found = discover_remote(&t, std::slice::from_ref(&folder), Limits::default());
    let labels: Vec<(&str, bool)> = found.iter().map(|f| (f.label.as_str(), f.linked)).collect();
    assert_eq!(
        labels,
        [
            ("ws dir/it's 日本語 [me@build]", false),
            ("ws dir/linked [me@build]", true),
        ]
    );
    let root = found[0].root.to_string_lossy().into_owned();
    assert!(root.starts_with("ssh://me@build/"), "{root}");
    let uri = yy_remote::RemoteUri::parse(&root).unwrap();
    assert_eq!(uri.path, repo.as_os_str().as_bytes());
    // フォルダを含む上のリポジトリ
    let inner = yy_remote::RemoteUri {
        path: repo.join("sub").as_os_str().as_bytes().to_vec(),
        ..folder.clone()
    };
    let up = discover_remote(&t, &[inner], Limits::default());
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(up[0].label.contains("sub を含む"), "{}", up[0].label);
    // 操作
    let g = Git::remote(t.clone(), &uri.path);
    std::fs::write(repo.join("a b.txt"), "one\n").unwrap();
    std::fs::write(repo.join("sub/'q'.txt"), "q\n").unwrap();
    let st = g.status().unwrap();
    assert_eq!(st.in_group(Group::Unstaged).len(), 2);
    g.stage_all().unwrap();
    let summary = g.commit("リモートのコミット\n", false).unwrap();
    assert!(summary.ends_with("リモートのコミット"), "{summary}");
    std::fs::write(repo.join("a b.txt"), "two\n").unwrap();
    assert_eq!(g.read_worktree("a b.txt").unwrap().unwrap(), b"two\n");
    assert_eq!(g.read_worktree("sub/'q'.txt").unwrap().unwrap(), b"q\n");
    assert_eq!(g.read_worktree("nothing").unwrap(), None);
    assert_eq!(g.show("HEAD", "a b.txt").unwrap().unwrap(), b"one\n");
    g.stage(&["a b.txt".into()]).unwrap();
    assert_eq!(g.status().unwrap().in_group(Group::Staged).len(), 1);
    g.unstage(&["a b.txt".into()]).unwrap();
    g.discard(&["a b.txt".into()], &[]).unwrap();
    assert!(g.status().unwrap().changes.is_empty());
    g.create_branch("feature/リモート").unwrap();
    assert_eq!(
        g.status().unwrap().branch.as_deref(),
        Some("feature/リモート")
    );
    // 接続先に git がない
    let missing = Git::remote(t.clone(), b"/nonexistent-dir");
    let e = missing.status().unwrap_err();
    assert!(!e.message.is_empty());
}

#[test]
fn reads_auth_failures_and_remote_urls() {
    use crate::auth::*;
    assert_eq!(
        auth_failure(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ),
        Some(AuthKind::Password)
    );
    assert_eq!(
        auth_failure(
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://x/'"
        ),
        Some(AuthKind::Password)
    );
    assert_eq!(
        auth_failure(
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository."
        ),
        Some(AuthKind::Ssh)
    );
    assert_eq!(auth_failure("fatal: not a git repository"), None);
    let r = remote_info("https://alice@github.com/owner/repo.git");
    assert_eq!(
        (r.key.as_str(), r.label.as_str(), r.user.as_str(), r.kind),
        (
            "git/https://github.com",
            "https://github.com",
            "alice",
            AuthKind::Password
        )
    );
    let r = remote_info("https://bob:secret@git.example.com:8443/x");
    assert_eq!(
        (r.key.as_str(), r.user.as_str()),
        ("git/https://git.example.com:8443", "bob")
    );
    let r = remote_info("git@github.com:owner/repo.git");
    assert_eq!(
        (r.key.as_str(), r.label.as_str(), r.kind),
        ("git/ssh/git@github.com", "git@github.com", AuthKind::Ssh)
    );
    let r = remote_info("ssh://git@host:2222/srv/repo.git");
    assert_eq!(r.key, "git/ssh/git@host:2222");
    assert_eq!(askpass_answer("Username for 'https://x': ", "u", "p"), "u");
    assert_eq!(
        askpass_answer("Password for 'https://u@x': ", "u", "p"),
        "p"
    );
    assert_eq!(
        askpass_answer("Enter passphrase for key '/k': ", "u", "p"),
        "p"
    );
    let c = Credentials {
        user: "u".into(),
        secret: "very secret".into(),
    };
    assert!(!format!("{c:?}").contains("secret"));
}

/// 資格情報を askpass で渡す（git credential fill は、資格情報ヘルパーがなければ askpass に尋ねる）。
#[cfg(unix)]
#[test]
fn passes_credentials_through_askpass() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    if !git_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("r");
    std::fs::create_dir_all(&repo).unwrap();
    init(&repo);
    let input = b"protocol=https\nhost=example.com\n\n";
    let cred = Credentials {
        user: "alice".into(),
        secret: "p@ss w'rd $HOME".into(),
    };
    // 資格情報がなければ尋ねずに失敗し、認証の失敗と分かる
    let plain = Git::new(&repo);
    let e = plain
        .run_with(
            ["-c", "credential.helper=", "credential", "fill"],
            Some(input),
        )
        .unwrap_err();
    assert_eq!(auth_failure(&e.message), Some(AuthKind::Password), "{e}");
    // 手元: askpass のプログラム
    let script = tmp.path().join("askpass.sh");
    std::fs::write(&script, crate::auth::ASKPASS_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let local = Git::with_runner(Arc::new(LocalRunner::new(&repo).with_askpass(&script)));
    let out = local
        .run_auth_with(&["credential", "fill"], input, &cred)
        .unwrap()
        .text();
    assert!(out.contains("username=alice\n"), "{out}");
    assert!(out.contains("password=p@ss w'rd $HOME\n"), "{out}");
    // 接続先: sh -c の中で一時的な askpass を作り、資格情報は標準入力で渡す
    let t: Arc<dyn yy_remote::Transport> = Arc::new(yy_remote::local::LocalTransport::new());
    let remote = Git::remote(t, repo.as_os_str().as_bytes());
    let out = remote
        .run_auth_with(&["credential", "fill"], input, &cred)
        .unwrap()
        .text();
    assert!(out.contains("username=alice\n"), "{out}");
    assert!(out.contains("password=p@ss w'rd $HOME\n"), "{out}");
    // 一時的な askpass は消える
    let askpass = remote
        .run_auth(
            [
                "-c",
                "alias.askpath=!printf '%s' \"$GIT_ASKPASS\"",
                "askpath",
            ],
            Some(&cred),
        )
        .unwrap()
        .text();
    let askpass = askpass.trim();
    assert!(askpass.ends_with("/askpass"), "{askpass}");
    assert!(!Path::new(askpass).exists());
}
