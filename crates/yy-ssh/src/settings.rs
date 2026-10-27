use russh::{cipher, client, compression, kex, keys::Algorithm, mac};
use std::{borrow::Cow, io, time::Duration};

fn invalid(what: &str, value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("未対応の{what}: {value}"),
    )
}

pub(super) fn client_config(
    settings: &yy_remote::ssh_config::HostSpecSettings,
    keepalive: Duration,
) -> io::Result<client::Config> {
    if settings.connect_timeout_secs == 0 || settings.connect_timeout_secs > 3600 {
        return Err(invalid(
            "接続タイムアウト（1～3600秒）",
            &settings.connect_timeout_secs.to_string(),
        ));
    }
    if !settings.public_key_auth && !settings.password_auth && !settings.keyboard_interactive_auth {
        return Err(invalid("認証設定", "すべての認証方式が無効です"));
    }
    let mut config = client::Config {
        keepalive_interval: match settings.keepalive_secs {
            Some(0) => None,
            Some(n) => Some(Duration::from_secs(n.into())),
            None => Some(keepalive),
        },
        keepalive_max: settings.keepalive_max as usize,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    };
    if !settings.ciphers.is_empty() {
        config.preferred.cipher = Cow::Owned(
            settings
                .ciphers
                .iter()
                .map(|s| cipher::Name::try_from(s.as_str()).map_err(|_| invalid("暗号", s)))
                .collect::<io::Result<_>>()?,
        );
    }
    if !settings.key_exchange.is_empty() {
        config.preferred.kex = Cow::Owned(
            settings
                .key_exchange
                .iter()
                .map(|s| kex::Name::try_from(s.as_str()).map_err(|_| invalid("鍵交換", s)))
                .collect::<io::Result<_>>()?,
        );
    }
    if !settings.macs.is_empty() {
        config.preferred.mac = Cow::Owned(
            settings
                .macs
                .iter()
                .map(|s| mac::Name::try_from(s.as_str()).map_err(|_| invalid("MAC", s)))
                .collect::<io::Result<_>>()?,
        );
    }
    if !settings.host_key_algorithms.is_empty() {
        config.preferred.key = Cow::Owned(
            settings
                .host_key_algorithms
                .iter()
                .map(|s| Algorithm::new(s).map_err(|_| invalid("ホスト鍵アルゴリズム", s)))
                .collect::<io::Result<_>>()?,
        );
    }
    if settings.compression {
        config.preferred.compression =
            Cow::Owned(vec![compression::ZLIB_LEGACY, compression::NONE]);
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_algorithms_and_keepalive_are_applied_without_silent_fallback() {
        let mut s = yy_remote::ssh_config::HostSpecSettings {
            keepalive_secs: Some(0),
            ciphers: vec!["aes256-ctr".into()],
            compression: true,
            ..Default::default()
        };
        let config = client_config(&s, Duration::from_secs(15)).unwrap();
        assert!(config.keepalive_interval.is_none());
        assert_eq!(config.preferred.cipher.len(), 1);
        assert_eq!(config.preferred.cipher[0].as_ref(), "aes256-ctr");
        assert_eq!(config.preferred.compression[0], compression::ZLIB_LEGACY);
        s.ciphers = vec!["typo-cipher".into()];
        assert_eq!(
            client_config(&s, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
