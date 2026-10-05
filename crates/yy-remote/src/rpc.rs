//! エージェントとの要求・応答のやり取り（11 章 8）。
//!
//! 要求には ID を付けて送り、応答は読み出し用のスレッドが ID ごとの待ち手に渡す。
//! 応答を待たずに次の要求を送れる（[`Client::send`]）ので、読み出しや書き込みを並べて
//! 回線の遅延を隠せる。

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, bounded};
use yy_proto::{Request, Response};

use crate::Exit;

pub struct Client {
    shared: Arc<Shared>,
}

struct Shared {
    writer: Mutex<Option<BufWriter<Box<dyn Write + Send>>>>,
    state: Mutex<State>,
    next: AtomicU32,
}

#[derive(Default)]
struct State {
    waiting: HashMap<u32, Sender<Response>>,
    /// 接続が切れた理由
    closed: Option<String>,
}

/// 送った要求への応答を待つもの。
pub struct Reply {
    rx: Receiver<Response>,
    shared: Arc<Shared>,
}

impl Reply {
    /// 応答を待つ。接続が切れたらエラー。
    pub fn wait(self) -> io::Result<Response> {
        match self.rx.recv() {
            Ok(r) => Ok(r),
            Err(_) => Err(self.shared.closed_error()),
        }
    }
}

impl Shared {
    fn closed_error(&self) -> io::Error {
        let reason = self.state.lock().unwrap().closed.clone();
        io::Error::new(
            io::ErrorKind::ConnectionAborted,
            reason.unwrap_or_else(|| "接続が切れました".to_owned()),
        )
    }

    fn close(&self, reason: String) {
        let mut st = self.state.lock().unwrap();
        st.closed.get_or_insert(reason);
        // 待ち手を起こす（受け取り口が消えるので Reply::wait はエラーになる）
        st.waiting.clear();
    }
}

impl Client {
    /// `reader` と `writer` でエージェントと話す。`finish` は接続が切れたときに呼び、
    /// エージェントの標準エラー出力をエラーの説明に加える。
    pub fn new(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        finish: Box<dyn FnOnce() -> io::Result<Exit> + Send>,
    ) -> Client {
        let shared = Arc::new(Shared {
            writer: Mutex::new(Some(BufWriter::with_capacity(1 << 20, writer))),
            state: Mutex::new(State::default()),
            next: AtomicU32::new(1),
        });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("yy-remote-rpc".into())
            .spawn(move || {
                let mut reader = BufReader::with_capacity(1 << 20, reader);
                let reason = loop {
                    match yy_proto::read_frame::<_, Response>(&mut reader) {
                        Ok(Some((id, resp))) => {
                            let tx = s.state.lock().unwrap().waiting.remove(&id);
                            if let Some(tx) = tx {
                                let _ = tx.send(resp);
                            }
                        }
                        Ok(None) => break "エージェントとの接続が切れました".to_owned(),
                        Err(e) => break format!("エージェントとの通信に失敗しました: {e}"),
                    }
                };
                // 待っている要求をすぐに失敗させてから、終わった理由を集める
                s.close(reason.clone());
                drop(reader);
                let detail = finish()
                    .ok()
                    .map(|x| String::from_utf8_lossy(x.stderr.trim_ascii()).into_owned());
                if let Some(d) = detail.filter(|d| !d.is_empty()) {
                    s.state.lock().unwrap().closed = Some(format!("{reason}: {d}"));
                }
            })
            .expect("spawn rpc reader");
        Client { shared }
    }

    /// 要求を送る（応答は待たない）。
    pub fn send(&self, req: &Request) -> io::Result<Reply> {
        let id = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = bounded(1);
        {
            let mut st = self.shared.state.lock().unwrap();
            if st.closed.is_some() {
                drop(st);
                return Err(self.shared.closed_error());
            }
            st.waiting.insert(id, tx);
        }
        let mut w = self.shared.writer.lock().unwrap();
        let written = match w.as_mut() {
            Some(w) => yy_proto::write_frame(w, id, req).and_then(|()| w.flush()),
            None => Err(io::Error::new(io::ErrorKind::NotConnected, "閉じています")),
        };
        if let Err(e) = written {
            self.shared.state.lock().unwrap().waiting.remove(&id);
            self.shared
                .close(format!("エージェントへの送信に失敗しました: {e}"));
            return Err(self.shared.closed_error());
        }
        Ok(Reply {
            rx,
            shared: self.shared.clone(),
        })
    }

    /// 要求を送って応答を待つ。
    pub fn call(&self, req: &Request) -> io::Result<Response> {
        self.send(req)?.wait()
    }

    /// 接続が切れたか。
    pub fn is_closed(&self) -> bool {
        self.shared.state.lock().unwrap().closed.is_some()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // 標準入力を閉じてエージェントを終わらせる
        self.shared.writer.lock().unwrap().take();
    }
}
