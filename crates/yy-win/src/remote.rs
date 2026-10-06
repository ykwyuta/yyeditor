//! SSH 接続先のファイルの編集（11 章）: 接続、取り寄せ、保存、接続中の問い合わせ。
//!
//! 接続・取り寄せ・送り出しはバックグラウンドのスレッドで行い、その間 UI スレッドは
//! [`wait`] でメッセージを処理し続ける（キーボードとマウスの操作は受け付けず、Esc で中止）。
//! パスワードやホスト鍵の確認は、接続中のスレッドが [`WM_APP_REMOTE_PROMPT`] でフレームに頼み、
//! フレームがダイアログを出して答えを返す。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;
use yy_config::RemoteConfig;
use yy_config::workspace;
use yy_core::{Document, Encoding, FileId, RemoteFile, SaveError, Upload};
use yy_remote::ssh_config::{HostOverride, Resolver};
use yy_remote::uri::{RemoteUri, Target};
use yy_remote::{
    AgentFiles, ConnectLog, Connector, ConnectorFactory, ConnectorOptions, FileInfo, HostKeyCheck,
    HostKeyQuestion, PassphraseRequest, PasswordAnswer, PasswordRequest, Prompter, RemoteFs,
    Session, SftpFs, Transport, UploadOutcome,
};

use crate::app::with_app;
use crate::util::{group_digits, human_size};

/// 接続中のスレッドからの問い合わせ（`LPARAM` は `Box<PromptRequest>`）
pub(crate) const WM_APP_REMOTE_PROMPT: u32 = WM_APP + 30;
/// バックグラウンドの処理の進み・終わりを UI スレッドに知らせる
pub(crate) const WM_APP_REMOTE_WAKE: u32 = WM_APP + 31;

/// これより大きなファイルは、全体を取り寄せる前に確かめる（M9.2 で表示範囲だけの取り寄せにする）
const LARGE_FILE: u64 = 256 << 20;

/// 接続先の状態（アプリに 1 つ）。
pub(crate) struct RemoteState {
    factory: Option<ConnectorFactory>,
    connector: Option<Arc<dyn Connector>>,
    sessions: Vec<(Target, Arc<Session>)>,
    /// エージェントを使わない接続（ターミナル）
    transports: Vec<(Target, Arc<dyn Transport>)>,
    config: RemoteConfig,
    /// 最後に使った場所（ファイル選択の初期値）
    pub(crate) last: Option<RemoteUri>,
    /// 接続先にエージェントを置いて使う（エディタは常に。ターミナルとファイル転送は設定・メニュー）。
    /// 使わなければ、一覧・ファイル操作は SFTP（[`fs`]）
    pub(crate) use_agent: bool,
    /// エージェントを使わないときの一覧・ファイル操作（SFTP）
    sftps: Vec<(Target, Arc<SftpFs>)>,
}

impl RemoteState {
    pub(crate) fn new(factory: Option<ConnectorFactory>, config: RemoteConfig) -> RemoteState {
        RemoteState {
            factory,
            connector: None,
            sessions: Vec::new(),
            transports: Vec::new(),
            config,
            last: None,
            use_agent: true,
            sftps: Vec::new(),
        }
    }

    /// SSH の実装が組み込まれているか。
    pub(crate) fn available(&self) -> bool {
        self.factory.is_some()
    }

    fn connector(&mut self) -> Option<Arc<dyn Connector>> {
        if self.connector.is_none() {
            let factory = self.factory.as_ref()?;
            let dir = yy_config::config_dir().unwrap_or_else(std::env::temp_dir);
            let mut extra = Vec::new();
            if self.config.read_ssh_known_hosts
                && let Some(home) = std::env::home_dir()
            {
                extra.push(yy_remote::ssh_config::user_known_hosts(&home));
            }
            let opts = ConnectorOptions {
                known_hosts: dir.join("known_hosts"),
                extra_known_hosts: extra,
                keepalive: Duration::from_secs(u64::from(self.config.keepalive_secs.max(1))),
                passwords: self.config.remember_passwords.then(|| {
                    Arc::new(crate::credstore::WindowsCredentials)
                        as Arc<dyn yy_remote::PasswordStore>
                }),
            };
            self.connector = Some(factory(&opts));
        }
        self.connector.clone()
    }

    fn resolver(&self) -> Resolver {
        let mut r = Resolver::from_environment(self.config.read_ssh_config);
        r.hosts = self
            .config
            .host
            .iter()
            .map(|(name, h)| {
                (
                    name.clone(),
                    HostOverride {
                        hostname: h.hostname.clone(),
                        user: h.user.clone(),
                        port: h.port,
                        identity_file: h.identity_file.clone(),
                        proxy_jump: h.proxy_jump.clone(),
                        proxy: h.proxy.clone(),
                        agent_dir: h.agent_dir.clone(),
                    },
                )
            })
            .collect();
        r.agent_dir = Some(self.config.agent_dir.clone()).filter(|d| !d.trim().is_empty());
        r.proxy = Some(self.config.proxy.clone()).filter(|p| !p.trim().is_empty());
        r
    }

    /// 接続済みの `target` のセッション（切れていれば捨てる）。
    fn live_session(&mut self, target: &Target) -> Option<Arc<Session>> {
        self.sessions.retain(|(_, s)| !s.is_closed());
        self.sessions
            .iter()
            .find(|(t, _)| t.same(target))
            .map(|(_, s)| s.clone())
    }

    /// 接続済みなら `target` のセッション（接続はしない）。
    pub(crate) fn connected(&mut self, target: &Target) -> Option<Arc<Session>> {
        self.live_session(target)
    }

    /// 接続先の候補（設定の名前、`~/.ssh/config` の Host、接続中・履歴の接続先）。
    pub(crate) fn known_targets(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |s: String| {
            if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
                out.push(s);
            }
        };
        if let Some(u) = &self.last {
            push(u.target().to_string());
        }
        for (t, _) in &self.sessions {
            push(t.to_string());
        }
        for p in crate::recentdlg::ListKind::History.load().items() {
            if let Some(u) = p.to_str().and_then(RemoteUri::parse) {
                push(u.target().to_string());
            }
        }
        for name in self.config.host.keys() {
            push(name.clone());
        }
        for name in ssh_config_hosts(&self.resolver().ssh_config) {
            push(name);
        }
        out
    }
}

/// `~/.ssh/config` の `Host` に書かれた名前（ワイルドカードと否定を除く）。
fn ssh_config_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line
            .get(..5)
            .filter(|k| k.eq_ignore_ascii_case("host "))
            .map(|_| &line[5..])
        else {
            continue;
        };
        for name in rest.split_whitespace() {
            if !name.contains(['*', '?', '!']) {
                out.push(name.trim_matches('"').to_owned());
            }
        }
    }
    out
}

// ---- バックグラウンドの処理を待つ ---------------------------------------------

/// `HWND` をスレッドに渡すための入れ物（`PostMessageW` にだけ使う）。
#[derive(Clone, Copy)]
struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn post(self, msg: u32, lparam: isize) -> bool {
        unsafe {
            PostMessageW(Some(HWND(self.0 as *mut _)), msg, WPARAM(0), LPARAM(lparam)).is_ok()
        }
    }
}

/// バックグラウンドの処理に渡す、中止の確認と進みの報告。
pub(crate) struct Work {
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<Option<String>>>,
    frame: SendHwnd,
    last: Mutex<Instant>,
}

impl Work {
    /// 利用者が中止したか（Esc）。
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 表示する進みを知らせる（頻度は 0.1 秒に 1 回まで）。
    pub(crate) fn report(&self, text: String) {
        let mut last = self.last.lock().unwrap();
        if last.elapsed() < Duration::from_millis(100) {
            return;
        }
        *last = Instant::now();
        *self.status.lock().unwrap() = Some(text);
        self.frame.post(WM_APP_REMOTE_WAKE, 0);
    }
}

/// リモート接続を使うアプリ（エディタ・ターミナル）。
pub(crate) struct Host {
    /// 問い合わせ・進みの知らせを受けるフレームのウィンドウ
    pub frame: HWND,
    /// ステータスバーに表示する
    pub status: fn(&str),
}

thread_local! {
    static STATE: std::cell::RefCell<Option<RemoteState>> = const { std::cell::RefCell::new(None) };
    static HOST: std::cell::RefCell<Option<Host>> = const { std::cell::RefCell::new(None) };
}

/// 接続先の状態とアプリを登録する（UI スレッドで 1 回）。
pub(crate) fn install(state: RemoteState, host: Host) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

/// 接続先の状態を使う（モーダルな処理の間は借りたままにしないこと）。
pub(crate) fn with_state<R>(f: impl FnOnce(&mut RemoteState) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

/// SSH の実装が組み込まれているか。
pub(crate) fn available() -> bool {
    with_state(|r| r.available()).unwrap_or(false)
}

/// 最後に使った場所。
pub(crate) fn last() -> Option<RemoteUri> {
    with_state(|r| r.last.clone()).flatten()
}

pub(crate) fn set_last(uri: RemoteUri) {
    with_state(|r| r.last = Some(uri));
}

/// 接続済みなら `target` のセッション（接続はしない）。
pub(crate) fn connected(target: &Target) -> Option<Arc<Session>> {
    with_state(|r| r.connected(target)).flatten()
}

fn frame() -> HWND {
    HOST.with(|h| h.borrow().as_ref().map(|h| h.frame))
        .unwrap_or_default()
}

/// `f` をバックグラウンドで実行し、終わるまで UI のメッセージを処理しながら待つ。
///
/// 待っている間はキーボードとマウスの操作を受け付けない（Esc で [`Work::cancelled`] になる）。
/// `show` には進みの表示（[`Work::report`]）が渡される。
pub(crate) fn wait<T: Send + 'static>(
    show: &dyn Fn(&str),
    f: impl FnOnce(&Work) -> T + Send + 'static,
) -> T {
    let frame = SendHwnd(frame().0 as isize);
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(None));
    let work = Work {
        cancel: cancel.clone(),
        status: status.clone(),
        frame,
        last: Mutex::new(Instant::now() - Duration::from_secs(1)),
    };
    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        let r = f(&work);
        let _ = tx.send(r);
        frame.post(WM_APP_REMOTE_WAKE, 0);
    });
    let mut quit = None;
    let mut msg = MSG::default();
    loop {
        if let Ok(r) = rx.try_recv() {
            if let Some(code) = quit {
                unsafe { PostQuitMessage(code) };
            }
            return r;
        }
        unsafe {
            match GetMessageW(&mut msg, None, 0, 0).0 {
                -1 => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                0 => {
                    // 終了の要求は、処理が終わってからメッセージループに戻す
                    quit = Some(msg.wParam.0 as i32);
                    cancel.store(true, Ordering::Relaxed);
                    continue;
                }
                _ => {}
            }
            let m = msg.message;
            if m == WM_APP_REMOTE_WAKE {
                if let Some(text) = status.lock().unwrap().take() {
                    show(&text);
                }
                continue;
            }
            let input = (WM_KEYFIRST..=WM_KEYLAST).contains(&m)
                || (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m)
                || (WM_NCMOUSEMOVE..=WM_NCXBUTTONDBLCLK).contains(&m);
            if input {
                if m == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize {
                    cancel.store(true, Ordering::Relaxed);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 進みをステータスバーに表示する。
pub(crate) fn show_status(text: &str) {
    if let Some(f) = HOST.with(|h| h.borrow().as_ref().map(|h| h.status)) {
        f(text);
    }
}

// ---- 接続中の問い合わせ -----------------------------------------------------------

enum Question {
    HostKey(HostKeyQuestion),
    Password(String),
    /// 保存を選べるパスワード・パスフレーズ（`check` は「保存する」のチェックボックスの文言。
    /// `None` ならチェックボックスを出さない）
    SavableSecret {
        title: &'static str,
        prompt: String,
        check: Option<&'static str>,
    },
    Passphrase(PathBuf),
    Keyboard {
        user_host: String,
        name: String,
        instructions: String,
        prompts: Vec<(String, bool)>,
    },
}

enum Answer {
    Yes(bool),
    /// パスワードと「保存する」の選択
    Secret(Option<(String, bool)>),
    Text(Option<String>),
    Texts(Option<Vec<String>>),
}

pub(crate) struct PromptRequest {
    question: Question,
    reply: Sender<Answer>,
}

/// 問い合わせを UI スレッドに頼む [`Prompter`]。
struct UiPrompter {
    frame: SendHwnd,
}

impl UiPrompter {
    fn ask(&self, question: Question) -> Option<Answer> {
        let (tx, rx) = bounded(1);
        let req = Box::into_raw(Box::new(PromptRequest {
            question,
            reply: tx,
        }));
        if !self.frame.post(WM_APP_REMOTE_PROMPT, req as isize) {
            // 届かなかった要求は自分で片付ける
            drop(unsafe { Box::from_raw(req) });
            return None;
        }
        rx.recv().ok()
    }
}

impl Prompter for UiPrompter {
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool {
        matches!(
            self.ask(Question::HostKey(q.clone())),
            Some(Answer::Yes(true))
        )
    }

    fn password(&self, user_host: &str) -> Option<String> {
        match self.ask(Question::Password(user_host.to_owned())) {
            Some(Answer::Text(t)) => t,
            _ => None,
        }
    }

    fn ask_password(&self, req: &PasswordRequest<'_>) -> Option<PasswordAnswer> {
        if !req.can_save && req.note.is_none() {
            return self.password(req.label).map(|password| PasswordAnswer {
                password,
                save: false,
            });
        }
        self.ask_secret(
            "パスワード",
            req.note,
            &format!("{} のパスワード:", req.label),
            req.can_save
                .then_some("このパスワードを保存する（Windows の資格情報マネージャー）"),
        )
    }

    fn ask_passphrase(&self, req: &PassphraseRequest<'_>) -> Option<PasswordAnswer> {
        if !req.can_save && req.note.is_none() {
            return self
                .passphrase(req.key_file)
                .map(|password| PasswordAnswer {
                    password,
                    save: false,
                });
        }
        self.ask_secret(
            "秘密鍵のパスフレーズ",
            req.note,
            &format!("秘密鍵 {} のパスフレーズ:", key_name(req.key_file)),
            req.can_save
                .then_some("このパスフレーズを保存する（Windows の資格情報マネージャー）"),
        )
    }

    fn passphrase(&self, key: &Path) -> Option<String> {
        match self.ask(Question::Passphrase(key.to_owned())) {
            Some(Answer::Text(t)) => t,
            _ => None,
        }
    }

    fn keyboard_interactive(
        &self,
        user_host: &str,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        match self.ask(Question::Keyboard {
            user_host: user_host.to_owned(),
            name: name.to_owned(),
            instructions: instructions.to_owned(),
            prompts: prompts.to_vec(),
        }) {
            Some(Answer::Texts(t)) => t,
            _ => None,
        }
    }
}

impl UiPrompter {
    /// 保存を選べるパスワード・パスフレーズを尋ねる（`note` は問いの前に添える）。
    fn ask_secret(
        &self,
        title: &'static str,
        note: Option<&str>,
        prompt: &str,
        check: Option<&'static str>,
    ) -> Option<PasswordAnswer> {
        let prompt = match note {
            Some(n) => format!("{n}\n{prompt}"),
            None => prompt.to_owned(),
        };
        match self.ask(Question::SavableSecret {
            title,
            prompt,
            check,
        }) {
            Some(Answer::Secret(Some((password, save)))) => Some(PasswordAnswer { password, save }),
            _ => None,
        }
    }
}

/// 秘密鍵のファイル名（表示用）。
fn key_name(key: &Path) -> String {
    key.file_name().map_or_else(
        || key.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// 問い合わせのダイアログの持ち主（ファイル選択などのダイアログを開いていればそれ）。
fn prompt_owner(frame: HWND) -> HWND {
    unsafe {
        let active = GetLastActivePopup(frame);
        if active.0.is_null() { frame } else { active }
    }
}

/// [`WM_APP_REMOTE_PROMPT`] を処理する（フレームのウィンドウプロシージャから）。
pub(crate) fn on_prompt(frame: HWND, lparam: LPARAM) {
    let req = unsafe { Box::from_raw(lparam.0 as *mut PromptRequest) };
    let owner = prompt_owner(frame);
    let answer = match req.question {
        Question::HostKey(q) => Answer::Yes(confirm_host_key(owner, &q)),
        Question::Password(user_host) => Answer::Text(crate::goto::prompt_secret(
            owner,
            "パスワード",
            &format!("{user_host} のパスワード:"),
        )),
        Question::SavableSecret {
            title,
            prompt,
            check,
        } => Answer::Secret(match check {
            Some(check) => crate::goto::prompt_secret_with_check(owner, title, &prompt, check),
            None => crate::goto::prompt_secret(owner, title, &prompt).map(|p| (p, false)),
        }),
        Question::Passphrase(key) => {
            let name = key_name(&key);
            Answer::Text(crate::goto::prompt_secret(
                owner,
                "秘密鍵のパスフレーズ",
                &format!("秘密鍵 {name} のパスフレーズ:"),
            ))
        }
        Question::Keyboard {
            user_host,
            name,
            instructions,
            prompts,
        } => {
            let title = if name.trim().is_empty() {
                format!("{user_host} の認証")
            } else {
                name
            };
            if !instructions.trim().is_empty() {
                crate::util::info_box(owner, instructions.trim());
            }
            let mut answers = Some(Vec::new());
            for (prompt, echo) in prompts {
                let a = if echo {
                    crate::goto::prompt_text(owner, &title, prompt.trim(), "")
                } else {
                    crate::goto::prompt_secret(owner, &title, prompt.trim())
                };
                match (a, answers.as_mut()) {
                    (Some(a), Some(v)) => v.push(a),
                    _ => {
                        answers = None;
                        break;
                    }
                }
            }
            Answer::Texts(answers)
        }
    };
    let _ = req.reply.send(answer);
}

fn confirm_host_key(owner: HWND, q: &HostKeyQuestion) -> bool {
    let host = if q.port == 22 {
        q.host.clone()
    } else {
        format!("{}:{}", q.host, q.port)
    };
    match &q.check {
        HostKeyCheck::Unknown => {
            let text = format!(
                "{host} に初めて接続します。\n\n\
                 ホスト鍵の種類: {}\n指紋: {}\n\n\
                 この指紋が、接続先の管理者から知らされたもの（ssh-keygen -lf で表示されるもの）と\n\
                 一致する場合だけ「はい」を選んでください。\n\n\
                 接続して、この鍵を記録しますか？",
                q.algorithm, q.fingerprint
            );
            message_box(owner, &text, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES
        }
        HostKeyCheck::Changed { file, line } => {
            let text = format!(
                "警告: {host} のホスト鍵が記録と違います。\n\n\
                 通信を盗み見られている（中間者攻撃の）おそれがあるため、接続しません。\n\n\
                 記録: {} の {line} 行目\n受け取った鍵: {} {}\n\n\
                 接続先の鍵が正しく変更されたことを管理者に確かめた場合は、記録の該当する行を\n\
                 削除してから接続し直してください。",
                file.display(),
                q.algorithm,
                q.fingerprint
            );
            message_box(owner, &text, MB_OK | MB_ICONERROR);
            false
        }
    }
}

fn message_box(owner: HWND, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            &HSTRING::from(crate::util::app_name()),
            style,
        )
    }
}

// ---- 接続 -------------------------------------------------------------------------

/// `target` のセッション。なければ接続してエージェントを起動する（接続中は `show` に表示）。
pub(crate) fn session(target: &Target, show: &dyn Fn(&str)) -> Result<Arc<Session>, String> {
    if let Some(s) = with_state(|r| r.live_session(target)).flatten() {
        return Ok(s);
    }
    let files = AgentFiles::beside_exe();
    let s = connect(target, show, move |connector, spec, prompter, log| {
        Session::connect(connector.as_ref(), &spec, &prompter, &files, &log)
    })?;
    let s = Arc::new(s);
    with_state(|r| r.sessions.push((target.clone(), s.clone())));
    Ok(s)
}

/// 接続先にエージェントを置いて使うか（エディタは常に。ターミナル・ファイル転送は設定・メニュー）。
pub(crate) fn use_agent() -> bool {
    with_state(|r| r.use_agent).unwrap_or(true)
}

/// エージェントを使うかを変える（変えた後の一覧・ファイル操作から）。
pub(crate) fn set_use_agent(on: bool) {
    with_state(|r| r.use_agent = on);
}

/// `target` のファイル操作（一覧・情報・作成・名前の変更・削除）。エージェントを使う設定なら
/// エージェント（[`session`]）、でなければ SFTP（接続先に何も置かない）。
pub(crate) fn fs(target: &Target, show: &dyn Fn(&str)) -> Result<Arc<dyn RemoteFs>, String> {
    if use_agent() {
        return session(target, show).map(|s| s as Arc<dyn RemoteFs>);
    }
    let cached = with_state(|r| {
        r.sftps.retain(|(_, f)| !f.is_closed());
        r.sftps
            .iter()
            .find(|(t, _)| t.same(target))
            .map(|(_, f)| f.clone())
    })
    .flatten();
    if let Some(f) = cached {
        return Ok(f);
    }
    let t = transport(target, show)?;
    show(&format!("{target} で SFTP を始めています…（Esc で中止）"));
    let r = wait(show, move |_| SftpFs::connect(t.as_ref()));
    show("");
    let f = Arc::new(r.map_err(|e| {
        format!(
            "{target} で SFTP を使えません（接続先で sftp-server が使えないなど）。\n\
             エージェントを使う設定にすると、SFTP がなくても一覧を表示できます。\n{e}"
        )
    })?);
    with_state(|r| r.sftps.push((target.clone(), f.clone())));
    Ok(f)
}

/// 転送の照合に使う、接続先のエージェントで SHA-256 を計算する関数（どのスレッドからでも
/// 呼べる）。転送に使っている接続にエージェントを配置して起動する（接続ごとに 1 つ）。
pub(crate) type BackgroundHasher =
    Arc<dyn Fn(&Arc<dyn Transport>, &[u8], u64) -> std::io::Result<Vec<u8>> + Send + Sync>;

pub(crate) fn background_hasher(target: &Target) -> BackgroundHasher {
    let agent_dir = with_state(|r| r.resolver().resolve(target).agent_dir).flatten();
    let files = AgentFiles::beside_exe();
    let target = target.clone();
    let cache: Mutex<Option<(usize, Arc<Session>)>> = Mutex::new(None);
    Arc::new(move |t, path, len| {
        let key = Arc::as_ptr(t) as *const () as usize;
        let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
        let s = match &*c {
            Some((k, s)) if *k == key && !s.is_closed() => s.clone(),
            _ => {
                let log = ConnectLog::new();
                let r = Session::start(t.clone(), &files, agent_dir.as_deref(), &log);
                save_log(&target, &log);
                let s = Arc::new(r?);
                *c = Some((key, s.clone()));
                s
            }
        };
        drop(c);
        s.hash(path, len)
    })
}

/// `target` への SSH の接続（ターミナル用。エージェントは使わない）。接続済みのセッションが
/// あればその接続を使い、なければ接続する（接続中は `show` に表示）。
pub(crate) fn transport(
    target: &Target,
    show: &dyn Fn(&str),
) -> Result<Arc<dyn Transport>, String> {
    let live = with_state(|r| {
        if let Some(s) = r.live_session(target) {
            return Some(s.transport());
        }
        r.transports.retain(|(_, t)| !t.is_closed());
        r.transports
            .iter()
            .find(|(t, _)| t.same(target))
            .map(|(_, t)| t.clone())
    })
    .flatten();
    if let Some(t) = live {
        return Ok(t);
    }
    let t = connect(target, show, move |connector, spec, prompter, log| {
        connector.connect(&spec, &prompter, &log)
    })?;
    with_state(|r| r.transports.push((target.clone(), t.clone())));
    Ok(t)
}

/// `target` に接続する（`op` を接続中の問い合わせを受けるスレッドで行う）。接続の記録を残し、
/// 失敗したらその最後の部分を添えた説明を返す。
fn connect<T: Send + 'static>(
    target: &Target,
    show: &dyn Fn(&str),
    op: impl FnOnce(
        Arc<dyn Connector>,
        yy_remote::HostSpec,
        UiPrompter,
        Arc<ConnectLog>,
    ) -> std::io::Result<T>
    + Send
    + 'static,
) -> Result<T, String> {
    let log = Arc::new(ConnectLog::new());
    let prepared = with_state(|r| {
        log.note(format!(
            "yyeditor {}、~/.ssh/config: {}、~/.ssh/known_hosts: {}",
            env!("CARGO_PKG_VERSION"),
            if r.config.read_ssh_config {
                "読む"
            } else {
                "読まない"
            },
            if r.config.read_ssh_known_hosts {
                "読む"
            } else {
                "読まない"
            }
        ));
        if let Some(name) = r
            .config
            .host
            .keys()
            .find(|n| n.eq_ignore_ascii_case(&target.host))
        {
            log.note(format!(
                "yyeditor の接続設定 [remote.host.{name}] を使います"
            ));
        }
        let spec = r.resolver().resolve(target);
        r.connector().map(|c| (c, spec))
    });
    let (connector, spec) = match prepared {
        Some(Some(p)) => p,
        Some(None) => {
            return Err("この yyeditor には SSH の機能が組み込まれていません。".into());
        }
        None => return Err("処理中のため接続できません。".into()),
    };
    show(&format!("{target} に接続しています…（Esc で中止）"));
    let prompter = UiPrompter {
        frame: SendHwnd(frame().0 as isize),
    };
    let l = log.clone();
    let r = wait(show, move |_| op(connector, spec, prompter, l));
    match r {
        Ok(v) => {
            log.note("接続しました");
            save_log(target, &log);
            show(&format!("{target} に接続しました"));
            Ok(v)
        }
        Err(e) => {
            log.note(format!("接続できませんでした: {e}"));
            let saved = save_log(target, &log);
            show("");
            let mut text = format!("{target} に接続できませんでした。\n\n{e}");
            if e.kind() != std::io::ErrorKind::Interrupted {
                text.push_str("\n\n―― 接続の記録（最後の部分）――\n");
                text.push_str(&log.tail(LOG_TAIL).join("\n"));
                if let Some(path) = saved {
                    text.push_str(&format!(
                        "\n\n記録の全体: {}\n（ヘルプ メニューの「リモート接続の記録を開く」でも開けます）",
                        path.display()
                    ));
                }
            }
            Err(text)
        }
    }
}

/// バックグラウンドのスレッドから接続する関数（転送の再接続に使う。13 章）。
pub(crate) type BackgroundConnect = Arc<
    dyn Fn(&yy_remote::log::TransferLog, u64) -> std::io::Result<Arc<dyn Transport>> + Send + Sync,
>;

/// `target` に、どのスレッドからでも接続できる関数を作る（UI スレッドで呼ぶ）。接続中の
/// 問い合わせ（パスワードなど）は UI スレッドのダイアログで尋ね、接続の記録は
/// `remote-ssh.log` と転送の記録の両方に残す。
pub(crate) fn background_connector(target: &Target) -> Result<BackgroundConnect, String> {
    let prepared = with_state(|r| {
        let spec = r.resolver().resolve(target);
        r.connector().map(|c| (c, spec))
    })
    .flatten();
    let Some((connector, spec)) = prepared else {
        return Err("SSH の機能が組み込まれていません。".into());
    };
    let prompter = UiPrompter {
        frame: SendHwnd(frame().0 as isize),
    };
    let target = target.clone();
    Ok(Arc::new(move |tlog, id| {
        let log = ConnectLog::new();
        let r = connector.connect(&spec, &prompter, &log);
        match &r {
            Ok(_) => log.note("接続しました"),
            Err(e) => log.note(format!("接続できませんでした: {e}")),
        }
        save_log(&target, &log);
        tlog.connect_log(Some(id), &log);
        r
    }))
}

/// 手元の日時（記録の行の先頭。`2026-10-06 12:34:56.789`）。
pub(crate) fn local_clock() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// 資格情報マネージャーに保存したパスワードを確かめて削除する（ヘルプ メニュー）。
pub(crate) fn forget_passwords(owner: HWND) {
    use yy_remote::PasswordStore;
    let store = crate::credstore::WindowsCredentials;
    let keys = store.keys();
    if keys.is_empty() {
        crate::util::info_box(
            owner,
            "保存したリモート接続のパスワード・パスフレーズはありません。",
        );
        return;
    }
    let shown: Vec<&str> = keys.iter().take(20).map(String::as_str).collect();
    let more = if keys.len() > shown.len() {
        format!("\n…ほか {} 件", keys.len() - shown.len())
    } else {
        String::new()
    };
    let text = format!(
        "Windows の資格情報マネージャーに保存した、次の {} 件のパスワード・パスフレーズを削除しますか？\n\n{}{more}\n\n\
         削除すると、次に接続するときに尋ねます。",
        keys.len(),
        shown.join("\n")
    );
    if message_box(owner, &text, MB_OKCANCEL | MB_ICONQUESTION) != IDOK {
        return;
    }
    for k in &keys {
        store.delete(k);
    }
    let left = store.keys().len();
    if left == 0 {
        crate::util::info_box(owner, "保存したパスワード・パスフレーズを削除しました。");
    } else {
        message_box(
            owner,
            &format!("{left} 件を削除できませんでした。"),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 接続に失敗したときに知らせる、接続の記録の行数
const LOG_TAIL: usize = 12;

/// 接続の記録のファイル（`%APPDATA%\yyeditor\logs\remote-ssh.log`）。
pub(crate) fn log_path() -> Option<PathBuf> {
    Some(yy_config::config_dir()?.join("logs").join("remote-ssh.log"))
}

/// 接続の記録をファイルに追記する。書けたらファイルのパスを返す（書けなくても接続は続ける）。
fn save_log(target: &Target, log: &ConnectLog) -> Option<PathBuf> {
    let path = log_path()?;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let header = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {target}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    yy_remote::log::append_to_file(&path, &header, log)
        .ok()
        .map(|()| path)
}

// ---- 開く ---------------------------------------------------------------------------

/// 開き方。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAs {
    /// 新しいタブで（既に開いていればそのタブへ）
    NewTab,
    /// 作業中のタブの文書を開き直す（文字コードの指定・バイナリとして）
    Replace,
}

/// 接続先のファイルを取り寄せて開く。
pub(crate) fn open(
    owner: HWND,
    uri: &RemoteUri,
    encoding: Option<Encoding>,
    raw: bool,
    how: OpenAs,
) -> Result<(), String> {
    let location = PathBuf::from(uri.to_string());
    if how == OpenAs::NewTab
        && encoding.is_none()
        && !raw
        && with_app(|a| a.focus_location(&location)) == Some(true)
    {
        return Ok(());
    }
    let session = session(&uri.target(), &show_status)?;
    let path = uri.path.clone();
    let p = path.clone();
    let s = session.clone();
    let info = wait(&show_status, move |_| s.stat(&p)).map_err(|e| describe(uri, &e))?;
    if info.is_dir() {
        return Err(format!("{uri}\n\nフォルダは開けません。"));
    }
    if info.len() > LARGE_FILE {
        let text = format!(
            "{uri} は {} あります。\n\n今のところ、リモートのファイルは全体を取り寄せてから開きます。\
             続けますか？",
            human_size(info.len())
        );
        if message_box(owner, &text, MB_OKCANCEL | MB_ICONQUESTION) != IDOK {
            return Ok(());
        }
    }
    let cache = yy_io::temp_path("remote");
    let c = cache.clone();
    let total = info.len();
    let r = wait(&show_status, move |work| -> std::io::Result<FileInfo> {
        let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&c)?);
        let info = session.download(&path, &mut out, &mut |done, _| {
            work.report(format!(
                "取り寄せています… {} / {}（Esc で中止）",
                group_digits(done),
                group_digits(total)
            ));
            !work.cancelled()
        })?;
        out.flush()?;
        Ok(info)
    });
    let info = match r {
        Ok(i) => i,
        Err(e) => {
            let _ = std::fs::remove_file(&cache);
            show_status("");
            if e.kind() == std::io::ErrorKind::Interrupted {
                show_status("開くのを中止しました");
                return Ok(());
            }
            return Err(describe(uri, &e));
        }
    };
    let name = yy_proto::display_path(yy_proto::file_name(&uri.path));
    let remote = RemoteFile {
        uri: uri.to_string(),
        name,
        id: Some(info.id),
    };
    let r = with_app(|a| {
        let opts = a.open_options(&location, encoding, raw);
        let doc = Document::open_remote(&cache, &opts, remote).map_err(|e| describe(uri, &e))?;
        match how {
            OpenAs::NewTab => a.add_document(doc),
            OpenAs::Replace => a.set_document(doc),
        }
        set_last(uri.clone());
        a.show_status_message("");
        Ok::<_, String>(())
    });
    match r {
        Some(r) => r?,
        None => {
            let _ = std::fs::remove_file(&cache);
            return Err("処理中のため開けません。".into());
        }
    }
    crate::recentdlg::remember(&location);
    Ok(())
}

fn describe(uri: &RemoteUri, e: &std::io::Error) -> String {
    let what = match e.kind() {
        std::io::ErrorKind::NotFound => "ファイルが見つかりません。".to_owned(),
        std::io::ErrorKind::PermissionDenied => format!("アクセスが拒否されました。\n{e}"),
        _ => e.to_string(),
    };
    format!("{uri}\n\n{what}")
}

// ---- 保存 ---------------------------------------------------------------------------

/// リモートのファイルへの保存先。
#[derive(Clone)]
pub(crate) struct RemoteDest {
    pub(crate) uri: RemoteUri,
    pub(crate) session: Arc<Session>,
    /// 開いた（前回保存した）ときのファイル。これと違えば外部で変更されている
    pub(crate) expected: Option<FileId>,
    /// 外部で変更されていても置き換える
    pub(crate) force: bool,
}

impl RemoteDest {
    pub(crate) fn file(&self) -> RemoteFile {
        RemoteFile {
            uri: self.uri.to_string(),
            name: yy_proto::display_path(yy_proto::file_name(&self.uri.path)),
            id: None,
        }
    }

    /// 保存の内容を送り出す処理（バックグラウンドの保存ジョブから呼ばれる）。
    pub(crate) fn upload(&self) -> Upload {
        let session = self.session.clone();
        let path = self.uri.path.clone();
        let expected = self.expected;
        let force = self.force;
        Box::new(move |local, progress| {
            let mut f = std::fs::File::open(local)?;
            match session.upload(&mut f, &path, expected, progress)? {
                UploadOutcome::Saved(info) => Ok(info.id),
                UploadOutcome::Conflict { pending, .. } if force => {
                    Ok(pending.force().map_err(SaveError::Io)?.id)
                }
                UploadOutcome::Conflict { current, .. } => {
                    let msg = match (current, expected) {
                        (Some(_), None) => "保存先に同じ名前のファイルがあります。",
                        _ => {
                            "保存先のファイルが、開いたあとで（ほかの人やプログラムによって）変更されています。"
                        }
                    };
                    Err(SaveError::Conflict(msg.into()))
                }
            }
        })
    }
}

/// 作業中の文書の保存先（上書き保存）。
pub(crate) fn current_dest(file: &RemoteFile) -> Result<RemoteDest, String> {
    let uri = RemoteUri::parse(&file.uri)
        .ok_or_else(|| format!("場所が正しくありません: {}", file.uri))?;
    let session = session(&uri.target(), &show_status)?;
    Ok(RemoteDest {
        uri,
        session,
        expected: file.id,
        force: false,
    })
}

/// リモートのフォルダの中身（ワークスペースのサイドバー用）。項目のパスは `ssh://…`。
/// フォルダを先に名前順、[`workspace::EXCLUDED`] を除き、[`workspace::MAX_ENTRIES`] を超えた数も返す。
pub(crate) fn list_dir(dir: &RemoteUri) -> Result<(Vec<workspace::Entry>, usize), String> {
    let fs = fs(&dir.target(), &show_status)?;
    let path = dir.path.clone();
    let listed = wait(&show_status, move |_| fs.read_dir(&path));
    show_status("");
    let items = listed.map_err(|e| describe(dir, &e))?;
    let mut entries = Vec::new();
    let mut skipped = 0;
    for e in items {
        let name = yy_proto::display_path(&e.name);
        if workspace::EXCLUDED.contains(&name.as_str()) {
            continue;
        }
        if entries.len() >= workspace::MAX_ENTRIES {
            skipped += 1;
            continue;
        }
        let child = RemoteUri {
            path: yy_proto::join_path(&dir.path, &e.name),
            ..dir.clone()
        };
        entries.push(workspace::Entry {
            name,
            path: PathBuf::from(child.to_string()),
            is_dir: e.info.as_ref().is_some_and(|i| i.is_dir()),
        });
    }
    workspace::sort_entries(&mut entries);
    Ok((entries, skipped))
}

/// 接続先 `uri` でファイル操作をする（接続していなければ接続する。待つ間は進みを表示する）。
pub(crate) fn file_op<T: Send + 'static>(
    uri: &RemoteUri,
    label: &str,
    f: impl FnOnce(&Arc<Session>) -> std::io::Result<T> + Send + 'static,
) -> Result<T, String> {
    let session = session(&uri.target(), &show_status)?;
    show_status(label);
    let r = wait(&show_status, move |_| f(&session));
    show_status("");
    r.map_err(|e| e.to_string())
}

/// `ssh://` の場所を開く（履歴・ブックマーク・コマンドラインから）。
pub(crate) fn open_location(
    owner: HWND,
    path: &Path,
    encoding: Option<Encoding>,
) -> Result<(), String> {
    let uri = path
        .to_str()
        .and_then(RemoteUri::parse)
        .ok_or_else(|| format!("場所が正しくありません: {}", path.display()))?;
    open(owner, &uri, encoding, false, OpenAs::NewTab)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_ssh_config_hosts() {
        let text = "Host build web\n  HostName x\nHost *.example.com !bad\nhost \"quoted\"\n";
        assert_eq!(ssh_config_hosts(text), ["build", "web", "quoted"]);
    }
}
