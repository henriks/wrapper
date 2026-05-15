#![no_main]

use agentvm_frontend::dns_proxy::{DnsDecision, DnsProxy, DnsUpstream, DnsUpstreamError};
use agentvm_frontend::network_policy::{EgressAction, VmnetPolicy};
use agentvm_frontend::GuestNetwork;
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct FailingUpstream;

impl DnsUpstream for FailingUpstream {
    fn exchange(
        &self,
        _query: &hickory_proto::op::Message,
    ) -> Result<hickory_proto::op::Message, DnsUpstreamError> {
        Err(DnsUpstreamError::Unavailable)
    }
}

fuzz_target!(|data: &[u8]| {
    let network = GuestNetwork::default();
    let mut policy = VmnetPolicy::default_sandbox(network);
    policy.egress.default_action = EgressAction::AllowPublicInternet;
    let proxy = DnsProxy::new(&policy, FailingUpstream);
    let result = proxy.handle_udp_payload(data);

    match result.log.decision {
        DnsDecision::Allowed | DnsDecision::UpstreamFailure => {
            assert!(result.log.domain.is_some());
        }
        DnsDecision::Blocked => {
            panic!("allow-public DNS fuzz policy should not block parsed domains");
        }
        DnsDecision::Malformed => {}
    }
    if let Some(response) = result.response {
        assert!(response.len() <= 4096);
    }
});
