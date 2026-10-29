use super::*;

#[test]
fn mnt_and_windows_drives_are_included_in_wsl_size_scans() {
    let root = Path::new(r"\\wsl.localhost\Ubuntu\");
    for path in [
        r"\\wsl.localhost\Ubuntu\mnt",
        r"\\wsl.localhost\Ubuntu\mnt\c",
        r"\\wsl.localhost\Ubuntu\mnt\c\Users",
    ] {
        assert!(!exclude_size_mount(root, Path::new(path)), "{path}");
    }
    assert!(exclude_size_mount(
        root,
        Path::new(r"\\wsl.localhost\Ubuntu\proc")
    ));
}

#[test]
fn command_arguments_keep_paths_and_secrets_separate() {
    let loc = Location {
        distro: "Ubuntu".into(),
        linux: "/tmp/a'$(id); name".into(),
    };
    let command = command(&loc, Some("alice"));
    let args: Vec<_> = command
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(args.windows(2).any(|a| a == ["--distribution", "Ubuntu"]));
    assert!(args.windows(2).any(|a| a == ["--user", "alice"]));
    assert!(!args.iter().any(|a| a.contains("$(id)")));
    assert!(APPLY_SCRIPT.contains("/proc/self/fd/3"));
}

#[test]
#[ignore = "requires a real Ubuntu WSL distribution and its UNC provider"]
fn real_wsl_permissions_and_windows_file_operations() {
    let cancel = AtomicBool::new(false);
    assert!(distributions().unwrap().iter().any(|d| d == "Ubuntu"));
    let root = Location {
        distro: "Ubuntu".into(),
        linux: "/".into(),
    };
    let raw = capture(
        command(&root, None).args(["/usr/bin/mktemp", "-d", "/tmp/yyfilemanager-wsl-XXXXXX"]),
        None,
        &cancel,
    )
    .unwrap();
    let path = String::from_utf8(raw).unwrap().trim().to_owned();
    assert!(path.starts_with("/tmp/yyfilemanager-wsl-") && !path[5..].contains('/'));
    struct Fixture(Location);
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(
                self.0.linux.starts_with("/tmp/yyfilemanager-wsl-")
                    && !self.0.linux[5..].contains('/')
            );
            let _ = capture(
                command(&self.0, None).args(["/usr/bin/rm", "-rf", "--", &self.0.linux]),
                None,
                &AtomicBool::new(false),
            );
        }
    }
    let fixture = Fixture(Location {
        distro: "Ubuntu".into(),
        linux: path,
    });
    let unc = PathBuf::from(fixture.0.unc());
    std::fs::create_dir(unc.join("nested")).unwrap();
    let file = unc.join("quoted ' name;$(touch PWN).txt");
    std::fs::write(&file, [1; 64]).unwrap();
    std::fs::write(unc.join("nested/.hidden"), [2; 128]).unwrap();
    std::fs::copy(&file, unc.join("copy.txt")).unwrap();
    std::fs::rename(unc.join("copy.txt"), unc.join("moved.txt")).unwrap();
    std::fs::remove_file(unc.join("moved.txt")).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), vec![1; 64]);
    let sizes = yy_files::folder_size::scan(&yy_files::Local, &unc, &|| false, |_, _| {}).unwrap();
    assert_eq!((sizes.bytes, sizes.files, sizes.errors), (192, 2, 0));
    // /mnt is included through WSL's own provider, without a Windows-path
    // fallback. That provider can legitimately deny some mounted-drive reads.
    let mounted = tempfile::tempdir().unwrap();
    std::fs::write(mounted.path().join("mnt-visible"), [0; 37]).unwrap();
    let mapped = capture(
        command(&root, None).args([
            "/usr/bin/wslpath",
            "-a",
            "-u",
            &mounted.path().to_string_lossy(),
        ]),
        None,
        &cancel,
    )
    .unwrap();
    let mapped = String::from_utf8(mapped).unwrap().trim().to_owned();
    assert!(mapped.starts_with("/mnt/"));
    let mounted = Location {
        distro: "Ubuntu".into(),
        linux: mapped,
    };
    let mounted_unc = PathBuf::from(mounted.unc());
    let mounted_size = yy_files::folder_size::scan_with(
        &yy_files::Local,
        &mounted_unc,
        &|p| exclude_size_mount(&mounted_unc, p),
        &|| false,
        |_, _| {},
    );
    match mounted_size {
        Ok(size) => {
            assert_eq!((size.bytes, size.errors), (37, 0));
            eprintln!("WSL /mnt fixture read through the UNC provider: 37 bytes");
        }
        Err(error) => {
            assert_eq!(
                error.kind(),
                io::ErrorKind::PermissionDenied,
                "unexpected WSL provider error"
            );
            eprintln!("WSL /mnt provider denied this fixture; no Windows path fallback was used");
        }
    }
    let loc = location(&file).unwrap();
    let before = read(&loc, &cancel).unwrap();
    let change = Change::new(
        &before.permissions.uid.to_string(),
        &before.permissions.gid.to_string(),
        "0600",
        &before.permissions,
    )
    .unwrap();
    apply(&loc, &before.user, &change, None, &cancel).unwrap();
    let after = read(&loc, &cancel).unwrap();
    assert_eq!(after.permissions.mode, 0o600);
    // Root is used only for this test-owned temporary file. The production
    // dialog always runs as the distro user read before displaying the dialog.
    let root_change = Change::new("0", "0", "0640", &after.permissions).unwrap();
    let secret = Secret::new("test-unused-root-password".into());
    let result = apply_as(&loc, &root_change, Some(&secret), Some("root"), &cancel).unwrap();
    assert_eq!((result.uid, result.gid, result.mode), (0, 0, 0o640));
    assert_eq!(read(&loc, &cancel).unwrap().permissions.uid, 0);
    // Keep the original inode alive while replacing its name.
    std::fs::rename(&file, unc.join("previous-inode")).unwrap();
    std::fs::write(&file, b"replacement").unwrap();
    assert!(apply_as(&loc, &root_change, Some(&secret), Some("root"), &cancel).is_err());
    let replacement = read(&loc, &cancel).unwrap();
    assert_ne!(
        replacement.permissions.fingerprint,
        root_change.expected.fingerprint
    );
    assert_ne!(replacement.permissions.uid, 0);
    assert!(!unc.join("PWN").exists());
    let link = Location {
        distro: "Ubuntu".into(),
        linux: format!("{}/link", fixture.0.linux),
    };
    capture(
        command(&link, None).args(["/usr/bin/ln", "-s", "--", &loc.linux, &link.linux]),
        None,
        &cancel,
    )
    .unwrap();
    let info = read(&link, &cancel).unwrap();
    assert!(info.permissions.symlink());
    assert!(Change::new("0", "0", "0777", &info.permissions).is_err());
}

#[test]
#[ignore = "requires an interactive Windows Credential Manager session"]
fn real_credential_manager_roundtrip_is_scoped_and_deletable() {
    use yy_remote::{PasswordStore, SavedPassword};
    let temp = tempfile::tempdir().unwrap();
    let key = format!(
        "wsl-sudo/test-{}-{}",
        std::process::id(),
        temp.path().file_name().unwrap().to_string_lossy()
    );
    struct Key(String);
    impl Drop for Key {
        fn drop(&mut self) {
            crate::credstore::WindowsCredentials.delete(&self.0);
        }
    }
    let key = Key(key);
    let password = "yy-test-only-not-a-user-password";
    crate::credstore::WindowsCredentials
        .save(
            &key.0,
            &SavedPassword {
                user: "fixture".into(),
                password: password.into(),
            },
        )
        .unwrap();
    let saved = crate::credstore::WindowsCredentials.load(&key.0).unwrap();
    assert!(
        saved.user == "fixture" && saved.password == password,
        "test credential roundtrip failed"
    );
    drop(Secret::new(saved.password));
    crate::credstore::WindowsCredentials.delete(&key.0);
    assert!(crate::credstore::WindowsCredentials.load(&key.0).is_none());
}
