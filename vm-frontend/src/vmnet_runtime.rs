use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant as StdInstant};

use smoltcp::time::Instant;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, info_span, warn};

use crate::dns_proxy::DnsUpstreamError;
use crate::host_ingress::{
    AcceptedHostConnection, HostIngressAcceptedQueue, HostIngressBridge, HostIngressEvent,
    HostIngressListenerSet, HostIngressReadiness, DEFAULT_HOST_INGRESS_ACCEPTS_PER_LISTENER_PUMP,
    DEFAULT_HOST_INGRESS_ACCEPT_QUEUE_LIMIT,
};
use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{
    MappedTcpConnector, StdTcpConnector, TcpConnectError, TcpUpstreamConnector, UpstreamMapping,
};
use crate::tcp_proxy::{
    TcpProxyBridge, TcpProxyConnectPlan, TcpProxyEvent, TcpProxyPendingConnect, TcpProxyReadiness,
};
use crate::tls_mitm::{TlsMitmAuthority, TlsMitmError};
use crate::vmnet_gateway::{
    default_dns_upstream, DnsFrameResult, GuestFrameOutcome, GuestFrameResult, UdpDenial,
    UnsupportedProtocol, VmnetDeferredDnsFrame, VmnetGateway, VmnetGatewayError,
    VmnetPendingDnsQuery,
};
use crate::vmnet_service_io::{
    spawn_dns_service_task, spawn_tcp_connect_service_task, VmnetAsyncDnsWorkerHandle,
    VmnetAsyncTcpConnectWorkerHandle, VmnetServiceCompletion, VmnetServiceIoLimitError,
    VmnetServiceIoLimits, VmnetServiceOwner, VmnetServiceToken,
};
use crate::vmnet_stream::{PcapWriter, QemuFrameIo, VmnetStreamEndpoint, VmnetStreamError};
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
    pub service_io_limits: VmnetServiceIoLimits,
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
            service_io_limits: VmnetServiceIoLimits::default(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum VmnetEventSource {
    HostListener(usize),
    HostSession(smoltcp::iface::SocketHandle),
    UpstreamSession(smoltcp::iface::SocketHandle),
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

pub struct VmnetCore<'a> {
    gateway: VmnetGateway<'a>,
}

impl<'a> VmnetCore<'a> {
    pub fn new(gateway: VmnetGateway<'a>) -> Self {
        Self { gateway }
    }

    pub fn handle_guest_frame(&mut self, frame: Vec<u8>, now: Instant) -> GuestFrameResult {
        self.gateway.handle_guest_frame(frame, now)
    }

    fn handle_guest_frame_with_deferred_dns(
        &mut self,
        frame: Vec<u8>,
        now: Instant,
    ) -> VmnetDeferredDnsFrame {
        self.gateway
            .handle_guest_frame_with_deferred_dns(frame, now)
    }

    fn complete_pending_dns_query(
        &mut self,
        pending: VmnetPendingDnsQuery,
        exchange: Result<hickory_proto::op::Message, DnsUpstreamError>,
    ) -> GuestFrameResult {
        dns_frame_result_to_guest(self.gateway.complete_pending_dns_query(pending, exchange))
    }

    pub fn poll_tcp(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.gateway.poll_tcp(now)
    }

    pub fn tcp_poll_delay(&mut self, now: Instant) -> Option<Duration> {
        self.gateway.tcp_poll_delay(now)
    }

    // Narrow escape hatch for synchronous driver adapters that still need to
    // call existing proxy/host-ingress APIs. Keep this private to this module:
    // new runtime code should prefer explicit VmnetCore methods.
    fn gateway_mut(&mut self) -> &mut VmnetGateway<'a> {
        &mut self.gateway
    }
}

#[cfg(test)]
fn run_qemu_stream_until_eof<T, C, F>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
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

        let result = core.handle_guest_frame(frame, now());
        if let Some(event) = gateway_event_from_outcome(&result.outcome) {
            stats.gateway_events.push(event);
        }
        write_guest_frames(frame_io, &result.guest_frames, &mut stats, None)?;

        let pump = pump_proxy_once(frame_io, core, proxy, now(), None)?;
        stats.guest_frames_written += pump.guest_frames_written;
        stats.proxy_events.extend(pump.events);
    }
    Ok(stats)
}

pub async fn serve_vmnet_gateway_async(
    config: VmnetRuntimeConfig,
) -> Result<VmnetRuntimeStats, VmnetRuntimeError> {
    serve_vmnet_gateway_async_owner(config).await
}

async fn serve_vmnet_gateway_async_owner(
    config: VmnetRuntimeConfig,
) -> Result<VmnetRuntimeStats, VmnetRuntimeError> {
    let span = info_span!(
        "vmnet.runtime",
        socket = %config.socket_path.display(),
        event_log = ?config.event_log_path,
        upstream_mapping_count = config.upstream_mappings.len(),
        host_listener_count = config.policy.host_listeners.len(),
        default_egress_action = ?config.policy.egress.default_action,
    );
    let _span_guard = span.enter();
    let host_listeners = HostIngressListenerSet::bind(&config.policy.host_listeners)?;
    info!(
        host_listener_count = config.policy.host_listeners.len(),
        "vmnet host listeners bound"
    );
    info!("binding vmnet gateway socket");
    let endpoint = VmnetStreamEndpoint::bind(&config.socket_path)?;
    let mut frame_io = endpoint.accept_one_tokio().await?;
    info!("accepted qemu vmnet stream");
    let started = StdInstant::now();
    let gateway = VmnetGateway::new_with_dns_upstream(
        &config.policy,
        &config.network,
        smoltcp_now(started),
        default_dns_upstream(),
    )?;
    let mut core = VmnetCore::new(gateway);
    let connector = MappedTcpConnector {
        base: StdTcpConnector {
            timeout: config.upstream_connect_timeout,
        },
        mappings: config.upstream_mappings.clone(),
    };
    let mut proxy = tcp_proxy_bridge_from_policy(connector.clone(), &config.policy)?;
    let mut host_ingress = HostIngressBridge::new();
    let mut host_accept_queue =
        HostIngressAcceptedQueue::new(DEFAULT_HOST_INGRESS_ACCEPT_QUEUE_LIMIT);
    let mut dns_worker =
        spawn_dns_service_task::<(), _>(default_dns_upstream(), config.service_io_limits)?;
    let mut tcp_connect_worker =
        spawn_tcp_connect_service_task(connector, config.service_io_limits)?;
    let mut dns_service =
        VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(config.service_io_limits)?;
    let mut tcp_connect_service =
        VmnetServiceOwner::<TcpProxyPendingConnect, std::net::TcpStream>::new(
            config.service_io_limits,
        )?;
    let mut stats = VmnetRuntimeStats::default();
    let mut event_log = open_event_log(config.event_log_path.as_deref())?;
    let mut pcap = open_pcap_capture(&config)?;
    let mut fd_cursor = 0;

    loop {
        let poll_timeout = core.tcp_poll_delay(smoltcp_now(started));
        let fd_registrations =
            snapshot_async_fd_registrations(&host_listeners, &host_ingress, &proxy);
        let event = next_async_owner_event_with_fd_snapshot(
            &mut frame_io,
            &mut dns_worker,
            &mut tcp_connect_worker,
            poll_timeout,
            &fd_registrations,
            &mut fd_cursor,
        )
        .await?;
        let now = smoltcp_now(started);
        let mut gateway_events = Vec::new();
        let mut host_events = Vec::new();
        let mut poll_all_host_ingress = false;
        let mut host_listener_ready = Vec::new();
        let mut host_readable = Vec::new();
        let mut host_writable = Vec::new();
        let mut proxy_readable = Vec::new();
        let mut proxy_writable = Vec::new();
        let mut proxy_events = Vec::new();

        match event {
            VmnetAsyncOwnerEvent::QemuEof => {
                info!(
                    guest_frames_read = stats.guest_frames_read,
                    guest_frames_written = stats.guest_frames_written,
                    "qemu vmnet stream reached EOF"
                );
                return Ok(stats);
            }
            VmnetAsyncOwnerEvent::QemuFrame(frame) => {
                stats.guest_frames_read += 1;
                poll_all_host_ingress = true;
                capture_frame(pcap.as_mut(), &frame)?;
                match handle_guest_frame_with_async_dns_worker(
                    &mut core,
                    &mut dns_service,
                    &dns_worker,
                    frame,
                    now,
                ) {
                    VmnetDnsServiceFrame::Immediate(result)
                    | VmnetDnsServiceFrame::QueueFull(result) => {
                        if let Some(event) = gateway_event_from_outcome(&result.outcome) {
                            gateway_events.push(event);
                        }
                        let mut frame_stats = VmnetRuntimeStats::default();
                        let _captured = write_guest_frames_async(
                            &mut frame_io,
                            &result.guest_frames,
                            &mut frame_stats,
                            pcap.as_mut(),
                        )
                        .await?;
                        stats.guest_frames_written += frame_stats.guest_frames_written;
                    }
                    VmnetDnsServiceFrame::Queued { .. } => {}
                }
            }
            VmnetAsyncOwnerEvent::Service(event) => {
                let applied = apply_async_service_completion_event(
                    &mut frame_io,
                    &mut core,
                    &mut dns_service,
                    &mut proxy,
                    &mut tcp_connect_service,
                    event,
                    now,
                    &mut stats,
                    pcap.as_mut(),
                )
                .await?;
                let _ = applied.captured_frames;
                gateway_events.extend(applied.gateway_events);
                proxy_events.extend(applied.proxy_events);
                if applied.dns_disconnected {
                    warn!("vmnet DNS service worker disconnected");
                }
                if applied.tcp_disconnected {
                    warn!("vmnet TCP connect service worker disconnected");
                }
            }
            VmnetAsyncOwnerEvent::Timer => {
                let guest_frames = core.poll_tcp(now);
                let mut frame_stats = VmnetRuntimeStats::default();
                let _captured = write_guest_frames_async(
                    &mut frame_io,
                    &guest_frames,
                    &mut frame_stats,
                    pcap.as_mut(),
                )
                .await?;
                stats.guest_frames_written += frame_stats.guest_frames_written;
                poll_all_host_ingress = true;
            }
            VmnetAsyncOwnerEvent::FdReady {
                source,
                readable,
                writable,
            } => match source {
                VmnetEventSource::HostListener(index) => {
                    if readable {
                        host_listener_ready.push(index);
                    }
                }
                VmnetEventSource::HostSession(handle) => {
                    if readable {
                        host_readable.push(handle);
                    }
                    if writable {
                        host_writable.push(handle);
                    }
                }
                VmnetEventSource::UpstreamSession(handle) => {
                    if readable {
                        proxy_readable.push(handle);
                    }
                    if writable {
                        proxy_writable.push(handle);
                    }
                }
            },
        }

        let tcp_connect_events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut tcp_connect_service,
            &tcp_connect_worker,
            now,
        );

        for index in host_listener_ready {
            let batch = host_listeners
                .accept_ready_limited(index, DEFAULT_HOST_INGRESS_ACCEPTS_PER_LISTENER_PUMP);
            for accepted in batch.accepted {
                let accepted = accepted?;
                if let Err(rejected) = host_accept_queue.push(accepted) {
                    host_events.push(HostIngressEvent::AcceptQueueFull {
                        guest_port: rejected.guest_port,
                        purpose: rejected.purpose,
                        capacity: host_accept_queue.capacity(),
                    });
                }
            }
            if let Some(limit) = batch.limit_reached {
                host_events.push(HostIngressEvent::AcceptLimitReached {
                    listener_index: limit.listener_index,
                    guest_port: limit.guest_port,
                    purpose: limit.purpose,
                    limit: limit.limit,
                    accepted: limit.accepted,
                });
            }
        }

        for accepted in host_accept_queue.drain_ready() {
            let (guest_frames, event) =
                open_host_ingress_session(&mut core, &mut host_ingress, accepted, now);
            let mut frame_stats = VmnetRuntimeStats::default();
            let _captured = write_guest_frames_async(
                &mut frame_io,
                &guest_frames,
                &mut frame_stats,
                pcap.as_mut(),
            )
            .await?;
            stats.guest_frames_written += frame_stats.guest_frames_written;
            host_events.push(event);
        }

        let mut proxy_pump = pump_proxy_ready_async(
            &mut frame_io,
            &mut core,
            &mut proxy,
            now,
            TcpProxyReadiness::selected(proxy_readable, proxy_writable),
            pcap.as_mut(),
        )
        .await?;
        stats.guest_frames_written += proxy_pump.guest_frames_written;
        let mut frame_stats = VmnetRuntimeStats::default();
        write_proxy_event_guest_frames_async(
            &mut frame_io,
            &tcp_connect_events,
            &mut frame_stats,
            pcap.as_mut(),
        )
        .await?;
        stats.guest_frames_written += frame_stats.guest_frames_written;
        proxy_pump.events.extend(tcp_connect_events);
        proxy_pump.events.extend(proxy_events);

        let host_pump = pump_host_ingress_ready_async(
            &mut frame_io,
            &mut core,
            &mut host_ingress,
            now,
            if poll_all_host_ingress {
                HostIngressReadiness::all()
            } else {
                HostIngressReadiness::selected(host_readable, host_writable)
            },
            pcap.as_mut(),
        )
        .await?;
        stats.guest_frames_written += host_pump.guest_frames_written;
        host_events.extend(host_pump.events);

        write_gateway_events(&mut event_log, &gateway_events)?;
        write_proxy_events(&mut event_log, &proxy_pump.events)?;
        write_host_ingress_events(&mut event_log, &host_events)?;
        stats.gateway_events.extend(gateway_events);
        stats.proxy_events.extend(proxy_pump.events);
        stats.host_ingress_events.extend(host_events);
        let reap = core.gateway_mut().reap_closed_tcp_sessions();
        if reap.listener_sessions > 0 || reap.host_connections > 0 {
            debug!(
                listener_sessions = reap.listener_sessions,
                host_connections = reap.host_connections,
                "reaped closed guest TCP sessions"
            );
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum VmnetDnsServiceFrame {
    Immediate(GuestFrameResult),
    Queued { token: VmnetServiceToken },
    QueueFull(GuestFrameResult),
}

#[cfg(test)]
fn handle_guest_frame_with_dns_service<C>(
    core: &mut VmnetCore<'_>,
    service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, C>,
    frame: Vec<u8>,
    now: Instant,
) -> VmnetDnsServiceFrame {
    match core.handle_guest_frame_with_deferred_dns(frame, now) {
        VmnetDeferredDnsFrame::Immediate(result) => VmnetDnsServiceFrame::Immediate(result),
        VmnetDeferredDnsFrame::Forward(pending) => {
            match service.submit(pending, |pending, token| pending.service_command(token)) {
                Ok(token) => VmnetDnsServiceFrame::Queued { token },
                Err(full) => {
                    let result = core.complete_pending_dns_query(
                        full.pending,
                        Err(DnsUpstreamError::Unavailable),
                    );
                    VmnetDnsServiceFrame::QueueFull(result)
                }
            }
        }
    }
}

fn handle_guest_frame_with_async_dns_worker<C>(
    core: &mut VmnetCore<'_>,
    service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, C>,
    worker: &VmnetAsyncDnsWorkerHandle<C>,
    frame: Vec<u8>,
    now: Instant,
) -> VmnetDnsServiceFrame {
    match core.handle_guest_frame_with_deferred_dns(frame, now) {
        VmnetDeferredDnsFrame::Immediate(result) => VmnetDnsServiceFrame::Immediate(result),
        VmnetDeferredDnsFrame::Forward(pending) => match service.submit_to_async_worker(
            pending,
            |pending, token| pending.service_command(token),
            worker,
        ) {
            Ok(token) => VmnetDnsServiceFrame::Queued { token },
            Err(error) => {
                let result = core
                    .complete_pending_dns_query(error.pending, Err(DnsUpstreamError::Unavailable));
                VmnetDnsServiceFrame::QueueFull(result)
            }
        },
    }
}

fn apply_dns_service_completion<C>(
    core: &mut VmnetCore<'_>,
    service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, C>,
    completion: VmnetServiceCompletion<C>,
) -> Option<GuestFrameResult> {
    let pending = service.remove_pending_for_completion(&completion)?;
    match completion {
        VmnetServiceCompletion::DnsLookup(completion) => {
            Some(core.complete_pending_dns_query(pending, completion.result))
        }
        VmnetServiceCompletion::TcpConnect(_) | VmnetServiceCompletion::Cancelled(_) => None,
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct VmnetDnsServiceDrain {
    guest_results: Vec<GuestFrameResult>,
    disconnected: bool,
}

#[cfg(test)]
fn drain_async_dns_worker_completions<C>(
    core: &mut VmnetCore<'_>,
    service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, C>,
    worker: &mut VmnetAsyncDnsWorkerHandle<C>,
) -> VmnetDnsServiceDrain {
    let mut drain = VmnetDnsServiceDrain::default();
    loop {
        match worker.try_recv_completion() {
            Ok(completion) => {
                if let Some(result) = apply_dns_service_completion(core, service, completion) {
                    drain.guest_results.push(result);
                }
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return drain,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                drain.disconnected = true;
                drain
                    .guest_results
                    .extend(fail_pending_dns_service_queries(core, service));
                return drain;
            }
        }
    }
}

#[cfg(test)]
fn fail_pending_dns_service_queries<C>(
    core: &mut VmnetCore<'_>,
    service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, C>,
) -> Vec<GuestFrameResult> {
    service
        .drain_pending()
        .into_iter()
        .map(|(_token, pending)| {
            core.complete_pending_dns_query(pending, Err(DnsUpstreamError::Unavailable))
        })
        .collect()
}

fn submit_tcp_connects_to_async_worker<T>(
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<T>,
    service: &mut VmnetServiceOwner<TcpProxyPendingConnect, T::Connection>,
    worker: &VmnetAsyncTcpConnectWorkerHandle<T::Connection>,
    now: Instant,
) -> Vec<TcpProxyEvent>
where
    T: TcpUpstreamConnector,
{
    let mut events = Vec::new();
    for plan in proxy.plan_connects_for_service(core.gateway_mut()) {
        match plan {
            TcpProxyConnectPlan::Event(event) => events.push(event),
            TcpProxyConnectPlan::Pending(pending) => {
                let handle = pending.handle;
                match service.submit_to_async_worker(
                    pending,
                    |pending, token| pending.service_command(token),
                    worker,
                ) {
                    Ok(_) => proxy.mark_connect_pending(handle),
                    Err(error) => {
                        let _ = proxy.complete_connect(
                            core.gateway_mut(),
                            error.pending,
                            Err(TcpConnectError::UpstreamUnavailable),
                            now,
                            &mut events,
                        );
                    }
                }
            }
        }
    }
    events
}

fn apply_tcp_connect_service_completion<T>(
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<T>,
    service: &mut VmnetServiceOwner<TcpProxyPendingConnect, T::Connection>,
    completion: VmnetServiceCompletion<T::Connection>,
    now: Instant,
) -> Vec<TcpProxyEvent>
where
    T: TcpUpstreamConnector,
{
    let Some(pending) = service.remove_pending_for_completion(&completion) else {
        return Vec::new();
    };
    let VmnetServiceCompletion::TcpConnect(completion) = completion else {
        return Vec::new();
    };
    let mut events = Vec::new();
    let _ = proxy.complete_connect(
        core.gateway_mut(),
        pending,
        completion.result,
        now,
        &mut events,
    );
    events
}

#[cfg(test)]
#[derive(Debug, Default)]
struct VmnetTcpConnectServiceDrain {
    events: Vec<TcpProxyEvent>,
    disconnected: bool,
}

#[cfg(test)]
fn drain_async_tcp_connect_worker_completions<T>(
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<T>,
    service: &mut VmnetServiceOwner<TcpProxyPendingConnect, T::Connection>,
    worker: &mut VmnetAsyncTcpConnectWorkerHandle<T::Connection>,
    now: Instant,
) -> VmnetTcpConnectServiceDrain
where
    T: TcpUpstreamConnector,
{
    let mut drain = VmnetTcpConnectServiceDrain::default();
    loop {
        match worker.try_recv_completion() {
            Ok(completion) => {
                drain.events.extend(apply_tcp_connect_service_completion(
                    core, proxy, service, completion, now,
                ));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return drain,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                drain.disconnected = true;
                drain
                    .events
                    .extend(fail_pending_tcp_connects(core, proxy, service, now));
                return drain;
            }
        }
    }
}

#[cfg(test)]
fn fail_pending_tcp_connects<T>(
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<T>,
    service: &mut VmnetServiceOwner<TcpProxyPendingConnect, T::Connection>,
    now: Instant,
) -> Vec<TcpProxyEvent>
where
    T: TcpUpstreamConnector,
{
    let mut events = Vec::new();
    for (_token, pending) in service.drain_pending() {
        let _ = proxy.complete_connect(
            core.gateway_mut(),
            pending,
            Err(TcpConnectError::UpstreamUnavailable),
            now,
            &mut events,
        );
    }
    events
}

fn dns_frame_result_to_guest(result: DnsFrameResult) -> GuestFrameResult {
    GuestFrameResult {
        outcome: GuestFrameOutcome::DnsQuery { log: result.log },
        guest_frames: result.response.into_iter().collect(),
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

fn open_host_ingress_session<C>(
    core: &mut VmnetCore<'_>,
    bridge: &mut HostIngressBridge<C>,
    accepted: AcceptedHostConnection<C>,
    now: Instant,
) -> (Vec<Vec<u8>>, HostIngressEvent) {
    match bridge.open_session(
        core.gateway_mut(),
        accepted.guest_port,
        accepted.connection,
        now,
    ) {
        Ok(open) => (
            open.guest_frames,
            HostIngressEvent::Opened {
                handle: open.handle,
                guest_port: open.guest_port,
                purpose: accepted.purpose,
            },
        ),
        Err(error) => (
            Vec::new(),
            HostIngressEvent::OpenFailed {
                guest_port: accepted.guest_port,
                purpose: accepted.purpose,
                error: format!("{error:?}"),
            },
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VmnetAsyncFdInterest {
    readable: bool,
    writable: bool,
}

impl VmnetAsyncFdInterest {
    fn new(readable: bool, writable: bool) -> Option<Self> {
        (readable || writable).then_some(Self { readable, writable })
    }

    const READABLE: Self = Self {
        readable: true,
        writable: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VmnetAsyncFdRegistration {
    source: VmnetEventSource,
    fd: RawFd,
    interest: VmnetAsyncFdInterest,
}

fn snapshot_async_fd_registrations(
    host_listeners: &HostIngressListenerSet,
    host_ingress: &HostIngressBridge<std::net::TcpStream>,
    proxy: &TcpProxyBridge<MappedTcpConnector>,
) -> Vec<VmnetAsyncFdRegistration> {
    let mut registrations = Vec::new();
    registrations.extend(host_listeners.listener_fds().into_iter().enumerate().map(
        |(index, fd)| VmnetAsyncFdRegistration {
            source: VmnetEventSource::HostListener(index),
            fd,
            interest: VmnetAsyncFdInterest::READABLE,
        },
    ));
    for handle in host_ingress.session_handles() {
        let Some(fd) = host_ingress.session_raw_fd(handle) else {
            continue;
        };
        let Some(interest) = host_ingress
            .session_interest(handle)
            .and_then(|interest| VmnetAsyncFdInterest::new(interest.readable, interest.writable))
        else {
            continue;
        };
        registrations.push(VmnetAsyncFdRegistration {
            source: VmnetEventSource::HostSession(handle),
            fd,
            interest,
        });
    }
    for handle in proxy.session_handles() {
        let Some(fd) = proxy.session_raw_fd(handle) else {
            continue;
        };
        let Some(interest) = proxy
            .session_interest(handle)
            .and_then(|interest| VmnetAsyncFdInterest::new(interest.readable, interest.writable))
        else {
            continue;
        };
        registrations.push(VmnetAsyncFdRegistration {
            source: VmnetEventSource::UpstreamSession(handle),
            fd,
            interest,
        });
    }
    registrations
}

#[cfg(test)]
fn choose_async_fd_registration(
    registrations: &[VmnetAsyncFdRegistration],
    cursor: &mut usize,
) -> Option<VmnetAsyncFdRegistration> {
    if registrations.is_empty() {
        *cursor = 0;
        return None;
    }
    let start = *cursor % registrations.len();
    for offset in 0..registrations.len() {
        let index = (start + offset) % registrations.len();
        if async_fd_registration_poll_ready(registrations[index]) {
            *cursor = (index + 1) % registrations.len();
            return Some(registrations[index]);
        }
    }
    let index = start;
    *cursor = (index + 1) % registrations.len();
    Some(registrations[index])
}

#[cfg(test)]
fn async_fd_registration_poll_ready(registration: VmnetAsyncFdRegistration) -> bool {
    let mut events = 0;
    if registration.interest.readable {
        events |= libc::POLLIN;
    }
    if registration.interest.writable {
        events |= libc::POLLOUT;
    }
    let mut poll_fd = libc::pollfd {
        fd: registration.fd,
        events,
        revents: 0,
    };
    // SAFETY: `poll_fd` points to one valid pollfd value for the duration of the call.
    let result = unsafe { libc::poll(&mut poll_fd, 1, 0) };
    result > 0 && poll_fd.revents != 0
}

async fn wait_async_fd_registrations(
    registrations: &[VmnetAsyncFdRegistration],
    cursor: &mut usize,
) -> Result<Option<(VmnetEventSource, bool, bool)>, VmnetRuntimeError> {
    if registrations.is_empty() {
        *cursor = 0;
        return Ok(None);
    }
    let start = *cursor % registrations.len();
    let async_fds = (0..registrations.len())
        .map(|offset| {
            let index = (start + offset) % registrations.len();
            let registration = registrations[index];
            AsyncFd::new(BorrowedRawFd {
                fd: registration.fd,
            })
            .map(|async_fd| (index, registration, async_fd))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let (index, source, readable, writable) = std::future::poll_fn(|cx| {
        for (index, registration, async_fd) in &async_fds {
            if registration.interest.readable {
                match async_fd.poll_read_ready(cx) {
                    Poll::Ready(Ok(mut ready)) => {
                        ready.clear_ready();
                        return Poll::Ready(Ok((*index, registration.source, true, false)));
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => {}
                }
            }
            if registration.interest.writable {
                match async_fd.poll_write_ready(cx) {
                    Poll::Ready(Ok(mut ready)) => {
                        ready.clear_ready();
                        return Poll::Ready(Ok((*index, registration.source, false, true)));
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => {}
                }
            }
        }
        Poll::Pending
    })
    .await?;
    *cursor = (index + 1) % registrations.len();
    Ok(Some((source, readable, writable)))
}

#[cfg(test)]
fn pump_proxy_once<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
    pcap: Option<&mut PcapWriter>,
) -> Result<VmnetProxyPump, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    pump_proxy_ready(frame_io, core, proxy, now, TcpProxyReadiness::all(), pcap)
}

#[cfg(test)]
fn pump_proxy_ready<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
    readiness: TcpProxyReadiness,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetProxyPump, VmnetRuntimeError>
where
    T: Read + Write,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut pump = VmnetProxyPump::default();
    for event in proxy.process_gateway_with_readiness(core.gateway_mut(), now, readiness) {
        let mut stats = VmnetRuntimeStats::default();
        write_proxy_event_guest_frames(
            frame_io,
            std::slice::from_ref(&event),
            &mut stats,
            pcap.as_deref_mut(),
        )?;
        pump.guest_frames_written += stats.guest_frames_written;
        pump.events.push(event);
    }
    Ok(pump)
}

async fn pump_proxy_ready_async<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    proxy: &mut TcpProxyBridge<C>,
    now: Instant,
    readiness: TcpProxyReadiness,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetProxyPump, VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut pump = VmnetProxyPump::default();
    for event in proxy.process_gateway_with_readiness(core.gateway_mut(), now, readiness) {
        let mut stats = VmnetRuntimeStats::default();
        write_proxy_event_guest_frames_async(
            frame_io,
            std::slice::from_ref(&event),
            &mut stats,
            pcap.as_deref_mut(),
        )
        .await?;
        pump.guest_frames_written += stats.guest_frames_written;
        pump.events.push(event);
    }
    Ok(pump)
}

#[cfg(test)]
fn write_proxy_event_guest_frames<T>(
    frame_io: &mut QemuFrameIo<T>,
    events: &[TcpProxyEvent],
    stats: &mut VmnetRuntimeStats,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<(), VmnetRuntimeError>
where
    T: Read + Write,
{
    for event in events {
        if let TcpProxyEvent::UpstreamPayload { guest_frames, .. }
        | TcpProxyEvent::TlsHandshakePayload { guest_frames, .. }
        | TcpProxyEvent::ConnectFailed { guest_frames, .. }
        | TcpProxyEvent::TlsMitmFailed { guest_frames, .. }
        | TcpProxyEvent::BufferLimitExceeded { guest_frames, .. } = event
        {
            for frame in guest_frames {
                capture_frame(pcap.as_deref_mut(), frame)?;
                frame_io.write_frame(frame)?;
                stats.guest_frames_written += 1;
            }
        }
    }
    Ok(())
}

async fn write_proxy_event_guest_frames_async<T>(
    frame_io: &mut QemuFrameIo<T>,
    events: &[TcpProxyEvent],
    stats: &mut VmnetRuntimeStats,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<(), VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
{
    for event in events {
        if let TcpProxyEvent::UpstreamPayload { guest_frames, .. }
        | TcpProxyEvent::TlsHandshakePayload { guest_frames, .. }
        | TcpProxyEvent::ConnectFailed { guest_frames, .. }
        | TcpProxyEvent::TlsMitmFailed { guest_frames, .. }
        | TcpProxyEvent::BufferLimitExceeded { guest_frames, .. } = event
        {
            for frame in guest_frames {
                capture_frame(pcap.as_deref_mut(), frame)?;
                frame_io.write_frame_async(frame).await?;
                stats.guest_frames_written += 1;
            }
        }
    }
    Ok(())
}

async fn pump_host_ingress_ready_async<T>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    bridge: &mut HostIngressBridge<std::net::TcpStream>,
    now: Instant,
    readiness: HostIngressReadiness,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetHostIngressPump, VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
{
    let mut pump = VmnetHostIngressPump::default();
    for event in bridge.process_gateway_with_readiness(core.gateway_mut(), now, readiness) {
        write_host_ingress_event_guest_frames_async(
            frame_io,
            std::slice::from_ref(&event),
            &mut pump,
            pcap.as_deref_mut(),
        )
        .await?;
        pump.events.push(event);
    }

    Ok(pump)
}

async fn write_host_ingress_event_guest_frames_async<T>(
    frame_io: &mut QemuFrameIo<T>,
    events: &[HostIngressEvent],
    pump: &mut VmnetHostIngressPump,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<(), VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
{
    for event in events {
        if let HostIngressEvent::HostPayload { guest_frames, .. }
        | HostIngressEvent::HostClosed { guest_frames, .. }
        | HostIngressEvent::BufferLimitExceeded { guest_frames, .. } = event
        {
            for frame in guest_frames {
                capture_frame(pcap.as_deref_mut(), frame)?;
                frame_io.write_frame_async(frame).await?;
                pump.guest_frames_written += 1;
            }
        }
    }
    Ok(())
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
    for event in events {
        trace_proxy_event(event);
        if let Some(log) = log.as_mut() {
            writeln!(log, "{}", format_proxy_event(event))?;
        }
    }
    if let Some(log) = log.as_mut() {
        log.flush()?;
    }
    Ok(())
}

fn write_gateway_events(log: &mut Option<File>, events: &[VmnetGatewayEvent]) -> io::Result<()> {
    for event in events {
        trace_gateway_event(event);
        if let Some(log) = log.as_mut() {
            writeln!(log, "{}", format_gateway_event(event))?;
        }
    }
    if let Some(log) = log.as_mut() {
        log.flush()?;
    }
    Ok(())
}

fn trace_gateway_event(event: &VmnetGatewayEvent) {
    let event_kind = gateway_event_kind(event);
    let event_detail = format_gateway_event(event);
    if gateway_event_is_failure(event) {
        warn!(event_kind, event = %event_detail, "vmnet gateway event");
    } else {
        debug!(event_kind, event = %event_detail, "vmnet gateway event");
    }
}

fn gateway_event_kind(event: &VmnetGatewayEvent) -> &'static str {
    match event {
        VmnetGatewayEvent::DnsQuery { .. } => "dns_query",
        VmnetGatewayEvent::UdpDenied(_) => "udp_denied",
        VmnetGatewayEvent::UnsupportedProtocol(_) => "unsupported_protocol",
        VmnetGatewayEvent::TcpDenied { .. } => "tcp_denied_preaccept",
        VmnetGatewayEvent::TcpSetupFailed { .. } => "tcp_setup_failed_preaccept",
    }
}

fn gateway_event_is_failure(event: &VmnetGatewayEvent) -> bool {
    matches!(
        event,
        VmnetGatewayEvent::UdpDenied(_)
            | VmnetGatewayEvent::UnsupportedProtocol(_)
            | VmnetGatewayEvent::TcpDenied { .. }
            | VmnetGatewayEvent::TcpSetupFailed { .. }
    )
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
    for event in events {
        trace_host_ingress_event(event);
        if let Some(log) = log.as_mut() {
            writeln!(log, "{}", format_host_ingress_event(event))?;
        }
    }
    if let Some(log) = log.as_mut() {
        log.flush()?;
    }
    Ok(())
}

fn trace_host_ingress_event(event: &HostIngressEvent) {
    let event_kind = host_ingress_event_kind(event);
    let event_detail = format_host_ingress_event(event);
    if host_ingress_event_is_failure(event) {
        warn!(event_kind, event = %event_detail, "vmnet host ingress event");
    } else {
        debug!(event_kind, event = %event_detail, "vmnet host ingress event");
    }
}

fn host_ingress_event_kind(event: &HostIngressEvent) -> &'static str {
    match event {
        HostIngressEvent::Opened { .. } => "host_ingress_opened",
        HostIngressEvent::OpenFailed { .. } => "host_ingress_open_failed",
        HostIngressEvent::AcceptQueueFull { .. } => "host_ingress_accept_queue_full",
        HostIngressEvent::AcceptLimitReached { .. } => "host_ingress_accept_limit_reached",
        HostIngressEvent::HostPayload { .. } => "host_ingress_host_payload",
        HostIngressEvent::GuestPayload { .. } => "host_ingress_guest_payload",
        HostIngressEvent::HostClosed { .. } => "host_ingress_host_closed",
        HostIngressEvent::GuestClosed { .. } => "host_ingress_guest_closed",
        HostIngressEvent::GuestReadFailed { .. } => "host_ingress_guest_read_failed",
        HostIngressEvent::GuestWriteFailed { .. } => "host_ingress_guest_write_failed",
        HostIngressEvent::HostReadFailed { .. } => "host_ingress_host_read_failed",
        HostIngressEvent::HostReadLimitReached { .. } => "host_ingress_host_read_limit_reached",
        HostIngressEvent::HostWriteLimitReached { .. } => "host_ingress_host_write_limit_reached",
        HostIngressEvent::HostWriteFailed { .. } => "host_ingress_host_write_failed",
        HostIngressEvent::BufferLimitExceeded { .. } => "host_ingress_buffer_limit_exceeded",
    }
}

fn host_ingress_event_is_failure(event: &HostIngressEvent) -> bool {
    matches!(
        event,
        HostIngressEvent::OpenFailed { .. }
            | HostIngressEvent::AcceptQueueFull { .. }
            | HostIngressEvent::AcceptLimitReached { .. }
            | HostIngressEvent::GuestReadFailed { .. }
            | HostIngressEvent::GuestWriteFailed { .. }
            | HostIngressEvent::HostReadFailed { .. }
            | HostIngressEvent::HostReadLimitReached { .. }
            | HostIngressEvent::HostWriteLimitReached { .. }
            | HostIngressEvent::HostWriteFailed { .. }
            | HostIngressEvent::BufferLimitExceeded { .. }
    )
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
        HostIngressEvent::AcceptQueueFull {
            guest_port,
            purpose,
            capacity,
        } => format!(
            "host_ingress_accept_queue_full guest_port={guest_port} purpose={purpose:?} capacity={capacity}"
        ),
        HostIngressEvent::AcceptLimitReached {
            listener_index,
            guest_port,
            purpose,
            limit,
            accepted,
        } => format!(
            "host_ingress_accept_limit_reached listener_index={listener_index} guest_port={guest_port} purpose={purpose:?} limit={limit} accepted={accepted}"
        ),
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
        HostIngressEvent::HostReadLimitReached {
            handle,
            guest_port,
            limit,
            read,
        } => format!(
            "host_ingress_host_read_limit_reached handle={handle:?} guest_port={guest_port} limit={limit} read={read}"
        ),
        HostIngressEvent::HostWriteLimitReached {
            handle,
            guest_port,
            limit,
            written,
        } => format!(
            "host_ingress_host_write_limit_reached handle={handle:?} guest_port={guest_port} limit={limit} written={written}"
        ),
        HostIngressEvent::HostWriteFailed { handle, error } => {
            format!("host_ingress_host_write_failed handle={handle:?} error={error}")
        }
        HostIngressEvent::BufferLimitExceeded {
            handle,
            guest_port,
            buffer,
            limit,
            attempted,
            guest_frames,
        } => format!(
            "host_ingress_buffer_limit_exceeded handle={handle:?} guest_port={guest_port} buffer={buffer:?} limit={limit} attempted={attempted} guest_frames={}",
            guest_frames.len()
        ),
    }
}

fn trace_proxy_event(event: &TcpProxyEvent) {
    let event_kind = proxy_event_kind(event);
    let event_detail = format_proxy_event(event);
    if proxy_event_is_failure(event) {
        warn!(event_kind, event = %event_detail, "vmnet proxy event");
    } else {
        debug!(event_kind, event = %event_detail, "vmnet proxy event");
    }
}

fn proxy_event_kind(event: &TcpProxyEvent) -> &'static str {
    match event {
        TcpProxyEvent::Connected { .. } => "tcp_connected",
        TcpProxyEvent::Denied { .. } => "tcp_denied",
        TcpProxyEvent::ConnectFailed { .. } => "tcp_connect_failed",
        TcpProxyEvent::GuestPayload { .. } => "guest_payload",
        TcpProxyEvent::HttpRequest { .. } => "http_request",
        TcpProxyEvent::HttpRequestIncomplete { .. } => "http_request_incomplete",
        TcpProxyEvent::HttpRequestMalformed { .. } => "http_request_malformed",
        TcpProxyEvent::UpstreamPayload { .. } => "upstream_payload",
        TcpProxyEvent::TlsHandshakePayload { .. } => "tls_handshake_payload",
        TcpProxyEvent::TlsUpstreamPayload { .. } => "tls_upstream_payload",
        TcpProxyEvent::TlsMitmUnavailable { .. } => "tls_mitm_unavailable",
        TcpProxyEvent::TlsMitmFailed { .. } => "tls_mitm_failed",
        TcpProxyEvent::BufferLimitExceeded { .. } => "proxy_buffer_limit_exceeded",
        TcpProxyEvent::GuestReadFailed { .. } => "guest_read_failed",
        TcpProxyEvent::GuestWriteFailed { .. } => "guest_write_failed",
        TcpProxyEvent::UpstreamWriteFailed { .. } => "upstream_write_failed",
        TcpProxyEvent::UpstreamReadFailed { .. } => "upstream_read_failed",
    }
}

fn proxy_event_is_failure(event: &TcpProxyEvent) -> bool {
    matches!(
        event,
        TcpProxyEvent::Denied { .. }
            | TcpProxyEvent::ConnectFailed { .. }
            | TcpProxyEvent::HttpRequestMalformed { .. }
            | TcpProxyEvent::TlsMitmUnavailable { .. }
            | TcpProxyEvent::TlsMitmFailed { .. }
            | TcpProxyEvent::BufferLimitExceeded { .. }
            | TcpProxyEvent::GuestReadFailed { .. }
            | TcpProxyEvent::GuestWriteFailed { .. }
            | TcpProxyEvent::UpstreamWriteFailed { .. }
            | TcpProxyEvent::UpstreamReadFailed { .. }
    )
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
            guest_frames,
        } => format!(
            "tcp_connect_failed handle={handle:?} dst={}:{} error={error:?} guest_frames={}",
            destination.ip,
            destination.port,
            guest_frames.len()
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
        TcpProxyEvent::TlsMitmFailed {
            handle,
            error,
            guest_frames,
        } => format!(
            "tls_mitm_failed handle={handle:?} error={error:?} guest_frames={}",
            guest_frames.len()
        ),
        TcpProxyEvent::BufferLimitExceeded {
            handle,
            buffer,
            limit,
            attempted,
            guest_frames,
        } => format!(
            "proxy_buffer_limit_exceeded handle={handle:?} buffer={buffer:?} limit={limit} attempted={attempted} guest_frames={}",
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

#[cfg(test)]
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

#[cfg(test)]
#[derive(Debug)]
enum VmnetAsyncQemuFrameStep {
    Frame(GuestFrameResult),
    Eof,
}

#[cfg(test)]
async fn handle_next_async_qemu_frame<T>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    now: Instant,
) -> Result<VmnetAsyncQemuFrameStep, VmnetRuntimeError>
where
    T: AsyncRead + Unpin,
{
    match frame_io.read_frame_async().await? {
        Some(frame) => Ok(VmnetAsyncQemuFrameStep::Frame(
            core.handle_guest_frame(frame, now),
        )),
        None => Ok(VmnetAsyncQemuFrameStep::Eof),
    }
}

async fn write_guest_frames_async<T>(
    frame_io: &mut QemuFrameIo<T>,
    frames: &[Vec<u8>],
    stats: &mut VmnetRuntimeStats,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<usize, VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
{
    let mut captured = 0;
    for frame in frames {
        capture_frame(pcap.as_deref_mut(), frame)?;
        captured += usize::from(pcap.is_some());
        frame_io.write_frame_async(frame).await?;
        stats.guest_frames_written += 1;
    }
    Ok(captured)
}

#[cfg(test)]
#[derive(Debug)]
enum VmnetAsyncQemuDnsEvent<C> {
    QemuFrame(Vec<u8>),
    QemuEof,
    DnsCompletion(VmnetServiceCompletion<C>),
    DnsDisconnected,
}

#[cfg(test)]
async fn next_async_qemu_or_dns_event<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    dns_worker: &mut VmnetAsyncDnsWorkerHandle<C>,
) -> Result<VmnetAsyncQemuDnsEvent<C>, VmnetRuntimeError>
where
    T: AsyncRead + Unpin,
{
    tokio::select! {
        frame = frame_io.read_frame_async() => {
            match frame? {
                Some(frame) => Ok(VmnetAsyncQemuDnsEvent::QemuFrame(frame)),
                None => Ok(VmnetAsyncQemuDnsEvent::QemuEof),
            }
        }
        completion = dns_worker.recv_completion() => {
            match completion {
                Some(completion) => Ok(VmnetAsyncQemuDnsEvent::DnsCompletion(completion)),
                None => Ok(VmnetAsyncQemuDnsEvent::DnsDisconnected),
            }
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
enum VmnetAsyncQemuTcpEvent<C> {
    QemuFrame,
    QemuEof,
    TcpCompletion(VmnetServiceCompletion<C>),
    TcpDisconnected,
}

#[cfg(test)]
async fn next_async_qemu_or_tcp_event<T, C>(
    frame_io: &mut QemuFrameIo<T>,
    tcp_worker: &mut VmnetAsyncTcpConnectWorkerHandle<C>,
) -> Result<VmnetAsyncQemuTcpEvent<C>, VmnetRuntimeError>
where
    T: AsyncRead + Unpin,
{
    tokio::select! {
        frame = frame_io.read_frame_async() => {
            match frame? {
                Some(_frame) => Ok(VmnetAsyncQemuTcpEvent::QemuFrame),
                None => Ok(VmnetAsyncQemuTcpEvent::QemuEof),
            }
        }
        completion = tcp_worker.recv_completion() => {
            match completion {
                Some(completion) => Ok(VmnetAsyncQemuTcpEvent::TcpCompletion(completion)),
                None => Ok(VmnetAsyncQemuTcpEvent::TcpDisconnected),
            }
        }
    }
}

#[derive(Debug)]
enum VmnetAsyncServiceCompletionEvent<DnsC, TcpC> {
    DnsCompletion(VmnetServiceCompletion<DnsC>),
    DnsDisconnected,
    TcpCompletion(VmnetServiceCompletion<TcpC>),
    TcpDisconnected,
}

async fn next_async_service_completion<DnsC, TcpC>(
    dns_worker: &mut VmnetAsyncDnsWorkerHandle<DnsC>,
    tcp_worker: &mut VmnetAsyncTcpConnectWorkerHandle<TcpC>,
) -> VmnetAsyncServiceCompletionEvent<DnsC, TcpC> {
    tokio::select! {
        completion = dns_worker.recv_completion() => {
            match completion {
                Some(completion) => VmnetAsyncServiceCompletionEvent::DnsCompletion(completion),
                None => VmnetAsyncServiceCompletionEvent::DnsDisconnected,
            }
        }
        completion = tcp_worker.recv_completion() => {
            match completion {
                Some(completion) => VmnetAsyncServiceCompletionEvent::TcpCompletion(completion),
                None => VmnetAsyncServiceCompletionEvent::TcpDisconnected,
            }
        }
    }
}

#[derive(Debug)]
enum VmnetAsyncOwnerEvent<DnsC, TcpC> {
    QemuFrame(Vec<u8>),
    QemuEof,
    Service(VmnetAsyncServiceCompletionEvent<DnsC, TcpC>),
    Timer,
    FdReady {
        source: VmnetEventSource,
        readable: bool,
        writable: bool,
    },
}

async fn next_async_owner_event_with_fd_snapshot<T, DnsC, TcpC>(
    frame_io: &mut QemuFrameIo<T>,
    dns_worker: &mut VmnetAsyncDnsWorkerHandle<DnsC>,
    tcp_worker: &mut VmnetAsyncTcpConnectWorkerHandle<TcpC>,
    delay: Option<Duration>,
    fd_registrations: &[VmnetAsyncFdRegistration],
    fd_cursor: &mut usize,
) -> Result<VmnetAsyncOwnerEvent<DnsC, TcpC>, VmnetRuntimeError>
where
    T: AsyncRead + Unpin,
{
    let read_frame = async {
        match frame_io.read_frame_async().await? {
            Some(frame) => Ok(VmnetAsyncOwnerEvent::QemuFrame(frame)),
            None => Ok(VmnetAsyncOwnerEvent::QemuEof),
        }
    };
    let service_completion = next_async_service_completion(dns_worker, tcp_worker);
    if fd_registrations.is_empty() {
        return match delay {
            Some(delay) => {
                tokio::select! {
                    event = read_frame => event,
                    event = service_completion => Ok(VmnetAsyncOwnerEvent::Service(event)),
                    _ = tokio::time::sleep(delay) => Ok(VmnetAsyncOwnerEvent::Timer),
                }
            }
            None => {
                tokio::select! {
                    event = read_frame => event,
                    event = service_completion => Ok(VmnetAsyncOwnerEvent::Service(event)),
                }
            }
        };
    }
    let fd_ready = wait_async_fd_registrations(fd_registrations, fd_cursor);
    match delay {
        Some(delay) => {
            tokio::select! {
                event = read_frame => event,
                event = service_completion => Ok(VmnetAsyncOwnerEvent::Service(event)),
                _ = tokio::time::sleep(delay) => Ok(VmnetAsyncOwnerEvent::Timer),
                ready = fd_ready => {
                    let Some((source, readable, writable)) = ready? else {
                        return Ok(VmnetAsyncOwnerEvent::Timer);
                    };
                    Ok(VmnetAsyncOwnerEvent::FdReady { source, readable, writable })
                }
            }
        }
        None => {
            tokio::select! {
                event = read_frame => event,
                event = service_completion => Ok(VmnetAsyncOwnerEvent::Service(event)),
                ready = fd_ready => {
                    let Some((source, readable, writable)) = ready? else {
                        return Ok(VmnetAsyncOwnerEvent::Timer);
                    };
                    Ok(VmnetAsyncOwnerEvent::FdReady { source, readable, writable })
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct VmnetAsyncServiceCompletionApply {
    guest_frames_written: usize,
    captured_frames: usize,
    gateway_events: Vec<VmnetGatewayEvent>,
    proxy_events: Vec<TcpProxyEvent>,
    dns_disconnected: bool,
    tcp_disconnected: bool,
}

async fn apply_async_service_completion_event<T, DnsC, C>(
    frame_io: &mut QemuFrameIo<T>,
    core: &mut VmnetCore<'_>,
    dns_service: &mut VmnetServiceOwner<VmnetPendingDnsQuery, DnsC>,
    proxy: &mut TcpProxyBridge<C>,
    tcp_connect_service: &mut VmnetServiceOwner<TcpProxyPendingConnect, C::Connection>,
    event: VmnetAsyncServiceCompletionEvent<DnsC, C::Connection>,
    now: Instant,
    stats: &mut VmnetRuntimeStats,
    mut pcap: Option<&mut PcapWriter>,
) -> Result<VmnetAsyncServiceCompletionApply, VmnetRuntimeError>
where
    T: AsyncWrite + Unpin,
    C: TcpUpstreamConnector,
    C::Connection: Read + Write,
{
    let mut applied = VmnetAsyncServiceCompletionApply::default();
    match event {
        VmnetAsyncServiceCompletionEvent::DnsCompletion(completion) => {
            if let Some(result) = apply_dns_service_completion(core, dns_service, completion) {
                if let Some(event) = gateway_event_from_outcome(&result.outcome) {
                    applied.gateway_events.push(event);
                }
                let mut frame_stats = VmnetRuntimeStats::default();
                let captured = write_guest_frames_async(
                    frame_io,
                    &result.guest_frames,
                    &mut frame_stats,
                    pcap.as_deref_mut(),
                )
                .await?;
                stats.guest_frames_written += frame_stats.guest_frames_written;
                applied.guest_frames_written += frame_stats.guest_frames_written;
                applied.captured_frames += captured;
            }
        }
        VmnetAsyncServiceCompletionEvent::DnsDisconnected => {
            applied.dns_disconnected = true;
        }
        VmnetAsyncServiceCompletionEvent::TcpCompletion(completion) => {
            let events = apply_tcp_connect_service_completion(
                core,
                proxy,
                tcp_connect_service,
                completion,
                now,
            );
            let mut frame_stats = VmnetRuntimeStats::default();
            write_proxy_event_guest_frames_async(
                frame_io,
                &events,
                &mut frame_stats,
                pcap.as_deref_mut(),
            )
            .await?;
            stats.guest_frames_written += frame_stats.guest_frames_written;
            applied.guest_frames_written += frame_stats.guest_frames_written;
            applied.proxy_events = events;
        }
        VmnetAsyncServiceCompletionEvent::TcpDisconnected => {
            applied.tcp_disconnected = true;
        }
    }
    Ok(applied)
}

#[derive(Debug, Clone, Copy)]
struct BorrowedRawFd {
    fd: RawFd,
}

impl AsRawFd for BorrowedRawFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

#[derive(Debug)]
pub enum VmnetRuntimeError {
    Io(io::Error),
    Stream(VmnetStreamError),
    Gateway(VmnetGatewayError),
    TlsMitm(TlsMitmError),
    ServiceIoLimits(VmnetServiceIoLimitError),
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

impl From<VmnetServiceIoLimitError> for VmnetRuntimeError {
    fn from(error: VmnetServiceIoLimitError) -> Self {
        Self::ServiceIoLimits(error)
    }
}

fn smoltcp_now(started: StdInstant) -> Instant {
    let elapsed = started.elapsed();
    Instant::from_millis(elapsed.as_millis().min(i64::MAX as u128) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns_proxy::{DnsDecision, DnsLogEntry, DnsUpstream, DnsUpstreamError};
    use crate::host_ingress::{HostIngressBufferKind, HostIngressEvent};
    use crate::network_policy::{HostListener, HostListenerPurpose, VmnetPolicy};
    use crate::tcp_gateway::{TcpAction, TcpConnectError, TcpDecision, TcpDestination};
    use crate::tcp_proxy::{TcpProxyBridge, TcpProxyConnectPlan};
    use crate::test_support;
    use crate::tls_mitm::TlsMitmError;
    use crate::vmnet_gateway::{UdpDenial, UnsupportedProtocol, VmnetGateway};
    use crate::vmnet_service_io::{
        spawn_dns_service_task, spawn_tcp_connect_service_task, VmnetDnsLookupCompletion,
        VmnetServiceCompletion, VmnetServiceIoLimits, VmnetServiceOwner, VmnetTcpConnectCompletion,
    };
    use crate::vmnet_stream::DEFAULT_MAX_FRAME_LEN;
    use crate::GuestNetwork;
    use hickory_proto::op::Message;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::socket::tcp;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind};
    use std::net::Ipv4Addr;

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(crate::guest_tcp::DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    #[test]
    fn vmnet_core_handles_guest_frames_without_runtime_io() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut core = VmnetCore::new(gateway);

        let result = core.handle_guest_frame(
            test_support::udp_frame(53000, 443, test_support::TEST_GUEST_IP, PUBLIC_IP, b"quic"),
            Instant::from_millis(1),
        );

        assert!(matches!(result.outcome, GuestFrameOutcome::UdpDenied(_)));
        assert!(result.guest_frames.is_empty());
    }

    #[tokio::test]
    async fn async_qemu_frame_harness_handles_guest_frame_and_writes_response() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut core = VmnetCore::new(gateway);
        let (guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut guest_io = QemuFrameIo::new(guest_stream, DEFAULT_MAX_FRAME_LEN);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);

        guest_io
            .write_frame_async(&test_support::tcp_syn_frame(PUBLIC_IP, 80))
            .await
            .expect("write guest frame");
        let step =
            handle_next_async_qemu_frame(&mut runtime_io, &mut core, Instant::from_millis(1))
                .await
                .expect("handle async guest frame");
        let VmnetAsyncQemuFrameStep::Frame(result) = step else {
            panic!("expected guest frame result");
        };
        assert!(matches!(
            result.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));
        assert!(!result.guest_frames.is_empty());

        let mut stats = VmnetRuntimeStats::default();
        write_guest_frames_async(&mut runtime_io, &result.guest_frames, &mut stats, None)
            .await
            .expect("write response frames");
        assert_eq!(stats.guest_frames_written, result.guest_frames.len());
        let response = tokio::time::timeout(Duration::from_secs(1), guest_io.read_frame_async())
            .await
            .expect("response timeout")
            .expect("read response")
            .expect("response frame");
        assert!(!response.is_empty());
    }

    #[tokio::test]
    async fn async_qemu_frame_harness_reports_clean_eof() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut core = VmnetCore::new(gateway);
        let (guest_stream, runtime_stream) = tokio::io::duplex(4096);
        drop(guest_stream);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);

        let step =
            handle_next_async_qemu_frame(&mut runtime_io, &mut core, Instant::from_millis(1))
                .await
                .expect("handle async eof");
        assert!(matches!(step, VmnetAsyncQemuFrameStep::Eof));
    }

    #[tokio::test]
    async fn async_event_guest_frame_writers_use_tokio_stream() {
        let (guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut reader = QemuFrameIo::new(guest_stream, DEFAULT_MAX_FRAME_LEN);
        let mut writer = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let proxy_frame = vec![0xaa, 0xbb, 0xcc];
        let host_frame = vec![0x11, 0x22, 0x33, 0x44];
        let mut proxy_stats = VmnetRuntimeStats::default();
        let proxy_events = [TcpProxyEvent::UpstreamPayload {
            handle: smoltcp::iface::SocketHandle::default(),
            bytes: proxy_frame.len(),
            guest_frames: vec![proxy_frame.clone()],
        }];

        write_proxy_event_guest_frames_async(&mut writer, &proxy_events, &mut proxy_stats, None)
            .await
            .expect("write proxy guest frame");
        assert_eq!(proxy_stats.guest_frames_written, 1);

        let mut host_pump = VmnetHostIngressPump::default();
        let host_events = [HostIngressEvent::HostPayload {
            handle: smoltcp::iface::SocketHandle::default(),
            guest_port: 8080,
            bytes: host_frame.len(),
            guest_frames: vec![host_frame.clone()],
        }];
        write_host_ingress_event_guest_frames_async(
            &mut writer,
            &host_events,
            &mut host_pump,
            None,
        )
        .await
        .expect("write host ingress guest frame");
        assert_eq!(host_pump.guest_frames_written, 1);

        assert_eq!(
            reader.read_frame_async().await.expect("read proxy frame"),
            Some(proxy_frame)
        );
        assert_eq!(
            reader.read_frame_async().await.expect("read host frame"),
            Some(host_frame)
        );
    }

    #[tokio::test]
    async fn async_select_harness_receives_dns_completion_without_wakeup_fd() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("service");
        let mut worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);

        let frame = handle_guest_frame_with_async_dns_worker(
            &mut core,
            &mut service,
            &worker,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        assert!(matches!(frame, VmnetDnsServiceFrame::Queued { .. }));
        assert_eq!(service.pending_len(), 1);

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_qemu_or_dns_event(&mut runtime_io, &mut worker),
        )
        .await
        .expect("select timeout")
        .expect("select event");
        let VmnetAsyncQemuDnsEvent::DnsCompletion(completion) = event else {
            panic!("expected DNS completion event");
        };
        let result = apply_dns_service_completion(&mut core, &mut service, completion)
            .expect("apply DNS completion");
        let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
            panic!("expected DNS query outcome");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(result.guest_frames.len(), 1);
        assert_eq!(service.pending_len(), 0);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[tokio::test]
    async fn async_select_harness_receives_qemu_frame_without_poller() {
        let (mut guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");
        let frame = test_support::tcp_syn_frame(PUBLIC_IP, 80);
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(frame.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&frame);
        tokio::io::AsyncWriteExt::write_all(&mut guest_stream, &encoded)
            .await
            .expect("write qemu frame");

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_qemu_or_dns_event(&mut runtime_io, &mut worker),
        )
        .await
        .expect("select timeout")
        .expect("select event");
        let VmnetAsyncQemuDnsEvent::QemuFrame(actual) = event else {
            panic!("expected qemu frame event");
        };
        assert_eq!(actual, frame);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[tokio::test]
    async fn async_select_harness_receives_tcp_completion_without_wakeup_fd() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let mut core = VmnetCore::new(gateway);
        let mut proxy = TcpProxyBridge::new(PanicTcpConnector);
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let mut worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);

        let submit_events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut service,
            &worker,
            Instant::from_millis(1),
        );
        assert!(submit_events.is_empty());
        assert!(proxy.has_pending_connect(active.handle));
        assert_eq!(service.pending_len(), 1);

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_qemu_or_tcp_event(&mut runtime_io, &mut worker),
        )
        .await
        .expect("select timeout")
        .expect("select event");
        let VmnetAsyncQemuTcpEvent::TcpCompletion(completion) = event else {
            panic!("expected TCP completion event");
        };
        let events = apply_tcp_connect_service_completion(
            &mut core,
            &mut proxy,
            &mut service,
            completion,
            Instant::from_millis(2),
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::Connected { .. })));
        assert_eq!(service.pending_len(), 0);
        assert_eq!(proxy.session_handles(), vec![active.handle]);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[tokio::test]
    async fn async_service_completion_select_receives_dns_without_wakeup_fd() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("service");
        let mut dns_worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");

        let frame = handle_guest_frame_with_async_dns_worker(
            &mut core,
            &mut service,
            &dns_worker,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        assert!(matches!(frame, VmnetDnsServiceFrame::Queued { .. }));

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_service_completion(&mut dns_worker, &mut tcp_worker),
        )
        .await
        .expect("service completion timeout");
        let VmnetAsyncServiceCompletionEvent::DnsCompletion(completion) = event else {
            panic!("expected DNS completion event");
        };
        let result = apply_dns_service_completion(&mut core, &mut service, completion)
            .expect("apply DNS completion");
        let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
            panic!("expected DNS query outcome");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(service.pending_len(), 0);
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_owner_event_select_receives_service_completion_before_timer() {
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
        let mut core = VmnetCore::new(gateway);
        let mut dns_service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("dns service");
        let mut dns_worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);

        let frame = handle_guest_frame_with_async_dns_worker(
            &mut core,
            &mut dns_service,
            &dns_worker,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        assert!(matches!(frame, VmnetDnsServiceFrame::Queued { .. }));

        let mut fd_cursor = 0;
        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_secs(30)),
                &[],
                &mut fd_cursor,
            ),
        )
        .await
        .expect("owner event timeout")
        .expect("owner event");
        let VmnetAsyncOwnerEvent::Service(VmnetAsyncServiceCompletionEvent::DnsCompletion(
            completion,
        )) = event
        else {
            panic!("expected DNS service completion event");
        };
        assert!(apply_dns_service_completion(&mut core, &mut dns_service, completion).is_some());
        assert_eq!(dns_service.pending_len(), 0);
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_owner_event_select_receives_timer_without_poller_or_wakeup() {
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut dns_worker = spawn_dns_service_task::<MemoryConnection, _>(
            FailingDnsUpstream,
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");

        let mut fd_cursor = 0;
        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_millis(1)),
                &[],
                &mut fd_cursor,
            ),
        )
        .await
        .expect("owner event timeout")
        .expect("owner event");

        assert!(matches!(event, VmnetAsyncOwnerEvent::Timer));
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[test]
    fn async_fd_snapshot_includes_host_listeners_without_poller_registration() {
        let listeners = HostIngressListenerSet::bind(&[
            HostListener {
                host_addr: "127.0.0.1".to_string(),
                host_port: 0,
                guest_port: 8080,
                purpose: HostListenerPurpose::PublishedTcp,
            },
            HostListener {
                host_addr: "127.0.0.1".to_string(),
                host_port: 0,
                guest_port: 9090,
                purpose: HostListenerPurpose::PayloadControl,
            },
        ])
        .expect("bind host listeners");
        let host_ingress = HostIngressBridge::new();
        let proxy = TcpProxyBridge::new(MappedTcpConnector {
            base: StdTcpConnector {
                timeout: Duration::from_secs(1),
            },
            mappings: Vec::new(),
        });

        let registrations = snapshot_async_fd_registrations(&listeners, &host_ingress, &proxy);

        assert_eq!(registrations.len(), 2);
        assert_eq!(
            registrations
                .iter()
                .map(|registration| registration.source)
                .collect::<Vec<_>>(),
            vec![
                VmnetEventSource::HostListener(0),
                VmnetEventSource::HostListener(1)
            ]
        );
        assert!(registrations.iter().all(|registration| {
            registration.fd >= 0
                && registration.interest
                    == VmnetAsyncFdInterest {
                        readable: true,
                        writable: false,
                    }
        }));
    }

    #[test]
    fn async_fd_registration_choice_prefers_ready_fd_from_cursor() {
        let (_first_writer, first_reader) =
            std::os::unix::net::UnixStream::pair().expect("first stream pair");
        let (mut second_writer, second_reader) =
            std::os::unix::net::UnixStream::pair().expect("second stream pair");
        first_reader
            .set_nonblocking(true)
            .expect("first reader nonblocking");
        second_reader
            .set_nonblocking(true)
            .expect("second reader nonblocking");
        second_writer
            .set_nonblocking(true)
            .expect("second writer nonblocking");
        std::io::Write::write_all(&mut second_writer, b"ready").expect("write readiness byte");
        let registrations = [
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(0),
                fd: first_reader.as_raw_fd(),
                interest: VmnetAsyncFdInterest::READABLE,
            },
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(1),
                fd: second_reader.as_raw_fd(),
                interest: VmnetAsyncFdInterest::READABLE,
            },
        ];
        let mut cursor = 0;

        let chosen = choose_async_fd_registration(&registrations, &mut cursor)
            .expect("chosen fd registration");

        assert_eq!(chosen.source, VmnetEventSource::HostListener(1));
        assert_eq!(cursor, 0);
    }

    #[test]
    fn async_fd_registration_choice_round_robins_when_none_ready() {
        let (_first_writer, first_reader) =
            std::os::unix::net::UnixStream::pair().expect("first stream pair");
        let (_second_writer, second_reader) =
            std::os::unix::net::UnixStream::pair().expect("second stream pair");
        first_reader
            .set_nonblocking(true)
            .expect("first reader nonblocking");
        second_reader
            .set_nonblocking(true)
            .expect("second reader nonblocking");
        let registrations = [
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(0),
                fd: first_reader.as_raw_fd(),
                interest: VmnetAsyncFdInterest::READABLE,
            },
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(1),
                fd: second_reader.as_raw_fd(),
                interest: VmnetAsyncFdInterest::READABLE,
            },
        ];
        let mut cursor = 1;

        let chosen = choose_async_fd_registration(&registrations, &mut cursor)
            .expect("chosen fd registration");

        assert_eq!(chosen.source, VmnetEventSource::HostListener(1));
        assert_eq!(cursor, 0);
    }

    #[tokio::test]
    async fn async_owner_event_snapshot_wait_receives_later_ready_fd() {
        let (_first_writer, first_reader) =
            std::os::unix::net::UnixStream::pair().expect("first stream pair");
        let (second_writer, second_reader) =
            std::os::unix::net::UnixStream::pair().expect("second stream pair");
        first_reader
            .set_nonblocking(true)
            .expect("first reader nonblocking");
        second_reader
            .set_nonblocking(true)
            .expect("second reader nonblocking");
        second_writer
            .set_nonblocking(true)
            .expect("second writer nonblocking");
        let second_fd = second_reader.as_raw_fd();
        let delayed_writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let mut writer = second_writer;
            std::io::Write::write_all(&mut writer, b"delayed-ready")
                .expect("write delayed readiness byte");
        });
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut dns_worker = spawn_dns_service_task::<MemoryConnection, _>(
            FailingDnsUpstream,
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");
        let registrations = [
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(0),
                fd: first_reader.as_raw_fd(),
                interest: VmnetAsyncFdInterest::READABLE,
            },
            VmnetAsyncFdRegistration {
                source: VmnetEventSource::HostListener(1),
                fd: second_fd,
                interest: VmnetAsyncFdInterest::READABLE,
            },
        ];
        let mut cursor = 0;

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_secs(30)),
                &registrations,
                &mut cursor,
            ),
        )
        .await
        .expect("owner event timeout")
        .expect("owner event");

        assert!(matches!(
            event,
            VmnetAsyncOwnerEvent::FdReady {
                source: VmnetEventSource::HostListener(1),
                readable: true,
                writable: false,
            }
        ));
        assert_eq!(cursor, 0);
        delayed_writer.await.expect("delayed writer task");
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_owner_event_select_receives_host_listener_fd_readiness() {
        let listeners = HostIngressListenerSet::bind(&[HostListener {
            host_addr: "127.0.0.1".to_string(),
            host_port: 0,
            guest_port: 8080,
            purpose: HostListenerPurpose::PublishedTcp,
        }])
        .expect("bind host listener");
        let addr = listeners
            .local_addrs()
            .expect("listener local addr")
            .into_iter()
            .next()
            .expect("listener addr");
        let listener_fd = listeners
            .listener_fds()
            .into_iter()
            .next()
            .expect("listener fd");
        let connect = tokio::spawn(async move {
            tokio::net::TcpStream::connect(addr)
                .await
                .expect("connect to host listener")
        });
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut dns_worker = spawn_dns_service_task::<MemoryConnection, _>(
            FailingDnsUpstream,
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");

        let mut fd_cursor = 0;
        let registrations = [VmnetAsyncFdRegistration {
            source: VmnetEventSource::HostListener(0),
            fd: listener_fd,
            interest: VmnetAsyncFdInterest::READABLE,
        }];
        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_secs(30)),
                &registrations,
                &mut fd_cursor,
            ),
        )
        .await
        .expect("owner event timeout")
        .expect("owner event");

        assert!(matches!(
            event,
            VmnetAsyncOwnerEvent::FdReady {
                source: VmnetEventSource::HostListener(0),
                readable: true,
                writable: false,
            }
        ));
        let batch = listeners.accept_ready_limited(0, 1);
        assert_eq!(batch.accepted.len(), 1);
        drop(batch);
        connect.await.expect("connect task");
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_owner_event_select_receives_session_fd_readiness() {
        let (_guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut runtime_io = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut dns_worker = spawn_dns_service_task::<MemoryConnection, _>(
            FailingDnsUpstream,
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");
        let handle = smoltcp::iface::SocketHandle::default();
        let (mut host_writer, host_reader) =
            std::os::unix::net::UnixStream::pair().expect("host stream pair");
        host_writer
            .set_nonblocking(true)
            .expect("host writer nonblocking");
        host_reader
            .set_nonblocking(true)
            .expect("host reader nonblocking");
        std::io::Write::write_all(&mut host_writer, b"host-ready")
            .expect("write host readiness byte");

        let mut fd_cursor = 0;
        let host_registrations = [VmnetAsyncFdRegistration {
            source: VmnetEventSource::HostSession(handle),
            fd: host_reader.as_raw_fd(),
            interest: VmnetAsyncFdInterest::READABLE,
        }];
        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_secs(30)),
                &host_registrations,
                &mut fd_cursor,
            ),
        )
        .await
        .expect("host owner event timeout")
        .expect("host owner event");
        assert!(matches!(
            event,
            VmnetAsyncOwnerEvent::FdReady {
                source: VmnetEventSource::HostSession(actual),
                readable: true,
                writable: false,
            } if actual == handle
        ));

        let (mut upstream_writer, upstream_reader) =
            std::os::unix::net::UnixStream::pair().expect("upstream stream pair");
        upstream_writer
            .set_nonblocking(true)
            .expect("upstream writer nonblocking");
        upstream_reader
            .set_nonblocking(true)
            .expect("upstream reader nonblocking");
        std::io::Write::write_all(&mut upstream_writer, b"upstream-ready")
            .expect("write upstream readiness byte");
        let upstream_registrations = [VmnetAsyncFdRegistration {
            source: VmnetEventSource::UpstreamSession(handle),
            fd: upstream_reader.as_raw_fd(),
            interest: VmnetAsyncFdInterest::READABLE,
        }];
        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_owner_event_with_fd_snapshot(
                &mut runtime_io,
                &mut dns_worker,
                &mut tcp_worker,
                Some(Duration::from_secs(30)),
                &upstream_registrations,
                &mut fd_cursor,
            ),
        )
        .await
        .expect("upstream owner event timeout")
        .expect("upstream owner event");
        assert!(matches!(
            event,
            VmnetAsyncOwnerEvent::FdReady {
                source: VmnetEventSource::UpstreamSession(actual),
                readable: true,
                writable: false,
            } if actual == handle
        ));
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_service_completion_apply_writes_dns_guest_frame_without_wakeup_fd() {
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
        let mut core = VmnetCore::new(gateway);
        let mut dns_service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("dns service");
        let mut dns_worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            block_reads: 0,
        });
        let mut tcp_service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("tcp service");
        let (guest_stream, runtime_stream) = tokio::io::duplex(4096);
        let mut reader = QemuFrameIo::new(guest_stream, DEFAULT_MAX_FRAME_LEN);
        let mut writer = QemuFrameIo::new(runtime_stream, DEFAULT_MAX_FRAME_LEN);
        let mut stats = VmnetRuntimeStats::default();

        let frame = handle_guest_frame_with_async_dns_worker(
            &mut core,
            &mut dns_service,
            &dns_worker,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        assert!(matches!(frame, VmnetDnsServiceFrame::Queued { .. }));

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_service_completion(&mut dns_worker, &mut tcp_worker),
        )
        .await
        .expect("service completion timeout");
        let applied = apply_async_service_completion_event(
            &mut writer,
            &mut core,
            &mut dns_service,
            &mut proxy,
            &mut tcp_service,
            event,
            Instant::from_millis(2),
            &mut stats,
            None,
        )
        .await
        .expect("apply service completion");

        assert_eq!(applied.guest_frames_written, 1);
        assert_eq!(stats.guest_frames_written, 1);
        assert_eq!(dns_service.pending_len(), 0);
        assert!(matches!(
            applied.gateway_events.as_slice(),
            [VmnetGatewayEvent::DnsQuery { .. }]
        ));
        let response_frame = reader
            .read_frame_async()
            .await
            .expect("read DNS response frame")
            .expect("DNS response frame");
        assert!(!response_frame.is_empty());
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[tokio::test]
    async fn async_service_completion_select_receives_tcp_without_wakeup_fd() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let mut core = VmnetCore::new(gateway);
        let mut proxy = TcpProxyBridge::new(PanicTcpConnector);
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let mut dns_worker = spawn_dns_service_task::<MemoryConnection, _>(
            FailingDnsUpstream,
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async dns worker");
        let mut tcp_worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp worker");

        let submit_events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut service,
            &tcp_worker,
            Instant::from_millis(1),
        );
        assert!(submit_events.is_empty());
        assert!(proxy.has_pending_connect(active.handle));

        let event = tokio::time::timeout(
            Duration::from_secs(1),
            next_async_service_completion(&mut dns_worker, &mut tcp_worker),
        )
        .await
        .expect("service completion timeout");
        let VmnetAsyncServiceCompletionEvent::TcpCompletion(completion) = event else {
            panic!("expected TCP completion event");
        };
        let events = apply_tcp_connect_service_completion(
            &mut core,
            &mut proxy,
            &mut service,
            completion,
            Instant::from_millis(2),
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::Connected { .. })));
        assert_eq!(service.pending_len(), 0);
        assert_eq!(proxy.session_handles(), vec![active.handle]);
        dns_worker.shutdown().await.expect("dns worker shutdown");
        tcp_worker.shutdown().await.expect("tcp worker shutdown");
    }

    #[test]
    fn dns_service_helper_queues_and_applies_completion() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("service");

        let step = handle_guest_frame_with_dns_service(
            &mut core,
            &mut service,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );

        let VmnetDnsServiceFrame::Queued { token } = step else {
            panic!("expected queued DNS service work");
        };
        assert_eq!(token.get(), 1);
        let command = service.service_recv_command().expect("dns command");
        assert_eq!(command.token(), token);
        service
            .service_complete(VmnetServiceCompletion::DnsLookup(
                VmnetDnsLookupCompletion {
                    token,
                    result: Err(DnsUpstreamError::Unavailable),
                },
            ))
            .expect("completion");
        let completion = service.owner_recv_completion().expect("owner completion");
        let result = apply_dns_service_completion(&mut core, &mut service, completion)
            .expect("applied completion");

        let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
            panic!("expected DNS result");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(result.guest_frames.len(), 1);
        assert_eq!(service.pending_len(), 0);
    }

    #[test]
    fn dns_service_helper_fails_closed_when_command_queue_is_full() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(1, 1))
                .expect("service");

        let first = handle_guest_frame_with_dns_service(
            &mut core,
            &mut service,
            test_support::dns_query_frame("one.example", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        assert!(matches!(first, VmnetDnsServiceFrame::Queued { .. }));
        let second = handle_guest_frame_with_dns_service(
            &mut core,
            &mut service,
            test_support::dns_query_frame("two.example", test_support::TEST_DNS_IP, 53001),
            Instant::from_millis(2),
        );

        let VmnetDnsServiceFrame::QueueFull(result) = second else {
            panic!("expected full queue DNS failure");
        };
        let GuestFrameOutcome::DnsQuery { log } = result.outcome else {
            panic!("expected DNS failure result");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(result.guest_frames.len(), 1);
        assert_eq!(service.pending_len(), 1);
        assert_eq!(service.command_len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_dns_worker_runtime_helper_applies_owner_side_response() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("service");
        let mut worker =
            spawn_dns_service_task::<(), _>(FailingDnsUpstream, VmnetServiceIoLimits::new(2, 2))
                .expect("async dns worker");

        let frame = handle_guest_frame_with_async_dns_worker(
            &mut core,
            &mut service,
            &worker,
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        let VmnetDnsServiceFrame::Queued { token } = frame else {
            panic!("expected queued async DNS service work");
        };
        assert_eq!(token.get(), 1);
        assert_eq!(service.pending_len(), 1);

        let drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let drain =
                    drain_async_dns_worker_completions(&mut core, &mut service, &mut worker);
                if !drain.guest_results.is_empty() || drain.disconnected {
                    break drain;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("async DNS completion timeout");

        assert!(!drain.disconnected);
        assert_eq!(drain.guest_results.len(), 1);
        let GuestFrameOutcome::DnsQuery { log } = &drain.guest_results[0].outcome else {
            panic!("expected DNS query outcome");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(drain.guest_results[0].guest_frames.len(), 1);
        assert_eq!(service.pending_len(), 0);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[test]
    fn tcp_connect_completion_success_is_applied_on_owner() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            block_reads: 0,
        });
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let destination = TcpDestination {
            ip: Ipv4Addr::from(PUBLIC_IP.octets()),
            port: 80,
            domain: None,
        };
        let TcpProxyConnectPlan::Pending(pending) =
            proxy.plan_connect(active.handle, destination.clone(), &policy)
        else {
            panic!("expected pending connect");
        };
        let token = match service.submit(pending, |pending, token| pending.service_command(token)) {
            Ok(token) => token,
            Err(_) => panic!("submit pending connect"),
        };
        let mut core = VmnetCore::new(gateway);

        let events = apply_tcp_connect_service_completion(
            &mut core,
            &mut proxy,
            &mut service,
            VmnetServiceCompletion::TcpConnect(VmnetTcpConnectCompletion {
                token,
                result: Ok(MemoryConnection {
                    response: Vec::new(),
                    written: Vec::new(),
                    block_reads: 0,
                }),
            }),
            Instant::from_millis(5),
        );

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::Connected {
                destination: event_destination,
                ..
            } if *event_destination == destination
        )));
        assert_eq!(service.pending_len(), 0);
        assert_eq!(proxy.session_handles(), vec![active.handle]);
    }

    #[test]
    fn tcp_connect_completion_failure_closes_guest_on_owner() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            block_reads: 0,
        });
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let destination = TcpDestination {
            ip: Ipv4Addr::from(PUBLIC_IP.octets()),
            port: 80,
            domain: None,
        };
        let TcpProxyConnectPlan::Pending(pending) =
            proxy.plan_connect(active.handle, destination, &policy)
        else {
            panic!("expected pending connect");
        };
        let token = match service.submit(pending, |pending, token| pending.service_command(token)) {
            Ok(token) => token,
            Err(_) => panic!("submit pending connect"),
        };
        let mut core = VmnetCore::new(gateway);

        let events = apply_tcp_connect_service_completion(
            &mut core,
            &mut proxy,
            &mut service,
            VmnetServiceCompletion::TcpConnect(VmnetTcpConnectCompletion {
                token,
                result: Err(TcpConnectError::UpstreamUnavailable),
            }),
            Instant::from_millis(5),
        );

        let failed = events
            .iter()
            .find(|event| matches!(event, TcpProxyEvent::ConnectFailed { .. }))
            .expect("connect failed event");
        let TcpProxyEvent::ConnectFailed { guest_frames, .. } = failed else {
            unreachable!();
        };
        assert!(!guest_frames.is_empty());
        assert_eq!(service.pending_len(), 0);
        assert!(proxy.session_handles().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_tcp_connect_worker_submission_marks_pending_and_skips_sync_connect() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let mut proxy = TcpProxyBridge::new(PanicTcpConnector);
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("tcp connect worker");

        let mut core = VmnetCore::new(gateway);
        let events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut service,
            &worker,
            Instant::from_millis(4),
        );

        assert!(events.is_empty());
        assert_eq!(service.pending_len(), 1);
        assert!(proxy.has_pending_connect(active.handle));
        assert!(proxy
            .process_gateway(core.gateway_mut(), Instant::from_millis(5))
            .is_empty());
        worker.shutdown().await.expect("worker shutdown");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_async_tcp_connect_worker_does_not_block_unrelated_tcp_syn() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let mut proxy = TcpProxyBridge::new(PanicTcpConnector);
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut worker = spawn_tcp_connect_service_task(
            BlockingConnector {
                started: started_tx,
                release: std::sync::Mutex::new(release_rx),
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp connect worker");

        let mut core = VmnetCore::new(gateway);
        let events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut service,
            &worker,
            Instant::from_millis(4),
        );
        assert!(events.is_empty());
        assert_eq!(service.pending_len(), 1);
        assert!(proxy.has_pending_connect(active.handle));
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                match started_rx.try_recv() {
                    Ok(()) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        panic!("async worker start channel disconnected")
                    }
                }
            }
        })
        .await
        .expect("async worker started blocked connect");

        let unrelated = core.handle_guest_frame(
            tcp_frame(81, TcpControl::Syn, TcpSeqNumber(700), None, &[]),
            Instant::from_millis(5),
        );
        assert!(matches!(
            unrelated.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));
        assert!(!unrelated.guest_frames.is_empty());
        assert_eq!(service.pending_len(), 1);

        release_tx.send(()).expect("release tcp connect worker");
        let drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let drain = drain_async_tcp_connect_worker_completions(
                    &mut core,
                    &mut proxy,
                    &mut service,
                    &mut worker,
                    Instant::from_millis(6),
                );
                if !drain.events.is_empty() || drain.disconnected {
                    break drain;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("async TCP completion timeout");
        assert!(!drain.disconnected);
        assert!(drain
            .events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::Connected { .. })));
        assert_eq!(service.pending_len(), 0);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_tcp_connect_worker_runtime_helper_applies_owner_side_success() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let mut proxy = TcpProxyBridge::new(PanicTcpConnector);
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let mut worker = spawn_tcp_connect_service_task(
            FakeConnector {
                response: Vec::new(),
                block_reads: 0,
            },
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("async tcp connect worker");
        let mut core = VmnetCore::new(gateway);

        let events = submit_tcp_connects_to_async_worker(
            &mut core,
            &mut proxy,
            &mut service,
            &worker,
            Instant::from_millis(4),
        );
        assert!(events.is_empty());
        assert_eq!(service.pending_len(), 1);
        assert!(proxy.has_pending_connect(active.handle));

        let drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let drain = drain_async_tcp_connect_worker_completions(
                    &mut core,
                    &mut proxy,
                    &mut service,
                    &mut worker,
                    Instant::from_millis(5),
                );
                if !drain.events.is_empty() || drain.disconnected {
                    break drain;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("async TCP completion timeout");

        assert!(!drain.disconnected);
        assert!(drain
            .events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::Connected { .. })));
        assert_eq!(service.pending_len(), 0);
        assert_eq!(proxy.session_handles(), vec![active.handle]);
        worker.shutdown().await.expect("worker shutdown");
    }

    #[test]
    fn dns_worker_disconnect_fails_pending_queries_closed_on_owner() {
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
        let mut core = VmnetCore::new(gateway);
        let mut service =
            VmnetServiceOwner::<VmnetPendingDnsQuery, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("service");
        let deferred = core.handle_guest_frame_with_deferred_dns(
            test_support::dns_query_frame("example.com", test_support::TEST_DNS_IP, 53000),
            Instant::from_millis(1),
        );
        let VmnetDeferredDnsFrame::Forward(pending) = deferred else {
            panic!("expected deferred dns query");
        };
        if service
            .submit_to(
                pending,
                |pending, token| pending.service_command(token),
                |_command| Ok::<(), ()>(()),
            )
            .is_err()
        {
            panic!("submit pending dns");
        }

        let results = fail_pending_dns_service_queries(&mut core, &mut service);

        assert_eq!(results.len(), 1);
        let GuestFrameOutcome::DnsQuery { log } = &results[0].outcome else {
            panic!("expected dns result");
        };
        assert_eq!(log.decision, DnsDecision::UpstreamFailure);
        assert_eq!(results[0].guest_frames.len(), 1);
        assert_eq!(service.pending_len(), 0);
    }

    #[test]
    fn tcp_worker_disconnect_fails_pending_connects_closed_on_owner() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            block_reads: 0,
        });
        let mut service = VmnetServiceOwner::<TcpProxyPendingConnect, MemoryConnection>::new(
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("service");
        let active = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active tcp session");
        let destination = TcpDestination {
            ip: Ipv4Addr::from(PUBLIC_IP.octets()),
            port: 80,
            domain: None,
        };
        let TcpProxyConnectPlan::Pending(pending) =
            proxy.plan_connect(active.handle, destination, &policy)
        else {
            panic!("expected pending connect");
        };
        if service
            .submit_to(
                pending,
                |pending, token| pending.service_command(token),
                |_command| Ok::<(), ()>(()),
            )
            .is_err()
        {
            panic!("submit pending connect");
        }
        proxy.mark_connect_pending(active.handle);

        let mut core = VmnetCore::new(gateway);
        let events =
            fail_pending_tcp_connects(&mut core, &mut proxy, &mut service, Instant::from_millis(5));

        let failed = events
            .iter()
            .find(|event| matches!(event, TcpProxyEvent::ConnectFailed { .. }))
            .expect("connect failed event");
        let TcpProxyEvent::ConnectFailed { guest_frames, .. } = failed else {
            unreachable!();
        };
        assert!(!guest_frames.is_empty());
        assert_eq!(service.pending_len(), 0);
        assert!(!proxy.has_pending_connect(active.handle));
        assert!(proxy.session_handles().is_empty());
    }

    #[test]
    fn stream_runtime_pumps_gateway_and_http_proxy_until_eof() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut core = VmnetCore::new(gateway);
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            block_reads: 0,
        });
        let mut io = QemuFrameIo::new(ScriptedIo::new(), DEFAULT_MAX_FRAME_LEN);
        let mut millis = 1;

        let stats = run_qemu_stream_until_eof(&mut io, &mut core, &mut proxy, || {
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
        let log_file = temp_file("vmnet-events.log");
        let log_path = log_file.path().to_path_buf();
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
        policy
            .egress
            .allow_ip_or_cidr(PUBLIC_IP)
            .expect("test allow ip");
        let gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut core = VmnetCore::new(gateway);
        let mut proxy = TcpProxyBridge::new(FakeConnector {
            response: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            block_reads: 2,
        });
        let mut io = QemuFrameIo::new(ScriptedIo::new(), DEFAULT_MAX_FRAME_LEN);
        let mut millis = 1;

        let stats = run_qemu_stream_until_eof(&mut io, &mut core, &mut proxy, || {
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
            &mut core,
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
        let log_file = temp_file("vmnet-failures.log");
        let log_path = log_file.path().to_path_buf();
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
                    guest_frames: Vec::new(),
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
                HostIngressEvent::AcceptQueueFull {
                    guest_port: 1075,
                    purpose: HostListenerPurpose::DockerApi,
                    capacity: 128,
                },
                HostIngressEvent::AcceptLimitReached {
                    listener_index: 0,
                    guest_port: 1075,
                    purpose: HostListenerPurpose::DockerApi,
                    limit: 64,
                    accepted: 64,
                },
                HostIngressEvent::HostWriteFailed {
                    handle,
                    error: "broken pipe".to_string(),
                },
                HostIngressEvent::HostReadLimitReached {
                    handle,
                    guest_port: 1075,
                    limit: 1024,
                    read: 1024,
                },
                HostIngressEvent::HostWriteLimitReached {
                    handle,
                    guest_port: 1075,
                    limit: 1024,
                    written: 1024,
                },
                HostIngressEvent::BufferLimitExceeded {
                    handle,
                    guest_port: 1075,
                    buffer: HostIngressBufferKind::PendingHostWrite,
                    limit: 1024,
                    attempted: 2048,
                    guest_frames: Vec::new(),
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
        assert!(log.contains("host_ingress_accept_queue_full"));
        assert!(log.contains("host_ingress_accept_limit_reached"));
        assert!(log.contains("host_ingress_host_write_failed"));
        assert!(log.contains("host_ingress_host_read_limit_reached"));
        assert!(log.contains("host_ingress_host_write_limit_reached"));
        assert!(log.contains("host_ingress_buffer_limit_exceeded"));
        assert!(!log.contains("BEGIN PRIVATE KEY"));
        assert!(!log.contains("mitm-ca.key"));
    }

    #[derive(Debug)]
    struct PanicDnsUpstream;

    impl DnsUpstream for PanicDnsUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            panic!("DNS service helper must not call upstream synchronously")
        }
    }

    #[derive(Debug)]
    struct FailingDnsUpstream;

    impl DnsUpstream for FailingDnsUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            Err(DnsUpstreamError::Unavailable)
        }
    }

    #[derive(Debug, Clone)]
    struct FakeConnector {
        response: Vec<u8>,
        block_reads: usize,
    }

    #[derive(Debug, Clone)]
    struct PanicTcpConnector;

    impl TcpUpstreamConnector for PanicTcpConnector {
        type Connection = MemoryConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            panic!("unexpected synchronous TCP connect")
        }
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
    struct BlockingConnector {
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl TcpUpstreamConnector for BlockingConnector {
        type Connection = MemoryConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            self.started.send(()).expect("signal blocked connect start");
            self.release
                .lock()
                .expect("blocking connect mutex")
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| TcpConnectError::UpstreamUnavailable)?;
            Ok(MemoryConnection {
                response: Vec::new(),
                written: Vec::new(),
                block_reads: 0,
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

    fn establish_tcp_session(gateway: &mut VmnetGateway<'_>, dst_port: u16) -> TcpSeqNumber {
        let syn_result = gateway.handle_guest_frame(
            tcp_frame(dst_port, TcpControl::Syn, TcpSeqNumber(100), None, &[]),
            Instant::from_millis(1),
        );
        assert!(matches!(
            syn_result.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));

        let syn_ack_result = gateway.handle_guest_frame(arp_reply_frame(), Instant::from_millis(2));
        let syn_ack = parse_tcp_reply(&syn_ack_result.guest_frames[0]).expect("syn ack");
        let server_ack = syn_ack.seq_number + 1;

        gateway.handle_guest_frame(
            tcp_frame(
                dst_port,
                TcpControl::None,
                TcpSeqNumber(101),
                Some(server_ack),
                &[],
            ),
            Instant::from_millis(3),
        );
        server_ack
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

    fn temp_file(name: &str) -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .prefix(&format!("agentvm-frontend-{name}-"))
            .tempfile()
            .expect("temp file")
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
