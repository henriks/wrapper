use std::env;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use agentvm_frontend::launch::{
    prepare_frontend_launch_with_policy, run_frontend_until_qemu_exit_with_policy_and_timeout,
};
use agentvm_frontend::network_policy::{
    EgressAction, EgressReason, HostListener, HostListenerPurpose, VmnetPolicy,
};
use agentvm_frontend::runtime_manifest::workspace_mounts;
use agentvm_frontend::tcp_gateway::UpstreamMapping;
use agentvm_frontend::vmnet_runtime::{serve_vmnet_gateway, VmnetRuntimeConfig};
use agentvm_frontend::{FrontendConfig, GuestNetwork};

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.first().map(String::as_str) {
        Some("vmnet-gateway") => {
            let config = vmnet_gateway_config_from_args(&args[1..])?;
            serve_vmnet_gateway(config)
                .map_err(|error| format!("vmnet gateway failed: {error:?}"))?;
            Ok(())
        }
        Some("prepare") => {
            let (config, policy_args) = frontend_config_from_args(&args[1..])?;
            let policy = policy_from_args(config.network.clone(), policy_args);
            let prep = prepare_frontend_launch_with_policy(
                &config,
                &workspace_mounts(config.project.clone()),
                &policy,
            )
            .map_err(|error| format!("prepare failed: {error}"))?;
            println!("{}", prep.qemu_command.join(" "));
            Ok(())
        }
        Some("launch") => {
            let (config, policy_args) = frontend_config_from_args(&args[1..])?;
            let qemu_timeout = policy_args.qemu_timeout;
            let local_http_smoke_upstream = policy_args.local_http_smoke_upstream;
            let policy = policy_from_args(config.network.clone(), policy_args);
            let mut config = config;
            if let Some(destination) = local_http_smoke_upstream {
                config
                    .upstream_mappings
                    .push(start_local_http_smoke_upstream(destination)?);
            }
            let qemu_exit = run_frontend_until_qemu_exit_with_policy_and_timeout(
                config.clone(),
                workspace_mounts(config.project.clone()),
                policy,
                qemu_timeout,
            )
            .map_err(|error| format!("launch failed: {error}"))?;
            if qemu_exit.status.success() {
                Ok(())
            } else if qemu_exit.timed_out {
                Err(format!(
                    "qemu timed out after {} seconds and was terminated with status: {}",
                    qemu_timeout.map_or(0, |timeout| timeout.as_secs()),
                    qemu_exit.status
                ))
            } else {
                Err(format!("qemu exited with status: {}", qemu_exit.status))
            }
        }
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}")),
    }
}

fn vmnet_gateway_config_from_args(args: &[String]) -> Result<VmnetRuntimeConfig, String> {
    let mut socket_path = None;
    let mut network = GuestNetwork::default();
    let mut allow_ips = Vec::new();
    let mut allow_domains = Vec::new();
    let mut allow_public = false;
    let mut no_net = false;
    let mut host_listeners = Vec::new();
    let mut tls_ca_cert = None;
    let mut tls_ca_key = None;
    let mut tls_generate_per_host_certs = false;
    let mut pcap_path = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--socket" => {
                socket_path = Some(PathBuf::from(value(args, &mut index, "--socket")?));
            }
            "--guest-ip" => network.guest_ip = value(args, &mut index, "--guest-ip")?,
            "--gateway-ip" => network.gateway_ip = value(args, &mut index, "--gateway-ip")?,
            "--dns-ip" => network.dns_ip = value(args, &mut index, "--dns-ip")?,
            "--guest-mac" => network.guest_mac = value(args, &mut index, "--guest-mac")?,
            "--prefix-len" => {
                network.prefix_len = value(args, &mut index, "--prefix-len")?
                    .parse()
                    .map_err(|_| "invalid --prefix-len".to_string())?;
            }
            "--allow-ip" => allow_ips.push(value(args, &mut index, "--allow-ip")?),
            "--allow-domain" => allow_domains.push(value(args, &mut index, "--allow-domain")?),
            "--allow-public-internet" => {
                allow_public = true;
            }
            "--no-net" => {
                no_net = true;
            }
            "--host-docker-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-docker-listener")?)?;
                host_listeners.push(HostListener::docker_api(host_port, guest_port));
            }
            "--host-payload-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-payload-listener")?)?;
                host_listeners.push(HostListener::payload_control(host_port, guest_port));
            }
            "--publish" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--publish")?)?;
                host_listeners.push(HostListener::published_tcp(host_port, guest_port));
            }
            "--pcap" => {
                pcap_path = Some(PathBuf::from(value(args, &mut index, "--pcap")?));
            }
            "--tls-ca-cert" => {
                tls_ca_cert = Some(PathBuf::from(value(args, &mut index, "--tls-ca-cert")?));
            }
            "--tls-ca-key" => {
                tls_ca_key = Some(PathBuf::from(value(args, &mut index, "--tls-ca-key")?));
            }
            "--tls-generate-per-host-certs" => {
                tls_generate_per_host_certs = true;
            }
            "-h" | "--help" => {
                print_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown vmnet-gateway option: {unknown}")),
        }
        index += 1;
    }

    let socket_path = socket_path.ok_or_else(|| "--socket is required".to_string())?;
    validate_no_net_args(
        no_net,
        allow_public,
        &allow_ips,
        &allow_domains,
        &host_listeners,
    )?;
    let mut policy = VmnetPolicy::default_sandbox(network.clone());
    policy.egress.allow_ips = allow_ips;
    policy.egress.allow_domains = allow_domains;
    if allow_public {
        policy.egress.default_action = EgressAction::AllowPublicInternet;
        policy.egress.reason = EgressReason::ExplicitAllowProfile;
    } else if no_net {
        policy.egress.default_action = EgressAction::Deny;
        policy.egress.reason = EgressReason::NoNetFlag;
    }
    policy.host_listeners = host_listeners;
    policy.tls_mitm.ca_cert_path = tls_ca_cert;
    policy.tls_mitm.ca_key_path = tls_ca_key;
    policy.tls_mitm.generate_per_host_certs = tls_generate_per_host_certs;
    policy.capture.pcap_path = pcap_path;
    policy.capture.capture_guest_side_frames = policy.capture.pcap_path.is_some();

    Ok(VmnetRuntimeConfig::new(socket_path, network, policy))
}

#[derive(Debug, Default)]
struct PolicyArgs {
    allow_ips: Vec<String>,
    allow_domains: Vec<String>,
    allow_public: bool,
    no_net: bool,
    qemu_timeout: Option<Duration>,
    local_http_smoke_upstream: Option<(Ipv4Addr, u16)>,
    host_listeners: Vec<HostListener>,
    tls_ca_cert: Option<PathBuf>,
    tls_ca_key: Option<PathBuf>,
    tls_generate_per_host_certs: bool,
    pcap_path: Option<PathBuf>,
}

fn frontend_config_from_args(args: &[String]) -> Result<(FrontendConfig, PolicyArgs), String> {
    let mut project = PathBuf::from(".");
    let mut run_dir = PathBuf::from(".sandbox/docker-vm/run");
    let mut artifact_manifest = PathBuf::from("docker/out/artifact-manifest.json");
    let mut qemu = PathBuf::from("qemu-system-x86_64");
    let mut policy = PolicyArgs::default();
    let mut guest_http_smoke_url = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--project" => project = PathBuf::from(value(args, &mut index, "--project")?),
            "--run-dir" => run_dir = PathBuf::from(value(args, &mut index, "--run-dir")?),
            "--artifact-manifest" => {
                artifact_manifest = PathBuf::from(value(args, &mut index, "--artifact-manifest")?);
            }
            "--qemu" => qemu = PathBuf::from(value(args, &mut index, "--qemu")?),
            "--guest-http-smoke-url" => {
                guest_http_smoke_url = Some(value(args, &mut index, "--guest-http-smoke-url")?);
            }
            "--allow-ip" => policy
                .allow_ips
                .push(value(args, &mut index, "--allow-ip")?),
            "--allow-domain" => {
                policy
                    .allow_domains
                    .push(value(args, &mut index, "--allow-domain")?)
            }
            "--allow-public-internet" => policy.allow_public = true,
            "--no-net" => policy.no_net = true,
            "--qemu-timeout-seconds" => {
                let seconds = value(args, &mut index, "--qemu-timeout-seconds")?
                    .parse::<u64>()
                    .map_err(|_| "invalid --qemu-timeout-seconds".to_string())?;
                policy.qemu_timeout = Some(Duration::from_secs(seconds));
            }
            "--local-http-smoke-upstream" => {
                policy.local_http_smoke_upstream = Some(parse_ip_port(&value(
                    args,
                    &mut index,
                    "--local-http-smoke-upstream",
                )?)?);
            }
            "--host-docker-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-docker-listener")?)?;
                policy
                    .host_listeners
                    .push(HostListener::docker_api(host_port, guest_port));
            }
            "--host-payload-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-payload-listener")?)?;
                policy
                    .host_listeners
                    .push(HostListener::payload_control(host_port, guest_port));
            }
            "--publish" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--publish")?)?;
                policy
                    .host_listeners
                    .push(HostListener::published_tcp(host_port, guest_port));
            }
            "--pcap" => {
                policy.pcap_path = Some(PathBuf::from(value(args, &mut index, "--pcap")?));
            }
            "--tls-ca-cert" => {
                policy.tls_ca_cert = Some(PathBuf::from(value(args, &mut index, "--tls-ca-cert")?));
            }
            "--tls-ca-key" => {
                policy.tls_ca_key = Some(PathBuf::from(value(args, &mut index, "--tls-ca-key")?));
            }
            "--tls-generate-per-host-certs" => {
                policy.tls_generate_per_host_certs = true;
            }
            "-h" | "--help" => {
                print_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown frontend option: {unknown}")),
        }
        index += 1;
    }

    let mut config =
        FrontendConfig::from_artifact_manifest_file(project, run_dir, qemu, &artifact_manifest)
            .map_err(|error| format!("failed to load frontend config: {error}"))?;
    config.guest_http_smoke_url = guest_http_smoke_url;
    validate_no_net_args(
        policy.no_net,
        policy.allow_public,
        &policy.allow_ips,
        &policy.allow_domains,
        &policy.host_listeners,
    )?;
    if config.guest_http_smoke_url.is_some() {
        ensure_smoke_hook_artifact_fresh(&artifact_manifest)?;
    }
    Ok((config, policy))
}

fn parse_ip_port(value: &str) -> Result<(Ipv4Addr, u16), String> {
    let (ip, port) = value
        .rsplit_once(':')
        .ok_or_else(|| "expected IP:PORT".to_string())?;
    Ok((
        ip.parse()
            .map_err(|_| format!("invalid IPv4 address in {value}"))?,
        port.parse()
            .map_err(|_| format!("invalid port in {value}"))?,
    ))
}

fn parse_port_pair(value: &str) -> Result<(u16, u16), String> {
    let (host_port, guest_port) = value
        .split_once(':')
        .ok_or_else(|| "expected HOST_PORT:GUEST_PORT".to_string())?;
    Ok((
        host_port
            .parse()
            .map_err(|_| format!("invalid host port in {value}"))?,
        guest_port
            .parse()
            .map_err(|_| format!("invalid guest port in {value}"))?,
    ))
}

fn start_local_http_smoke_upstream(
    destination: (Ipv4Addr, u16),
) -> Result<UpstreamMapping, String> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| error.to_string())?;
    let host_port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    thread::Builder::new()
        .name("agentvm-local-http-smoke".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                );
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(UpstreamMapping {
        guest_ip: destination.0,
        guest_port: destination.1,
        host_ip: Ipv4Addr::LOCALHOST,
        host_port,
    })
}

fn ensure_smoke_hook_artifact_fresh(artifact_manifest: &std::path::Path) -> Result<(), String> {
    let Some(repo_root) = artifact_manifest
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
    else {
        return Ok(());
    };
    let guest_init = repo_root.join("docker/guest-init.sh");
    let rootfs = repo_root.join("docker/out/rootfs.raw");
    if !guest_init.exists() || !rootfs.exists() {
        return Ok(());
    }
    let guest_init_modified = guest_init
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("failed to stat {}: {error}", guest_init.display()))?;
    let rootfs_modified = rootfs
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("failed to stat {}: {error}", rootfs.display()))?;
    if guest_init_modified >= rootfs_modified {
        return Err(format!(
            "--guest-http-smoke-url requires rebuilt appliance artifacts; {} is newer than {}",
            guest_init.display(),
            rootfs.display()
        ));
    }
    Ok(())
}

fn policy_from_args(network: GuestNetwork, args: PolicyArgs) -> VmnetPolicy {
    let mut policy = VmnetPolicy::default_sandbox(network);
    policy.egress.allow_ips = args.allow_ips;
    policy.egress.allow_domains = args.allow_domains;
    if args.allow_public {
        policy.egress.default_action = EgressAction::AllowPublicInternet;
        policy.egress.reason = EgressReason::ExplicitAllowProfile;
    } else if args.no_net {
        policy.egress.default_action = EgressAction::Deny;
        policy.egress.reason = EgressReason::NoNetFlag;
    }
    policy.host_listeners = args.host_listeners;
    policy.tls_mitm.ca_cert_path = args.tls_ca_cert;
    policy.tls_mitm.ca_key_path = args.tls_ca_key;
    policy.tls_mitm.generate_per_host_certs = args.tls_generate_per_host_certs;
    policy.capture.pcap_path = args.pcap_path;
    policy.capture.capture_guest_side_frames = policy.capture.pcap_path.is_some();
    policy
}

fn validate_no_net_args(
    no_net: bool,
    allow_public: bool,
    allow_ips: &[String],
    allow_domains: &[String],
    host_listeners: &[HostListener],
) -> Result<(), String> {
    if !no_net {
        return Ok(());
    }
    if allow_public || !allow_ips.is_empty() || !allow_domains.is_empty() {
        return Err("--no-net cannot be combined with egress allow options".to_string());
    }
    if host_listeners
        .iter()
        .any(|listener| listener.purpose == HostListenerPurpose::PublishedTcp)
    {
        return Err("--no-net cannot be combined with --publish".to_string());
    }
    Ok(())
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: agentvm-frontend <prepare|launch|vmnet-gateway> [options]\n\
         prepare/launch options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--guest-http-smoke-url URL] [--allow-public-internet|--no-net] [--qemu-timeout-seconds N] [--local-http-smoke-upstream IP:PORT] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         vmnet-gateway options: --socket PATH [--allow-ip IP_OR_CIDR] [--allow-domain DOMAIN] [--allow-public-internet|--no-net] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vmnet_gateway_runtime_config() {
        let config = vmnet_gateway_config_from_args(&[
            "--socket".to_string(),
            "/tmp/vmnet.sock".to_string(),
            "--guest-ip".to_string(),
            "10.0.2.20".to_string(),
            "--allow-ip".to_string(),
            "93.184.216.34".to_string(),
            "--allow-domain".to_string(),
            "example.com".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
            "--publish".to_string(),
            "18080:8080".to_string(),
            "--tls-ca-cert".to_string(),
            "/tmp/ca.pem".to_string(),
            "--tls-ca-key".to_string(),
            "/tmp/ca-key.pem".to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ])
        .expect("config");

        assert_eq!(config.socket_path, PathBuf::from("/tmp/vmnet.sock"));
        assert_eq!(config.network.guest_ip, "10.0.2.20");
        assert_eq!(config.policy.egress.allow_ips, vec!["93.184.216.34"]);
        assert_eq!(config.policy.egress.allow_domains, vec!["example.com"]);
        assert_eq!(
            config.policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
                HostListener::published_tcp(18080, 8080),
            ]
        );
        assert_eq!(
            config.policy.tls_mitm.ca_cert_path,
            Some(PathBuf::from("/tmp/ca.pem"))
        );
        assert_eq!(
            config.policy.tls_mitm.ca_key_path,
            Some(PathBuf::from("/tmp/ca-key.pem"))
        );
        assert!(config.policy.tls_mitm.generate_per_host_certs);
    }

    #[test]
    fn vmnet_gateway_runtime_config_requires_socket() {
        assert_eq!(
            vmnet_gateway_config_from_args(&[]).expect_err("missing socket"),
            "--socket is required"
        );
    }

    #[test]
    fn parses_frontend_prepare_defaults_and_policy() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::create_dir_all(root.join("repo")).expect("repo");
        std::fs::write(root.join("docker/out/vmlinuz"), b"kernel").expect("kernel");
        std::fs::write(root.join("docker/out/initrd.img"), b"initrd").expect("initrd");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        std::fs::write(
            root.join("docker/out/artifact-manifest.json"),
            r#"{
              "schema_version": 1,
              "artifacts": {
                "kernel": "docker/out/vmlinuz",
                "initrd": "docker/out/initrd.img",
                "rootfs": "docker/out/rootfs.raw"
              },
              "vm": {
                "cpus": 2,
                "memory_bytes": 2147483648,
                "virtiofs_tag": "agentvm",
                "kernel_cmdline": "console=hvc0 root=/dev/vda"
              }
            }"#,
        )
        .expect("manifest");

        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--allow-public-internet".to_string(),
            "--guest-http-smoke-url".to_string(),
            "http://93.184.216.34/".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
            "--publish".to_string(),
            "18080:8080".to_string(),
            "--tls-ca-cert".to_string(),
            "/tmp/ca.pem".to_string(),
            "--tls-ca-key".to_string(),
            "/tmp/ca-key.pem".to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ])
        .expect("config");

        assert_eq!(config.vm.cpus, 2);
        assert_eq!(
            config.guest_http_smoke_url.as_deref(),
            Some("http://93.184.216.34/")
        );
        assert!(policy.allow_public);
        assert_eq!(
            policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
                HostListener::published_tcp(18080, 8080),
            ]
        );
        let runtime_policy = policy_from_args(config.network.clone(), policy);
        assert_eq!(
            runtime_policy.tls_mitm.ca_cert_path,
            Some(PathBuf::from("/tmp/ca.pem"))
        );
        assert_eq!(
            runtime_policy.tls_mitm.ca_key_path,
            Some(PathBuf::from("/tmp/ca-key.pem"))
        );
        assert!(runtime_policy.tls_mitm.generate_per_host_certs);
    }

    #[test]
    fn frontend_no_net_sets_policy_reason_and_allows_control_listeners() {
        let root = frontend_test_root();

        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--no-net".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
        ])
        .expect("config");

        let runtime_policy = policy_from_args(config.network.clone(), policy);
        assert_eq!(runtime_policy.egress.default_action, EgressAction::Deny);
        assert_eq!(runtime_policy.egress.reason, EgressReason::NoNetFlag);
        assert_eq!(
            runtime_policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
            ]
        );
    }

    #[test]
    fn frontend_no_net_rejects_published_ports_and_allow_rules() {
        let root = frontend_test_root();
        let base_args = [
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--no-net".to_string(),
        ];

        let mut publish_args = base_args.to_vec();
        publish_args.extend(["--publish".to_string(), "18080:8080".to_string()]);
        assert_eq!(
            frontend_config_from_args(&publish_args).expect_err("publish rejected"),
            "--no-net cannot be combined with --publish"
        );

        let mut allow_args = base_args.to_vec();
        allow_args.extend(["--allow-public-internet".to_string()]);
        assert_eq!(
            frontend_config_from_args(&allow_args).expect_err("allow rejected"),
            "--no-net cannot be combined with egress allow options"
        );
    }

    #[test]
    fn guest_http_smoke_requires_fresh_artifact() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::write(root.join("docker/out/artifact-manifest.json"), b"{}").expect("manifest");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(root.join("docker/guest-init.sh"), b"#!/bin/sh\n").expect("guest init");

        let error =
            ensure_smoke_hook_artifact_fresh(&root.join("docker/out/artifact-manifest.json"))
                .expect_err("stale artifact");

        assert!(error.contains("requires rebuilt appliance artifacts"));
    }

    #[test]
    fn parses_ip_port() {
        assert_eq!(
            parse_ip_port("198.51.100.10:80").expect("ip port"),
            (Ipv4Addr::new(198, 51, 100, 10), 80)
        );
        assert!(parse_ip_port("198.51.100.10").is_err());
    }

    #[test]
    fn parses_port_pair() {
        assert_eq!(
            parse_port_pair("18080:8080").expect("port pair"),
            (18080, 8080)
        );
        assert!(parse_port_pair("18080").is_err());
        assert!(parse_port_pair("localhost:8080").is_err());
    }

    fn unique_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentvm-frontend-main-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn frontend_test_root() -> PathBuf {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::create_dir_all(root.join("repo")).expect("repo");
        std::fs::write(root.join("docker/out/vmlinuz"), b"kernel").expect("kernel");
        std::fs::write(root.join("docker/out/initrd.img"), b"initrd").expect("initrd");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        std::fs::write(
            root.join("docker/out/artifact-manifest.json"),
            r#"{
              "schema_version": 1,
              "artifacts": {
                "kernel": "docker/out/vmlinuz",
                "initrd": "docker/out/initrd.img",
                "rootfs": "docker/out/rootfs.raw"
              },
              "vm": {
                "cpus": 2,
                "memory_bytes": 2147483648,
                "virtiofs_tag": "agentvm",
                "kernel_cmdline": "console=hvc0 root=/dev/vda"
              }
            }"#,
        )
        .expect("manifest");
        root
    }
}
