use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use ipnet::Ipv4Net;

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
        TcpStream::connect_timeout(&addr.into(), self.timeout)
            .map_err(|_| TcpConnectError::UpstreamUnavailable)
    }
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
    if ip_in_ranges(destination.ip, &policy.egress.deny_ranges) {
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
    ip_in_ranges(destination.ip, &policy.egress.allow_ips)
}

fn ip_in_ranges(ip: Ipv4Addr, ranges: &[String]) -> bool {
    ranges.iter().any(|range| {
        if let Ok(net) = range.parse::<Ipv4Net>() {
            return net.contains(&ip);
        }
        range.parse::<Ipv4Addr>().is_ok_and(|single| single == ip)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{network_policy::VmnetPolicy, GuestNetwork};

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
        policy.egress.allow_ips.push("93.184.216.34".to_string());
        policy
    }

    #[test]
    fn blocks_private_ranges_before_allow_rules() {
        let mut policy = policy();
        policy.egress.allow_ips.push("10.1.2.3".to_string());
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
        let decision = evaluate_tcp_destination(
            &policy(),
            &TcpDestination {
                ip: Ipv4Addr::new(93, 184, 216, 34),
                port: 443,
                domain: Some("example.com".to_string()),
            },
        );

        assert_eq!(decision.action, TcpAction::InterceptHttps);
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
}
