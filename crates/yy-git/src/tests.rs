use super::*;

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
    let mut missing = Git::new(tmp.path());
    missing.program = PathBuf::from("/nonexistent/git-not-here");
    let e = missing.status().unwrap_err();
    assert!(e.message.contains("git が見つかりません"), "{e}");
}
