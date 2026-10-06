//! HTTP（CONNECT）・SOCKS5・SOCKS4a のプロキシを通した TCP 接続（11 章 4.5）。

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use yy_remote::proxy::{Proxy, ProxyKind};

/// プロキシの認証に使うユーザー名とパスワード。
#[derive(Clone, Copy, Debug)]
pub struct Credentials<'a> {
    pub user: &'a str,
    pub password: &'a str,
}

/// プロキシの応答の見出しの上限
const HEADER_LIMIT: usize = 16 << 10;

/// `proxy` に接続し、`host:port` への中継を頼む。
pub async fn connect(
    proxy: &Proxy,
    creds: Option<Credentials<'_>>,
    host: &str,
    port: u16,
) -> io::Result<TcpStream> {
    let mut s = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("プロキシ {proxy} に接続できませんでした: {e}"),
            )
        })?;
    s.set_nodelay(true)?;
    handshake(&mut s, proxy, creds, host, port).await?;
    Ok(s)
}

/// 接続済みの `s` でプロキシに中継を頼む。
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    proxy: &Proxy,
    creds: Option<Credentials<'_>>,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let r = match proxy.kind {
        ProxyKind::Http => http(s, creds, host, port).await,
        ProxyKind::Socks5 => socks5(s, creds, host, port).await,
        ProxyKind::Socks4 => socks4(s, proxy, host, port).await,
    };
    r.map_err(|e| {
        let kind = match e.kind() {
            io::ErrorKind::UnexpectedEof => io::ErrorKind::ConnectionAborted,
            k => k,
        };
        io::Error::new(
            kind,
            format!("プロキシ {proxy} 経由で {host}:{port} に接続できませんでした: {e}"),
        )
    })
}

fn refused(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, msg.into())
}

/// 認証の情報がなく、プロキシに求められた（呼び出し側は尋ねてやり直す）。
fn auth_required() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "プロキシの認証が必要です")
}

/// 認証が必要か、認証に失敗したか（どちらも尋ね直してやり直せる）。
pub fn needs_credentials(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
}

fn auth_failed() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "プロキシの認証に失敗しました",
    )
}

async fn http<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    creds: Option<Credentials<'_>>,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(c) = creds {
        let cred = format!("{}:{}", c.user, c.password);
        req.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(cred.as_bytes())
        ));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    s.flush().await?;
    // 中継が始まった後のデータを読みすぎないよう、1 バイトずつ見出しの終わりまで読む
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= HEADER_LIMIT {
            return Err(io::Error::other("プロキシの応答が長すぎます"));
        }
        head.push(s.read_u8().await?);
    }
    let head = String::from_utf8_lossy(&head);
    let status_line = head.lines().next().unwrap_or("");
    let mut words = status_line.split_whitespace();
    let code: u16 = match (words.next(), words.next()) {
        (Some(v), Some(c)) if v.starts_with("HTTP/") => c
            .parse()
            .map_err(|_| io::Error::other("プロキシの応答を読めません"))?,
        _ => return Err(io::Error::other("プロキシの応答を読めません")),
    };
    match code {
        200..=299 => Ok(()),
        407 if creds.is_none() => Err(auth_required()),
        407 => Err(auth_failed()),
        _ => Err(refused(format!("プロキシの応答: {}", status_line.trim()))),
    }
}

/// 接続先のアドレス（SOCKS5 の ATYP と中身）。
fn socks5_address(host: &str) -> io::Result<Vec<u8>> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(match ip {
            std::net::IpAddr::V4(v4) => [&[1u8][..], &v4.octets()].concat(),
            std::net::IpAddr::V6(v6) => [&[4u8][..], &v6.octets()].concat(),
        });
    }
    let len = u8::try_from(host.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ホスト名が長すぎます"))?;
    Ok([&[3u8, len][..], host.as_bytes()].concat())
}

async fn socks5<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    creds: Option<Credentials<'_>>,
    host: &str,
    port: u16,
) -> io::Result<()> {
    // 認証方式: 0 = なし、2 = ユーザー名とパスワード（求められたら尋ねてやり直す）
    s.write_all(&[5, 2, 0, 2]).await?;
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).await?;
    if reply[0] != 5 {
        return Err(io::Error::other("SOCKS5 のプロキシではありません"));
    }
    match reply[1] {
        0 => {}
        2 => {
            let Some(c) = creds else {
                return Err(auth_required());
            };
            let user = c.user.as_bytes();
            let pass = c.password.as_bytes();
            let (Ok(ul), Ok(pl)) = (u8::try_from(user.len()), u8::try_from(pass.len())) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "プロキシのユーザー名かパスワードが長すぎます",
                ));
            };
            let msg = [&[1, ul][..], user, &[pl], pass].concat();
            s.write_all(&msg).await?;
            let mut r = [0u8; 2];
            s.read_exact(&mut r).await?;
            if r[1] != 0 {
                return Err(auth_failed());
            }
        }
        _ if creds.is_none() => return Err(auth_required()),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "プロキシが受け付ける認証方式がありません",
            ));
        }
    }
    let req = [
        &[5u8, 1, 0][..],
        &socks5_address(host)?,
        &port.to_be_bytes(),
    ]
    .concat();
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0 {
        let why = match head[1] {
            2 => "規則で許可されていません",
            3 => "ネットワークに到達できません",
            4 => "ホストに到達できません",
            5 => "接続を拒否されました",
            6 => "時間切れ",
            7 => "対応していない要求です",
            8 => "対応していないアドレスの種類です",
            _ => "失敗しました",
        };
        return Err(refused(format!("SOCKS5: {why}")));
    }
    // 中継に使うアドレス（読み捨てる）
    let skip = match head[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(s.read_u8().await?),
        _ => return Err(io::Error::other("SOCKS5 の応答を読めません")),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await?;
    Ok(())
}

async fn socks4<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    proxy: &Proxy,
    host: &str,
    port: u16,
) -> io::Result<()> {
    let user = proxy.user.as_deref().unwrap_or("").as_bytes();
    let mut req = vec![4u8, 1];
    req.extend_from_slice(&port.to_be_bytes());
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            req.extend_from_slice(&ip.octets());
            req.extend_from_slice(user);
            req.push(0);
        }
        // SOCKS4a: 0.0.0.x を送り、ホスト名はプロキシに解決させる
        Err(_) => {
            req.extend_from_slice(&[0, 0, 0, 1]);
            req.extend_from_slice(user);
            req.push(0);
            req.extend_from_slice(host.as_bytes());
            req.push(0);
        }
    }
    s.write_all(&req).await?;
    let mut r = [0u8; 8];
    s.read_exact(&mut r).await?;
    match r[1] {
        0x5a => Ok(()),
        0x5c | 0x5d => Err(auth_failed()),
        _ => Err(refused("SOCKS4: 接続を拒否されました")),
    }
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn proxy(s: &str) -> Proxy {
        Proxy::parse(s).unwrap().unwrap()
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"alice:secret"), "YWxpY2U6c2VjcmV0");
    }

    /// プロキシ側で `request` を受け取り `answer` を返し、その後のデータが通ることを確かめる。
    fn exchange(
        p: &Proxy,
        creds: Option<Credentials<'_>>,
        host: &str,
        answer: &'static [u8],
    ) -> (io::Result<()>, Vec<u8>) {
        rt().block_on(async {
            let (mut client, mut server) = duplex(4096);
            let srv = tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = [0u8; 512];
                // 要求を受け取ったら応答する（応答の後ろに中継したデータを付ける）
                let n = server.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let mut answer = answer;
                // SOCKS5 は 2 往復以上する
                while let Some(&len) = answer.first() {
                    let len = usize::from(len);
                    server.write_all(&answer[1..1 + len]).await.unwrap();
                    answer = &answer[1 + len..];
                    if answer.is_empty() {
                        break;
                    }
                    let n = server.read(&mut buf).await.unwrap();
                    got.extend_from_slice(&buf[..n]);
                }
                server.write_all(b"SSH-2.0-test").await.unwrap();
                got
            });
            let r = handshake(&mut client, p, creds, host, 22).await;
            if r.is_ok() {
                let mut after = [0u8; 12];
                client.read_exact(&mut after).await.unwrap();
                assert_eq!(&after, b"SSH-2.0-test");
            }
            (r, srv.await.unwrap())
        })
    }

    fn cred(user: &'static str, password: &'static str) -> Option<Credentials<'static>> {
        Some(Credentials { user, password })
    }

    /// `[長さ, 中身…]` を並べる。
    fn frames(parts: &[&[u8]]) -> &'static [u8] {
        let mut v = Vec::new();
        for p in parts {
            v.push(p.len() as u8);
            v.extend_from_slice(p);
        }
        v.leak()
    }

    #[test]
    fn http_connect() {
        let p = proxy("http://proxy:3128");
        let (r, got) = exchange(
            &p,
            cred("alice", "secret"),
            "build.example.com",
            frames(&[b"HTTP/1.1 200 Connection established\r\nVia: x\r\n\r\n"]),
        );
        r.unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "CONNECT build.example.com:22 HTTP/1.1\r\nHost: build.example.com:22\r\n\
             Proxy-Authorization: Basic YWxpY2U6c2VjcmV0\r\n\r\n"
        );
        let (r, _) = exchange(
            &proxy("http://proxy"),
            None,
            "::1",
            frames(&[b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"]),
        );
        let e = r.unwrap_err();
        assert!(
            needs_credentials(&e) && e.to_string().contains("認証が必要"),
            "{e}"
        );
        let (r, _) = exchange(
            &proxy("http://proxy"),
            cred("alice", "wrong"),
            "h",
            frames(&[b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"]),
        );
        let e = r.unwrap_err();
        assert!(
            needs_credentials(&e) && e.to_string().contains("失敗"),
            "{e}"
        );
        let (r, _) = exchange(
            &proxy("http://proxy"),
            None,
            "host",
            frames(&[b"HTTP/1.1 403 Forbidden\r\n\r\n"]),
        );
        let e = r.unwrap_err();
        assert!(e.to_string().contains("403 Forbidden"), "{e}");
    }

    #[test]
    fn socks5_connect() {
        let (r, got) = exchange(
            &proxy("socks5://socks"),
            None,
            "build",
            frames(&[&[5, 0], &[5, 0, 0, 1, 10, 0, 0, 1, 0, 22]]),
        );
        r.unwrap();
        assert_eq!(
            got,
            [&[5, 2, 0, 2, 5, 1, 0, 3, 5][..], b"build", &[0, 22]].concat()
        );

        // ユーザー名とパスワード、IPv6 のアドレス
        let p = proxy("socks5://socks");
        let (r, got) = exchange(
            &p,
            cred("bob", "pw"),
            "[::1]",
            frames(&[&[5, 2], &[1, 0], &[5, 0, 0, 3, 1, b'x', 0, 22]]),
        );
        r.unwrap();
        let mut v6 = vec![5, 1, 0, 4];
        v6.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        v6.extend_from_slice(&[0, 22]);
        assert_eq!(
            got,
            [&[5, 2, 0, 2, 1, 3][..], b"bob", &[2], b"pw", &v6].concat()
        );

        let (r, _) = exchange(&p, cred("bob", "bad"), "h", frames(&[&[5, 2], &[1, 1]]));
        assert!(needs_credentials(&r.unwrap_err()));
        // 認証を求められたが、ユーザー名とパスワードがない
        let (r, got) = exchange(&p, None, "h", frames(&[&[5, 2]]));
        let e = r.unwrap_err();
        assert!(
            needs_credentials(&e) && e.to_string().contains("認証が必要"),
            "{e}"
        );
        assert_eq!(got, [5, 2, 0, 2]);
        let (r, _) = exchange(
            &proxy("socks5://socks"),
            None,
            "h",
            frames(&[&[5, 0], &[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]]),
        );
        let e = r.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        assert!(e.to_string().contains("拒否"), "{e}");
    }

    #[test]
    fn socks4a_connect() {
        let (r, got) = exchange(
            &proxy("socks4://me@socks"),
            None,
            "build",
            frames(&[&[0, 0x5a, 0, 0, 0, 0, 0, 0]]),
        );
        r.unwrap();
        assert_eq!(
            got,
            [&[4, 1, 0, 22, 0, 0, 0, 1][..], b"me\0build\0"].concat()
        );
        let (r, got) = exchange(
            &proxy("socks4://socks"),
            None,
            "10.1.2.3",
            frames(&[&[0, 0x5b, 0, 0, 0, 0, 0, 0]]),
        );
        assert!(r.is_err());
        assert_eq!(got, [4, 1, 0, 22, 10, 1, 2, 3, 0]);
    }
}
