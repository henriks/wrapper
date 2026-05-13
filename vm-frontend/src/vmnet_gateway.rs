use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr,
    TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
};

use crate::guest_tcp::{
    evaluate_tcp_syn_frame, GuestTcpConnectError, GuestTcpCore, GuestTcpCoreError,
    GuestTcpSessionRef, QueuedEthernetDevice, DEFAULT_GATEWAY_MAC,
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
                let guest_frames = tcp_reset_for_denied_syn(&frame).into_iter().collect();
                return GuestFrameResult {
                    outcome: GuestFrameOutcome::TcpDenied {
                        destination,
                        decision,
                    },
                    guest_frames,
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

    pub fn connect_host_to_guest(
        &mut self,
        guest_port: u16,
        local_port: u16,
        now: Instant,
    ) -> Result<HostIngressConnect, GuestTcpConnectError> {
        let handle = self.tcp_core.connect_to_guest(
            &self.policy.assignment,
            guest_port,
            local_port,
            now,
            &mut self.tcp_device,
        )?;
        self.tcp_core.poll(now, &mut self.tcp_device);
        Ok(HostIngressConnect {
            handle,
            guest_frames: self.drain_tcp_frames(),
        })
    }

    pub fn host_ingress_sessions(&self) -> Vec<GuestTcpSessionRef> {
        self.tcp_core.host_ingress_sessions()
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

    pub fn close_tcp_session(
        &mut self,
        handle: smoltcp::iface::SocketHandle,
        now: Instant,
    ) -> Vec<Vec<u8>> {
        self.tcp_core.close_session(handle);
        self.tcp_core.poll(now, &mut self.tcp_device);
        self.drain_tcp_frames()
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
pub struct HostIngressConnect {
    pub handle: smoltcp::iface::SocketHandle,
    pub guest_frames: Vec<Vec<u8>>,
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

fn tcp_reset_for_denied_syn(frame: &[u8]) -> Option<Vec<u8>> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    let ethernet = EthernetRepr::parse(&ethernet).ok()?;
    if ethernet.ethertype != EthernetProtocol::Ipv4 {
        return None;
    }

    let ip_offset = ethernet.buffer_len();
    let ipv4_packet = Ipv4Packet::new_checked(&frame[ip_offset..]).ok()?;
    let ipv4 = Ipv4Repr::parse(&ipv4_packet, &Default::default()).ok()?;
    if ipv4.next_header != IpProtocol::Tcp {
        return None;
    }

    let tcp_offset = ip_offset + ipv4.buffer_len();
    let tcp_packet = TcpPacket::new_checked(&frame[tcp_offset..]).ok()?;
    let tcp = TcpRepr::parse(
        &tcp_packet,
        &IpAddress::Ipv4(ipv4.src_addr),
        &IpAddress::Ipv4(ipv4.dst_addr),
        &Default::default(),
    )
    .ok()?;

    let reply_tcp = TcpRepr {
        src_port: tcp.dst_port,
        dst_port: tcp.src_port,
        control: TcpControl::Rst,
        seq_number: TcpSeqNumber(0),
        ack_number: Some(tcp.seq_number + 1),
        window_len: 0,
        window_scale: None,
        max_seg_size: None,
        sack_permitted: false,
        sack_ranges: [None, None, None],
        timestamp: None,
        payload: &[],
    };
    let reply_ipv4 = Ipv4Repr {
        src_addr: ipv4.dst_addr,
        dst_addr: ipv4.src_addr,
        next_header: IpProtocol::Tcp,
        payload_len: reply_tcp.buffer_len(),
        hop_limit: 64,
    };
    let reply_ethernet = EthernetRepr {
        src_addr: ethernet.dst_addr,
        dst_addr: ethernet.src_addr,
        ethertype: EthernetProtocol::Ipv4,
    };

    let mut reply =
        vec![0; reply_ethernet.buffer_len() + reply_ipv4.buffer_len() + reply_tcp.buffer_len()];
    reply_ethernet.emit(&mut EthernetFrame::new_unchecked(&mut reply));
    reply_ipv4.emit(
        &mut Ipv4Packet::new_unchecked(&mut reply[reply_ethernet.buffer_len()..]),
        &Default::default(),
    );
    reply_tcp.emit(
        &mut TcpPacket::new_unchecked(
            &mut reply[reply_ethernet.buffer_len() + reply_ipv4.buffer_len()..],
        ),
        &IpAddress::Ipv4(reply_ipv4.src_addr),
        &IpAddress::Ipv4(reply_ipv4.dst_addr),
        &Default::default(),
    );
    Some(reply)
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
        assert_eq!(result.guest_frames.len(), 1);
        let reset = parse_tcp_reply(&result.guest_frames[0]);
        assert_eq!(reset.control, TcpControl::Rst);
        assert_eq!(reset.src_port, 80);
        assert_eq!(reset.dst_port, 49152);
        assert_eq!(reset.ack_number, Some(TcpSeqNumber(101)));
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

    #[test]
    fn host_ingress_connect_emits_guest_frames_without_guest_egress_session() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let connect = gateway
            .connect_host_to_guest(1075, 40000, Instant::from_millis(1))
            .expect("connect");

        assert_eq!(connect.guest_frames.len(), 1);
        assert_eq!(
            &connect.guest_frames[0][0..6],
            EthernetAddress::BROADCAST.as_bytes()
        );
        assert!(gateway.active_tcp_sessions().is_empty());
        assert_eq!(gateway.host_ingress_sessions().len(), 1);
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

    fn parse_tcp_reply(frame: &[u8]) -> TcpRepr<'_> {
        let ethernet = EthernetFrame::new_unchecked(frame);
        let ethernet = EthernetRepr::parse(&ethernet).expect("ethernet");
        let ip_offset = ethernet.buffer_len();
        let ipv4 = Ipv4Packet::new_unchecked(&frame[ip_offset..]);
        let ipv4 = Ipv4Repr::parse(&ipv4, &ChecksumCapabilities::default()).expect("ipv4");
        let tcp_offset = ip_offset + ipv4.buffer_len();
        TcpRepr::parse(
            &TcpPacket::new_unchecked(&frame[tcp_offset..]),
            &IpAddress::Ipv4(ipv4.src_addr),
            &IpAddress::Ipv4(ipv4.dst_addr),
            &ChecksumCapabilities::default(),
        )
        .expect("tcp")
    }
}
