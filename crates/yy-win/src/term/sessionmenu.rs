//! セッション > ホスト > セッション。ホストのサブメニューを開いた時点で一覧を更新する。

use super::*;
use yy_config::TerminalMultiplexer;

const BASE: u16 = 0x5000;
const LIMIT: usize = 0x2000;

#[derive(Clone)]
struct Host {
    menu: HMENU,
    target: Target,
    kind: Option<Multiplexer>,
    prefix: String,
}

#[derive(Clone)]
struct Choice {
    target: Target,
    binding: Option<Binding>,
}

#[derive(Default)]
struct State {
    root: HMENU,
    hosts: Vec<Host>,
    choices: Vec<Option<Choice>>,
}

thread_local! {
    static MENU: RefCell<State> = RefCell::new(State::default());
}

pub(super) fn init(root: HMENU) {
    MENU.with(|m| {
        *m.borrow_mut() = State {
            root,
            ..Default::default()
        }
    });
}

fn clear(menu: HMENU) {
    unsafe {
        while GetMenuItemCount(Some(menu)) > 0 {
            let id = GetMenuItemID(menu, 0);
            if let Some(index) = id
                .checked_sub(BASE as u32)
                .filter(|i| (*i as usize) < LIMIT)
            {
                MENU.with(|m| {
                    if let Some(choice) = m.borrow_mut().choices.get_mut(index as usize) {
                        *choice = None;
                    }
                });
            }
            let _ = DeleteMenu(menu, 0, MF_BYPOSITION);
        }
    }
}

fn append(menu: HMENU, text: &str, choice: Option<Choice>) {
    let id = choice.and_then(|choice| {
        MENU.with(|m| {
            let mut m = m.borrow_mut();
            if let Some(index) = m.choices.iter().position(Option::is_none) {
                m.choices[index] = Some(choice);
                return Some(BASE + index as u16);
            }
            if m.choices.len() >= LIMIT {
                return None;
            }
            let id = BASE + m.choices.len() as u16;
            m.choices.push(Some(choice));
            Some(id)
        })
    });
    // & はアクセスキーとして扱わず、名前そのものを表示する。
    let text = windows::core::HSTRING::from(text.replace('&', "&&"));
    unsafe {
        let flags = if id.is_some() {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        let _ = AppendMenuW(menu, flags, id.unwrap_or(0) as usize, &text);
    }
}

pub(super) fn populate(_hwnd: HWND, menu: HMENU) {
    let is_root = MENU.with(|m| m.borrow().root == menu);
    if is_root {
        fill_hosts(menu);
        return;
    }
    let host = MENU.with(|m| m.borrow().hosts.iter().find(|h| h.menu == menu).cloned());
    let Some(host) = host else {
        return;
    };
    clear(menu);
    let Some(kind) = host.kind else {
        append(
            menu,
            "ホスト設定で tmux または screen を選択してください",
            None,
        );
        return;
    };
    if let Err(e) = yy_remote::multiplex::validate_prefix(&host.prefix) {
        append(menu, &e, None);
        return;
    }
    append(
        menu,
        "新規セッションを作成...",
        Some(Choice {
            target: host.target.clone(),
            binding: None,
        }),
    );
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    }
    set_status(&format!(
        "{} のセッション一覧を取得しています…",
        host.target
    ));
    let result = crate::remote::transport(&host.target, &set_status).and_then(|t| {
        crate::remote::wait(&set_status, move |_| {
            yy_remote::multiplex::list_sessions(t.as_ref(), kind)
        })
        .map_err(|e| e.to_string())
    });
    set_status("");
    match result {
        Ok(sessions) if sessions.is_empty() => append(menu, "セッションはありません", None),
        Ok(sessions) => {
            for session in sessions.into_iter().take(1000) {
                let binding = Binding::from_session(kind, &host.prefix, &session.name).ok();
                let state = if session.attached {
                    "接続中"
                } else {
                    "デタッチ中"
                };
                let suffix = if binding.is_none() {
                    "・登録プレフィクス／タブ名の対象外"
                } else {
                    ""
                };
                append(
                    menu,
                    &format!("{}（{state}{suffix}）", session.name),
                    binding.map(|binding| Choice {
                        target: host.target.clone(),
                        binding: Some(binding),
                    }),
                );
            }
        }
        Err(e) => {
            append(
                menu,
                "一覧を取得できませんでした（ホストを開き直すと再試行）",
                None,
            );
            set_status(&e);
        }
    }
}

fn fill_hosts(menu: HMENU) {
    clear(menu);
    MENU.with(|m| {
        let mut m = m.borrow_mut();
        m.hosts.clear();
        m.choices.clear();
    });
    let mut targets = crate::remote::with_state(|r| r.known_targets()).unwrap_or_default();
    let config = with(|a| {
        for tab in &a.tabs {
            if let Place::Remote { target, .. } = &tab.place {
                targets.push(target.to_string());
            }
        }
        a.config.remote.clone()
    })
    .unwrap_or_default();
    targets.extend(config.host.keys().cloned());
    targets.sort_by_key(|t| t.to_lowercase());
    targets.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    for name in targets.into_iter().take(256) {
        let Some(target) = Target::parse(&name) else {
            continue;
        };
        let configured = config
            .host
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&target.host))
            .map(|(_, h)| h);
        let kind = configured.and_then(|h| match h.terminal_multiplexer {
            TerminalMultiplexer::None => None,
            TerminalMultiplexer::Tmux => Some(Multiplexer::Tmux),
            TerminalMultiplexer::Screen => Some(Multiplexer::Screen),
        });
        let prefix = configured
            .map(|h| h.terminal_session_prefix.clone())
            .unwrap_or_default();
        let Ok(submenu) = (unsafe { CreatePopupMenu() }) else {
            continue;
        };
        let text = windows::core::HSTRING::from(name.replace('&', "&&"));
        unsafe {
            let _ = AppendMenuW(menu, MF_POPUP, submenu.0 as usize, &text);
        }
        append(
            submenu,
            "ホストにカーソルを合わせると一覧を取得します",
            None,
        );
        MENU.with(|m| {
            m.borrow_mut().hosts.push(Host {
                menu: submenu,
                target,
                kind,
                prefix,
            })
        });
    }
    if MENU.with(|m| m.borrow().hosts.is_empty()) {
        append(
            menu,
            "接続先ホストがありません。設定または SSH 接続を登録してください",
            None,
        );
    }
}

pub(super) fn command(id: u16) -> bool {
    let Some(index) = id.checked_sub(BASE).filter(|i| (*i as usize) < LIMIT) else {
        return false;
    };
    let choice = MENU.with(|m| m.borrow().choices.get(index as usize).cloned().flatten());
    if let Some(choice) = choice {
        if let Some(binding) = &choice.binding {
            let active = with(|a| a.tabs.iter().position(|t| {
                t.exited.is_none()
                    && matches!(&t.place, Place::Remote { target, .. } if target.same(&choice.target))
                    && t.binding.as_ref() == Some(binding)
            })).flatten();
            if let Some(index) = active {
                with(|a| a.activate(index));
                return true;
            }
        }
        let mode = if choice.binding.is_some() {
            OpenMode::Attach
        } else {
            OpenMode::Create
        };
        open_tab_request(
            Place::Remote {
                target: choice.target,
                dir: None,
            },
            &[],
            choice.binding,
            mode,
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Mutex, atomic::AtomicUsize};

    #[derive(Default)]
    struct Server {
        shells: Mutex<Vec<Vec<u8>>>,
        closes: Arc<AtomicUsize>,
    }

    impl yy_remote::Transport for Server {
        fn is_closed(&self) -> bool {
            false
        }
        fn exec(&self, command: &[u8]) -> io::Result<yy_remote::Process> {
            let command = String::from_utf8_lossy(command);
            let stdout = if command.contains("list-sessions") {
                b"p-build:0\n".to_vec()
            } else {
                Vec::new()
            };
            Ok(yy_remote::Process::new(
                Box::new(io::sink()),
                Box::new(io::Cursor::new(stdout)),
                Box::new(|| {
                    Ok(yy_remote::Exit {
                        status: Some(0),
                        stderr: Vec::new(),
                    })
                }),
            ))
        }
        fn shell(
            &self,
            _: &str,
            _: (u16, u16),
            command: Option<&[u8]>,
        ) -> io::Result<yy_remote::Shell> {
            self.shells.lock().unwrap().push(command.unwrap().to_vec());
            let closes = self.closes.clone();
            Ok(yy_remote::Shell {
                input: Box::new(io::sink()),
                output: Box::new(io::empty()),
                resize: Box::new(|_, _| {}),
                finish: Box::new(|| Ok(yy_remote::Exit::default())),
                close: Box::new(move || {
                    closes.fetch_add(1, Ordering::SeqCst);
                }),
            })
        }
    }

    struct Connector(Arc<Server>);
    impl yy_remote::Connector for Connector {
        fn connect(
            &self,
            _: &yy_remote::HostSpec,
            _: &dyn yy_remote::Prompter,
            _: &yy_remote::ConnectLog,
        ) -> io::Result<Arc<dyn yy_remote::Transport>> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn session_menu_selection_opens_bound_tab_and_reselects_it() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        QUIET.with(|q| q.set(true));
        let server = Arc::new(Server::default());
        let clone = server.clone();
        let factory: yy_remote::ConnectorFactory =
            Arc::new(move |_| Arc::new(Connector(clone.clone())));
        let frame = create(
            Place::Local(None),
            None,
            Some(vec![
                "cmd.exe".into(),
                "/c".into(),
                "echo session-menu-test".into(),
            ]),
        )
        .unwrap();
        let mut config = yy_config::RemoteConfig {
            read_ssh_config: false,
            ..Default::default()
        };
        config.host.insert(
            "menu-test".into(),
            yy_config::RemoteHost {
                terminal_multiplexer: TerminalMultiplexer::Tmux,
                terminal_session_prefix: "p-".into(),
                ..Default::default()
            },
        );
        with(|a| a.config.remote = config.clone());
        crate::remote::with_state(|r| *r = crate::remote::RemoteState::new(Some(factory), config));
        let root = MENU.with(|m| m.borrow().root);
        populate(frame, root);
        let host = MENU.with(|m| {
            m.borrow()
                .hosts
                .iter()
                .find(|h| h.target.host == "menu-test")
                .unwrap()
                .menu
        });
        populate(frame, host);
        let count = MENU.with(|m| m.borrow().choices.len());
        populate(frame, host);
        assert_eq!(MENU.with(|m| m.borrow().choices.len()), count);
        let id = MENU.with(|m| {
            BASE + m
                .borrow()
                .choices
                .iter()
                .position(|c| {
                    c.as_ref().is_some_and(|c| {
                        c.binding
                            .as_ref()
                            .is_some_and(|b| b.session_name() == "p-build")
                    })
                })
                .unwrap() as u16
        });
        // 実際の WM_COMMAND 経路から、名前入力なしで既存セッションのタブを開く。
        super::super::command(frame, id);
        assert_eq!(with(|a| a.tabs.len()), Some(2));
        assert_eq!(with(|a| a.tab().unwrap().title()), Some("build".into()));
        assert!(
            String::from_utf8_lossy(&server.shells.lock().unwrap()[0])
                .contains("attach-session -t")
        );
        with(|a| a.activate(0));
        super::super::command(frame, id);
        assert_eq!(with(|a| a.active), Some(1));
        assert_eq!(server.shells.lock().unwrap().len(), 1);
        with(|a| a.close_tab(1));
        assert_eq!(server.closes.load(Ordering::SeqCst), 1);
        unsafe {
            let _ = DestroyWindow(frame);
        }
        shutdown();
    }

    #[test]
    fn folder_launch_paths_require_new_sessions() {
        // yyeditor / yysftp が渡す SSH URI と、yyterm のワークスペースで使う場所。
        let uri = "ssh://user@build/work/project";
        assert_eq!(
            folder_open_mode(&initial_place(Some(uri))),
            OpenMode::Create
        );
        let uri = RemoteUri::parse(uri).unwrap();
        let workspace = Place::Remote {
            target: uri.target(),
            dir: Some(uri.path),
        };
        assert_eq!(folder_open_mode(&workspace), OpenMode::Create);
        assert_eq!(
            folder_open_mode(&initial_place(Some("user@build"))),
            OpenMode::CreateOrAttach
        );
    }
}
