use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant as StdInstant};

use smoltcp::time::Instant;

use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{
    MappedTcpConnector, StdTcpConnector, TcpUpstreamConnector, UpstreamMapping,
};
use crate::tcp_proxy::{TcpProxyBridge, TcpProxyEvent};
use crate::vmnet_gateway::{VmnetGateway, VmnetGatewayError};
use crate::vmnet_stream::{FrameRead, QemuFrameIo, VmnetStreamEndpoint, VmnetStreamError};
use crate::GuestNetwork;

pub const DEFAULT_UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_VMNET_IDLE_SLEEP: Duration = Duration::from_millis(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetRuntimeConfig {
    pub socket_path: PathBuf,
    pub event_log_path: Option<PathBuf>,
    pub upstream_mappings: Vec<UpstreamMapping>,
    pub network: GuestNetwork,
    pub policy: VmnetPolicy,
    pub upstream_connect_timeout: Duration,
    pub idle_sleep: Duration,
}

impl VmnetRuntimeConfig {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        network: GuestNetwork,
        policy: VmnetPolicy,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            event_log_path: None,
            upstream_mappings: Vec::new(),
            network,
            policy,
            upstream_connect_timeout: DEFAULT_UPSTREAM_CONNECT_TIMEOUT,
            idle_sleep: DEFAULT_VMNET_IDLE_SLEEP,
        }
    }
}

#[derive(Debug, Default)]
pub struct VmnetRuntimeStats {
    pub guest_frames_read: usize,
    pub guest_frames_written: usize,
    pub proxy_events: Vec<TcpProxyEvent>,
}

#[derive(Debug, Default)]
pub struct VmnetProxyPump {
    pub guest_frames_written: usize,
    pub events: Vec<TcpProxyEvent>,
}

#[derive(Debug, Default)]
pub struct VmnetRuntimeTick {
    pub eof: bool,
    pub guest_frame_read: bool,
    pub guest_frames_written: usize,
    pub proxy_events: Vec<TcpProxyEvent>,
}

pub fn run_qemu_stream_until_eof<T, C, F>(
    frame_io: &mut QemuFrameIo<T>,
    gateway: &mut VmnetGateway<'_>,
    proxy: &mut TcpProxyBridge<C>,
    mut now: F,
) -> Result<VmnetRuntimeStats, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
    F: FnMut() -> Instant,
{
    let mut stats = VmnetRuntimeStats::default();
    while let Some(frame) = frame_io.read_frame()? {
        stats.guest_frames_read += 1;

        let result = gateway.handle_guest_frame(frame, now());
        write_guest_frames(frame_io, &result.guest_frames, &mut stats)?;

        let pump = pump_proxy_once(frame_io, gateway, proxy, now())?;
        stats.guest_frames_written += pump.guest_frames_written;
        stats.proxy_events.extend(pump.events);
    }
    Ok(stats)
}

pub fn run_qemu_stream_tick<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    gateway: &mut VmnetGateway<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
) -> Result<VmnetRuntimeTick, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut tick = VmnetRuntimeTick::default();
    match frame_io.try_read_frame()? {
        FrameRead::Frame(frame) => {
            tick.guest_frame_read = true;
            let result = gateway.handle_guest_frame(frame, now);
            let mut stats = VmnetRuntimeStats::default();
            write_guest_frames(frame_io, &result.guest_frames, &mut stats)?;
            tick.guest_frames_written += stats.guest_frames_written;
        }
        FrameRead::WouldBlock => {}
        FrameRead::Eof => tick.eof = true,
    }

    let pump = pump_proxy_once(frame_io, gateway, proxy, now)?;
    tick.guest_frames_written += pump.guest_frames_written;
    tick.proxy_events = pump.events;
    Ok(tick)
}

pub fn serve_vmnet_gateway(
    config: VmnetRuntimeConfig,
) -> Result<VmnetRuntimeStats, VmnetRuntimeError> {
    let endpoint = VmnetStreamEndpoint::bind(&config.socket_path)?;
    let mut frame_io = endpoint.accept_one_nonblocking()?;
    let started = StdInstant::now();
    let mut gateway = VmnetGateway::new(&config.policy, &config.network, smoltcp_now(started))?;
    let connector = MappedTcpConnector {
        base: StdTcpConnector {
            timeout: config.upstream_connect_timeout,
        },
        mappings: config.upstream_mappings.clone(),
    };
    let mut proxy = TcpProxyBridge::new(connector);
    let mut stats = VmnetRuntimeStats::default();
    let mut event_log = open_event_log(config.event_log_path.as_deref())?;

    loop {
        let tick = run_qemu_stream_tick(
            &mut frame_io,
            &mut gateway,
            &mut proxy,
            smoltcp_now(started),
        )?;
        if tick.eof {
            return Ok(stats);
        }
        if tick.guest_frame_read {
            stats.guest_frames_read += 1;
        }
        stats.guest_frames_written += tick.guest_frames_written;
        let idle = !tick.guest_frame_read
            && tick.guest_frames_written == 0
            && tick.proxy_events.is_empty();
        write_proxy_events(&mut event_log, &tick.proxy_events)?;
        stats.proxy_events.extend(tick.proxy_events);
        if idle {
            thread::sleep(config.idle_sleep);
        }
    }
}

pub fn pump_proxy_once<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    gateway: &mut VmnetGateway<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
) -> Result<VmnetProxyPump, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut pump = VmnetProxyPump::default();
    for event in proxy.process_gateway(gateway, now) {
        if let TcpProxyEvent::UpstreamPayload { guest_frames, .. } = &event {
            for frame in guest_frames {
                frame_io.write_frame(frame)?;
                pump.guest_frames_written += 1;
            }
        }
        pump.events.push(event);
    }
    Ok(pump)
}

fn open_event_log(path: Option<&std::path::Path>) -> io::Result<Option<File>> {
    path.map(|path| OpenOptions::new().create(true).append(true).open(path))
        .transpose()
}

fn write_proxy_events(log: &mut Option<File>, events: &[TcpProxyEvent]) -> io::Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    for event in events {
        writeln!(log, "{}", format_proxy_event(event))?;
    }
    log.flush()
}

fn format_proxy_event(event: &TcpProxyEvent) -> String {
    match event {
        TcpProxyEvent::Connected {
            handle,
            destination,
            decision,
        } => format!(
            "tcp_connected handle={handle:?} dst={}:{} action={:?} reason={}",
            destination.ip, destination.port, decision.action, decision.reason
        ),
        TcpProxyEvent::Denied {
            handle,
            destination,
            decision,
        } => format!(
            "tcp_denied handle={handle:?} dst={}:{} action={:?} reason={}",
            destination.ip, destination.port, decision.action, decision.reason
        ),
        TcpProxyEvent::ConnectFailed {
            handle,
            destination,
            error,
        } => format!(
            "tcp_connect_failed handle={handle:?} dst={}:{} error={error:?}",
            destination.ip, destination.port
        ),
        TcpProxyEvent::GuestPayload { handle, bytes } => {
            format!("guest_payload handle={handle:?} bytes={bytes}")
        }
        TcpProxyEvent::HttpRequest {
            handle,
            destination,
            summary,
        } => format!(
            "http_request handle={handle:?} dst={}:{} method={} host={} path={}",
            destination.ip,
            destination.port,
            summary.method,
            summary.host.as_deref().unwrap_or("-"),
            summary.path
        ),
        TcpProxyEvent::HttpRequestIncomplete { handle } => {
            format!("http_request_incomplete handle={handle:?}")
        }
        TcpProxyEvent::HttpRequestMalformed { handle } => {
            format!("http_request_malformed handle={handle:?}")
        }
        TcpProxyEvent::UpstreamPayload {
            handle,
            bytes,
            guest_frames,
        } => format!(
            "upstream_payload handle={handle:?} bytes={bytes} guest_frames={}",
            guest_frames.len()
        ),
        TcpProxyEvent::GuestReadFailed { handle, error } => {
            format!("guest_read_failed handle={handle:?} error={error:?}")
        }
        TcpProxyEvent::GuestWriteFailed { handle, error } => {
            format!("guest_write_failed handle={handle:?} error={error:?}")
        }
        TcpProxyEvent::UpstreamWriteFailed { handle, error } => {
            format!("upstream_write_failed handle={handle:?} error={error}")
        }
        TcpProxyEvent::UpstreamReadFailed { handle, error } => {
            format!("upstream_read_failed handle={handle:?} error={error}")
        }
    }
}

fn write_guest_frames<T>(
    frame_io: &mut QemuFrameIo<T>,
    frames: &[Vec<u8>],
    stats: &mut VmnetRuntimeStats,
) -> Result<(), VmnetRuntimeError>
where
    T: Read + Write,
{
    for frame in frames {
        frame_io.write_frame(frame)?;
        stats.guest_frames_written += 1;
    }
    Ok(())
}

#[derive(Debug)]
pub enum VmnetRuntimeError {
    Io(io::Error),
    Stream(VmnetStreamError),
    Gateway(VmnetGatewayError),
}

impl From<io::Error> for VmnetRuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<VmnetStreamError> for VmnetRuntimeError {
    fn from(error: VmnetStreamError) -> Self {
        Self::Stream(error)
    }
}

impl From<VmnetGatewayError> for VmnetRuntimeError {
    fn from(error: VmnetGatewayError) -> Self {
        Self::Gateway(error)
    }
}

fn smoltcp_now(started: StdInstant) -> Instant {
    let elapsed = started.elapsed();
    Instant::from_millis(elapsed.as_millis().min(i64::MAX as u128) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_policy::VmnetPolicy;
    use crate::tcp_gateway::{TcpConnectError, TcpDestination};
    use crate::tcp_proxy::TcpProxyBridge;
    use crate::vmnet_gateway::VmnetGateway;
    use crate::vmnet_stream::DEFAULT_MAX_FRAME_LEN;
    use crate::GuestNetwork;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind};

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(crate::guest_tcp::DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    #[test]
    fn stream_runtime_pumps_gateway_and_http_proxy_until_eof() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            block_reads: 0,
        });
        let mut io = QemuFrameIo::new(ScriptedIo::new(), DEFAULT_MAX_FRAME_LEN);
        let mut millis = 1;

        let stats = run_qemu_stream_until_eof(&mut io, &mut gateway, &mut proxy, || {
            let now = Instant::from_millis(millis);
            millis += 1;
            now
        })
        .expect("runtime");

        assert_eq!(stats.guest_frames_read, 4);
        assert!(stats.guest_frames_written >= 3);
        assert!(stats
            .proxy_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::HttpRequest { .. })));
        assert!(stats
            .proxy_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamPayload { .. })));
        let log_path = unique_temp_file("vmnet-events.log");
        let mut event_log = open_event_log(Some(&log_path)).expect("event log");
        write_proxy_events(&mut event_log, &stats.proxy_events).expect("write event log");
        let event_log = std::fs::read_to_string(log_path).expect("read event log");
        assert!(event_log.contains("tcp_connected"));
        assert!(event_log.contains("http_request"));
        assert!(event_log.contains("upstream_payload"));
        assert!(!io.into_inner().written.is_empty());
    }

    #[test]
    fn proxy_pump_can_deliver_delayed_upstream_response_without_guest_frame() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            block_reads: 2,
        });
        let mut io = QemuFrameIo::new(ScriptedIo::new(), DEFAULT_MAX_FRAME_LEN);
        let mut millis = 1;

        let stats = run_qemu_stream_until_eof(&mut io, &mut gateway, &mut proxy, || {
            let now = Instant::from_millis(millis);
            millis += 1;
            now
        })
        .expect("runtime");
        assert!(!stats
            .proxy_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamPayload { .. })));

        let pump = pump_proxy_once(
            &mut io,
            &mut gateway,
            &mut proxy,
            Instant::from_millis(millis),
        )
        .expect("pump");

        assert!(pump.guest_frames_written > 0);
        assert!(pump
            .events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamPayload { .. })));
    }

    #[derive(Debug, Clone)]
    struct FakeConnector {
        response: Vec<u8>,
        block_reads: usize,
    }

    impl TcpUpstreamConnector for FakeConnector {
        type Connection = MemoryConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Ok(MemoryConnection {
                response: self.response.clone(),
                written: Vec::new(),
                block_reads: self.block_reads,
            })
        }
    }

    #[derive(Debug)]
    struct MemoryConnection {
        response: Vec<u8>,
        written: Vec<u8>,
        block_reads: usize,
    }

    impl Read for MemoryConnection {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.block_reads > 0 {
                self.block_reads -= 1;
                return Err(io::Error::from(ErrorKind::WouldBlock));
            }
            if self.response.is_empty() {
                return Err(io::Error::from(ErrorKind::WouldBlock));
            }
            let count = self.response.len().min(buf.len());
            buf[..count].copy_from_slice(&self.response[..count]);
            self.response.drain(..count);
            Ok(count)
        }
    }

    impl Write for MemoryConnection {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct ScriptedIo {
        read: VecDeque<u8>,
        written: Vec<u8>,
        observed_writes: Vec<u8>,
        injected_arp_reply: bool,
        injected_http_request: bool,
    }

    impl ScriptedIo {
        fn new() -> Self {
            let read = frame_bytes(&tcp_frame(
                80,
                TcpControl::Syn,
                TcpSeqNumber(100),
                None,
                &[],
            ))
            .into();
            Self {
                read,
                written: Vec::new(),
                observed_writes: Vec::new(),
                injected_arp_reply: false,
                injected_http_request: false,
            }
        }

        fn enqueue_frame(&mut self, frame: Vec<u8>) {
            self.read.extend(frame_bytes(&frame));
        }

        fn observe_host_write(&mut self, bytes: &[u8]) {
            self.observed_writes.extend_from_slice(bytes);
            loop {
                if self.observed_writes.len() < 4 {
                    return;
                }
                let len = u32::from_be_bytes(
                    self.observed_writes[0..4]
                        .try_into()
                        .expect("length prefix"),
                ) as usize;
                if self.observed_writes.len() < 4 + len {
                    return;
                }
                let frame = self.observed_writes[4..4 + len].to_vec();
                self.observed_writes.drain(..4 + len);
                self.react_to_host_frame(&frame);
            }
        }

        fn react_to_host_frame(&mut self, frame: &[u8]) {
            if !self.injected_arp_reply && is_arp_request(frame) {
                self.injected_arp_reply = true;
                self.enqueue_frame(arp_reply_frame());
                return;
            }

            if self.injected_http_request {
                return;
            }
            let Some(tcp) = parse_tcp_reply(frame) else {
                return;
            };
            if tcp.control != TcpControl::Syn || tcp.ack_number.is_none() {
                return;
            }
            self.injected_http_request = true;
            let server_ack = tcp.seq_number + 1;
            self.enqueue_frame(tcp_frame(
                80,
                TcpControl::None,
                TcpSeqNumber(101),
                Some(server_ack),
                &[],
            ));
            self.enqueue_frame(tcp_frame(
                80,
                TcpControl::Psh,
                TcpSeqNumber(101),
                Some(server_ack),
                b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n",
            ));
        }
    }

    impl Read for ScriptedIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = self.read.len().min(buf.len());
            for slot in &mut buf[..count] {
                *slot = self.read.pop_front().expect("queued byte");
            }
            Ok(count)
        }
    }

    impl Write for ScriptedIo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            self.observe_host_write(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn frame_bytes(frame: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + frame.len());
        bytes.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        bytes.extend_from_slice(frame);
        bytes
    }

    fn tcp_frame(
        dst_port: u16,
        control: TcpControl,
        seq: TcpSeqNumber,
        ack: Option<TcpSeqNumber>,
        payload: &[u8],
    ) -> Vec<u8> {
        let tcp = TcpRepr {
            src_port: 49152,
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
        frame.extend_from_slice(&GATEWAY_IP.octets());
        frame
    }

    fn is_arp_request(frame: &[u8]) -> bool {
        frame.len() >= 22
            && frame[0..6] == [0xff; 6]
            && u16::from_be_bytes([frame[12], frame[13]]) == 0x0806
            && u16::from_be_bytes([frame[20], frame[21]]) == 1
    }

    fn unique_temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentvm-frontend-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn parse_tcp_reply(frame: &[u8]) -> Option<TcpRepr<'_>> {
        let ethernet = EthernetFrame::new_checked(frame).ok()?;
        let ethernet = EthernetRepr::parse(&ethernet).ok()?;
        if ethernet.ethertype != EthernetProtocol::Ipv4 {
            return None;
        }
        let ip_offset = ethernet.buffer_len();
        let ipv4 = Ipv4Packet::new_checked(&frame[ip_offset..]).ok()?;
        let ipv4 = Ipv4Repr::parse(&ipv4, &ChecksumCapabilities::default()).ok()?;
        let tcp_offset = ip_offset + ipv4.buffer_len();
        let tcp = TcpPacket::new_checked(&frame[tcp_offset..]).ok()?;
        TcpRepr::parse(
            &tcp,
            &IpAddress::Ipv4(ipv4.src_addr),
            &IpAddress::Ipv4(ipv4.dst_addr),
            &ChecksumCapabilities::default(),
        )
        .ok()
    }
}
