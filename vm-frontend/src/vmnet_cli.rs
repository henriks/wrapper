use std::path::PathBuf;

use agentvm_frontend::network_policy::{EgressAction, EgressReason, HostListener, VmnetPolicy};
use agentvm_frontend::vmnet_runtime::VmnetRuntimeConfig;
use agentvm_frontend::GuestNetwork;
use clap::{Arg, ArgAction, Command as ClapCommand};

use super::{append_many, parse_clap_matches, validate_no_net_args, PortPair};

pub(crate) type VmnetCliResult<T> = Result<T, VmnetCliError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum VmnetCliError {
    #[error("{message}")]
    Clap { message: String },
    #[error("--socket is required")]
    MissingSocket,
    #[error("{message}")]
    InvalidNoNetPolicy { message: String },
}

impl From<VmnetCliError> for String {
    fn from(error: VmnetCliError) -> Self {
        error.to_string()
    }
}

pub(crate) fn vmnet_gateway_config_from_args(
    args: &[String],
) -> VmnetCliResult<VmnetRuntimeConfig> {
    let matches = parse_clap_matches(vmnet_gateway_clap_command(), args)
        .map_err(|message| VmnetCliError::Clap { message })?;
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .cloned()
        .ok_or(VmnetCliError::MissingSocket)?;
    let mut network = GuestNetwork::default();
    if let Some(value) = matches.get_one::<String>("guest_ip") {
        network.guest_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("gateway_ip") {
        network.gateway_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("dns_ip") {
        network.dns_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("guest_mac") {
        network.guest_mac = value.clone();
    }
    if let Some(value) = matches.get_one::<u8>("prefix_len") {
        network.prefix_len = *value;
    }
    let allow_ips = append_many(&matches, "allow_ip");
    let allow_domains = append_many(&matches, "allow_domain");
    let allow_public = matches.get_flag("allow_public_internet");
    let no_net = matches.get_flag("no_net");
    let mut host_listeners = Vec::new();
    if let Some(values) = matches.get_many::<PortPair>("host_docker_listener") {
        for value in values {
            host_listeners.push(HostListener::docker_api(value.host, value.guest));
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("host_payload_listener") {
        for value in values {
            host_listeners.push(HostListener::payload_control(value.host, value.guest));
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("publish") {
        for value in values {
            host_listeners.push(HostListener::published_tcp(value.host, value.guest));
        }
    }
    let pcap_path = matches.get_one::<String>("pcap").map(PathBuf::from);
    let tls_ca_cert = matches.get_one::<String>("tls_ca_cert").map(PathBuf::from);
    let tls_ca_key = matches.get_one::<String>("tls_ca_key").map(PathBuf::from);
    let tls_generate_per_host_certs = matches.get_flag("tls_generate_per_host_certs");
    validate_no_net_args(
        no_net,
        allow_public,
        &allow_ips,
        &allow_domains,
        &host_listeners,
    )
    .map_err(|error| VmnetCliError::InvalidNoNetPolicy {
        message: error.to_string(),
    })?;
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

fn vmnet_gateway_clap_command() -> ClapCommand {
    ClapCommand::new("vmnet-gateway")
        .arg(
            Arg::new("socket")
                .long("socket")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(Arg::new("guest_ip").long("guest-ip").value_name("IP"))
        .arg(Arg::new("gateway_ip").long("gateway-ip").value_name("IP"))
        .arg(Arg::new("dns_ip").long("dns-ip").value_name("IP"))
        .arg(Arg::new("guest_mac").long("guest-mac").value_name("MAC"))
        .arg(
            Arg::new("prefix_len")
                .long("prefix-len")
                .value_name("N")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("allow_ip")
                .long("allow-ip")
                .value_name("IP_OR_CIDR")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_domain")
                .long("allow-domain")
                .value_name("DOMAIN")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_public_internet")
                .long("allow-public-internet")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("host_docker_listener")
                .long("host-docker-listener")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("host_payload_listener")
                .long("host-payload-listener")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("publish")
                .long("publish")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(Arg::new("pcap").long("pcap").value_name("PATH"))
        .arg(
            Arg::new("tls_ca_cert")
                .long("tls-ca-cert")
                .value_name("PATH"),
        )
        .arg(Arg::new("tls_ca_key").long("tls-ca-key").value_name("PATH"))
        .arg(
            Arg::new("tls_generate_per_host_certs")
                .long("tls-generate-per-host-certs")
                .action(ArgAction::SetTrue),
        )
}
