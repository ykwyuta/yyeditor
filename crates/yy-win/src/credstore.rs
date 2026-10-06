//! リモート接続のパスワードを Windows の資格情報マネージャーに保存する（11 章 4.7）。
//!
//! 「汎用資格情報」として、名前 `yyeditor/<キー>`（例: `yyeditor/ssh/yamada@build01:22`、
//! `yyeditor/proxy/http://proxy:8080`、`yyeditor/key/C:\Users\…\.ssh\id_ed25519`）で
//! このパソコンのこのユーザーにだけ保存する（`CRED_PERSIST_LOCAL_MACHINE`。ほかのパソコンへは
//! 移らない）。中身は Windows が利用者の資格で暗号化して持つ。コントロール パネルの
//! 「資格情報マネージャー」→「Windows 資格情報」からも確認・削除できる。

use std::io;

use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW,
    CredFree, CredReadW, CredWriteW,
};
use windows::core::{HSTRING, PWSTR};
use yy_remote::{PasswordStore, SavedPassword};

/// 資格情報の名前の先頭
const PREFIX: &str = "yyeditor/";

/// Windows の資格情報マネージャー。
pub(crate) struct WindowsCredentials;

fn target(key: &str) -> HSTRING {
    HSTRING::from(format!("{PREFIX}{key}"))
}

/// `PWSTR` を文字列にする（null なら空）。
fn pwstr(p: PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string().unwrap_or_default() }
    }
}

impl PasswordStore for WindowsCredentials {
    fn load(&self, key: &str) -> Option<SavedPassword> {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe { CredReadW(&target(key), CRED_TYPE_GENERIC, None, &mut p) }.ok()?;
        let c = unsafe { &*p };
        // パスワードは UTF-16 で持つ（資格情報マネージャーの表示と同じ形）
        let blob = if c.CredentialBlob.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize) }
        };
        let units: Vec<u16> = blob
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let saved = SavedPassword {
            user: pwstr(c.UserName),
            password: String::from_utf16_lossy(&units),
        };
        unsafe { CredFree(p as *const _) };
        Some(saved)
    }

    fn save(&self, key: &str, saved: &SavedPassword) -> io::Result<()> {
        let mut name: Vec<u16> = format!("{PREFIX}{key}").encode_utf16().collect();
        name.push(0);
        let mut user: Vec<u16> = saved.user.encode_utf16().collect();
        user.push(0);
        let mut comment: Vec<u16> = "yyeditor のリモート接続（SSH）".encode_utf16().collect();
        comment.push(0);
        let mut blob: Vec<u8> = saved
            .password
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..CREDENTIALW::default()
        };
        let r = unsafe { CredWriteW(&cred, 0) };
        // 手元に残る写しを消す
        blob.fill(0);
        r.map_err(|e| io::Error::other(e.message()))
    }

    fn delete(&self, key: &str) {
        let _ = unsafe { CredDeleteW(&target(key), CRED_TYPE_GENERIC, None) };
    }
}

impl WindowsCredentials {
    /// yyeditor が保存した資格情報の名前（`yyeditor/` を除いたもの）。
    pub(crate) fn keys(&self) -> Vec<String> {
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
        let filter = HSTRING::from(format!("{PREFIX}*"));
        if unsafe { CredEnumerateW(&filter, None, &mut count, &mut list) }.is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..count as usize {
            let c = unsafe { &**list.add(i) };
            if c.Type == CRED_TYPE_GENERIC
                && let Some(key) = pwstr(c.TargetName).strip_prefix(PREFIX)
            {
                out.push(key.to_owned());
            }
        }
        unsafe { CredFree(list as *const _) };
        out.sort();
        out
    }
}
