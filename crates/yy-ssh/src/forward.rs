//! ポートフォワーディングの実装（12 章 4.3）。
//!
//! - `-L`: 手元で待ち受け、来た接続ごとに `direct-tcpip` チャネルを開いて中継する。
//! - `-R`: 接続先に `tcpip-forward` を求め、接続先から開かれた `forwarded-tcpip` チャネルを
//!   ポートで振り分けて（[`Routes`]）、手元の転送先に接続して中継する。
//! - `-D`: 手元で待ち受け、SOCKS（4・4a・5 の CONNECT）の要求を読んでから `direct-tcpip` で中継する。

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};

use russh::client::{self, Handle, Msg};
use russh::{Channel, ChannelOpenFailure};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use yy_remote::{ActiveForward, Forward, ForwardNote};

use crate::Client;

/// `-R` の振り分け表（接続先で待ち受けているポート → 手元の転送先）。
pub(crate) type Routes = Arc<Mutex<Vec<RemoteRoute>>>;

#[derive(Clone)]
pub(crate) struct RemoteRoute {
    port: u32,
    host: String,
    host_port: u16,
    note: ForwardNote,
}

/// 接続先から `forwarded-tcpip` チャネルが開かれた（[`client::Handler`] から呼ぶ）。
pub(crate) fn on_forwarded(
    routes: &Routes,
    channel: Channel<Msg>,
    connected_port: u32,
    originator: String,
    reply: client::ChannelOpenHandle,
) {
    let route = routes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|r| r.port == connected_port)
        .cloned();
    let Some(r) = route else {
        // 求めていないポート
        tokio::spawn(reply.reject(ChannelOpenFailure::AdministrativelyProhibited));
        return;
    };
    tokio::spawn(async move {
        match TcpStream::connect((r.host.as_str(), r.host_port)).await {
            Ok(tcp) => {
                let _ = tcp.set_nodelay(true);
                reply.accept().await;
                pump(channel, tcp).await;
            }
            Err(e) => {
                (r.note)(&format!(
                    "-R {}: {originator} からの接続を {}:{} に転送できません: {e}",
                    r.port, r.host, r.host_port
                ));
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
            }
        }
    });
}

/// チャネルと TCP の接続の間で中継する（どちらかが閉じるまで）。
async fn pump(channel: Channel<Msg>, mut tcp: TcpStream) {
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
    let _ = stream.shutdown().await;
}

fn describe_target(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// フォワーディングを始める。
pub(crate) fn start(
    rt: &Runtime,
    handle: &Arc<Handle<Client>>,
    routes: &Routes,
    f: &Forward,
    note: ForwardNote,
) -> io::Result<ActiveForward> {
    match f {
        Forward::Local {
            port,
            host,
            host_port,
            ..
        } => {
            let bind = f.local_bind();
            let listener = rt
                .block_on(TcpListener::bind((bind.as_str(), *port)))
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "手元の {} で待ち受けられません: {e}",
                            describe_target(&bind, *port)
                        ),
                    )
                })?;
            let local_port = listener.local_addr().map_or(*port, |a| a.port());
            let local = listener
                .local_addr()
                .map_or_else(|_| describe_target(&bind, *port), |a| a.to_string());
            let target = (host.clone(), *host_port);
            let h = handle.clone();
            let task = rt.spawn(accept_loop(
                listener,
                h,
                note,
                Mode::Fixed(target.0, target.1),
            ));
            Ok(ActiveForward::new(
                format!(
                    "{f}（手元の {local} → 接続先から {}）",
                    describe_target(host, *host_port)
                ),
                local_port,
                Box::new(move || task.abort()),
            ))
        }
        Forward::Dynamic { port, .. } => {
            let bind = f.local_bind();
            let listener = rt
                .block_on(TcpListener::bind((bind.as_str(), *port)))
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "手元の {} で待ち受けられません: {e}",
                            describe_target(&bind, *port)
                        ),
                    )
                })?;
            let local_port = listener.local_addr().map_or(*port, |a| a.port());
            let local = listener
                .local_addr()
                .map_or_else(|_| describe_target(&bind, *port), |a| a.to_string());
            let h = handle.clone();
            let task = rt.spawn(accept_loop(listener, h, note, Mode::Socks));
            Ok(ActiveForward::new(
                format!("{f}（手元の {local} を SOCKS のプロキシに）"),
                local_port,
                Box::new(move || task.abort()),
            ))
        }
        Forward::Remote {
            port,
            host,
            host_port,
            ..
        } => {
            let bind = f.remote_bind();
            let h = handle.clone();
            let b = bind.clone();
            let requested = u32::from(*port);
            let got = rt
                .block_on(async move { h.tcpip_forward(b, requested).await })
                .map_err(|e| {
                    let why = match e {
                        russh::Error::RequestDenied => {
                            "接続先が断りました（sshd の AllowTcpForwarding、ポートの使用中・権限など）"
                                .to_owned()
                        }
                        e => e.to_string(),
                    };
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("接続先のポート {port} で待ち受けられません: {why}"),
                    )
                })?;
            // 0 を求めたときは接続先が選んだポート
            let actual = if requested == 0 { got } else { requested };
            routes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(RemoteRoute {
                    port: actual,
                    host: host.clone(),
                    host_port: *host_port,
                    note,
                });
            let shown_bind = if bind.is_empty() { "*" } else { bind.as_str() };
            let h = handle.clone();
            let routes = routes.clone();
            let rt_handle = rt.handle().clone();
            Ok(ActiveForward::new(
                format!(
                    "{f}（接続先の {} → 手元から {}）",
                    describe_target(shown_bind, actual as u16),
                    describe_target(host, *host_port)
                ),
                actual as u16,
                Box::new(move || {
                    routes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .retain(|r| r.port != actual);
                    rt_handle.spawn(async move {
                        let _ = h.cancel_tcpip_forward(bind, actual).await;
                    });
                }),
            ))
        }
    }
}

/// 転送先（ホスト, ポート, SOCKS の版）。
type Dest = (String, u16, Option<u8>);

/// 転送先の決め方。
#[derive(Clone)]
enum Mode {
    /// `-L`: 決まった転送先
    Fixed(String, u16),
    /// `-D`: SOCKS の要求を読む
    Socks,
}

/// 手元で待ち受けて、来た接続ごとに `target`（転送先を決める。SOCKS ではここで要求を読む）を
/// 行ってから `direct-tcpip` で中継する。SSH の接続が切れたら終わる。
async fn accept_loop(
    listener: TcpListener,
    handle: Arc<Handle<Client>>,
    note: ForwardNote,
    mode: Mode,
) {
    let local = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();
    loop {
        let (mut sock, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                note(&format!("{local}: 接続を受け付けられません: {e}"));
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                continue;
            }
        };
        if handle.is_closed() {
            break;
        }
        let _ = sock.set_nodelay(true);
        let h = handle.clone();
        let note = note.clone();
        let mode = mode.clone();
        let local = local.clone();
        tokio::spawn(async move {
            let dest = match mode {
                Mode::Fixed(h, p) => Ok((h, p, None)),
                Mode::Socks => socks_handshake(&mut sock).await,
            };
            let (host, port, socks) = match dest {
                Ok(t) => t,
                Err(e) => {
                    note(&format!("{local}: {peer} からの要求を読めません: {e}"));
                    return;
                }
            };
            match h
                .channel_open_direct_tcpip(
                    host.clone(),
                    u32::from(port),
                    peer.ip().to_string(),
                    u32::from(peer.port()),
                )
                .await
            {
                Ok(ch) => {
                    socks_reply(&mut sock, socks, true).await;
                    pump(ch, sock).await;
                }
                Err(e) => {
                    socks_reply(&mut sock, socks, false).await;
                    note(&format!(
                        "{local}: 接続先から {} に接続できません: {e}",
                        describe_target(&host, port)
                    ));
                }
            }
        });
    }
}

/// SOCKS の応答を送る（SOCKS の接続でなければ何もしない）。
async fn socks_reply(sock: &mut TcpStream, version: Option<u8>, ok: bool) {
    let reply: &[u8] = match (version, ok) {
        (Some(4), true) => &[0, 0x5a, 0, 0, 0, 0, 0, 0],
        (Some(4), false) => &[0, 0x5b, 0, 0, 0, 0, 0, 0],
        (Some(5), true) => &[5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
        // 5: 接続を拒否された
        (Some(5), false) => &[5, 5, 0, 1, 0, 0, 0, 0, 0, 0],
        _ => return,
    };
    let _ = sock.write_all(reply).await;
}

/// SOCKS（4・4a・5）の CONNECT の要求を読む。
async fn socks_handshake(sock: &mut TcpStream) -> io::Result<Dest> {
    let ver = sock.read_u8().await?;
    match ver {
        4 => {
            let cmd = sock.read_u8().await?;
            let port = sock.read_u16().await?;
            let mut ip = [0u8; 4];
            sock.read_exact(&mut ip).await?;
            read_cstr(sock).await?; // ユーザー ID
            if cmd != 1 {
                let _ = sock.write_all(&[0, 0x5b, 0, 0, 0, 0, 0, 0]).await;
                return Err(io::Error::other("SOCKS4: CONNECT 以外の要求です"));
            }
            // SOCKS4a: 0.0.0.x ならホスト名が続く
            let host = if ip[..3] == [0, 0, 0] && ip[3] != 0 {
                read_cstr(sock).await?
            } else {
                Ipv4Addr::from(ip).to_string()
            };
            Ok((host, port, Some(4)))
        }
        5 => {
            let n = sock.read_u8().await?;
            let mut methods = vec![0u8; n as usize];
            sock.read_exact(&mut methods).await?;
            if !methods.contains(&0) {
                sock.write_all(&[5, 0xff]).await?;
                return Err(io::Error::other(
                    "SOCKS5: 認証なしの方式に対応していないクライアントです",
                ));
            }
            sock.write_all(&[5, 0]).await?;
            let mut head = [0u8; 4];
            sock.read_exact(&mut head).await?;
            let [v, cmd, _, atyp] = head;
            if v != 5 {
                return Err(io::Error::other("SOCKS5: 要求の版が違います"));
            }
            let host = match atyp {
                1 => {
                    let mut ip = [0u8; 4];
                    sock.read_exact(&mut ip).await?;
                    Ipv4Addr::from(ip).to_string()
                }
                3 => {
                    let len = sock.read_u8().await?;
                    let mut name = vec![0u8; len as usize];
                    sock.read_exact(&mut name).await?;
                    String::from_utf8_lossy(&name).into_owned()
                }
                4 => {
                    let mut ip = [0u8; 16];
                    sock.read_exact(&mut ip).await?;
                    Ipv6Addr::from(ip).to_string()
                }
                _ => {
                    let _ = sock.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                    return Err(io::Error::other("SOCKS5: アドレスの種類が不正です"));
                }
            };
            let port = sock.read_u16().await?;
            if cmd != 1 {
                let _ = sock.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                return Err(io::Error::other("SOCKS5: CONNECT 以外の要求です"));
            }
            Ok((host, port, Some(5)))
        }
        v => Err(io::Error::other(format!(
            "SOCKS の版 {v} には対応していません"
        ))),
    }
}

async fn read_cstr(sock: &mut TcpStream) -> io::Result<String> {
    let mut out = Vec::new();
    loop {
        let b = sock.read_u8().await?;
        if b == 0 {
            break;
        }
        if out.len() > 255 {
            return Err(io::Error::other("SOCKS4: 文字列が長すぎます"));
        }
        out.push(b);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}
