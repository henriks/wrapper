use std::path::PathBuf;

use crate::GuestNetwork;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetPolicy {
    pub assignment: GuestNetwork,
    pub mtu: u16,
    pub egress: EgressPolicy,
    pub protocols: ProtocolPolicy,
    pub host_listeners: Vec<HostListener>,
    pub capture: CapturePolicy,
    pub tls_mitm: TlsMitmPolicy,
}

impl VmnetPolicy {
    pub fn default_sandbox(assignment: GuestNetwork) -> Self {
        Self {
            assignment,
            mtu: 1500,
            egress: EgressPolicy::default(),
            protocols: ProtocolPolicy::default(),
            host_listeners: Vec::new(),
            capture: CapturePolicy::default(),
            tls_mitm: TlsMitmPolicy::default(),
        }
    }

    pub fn from_cli(
        assignment: GuestNetwork,
        no_net: bool,
        published_ports: Vec<PublishedPort>,
    ) -> Result<Self, PolicyError> {
        if no_net && !published_ports.is_empty() {
            return Err(PolicyError::PublishedPortsRequireGuestEgress);
        }

        let mut policy = Self::default_sandbox(assignment);
        if no_net {
            policy.egress.default_action = EgressAction::Deny;
            policy.egress.reason = EgressReason::NoNetFlag;
        }
        policy.host_listeners.extend(
            published_ports
                .into_iter()
                .map(|port| HostListener::published_tcp(port.host_port, port.guest_port)),
        );
        Ok(policy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressPolicy {
    pub default_action: EgressAction,
    pub reason: EgressReason,
    pub allow_domains: Vec<String>,
    pub allow_ips: Vec<String>,
    pub deny_ranges: Vec<String>,
}

impl Default for EgressPolicy {
    fn default() -> Self {
        Self {
            default_action: EgressAction::Deny,
            reason: EgressReason::DenyByDefault,
            allow_domains: Vec::new(),
            allow_ips: Vec::new(),
            deny_ranges: default_deny_ranges(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressAction {
    Deny,
    AllowPublicInternet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressReason {
    DenyByDefault,
    NoNetFlag,
    ExplicitAllowProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolPolicy {
    pub ethernet: EthernetPolicy,
    pub ipv4: Ipv4Policy,
    pub ipv6: UnsupportedPolicy,
    pub udp: UdpPolicy,
    pub tcp: TcpPolicy,
}

impl Default for ProtocolPolicy {
    fn default() -> Self {
        Self {
            ethernet: EthernetPolicy {
                allow_arp: true,
                unknown_ethertypes: UnsupportedPolicy::DenyAndLog,
            },
            ipv4: Ipv4Policy {
                allow_icmp_to_gateway: true,
                unknown_protocols: UnsupportedPolicy::DenyAndLog,
            },
            ipv6: UnsupportedPolicy::DenyAndLog,
            udp: UdpPolicy {
                allow_dns_to_gateway: true,
                block_udp_443: true,
                default_action: EgressAction::Deny,
            },
            tcp: TcpPolicy {
                default_action: EgressAction::Deny,
                intercept_http_port_80: true,
                intercept_https_port_443: true,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthernetPolicy {
    pub allow_arp: bool,
    pub unknown_ethertypes: UnsupportedPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4Policy {
    pub allow_icmp_to_gateway: bool,
    pub unknown_protocols: UnsupportedPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedPolicy {
    DenyAndLog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpPolicy {
    pub allow_dns_to_gateway: bool,
    pub block_udp_443: bool,
    pub default_action: EgressAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpPolicy {
    pub default_action: EgressAction,
    pub intercept_http_port_80: bool,
    pub intercept_https_port_443: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostListener {
    pub host_addr: String,
    pub host_port: u16,
    pub guest_port: u16,
    pub purpose: HostListenerPurpose,
}

impl HostListener {
    pub fn docker_api(host_port: u16, guest_port: u16) -> Self {
        Self::loopback(host_port, guest_port, HostListenerPurpose::DockerApi)
    }

    pub fn payload_control(host_port: u16, guest_port: u16) -> Self {
        Self::loopback(host_port, guest_port, HostListenerPurpose::PayloadControl)
    }

    pub fn published_tcp(host_port: u16, guest_port: u16) -> Self {
        Self::loopback(host_port, guest_port, HostListenerPurpose::PublishedTcp)
    }

    fn loopback(host_port: u16, guest_port: u16, purpose: HostListenerPurpose) -> Self {
        Self {
            host_addr: "127.0.0.1".to_string(),
            host_port,
            guest_port,
            purpose,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostListenerPurpose {
    DockerApi,
    PayloadControl,
    PublishedTcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPort {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapturePolicy {
    pub pcap_path: Option<PathBuf>,
    pub capture_guest_side_frames: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TlsMitmPolicy {
    pub ca_cert_path: Option<PathBuf>,
    pub ca_key_path: Option<PathBuf>,
    pub generate_per_host_certs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    PublishedPortsRequireGuestEgress,
}

fn default_deny_ranges() -> Vec<String> {
    [
        "0.0.0.0/8",
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "169.254.169.254/32",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "224.0.0.0/4",
        "240.0.0.0/4",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_deny_by_default_and_blocks_bypass_protocols() {
        let policy = VmnetPolicy::default_sandbox(GuestNetwork::default());

        assert_eq!(policy.egress.default_action, EgressAction::Deny);
        assert_eq!(policy.protocols.ipv6, UnsupportedPolicy::DenyAndLog);
        assert!(policy.protocols.udp.block_udp_443);
        assert_eq!(policy.protocols.udp.default_action, EgressAction::Deny);
        assert!(policy
            .egress
            .deny_ranges
            .contains(&"169.254.169.254/32".to_string()));
    }

    #[test]
    fn no_net_rejects_published_ports() {
        let error = VmnetPolicy::from_cli(
            GuestNetwork::default(),
            true,
            vec![PublishedPort {
                host_port: 8080,
                guest_port: 80,
            }],
        )
        .expect_err("published port with no-net should fail");

        assert_eq!(error, PolicyError::PublishedPortsRequireGuestEgress);
    }

    #[test]
    fn published_ports_become_frontend_host_listeners() {
        let policy = VmnetPolicy::from_cli(
            GuestNetwork::default(),
            false,
            vec![PublishedPort {
                host_port: 8080,
                guest_port: 80,
            }],
        )
        .expect("policy");

        assert_eq!(
            policy.host_listeners,
            vec![HostListener::published_tcp(8080, 80)]
        );
    }
}
