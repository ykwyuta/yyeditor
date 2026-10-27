//! Real SSH sockets and SFTP packets, with a simulated access gateway.
//! No external SSH/SFTP programs or product credentials are required.
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yy_remote::sftp::{Attrs, Sftp};
use yy_remote::ssh_config::{HostOverride, Resolver};
use yy_remote::{ConnectLog, Connector, HostKeyQuestion, Prompter};

struct Gateway {
    channels: HashMap<ChannelId, Channel<Msg>>,
    events: Arc<Mutex<Vec<String>>>,
    relay_port: Option<u16>,
    challenge: usize,
}

impl Clone for Gateway {
    fn clone(&self) -> Self {
        Self {
            channels: HashMap::new(),
            events: self.events.clone(),
            relay_port: self.relay_port,
            challenge: 0,
        }
    }
}

impl server::Handler for Gateway {
    type Error = russh::Error;
    async fn shell_request(
        &mut self,
        id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push("shell:no-pty".into());
        session.channel_success(id)?;
        let channel = self.channels.remove(&id).unwrap();
        let handle = session.handle();
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            stream
                .write_all(b"Gateway login banner\r\n$ ")
                .await
                .unwrap();
            let mut script = Vec::new();
            loop {
                let b = stream.read_u8().await.unwrap();
                script.push(b);
                if b == b'\n' {
                    break;
                }
                assert!(script.len() < 65536);
            }
            assert!(
                script
                    .windows(b"exec sh -c".len())
                    .any(|w| w == b"exec sh -c")
            );
            let start = script.iter().position(|&b| b == 1).unwrap();
            let end = script.iter().position(|&b| b == 2).unwrap() + 1;
            // Split the marker to exercise scanning across SSH packets, then send every byte value.
            stream.write_all(&script[start..start + 2]).await.unwrap();
            stream.flush().await.unwrap();
            stream.write_all(&script[start + 2..end]).await.unwrap();
            let binary: Vec<_> = (0..65536).map(|n| (n % 256) as u8).collect();
            stream.write_all(&binary).await.unwrap();
            stream.flush().await.unwrap();
            let _ = handle.exit_status_request(id, 0).await;
            let _ = handle.eof(id).await;
            let _ = handle.close(id).await;
        });
        Ok(())
    }
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        self.events.lock().unwrap().push(format!("auth:{user}"));
        if let Some(mut response) = response {
            if response.next().as_deref() != Some(b"answer") {
                return Ok(Auth::reject());
            }
            self.challenge += 1;
            if self.challenge == 2 {
                return Ok(Auth::Accept);
            }
        }
        Ok(Auth::Partial {
            name: "Gateway".into(),
            instructions: "Two-stage authentication".into(),
            prompts: vec![(
                if self.challenge == 0 {
                    "Password"
                } else {
                    "OTP"
                }
                .into(),
                false,
            )]
            .into(),
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.events.lock().unwrap().push("direct-tcpip".into());
        if self.relay_port == Some(port as u16) {
            reply.accept().await;
            relay(channel, port as u16);
        } else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.events
            .lock()
            .unwrap()
            .push(format!("subsystem:{name}"));
        if name == "gateway-sftp" || name == "sftp" {
            session.channel_success(id)?;
            tokio::spawn(serve_sftp(self.channels.remove(&id).unwrap()));
        } else {
            session.channel_failure(id)?;
        }
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(command);
        self.events.lock().unwrap().push(format!("exec:{command}"));
        if command == "/test/sftp-server" {
            session.channel_success(id)?;
            tokio::spawn(serve_sftp(self.channels.remove(&id).unwrap()));
        } else if let Some(port) = self.relay_port
            && command == format!("relay '127.0.0.1' {port}")
        {
            session.channel_success(id)?;
            relay(self.channels.remove(&id).unwrap(), port);
        } else {
            session.channel_failure(id)?;
        }
        Ok(())
    }
}

fn relay(channel: Channel<Msg>, port: u16) {
    tokio::spawn(async move {
        let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let _ = tokio::io::copy_bidirectional(&mut channel.into_stream(), &mut tcp).await;
    });
}

struct Server {
    port: u16,
    events: Arc<Mutex<Vec<String>>>,
    _rt: tokio::runtime::Runtime,
}
impl Server {
    fn new(relay_port: Option<u16>) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let handler = Gateway {
            channels: HashMap::new(),
            events: events.clone(),
            relay_port,
            challenge: 0,
        };
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        rt.spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let h = handler.clone();
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(s) = server::run_stream(config, socket, h).await {
                        let _ = s.await;
                    }
                });
            }
        });
        Self {
            port,
            events,
            _rt: rt,
        }
    }
}

#[derive(Default)]
struct Answers(Mutex<Vec<String>>);
impl Prompter for Answers {
    fn confirm_host_key(&self, q: &HostKeyQuestion) -> bool {
        self.0.lock().unwrap().push(format!("key:{}", q.port));
        true
    }
    fn password(&self, _: &str) -> Option<String> {
        panic!("expected keyboard-interactive")
    }
    fn passphrase(&self, _: &Path) -> Option<String> {
        None
    }
    fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &str,
        q: &[(String, bool)],
    ) -> Option<Vec<String>> {
        self.0.lock().unwrap().push(q[0].0.clone());
        Some(vec!["answer".into(); q.len()])
    }
}

// Minimal SFTP-v3 fixture with real framing, IDs, offsets and binary file data.
async fn serve_sftp(channel: Channel<Msg>) -> io::Result<()> {
    let mut stream = channel.into_stream();
    let mut file = Vec::new();
    let mut listed = false;
    while let Ok(len) = stream.read_u32().await {
        assert!(len < 1 << 20);
        let mut packet = vec![0; len as usize];
        stream.read_exact(&mut packet).await?;
        let kind = packet[0];
        let mut p = &packet[1..];
        let id = number(&mut p);
        let mut out = if kind == 1 { vec![2] } else { vec![101] };
        out.extend_from_slice(&id.to_be_bytes());
        match kind {
            1 => assert_eq!(id, 3), // VERSION uses the version slot instead of an ID
            3 | 11 => {
                out[0] = 102;
                bytes(&mut out, b"file");
            }
            16 => {
                out[0] = 104;
                out.extend_from_slice(&1u32.to_be_bytes());
                bytes(&mut out, b"/test");
                bytes(&mut out, b"/test");
                out.extend_from_slice(&0u32.to_be_bytes());
            }
            12 if !listed => {
                listed = true;
                out[0] = 104;
                out.extend_from_slice(&1u32.to_be_bytes());
                bytes(&mut out, b"binary.dat");
                bytes(&mut out, b"binary.dat");
                out.extend_from_slice(&1u32.to_be_bytes());
                out.extend_from_slice(&(file.len() as u64).to_be_bytes());
            }
            12 => {
                out.extend_from_slice(&1u32.to_be_bytes());
                bytes(&mut out, b"EOF");
                bytes(&mut out, b"");
            }
            4 => {
                out.extend_from_slice(&0u32.to_be_bytes());
                bytes(&mut out, b"");
                bytes(&mut out, b"");
            }
            6 | 5 => {
                let _handle = string(&mut p);
                let offset = u64::from_be_bytes(p[..8].try_into().unwrap()) as usize;
                p = &p[8..];
                if kind == 6 {
                    let data = string(&mut p);
                    file.resize(file.len().max(offset + data.len()), 0);
                    file[offset..offset + data.len()].copy_from_slice(data);
                    out.extend_from_slice(&0u32.to_be_bytes());
                    bytes(&mut out, b"");
                    bytes(&mut out, b"");
                } else {
                    let len = number(&mut p) as usize;
                    out[0] = 103;
                    bytes(&mut out, &file[offset..(offset + len).min(file.len())]);
                }
            }
            _ => panic!("unexpected SFTP request {kind}"),
        }
        stream.write_u32(out.len() as u32).await?;
        stream.write_all(&out).await?;
        stream.flush().await?;
    }
    Ok(())
}
fn number(p: &mut &[u8]) -> u32 {
    let n = u32::from_be_bytes(p[..4].try_into().unwrap());
    *p = &p[4..];
    n
}
fn string<'a>(p: &mut &'a [u8]) -> &'a [u8] {
    let len = number(p) as usize;
    let value = &p[..len];
    *p = &p[len..];
    value
}
fn bytes(p: &mut Vec<u8>, value: &[u8]) {
    p.extend_from_slice(&(value.len() as u32).to_be_bytes());
    p.extend_from_slice(value);
}

#[test]
fn transfers_binary_through_gateway_using_tunnel_command_or_sftp_proxy() {
    for route in ["tunnel", "command", "proxy", "default"] {
        let target = Server::new(None);
        let gw = Server::new(Some(target.port));
        let dir = tempfile::tempdir().unwrap();
        let connector = yy_ssh::SshConnector {
            known_hosts: dir.path().join("known_hosts"),
            extra_known_hosts: vec![],
            keepalive: Duration::from_secs(30),
            passwords: None,
        };
        let mut resolver = Resolver::default();
        resolver.hosts.push((
            "gw".into(),
            HostOverride {
                hostname: Some("127.0.0.1".into()),
                port: Some(gw.port),
                user: Some("operator@target-user@target-host-user@target-host".into()),
                jump_command: (route == "command").then(|| "relay %h %p".into()),
                sftp_subsystem: Some("gateway-sftp".into()),
                ..Default::default()
            },
        ));
        resolver.hosts.push((
            "target".into(),
            HostOverride {
                hostname: Some("127.0.0.1".into()),
                port: Some(target.port),
                user: Some("target-user".into()),
                proxy_jump: Some("gw".into()),
                sftp_server: (route != "default").then(|| "/test/sftp-server".into()),
                ..Default::default()
            },
        ));
        let spec = resolver.resolve(
            &yy_remote::uri::Target::parse(if route == "proxy" { "gw" } else { "target" }).unwrap(),
        );
        let answers = Answers::default();
        let transport = connector
            .connect(&spec, &answers, &ConnectLog::default())
            .unwrap();
        let fs = yy_remote::SftpFs::connect(transport.as_ref()).unwrap();
        let sftp = fs.sftp();
        let handle = sftp.open(b"binary.dat", 0x1b, &Attrs::default()).unwrap();
        let content: Vec<u8> = (0..65536).map(|n| (n % 256) as u8).collect();
        let pending: Vec<_> = content
            .chunks(16384)
            .enumerate()
            .map(|(i, data)| sftp.send_write(&handle, (i * 16384) as u64, data).unwrap())
            .collect();
        for p in pending {
            p.wait().unwrap();
        }
        let mut received = Vec::new();
        for i in 0..4 {
            received.extend(
                sftp.send_read(&handle, i * 16384, 16384)
                    .unwrap()
                    .wait()
                    .unwrap(),
            );
        }
        assert_eq!(received, content);
        sftp.close(&handle).unwrap();
        let listing = sftp.read_dir(b".").unwrap();
        assert_eq!(listing[0].name, b"binary.dat");
        assert_eq!(listing[0].attrs.size, Some(content.len() as u64));
        let events = gw.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e == "auth:operator@target-user@target-host-user@target-host")
        );
        assert!(events.iter().any(|e| match route {
            "tunnel" | "default" => e == "direct-tcpip",
            "command" => e.starts_with("exec:relay "),
            _ => e == "subsystem:gateway-sftp",
        }));
        if route == "command" {
            assert!(!events.iter().any(|e| e == "direct-tcpip"));
        }
        assert!(answers.0.lock().unwrap().contains(&"OTP".into()));
        drop(events);
        drop(fs);
        // A failed start must return promptly, without hanging on a packet read.
        let mut bad = spec.clone();
        bad.sftp = yy_remote::ssh_config::SftpStart::Subsystem("denied".into());
        let other = connector
            .connect(&bad, &answers, &ConnectLog::default())
            .unwrap();
        let error = Sftp::connect(other.as_ref())
            .err()
            .expect("must reject unknown subsystem");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
}

#[test]
fn shell_request_mode_strips_login_banner_and_keeps_binary_bytes_without_exec_or_pty() {
    use std::io::Read;
    let server = Server::new(None);
    let dir = tempfile::tempdir().unwrap();
    let connector = yy_ssh::SshConnector::new(&yy_remote::ConnectorOptions {
        known_hosts: dir.path().join("known_hosts"),
        extra_known_hosts: vec![],
        keepalive: Duration::from_secs(15),
        passwords: None,
    });
    let mut resolver = Resolver::default();
    resolver.hosts.push((
        "shell-only".into(),
        HostOverride {
            hostname: Some("127.0.0.1".into()),
            port: Some(server.port),
            user: Some("operator@target-user@target-host".into()),
            ..Default::default()
        },
    ));
    let mut spec = resolver.resolve(&yy_remote::uri::Target::parse("shell-only").unwrap());
    spec.ssh.exec_via_shell = true;
    let transport = connector
        .connect(&spec, &Answers::default(), &ConnectLog::default())
        .unwrap();
    let (input, mut output, finish) = transport.exec(b"binary-test").unwrap().into_parts();
    drop(input);
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        bytes,
        (0..65536).map(|n| (n % 256) as u8).collect::<Vec<_>>()
    );
    drop(output);
    assert_eq!(finish().unwrap().status, Some(0));
    let events = server.events.lock().unwrap();
    assert!(events.iter().any(|e| e == "shell:no-pty"));
    assert!(
        events
            .iter()
            .any(|e| e == "auth:operator@target-user@target-host")
    );
    assert!(
        !events
            .iter()
            .any(|e| e.starts_with("exec:") || e.starts_with("subsystem:"))
    );
}
