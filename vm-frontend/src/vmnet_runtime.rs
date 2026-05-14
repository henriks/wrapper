use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant as StdInstant};

use smoltcp::time::Instant;

use crate::host_ingress::{HostIngressBridge, HostIngressEvent, HostIngressListenerSet};
use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{
    MappedTcpConnector, StdTcpConnector, TcpUpstreamConnector, UpstreamMapping,
};
use crate::tcp_proxy::{TcpProxyBridge, TcpProxyEvent};
use crate::tls_mitm::{TlsMitmAuthority, TlsMitmError};
use crate::vmnet_gateway::{
    GuestFrameOutcome, UdpDenial, UnsupportedProtocol, VmnetGateway, VmnetGatewayError,
};
use crate::vmnet_stream::{
    FrameRead, PcapWriter, QemuFrameIo, VmnetStreamEndpoint, VmnetStreamError,
};
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
    pub gateway_events: Vec<VmnetGatewayEvent>,
    pub proxy_events: Vec<TcpProxyEvent>,
    pub host_ingress_events: Vec<HostIngressEvent>,
}

#[derive(Debug, Default)]
pub struct VmnetProxyPump {
    pub guest_frames_written: usize,
    pub events: Vec<TcpProxyEvent>,
}

#[derive(Debug, Default)]
pub struct VmnetHostIngressPump {
    pub guest_frames_written: usize,
    pub events: Vec<HostIngressEvent>,
}

#[derive(Debug, Default)]
pub struct VmnetRuntimeTick {
    pub eof: bool,
    pub guest_frame_read: bool,
    pub guest_frames_written: usize,
    pub captured_frames: usize,
    pub gateway_events: Vec<VmnetGatewayEvent>,
    pub proxy_events: Vec<TcpProxyEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetGatewayEvent {
    DnsQuery {
        log: crate::dns_proxy::DnsLogEntry,
    },
    UdpDenied(UdpDenial),
    UnsupportedProtocol(UnsupportedProtocol),
    TcpDenied {
        destination: crate::tcp_gateway::TcpDestination,
        decision: crate::tcp_gateway::TcpDecision,
    },
    TcpSetupFailed {
        destination: crate::tcp_gateway::TcpDestination,
        detail: String,
    },
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
        if let Some(event) = gateway_event_from_outcome(&result.outcome) {
            stats.gateway_events.push(event);
        }
        write_guest_frames(frame_io, &result.guest_frames, &mut stats, None)?;

        let pump = pump_proxy_once(frame_io, gateway, proxy, now(), None)?;
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
    mut pcap: Option<&mut PcapWriter>,
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
            capture_frame(pcap.as_deref_mut(), &frame)?;
            tick.captured_frames += usize::from(pcap.is_some());
            let result = gateway.handle_guest_frame(frame, now);
            if let Some(event) = gateway_event_from_outcome(&result.outcome) {
                tick.gateway_events.push(event);
            }
            let mut stats = VmnetRuntimeStats::default();
            let captured = write_guest_frames(
                frame_io,
                &result.guest_frames,
                &mut stats,
                pcap.as_deref_mut(),
            )?;
            tick.guest_frames_written += stats.guest_frames_written;
            tick.captured_frames += captured;
        }
        FrameRead::WouldBlock => {}
        FrameRead::Eof => tick.eof = true,
    }

    let pump = pump_proxy_once(frame_io, gateway, proxy, now, pcap.as_deref_mut())?;
    tick.guest_frames_written += pump.guest_frames_written;
    tick.captured_frames += pump.guest_frames_written;
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
    let mut proxy = tcp_proxy_bridge_from_policy(connector, &config.policy)?;
    let mut host_ingress = HostIngressBridge::new();
    let host_listeners = HostIngressListenerSet::bind(&config.policy.host_listeners)?;
    let mut stats = VmnetRuntimeStats::default();
    let mut event_log = open_event_log(config.event_log_path.as_deref())?;
    let mut pcap = open_pcap_capture(&config)?;

    loop {
        let tick = run_qemu_stream_tick(
            &mut frame_io,
            &mut gateway,
            &mut proxy,
            smoltcp_now(started),
            pcap.as_mut(),
        )?;
        if tick.eof {
            return Ok(stats);
        }
        if tick.guest_frame_read {
            stats.guest_frames_read += 1;
        }
        stats.guest_frames_written += tick.guest_frames_written;
        let host_pump = pump_host_ingress_once(
            &mut frame_io,
            &mut gateway,
            &mut host_ingress,
            &host_listeners,
            smoltcp_now(started),
            pcap.as_mut(),
        )?;
        stats.guest_frames_written += host_pump.guest_frames_written;
        let idle = !tick.guest_frame_read
            && tick.guest_frames_written == 0
            && tick.gateway_events.is_empty()
            && tick.proxy_events.is_empty()
            && host_pump.events.is_empty();
        write_gateway_events(&mut event_log, &tick.gateway_events)?;
        write_proxy_events(&mut event_log, &tick.proxy_events)?;
        write_host_ingress_events(&mut event_log, &host_pump.events)?;
        stats.gateway_events.extend(tick.gateway_events);
        stats.proxy_events.extend(tick.proxy_events);
        stats.host_ingress_events.extend(host_pump.events);
        if idle {
            thread::sleep(config.idle_sleep);
        }
    }
}

fn tcp_proxy_bridge_from_policy<C>(
    connector: C,
    policy: &VmnetPolicy,
) -> Result<TcpProxyBridge<C>, VmnetRuntimeError>
where
    C: TcpUpstreamConnector,
{
    let Some(ca_cert_path) = policy.tls_mitm.ca_cert_path.as_ref() else {
        return Ok(TcpProxyBridge::new(connector));
    };
    let Some(ca_key_path) = policy.tls_mitm.ca_key_path.as_ref() else {
        return Ok(TcpProxyBridge::new(connector));
    };
    if !policy.tls_mitm.generate_per_host_certs {
        return Ok(TcpProxyBridge::new(connector));
    }
    let authority = Arc::new(TlsMitmAuthority::from_files(ca_cert_path, ca_key_path)?);
    TcpProxyBridge::with_tls_mitm(connector, authority).map_err(VmnetRuntimeError::from)
}

pub fn pump_proxy_once<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    gateway: &mut VmnetGateway<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetProxyPump, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut pump = VmnetProxyPump::default();
    for event in proxy.process_gateway(gateway, now) {
        if let TcpProxyEvent::UpstreamPayload { guest_frames, .. }
        | TcpProxyEvent::TlsHandshakePayload { guest_frames, .. } = &event
        {
            for frame in guest_frames {
                capture_frame(pcap.as_deref_mut(), frame)?;
                frame_io.write_frame(frame)?;
                pump.guest_frames_written += 1;
            }
        }
        pump.events.push(event);
    }
    Ok(pump)
}

pub fn pump_host_ingress_once<T>(
    frame_io: &mut QemuFrameIo<T>,
    gateway: &mut VmnetGateway<'_>,
    bridge: &mut HostIngressBridge<std::net::TcpStream>,
    listeners: &HostIngressListenerSet,
    now: Instant,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetHostIngressPump, VmnetRuntimeError>
where
    T: Read + Write,
{
    let mut pump = VmnetHostIngressPump::default();
    for accepted in listeners.accept_pending() {
        let accepted = accepted?;
        match bridge.open_session(gateway, accepted.guest_port, accepted.connection, now) {
            Ok(open) => {
                for frame in &open.guest_frames {
                    capture_frame(pcap.as_deref_mut(), frame)?;
                    frame_io.write_frame(frame)?;
                    pump.guest_frames_written += 1;
                }
                pump.events.push(HostIngressEvent::Opened {
                    handle: open.handle,
                    guest_port: open.guest_port,
                    purpose: accepted.purpose,
                });
            }
            Err(error) => pump.events.push(HostIngressEvent::OpenFailed {
                guest_port: accepted.guest_port,
                purpose: accepted.purpose,
                error: format!("{error:?}"),
            }),
        }
    }

    for event in bridge.process_gateway(gateway, now) {
        if let HostIngressEvent::HostPayload { guest_frames, .. }
        | HostIngressEvent::HostClosed { guest_frames, .. } = &event
        {
            for frame in guest_frames {
                capture_frame(pcap.as_deref_mut(), frame)?;
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

fn open_pcap_capture(config: &VmnetRuntimeConfig) -> io::Result<Option<PcapWriter>> {
    if !config.policy.capture.capture_guest_side_frames {
        return Ok(None);
    }
    config
        .policy
        .capture
        .pcap_path
        .as_ref()
        .map(|path| PcapWriter::create(path, 65_535))
        .transpose()
}

fn capture_frame(pcap: Option<&mut PcapWriter>, frame: &[u8]) -> Result<(), VmnetRuntimeError> {
    if let Some(pcap) = pcap {
        pcap.write_ethernet_frame(frame)?;
    }
    Ok(())
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

fn write_gateway_events(log: &mut Option<File>, events: &[VmnetGatewayEvent]) -> io::Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    for event in events {
        writeln!(log, "{}", format_gateway_event(event))?;
    }
    log.flush()
}

fn format_gateway_event(event: &VmnetGatewayEvent) -> String {
    match event {
        VmnetGatewayEvent::DnsQuery { log } => format!(
            "dns_query domain={} decision={:?} detail={}",
            log.domain.as_deref().unwrap_or("-"),
            log.decision,
            log.detail
        ),
        VmnetGatewayEvent::UdpDenied(denial) => format!(
            "udp_denied src={}:{} dst={}:{} reason={}",
            std::net::Ipv4Addr::from(denial.src_ip),
            denial.src_port,
            std::net::Ipv4Addr::from(denial.dst_ip),
            denial.dst_port,
            denial.reason
        ),
        VmnetGatewayEvent::UnsupportedProtocol(unsupported) => {
            format!("unsupported_protocol reason={}", unsupported.reason)
        }
        VmnetGatewayEvent::TcpDenied {
            destination,
            decision,
        } => format!(
            "tcp_denied_preaccept dst={}:{} action={:?} reason={}",
            destination.ip, destination.port, decision.action, decision.reason
        ),
        VmnetGatewayEvent::TcpSetupFailed {
            destination,
            detail,
        } => format!(
            "tcp_setup_failed_preaccept dst={}:{} detail={}",
            destination.ip, destination.port, detail
        ),
    }
}

fn gateway_event_from_outcome(outcome: &GuestFrameOutcome) -> Option<VmnetGatewayEvent> {
    match outcome {
        GuestFrameOutcome::DnsQuery { log } => {
            Some(VmnetGatewayEvent::DnsQuery { log: log.clone() })
        }
        GuestFrameOutcome::UdpDenied(denial) => Some(VmnetGatewayEvent::UdpDenied(denial.clone())),
        GuestFrameOutcome::UnsupportedProtocol(unsupported) => {
            Some(VmnetGatewayEvent::UnsupportedProtocol(unsupported.clone()))
        }
        GuestFrameOutcome::TcpDenied {
            destination,
            decision,
        } => Some(VmnetGatewayEvent::TcpDenied {
            destination: destination.clone(),
            decision: decision.clone(),
        }),
        GuestFrameOutcome::TcpSetupFailed {
            destination,
            detail,
        } => Some(VmnetGatewayEvent::TcpSetupFailed {
            destination: destination.clone(),
            detail: detail.clone(),
        }),
        GuestFrameOutcome::L2Response
        | GuestFrameOutcome::TcpAccepted { .. }
        | GuestFrameOutcome::TcpProgress
        | GuestFrameOutcome::Ignored => None,
    }
}

fn write_host_ingress_events(
    log: &mut Option<File>,
    events: &[HostIngressEvent],
) -> io::Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    for event in events {
        writeln!(log, "{}", format_host_ingress_event(event))?;
    }
    log.flush()
}

fn format_host_ingress_event(event: &HostIngressEvent) -> String {
    match event {
        HostIngressEvent::Opened {
            handle,
            guest_port,
            purpose,
        } => {
            format!("host_ingress_opened handle={handle:?} guest_port={guest_port} purpose={purpose:?}")
        }
        HostIngressEvent::OpenFailed {
            guest_port,
            purpose,
            error,
        } => {
            format!("host_ingress_open_failed guest_port={guest_port} purpose={purpose:?} error={error}")
        }
        HostIngressEvent::HostPayload {
            handle,
            guest_port,
            bytes,
            guest_frames,
        } => format!(
            "host_ingress_host_payload handle={handle:?} guest_port={guest_port} bytes={bytes} guest_frames={}",
            guest_frames.len()
        ),
        HostIngressEvent::GuestPayload {
            handle,
            guest_port,
            bytes,
        } => {
            format!("host_ingress_guest_payload handle={handle:?} guest_port={guest_port} bytes={bytes}")
        }
        HostIngressEvent::HostClosed {
            handle,
            guest_port,
            guest_frames,
        } => format!(
            "host_ingress_host_closed handle={handle:?} guest_port={guest_port} guest_frames={}",
            guest_frames.len()
        ),
        HostIngressEvent::GuestClosed {
            handle,
            guest_port,
            state,
        } => format!(
            "host_ingress_guest_closed handle={handle:?} guest_port={guest_port} state={state:?}"
        ),
        HostIngressEvent::GuestReadFailed { handle, error } => {
            format!("host_ingress_guest_read_failed handle={handle:?} error={error:?}")
        }
        HostIngressEvent::GuestWriteFailed { handle, error } => {
            format!("host_ingress_guest_write_failed handle={handle:?} error={error:?}")
        }
        HostIngressEvent::HostReadFailed { handle, error } => {
            format!("host_ingress_host_read_failed handle={handle:?} error={error}")
        }
        HostIngressEvent::HostWriteFailed { handle, error } => {
            format!("host_ingress_host_write_failed handle={handle:?} error={error}")
        }
    }
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
        TcpProxyEvent::TlsHandshakePayload {
            handle,
            bytes,
            guest_frames,
        } => format!(
            "tls_handshake_payload handle={handle:?} bytes={bytes} guest_frames={}",
            guest_frames.len()
        ),
        TcpProxyEvent::TlsUpstreamPayload { handle, bytes } => {
            format!("tls_upstream_payload handle={handle:?} bytes={bytes}")
        }
        TcpProxyEvent::TlsMitmUnavailable {
            handle,
            destination,
        } => format!(
            "tls_mitm_unavailable handle={handle:?} dst={}:{}",
            destination.ip, destination.port
        ),
        TcpProxyEvent::TlsMitmFailed { handle, error } => {
            format!("tls_mitm_failed handle={handle:?} error={error:?}")
        }
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
    mut pcap: Option<&mut PcapWriter>,
) -> Result<usize, VmnetRuntimeError>
where
    T: Read + Write,
{
    let mut captured = 0;
    for frame in frames {
        capture_frame(pcap.as_deref_mut(), frame)?;
        captured += usize::from(pcap.is_some());
        frame_io.write_frame(frame)?;
        stats.guest_frames_written += 1;
    }
    Ok(captured)
}

#[derive(Debug)]
pub enum VmnetRuntimeError {
    Io(io::Error),
    Stream(VmnetStreamError),
    Gateway(VmnetGatewayError),
    TlsMitm(TlsMitmError),
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

impl From<TlsMitmError> for VmnetRuntimeError {
    fn from(error: TlsMitmError) -> Self {
        Self::TlsMitm(error)
    }
}

fn smoltcp_now(started: StdInstant) -> Instant {
    let elapsed = started.elapsed();
    Instant::from_millis(elapsed.as_millis().min(i64::MAX as u128) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns_proxy::{DnsDecision, DnsLogEntry};
    use crate::host_ingress::HostIngressEvent;
    use crate::network_policy::{HostListenerPurpose, VmnetPolicy};
    use crate::tcp_gateway::{TcpAction, TcpConnectError, TcpDecision, TcpDestination};
    use crate::tcp_proxy::TcpProxyBridge;
    use crate::tls_mitm::TlsMitmError;
    use crate::vmnet_gateway::{UdpDenial, UnsupportedProtocol, VmnetGateway};
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
            None,
        )
        .expect("pump");

        assert!(pump.guest_frames_written > 0);
        assert!(pump
            .events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamPayload { .. })));
    }

    #[test]
    fn unsupported_protocol_gateway_event_has_log_detail() {
        let event = VmnetGatewayEvent::UnsupportedProtocol(UnsupportedProtocol {
            reason: "IPv6 DenyAndLog by policy".to_string(),
        });

        assert_eq!(
            format_gateway_event(&event),
            "unsupported_protocol reason=IPv6 DenyAndLog by policy"
        );
    }

    #[test]
    fn event_log_includes_representative_failure_artifacts_without_secrets() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let handle = gateway
            .connect_host_to_guest(1075, 40000, Instant::from_millis(1))
            .expect("host ingress handle")
            .handle;
        let destination = TcpDestination {
            ip: std::net::Ipv4Addr::new(169, 254, 169, 254),
            port: 443,
            domain: Some("metadata.invalid".to_string()),
        };
        let decision = TcpDecision {
            action: TcpAction::Deny,
            reason: "metadata range denied".to_string(),
        };
        let log_path = unique_temp_file("vmnet-failures.log");
        let mut event_log = open_event_log(Some(&log_path)).expect("event log");

        write_gateway_events(
            &mut event_log,
            &[
                VmnetGatewayEvent::DnsQuery {
                    log: DnsLogEntry {
                        domain: Some("blocked.example".to_string()),
                        decision: DnsDecision::Blocked,
                        detail: "domain denied by VmnetPolicy".to_string(),
                    },
                },
                VmnetGatewayEvent::UdpDenied(UdpDenial {
                    src_ip: [10, 0, 2, 15],
                    dst_ip: [93, 184, 216, 34],
                    src_port: 53000,
                    dst_port: 443,
                    reason: "UDP/443 blocked to prevent QUIC bypass".to_string(),
                }),
                VmnetGatewayEvent::UnsupportedProtocol(UnsupportedProtocol {
                    reason: "IPv6 DenyAndLog by policy".to_string(),
                }),
                VmnetGatewayEvent::TcpDenied {
                    destination: destination.clone(),
                    decision: decision.clone(),
                },
                VmnetGatewayEvent::TcpSetupFailed {
                    destination: destination.clone(),
                    detail: "listener provisioning failed".to_string(),
                },
            ],
        )
        .expect("gateway events");
        write_proxy_events(
            &mut event_log,
            &[
                TcpProxyEvent::TlsMitmUnavailable {
                    handle,
                    destination: destination.clone(),
                },
                TcpProxyEvent::TlsMitmFailed {
                    handle,
                    error: TlsMitmError::Tls("bad record mac".to_string()),
                },
                TcpProxyEvent::UpstreamWriteFailed {
                    handle,
                    error: "broken pipe".to_string(),
                },
            ],
        )
        .expect("proxy events");
        write_host_ingress_events(
            &mut event_log,
            &[
                HostIngressEvent::OpenFailed {
                    guest_port: 1075,
                    purpose: HostListenerPurpose::DockerApi,
                    error: "connection refused".to_string(),
                },
                HostIngressEvent::HostWriteFailed {
                    handle,
                    error: "broken pipe".to_string(),
                },
            ],
        )
        .expect("host ingress events");

        let log = std::fs::read_to_string(log_path).expect("read event log");
        assert!(log.contains("dns_query domain=blocked.example decision=Blocked"));
        assert!(log.contains("udp_denied src=10.0.2.15:53000 dst=93.184.216.34:443"));
        assert!(log.contains("unsupported_protocol reason=IPv6 DenyAndLog by policy"));
        assert!(log.contains("tcp_denied_preaccept dst=169.254.169.254:443"));
        assert!(log.contains("tcp_setup_failed_preaccept dst=169.254.169.254:443"));
        assert!(log.contains("tls_mitm_unavailable"));
        assert!(log.contains("tls_mitm_failed"));
        assert!(log.contains("upstream_write_failed"));
        assert!(log.contains("host_ingress_open_failed guest_port=1075 purpose=DockerApi"));
        assert!(log.contains("host_ingress_host_write_failed"));
        assert!(!log.contains("BEGIN PRIVATE KEY"));
        assert!(!log.contains("mitm-ca.key"));
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
