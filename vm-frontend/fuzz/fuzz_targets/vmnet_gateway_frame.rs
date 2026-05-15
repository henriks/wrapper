#![no_main]

use agentvm_frontend::network_policy::VmnetPolicy;
use agentvm_frontend::vmnet_gateway::VmnetGateway;
use agentvm_frontend::GuestNetwork;
use libfuzzer_sys::fuzz_target;
use smoltcp::time::Instant;

fuzz_target!(|data: &[u8]| {
    let network = GuestNetwork::default();
    let policy = VmnetPolicy::default_sandbox(network.clone());
    let mut gateway =
        VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
    let result = gateway.handle_guest_frame(data.to_vec(), Instant::from_millis(1));
    let max_frame_len = usize::from(policy.mtu) + 14;
    for frame in &result.guest_frames {
        assert!(frame.len() <= max_frame_len);
    }
    if !matches!(
        result.outcome,
        agentvm_frontend::vmnet_gateway::GuestFrameOutcome::TcpAccepted { .. }
    ) {
        assert!(gateway.active_tcp_sessions().is_empty());
    }
});

