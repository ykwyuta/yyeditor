use super::*;
use crate::fs::Local;

fn write(root: &Path, path: &str, bytes: usize) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn all() -> Options {
    Options {
        min_bytes: 0,
        build_age_days: 0,
        ..Options::default()
    }
}

fn age_directory(path: &Path) {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .access_mode(0x100)
            .custom_flags(0x02000000)
            .open(path)
            .unwrap()
    };
    #[cfg(not(windows))]
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(crate::fs::from_nanos(1_000_000_000))
        .unwrap();
}

#[test]
fn markers_age_size_and_nested_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "web/package.json", 0);
    write(root, "web/node_modules/a/node_modules/b/data", 400);
    write(root, "web/dist/app.js", 200);
    write(root, "web/.next/cache/data", 300);
    write(root, "python/pyproject.toml", 0);
    write(root, "python/dist/a.whl", 150);
    write(root, "python/__pycache__/x.pyc", 20);
    write(root, "python/.venv/pyvenv.cfg", 10);
    write(root, "java/pom.xml", 0);
    write(root, "java/target/a.jar", 350);
    write(root, "gradle/build.gradle.kts", 0);
    write(root, "gradle/build/a.jar", 100);
    write(root, "rust/Cargo.toml", 0);
    write(root, "rust/target/debug/exe", 250);
    write(root, "dotnet/app.csproj", 0);
    write(root, "dotnet/bin/app.dll", 120);
    write(root, "documents/build/source.txt", 1000);
    write(root, "documents/dist/source.txt", 1000);
    write(root, "documents/venv/source.txt", 1000);
    write(root, ".git/objects/node_modules/keep", 1000);
    write(root, "containers/storage/overlay/node_modules/keep", 1000);
    let before = std::fs::read(root.join("web/node_modules/a/node_modules/b/data")).unwrap();
    let result = scan(
        &Local,
        &[root.into(), root.join("web")],
        &[],
        &all(),
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(result.items.len(), 10, "{:#?}", result.items);
    assert_eq!(result.items[0].bytes, 400);
    assert_eq!(
        result
            .items
            .iter()
            .filter(|c| c.kind == "依存パッケージ")
            .count(),
        1
    );
    assert_eq!(
        before,
        std::fs::read(root.join("web/node_modules/a/node_modules/b/data")).unwrap()
    );
    let small = scan(
        &Local,
        &[root.into()],
        &[],
        &Options {
            min_bytes: 350,
            ..all()
        },
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(small.items.len(), 2);
    // A recently changed file within an old directory must keep the whole build out of candidates.
    age_directory(&root.join("web/dist"));
    let recent = scan(
        &Local,
        &[root.join("web")],
        &[],
        &Options {
            min_bytes: 0,
            build_age_days: 30,
            ..Options::default()
        },
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(recent.items.len(), 1);
    Local
        .set_mtime(&root.join("web/dist/app.js"), 1_000_000_000)
        .unwrap();
    let old = scan(
        &Local,
        &[root.join("web/dist")],
        &[],
        &Options {
            min_bytes: 0,
            build_age_days: 30,
            ..Options::default()
        },
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(old.items.len(), 1);
    assert!(old.items[0].reason.contains("30 日"));
}

#[test]
fn common_caches_and_scope_filter_are_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".m2/repository/lib.jar", 100);
    write(dir.path(), ".gradle/caches/java.bin", 120);
    let caches = user_caches(|key| (key == "USERPROFILE").then(|| dir.path().into()));
    let report = scan(
        &Local,
        &[dir.path().into()],
        &caches,
        &Options {
            ecosystem: Some(Ecosystem::Java),
            ..all()
        },
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.items.iter().map(|c| c.bytes).sum::<u64>(), 220);
    let report = scan(&Local, &[], &caches, &all(), &|_, _| true).unwrap();
    assert_eq!(report.items.len(), 2);
    let cancelled = scan(&Local, &[dir.path().into()], &caches, &all(), &|_, _| false).unwrap_err();
    assert!(crate::is_cancelled(&cancelled));
}

#[cfg(unix)]
#[test]
fn links_do_not_escape_roots_or_contribute_to_sizes() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(root.path(), "node_modules/small", 10);
    write(outside.path(), "huge", 9999);
    symlink(outside.path(), root.path().join("node_modules/outside")).unwrap();
    symlink(root.path(), root.path().join("loop")).unwrap();
    symlink(outside.path(), root.path().join("elsewhere")).unwrap();
    let result = scan(&Local, &[root.path().into()], &[], &all(), &|_, _| true).unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].bytes, 10);
    assert_eq!(result.items[0].files, 1);
}

#[test]
fn container_queries_include_stopped_containers_and_deduplicate_tags() {
    let used = "a".repeat(64);
    let unused = "b".repeat(64);
    let container = "c".repeat(64);
    for engine in [Ecosystem::Docker, Ecosystem::Podman] {
        let result = containers(engine, 1, &|_, args| {
            match &args[..2] {
                ["image", "ls"] => Ok(format!("sha256:{used}\tused:latest\t2GB\t2020\nsha256:{unused}\tunused:latest\t1.5GB\t2020\nsha256:{unused}\tunused:old\t1.5GB\t2020\n")),
                ["container", "ls"] => { assert!(args.contains(&"--all")); Ok(format!("{container}\n")) },
                ["container", "inspect"] => { assert_eq!(args.last(), Some(&container.as_str())); Ok(format!("sha256:{used}\n")) },
                ["system", "df"] => Ok("Build Cache\t500MB\t200MB (40%)\n".into()),
                _ => panic!("unexpected command: {args:?}"),
            }
        }).unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].bytes, 1_500_000_000);
        assert!(result.items[0].reason.contains("unused:old"));
        assert!(result.items[0].advice.ends_with(&unused));
    }
}

#[test]
fn unknown_usage_is_not_treated_as_unused_and_no_shell_input_is_accepted() {
    let result = containers(Ecosystem::Docker, 0, &|_, args| match &args[..2] {
        ["image", "ls"] => Ok(format!("{}\timage:tag\t1GB\t2020\n", "a".repeat(64))),
        ["container", "ls"] => Err(io::Error::other("daemon stopped")),
        ["system", "df"] => Ok(String::new()),
        _ => panic!("unexpected command"),
    })
    .unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.warning_count, 1);
    let result = containers(Ecosystem::Docker, 0, &|_, args| match &args[..2] {
        ["image", "ls"] => Ok("--force & echo malicious\tname\t1GB\t2020\n".into()),
        ["container", "ls"] | ["system", "df"] => Ok(String::new()),
        _ => panic!("unexpected command"),
    })
    .unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.warning_count, 1);
    assert_eq!(parse_size("1.5 GB"), Some(1_500_000_000));
    assert_eq!(parse_size("1.5GiB"), Some(1_610_612_736));
    assert_eq!(parse_size("N/A"), None);
    assert_eq!(parse_size("-1GB"), None);
}

#[test]
fn csv_handles_commas_quotes_multiline_and_formula_prefixes() {
    let mut report = Report::default();
    report.items.push(Candidate {
        ecosystem: Ecosystem::Node,
        kind: "cache".into(),
        location: Location::Folder("=bad,\"path\n".into()),
        bytes: 5,
        modified: 0,
        files: 1,
        incomplete: false,
        reason: "line1\nline2".into(),
        advice: "review".into(),
    });
    let csv = report.csv();
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("\"'=bad,\"\"path\n\""));
    assert!(csv.contains("\"line1\nline2\""));
}

#[test]
fn disappeared_files_are_reported_as_incomplete_and_cancel_is_propagated() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "node_modules/gone", 99);
    let removed = std::cell::Cell::new(false);
    let report = scan(&Local, &[dir.path().into()], &[], &all(), &|_, path| {
        if path.file_name().is_some_and(|n| n == "gone") && !removed.replace(true) {
            std::fs::remove_file(path).unwrap();
        }
        true
    })
    .unwrap();
    assert_eq!(report.items.len(), 1);
    assert!(report.items[0].incomplete);
    assert_eq!(report.items[0].bytes, 0);
    assert_eq!(report.warning_count, 1);
    let result = containers(Ecosystem::Docker, 0, &|_, args| match &args[..2] {
        ["image", "ls"] => Ok(String::new()),
        ["container", "ls"] => Err(crate::cancelled()),
        _ => panic!("no further command may run after cancellation"),
    });
    assert!(crate::is_cancelled(&result.unwrap_err()));
}
