use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use hickory_proto::op::{Message, Metadata, ResponseCode};

use crate::network_policy::{EgressAction, VmnetPolicy};

pub const DNS_PORT: u16 = 53;

#[derive(Debug)]
pub struct DnsProxy<'a, U> {
    policy: &'a VmnetPolicy,
    upstream: U,
}

impl<'a, U> DnsProxy<'a, U>
where
    U: DnsUpstream,
{
    pub fn new(policy: &'a VmnetPolicy, upstream: U) -> Self {
        Self { policy, upstream }
    }

    pub fn handle_udp_payload(&self, payload: &[u8]) -> DnsProxyResult {
        let query = match Message::from_vec(payload) {
            Ok(query) => query,
            Err(error) => {
                return DnsProxyResult {
                    response: None,
                    log: DnsLogEntry {
                        domain: None,
                        decision: DnsDecision::Malformed,
                        detail: error.to_string(),
                    },
                };
            }
        };

        let Some(domain) = query_domain(&query) else {
            return DnsProxyResult {
                response: serialize_response(error_response(&query, ResponseCode::FormErr)),
                log: DnsLogEntry {
                    domain: None,
                    decision: DnsDecision::Malformed,
                    detail: "expected exactly one DNS question".to_string(),
                },
            };
        };

        if !domain_allowed(self.policy, &domain) {
            return DnsProxyResult {
                response: serialize_response(error_response(&query, ResponseCode::Refused)),
                log: DnsLogEntry {
                    domain: Some(domain),
                    decision: DnsDecision::Blocked,
                    detail: "domain denied by VmnetPolicy".to_string(),
                },
            };
        }

        match self.upstream.exchange(&query) {
            Ok(response) => DnsProxyResult {
                response: serialize_response(response),
                log: DnsLogEntry {
                    domain: Some(domain),
                    decision: DnsDecision::Allowed,
                    detail: "forwarded to upstream".to_string(),
                },
            },
            Err(error) => DnsProxyResult {
                response: serialize_response(error_response(&query, ResponseCode::ServFail)),
                log: DnsLogEntry {
                    domain: Some(domain),
                    decision: DnsDecision::UpstreamFailure,
                    detail: format!("{error:?}"),
                },
            },
        }
    }
}

pub trait DnsUpstream {
    fn exchange(&self, query: &Message) -> Result<Message, DnsUpstreamError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpDnsUpstream {
    pub server: SocketAddr,
    pub timeout: Duration,
}

impl DnsUpstream for UdpDnsUpstream {
    fn exchange(&self, query: &Message) -> Result<Message, DnsUpstreamError> {
        let wire = query.to_vec().map_err(|_| DnsUpstreamError::InvalidQuery)?;
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|_| DnsUpstreamError::Unavailable)?;
        socket
            .set_read_timeout(Some(self.timeout))
            .map_err(|_| DnsUpstreamError::Unavailable)?;
        socket
            .set_write_timeout(Some(self.timeout))
            .map_err(|_| DnsUpstreamError::Unavailable)?;
        socket
            .send_to(&wire, self.server)
            .map_err(|_| DnsUpstreamError::Unavailable)?;
        let mut response = vec![0; 4096];
        let (len, _addr) = socket
            .recv_from(&mut response)
            .map_err(|_| DnsUpstreamError::Unavailable)?;
        response.truncate(len);
        Message::from_vec(&response).map_err(|_| DnsUpstreamError::InvalidResponse)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsProxyResult {
    pub response: Option<Vec<u8>>,
    pub log: DnsLogEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsLogEntry {
    pub domain: Option<String>,
    pub decision: DnsDecision,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsDecision {
    Allowed,
    Blocked,
    Malformed,
    UpstreamFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsUpstreamError {
    InvalidQuery,
    Unavailable,
    InvalidResponse,
}

pub fn dns_allowed_to_destination(policy: &VmnetPolicy, dst_ip: [u8; 4], dst_port: u16) -> bool {
    policy.protocols.udp.allow_dns_to_gateway
        && dst_port == DNS_PORT
        && parse_ipv4(&policy.assignment.dns_ip).is_some_and(|dns_ip| dns_ip == dst_ip)
}

pub(crate) fn domain_allowed(policy: &VmnetPolicy, domain: &str) -> bool {
    if matches!(
        policy.egress.default_action,
        EgressAction::AllowPublicInternet
    ) {
        return true;
    }
    policy
        .egress
        .allow_domains
        .iter()
        .any(|allowed| domain_matches(allowed, domain))
}

fn domain_matches(allowed: &str, domain: &str) -> bool {
    let allowed = allowed.trim_end_matches('.').to_ascii_lowercase();
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if let Some(suffix) = allowed.strip_prefix("*.") {
        return domain == suffix || domain.ends_with(&format!(".{suffix}"));
    }
    allowed == domain
}

fn query_domain(query: &Message) -> Option<String> {
    if query.queries.len() != 1 {
        return None;
    }
    Some(
        query.queries[0]
            .name()
            .to_ascii()
            .trim_end_matches('.')
            .to_ascii_lowercase(),
    )
}

fn error_response(query: &Message, code: ResponseCode) -> Message {
    let mut response = Message::new(
        query.metadata.id,
        hickory_proto::op::MessageType::Response,
        query.metadata.op_code,
    );
    response.metadata = Metadata::response_from_request(&query.metadata);
    response.metadata.response_code = code;
    response.metadata.recursion_available = true;
    response.add_queries(query.queries.iter().cloned());
    response
}

fn serialize_response(response: Message) -> Option<Vec<u8>> {
    response.to_vec().ok()
}

fn parse_ipv4(value: &str) -> Option<[u8; 4]> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0; 4];
    for (index, part) in parts.iter().enumerate() {
        bytes[index] = part.parse::<u8>().ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use hickory_proto::op::{Message, Query, ResponseCode};
    use hickory_proto::rr::{Name, RecordType};

    use super::*;
    use crate::{network_policy::VmnetPolicy, GuestNetwork};

    #[derive(Clone)]
    struct StaticUpstream {
        result: Result<Message, DnsUpstreamError>,
    }

    impl DnsUpstream for StaticUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            self.result.clone()
        }
    }

    fn policy_allowing(domain: &str) -> VmnetPolicy {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy.egress.allow_domains.push(domain.to_string());
        policy
    }

    fn query(domain: &str) -> Vec<u8> {
        let mut message = Message::query();
        message.metadata.id = 0x1234;
        message.add_query(Query::query(
            Name::from_ascii(domain).expect("query name"),
            RecordType::A,
        ));
        message.to_vec().expect("serialize query")
    }

    fn empty_success_response(id: u16, domain: &str) -> Message {
        let mut message = Message::response(id, hickory_proto::op::OpCode::Query);
        message.metadata.recursion_available = true;
        message.add_query(Query::query(
            Name::from_ascii(domain).expect("query name"),
            RecordType::A,
        ));
        message
    }

    #[test]
    fn forwards_allowed_domain_to_upstream() {
        let policy = policy_allowing("example.com");
        let upstream_response = empty_success_response(0x1234, "example.com");
        let proxy = DnsProxy::new(
            &policy,
            StaticUpstream {
                result: Ok(upstream_response.clone()),
            },
        );

        let result = proxy.handle_udp_payload(&query("example.com"));
        let response = Message::from_vec(&result.response.expect("response")).expect("parse");

        assert_eq!(response.metadata.id, upstream_response.metadata.id);
        assert_eq!(
            response.metadata.response_code,
            upstream_response.metadata.response_code
        );
        assert_eq!(response.queries.len(), 1);
        assert_eq!(
            response.queries[0].name().to_ascii().trim_end_matches('.'),
            "example.com"
        );
        assert_eq!(result.log.domain.as_deref(), Some("example.com"));
        assert_eq!(result.log.decision, DnsDecision::Allowed);
    }

    #[test]
    fn blocks_domain_not_allowed_by_policy() {
        let policy = policy_allowing("allowed.example");
        let proxy = DnsProxy::new(
            &policy,
            StaticUpstream {
                result: Ok(empty_success_response(0x1234, "blocked.example")),
            },
        );

        let result = proxy.handle_udp_payload(&query("blocked.example"));
        let response =
            Message::from_vec(&result.response.expect("denial response")).expect("parse");

        assert_eq!(result.log.decision, DnsDecision::Blocked);
        assert_eq!(response.metadata.id, 0x1234);
        assert_eq!(response.metadata.response_code, ResponseCode::Refused);
        assert_eq!(response.answers.len(), 0);
    }

    #[test]
    fn wildcard_allow_matches_subdomains() {
        let policy = policy_allowing("*.example.com");
        let proxy = DnsProxy::new(
            &policy,
            StaticUpstream {
                result: Ok(empty_success_response(0x1234, "api.example.com")),
            },
        );

        let result = proxy.handle_udp_payload(&query("api.example.com"));

        assert_eq!(result.log.decision, DnsDecision::Allowed);
    }

    #[test]
    fn malformed_query_is_logged_without_response() {
        let policy = policy_allowing("example.com");
        let proxy = DnsProxy::new(
            &policy,
            StaticUpstream {
                result: Ok(empty_success_response(0x1234, "example.com")),
            },
        );

        let result = proxy.handle_udp_payload(&[1, 2, 3]);

        assert_eq!(result.response, None);
        assert_eq!(result.log.decision, DnsDecision::Malformed);
    }

    #[test]
    fn upstream_failure_returns_servfail() {
        let policy = policy_allowing("example.com");
        let proxy = DnsProxy::new(
            &policy,
            StaticUpstream {
                result: Err(DnsUpstreamError::Unavailable),
            },
        );

        let result = proxy.handle_udp_payload(&query("example.com"));
        let response =
            Message::from_vec(&result.response.expect("servfail response")).expect("parse");

        assert_eq!(result.log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(response.metadata.response_code, ResponseCode::ServFail);
    }

    #[test]
    fn non_gateway_dns_destination_is_denied() {
        let policy = VmnetPolicy::default_sandbox(GuestNetwork::default());

        assert!(dns_allowed_to_destination(&policy, [10, 0, 2, 3], DNS_PORT));
        assert!(!dns_allowed_to_destination(&policy, [8, 8, 8, 8], DNS_PORT));
        assert!(!dns_allowed_to_destination(&policy, [10, 0, 2, 3], 5353));
    }
}
