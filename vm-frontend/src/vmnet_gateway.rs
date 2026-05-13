use smoltcp::time::Instant;

use crate::guest_tcp::{
    evaluate_tcp_syn_frame, GuestTcpCore, GuestTcpCoreError, GuestTcpSessionRef,
    QueuedEthernetDevice, DEFAULT_GATEWAY_MAC,
};
use crate::l2_gateway::{L2Gateway, ParseAddressError};
use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{TcpAction, TcpDecision, TcpDestination};
use crate::GuestNetwork;

pub struct VmnetGateway<'a> {
    policy: &'a VmnetPolicy,
    l2: L2Gateway,
    tcp_core: GuestTcpCore,
    tcp_device: QueuedEthernetDevice,
}

impl<'a> VmnetGateway<'a> {
    pub fn new(
        policy: &'a VmnetPolicy,
        network: &GuestNetwork,
        now: Instant,
    ) -> Result<Self, VmnetGatewayError> {
        let mut tcp_device = QueuedEthernetDevice::new(ethernet_mtu(policy));
        let tcp_core = GuestTcpCore::new(network, DEFAULT_GATEWAY_MAC, now, &mut tcp_device)
            .map_err(VmnetGatewayError::TcpCore)?;
        let l2 = L2Gateway::from_guest_network(network, DEFAULT_GATEWAY_MAC, policy.mtu)
            .map_err(VmnetGatewayError::Address)?;
        Ok(Self {
            policy,
            l2,
            tcp_core,
            tcp_device,
        })
    }

    pub fn handle_guest_frame(&mut self, frame: Vec<u8>, now: Instant) -> GuestFrameResult {
        if let Some(reply) = self.l2.handle_frame(&frame) {
            return GuestFrameResult {
                outcome: GuestFrameOutcome::L2Response,
                guest_frames: vec![reply],
            };
        }

        if let Some((destination, decision)) = evaluate_tcp_syn_frame(self.policy, &frame) {
            if decision.action == TcpAction::Deny {
                return GuestFrameResult {
                    outcome: GuestFrameOutcome::TcpDenied {
                        destination,
                        decision,
                    },
                    guest_frames: Vec::new(),
                };
            }
            if let Err(error) = self.tcp_core.ensure_listener(destination.port) {
                return GuestFrameResult {
                    outcome: GuestFrameOutcome::TcpSetupFailed {
                        destination,
                        detail: error.to_string(),
                    },
                    guest_frames: Vec::new(),
                };
            }
            self.tcp_device.push_rx(frame);
            self.tcp_core.poll(now, &mut self.tcp_device);
            return GuestFrameResult {
                outcome: GuestFrameOutcome::TcpAccepted {
                    destination,
                    decision,
                },
                guest_frames: self.drain_tcp_frames(),
            };
        }

        self.tcp_device.push_rx(frame);
        self.tcp_core.poll(now, &mut self.tcp_device);
        let guest_frames = self.drain_tcp_frames();
        let outcome = if guest_frames.is_empty() {
            GuestFrameOutcome::Ignored
        } else {
            GuestFrameOutcome::TcpProgress
        };
        GuestFrameResult {
            outcome,
            guest_frames,
        }
    }

    pub fn active_tcp_sessions(&self) -> Vec<GuestTcpSessionRef> {
        self.tcp_core.active_sessions()
    }

    pub fn recv_tcp_session(
        &mut self,
        handle: smoltcp::iface::SocketHandle,
    ) -> Result<Vec<u8>, smoltcp::socket::tcp::RecvError> {
        self.tcp_core.recv_available(handle)
    }

    pub fn send_tcp_session(
        &mut self,
        handle: smoltcp::iface::SocketHandle,
        data: &[u8],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>, smoltcp::socket::tcp::SendError> {
        self.tcp_core.send_to_session(handle, data)?;
        self.tcp_core.poll(now, &mut self.tcp_device);
        Ok(self.drain_tcp_frames())
    }

    pub fn policy(&self) -> &VmnetPolicy {
        self.policy
    }

    fn drain_tcp_frames(&mut self) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        while let Some(frame) = self.tcp_device.pop_tx() {
            frames.push(frame);
        }
        frames
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestFrameResult {
    pub outcome: GuestFrameOutcome,
    pub guest_frames: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestFrameOutcome {
    L2Response,
    TcpAccepted {
        destination: TcpDestination,
        decision: TcpDecision,
    },
    TcpDenied {
        destination: TcpDestination,
        decision: TcpDecision,
    },
    TcpSetupFailed {
        destination: TcpDestination,
        detail: String,
    },
    TcpProgress,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetGatewayError {
    Address(ParseAddressError),
    TcpCore(GuestTcpCoreError),
}

fn ethernet_mtu(policy: &VmnetPolicy) -> usize {
    usize::from(policy.mtu) + 14
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_policy::VmnetPolicy;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    #[test]
    fn denied_syn_fails_closed_before_tcp_core() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(tcp_syn_frame(80), Instant::from_millis(1));

        assert!(matches!(
            result.outcome,
            GuestFrameOutcome::TcpDenied { .. }
        ));
        assert!(result.guest_frames.is_empty());
    }

    #[test]
    fn allowed_syn_installs_listener_and_polls_smoltcp() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(tcp_syn_frame(80), Instant::from_millis(1));

        assert!(matches!(
            result.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));
        assert_eq!(result.guest_frames.len(), 1);
        assert_eq!(
            &result.guest_frames[0][0..6],
            EthernetAddress::BROADCAST.as_bytes()
        );
    }

    fn tcp_syn_frame(dst_port: u16) -> Vec<u8> {
        let tcp = TcpRepr {
            src_port: 49152,
            dst_port,
            control: TcpControl::Syn,
            seq_number: TcpSeqNumber(100),
            ack_number: None,
            window_len: 4096,
            window_scale: None,
            max_seg_size: Some(1460),
            sack_permitted: false,
            sack_ranges: [None, None, None],
            timestamp: None,
            payload: &[],
        };
        let ipv4 = Ipv4Repr {
            src_addr: GUEST_IP,
            dst_addr: PUBLIC_IP,
            next_header: IpProtocol::Tcp,
            payload_len: tcp.buffer_len(),
            hop_limit: 64,
        };
        let ethernet = EthernetRepr {
            src_addr: GUEST_MAC,
            dst_addr: GATEWAY_MAC,
            ethertype: EthernetProtocol::Ipv4,
        };

        let mut frame = vec![0; ethernet.buffer_len() + ipv4.buffer_len() + tcp.buffer_len()];
        ethernet.emit(&mut EthernetFrame::new_unchecked(&mut frame));
        ipv4.emit(
            &mut Ipv4Packet::new_unchecked(&mut frame[ethernet.buffer_len()..]),
            &ChecksumCapabilities::default(),
        );
        tcp.emit(
            &mut TcpPacket::new_unchecked(&mut frame[ethernet.buffer_len() + ipv4.buffer_len()..]),
            &IpAddress::Ipv4(GUEST_IP),
            &IpAddress::Ipv4(PUBLIC_IP),
            &ChecksumCapabilities::default(),
        );
        frame
    }
}
