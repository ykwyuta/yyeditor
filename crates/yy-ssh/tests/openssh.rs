//! Independent OpenSSH peer, all AEAD ciphers, binary traffic and forced rekeying.
#![cfg(unix)]

use std::borrow::Cow;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Session};
use russh::{Channel, ChannelId, cipher};
use tokio::io::AsyncReadExt;

struct Peer {
    allowed: PublicKey,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if *key == self.allowed {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(command, b"echo-bytes");
        session.channel_success(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, data.to_vec())?;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn openssh_aead_binary_transfer_and_rekey() {
    let available = std::process::Command::new("ssh").arg("-V").output();
    if available.is_err() {
        assert!(
            std::env::var_os("YY_REQUIRE_OPENSSH").is_none(),
            "OpenSSH client is required"
        );
        eprintln!("OpenSSH client unavailable; set YY_REQUIRE_OPENSSH=1 to require it");
        return;
    }
    for algorithm in [
        cipher::CHACHA20_POLY1305,
        cipher::AES_128_GCM,
        cipher::AES_256_GCM,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let key_path = dir.path().join("identity");
        std::fs::write(
            &key_path,
            client_key
                .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let host = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let host_public = host.public_key().to_openssh().unwrap();
        let preferred = russh::Preferred {
            cipher: Cow::Owned(vec![algorithm]),
            ..Default::default()
        };
        let config = Arc::new(server::Config {
            keys: vec![host],
            preferred,
            limits: russh::Limits {
                rekey_write_limit: 64 * 1024,
                ..Default::default()
            },
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = Peer {
            allowed: client_key.public_key().clone(),
        };
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            server::run_stream(config, stream, peer).await?.await
        });
        let known_hosts = dir.path().join("known_hosts");
        std::fs::write(&known_hosts, format!("[127.0.0.1]:{port} {host_public}\n")).unwrap();
        let payload: Vec<_> = (0..512 * 1024).map(|n| (n % 251) as u8).collect();
        let input = dir.path().join("input");
        std::fs::write(&input, &payload).unwrap();
        let mut child = tokio::process::Command::new("ssh")
            .args(["-v", "-F", "/dev/null", "-T", "-p", &port.to_string(), "-i"])
            .arg(&key_path)
            .args([
                "-oBatchMode=yes",
                "-oIdentitiesOnly=yes",
                "-oIdentityAgent=none",
                "-oStrictHostKeyChecking=yes",
                "-oGlobalKnownHostsFile=/dev/null",
                "-oUpdateHostKeys=no",
                "-oConnectTimeout=10",
                "-oRekeyLimit=64K",
            ])
            .arg(format!("-oUserKnownHostsFile={}", known_hosts.display()))
            .arg(format!("-oCiphers={}", algorithm.as_ref()))
            .args(["test@127.0.0.1", "echo-bytes"])
            .stdin(std::fs::File::open(input).unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let output = tokio::spawn(async move {
            let mut b = Vec::new();
            stdout.read_to_end(&mut b).await?;
            Ok::<_, io::Error>(b)
        });
        let errors = tokio::spawn(async move {
            let mut b = String::new();
            stderr.read_to_string(&mut b).await?;
            Ok::<_, io::Error>(b)
        });
        let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
        if status.is_err() {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
        }
        let output = output.await.unwrap().unwrap();
        let errors = errors.await.unwrap().unwrap();
        let status = status
            .unwrap_or_else(|_| panic!("OpenSSH timeout: {errors}"))
            .unwrap();
        assert!(
            status.success(),
            "{}: {status}: {errors}",
            algorithm.as_ref()
        );
        assert_eq!(output, payload, "{} binary payload", algorithm.as_ref());
        assert!(
            errors.matches("SSH2_MSG_NEWKEYS sent").count() >= 2,
            "{} never rekeyed: {errors}",
            algorithm.as_ref()
        );
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
