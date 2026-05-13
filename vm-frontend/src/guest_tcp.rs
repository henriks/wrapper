use std::collections::VecDeque;
use std::net::Ipv4Addr;

use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, PacketMeta, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, IpEndpoint, Ipv4Address, Ipv4Cidr};

use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{evaluate_tcp_destination, TcpDecision, TcpDestination};
use crate::GuestNetwork;

pub const DEFAULT_GATEWAY_MAC: [u8; 6] = [0x02, 0xfc, 0x12, 0x34, 0x56, 0x01];
pub const DEFAULT_TCP_BUFFER_BYTES: usize = 64 * 1024;

pub struct GuestTcpCore {
    interface: Interface,
    sockets: SocketSet<'static>,
    listeners: Vec<TcpListenerSlot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TcpListenerSlot {
    port: u16,
    handle: SocketHandle,
}

impl GuestTcpCore {
    pub fn new(
        network: &GuestNetwork,
        gateway_mac: [u8; 6],
        now: Instant,
        device: &mut QueuedEthernetDevice,
    ) -> Result<Self, GuestTcpCoreError> {
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(gateway_mac)));
        config.random_seed = 0xfeed_cafe_d00d_beef;

        let mut interface = Interface::new(config, device, now);
        let gateway_ip = parse_ipv4(&network.gateway_ip)?;
        interface.update_ip_addrs(|ip_addrs| {
            ip_addrs
                .push(IpCidr::Ipv4(Ipv4Cidr::new(gateway_ip, network.prefix_len)))
                .expect("smoltcp supports at least one interface address");
        });
        interface.set_any_ip(true);

        Ok(Self {
            interface,
            sockets: SocketSet::new(Vec::new()),
            listeners: Vec::new(),
        })
    }

    pub fn listen_tcp(&mut self, port: u16) -> Result<SocketHandle, tcp::ListenError> {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; DEFAULT_TCP_BUFFER_BYTES]),
            tcp::SocketBuffer::new(vec![0; DEFAULT_TCP_BUFFER_BYTES]),
        );
        socket.listen(port)?;
        let handle = self.sockets.add(socket);
        self.listeners.push(TcpListenerSlot { port, handle });
        Ok(handle)
    }

    pub fn ensure_listener(&mut self, port: u16) -> Result<SocketHandle, tcp::ListenError> {
        if let Some(slot) = self.listeners.iter().find(|slot| {
            slot.port == port && self.listener_state(slot.handle) == tcp::State::Listen
        }) {
            return Ok(slot.handle);
        }
        self.listen_tcp(port)
    }

    pub fn poll(&mut self, now: Instant, device: &mut QueuedEthernetDevice) -> PollResult {
        self.interface.poll(now, device, &mut self.sockets)
    }

    pub fn session(&self, handle: SocketHandle) -> Option<GuestTcpSession> {
        let socket = self.sockets.get::<tcp::Socket>(handle);
        Some(GuestTcpSession {
            state: socket.state(),
            local: socket.local_endpoint()?,
            remote: socket.remote_endpoint()?,
        })
    }

    pub fn active_sessions(&self) -> Vec<GuestTcpSessionRef> {
        self.listeners
            .iter()
            .filter_map(|slot| {
                Some(GuestTcpSessionRef {
                    handle: slot.handle,
                    session: self.session(slot.handle)?,
                })
            })
            .collect()
    }

    pub fn listener_state(&self, handle: SocketHandle) -> tcp::State {
        self.sockets.get::<tcp::Socket>(handle).state()
    }

    pub fn recv_available(&mut self, handle: SocketHandle) -> Result<Vec<u8>, tcp::RecvError> {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        let mut received = Vec::new();
        while socket.can_recv() {
            let chunk = socket.recv(|data| {
                let len = data.len();
                (len, data.to_vec())
            })?;
            if chunk.is_empty() {
                break;
            }
            received.extend_from_slice(&chunk);
        }
        Ok(received)
    }

    pub fn send_to_session(
        &mut self,
        handle: SocketHandle,
        data: &[u8],
    ) -> Result<usize, tcp::SendError> {
        self.sockets.get_mut::<tcp::Socket>(handle).send_slice(data)
    }

    pub fn any_ip_enabled(&self) -> bool {
        self.interface.any_ip()
    }

    pub fn ip_addrs(&self) -> &[IpCidr] {
        self.interface.ip_addrs()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestTcpSession {
    pub state: tcp::State,
    pub local: IpEndpoint,
    pub remote: IpEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestTcpSessionRef {
    pub handle: SocketHandle,
    pub session: GuestTcpSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestTcpCoreError {
    InvalidIpv4(String),
}

pub fn parse_tcp_syn_destination(frame: &[u8]) -> Option<TcpDestination> {
    let ethernet = smoltcp::wire::EthernetFrame::new_checked(frame).ok()?;
    let ethernet = smoltcp::wire::EthernetRepr::parse(&ethernet).ok()?;
    if ethernet.ethertype != smoltcp::wire::EthernetProtocol::Ipv4 {
        return None;
    }

    let ip_offset = ethernet.buffer_len();
    let ipv4 = smoltcp::wire::Ipv4Packet::new_checked(&frame[ip_offset..]).ok()?;
    let ipv4 = smoltcp::wire::Ipv4Repr::parse(&ipv4, &Default::default()).ok()?;
    if ipv4.next_header != smoltcp::wire::IpProtocol::Tcp {
        return None;
    }

    let tcp_offset = ip_offset + ipv4.buffer_len();
    let tcp = smoltcp::wire::TcpPacket::new_checked(&frame[tcp_offset..]).ok()?;
    let tcp = smoltcp::wire::TcpRepr::parse(
        &tcp,
        &smoltcp::wire::IpAddress::Ipv4(ipv4.src_addr),
        &smoltcp::wire::IpAddress::Ipv4(ipv4.dst_addr),
        &Default::default(),
    )
    .ok()?;

    (tcp.control == smoltcp::wire::TcpControl::Syn && tcp.ack_number.is_none()).then_some(
        TcpDestination {
            ip: ipv4.dst_addr,
            port: tcp.dst_port,
            domain: None,
        },
    )
}

pub fn evaluate_tcp_syn_frame(
    policy: &VmnetPolicy,
    frame: &[u8],
) -> Option<(TcpDestination, TcpDecision)> {
    let destination = parse_tcp_syn_destination(frame)?;
    let decision = evaluate_tcp_destination(policy, &destination);
    Some((destination, decision))
}

#[derive(Debug, Default)]
pub struct QueuedEthernetDevice {
    mtu: usize,
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

impl QueuedEthernetDevice {
    pub fn new(mtu: usize) -> Self {
        Self {
            mtu,
            rx: VecDeque::new(),
            tx: VecDeque::new(),
        }
    }

    pub fn push_rx(&mut self, frame: Vec<u8>) {
        self.rx.push_back(frame);
    }

    pub fn pop_tx(&mut self) -> Option<Vec<u8>> {
        self.tx.pop_front()
    }

    pub fn tx_len(&self) -> usize {
        self.tx.len()
    }
}

impl Device for QueuedEthernetDevice {
    type RxToken<'a>
        = QueuedRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = QueuedTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let frame = self.rx.pop_front()?;
        Some((QueuedRxToken { frame }, QueuedTxToken { tx: &mut self.tx }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(QueuedTxToken { tx: &mut self.tx })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.mtu;
        caps.max_burst_size = Some(1);
        caps
    }
}

#[derive(Debug)]
pub struct QueuedRxToken {
    frame: Vec<u8>,
}

impl RxToken for QueuedRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.frame)
    }

    fn meta(&self) -> PacketMeta {
        PacketMeta::default()
    }
}

#[derive(Debug)]
pub struct QueuedTxToken<'a> {
    tx: &'a mut VecDeque<Vec<u8>>,
}

impl TxToken for QueuedTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut frame = vec![0; len];
        let result = f(&mut frame);
        self.tx.push_back(frame);
        result
    }
}

fn parse_ipv4(value: &str) -> Result<Ipv4Address, GuestTcpCoreError> {
    let octets = value
        .parse::<Ipv4Addr>()
        .map_err(|_| GuestTcpCoreError::InvalidIpv4(value.to_string()))?
        .octets();
    Ok(Ipv4Address::from_octets(octets))
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol, Ipv4Packet, Ipv4Repr,
        TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    fn core() -> (GuestTcpCore, QueuedEthernetDevice, SocketHandle) {
        let network = GuestNetwork::default();
        let mut device = QueuedEthernetDevice::new(1514);
        let mut core = GuestTcpCore::new(
            &network,
            DEFAULT_GATEWAY_MAC,
            Instant::from_millis(0),
            &mut device,
        )
        .expect("core");
        let http = core.listen_tcp(80).expect("listen");
        (core, device, http)
    }

    #[test]
    fn configures_gateway_ip_and_any_ip_for_original_destinations() {
        let (core, _device, _http) = core();

        assert!(core.any_ip_enabled());
        assert_eq!(
            core.ip_addrs(),
            &[IpCidr::Ipv4(Ipv4Cidr::new(
                Ipv4Address::new(10, 0, 2, 2),
                24
            ))]
        );
    }

    #[test]
    fn starts_tcp_listeners_in_smoltcp() {
        let (core, _device, http) = core();

        assert_eq!(core.listener_state(http), tcp::State::Listen);
    }

    #[test]
    fn reuses_open_listener_and_replenishes_after_accept() {
        let (mut core, mut device, http) = core();

        assert_eq!(core.ensure_listener(80).expect("reuse"), http);

        complete_http_handshake(&mut core, &mut device, http);
        let next_http = core.ensure_listener(80).expect("replenish");

        assert_ne!(next_http, http);
        assert_eq!(core.listener_state(next_http), tcp::State::Listen);
    }

    #[test]
    fn extracts_initial_syn_destination_before_smoltcp_consumes_frame() {
        let destination = parse_tcp_syn_destination(&tcp_frame(
            49152,
            80,
            TcpControl::Syn,
            TcpSeqNumber(100),
            None,
            &[],
        ))
        .expect("destination");

        assert_eq!(destination.ip, Ipv4Address::new(93, 184, 216, 34));
        assert_eq!(destination.port, 80);
        assert_eq!(destination.domain, None);
    }

    #[test]
    fn ignores_non_initial_tcp_segments_for_listener_provisioning() {
        assert_eq!(
            parse_tcp_syn_destination(&tcp_frame(
                49152,
                80,
                TcpControl::None,
                TcpSeqNumber(101),
                Some(TcpSeqNumber(1)),
                &[],
            )),
            None
        );
    }

    #[test]
    fn evaluates_syn_policy_before_smoltcp_accepts_connection() {
        let mut policy =
            crate::network_policy::VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy.egress.allow_ips.push("93.184.216.34".to_string());

        let (_destination, decision) = evaluate_tcp_syn_frame(
            &policy,
            &tcp_frame(49152, 80, TcpControl::Syn, TcpSeqNumber(100), None, &[]),
        )
        .expect("decision");

        assert_eq!(
            decision.action,
            crate::tcp_gateway::TcpAction::InterceptHttp
        );
    }

    #[test]
    fn denied_syn_policy_can_fail_closed_before_smoltcp() {
        let policy = crate::network_policy::VmnetPolicy::default_sandbox(GuestNetwork::default());

        let (_destination, decision) = evaluate_tcp_syn_frame(
            &policy,
            &tcp_frame(49152, 80, TcpControl::Syn, TcpSeqNumber(100), None, &[]),
        )
        .expect("decision");

        assert_eq!(decision.action, crate::tcp_gateway::TcpAction::Deny);
    }

    #[test]
    fn guest_syn_advances_listener_and_emits_syn_ack_after_neighbor_resolution() {
        let (mut core, mut device, http) = core();
        device.push_rx(tcp_frame(
            49152,
            80,
            TcpControl::Syn,
            TcpSeqNumber(100),
            None,
            &[],
        ));

        assert_eq!(
            core.poll(Instant::from_millis(1), &mut device),
            PollResult::SocketStateChanged
        );

        let session = core.session(http).expect("accepted session");
        assert_eq!(session.state, tcp::State::SynReceived);
        assert_eq!(
            session.local,
            IpEndpoint::new(IpAddress::Ipv4(PUBLIC_IP), 80)
        );
        assert_eq!(
            session.remote,
            IpEndpoint::new(IpAddress::Ipv4(GUEST_IP), 49152)
        );
        assert_eq!(device.tx_len(), 1);
        let arp_request = device.pop_tx().expect("arp request");
        assert_eq!(&arp_request[0..6], EthernetAddress::BROADCAST.as_bytes());

        device.push_rx(arp_reply_frame());
        core.poll(Instant::from_millis(2), &mut device);

        let syn_ack = device.pop_tx().expect("syn ack");
        assert_eq!(&syn_ack[0..6], GUEST_MAC.as_bytes());
        assert_eq!(&syn_ack[6..12], GATEWAY_MAC.as_bytes());
    }

    #[test]
    fn established_session_exposes_guest_http_bytes() {
        let (mut core, mut device, http) = core();
        let server_ack = complete_http_handshake(&mut core, &mut device, http);

        let request = b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n";
        device.push_rx(tcp_frame(
            49152,
            80,
            TcpControl::Psh,
            TcpSeqNumber(101),
            Some(server_ack),
            request,
        ));
        core.poll(Instant::from_millis(4), &mut device);

        assert_eq!(core.recv_available(http).expect("recv"), request);
    }

    #[test]
    fn writes_proxy_response_bytes_back_to_guest_session() {
        let (mut core, mut device, http) = core();
        complete_http_handshake(&mut core, &mut device, http);
        drain_tx(&mut device);

        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
        assert_eq!(
            core.send_to_session(http, response).expect("send"),
            response.len()
        );
        core.poll(Instant::from_millis(4), &mut device);

        let frame = device.pop_tx().expect("guest frame");
        assert_eq!(&frame[0..6], GUEST_MAC.as_bytes());
        let tcp = parse_tcp_reply(&frame);
        assert_eq!(tcp.payload, response);
    }

    fn complete_http_handshake(
        core: &mut GuestTcpCore,
        device: &mut QueuedEthernetDevice,
        http: SocketHandle,
    ) -> TcpSeqNumber {
        device.push_rx(tcp_frame(
            49152,
            80,
            TcpControl::Syn,
            TcpSeqNumber(100),
            None,
            &[],
        ));
        core.poll(Instant::from_millis(1), device);
        device.pop_tx().expect("arp request");
        device.push_rx(arp_reply_frame());
        core.poll(Instant::from_millis(2), device);
        let syn_ack_frame = device.pop_tx().expect("syn ack");
        let syn_ack = parse_tcp_reply(&syn_ack_frame);

        let server_ack = syn_ack.seq_number + 1;
        device.push_rx(tcp_frame(
            49152,
            80,
            TcpControl::None,
            TcpSeqNumber(101),
            Some(server_ack),
            &[],
        ));
        core.poll(Instant::from_millis(3), device);
        assert_eq!(
            core.session(http).expect("session").state,
            tcp::State::Established
        );
        server_ack
    }

    fn tcp_frame(
        src_port: u16,
        dst_port: u16,
        control: TcpControl,
        seq: TcpSeqNumber,
        ack: Option<TcpSeqNumber>,
        payload: &[u8],
    ) -> Vec<u8> {
        let tcp = TcpRepr {
            src_port,
            dst_port,
            control,
            seq_number: seq,
            ack_number: ack,
            window_len: 4096,
            window_scale: None,
            max_seg_size: (control == TcpControl::Syn).then_some(1460),
            sack_permitted: false,
            sack_ranges: [None, None, None],
            timestamp: None,
            payload,
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
        assert_eq!(ethernet.ethertype, EthernetProtocol::Ipv4);

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

    fn drain_tx(device: &mut QueuedEthernetDevice) {
        while device.pop_tx().is_some() {}
    }

    fn arp_reply_frame() -> Vec<u8> {
        let mut frame = Vec::with_capacity(42);
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&0x0806u16.to_be_bytes());
        frame.extend_from_slice(&1u16.to_be_bytes());
        frame.extend_from_slice(&0x0800u16.to_be_bytes());
        frame.push(6);
        frame.push(4);
        frame.extend_from_slice(&2u16.to_be_bytes());
        frame.extend_from_slice(GUEST_MAC.as_bytes());
        frame.extend_from_slice(&GUEST_IP.octets());
        frame.extend_from_slice(GATEWAY_MAC.as_bytes());
        frame.extend_from_slice(&Ipv4Address::new(10, 0, 2, 2).octets());
        frame
    }
}
