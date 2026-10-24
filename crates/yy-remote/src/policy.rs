//! Shared per-host SSH and file-transfer policy for all YY applications.
use crate::{
    listing::ListCommand,
    ssh_config::{HostOverride, Resolver},
    uri::Target,
    xfer::Protocol,
};
use yy_config::{RemoteConfig, RemoteHost, TransferProtocol};

pub fn host<'a>(config: &'a RemoteConfig, target: &Target) -> Option<&'a RemoteHost> {
    config
        .host
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&target.host))
        .map(|(_, host)| host)
}

pub fn protocol(config: &RemoteConfig, target: &Target, fallback: Protocol) -> Protocol {
    match host(config, target).and_then(|h| h.protocol) {
        Some(TransferProtocol::Scp) => Protocol::Scp,
        Some(TransferProtocol::Sftp) => Protocol::Sftp,
        None => fallback,
    }
}

/// An explicit transfer protocol always disables automatic agent deployment.
pub fn use_agent(config: &RemoteConfig, target: &Target, fallback: bool) -> bool {
    match host(config, target) {
        Some(h) if h.protocol.is_some() => false,
        Some(h) => h.use_agent.unwrap_or(fallback),
        None => fallback,
    }
}

pub fn listing(config: &RemoteConfig, target: &Target) -> std::io::Result<Option<ListCommand>> {
    let h = host(config, target);
    ListCommand::from_settings(
        h.and_then(|h| h.list_command.as_deref()),
        h.and_then(|h| h.list_format.as_deref()),
    )
}

pub fn resolver(config: &RemoteConfig) -> Resolver {
    let mut r = Resolver::from_environment(config.read_ssh_config);
    r.hosts = config
        .host
        .iter()
        .map(|(name, h)| {
            (
                name.clone(),
                HostOverride {
                    ssh: h.ssh.clone(),
                    hostname: h.hostname.clone(),
                    user: h.user.clone(),
                    port: h.port,
                    identity_file: h.identity_file.clone(),
                    proxy_jump: h.proxy_jump.clone(),
                    jump_command: h.jump_command.clone(),
                    sftp_subsystem: h.sftp_subsystem.clone(),
                    sftp_server: h.sftp_server.clone(),
                    proxy: h.proxy.clone(),
                    agent_dir: h.agent_dir.clone(),
                    forward: h.forward.clone(),
                },
            )
        })
        .collect();
    r.agent_dir = Some(config.agent_dir.clone()).filter(|d| !d.trim().is_empty());
    r.proxy = Some(config.proxy.clone()).filter(|p| !p.trim().is_empty());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_share_resolution_and_explicit_protocol_wins_over_agent_defaults() {
        let config = yy_config::Config::from_toml(
            r#"
            [remote]
            read_ssh_config = false
            [remote.host.gateway]
            hostname = "gateway.example"
            user = "operator@target-user@target-host"
            protocol = "scp"
            use_agent = true
            list_command = "ls -la"
            list_format = "ls"
            [remote.host.sftp]
            protocol = "sftp"
            [remote.host.agent]
            use_agent = true
        "#,
        )
        .unwrap()
        .remote;
        let target = Target::parse("GATEWAY").unwrap();
        assert_eq!(protocol(&config, &target, Protocol::Sftp), Protocol::Scp);
        assert!(!use_agent(&config, &target, true));
        assert!(listing(&config, &target).unwrap().is_some());
        let spec = resolver(&config).resolve(&target);
        assert_eq!(spec.user, "operator@target-user@target-host");
        assert_eq!(spec.hostname, "gateway.example");
        let sftp = Target::parse("sftp").unwrap();
        assert_eq!(protocol(&config, &sftp, Protocol::Scp), Protocol::Sftp);
        assert!(!use_agent(&config, &sftp, true));
        assert!(use_agent(&config, &Target::parse("agent").unwrap(), false));
        assert!(
            listing(&config, &Target::parse("gateway.example").unwrap())
                .unwrap()
                .is_none()
        );
    }
}
