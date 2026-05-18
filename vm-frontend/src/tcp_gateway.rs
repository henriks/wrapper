use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use crate::dns_proxy::domain_allowed;
use crate::network_policy::{EgressAction, VmnetPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpDestination {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub domain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpDecision {
    pub action: TcpAction,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpAction {
    Deny,
    Connect,
    InterceptHttp,
    InterceptHttps,
}

pub trait TcpUpstreamConnector {
    type Connection;

    fn connect(&self, destination: &TcpDestination) -> Result<Self::Connection, TcpConnectError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdTcpConnector {
    pub timeout: Duration,
}

impl TcpUpstreamConnector for StdTcpConnector {
    type Connection = TcpStream;

    fn connect(&self, destination: &TcpDestination) -> Result<Self::Connection, TcpConnectError> {
        let addr = SocketAddrV4::new(destination.ip, destination.port);
        connect_socket_addr(addr, self.timeout)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamMapping {
    pub guest_ip: Ipv4Addr,
    pub guest_port: u16,
    pub host_ip: Ipv4Addr,
    pub host_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedTcpConnector {
    pub base: StdTcpConnector,
    pub mappings: Vec<UpstreamMapping>,
}

impl TcpUpstreamConnector for MappedTcpConnector {
    type Connection = TcpStream;

    fn connect(&self, destination: &TcpDestination) -> Result<Self::Connection, TcpConnectError> {
        if let Some(mapping) = self.mappings.iter().find(|mapping| {
            mapping.guest_ip == destination.ip && mapping.guest_port == destination.port
        }) {
            return connect_socket_addr(
                SocketAddrV4::new(mapping.host_ip, mapping.host_port),
                self.base.timeout,
            );
        }
        self.base.connect(destination)
    }
}

fn connect_socket_addr(
    addr: SocketAddrV4,
    timeout: Duration,
) -> Result<TcpStream, TcpConnectError> {
    let stream = TcpStream::connect_timeout(&addr.into(), timeout)
        .map_err(|_| TcpConnectError::UpstreamUnavailable)?;
    stream
        .set_nonblocking(true)
        .map_err(|_| TcpConnectError::UpstreamUnavailable)?;
    stream
        .set_nodelay(true)
        .map_err(|_| TcpConnectError::UpstreamUnavailable)?;
    Ok(stream)
}

pub fn connect_if_allowed<C>(
    policy: &VmnetPolicy,
    destination: &TcpDestination,
    connector: &C,
) -> Result<(TcpDecision, C::Connection), TcpConnectError>
where
    C: TcpUpstreamConnector,
{
    let decision = evaluate_tcp_destination(policy, destination);
    if decision.action == TcpAction::Deny {
        return Err(TcpConnectError::PolicyDenied(decision));
    }
    let connection = connector.connect(destination)?;
    Ok((decision, connection))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpConnectError {
    PolicyDenied(TcpDecision),
    UpstreamUnavailable,
}

pub fn evaluate_tcp_destination(policy: &VmnetPolicy, destination: &TcpDestination) -> TcpDecision {
    if ip_in_ranges(destination.ip, &policy.egress.deny_ip_ranges) {
        return TcpDecision {
            action: TcpAction::Deny,
            reason: "destination is in a denied range".to_string(),
        };
    }

    if !tcp_destination_allowed(policy, destination) {
        return TcpDecision {
            action: TcpAction::Deny,
            reason: "destination denied by egress policy".to_string(),
        };
    }

    if destination.port == 80 && policy.protocols.tcp.intercept_http_port_80 {
        return TcpDecision {
            action: TcpAction::InterceptHttp,
            reason: "HTTP interception enabled".to_string(),
        };
    }

    if destination.port == 443 && policy.protocols.tcp.intercept_https_port_443 {
        if !https_mitm_configured(policy) {
            return TcpDecision {
                action: TcpAction::Deny,
                reason:
                    "HTTPS MITM requires configured CA cert/key and per-host certificate generation"
                        .to_string(),
            };
        }
        return TcpDecision {
            action: TcpAction::InterceptHttps,
            reason: "HTTPS interception enabled".to_string(),
        };
    }

    TcpDecision {
        action: TcpAction::Connect,
        reason: "generic TCP connect allowed".to_string(),
    }
}

fn https_mitm_configured(policy: &VmnetPolicy) -> bool {
    policy.tls_mitm.ca_cert_path.is_some()
        && policy.tls_mitm.ca_key_path.is_some()
        && policy.tls_mitm.generate_per_host_certs
}

pub fn parse_http_request(buffer: &[u8]) -> Result<Option<HttpRequestSummary>, HttpParseError> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(buffer) {
        Ok(httparse::Status::Partial) => Ok(None),
        Ok(httparse::Status::Complete(_len)) => {
            let method = request.method.ok_or(HttpParseError::Malformed)?.to_string();
            let path = request.path.ok_or(HttpParseError::Malformed)?.to_string();
            let host = request
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("host"))
                .map(|header| String::from_utf8_lossy(header.value).trim().to_string());
            Ok(Some(HttpRequestSummary { method, path, host }))
        }
        Err(_) => Err(HttpParseError::Malformed),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequestSummary {
    pub method: String,
    pub path: String,
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpParseError {
    Malformed,
}

fn tcp_destination_allowed(policy: &VmnetPolicy, destination: &TcpDestination) -> bool {
    if matches!(
        policy.egress.default_action,
        EgressAction::AllowPublicInternet
    ) {
        return true;
    }
    if destination
        .domain
        .as_deref()
        .is_some_and(|domain| domain_allowed(policy, domain))
    {
        return true;
    }
    ip_in_ranges(destination.ip, &policy.egress.allow_ip_ranges)
}

fn ip_in_ranges(ip: Ipv4Addr, ranges: &[crate::network_policy::Ipv4Range]) -> bool {
    ranges.iter().any(|range| range.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmnet_gateway::{GuestFrameOutcome, VmnetGateway};
    use crate::{network_policy::VmnetPolicy, test_support, GuestNetwork};
    use proptest::prelude::*;
    use smoltcp::wire::Ipv4Address as SmolIpv4Address;
    use std::io::{ErrorKind, Read};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;

    struct FakeConnector;

    impl TcpUpstreamConnector for FakeConnector {
        type Connection = String;

        fn connect(
            &self,
            destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Ok(format!("{}:{}", destination.ip, destination.port))
        }
    }

    fn policy() -> VmnetPolicy {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy.egress.allow_domains.push("example.com".to_string());
        policy
            .egress
            .allow_ip_or_cidr("93.184.216.34")
            .expect("test allow ip");
        policy
    }

    #[test]
    fn blocks_private_ranges_before_allow_rules() {
        let mut policy = policy();
        policy
            .egress
            .allow_ip_or_cidr("10.1.2.3")
            .expect("test allow ip");
        let decision = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(10, 1, 2, 3),
                port: 80,
                domain: Some("example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::Deny);
    }

    #[test]
    fn intercepts_allowed_http_destination() {
        let decision = evaluate_tcp_destination(
            &policy(),
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 34),
                port: 80,
                domain: Some("example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::InterceptHttp);
    }

    #[test]
    fn intercepts_allowed_https_destination() {
        let mut policy = policy();
        policy.tls_mitm.ca_cert_path = Some("/tmp/ca.pem".into());
        policy.tls_mitm.ca_key_path = Some("/tmp/ca-key.pem".into());
        policy.tls_mitm.generate_per_host_certs = true;

        let decision = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 34),
                port: 443,
                domain: Some("example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::InterceptHttps);
    }

    #[test]
    fn https_interception_fails_closed_without_ca_material() {
        let decision = evaluate_tcp_destination(
            &policy(),
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 34),
                port: 443,
                domain: Some("example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::Deny);
        assert!(decision
            .reason
            .contains("HTTPS MITM requires configured CA"));
    }

    #[test]
    fn denies_unallowed_tcp_destination_by_default() {
        let decision = evaluate_tcp_destination(
            &policy(),
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 35),
                port: 80,
                domain: Some("blocked.example".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::Deny);
    }

    #[test]
    fn allow_domain_and_wildcard_domain_enable_tcp_without_ip_allow() {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy
            .egress
            .allow_domains
            .push("*.example.com".to_string());

        let decision = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 35),
                port: 8080,
                domain: Some("api.example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::Connect);
        assert_eq!(decision.reason, "generic TCP connect allowed");
    }

    #[test]
    fn public_egress_profile_allows_public_tcp_but_still_intercepts_http() {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy.egress.default_action = EgressAction::AllowPublicInternet;

        let generic = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 35),
                port: 8080,
                domain: None,
            },
        );
        let http = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 35),
                port: 80,
                domain: None,
            },
        );

        assert_eq!(generic.action, TcpAction::Connect);
        assert_eq!(http.action, TcpAction::InterceptHttp);
    }

    #[test]
    fn denied_range_wins_over_public_egress_profile() {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy.egress.default_action = EgressAction::AllowPublicInternet;

        let decision = evaluate_tcp_destination(
            &policy,
            &TcpDestination {
                ip: Ipv4Addr::new(169, 254, 169, 254),
                port: 80,
                domain: None,
            },
        );

        assert_eq!(decision.action, TcpAction::Deny);
        assert_eq!(decision.reason, "destination is in a denied range");
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 128,
            max_shrink_iters: 2048,
            ..ProptestConfig::default()
        })]

        #[test]
        fn proptest_tcp_policy_matches_gateway_syn_outcome(
            octets in any::<[u8; 4]>(),
            port in 1_u16..=u16::MAX,
            default_public in any::<bool>(),
            explicit_allow in any::<bool>(),
            explicit_deny in any::<bool>(),
            mitm_configured in any::<bool>(),
        ) {
            let network = GuestNetwork::default();
            let ip = Ipv4Addr::from(octets);
            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            if default_public {
                policy.egress.default_action = EgressAction::AllowPublicInternet;
            }
            if explicit_allow {
                policy
                    .egress
                    .allow_ip_or_cidr(&ip.to_string())
                    .expect("test allow ip");
            }
            if explicit_deny {
                policy.egress.deny_ip_ranges.push(
                    crate::network_policy::Ipv4Range::parse(&format!("{ip}/32"))
                        .expect("test deny ip"),
                );
            }
            if mitm_configured {
                policy.tls_mitm.ca_cert_path = Some(PathBuf::from("/tmp/generated-test-ca.pem"));
                policy.tls_mitm.ca_key_path = Some(PathBuf::from("/tmp/generated-test-ca-key.pem"));
                policy.tls_mitm.generate_per_host_certs = true;
            }

            let destination = TcpDestination {
                ip,
                port,
                domain: None,
            };
            let decision = evaluate_tcp_destination(&policy, &destination);
            let mut gateway = VmnetGateway::new(&policy, &network, smoltcp::time::Instant::from_millis(0))
                .expect("gateway");
            let result = gateway.handle_guest_frame(
                test_support::tcp_syn_frame(SmolIpv4Address::from_octets(octets), port),
                smoltcp::time::Instant::from_millis(1),
            );

            match decision.action {
                TcpAction::Deny => {
                    if !matches!(result.outcome, GuestFrameOutcome::TcpDenied { .. }) {
                        return Err(TestCaseError::fail(format!(
                            "pure TCP deny did not match gateway outcome: {:?}",
                            result.outcome
                        )));
                    }
                    prop_assert!(gateway.active_tcp_sessions().is_empty());
                }
                TcpAction::Connect | TcpAction::InterceptHttp | TcpAction::InterceptHttps => {
                    if !matches!(result.outcome, GuestFrameOutcome::TcpAccepted { .. }) {
                        return Err(TestCaseError::fail(format!(
                            "pure TCP allow did not match gateway outcome: {:?}",
                            result.outcome
                        )));
                    }
                    prop_assert_eq!(gateway.active_tcp_sessions().len(), 1);
                }
            }
        }

        #[test]
        fn proptest_deny_range_precedence_over_every_allow_path(
            octets in any::<[u8; 4]>(),
            port in 1_u16..=u16::MAX,
            domain_label in 0_u16..=999,
        ) {
            let ip = Ipv4Addr::from(octets);
            let domain = format!("host-{domain_label}.example");
            let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
            policy.egress.default_action = EgressAction::AllowPublicInternet;
            policy
                .egress
                .allow_ip_or_cidr(&ip.to_string())
                .expect("test allow ip");
            policy.egress.allow_domains.push(domain.clone());
            policy.egress.deny_ip_ranges.push(
                crate::network_policy::Ipv4Range::parse(&format!("{ip}/32"))
                    .expect("test deny ip"),
            );
            policy.tls_mitm.ca_cert_path = Some(PathBuf::from("/tmp/generated-test-ca.pem"));
            policy.tls_mitm.ca_key_path = Some(PathBuf::from("/tmp/generated-test-ca-key.pem"));
            policy.tls_mitm.generate_per_host_certs = true;

            let decision = evaluate_tcp_destination(
                &policy,
                &TcpDestination {
                    ip,
                    port,
                    domain: Some(domain),
                },
            );

            prop_assert_eq!(decision.action, TcpAction::Deny);
            prop_assert_eq!(decision.reason, "destination is in a denied range");
        }

        #[test]
        fn proptest_https_requires_complete_mitm_config(
            octets in any::<[u8; 4]>(),
            has_cert in any::<bool>(),
            has_key in any::<bool>(),
            generate_certs in any::<bool>(),
        ) {
            let ip = Ipv4Addr::from(octets);
            let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
            policy.egress.default_action = EgressAction::AllowPublicInternet;
            if has_cert {
                policy.tls_mitm.ca_cert_path = Some(PathBuf::from("/tmp/generated-test-ca.pem"));
            }
            if has_key {
                policy.tls_mitm.ca_key_path = Some(PathBuf::from("/tmp/generated-test-ca-key.pem"));
            }
            policy.tls_mitm.generate_per_host_certs = generate_certs;

            let decision = evaluate_tcp_destination(
                &policy,
                &TcpDestination {
                    ip,
                    port: 443,
                    domain: None,
                },
            );

            if has_cert && has_key && generate_certs && !ip_in_ranges(ip, &policy.egress.deny_ip_ranges) {
                prop_assert_eq!(decision.action, TcpAction::InterceptHttps);
            } else {
                prop_assert_eq!(decision.action, TcpAction::Deny);
            }
        }
    }

    #[test]
    fn parses_complete_http_request_summary() {
        let request = b"GET /status?q=1 HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n";
        let summary = parse_http_request(request)
            .expect("parse")
            .expect("complete");

        assert_eq!(summary.method, "GET");
        assert_eq!(summary.path, "/status?q=1");
        assert_eq!(summary.host.as_deref(), Some("example.com"));
    }

    #[test]
    fn reports_incomplete_http_request() {
        assert_eq!(
            parse_http_request(b"GET / HTTP/1.1\r\n").expect("parse"),
            None
        );
    }

    #[test]
    fn rejects_malformed_http_request() {
        assert_eq!(
            parse_http_request(b"\0\0\0").expect_err("malformed"),
            HttpParseError::Malformed
        );
    }

    #[test]
    fn connect_if_allowed_opens_upstream_after_policy_allows() {
        let destination = TcpDestination {
            ip: Ipv4Addr::new(93, 184, 216, 34),
            port: 80,
            domain: Some("example.com".to_string()),
        };

        let (decision, connection) =
            connect_if_allowed(&policy(), &destination, &FakeConnector).expect("connect");

        assert_eq!(decision.action, TcpAction::InterceptHttp);
        assert_eq!(connection, "93.184.216.34:80");
    }

    #[test]
    fn connect_if_allowed_fails_closed_before_connector_for_denied_destination() {
        let destination = TcpDestination {
            ip: Ipv4Addr::new(93, 184, 216, 35),
            port: 80,
            domain: Some("blocked.example".to_string()),
        };

        let error = connect_if_allowed(&policy(), &destination, &FakeConnector)
            .expect_err("denied destination");

        assert!(matches!(
            error,
            TcpConnectError::PolicyDenied(TcpDecision {
                action: TcpAction::Deny,
                ..
            })
        ));
    }

    #[test]
    #[ignore = "sandbox blocks loopback TCP bind; run explicitly when validating StdTcpConnector"]
    fn std_connector_returns_nonblocking_tcp_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let accept_thread = thread::spawn(move || {
            let (_stream, _addr) = listener.accept().expect("accept");
            accepted_tx.send(()).expect("accepted signal");
        });

        let connector = StdTcpConnector {
            timeout: Duration::from_secs(1),
        };
        let mut stream = connector
            .connect(&TcpDestination {
                ip: match addr.ip() {
                    std::net::IpAddr::V4(ip) => ip,
                    std::net::IpAddr::V6(_) => unreachable!("bound IPv4"),
                },
                port: addr.port(),
                domain: None,
            })
            .expect("connect");
        accepted_rx.recv().expect("accepted");

        let mut buf = [0; 1];
        let error = stream.read(&mut buf).expect_err("nonblocking read");
        assert_eq!(error.kind(), ErrorKind::WouldBlock);

        accept_thread.join().expect("accept thread");
    }
}
