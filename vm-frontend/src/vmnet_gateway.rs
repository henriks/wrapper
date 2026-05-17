use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr,
    TcpControl, TcpPacket, TcpRepr, TcpSeqNumber, UdpPacket,
};

use crate::dns_proxy::{
    dns_allowed_to_destination, DnsForwardRequest, DnsLogEntry, DnsProxy, DnsProxyPlan,
    DnsUpstream, DnsUpstreamError, UdpDnsUpstream,
};
use crate::guest_tcp::{
    evaluate_tcp_syn_frame, GuestTcpConnectError, GuestTcpCore, GuestTcpCoreError,
    GuestTcpSessionRef, QueuedEthernetDevice, DEFAULT_GATEWAY_MAC,
};
use crate::l2_gateway::{ipv4_checksum, L2Gateway, ParseAddressError};
use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{TcpAction, TcpDecision, TcpDestination};
use crate::vmnet_service_io::{VmnetDnsLookupCommand, VmnetServiceCommand, VmnetServiceToken};
use crate::GuestNetwork;

pub struct VmnetGateway<'a> {
    policy: &'a VmnetPolicy,
    l2: L2Gateway,
    tcp_core: GuestTcpCore,
    tcp_device: QueuedEthernetDevice,
    dns_upstream: Box<dyn DnsUpstream>,
}

impl<'a> VmnetGateway<'a> {
    pub fn new(
        policy: &'a VmnetPolicy,
        network: &GuestNetwork,
        now: Instant,
    ) -> Result<Self, VmnetGatewayError> {
        Self::new_with_dns_upstream(policy, network, now, default_dns_upstream())
    }

    pub fn new_with_dns_upstream(
        policy: &'a VmnetPolicy,
        network: &GuestNetwork,
        now: Instant,
        dns_upstream: Box<dyn DnsUpstream>,
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
            dns_upstream,
        })
    }

    pub fn handle_guest_frame(&mut self, frame: Vec<u8>, now: Instant) -> GuestFrameResult {
        match self.handle_guest_frame_with_deferred_dns(frame, now) {
            VmnetDeferredDnsFrame::Immediate(result) => result,
            VmnetDeferredDnsFrame::Forward(pending) => {
                let exchange = self.dns_upstream.exchange(&pending.request.query);
                let result = self.complete_pending_dns_query(pending, exchange);
                GuestFrameResult {
                    outcome: GuestFrameOutcome::DnsQuery { log: result.log },
                    guest_frames: result.response.into_iter().collect(),
                }
            }
        }
    }

    pub(crate) fn handle_guest_frame_with_deferred_dns(
        &mut self,
        frame: Vec<u8>,
        now: Instant,
    ) -> VmnetDeferredDnsFrame {
        if let Some(reply) = self.l2.handle_frame(&frame) {
            return VmnetDeferredDnsFrame::Immediate(GuestFrameResult {
                outcome: GuestFrameOutcome::L2Response,
                guest_frames: vec![reply],
            });
        }

        if let Some(plan) = self.plan_dns_frame(&frame) {
            return match plan {
                VmnetDnsFramePlan::Immediate(result) => {
                    VmnetDeferredDnsFrame::Immediate(GuestFrameResult {
                        outcome: GuestFrameOutcome::DnsQuery { log: result.log },
                        guest_frames: result.response.into_iter().collect(),
                    })
                }
                VmnetDnsFramePlan::Forward(pending) => VmnetDeferredDnsFrame::Forward(pending),
            };
        }

        VmnetDeferredDnsFrame::Immediate(self.handle_non_dns_guest_frame(frame, now))
    }

    fn handle_non_dns_guest_frame(&mut self, frame: Vec<u8>, now: Instant) -> GuestFrameResult {
        if let Some(denial) = udp_denial_from_frame(self.policy, &frame) {
            return GuestFrameResult {
                outcome: GuestFrameOutcome::UdpDenied(denial),
                guest_frames: Vec::new(),
            };
        }

        if let Some(unsupported) = unsupported_protocol_from_frame(self.policy, &frame) {
            return GuestFrameResult {
                outcome: GuestFrameOutcome::UnsupportedProtocol(unsupported),
                guest_frames: Vec::new(),
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
        let _ = self.tcp_core.send_to_session(handle, data)?;
        self.tcp_core.poll(now, &mut self.tcp_device);
        Ok(self.drain_tcp_frames())
    }

    pub fn send_tcp_session_partial(
        &mut self,
        handle: smoltcp::iface::SocketHandle,
        data: &[u8],
        now: Instant,
    ) -> Result<(usize, Vec<Vec<u8>>), smoltcp::socket::tcp::SendError> {
        let written = self.tcp_core.send_to_session(handle, data)?;
        self.tcp_core.poll(now, &mut self.tcp_device);
        Ok((written, self.drain_tcp_frames()))
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

    pub fn tcp_poll_delay(&mut self, now: Instant) -> Option<std::time::Duration> {
        self.tcp_core.poll_delay(now)
    }

    pub fn poll_tcp(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.tcp_core.poll(now, &mut self.tcp_device);
        self.drain_tcp_frames()
    }

    pub(crate) fn plan_dns_frame(&self, frame: &[u8]) -> Option<VmnetDnsFramePlan> {
        let query = parse_dns_query_frame(self.policy, frame)?;
        let context = query.response_context();
        let proxy = DnsProxy::new(self.policy, self.dns_upstream.as_ref());
        match proxy.plan_udp_payload(query.payload) {
            DnsProxyPlan::Immediate(result) => Some(VmnetDnsFramePlan::Immediate(
                DnsFrameResult::from_proxy_result(context, result),
            )),
            DnsProxyPlan::Forward(request) => {
                Some(VmnetDnsFramePlan::Forward(VmnetPendingDnsQuery {
                    context,
                    request,
                }))
            }
        }
    }

    pub(crate) fn complete_pending_dns_query(
        &self,
        pending: VmnetPendingDnsQuery,
        exchange: Result<hickory_proto::op::Message, DnsUpstreamError>,
    ) -> DnsFrameResult {
        let proxy = DnsProxy::new(self.policy, self.dns_upstream.as_ref());
        let context = pending.context;
        let result = proxy.complete_forward(pending.request, exchange);
        DnsFrameResult::from_proxy_result(context, result)
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
    DnsQuery {
        log: DnsLogEntry,
    },
    UdpDenied(UdpDenial),
    UnsupportedProtocol(UnsupportedProtocol),
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
pub struct UdpDenial {
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedProtocol {
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetGatewayError {
    Address(ParseAddressError),
    TcpCore(GuestTcpCoreError),
}

fn ethernet_mtu(policy: &VmnetPolicy) -> usize {
    usize::from(policy.mtu) + 14
}

pub(crate) enum VmnetDeferredDnsFrame {
    Immediate(GuestFrameResult),
    Forward(VmnetPendingDnsQuery),
}

pub(crate) enum VmnetDnsFramePlan {
    Immediate(DnsFrameResult),
    Forward(VmnetPendingDnsQuery),
}

pub(crate) struct VmnetPendingDnsQuery {
    pub context: DnsResponseContext,
    pub request: DnsForwardRequest,
}

impl VmnetPendingDnsQuery {
    #[allow(dead_code)]
    pub(crate) fn service_command(&self, token: VmnetServiceToken) -> VmnetServiceCommand {
        VmnetServiceCommand::DnsLookup(VmnetDnsLookupCommand {
            token,
            request: self.request.clone(),
        })
    }
}

pub(crate) struct DnsFrameResult {
    pub response: Option<Vec<u8>>,
    pub log: DnsLogEntry,
}

impl DnsFrameResult {
    fn from_proxy_result(
        context: DnsResponseContext,
        result: crate::dns_proxy::DnsProxyResult,
    ) -> Self {
        let response = result
            .response
            .as_ref()
            .map(|payload| dns_response_frame(&context, payload));
        Self {
            response,
            log: result.log,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DnsResponseContext {
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
}

struct DnsQueryFrame<'a> {
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &'a [u8],
}

impl DnsQueryFrame<'_> {
    fn response_context(&self) -> DnsResponseContext {
        DnsResponseContext {
            src_mac: self.src_mac,
            dst_mac: self.dst_mac,
            src_ip: self.src_ip,
            dst_ip: self.dst_ip,
            src_port: self.src_port,
            dst_port: self.dst_port,
        }
    }
}

fn parse_dns_query_frame<'a>(policy: &VmnetPolicy, frame: &'a [u8]) -> Option<DnsQueryFrame<'a>> {
    let udp = parse_udp_frame(frame)?;
    if !dns_allowed_to_destination(policy, udp.dst_ip, udp.dst_port) {
        return None;
    }
    Some(DnsQueryFrame {
        src_mac: udp.src_mac,
        dst_mac: udp.dst_mac,
        src_ip: udp.src_ip,
        dst_ip: udp.dst_ip,
        src_port: udp.src_port,
        dst_port: udp.dst_port,
        payload: udp.payload,
    })
}

fn dns_response_frame(query: &DnsResponseContext, payload: &[u8]) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let ip_total_len = 20 + udp_len;
    let mut ipv4 = Vec::with_capacity(ip_total_len);
    ipv4.push(0x45);
    ipv4.push(0);
    ipv4.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.push(64);
    ipv4.push(17);
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.extend_from_slice(&query.dst_ip);
    ipv4.extend_from_slice(&query.src_ip);
    let checksum = ipv4_checksum(&ipv4);
    ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());
    ipv4.extend_from_slice(&query.dst_port.to_be_bytes());
    ipv4.extend_from_slice(&query.src_port.to_be_bytes());
    ipv4.extend_from_slice(&(udp_len as u16).to_be_bytes());
    ipv4.extend_from_slice(&0_u16.to_be_bytes());
    ipv4.extend_from_slice(payload);

    let mut frame = Vec::with_capacity(14 + ipv4.len());
    frame.extend_from_slice(&query.src_mac);
    frame.extend_from_slice(&query.dst_mac);
    frame.extend_from_slice(&0x0800_u16.to_be_bytes());
    frame.extend_from_slice(&ipv4);
    frame
}

fn udp_denial_from_frame(policy: &VmnetPolicy, frame: &[u8]) -> Option<UdpDenial> {
    let udp = parse_udp_frame(frame)?;
    let reason = if policy.protocols.udp.block_udp_443 && udp.dst_port == 443 {
        "udp/443 blocked to prevent QUIC bypass".to_string()
    } else {
        match policy.protocols.udp.default_action {
            crate::network_policy::EgressAction::Deny => {
                "unsupported UDP denied by default".to_string()
            }
            crate::network_policy::EgressAction::AllowPublicInternet => return None,
        }
    };
    Some(UdpDenial {
        src_ip: udp.src_ip,
        dst_ip: udp.dst_ip,
        src_port: udp.src_port,
        dst_port: udp.dst_port,
        reason,
    })
}

fn unsupported_protocol_from_frame(
    policy: &VmnetPolicy,
    frame: &[u8],
) -> Option<UnsupportedProtocol> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    match ethernet.ethertype() {
        EthernetProtocol::Arp => None,
        EthernetProtocol::Ipv6 => Some(UnsupportedProtocol {
            reason: format!("IPv6 {:?} by policy", policy.protocols.ipv6),
        }),
        EthernetProtocol::Ipv4 => unsupported_ipv4_protocol_from_frame(policy, ethernet.payload()),
        other => Some(UnsupportedProtocol {
            reason: format!(
                "ethertype {other} {:?} by policy",
                policy.protocols.ethernet.unknown_ethertypes
            ),
        }),
    }
}

fn unsupported_ipv4_protocol_from_frame(
    policy: &VmnetPolicy,
    ip_payload: &[u8],
) -> Option<UnsupportedProtocol> {
    let ip = match Ipv4Packet::new_checked(ip_payload) {
        Ok(ip) => ip,
        Err(_) => {
            return Some(UnsupportedProtocol {
                reason: "malformed IPv4 denied by policy".to_string(),
            })
        }
    };
    match ip.next_header() {
        IpProtocol::Tcp => None,
        IpProtocol::Udp => {
            if UdpPacket::new_checked(ip.payload()).is_ok() {
                Some(UnsupportedProtocol {
                    reason: "UDP forwarding is not implemented".to_string(),
                })
            } else {
                Some(UnsupportedProtocol {
                    reason: "malformed UDP denied by policy".to_string(),
                })
            }
        }
        protocol => Some(UnsupportedProtocol {
            reason: format!(
                "IPv4 protocol {protocol} {:?} by policy",
                policy.protocols.ipv4.unknown_protocols
            ),
        }),
    }
}

struct UdpFrame<'a> {
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &'a [u8],
}

fn parse_udp_frame(frame: &[u8]) -> Option<UdpFrame<'_>> {
    let ethernet = EthernetFrame::new_checked(frame).ok()?;
    if ethernet.ethertype() != EthernetProtocol::Ipv4 {
        return None;
    }
    let ip = Ipv4Packet::new_checked(ethernet.payload()).ok()?;
    if ip.next_header() != IpProtocol::Udp {
        return None;
    }
    let udp = UdpPacket::new_checked(ip.payload()).ok()?;
    Some(UdpFrame {
        src_mac: ethernet.src_addr().0,
        dst_mac: ethernet.dst_addr().0,
        src_ip: ip.src_addr().octets(),
        dst_ip: ip.dst_addr().octets(),
        src_port: udp.src_port(),
        dst_port: udp.dst_port(),
        payload: udp.payload(),
    })
}

pub(crate) fn default_dns_upstream() -> Box<dyn DnsUpstream + Send + Sync> {
    Box::new(UdpDnsUpstream {
        server: host_dns_server().unwrap_or_else(|| SocketAddr::from(([1, 1, 1, 1], 53))),
        timeout: Duration::from_secs(5),
    })
}

fn host_dns_server() -> Option<SocketAddr> {
    let resolv_conf = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    for line in resolv_conf.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        if parts.next() != Some("nameserver") {
            continue;
        }
        let ip = parts.next()?.parse::<IpAddr>().ok()?;
        return Some(SocketAddr::new(ip, 53));
    }
    None
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
    use crate::dns_proxy::{domain_allowed, DnsDecision, DnsUpstreamError};
    use crate::network_policy::VmnetPolicy;
    use crate::test_support;
    use hickory_proto::op::{Message, Query};
    use hickory_proto::rr::{Name, RecordType};
    use proptest::prelude::*;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const DNS_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    #[test]
    fn dns_query_to_gateway_is_proxied_and_logged() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        let mut gateway = VmnetGateway::new_with_dns_upstream(
            &policy,
            &network,
            Instant::from_millis(0),
            Box::new(StaticDnsUpstream),
        )
        .expect("gateway");

        let result = gateway.handle_guest_frame(
            dns_query_frame("example.com", DNS_IP, 53000),
            Instant::from_millis(1),
        );

        let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
            panic!("expected DNS query outcome");
        };
        assert_eq!(log.domain.as_deref(), Some("example.com"));
        assert_eq!(log.decision, DnsDecision::Allowed);
        assert_eq!(result.guest_frames.len(), 1);

        let (src_port, dst_port, payload) = parse_udp_reply(&result.guest_frames[0]);
        assert_eq!(src_port, 53);
        assert_eq!(dst_port, 53000);
        let response = Message::from_vec(payload).expect("dns response");
        assert_eq!(response.metadata.id, 0x1234);
        assert!(response.metadata.recursion_available);
    }

    #[test]
    fn deferred_dns_query_does_not_block_unrelated_tcp_syn() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway = VmnetGateway::new_with_dns_upstream(
            &policy,
            &network,
            Instant::from_millis(0),
            Box::new(PanicDnsUpstream),
        )
        .expect("gateway");

        let dns = gateway.handle_guest_frame_with_deferred_dns(
            dns_query_frame("example.com", DNS_IP, 53000),
            Instant::from_millis(1),
        );
        let VmnetDeferredDnsFrame::Forward(pending) = dns else {
            panic!("allowed DNS query should be deferred");
        };
        assert_eq!(pending.request.domain, "example.com");

        let tcp = gateway
            .handle_guest_frame_with_deferred_dns(tcp_syn_frame(80), Instant::from_millis(2));
        let VmnetDeferredDnsFrame::Immediate(result) = tcp else {
            panic!("TCP frames must stay owner-side while DNS is pending");
        };
        assert!(matches!(
            result.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));
    }

    #[test]
    fn pending_dns_query_builds_service_command_without_losing_owner_context() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        let gateway = VmnetGateway::new_with_dns_upstream(
            &policy,
            &network,
            Instant::from_millis(0),
            Box::new(PanicDnsUpstream),
        )
        .expect("gateway");

        let VmnetDnsFramePlan::Forward(pending) = gateway
            .plan_dns_frame(&dns_query_frame("example.com", DNS_IP, 53000))
            .expect("dns plan")
        else {
            panic!("allowed DNS query should be service IO");
        };
        let command = pending.service_command(VmnetServiceToken::new(99));

        let VmnetServiceCommand::DnsLookup(command) = command else {
            panic!("expected DNS service command");
        };
        assert_eq!(command.token, VmnetServiceToken::new(99));
        assert_eq!(command.request.domain, "example.com");
        assert_eq!(pending.context.src_port, 53000);
    }

    #[test]
    fn dns_query_can_be_planned_and_completed_without_upstream_on_owner_path() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        let gateway = VmnetGateway::new_with_dns_upstream(
            &policy,
            &network,
            Instant::from_millis(0),
            Box::new(PanicDnsUpstream),
        )
        .expect("gateway");

        let plan = gateway
            .plan_dns_frame(&dns_query_frame("example.com", DNS_IP, 53000))
            .expect("dns plan");
        let VmnetDnsFramePlan::Forward(pending) = plan else {
            panic!("allowed DNS query should be deferred to service IO");
        };
        assert_eq!(pending.request.domain, "example.com");

        let result =
            gateway.complete_pending_dns_query(pending, Err(DnsUpstreamError::Unavailable));

        assert_eq!(result.log.domain.as_deref(), Some("example.com"));
        assert_eq!(result.log.decision, DnsDecision::UpstreamFailure);
        let frame = result.response.expect("servfail frame");
        let (src_port, dst_port, payload) = parse_udp_reply(&frame);
        assert_eq!(src_port, 53);
        assert_eq!(dst_port, 53000);
        let response = Message::from_vec(payload).expect("dns response");
        assert_eq!(
            response.metadata.response_code,
            hickory_proto::op::ResponseCode::ServFail
        );
    }

    #[test]
    fn dns_query_to_non_gateway_destination_is_ignored_by_dns_proxy() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway = VmnetGateway::new_with_dns_upstream(
            &policy,
            &network,
            Instant::from_millis(0),
            Box::new(StaticDnsUpstream),
        )
        .expect("gateway");

        let result = gateway.handle_guest_frame(
            dns_query_frame("example.com", Ipv4Address::new(8, 8, 8, 8), 53000),
            Instant::from_millis(1),
        );

        assert!(!matches!(
            result.outcome,
            GuestFrameOutcome::DnsQuery { .. }
        ));
    }

    #[test]
    fn udp_443_is_denied_before_tcp_core() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(
            udp_frame(53000, 443, GUEST_IP, PUBLIC_IP, b"quic?"),
            Instant::from_millis(1),
        );

        let GuestFrameOutcome::UdpDenied(denial) = result.outcome else {
            panic!("expected UDP denial");
        };
        assert_eq!(denial.dst_port, 443);
        assert!(denial.reason.contains("QUIC"));
        assert!(result.guest_frames.is_empty());
    }

    #[test]
    fn unsupported_udp_is_denied_by_default_with_diagnostic_reason() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(
            udp_frame(53000, 1234, GUEST_IP, PUBLIC_IP, b"not dns"),
            Instant::from_millis(1),
        );

        let GuestFrameOutcome::UdpDenied(denial) = result.outcome else {
            panic!("expected UDP denial");
        };
        assert_eq!(denial.dst_port, 1234);
        assert_eq!(denial.reason, "unsupported UDP denied by default");
        assert!(result.guest_frames.is_empty());
    }

    #[test]
    fn public_egress_does_not_implement_non_quic_udp_forwarding() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
        policy.protocols.udp.default_action =
            crate::network_policy::EgressAction::AllowPublicInternet;
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(
            udp_frame(53000, 1234, GUEST_IP, PUBLIC_IP, b"allowed udp"),
            Instant::from_millis(1),
        );

        let GuestFrameOutcome::UnsupportedProtocol(unsupported) = result.outcome else {
            panic!("expected unsupported UDP outcome");
        };
        assert_eq!(unsupported.reason, "UDP forwarding is not implemented");
        assert!(result.guest_frames.is_empty());
    }

    #[test]
    fn unsupported_ipv6_unknown_ethertype_and_unknown_ipv4_protocol_are_ignored() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        for frame in [
            ipv6_frame(),
            unknown_ethertype_frame(),
            unknown_ipv4_protocol_frame(132),
        ] {
            let result = gateway.handle_guest_frame(frame, Instant::from_millis(1));
            assert!(matches!(
                result.outcome,
                GuestFrameOutcome::UnsupportedProtocol(_)
            ));
            assert!(result.guest_frames.is_empty());
        }
    }

    #[test]
    fn malformed_udp_lengths_are_ignored_without_policy_decision() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");

        let result = gateway.handle_guest_frame(malformed_udp_frame(), Instant::from_millis(1));

        let GuestFrameOutcome::UnsupportedProtocol(unsupported) = result.outcome else {
            panic!("expected unsupported malformed UDP outcome");
        };
        assert_eq!(unsupported.reason, "malformed UDP denied by policy");
        assert!(result.guest_frames.is_empty());
    }

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

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 128,
            max_shrink_iters: 2048,
            ..ProptestConfig::default()
        })]

        #[test]
        fn proptest_arbitrary_guest_frames_do_not_panic_or_grow_outputs(
            frame in prop::collection::vec(any::<u8>(), 0..=1700),
        ) {
            let network = GuestNetwork::default();
            let policy = VmnetPolicy::default_sandbox(network.clone());
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");

            let result = gateway.handle_guest_frame(frame, Instant::from_millis(1));

            prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;
            if !matches!(result.outcome, GuestFrameOutcome::TcpAccepted { .. }) {
                prop_assert!(gateway.active_tcp_sessions().is_empty());
            }
        }

        #[test]
        fn proptest_udp_frames_fail_closed_without_forwarding(
            src_port in any::<u16>(),
            dst_port in any::<u16>(),
            payload in prop::collection::vec(any::<u8>(), 0..=256),
        ) {
            let network = GuestNetwork::default();
            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");

            let result = gateway.handle_guest_frame(
                test_support::udp_frame(
                    src_port,
                    dst_port,
                    test_support::TEST_GUEST_IP,
                    test_support::TEST_PUBLIC_IP,
                    &payload,
                ),
                Instant::from_millis(1),
            );

            prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;
            prop_assert!(result.guest_frames.is_empty());
            if dst_port == 443 {
                let GuestFrameOutcome::UdpDenied(denial) = result.outcome else {
                    prop_assert!(false, "udp/443 should be denied, got {:?}", result.outcome);
                    return Ok(());
                };
                prop_assert_eq!(denial.dst_port, 443);
                prop_assert!(denial.reason.contains("QUIC"));
            } else {
                prop_assert!(matches!(
                    result.outcome,
                    GuestFrameOutcome::UdpDenied(_) | GuestFrameOutcome::UnsupportedProtocol(_)
                ));
            }
            prop_assert!(gateway.active_tcp_sessions().is_empty());
        }

        #[test]
        fn proptest_unknown_ethertypes_and_ipv6_are_unsupported(
            ethertype in any::<u16>(),
            payload in prop::collection::vec(any::<u8>(), 0..=256),
        ) {
            prop_assume!(!matches!(ethertype, 0x0800 | 0x0806));
            let network = GuestNetwork::default();
            let policy = VmnetPolicy::default_sandbox(network.clone());
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");

            let result = gateway.handle_guest_frame(
                test_support::unknown_ethertype_frame(ethertype, &payload),
                Instant::from_millis(1),
            );

            prop_assert!(matches!(result.outcome, GuestFrameOutcome::UnsupportedProtocol(_)));
            prop_assert!(result.guest_frames.is_empty());
            prop_assert!(gateway.active_tcp_sessions().is_empty());
        }

        #[test]
        fn proptest_default_policy_denied_syns_do_not_create_sessions(
            dst_port in any::<u16>(),
        ) {
            let network = GuestNetwork::default();
            let policy = VmnetPolicy::default_sandbox(network.clone());
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");

            let result = gateway.handle_guest_frame(
                test_support::tcp_syn_frame(test_support::TEST_PUBLIC_IP, dst_port),
                Instant::from_millis(1),
            );

            if !matches!(result.outcome, GuestFrameOutcome::TcpDenied { .. }) {
                return Err(TestCaseError::fail(format!(
                    "default policy TCP SYN was not denied: {:?}",
                    result.outcome
                )));
            }
            prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;
            prop_assert!(gateway.active_tcp_sessions().is_empty());
        }

        #[test]
        fn proptest_malformed_ipv4_payloads_fail_closed(
            payload in prop::collection::vec(any::<u8>(), 0..=96),
        ) {
            let network = GuestNetwork::default();
            let policy = VmnetPolicy::default_sandbox(network.clone());
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");
            let mut frame = Vec::with_capacity(14 + payload.len());
            frame.extend_from_slice(GATEWAY_MAC.as_bytes());
            frame.extend_from_slice(GUEST_MAC.as_bytes());
            frame.extend_from_slice(&0x0800_u16.to_be_bytes());
            frame.extend_from_slice(&payload);

            let result = gateway.handle_guest_frame(frame, Instant::from_millis(1));

            prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;
            if result.guest_frames.is_empty() {
                if matches!(result.outcome, GuestFrameOutcome::TcpAccepted { .. }) {
                    return Err(TestCaseError::fail(
                        "malformed IPv4 payload was accepted as TCP".to_string(),
                    ));
                }
            }
            prop_assert!(gateway.active_tcp_sessions().is_empty());
        }

        #[test]
        fn proptest_dns_gateway_outcome_matches_domain_policy(
            label in 0_u16..=999,
            use_subdomain in any::<bool>(),
            allow_variant in 0_u8..=3,
            uppercase_query in any::<bool>(),
            trailing_dot in any::<bool>(),
            default_public in any::<bool>(),
        ) {
            let network = GuestNetwork::default();
            let base = format!("case{label}.example");
            let query_domain = if use_subdomain {
                format!("api.{base}")
            } else {
                base.clone()
            };
            let mut wire_domain = if uppercase_query {
                query_domain.to_ascii_uppercase()
            } else {
                query_domain.clone()
            };
            if trailing_dot {
                wire_domain.push('.');
            }

            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            if default_public {
                policy.egress.default_action = crate::network_policy::EgressAction::AllowPublicInternet;
            }
            match allow_variant {
                0 => policy.egress.allow_domains.push(query_domain.clone()),
                1 => policy.egress.allow_domains.push(format!("*.{base}")),
                2 => policy.egress.allow_domains.push("unrelated.example".to_string()),
                _ => {}
            }

            let expected_allowed = domain_allowed(&policy, &wire_domain);
            let mut gateway = VmnetGateway::new_with_dns_upstream(
                &policy,
                &network,
                Instant::from_millis(0),
                Box::new(StaticDnsUpstream),
            )
            .expect("gateway");

            let result = gateway.handle_guest_frame(
                test_support::dns_query_frame(&wire_domain, test_support::TEST_DNS_IP, 53000),
                Instant::from_millis(1),
            );

            let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
                return Err(TestCaseError::fail("gateway did not classify gateway DNS as DNS query"));
            };
            prop_assert_eq!(
                log.decision,
                if expected_allowed {
                    DnsDecision::Allowed
                } else {
                    DnsDecision::Blocked
                }
            );
            let normalized_query_domain = query_domain.to_ascii_lowercase();
            prop_assert_eq!(log.domain.as_deref(), Some(normalized_query_domain.as_str()));
            prop_assert_eq!(result.guest_frames.len(), 1);
        }

        #[test]
        fn proptest_structured_guest_packets_keep_policy_invariants(
            packet_kind in 0_u8..=10,
            port in 1_u16..=u16::MAX,
            payload in prop::collection::vec(any::<u8>(), 0..=128),
            domain_label in 0_u16..=999,
        ) {
            let network = GuestNetwork::default();
            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            let mut gateway = VmnetGateway::new_with_dns_upstream(
                &policy,
                &network,
                Instant::from_millis(0),
                Box::new(StaticDnsUpstream),
            )
            .expect("gateway");

            let frame = match packet_kind {
                0 => test_support::ipv6_frame(&payload),
                1 => test_support::unknown_ethertype_frame(0x88b5, &payload),
                2 => test_support::unknown_ipv4_protocol_frame(132),
                3 => test_support::malformed_udp_len_frame((payload.len() as u16).min(7)),
                4 => test_support::udp_frame(
                    port,
                    443,
                    test_support::TEST_GUEST_IP,
                    test_support::TEST_PUBLIC_IP,
                    &payload,
                ),
                5 => test_support::udp_frame(
                    port,
                    port.saturating_add(1).max(1),
                    test_support::TEST_GUEST_IP,
                    test_support::TEST_PUBLIC_IP,
                    &payload,
                ),
                6 => {
                    policy.egress.allow_domains.push(format!("case{domain_label}.example"));
                    gateway = VmnetGateway::new_with_dns_upstream(
                        &policy,
                        &network,
                        Instant::from_millis(0),
                        Box::new(StaticDnsUpstream),
                    )
                    .expect("gateway");
                    test_support::dns_query_frame(
                        &format!("case{domain_label}.example"),
                        test_support::TEST_DNS_IP,
                        port,
                    )
                }
                7 => test_support::dns_query_frame(
                    &format!("blocked{domain_label}.example"),
                    test_support::TEST_DNS_IP,
                    port,
                ),
                8 => test_support::tcp_syn_frame(test_support::TEST_PUBLIC_IP, port),
                9 => {
                    policy.egress.allow_ips.push(test_support::TEST_PUBLIC_IP.to_string());
                    gateway = VmnetGateway::new_with_dns_upstream(
                        &policy,
                        &network,
                        Instant::from_millis(0),
                        Box::new(StaticDnsUpstream),
                    )
                    .expect("gateway");
                    test_support::tcp_syn_frame(test_support::TEST_PUBLIC_IP, port)
                }
                _ => {
                    let mut frame = test_support::tcp_syn_frame(test_support::TEST_PUBLIC_IP, port);
                    frame[14 + 6..14 + 8].copy_from_slice(&0x2001_u16.to_be_bytes());
                    frame
                }
            };

            let result = gateway.handle_guest_frame(frame, Instant::from_millis(1));
            prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;

            match packet_kind {
                4 => prop_assert!(matches!(result.outcome, GuestFrameOutcome::UdpDenied(_))),
                6 | 7 => prop_assert!(
                    matches!(result.outcome, GuestFrameOutcome::DnsQuery { .. }),
                    "expected DNS query outcome, got {:?}",
                    result.outcome
                ),
                8 => {
                    prop_assert!(
                        matches!(result.outcome, GuestFrameOutcome::TcpDenied { .. }),
                        "expected TCP deny outcome, got {:?}",
                        result.outcome
                    );
                    prop_assert!(gateway.active_tcp_sessions().is_empty());
                }
                9 => {
                    prop_assert!(
                        matches!(result.outcome, GuestFrameOutcome::TcpAccepted { .. }),
                        "expected TCP accepted outcome, got {:?}",
                        result.outcome
                    );
                    prop_assert!(gateway.active_tcp_sessions().len() <= 1);
                }
                _ => {
                    prop_assert!(gateway.active_tcp_sessions().is_empty());
                }
            }
        }

        #[test]
        fn proptest_repeated_tcp_syns_do_not_grow_sessions_unbounded(
            port in 1_u16..=u16::MAX,
            allowed in any::<bool>(),
            repeats in 1_usize..=8,
        ) {
            let network = GuestNetwork::default();
            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            if allowed {
                policy.egress.allow_ips.push(test_support::TEST_PUBLIC_IP.to_string());
            }
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");

            for step in 0..repeats {
                let result = gateway.handle_guest_frame(
                    test_support::tcp_syn_frame(test_support::TEST_PUBLIC_IP, port),
                    Instant::from_millis(step as i64 + 1),
                );
                prop_assert_guest_frames_bounded(&result.guest_frames, &policy)?;
                if allowed {
                    prop_assert!(gateway.active_tcp_sessions().len() <= 1);
                } else {
                    prop_assert!(
                        matches!(result.outcome, GuestFrameOutcome::TcpDenied { .. }),
                        "expected TCP deny outcome, got {:?}",
                        result.outcome
                    );
                    prop_assert!(gateway.active_tcp_sessions().is_empty());
                }
            }
        }
    }

    #[test]
    #[ignore = "property stress: run explicitly with `cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_gateway::tests::stress_seeded_generated_guest_frames -- --ignored --nocapture`"]
    fn stress_seeded_generated_guest_frames() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut seed = 0x6761_7465_7761_795fu64;

        for step in 0..2048 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let len = (seed as usize) % 1700;
            let mut frame = Vec::with_capacity(len);
            for byte_index in 0..len {
                seed ^= seed.rotate_left(13).wrapping_add(byte_index as u64);
                frame.push(seed as u8);
            }

            let mut gateway =
                VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
            let result = gateway.handle_guest_frame(frame, Instant::from_millis(step));
            assert_guest_frames_bounded(&result.guest_frames, &policy);
            if !matches!(result.outcome, GuestFrameOutcome::TcpAccepted { .. }) {
                assert!(
                    gateway.active_tcp_sessions().is_empty(),
                    "unexpected session at generated step {step}"
                );
            }
        }
    }

    #[derive(Debug)]
    struct PanicDnsUpstream;

    impl DnsUpstream for PanicDnsUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            panic!("planned DNS path must not call upstream synchronously")
        }
    }

    #[derive(Debug)]
    struct StaticDnsUpstream;

    impl DnsUpstream for StaticDnsUpstream {
        fn exchange(&self, query: &Message) -> Result<Message, DnsUpstreamError> {
            let mut response = Message::response(query.metadata.id, query.metadata.op_code);
            response.metadata.recursion_available = true;
            response.add_queries(query.queries.iter().cloned());
            Ok(response)
        }
    }

    fn dns_query_frame(domain: &str, dst_ip: Ipv4Address, src_port: u16) -> Vec<u8> {
        let mut message = Message::query();
        message.metadata.id = 0x1234;
        message.add_query(Query::query(
            Name::from_ascii(domain).expect("query name"),
            RecordType::A,
        ));
        let payload = message.to_vec().expect("serialize dns query");
        udp_frame(src_port, 53, GUEST_IP, dst_ip, &payload)
    }

    fn udp_frame(
        src_port: u16,
        dst_port: u16,
        src_ip: Ipv4Address,
        dst_ip: Ipv4Address,
        payload: &[u8],
    ) -> Vec<u8> {
        let udp_len = 8 + payload.len();
        let ip_total_len = 20 + udp_len;
        let mut ipv4 = Vec::with_capacity(ip_total_len);
        ipv4.push(0x45);
        ipv4.push(0);
        ipv4.extend_from_slice(&(ip_total_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.push(64);
        ipv4.push(17);
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&src_ip.octets());
        ipv4.extend_from_slice(&dst_ip.octets());
        let checksum = ipv4_checksum(&ipv4);
        ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());
        ipv4.extend_from_slice(&src_port.to_be_bytes());
        ipv4.extend_from_slice(&dst_port.to_be_bytes());
        ipv4.extend_from_slice(&(udp_len as u16).to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(payload);

        let mut frame = Vec::with_capacity(14 + ipv4.len());
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&0x0800_u16.to_be_bytes());
        frame.extend_from_slice(&ipv4);
        frame
    }

    fn ipv6_frame() -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&0x86dd_u16.to_be_bytes());
        frame.extend_from_slice(&[0x60, 0, 0, 0, 0, 0, 59, 64]);
        frame.extend_from_slice(&[0; 32]);
        frame
    }

    fn unknown_ethertype_frame() -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&0x88b5_u16.to_be_bytes());
        frame.extend_from_slice(b"unknown");
        frame
    }

    fn unknown_ipv4_protocol_frame(protocol: u8) -> Vec<u8> {
        let mut ipv4 = Vec::new();
        ipv4.push(0x45);
        ipv4.push(0);
        ipv4.extend_from_slice(&20_u16.to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.push(64);
        ipv4.push(protocol);
        ipv4.extend_from_slice(&0_u16.to_be_bytes());
        ipv4.extend_from_slice(&GUEST_IP.octets());
        ipv4.extend_from_slice(&PUBLIC_IP.octets());
        let checksum = ipv4_checksum(&ipv4);
        ipv4[10..12].copy_from_slice(&checksum.to_be_bytes());

        let mut frame = Vec::new();
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&0x0800_u16.to_be_bytes());
        frame.extend_from_slice(&ipv4);
        frame
    }

    fn malformed_udp_frame() -> Vec<u8> {
        let mut frame = udp_frame(53000, 1234, GUEST_IP, PUBLIC_IP, b"short");
        frame[38..40].copy_from_slice(&4_u16.to_be_bytes());
        frame
    }

    fn parse_udp_reply(frame: &[u8]) -> (u16, u16, &[u8]) {
        assert_eq!(&frame[0..6], GUEST_MAC.as_bytes());
        assert_eq!(&frame[6..12], GATEWAY_MAC.as_bytes());
        let ip = &frame[14..];
        assert_eq!(&ip[12..16], &DNS_IP.octets());
        assert_eq!(&ip[16..20], &GUEST_IP.octets());
        let udp = &ip[20..];
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let udp_len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
        (src_port, dst_port, &udp[8..udp_len])
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

    fn prop_assert_guest_frames_bounded(
        frames: &[Vec<u8>],
        policy: &VmnetPolicy,
    ) -> Result<(), TestCaseError> {
        let max_frame_len = usize::from(policy.mtu) + 14;
        for frame in frames {
            prop_assert!(
                frame.len() <= max_frame_len,
                "guest frame len {} exceeded {}",
                frame.len(),
                max_frame_len
            );
        }
        Ok(())
    }

    fn assert_guest_frames_bounded(frames: &[Vec<u8>], policy: &VmnetPolicy) {
        let max_frame_len = usize::from(policy.mtu) + 14;
        for frame in frames {
            assert!(
                frame.len() <= max_frame_len,
                "guest frame len {} exceeded {}",
                frame.len(),
                max_frame_len
            );
        }
    }
}
