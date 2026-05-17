use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind, Read, Write};
use std::net::Ipv4Addr;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::IpAddress;

use crate::guest_tcp::GuestTcpSession;
use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::{
    evaluate_tcp_destination, parse_http_request, HttpParseError, HttpRequestSummary, TcpAction,
    TcpConnectError, TcpDecision, TcpDestination, TcpUpstreamConnector,
};
use crate::tls_mitm::{
    rustls_client_config_with_native_roots, GuestTlsSession, TlsMitmAuthority, TlsMitmError,
    TlsUpstreamSession,
};
use crate::vmnet_gateway::VmnetGateway;
use crate::vmnet_service_io::{VmnetServiceCommand, VmnetServiceToken, VmnetTcpConnectCommand};

pub struct TcpProxyBridge<C>
where
    C: TcpUpstreamConnector,
{
    connector: C,
    sessions: HashMap<SocketHandle, UpstreamSession<C::Connection>>,
    pending_connects: HashSet<SocketHandle>,
    tls_server_config: Option<Arc<rustls::ServerConfig>>,
    tls_client_config: Option<Arc<rustls::ClientConfig>>,
    buffer_limits: TcpProxyBufferLimits,
}

impl<C> TcpProxyBridge<C>
where
    C: TcpUpstreamConnector,
{
    pub fn new(connector: C) -> Self {
        Self {
            connector,
            sessions: HashMap::new(),
            pending_connects: HashSet::new(),
            tls_server_config: None,
            tls_client_config: None,
            buffer_limits: TcpProxyBufferLimits::default(),
        }
    }

    pub fn with_tls_mitm(
        connector: C,
        authority: Arc<TlsMitmAuthority>,
    ) -> Result<Self, TlsMitmError> {
        Self::with_tls_mitm_and_client_config(
            connector,
            authority,
            Arc::new(rustls_client_config_with_native_roots()?),
        )
    }

    fn with_tls_mitm_and_client_config(
        connector: C,
        authority: Arc<TlsMitmAuthority>,
        tls_client_config: Arc<rustls::ClientConfig>,
    ) -> Result<Self, TlsMitmError> {
        Ok(Self {
            connector,
            sessions: HashMap::new(),
            pending_connects: HashSet::new(),
            tls_server_config: Some(Arc::new(authority.rustls_server_config()?)),
            tls_client_config: Some(tls_client_config),
            buffer_limits: TcpProxyBufferLimits::default(),
        })
    }

    #[cfg(test)]
    fn with_buffer_limits(mut self, buffer_limits: TcpProxyBufferLimits) -> Self {
        self.buffer_limits = buffer_limits;
        self
    }

    pub(crate) fn plan_connect(
        &self,
        handle: SocketHandle,
        destination: TcpDestination,
        policy: &VmnetPolicy,
    ) -> TcpProxyConnectPlan {
        let decision = evaluate_tcp_destination(policy, &destination);
        if decision.action == TcpAction::Deny {
            return TcpProxyConnectPlan::Event(TcpProxyEvent::Denied {
                handle,
                destination,
                decision,
            });
        }
        let guest_tls = if decision.action == TcpAction::InterceptHttps {
            let Some(config) = &self.tls_server_config else {
                return TcpProxyConnectPlan::Event(TcpProxyEvent::TlsMitmUnavailable {
                    handle,
                    destination,
                });
            };
            match GuestTlsSession::new(config.clone()) {
                Ok(session) => Some(session),
                Err(error) => {
                    return TcpProxyConnectPlan::Event(TcpProxyEvent::TlsMitmFailed {
                        handle,
                        error,
                        guest_frames: Vec::new(),
                    });
                }
            }
        } else {
            None
        };
        TcpProxyConnectPlan::Pending(TcpProxyPendingConnect {
            handle,
            destination,
            decision,
            guest_tls,
        })
    }

    pub(crate) fn complete_connect(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        pending: TcpProxyPendingConnect,
        result: Result<C::Connection, TcpConnectError>,
        now: Instant,
        events: &mut Vec<TcpProxyEvent>,
    ) -> Result<(), TcpConnectError> {
        self.pending_connects.remove(&pending.handle);
        match result {
            Ok(connection) => {
                events.push(TcpProxyEvent::Connected {
                    handle: pending.handle,
                    destination: pending.destination.clone(),
                    decision: pending.decision.clone(),
                });
                self.sessions.insert(
                    pending.handle,
                    UpstreamSession {
                        destination: pending.destination,
                        decision: pending.decision,
                        connection,
                        http_buffer: Vec::new(),
                        guest_tls: pending.guest_tls,
                        upstream_tls: None,
                        upstream_tls_ready: false,
                        pending_upstream_bytes: Vec::new(),
                        pending_upstream_plaintext: Vec::new(),
                        pending_guest_bytes: Vec::new(),
                        buffer_limits: self.buffer_limits,
                    },
                );
                Ok(())
            }
            Err(error) => {
                let guest_frames = gateway.close_tcp_session(pending.handle, now);
                events.push(TcpProxyEvent::ConnectFailed {
                    handle: pending.handle,
                    destination: pending.destination,
                    error: error.clone(),
                    guest_frames,
                });
                Err(error)
            }
        }
    }

    pub(crate) fn plan_connects_for_service(
        &self,
        gateway: &VmnetGateway<'_>,
    ) -> Vec<TcpProxyConnectPlan> {
        let mut plans = Vec::new();
        for active in gateway.active_tcp_sessions() {
            if active.session.state != tcp::State::Established {
                continue;
            }
            if self.sessions.contains_key(&active.handle)
                || self.pending_connects.contains(&active.handle)
            {
                continue;
            }
            let Some(destination) = destination_from_session(&active.session) else {
                continue;
            };
            plans.push(self.plan_connect(active.handle, destination, gateway.policy()));
        }
        plans
    }

    pub fn process_gateway(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
    ) -> Vec<TcpProxyEvent>
    where
        C::Connection: Read + Write,
    {
        self.process_gateway_with_readiness(gateway, now, TcpProxyReadiness::all())
    }

    pub fn process_gateway_with_readiness(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
        readiness: TcpProxyReadiness,
    ) -> Vec<TcpProxyEvent>
    where
        C::Connection: Read + Write,
    {
        let mut events = Vec::new();
        let active_sessions = gateway.active_tcp_sessions();
        let active_handles = active_sessions
            .iter()
            .filter(|active| active.session.state == tcp::State::Established)
            .map(|active| active.handle)
            .collect::<HashSet<_>>();
        self.pending_connects
            .retain(|handle| active_handles.contains(handle));
        for active in active_sessions {
            if active.session.state != tcp::State::Established {
                continue;
            }
            let Some(destination) = destination_from_session(&active.session) else {
                continue;
            };

            if !self.sessions.contains_key(&active.handle) {
                if self.pending_connects.contains(&active.handle) {
                    continue;
                }
                let pending = match self.plan_connect(active.handle, destination, gateway.policy())
                {
                    TcpProxyConnectPlan::Pending(pending) => pending,
                    TcpProxyConnectPlan::Event(event) => {
                        events.push(event);
                        continue;
                    }
                };
                let result = self.connector.connect(&pending.destination);
                if self
                    .complete_connect(gateway, pending, result, now, &mut events)
                    .is_err()
                {
                    continue;
                }
            }

            let Some(session) = self.sessions.get_mut(&active.handle) else {
                continue;
            };
            flush_pending_guest_bytes(
                session,
                active.handle,
                gateway,
                now,
                GuestSendEvent::UpstreamPayload,
                &mut events,
            );
            if !session.pending_guest_bytes.is_empty() {
                continue;
            }

            let guest_bytes = match gateway.recv_tcp_session(active.handle) {
                Ok(bytes) => bytes,
                Err(error) => {
                    events.push(TcpProxyEvent::GuestReadFailed {
                        handle: active.handle,
                        error,
                    });
                    continue;
                }
            };
            if !guest_bytes.is_empty() {
                self.process_guest_payload(
                    active.handle,
                    guest_bytes,
                    gateway,
                    now,
                    readiness.writable(active.handle),
                    &mut events,
                );
            }

            let Some(session) = self.sessions.get_mut(&active.handle) else {
                continue;
            };
            flush_pending_guest_bytes(
                session,
                active.handle,
                gateway,
                now,
                GuestSendEvent::UpstreamPayload,
                &mut events,
            );
            if !session.pending_guest_bytes.is_empty() {
                continue;
            }
            if readiness.writable(active.handle) {
                flush_pending_https_plaintext(session, active.handle, gateway, now, &mut events);
                drain_upstream_tls_writes(session, active.handle, gateway, now, &mut events);
                flush_pending_upstream_bytes(session, active.handle, gateway, now, &mut events);
            }
            if readiness.readable(active.handle) {
                let upstream_bytes = match read_available(
                    &mut session.connection,
                    &mut events,
                    active.handle,
                    session.buffer_limits.pending_guest_bytes,
                ) {
                    UpstreamRead::Data(bytes) => bytes,
                    UpstreamRead::WouldBlock => continue,
                    UpstreamRead::Closed | UpstreamRead::Failed => {
                        let _guest_frames = gateway.close_tcp_session(active.handle, now);
                        self.sessions.remove(&active.handle);
                        continue;
                    }
                };
                let guest_bytes = if session.decision.action == TcpAction::InterceptHttps {
                    let upstream_read = match session
                        .upstream_tls
                        .as_mut()
                        .ok_or_else(|| {
                            TlsMitmError::Tls("upstream TLS is not initialized".to_string())
                        })
                        .and_then(|upstream_tls| upstream_tls.read_upstream_tls(&upstream_bytes))
                    {
                        Ok(read) => read,
                        Err(error) => {
                            events.push(tls_mitm_failed_event(active.handle, gateway, now, error));
                            continue;
                        }
                    };
                    if !upstream_read.tls_to_upstream.is_empty() {
                        if readiness.writable(active.handle) {
                            let Some(bytes) = write_pending_upstream_bytes_best_effort(
                                session,
                                active.handle,
                                gateway,
                                now,
                                &upstream_read.tls_to_upstream,
                                &mut events,
                            ) else {
                                continue;
                            };
                            if bytes > 0 {
                                events.push(TcpProxyEvent::TlsUpstreamPayload {
                                    handle: active.handle,
                                    bytes,
                                });
                            }
                        } else if !buffer_pending_upstream_bytes(
                            session,
                            active.handle,
                            gateway,
                            now,
                            &upstream_read.tls_to_upstream,
                            &mut events,
                        ) {
                            continue;
                        }
                    }
                    if !session.upstream_tls_ready {
                        session.upstream_tls_ready = session
                            .upstream_tls
                            .as_ref()
                            .is_some_and(|upstream_tls| !upstream_tls.is_handshaking());
                    }
                    if readiness.writable(active.handle) {
                        flush_pending_https_plaintext(
                            session,
                            active.handle,
                            gateway,
                            now,
                            &mut events,
                        );
                        drain_upstream_tls_writes(
                            session,
                            active.handle,
                            gateway,
                            now,
                            &mut events,
                        );
                    }
                    if upstream_read.plaintext.is_empty() {
                        continue;
                    }
                    let Some(guest_tls) = session.guest_tls.as_mut() else {
                        events.push(TcpProxyEvent::TlsMitmUnavailable {
                            handle: active.handle,
                            destination: session.destination.clone(),
                        });
                        continue;
                    };
                    match guest_tls.write_guest_plaintext(&upstream_read.plaintext) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            events.push(tls_mitm_failed_event(active.handle, gateway, now, error));
                            continue;
                        }
                    }
                } else {
                    upstream_bytes.clone()
                };
                send_guest_buffered(
                    session,
                    active.handle,
                    gateway,
                    now,
                    &guest_bytes,
                    GuestSendEvent::UpstreamPayload,
                    &mut events,
                );
            }
        }
        self.sessions
            .retain(|handle, _session| active_handles.contains(handle));
        events
    }

    fn process_guest_payload(
        &mut self,
        handle: SocketHandle,
        guest_bytes: Vec<u8>,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
        upstream_writable: bool,
        events: &mut Vec<TcpProxyEvent>,
    ) where
        C::Connection: Write,
    {
        let Some(session) = self.sessions.get_mut(&handle) else {
            return;
        };

        let payload = if session.decision.action == TcpAction::InterceptHttps {
            let Some(guest_tls) = session.guest_tls.as_mut() else {
                events.push(TcpProxyEvent::TlsMitmUnavailable {
                    handle,
                    destination: session.destination.clone(),
                });
                return;
            };
            let read = match guest_tls.read_guest_tls(&guest_bytes) {
                Ok(read) => read,
                Err(error) => {
                    events.push(tls_mitm_failed_event(handle, gateway, now, error));
                    return;
                }
            };
            if !read.tls_to_guest.is_empty() {
                send_guest_buffered(
                    session,
                    handle,
                    gateway,
                    now,
                    &read.tls_to_guest,
                    GuestSendEvent::TlsHandshakePayload,
                    events,
                );
            }
            if !ensure_https_upstream_tls(
                session,
                handle,
                gateway,
                now,
                self.tls_client_config.clone(),
                upstream_writable,
                events,
            ) {
                return;
            }
            read.plaintext
        } else {
            guest_bytes
        };

        if session.decision.action == TcpAction::InterceptHttp
            || session.decision.action == TcpAction::InterceptHttps
        {
            record_http_request(
                handle,
                &session.destination,
                &mut session.http_buffer,
                &payload,
                events,
            );
        }

        if payload.is_empty() {
            return;
        }

        if session.decision.action == TcpAction::InterceptHttps {
            if !session.upstream_tls_ready {
                let _ = buffer_pending_upstream_plaintext(
                    session, handle, gateway, now, &payload, events,
                );
                return;
            }
            if upstream_writable {
                write_https_plaintext_upstream(session, handle, gateway, now, &payload, events);
            } else {
                let _ = buffer_pending_upstream_plaintext(
                    session, handle, gateway, now, &payload, events,
                );
            }
        } else {
            if upstream_writable {
                if let Some(bytes) = write_pending_upstream_bytes_best_effort(
                    session, handle, gateway, now, &payload, events,
                ) {
                    events.push(TcpProxyEvent::GuestPayload { handle, bytes });
                }
            } else {
                let _ =
                    buffer_pending_upstream_bytes(session, handle, gateway, now, &payload, events);
            }
        }
    }
}

impl TcpProxyBridge<crate::tcp_gateway::MappedTcpConnector> {
    pub fn session_raw_fd(&self, handle: SocketHandle) -> Option<RawFd> {
        self.sessions
            .get(&handle)
            .map(|session| session.connection.as_raw_fd())
    }
}

impl<C> TcpProxyBridge<C>
where
    C: TcpUpstreamConnector,
{
    pub fn session_handles(&self) -> Vec<SocketHandle> {
        self.sessions.keys().copied().collect()
    }

    pub(crate) fn mark_connect_pending(&mut self, handle: SocketHandle) {
        self.pending_connects.insert(handle);
    }

    #[cfg(test)]
    pub(crate) fn has_pending_connect(&self, handle: SocketHandle) -> bool {
        self.pending_connects.contains(&handle)
    }

    pub fn session_interest(&self, handle: SocketHandle) -> Option<UpstreamSessionInterest> {
        let session = self.sessions.get(&handle)?;
        Some(UpstreamSessionInterest {
            readable: session.pending_guest_bytes.is_empty(),
            writable: !session.pending_upstream_bytes.is_empty()
                || !session.pending_upstream_plaintext.is_empty()
                || session
                    .upstream_tls
                    .as_ref()
                    .is_some_and(|upstream_tls| upstream_tls.wants_write()),
        })
    }
}

pub(crate) enum TcpProxyConnectPlan {
    Pending(TcpProxyPendingConnect),
    Event(TcpProxyEvent),
}

pub(crate) struct TcpProxyPendingConnect {
    pub handle: SocketHandle,
    pub destination: TcpDestination,
    pub decision: TcpDecision,
    guest_tls: Option<GuestTlsSession>,
}

impl TcpProxyPendingConnect {
    #[allow(dead_code)]
    pub(crate) fn service_command(&self, token: VmnetServiceToken) -> VmnetServiceCommand {
        VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
            token,
            destination: self.destination.clone(),
        })
    }
}

struct UpstreamSession<T> {
    destination: TcpDestination,
    decision: TcpDecision,
    connection: T,
    http_buffer: Vec<u8>,
    guest_tls: Option<GuestTlsSession>,
    upstream_tls: Option<TlsUpstreamSession>,
    upstream_tls_ready: bool,
    pending_upstream_bytes: Vec<u8>,
    pending_upstream_plaintext: Vec<u8>,
    pending_guest_bytes: Vec<u8>,
    buffer_limits: TcpProxyBufferLimits,
}

const DEFAULT_TCP_PROXY_BUFFER_LIMIT: usize = 1024 * 1024;
const DEFAULT_TCP_PROXY_PENDING_GUEST_BUFFER_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpProxyBufferLimits {
    pub pending_upstream_bytes: usize,
    pub pending_upstream_plaintext: usize,
    pub pending_guest_bytes: usize,
}

impl TcpProxyBufferLimits {
    pub const fn new(pending_upstream_plaintext: usize) -> Self {
        Self {
            pending_upstream_bytes: DEFAULT_TCP_PROXY_BUFFER_LIMIT,
            pending_upstream_plaintext,
            pending_guest_bytes: DEFAULT_TCP_PROXY_PENDING_GUEST_BUFFER_LIMIT,
        }
    }

    pub const fn with_upstream_bytes(
        pending_upstream_bytes: usize,
        pending_upstream_plaintext: usize,
    ) -> Self {
        Self {
            pending_upstream_bytes,
            pending_upstream_plaintext,
            pending_guest_bytes: DEFAULT_TCP_PROXY_PENDING_GUEST_BUFFER_LIMIT,
        }
    }

    pub const fn with_all(
        pending_upstream_bytes: usize,
        pending_upstream_plaintext: usize,
        pending_guest_bytes: usize,
    ) -> Self {
        Self {
            pending_upstream_bytes,
            pending_upstream_plaintext,
            pending_guest_bytes,
        }
    }
}

impl Default for TcpProxyBufferLimits {
    fn default() -> Self {
        Self::with_all(
            DEFAULT_TCP_PROXY_BUFFER_LIMIT,
            DEFAULT_TCP_PROXY_BUFFER_LIMIT,
            DEFAULT_TCP_PROXY_PENDING_GUEST_BUFFER_LIMIT,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpProxyBufferKind {
    PendingUpstreamBytes,
    PendingUpstreamPlaintext,
    PendingGuestBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamSessionInterest {
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpProxyReadiness {
    poll_all: bool,
    readable: Vec<SocketHandle>,
    writable: Vec<SocketHandle>,
}

impl TcpProxyReadiness {
    pub fn all() -> Self {
        Self {
            poll_all: true,
            readable: Vec::new(),
            writable: Vec::new(),
        }
    }

    pub fn selected(readable: Vec<SocketHandle>, writable: Vec<SocketHandle>) -> Self {
        Self {
            poll_all: false,
            readable,
            writable,
        }
    }

    fn readable(&self, handle: SocketHandle) -> bool {
        self.poll_all || self.readable.contains(&handle)
    }

    fn writable(&self, handle: SocketHandle) -> bool {
        self.poll_all || self.writable.contains(&handle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestSendEvent {
    UpstreamPayload,
    TlsHandshakePayload,
}

fn tls_mitm_failed_event(
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    error: TlsMitmError,
) -> TcpProxyEvent {
    TcpProxyEvent::TlsMitmFailed {
        handle,
        error,
        guest_frames: gateway.close_tcp_session(handle, now),
    }
}

fn push_buffer_limit_exceeded(
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    buffer: TcpProxyBufferKind,
    limit: usize,
    attempted: usize,
    events: &mut Vec<TcpProxyEvent>,
) {
    events.push(TcpProxyEvent::BufferLimitExceeded {
        handle,
        buffer,
        limit,
        attempted,
        guest_frames: gateway.close_tcp_session(handle, now),
    });
}

fn buffer_pending_upstream_bytes<T>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    bytes: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) -> bool {
    let limit = session.buffer_limits.pending_upstream_bytes;
    let attempted = session.pending_upstream_bytes.len() + bytes.len();
    if attempted > limit {
        push_buffer_limit_exceeded(
            handle,
            gateway,
            now,
            TcpProxyBufferKind::PendingUpstreamBytes,
            limit,
            attempted,
            events,
        );
        return false;
    }
    session.pending_upstream_bytes.extend_from_slice(bytes);
    true
}

fn write_pending_upstream_bytes_best_effort<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    bytes: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) -> Option<usize> {
    let limit = session.buffer_limits.pending_upstream_bytes;
    let attempted = session.pending_upstream_bytes.len() + bytes.len();
    if attempted > limit {
        push_buffer_limit_exceeded(
            handle,
            gateway,
            now,
            TcpProxyBufferKind::PendingUpstreamBytes,
            limit,
            attempted,
            events,
        );
        return None;
    }
    match write_buffered_best_effort(
        &mut session.connection,
        &mut session.pending_upstream_bytes,
        bytes,
    ) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error });
            None
        }
    }
}

fn buffer_pending_upstream_plaintext<T>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    bytes: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) -> bool {
    let limit = session.buffer_limits.pending_upstream_plaintext;
    let attempted = session.pending_upstream_plaintext.len() + bytes.len();
    if attempted > limit {
        push_buffer_limit_exceeded(
            handle,
            gateway,
            now,
            TcpProxyBufferKind::PendingUpstreamPlaintext,
            limit,
            attempted,
            events,
        );
        return false;
    }
    session.pending_upstream_plaintext.extend_from_slice(bytes);
    true
}

fn send_guest_buffered<T>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    bytes: &[u8],
    event: GuestSendEvent,
    events: &mut Vec<TcpProxyEvent>,
) {
    flush_pending_guest_bytes(session, handle, gateway, now, event, events);
    if session.pending_guest_bytes.is_empty() {
        match gateway.send_tcp_session_partial(handle, bytes, now) {
            Ok((written, guest_frames)) => {
                if written > 0 {
                    push_guest_send_event(events, event, handle, written, guest_frames);
                }
                if written == bytes.len() {
                    return;
                }
                queue_pending_guest_bytes(session, handle, gateway, now, &bytes[written..], events);
            }
            Err(error) => {
                events.push(TcpProxyEvent::GuestWriteFailed { handle, error });
                queue_pending_guest_bytes(session, handle, gateway, now, bytes, events);
            }
        }
        return;
    }

    queue_pending_guest_bytes(session, handle, gateway, now, bytes, events);
}

fn queue_pending_guest_bytes<T>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    bytes: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) -> bool {
    let limit = session.buffer_limits.pending_guest_bytes;
    let attempted = session.pending_guest_bytes.len() + bytes.len();
    if attempted > limit {
        push_buffer_limit_exceeded(
            handle,
            gateway,
            now,
            TcpProxyBufferKind::PendingGuestBytes,
            limit,
            attempted,
            events,
        );
        return false;
    }
    session.pending_guest_bytes.extend_from_slice(bytes);
    true
}

fn push_guest_send_event(
    events: &mut Vec<TcpProxyEvent>,
    event: GuestSendEvent,
    handle: SocketHandle,
    bytes: usize,
    guest_frames: Vec<Vec<u8>>,
) {
    match event {
        GuestSendEvent::UpstreamPayload => {
            events.push(TcpProxyEvent::UpstreamPayload {
                handle,
                bytes,
                guest_frames,
            });
        }
        GuestSendEvent::TlsHandshakePayload => {
            events.push(TcpProxyEvent::TlsHandshakePayload {
                handle,
                bytes,
                guest_frames,
            });
        }
    }
}

fn flush_pending_guest_bytes<T>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    event: GuestSendEvent,
    events: &mut Vec<TcpProxyEvent>,
) {
    if session.pending_guest_bytes.is_empty() {
        return;
    }
    match gateway.send_tcp_session_partial(handle, &session.pending_guest_bytes, now) {
        Ok((bytes, guest_frames)) => {
            if bytes > 0 {
                session.pending_guest_bytes.drain(..bytes);
                push_guest_send_event(events, event, handle, bytes, guest_frames);
            }
        }
        Err(error) => events.push(TcpProxyEvent::GuestWriteFailed { handle, error }),
    }
}

fn ensure_https_upstream_tls<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    tls_client_config: Option<Arc<rustls::ClientConfig>>,
    upstream_writable: bool,
    events: &mut Vec<TcpProxyEvent>,
) -> bool {
    if session.upstream_tls.is_some() {
        return true;
    }

    let Some(guest_tls) = session.guest_tls.as_ref() else {
        events.push(TcpProxyEvent::TlsMitmUnavailable {
            handle,
            destination: session.destination.clone(),
        });
        return false;
    };
    let Some(server_name) = guest_tls.server_name().map(ToOwned::to_owned) else {
        if !guest_tls.is_handshaking() {
            events.push(tls_mitm_failed_event(
                handle,
                gateway,
                now,
                TlsMitmError::InvalidHost("guest TLS connection did not provide SNI".to_string()),
            ));
        }
        return false;
    };
    let Some(config) = tls_client_config else {
        events.push(TcpProxyEvent::TlsMitmUnavailable {
            handle,
            destination: session.destination.clone(),
        });
        return false;
    };
    let mut upstream_tls = match TlsUpstreamSession::new(config, &server_name) {
        Ok(upstream_tls) => upstream_tls,
        Err(error) => {
            events.push(tls_mitm_failed_event(handle, gateway, now, error));
            return false;
        }
    };
    let client_hello = match upstream_tls.drain_tls_to_upstream() {
        Ok(client_hello) => client_hello,
        Err(error) => {
            events.push(tls_mitm_failed_event(handle, gateway, now, error));
            return false;
        }
    };
    if !client_hello.is_empty() {
        if upstream_writable {
            let Some(bytes) = write_pending_upstream_bytes_best_effort(
                session,
                handle,
                gateway,
                now,
                &client_hello,
                events,
            ) else {
                return false;
            };
            if bytes > 0 {
                events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
            }
        } else if !buffer_pending_upstream_bytes(
            session,
            handle,
            gateway,
            now,
            &client_hello,
            events,
        ) {
            return false;
        }
    }
    session.upstream_tls = Some(upstream_tls);
    true
}

fn flush_pending_https_plaintext<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    events: &mut Vec<TcpProxyEvent>,
) {
    if session.pending_upstream_plaintext.is_empty() {
        return;
    }
    if !session.upstream_tls_ready {
        return;
    }
    let pending = std::mem::take(&mut session.pending_upstream_plaintext);
    write_https_plaintext_upstream(session, handle, gateway, now, &pending, events);
}

fn write_https_plaintext_upstream<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    plaintext: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) {
    let Some(upstream_tls) = session.upstream_tls.as_mut() else {
        let _ = buffer_pending_upstream_plaintext(session, handle, gateway, now, plaintext, events);
        return;
    };
    let tls_bytes = match upstream_tls.write_upstream_plaintext(plaintext) {
        Ok(tls_bytes) => tls_bytes,
        Err(error) => {
            events.push(tls_mitm_failed_event(handle, gateway, now, error));
            return;
        }
    };
    if let Some(bytes) =
        write_pending_upstream_bytes_best_effort(session, handle, gateway, now, &tls_bytes, events)
    {
        events.push(TcpProxyEvent::GuestPayload {
            handle,
            bytes: plaintext.len(),
        });
        if bytes > 0 {
            events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
        }
    }
}

fn drain_upstream_tls_writes<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    events: &mut Vec<TcpProxyEvent>,
) {
    if session.decision.action != TcpAction::InterceptHttps {
        return;
    }
    let Some(upstream_tls) = session.upstream_tls.as_mut() else {
        return;
    };
    let tls_bytes = match upstream_tls.drain_tls_to_upstream() {
        Ok(tls_bytes) => tls_bytes,
        Err(error) => {
            events.push(tls_mitm_failed_event(handle, gateway, now, error));
            return;
        }
    };
    if tls_bytes.is_empty() {
        return;
    }
    if let Some(bytes) =
        write_pending_upstream_bytes_best_effort(session, handle, gateway, now, &tls_bytes, events)
    {
        if bytes > 0 {
            events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
        }
    }
}

fn flush_pending_upstream_bytes<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    gateway: &mut VmnetGateway<'_>,
    now: Instant,
    events: &mut Vec<TcpProxyEvent>,
) {
    if session.pending_upstream_bytes.is_empty() {
        return;
    }
    match write_buffered_best_effort(
        &mut session.connection,
        &mut session.pending_upstream_bytes,
        &[],
    ) {
        Ok(bytes) if bytes > 0 && session.decision.action == TcpAction::InterceptHttps => {
            events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
        }
        Ok(bytes) if bytes > 0 => events.push(TcpProxyEvent::GuestPayload { handle, bytes }),
        Ok(_) => {}
        Err(error) => events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error }),
    }
    if session.decision.action == TcpAction::InterceptHttps
        && session.pending_upstream_bytes.is_empty()
        && session
            .upstream_tls
            .as_ref()
            .is_some_and(|upstream_tls| !upstream_tls.is_handshaking())
    {
        session.upstream_tls_ready = true;
        flush_pending_https_plaintext(session, handle, gateway, now, events);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TcpProxyEvent {
    Connected {
        handle: SocketHandle,
        destination: TcpDestination,
        decision: TcpDecision,
    },
    Denied {
        handle: SocketHandle,
        destination: TcpDestination,
        decision: TcpDecision,
    },
    ConnectFailed {
        handle: SocketHandle,
        destination: TcpDestination,
        error: TcpConnectError,
        guest_frames: Vec<Vec<u8>>,
    },
    GuestPayload {
        handle: SocketHandle,
        bytes: usize,
    },
    HttpRequest {
        handle: SocketHandle,
        destination: TcpDestination,
        summary: HttpRequestSummary,
    },
    HttpRequestIncomplete {
        handle: SocketHandle,
    },
    HttpRequestMalformed {
        handle: SocketHandle,
    },
    UpstreamPayload {
        handle: SocketHandle,
        bytes: usize,
        guest_frames: Vec<Vec<u8>>,
    },
    TlsHandshakePayload {
        handle: SocketHandle,
        bytes: usize,
        guest_frames: Vec<Vec<u8>>,
    },
    TlsUpstreamPayload {
        handle: SocketHandle,
        bytes: usize,
    },
    TlsMitmUnavailable {
        handle: SocketHandle,
        destination: TcpDestination,
    },
    TlsMitmFailed {
        handle: SocketHandle,
        error: TlsMitmError,
        guest_frames: Vec<Vec<u8>>,
    },
    BufferLimitExceeded {
        handle: SocketHandle,
        buffer: TcpProxyBufferKind,
        limit: usize,
        attempted: usize,
        guest_frames: Vec<Vec<u8>>,
    },
    GuestReadFailed {
        handle: SocketHandle,
        error: tcp::RecvError,
    },
    GuestWriteFailed {
        handle: SocketHandle,
        error: tcp::SendError,
    },
    UpstreamWriteFailed {
        handle: SocketHandle,
        error: String,
    },
    UpstreamReadFailed {
        handle: SocketHandle,
        error: String,
    },
}

fn record_http_request(
    handle: SocketHandle,
    destination: &TcpDestination,
    http_buffer: &mut Vec<u8>,
    payload: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) {
    if payload.is_empty() {
        return;
    }
    http_buffer.extend_from_slice(payload);
    match parse_http_request(http_buffer) {
        Ok(Some(summary)) => events.push(TcpProxyEvent::HttpRequest {
            handle,
            destination: destination.clone(),
            summary,
        }),
        Ok(None) => events.push(TcpProxyEvent::HttpRequestIncomplete { handle }),
        Err(HttpParseError::Malformed) => {
            events.push(TcpProxyEvent::HttpRequestMalformed { handle })
        }
    }
}

fn destination_from_session(session: &GuestTcpSession) -> Option<TcpDestination> {
    let IpAddress::Ipv4(ip) = session.local.addr;
    Some(TcpDestination {
        ip: Ipv4Addr::from(ip.octets()),
        port: session.local.port,
        domain: None,
    })
}

fn write_buffered_best_effort(
    connection: &mut impl Write,
    pending: &mut Vec<u8>,
    bytes: &[u8],
) -> Result<usize, String> {
    pending.extend_from_slice(bytes);
    let mut written = 0;
    while !pending.is_empty() {
        match connection.write(pending) {
            Ok(0) => break,
            Ok(count) => {
                written += count;
                pending.drain(..count);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(written)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpstreamRead {
    Data(Vec<u8>),
    WouldBlock,
    Closed,
    Failed,
}

fn read_available(
    connection: &mut impl Read,
    events: &mut Vec<TcpProxyEvent>,
    handle: SocketHandle,
    max_bytes: usize,
) -> UpstreamRead {
    let max_bytes = max_bytes.max(1);
    let mut collected = Vec::new();
    while collected.len() < max_bytes {
        let remaining = max_bytes - collected.len();
        let mut buffer = vec![0; remaining.min(64 * 1024)];
        match connection.read(&mut buffer) {
            Ok(0) => {
                return if collected.is_empty() {
                    UpstreamRead::Closed
                } else {
                    UpstreamRead::Data(collected)
                };
            }
            Ok(count) => {
                buffer.truncate(count);
                collected.extend_from_slice(&buffer);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return if collected.is_empty() {
                    UpstreamRead::WouldBlock
                } else {
                    UpstreamRead::Data(collected)
                };
            }
            Err(error) => {
                events.push(TcpProxyEvent::UpstreamReadFailed {
                    handle,
                    error: error.to_string(),
                });
                return if collected.is_empty() {
                    UpstreamRead::Failed
                } else {
                    UpstreamRead::Data(collected)
                };
            }
        }
    }
    UpstreamRead::Data(collected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_policy::VmnetPolicy;
    use crate::test_support::TestCa;
    use crate::tls_mitm::rustls_client_config_with_roots;
    use crate::vmnet_gateway::{GuestFrameOutcome, VmnetGateway};
    use crate::GuestNetwork;
    use proptest::prelude::*;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };
    use std::sync::Arc;

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(crate::guest_tcp::DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
    const PUBLIC_IP: Ipv4Address = Ipv4Address::new(93, 184, 216, 34);

    #[test]
    fn bridges_http_request_to_upstream_and_response_to_guest() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response: b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".to_vec(),
            write_would_block_count: 0,
        });

        let syn_result = gateway.handle_guest_frame(
            tcp_frame(80, TcpControl::Syn, TcpSeqNumber(100), None, &[]),
            Instant::from_millis(1),
        );
        assert!(matches!(
            syn_result.outcome,
            GuestFrameOutcome::TcpAccepted { .. }
        ));
        assert_eq!(
            &syn_result.guest_frames[0][0..6],
            EthernetAddress::BROADCAST.as_bytes()
        );

        let syn_ack_result = gateway.handle_guest_frame(arp_reply_frame(), Instant::from_millis(2));
        let syn_ack = parse_tcp_reply(&syn_ack_result.guest_frames[0]);
        let server_ack = syn_ack.seq_number + 1;

        gateway.handle_guest_frame(
            tcp_frame(
                80,
                TcpControl::None,
                TcpSeqNumber(101),
                Some(server_ack),
                &[],
            ),
            Instant::from_millis(3),
        );

        let request = b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n";
        gateway.handle_guest_frame(
            tcp_frame(
                80,
                TcpControl::Psh,
                TcpSeqNumber(101),
                Some(server_ack),
                request,
            ),
            Instant::from_millis(4),
        );

        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::Connected { destination, .. }
                if destination.ip == std::net::Ipv4Addr::new(93, 184, 216, 34)
                    && destination.port == 80
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::HttpRequest { summary, .. }
                if summary.method == "GET"
                    && summary.path == "/health"
                    && summary.host.as_deref() == Some("example.com")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::GuestPayload { bytes, .. } if *bytes == request.len()
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::UpstreamPayload {
                bytes,
                guest_frames,
                ..
            } if *bytes > 0 && !guest_frames.is_empty()
        )));
    }

    #[test]
    fn upstream_readiness_drains_response_until_would_block() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Length: 70000\r\n\r\n".to_vec();
        response.extend(vec![b'R'; 70_000]);
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response,
            write_would_block_count: 0,
        });

        establish_http_session(&mut gateway);
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::UpstreamPayload { bytes, .. } if *bytes > 0
        )));
        let session = bridge.sessions.values().next().expect("session");
        assert!(
            session.connection.response.is_empty(),
            "one readiness event should drain all immediately available upstream bytes"
        );
    }

    #[test]
    fn upstream_read_is_capped_per_owner_pass() {
        let mut connection = MemoryConnection {
            response: vec![b'R'; 10],
            written: Vec::new(),
            write_would_block_count: 0,
        };
        let mut events = Vec::new();

        let read = read_available(&mut connection, &mut events, SocketHandle::default(), 4);

        assert_eq!(read, UpstreamRead::Data(vec![b'R'; 4]));
        assert_eq!(connection.response.len(), 6);
        assert!(events.is_empty());
    }

    #[test]
    fn retains_guest_payload_when_tcp_send_buffer_is_full() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Length: 131072\r\n\r\n".to_vec();
        response.extend(vec![b'A'; 128 * 1024]);
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response,
            write_would_block_count: 0,
        });

        establish_http_session(&mut gateway);

        bridge.process_gateway(&mut gateway, Instant::from_millis(5));
        bridge.process_gateway(&mut gateway, Instant::from_millis(6));

        let pending = bridge
            .sessions
            .values()
            .next()
            .expect("session")
            .pending_guest_bytes
            .len();
        assert!(
            pending > 0,
            "large upstream responses must be retained when smoltcp accepts only a partial guest send"
        );
    }

    #[test]
    fn readiness_buffers_guest_payload_until_upstream_socket_is_writable() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            write_would_block_count: 0,
        });

        establish_http_session(&mut gateway);

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            TcpProxyReadiness::selected(Vec::new(), Vec::new()),
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::Connected { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::GuestPayload { .. })));

        let handle = bridge
            .sessions
            .keys()
            .copied()
            .next()
            .expect("proxy session handle");
        let session = bridge.sessions.get(&handle).expect("proxy session");
        assert!(!session.pending_upstream_bytes.is_empty());
        assert!(session.connection.written.is_empty());
        assert_eq!(
            bridge.session_interest(handle),
            Some(UpstreamSessionInterest {
                readable: true,
                writable: true
            })
        );

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(6),
            TcpProxyReadiness::selected(Vec::new(), vec![handle]),
        );

        assert!(events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::GuestPayload { bytes, .. } if *bytes > 0)));
        let session = bridge.sessions.get(&handle).expect("proxy session");
        assert!(session.pending_upstream_bytes.is_empty());
        assert!(!session.connection.written.is_empty());
    }

    #[test]
    fn fragmented_http_headers_are_buffered_until_complete() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            write_would_block_count: 0,
        });

        let server_ack = establish_tcp_session(&mut gateway, 80);
        let first = b"GET /fragmented HTTP/1.1\r\nHost: ex";
        gateway.handle_guest_frame(
            tcp_frame(
                80,
                TcpControl::Psh,
                TcpSeqNumber(101),
                Some(server_ack),
                first,
            ),
            Instant::from_millis(4),
        );
        let first_events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));
        assert!(first_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::HttpRequestIncomplete { .. })));
        assert!(!first_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::HttpRequest { .. })));

        let second = b"ample.com\r\n\r\n";
        gateway.handle_guest_frame(
            tcp_frame(
                80,
                TcpControl::Psh,
                TcpSeqNumber(101 + first.len() as i32),
                Some(server_ack),
                second,
            ),
            Instant::from_millis(6),
        );
        let second_events = bridge.process_gateway(&mut gateway, Instant::from_millis(7));
        assert!(second_events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::HttpRequest { summary, .. }
                if summary.path == "/fragmented"
                    && summary.host.as_deref() == Some("example.com")
        )));
    }

    #[test]
    fn guest_close_removes_proxy_session_and_interest() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            write_would_block_count: 0,
        });

        establish_http_session(&mut gateway);
        bridge.process_gateway(&mut gateway, Instant::from_millis(5));
        let handle = bridge.session_handles().pop().expect("proxy session");
        assert!(bridge.session_interest(handle).is_some());

        gateway.close_tcp_session(handle, Instant::from_millis(6));
        bridge.process_gateway(&mut gateway, Instant::from_millis(7));

        assert!(bridge.session_handles().is_empty());
        assert_eq!(bridge.session_interest(handle), None);
    }

    #[test]
    fn connect_can_be_planned_without_calling_connector() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let bridge = TcpProxyBridge::new(PanicConnector);
        let destination = TcpDestination {
            ip: Ipv4Addr::from(PUBLIC_IP.octets()),
            port: 12345,
            domain: None,
        };

        let plan = bridge.plan_connect(SocketHandle::default(), destination, &policy);

        let TcpProxyConnectPlan::Pending(pending) = plan else {
            panic!("allowed destination should produce pending connect");
        };
        assert_eq!(pending.destination.port, 12345);
        assert_eq!(pending.decision.action, TcpAction::Connect);

        let command = pending.service_command(VmnetServiceToken::new(100));
        let VmnetServiceCommand::TcpConnect(command) = command else {
            panic!("expected TCP connect service command");
        };
        assert_eq!(command.token, VmnetServiceToken::new(100));
        assert_eq!(command.destination.port, 12345);
    }

    #[test]
    fn upstream_connect_failure_is_reported_without_creating_session() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FailingConnector);

        establish_http_session(&mut gateway);
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        let failed = events
            .iter()
            .find(|event| {
                matches!(
                    event,
                    TcpProxyEvent::ConnectFailed {
                        error: TcpConnectError::UpstreamUnavailable,
                        ..
                    }
                )
            })
            .expect("connect failure event");
        let TcpProxyEvent::ConnectFailed { guest_frames, .. } = failed else {
            unreachable!();
        };
        assert!(
            !guest_frames.is_empty(),
            "failed upstream connect must be guest-visible"
        );
        assert!(bridge.sessions.is_empty());
    }

    #[test]
    fn upstream_eof_closes_guest_session_and_removes_proxy_session() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(EofConnector);

        establish_http_session(&mut gateway);
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(!events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamReadFailed { .. })));
        assert!(bridge.session_handles().is_empty());
    }

    #[test]
    fn upstream_read_error_closes_guest_session_and_removes_proxy_session() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(ReadErrorConnector);

        establish_http_session(&mut gateway);
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::UpstreamReadFailed { .. })));
        assert!(bridge.session_handles().is_empty());
    }

    #[test]
    fn upstream_write_backpressure_retains_and_flushes_guest_payload() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FakeConnector {
            response: Vec::new(),
            write_would_block_count: 2,
        });

        establish_http_session(&mut gateway);
        let first_events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));
        let session = bridge.sessions.values().next().expect("session");
        assert!(
            !session.pending_upstream_bytes.is_empty(),
            "guest payload should stay queued when upstream write would block"
        );
        assert!(first_events
            .iter()
            .any(|event| { matches!(event, TcpProxyEvent::GuestPayload { bytes: 0, .. }) }));

        bridge.process_gateway(&mut gateway, Instant::from_millis(6));
        let session = bridge.sessions.values().next().expect("session");
        assert!(session.pending_upstream_bytes.is_empty());
        assert!(!session.connection.written.is_empty());
    }

    #[test]
    fn https_reused_connection_flushes_pending_plaintext_when_upstream_becomes_writable() {
        let ca = TestCa::new("tcp-proxy-https-reused-connection");
        let authority = Arc::new(ca.authority());
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let upstream_server_config = test_tls_server_config(&authority, "example.com");
        let mut bridge = TcpProxyBridge::with_tls_mitm_and_client_config(
            TlsServerConnector {
                server_config: upstream_server_config,
            },
            authority.clone(),
            Arc::new(rustls_client_config_with_roots(roots.clone()).expect("proxy client config")),
        )
        .expect("bridge");
        let (mut gateway, server_ack) = configured_https_gateway(&ca);
        let mut guest_client = ClientConnection::new(
            guest_client_config(roots),
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("guest client");
        let mut guest_seq = 101_i32;
        let mut clock = 4_i64;

        send_guest_tls_to_proxy(
            &mut guest_client,
            &mut gateway,
            &mut bridge,
            server_ack,
            &mut guest_seq,
            &mut clock,
        );
        assert!(!guest_client.is_handshaking());

        guest_client
            .writer()
            .write_all(b"GET /first HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .expect("first request");
        let first_events = send_guest_tls_to_proxy(
            &mut guest_client,
            &mut gateway,
            &mut bridge,
            server_ack,
            &mut guest_seq,
            &mut clock,
        );
        assert!(first_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::GuestPayload { bytes, .. } if *bytes > 0)));
        let handle = bridge.session_handles().pop().expect("proxy session");
        let session = bridge.sessions.get(&handle).expect("session");
        assert_eq!(session.connection.request_count, 1);
        assert!(session.connection.requests[0].starts_with("GET /first "));

        guest_client
            .writer()
            .write_all(b"GET /second HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .expect("second request");
        let second_tls = drain_guest_client_tls(&mut guest_client);
        assert!(!second_tls.is_empty());
        gateway.handle_guest_frame(
            tcp_frame(
                443,
                TcpControl::Psh,
                TcpSeqNumber(guest_seq),
                Some(server_ack),
                &second_tls,
            ),
            Instant::from_millis(clock),
        );
        clock += 1;

        let not_writable_events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(clock),
            TcpProxyReadiness::selected(Vec::new(), Vec::new()),
        );
        clock += 1;
        assert!(not_writable_events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::HttpRequest { summary, .. }
                if summary.host.as_deref() == Some("example.com")
        )));
        let session = bridge.sessions.get(&handle).expect("session");
        assert_eq!(
            session.connection.request_count, 1,
            "second request must not be written while upstream is not writable"
        );
        assert!(
            !session.pending_upstream_plaintext.is_empty(),
            "second HTTPS plaintext should be queued for upstream writable readiness"
        );
        assert_eq!(
            bridge.session_interest(handle),
            Some(UpstreamSessionInterest {
                readable: true,
                writable: true,
            })
        );

        let writable_events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(clock),
            TcpProxyReadiness::selected(Vec::new(), vec![handle]),
        );
        assert!(writable_events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::GuestPayload { bytes, .. } if *bytes > 0)));
        let session = bridge.sessions.get(&handle).expect("session");
        assert!(session.pending_upstream_plaintext.is_empty());
        assert_eq!(session.connection.request_count, 2);
        assert!(session.connection.requests[1].starts_with("GET /second "));
    }

    proptest! {
        #[test]
        fn proptest_pending_upstream_buffer_limits_stay_bounded(
            limit in 0_usize..=32,
            existing in 0_usize..=32,
            incoming in 0_usize..=32,
            plaintext in any::<bool>(),
        ) {
            let network = GuestNetwork::default();
            let mut policy = VmnetPolicy::default_sandbox(network.clone());
            policy.egress.allow_ips.push(PUBLIC_IP.to_string());
            let mut gateway = VmnetGateway::new(&policy, &network, Instant::from_millis(0))
                .expect("gateway");
            establish_tcp_session(&mut gateway, 80);
            let handle = gateway
                .active_tcp_sessions()
                .into_iter()
                .find(|active| active.session.state == tcp::State::Established)
                .expect("active session")
                .handle;
            let mut session = test_upstream_session(TcpProxyBufferLimits::with_upstream_bytes(
                limit,
                limit,
            ));
            if plaintext {
                session.pending_upstream_plaintext = vec![b'x'; existing];
            } else {
                session.pending_upstream_bytes = vec![b'x'; existing];
            }
            let mut events = Vec::new();

            let accepted = if plaintext {
                buffer_pending_upstream_plaintext(
                    &mut session,
                    handle,
                    &mut gateway,
                    Instant::from_millis(4),
                    &vec![b'y'; incoming],
                    &mut events,
                )
            } else {
                buffer_pending_upstream_bytes(
                    &mut session,
                    handle,
                    &mut gateway,
                    Instant::from_millis(4),
                    &vec![b'y'; incoming],
                    &mut events,
                )
            };

            let attempted = existing + incoming;
            if attempted > limit {
                prop_assert!(!accepted);
                let has_limit_event = events.iter().any(|event| {
                    matches!(event, TcpProxyEvent::BufferLimitExceeded { attempted: event_attempted, .. } if *event_attempted == attempted)
                });
                prop_assert!(has_limit_event);
            } else {
                prop_assert!(accepted);
                prop_assert!(events.is_empty());
                if plaintext {
                    prop_assert_eq!(session.pending_upstream_plaintext.len(), attempted);
                } else {
                    prop_assert_eq!(session.pending_upstream_bytes.len(), attempted);
                }
            }
        }
    }

    #[test]
    fn pending_guest_bytes_limit_does_not_reject_direct_guest_send() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let handle = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active session")
            .handle;
        let mut session = test_upstream_session(TcpProxyBufferLimits::with_all(
            DEFAULT_TCP_PROXY_BUFFER_LIMIT,
            DEFAULT_TCP_PROXY_BUFFER_LIMIT,
            8,
        ));
        let mut events = Vec::new();

        send_guest_buffered(
            &mut session,
            handle,
            &mut gateway,
            Instant::from_millis(5),
            b"larger-than-limit-but-sendable",
            GuestSendEvent::UpstreamPayload,
            &mut events,
        );

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::UpstreamPayload { bytes, .. } if *bytes > 0
        )));
        assert!(!events
            .iter()
            .any(|event| matches!(event, TcpProxyEvent::BufferLimitExceeded { .. })));
        assert!(session.pending_guest_bytes.is_empty());
    }

    #[test]
    fn pending_guest_bytes_limit_fails_closed_before_buffering() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        establish_tcp_session(&mut gateway, 80);
        let handle = gateway
            .active_tcp_sessions()
            .into_iter()
            .find(|active| active.session.state == tcp::State::Established)
            .expect("active session")
            .handle;
        let fill = vec![b'z'; 4096];
        let mut filled = false;
        for _ in 0..1024 {
            let (written, _) = gateway
                .send_tcp_session_partial(handle, &fill, Instant::from_millis(4))
                .expect("fill guest send buffer");
            if written == 0 {
                filled = true;
                break;
            }
        }
        assert!(filled, "test requires a full guest-side TCP send buffer");
        let mut session = UpstreamSession {
            destination: TcpDestination {
                ip: Ipv4Addr::from(PUBLIC_IP.octets()),
                port: 80,
                domain: None,
            },
            decision: TcpDecision {
                action: TcpAction::Connect,
                reason: "test".to_string(),
            },
            connection: MemoryConnection {
                response: Vec::new(),
                written: Vec::new(),
                write_would_block_count: 0,
            },
            http_buffer: Vec::new(),
            guest_tls: None,
            upstream_tls: None,
            upstream_tls_ready: false,
            pending_upstream_bytes: Vec::new(),
            pending_upstream_plaintext: Vec::new(),
            pending_guest_bytes: vec![b'x'; 4],
            buffer_limits: TcpProxyBufferLimits::with_all(
                DEFAULT_TCP_PROXY_BUFFER_LIMIT,
                DEFAULT_TCP_PROXY_BUFFER_LIMIT,
                8,
            ),
        };
        let mut events = Vec::new();

        send_guest_buffered(
            &mut session,
            handle,
            &mut gateway,
            Instant::from_millis(5),
            b"too-large",
            GuestSendEvent::UpstreamPayload,
            &mut events,
        );

        let limit_event = events
            .iter()
            .find(|event| matches!(event, TcpProxyEvent::BufferLimitExceeded { .. }))
            .expect("buffer limit event");
        let TcpProxyEvent::BufferLimitExceeded {
            buffer,
            limit,
            attempted,
            guest_frames,
            ..
        } = limit_event
        else {
            unreachable!();
        };
        assert_eq!(*buffer, TcpProxyBufferKind::PendingGuestBytes);
        assert_eq!(*limit, 8);
        assert!(*attempted > *limit);
        assert!(!guest_frames.is_empty());
        assert_eq!(session.pending_guest_bytes, vec![b'x'; 4]);
    }

    #[test]
    fn https_pending_upstream_tls_limit_fails_closed_when_upstream_not_writable() {
        let ca = TestCa::new("tcp-proxy-https-upstream-tls-limit");
        let authority = Arc::new(ca.authority());
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let upstream_server_config = test_tls_server_config(&authority, "example.com");
        let mut bridge = TcpProxyBridge::with_tls_mitm_and_client_config(
            TlsServerConnector {
                server_config: upstream_server_config,
            },
            authority.clone(),
            Arc::new(rustls_client_config_with_roots(roots.clone()).expect("proxy client config")),
        )
        .expect("bridge")
        .with_buffer_limits(TcpProxyBufferLimits::with_upstream_bytes(
            8,
            DEFAULT_TCP_PROXY_BUFFER_LIMIT,
        ));
        let (mut gateway, server_ack) = configured_https_gateway(&ca);
        let mut guest_client = ClientConnection::new(
            guest_client_config(roots),
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("guest client");
        let guest_tls = drain_guest_client_tls(&mut guest_client);
        assert!(!guest_tls.is_empty());
        gateway.handle_guest_frame(
            tcp_frame(
                443,
                TcpControl::Psh,
                TcpSeqNumber(101),
                Some(server_ack),
                &guest_tls,
            ),
            Instant::from_millis(4),
        );

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            TcpProxyReadiness::selected(Vec::new(), Vec::new()),
        );

        let limit_event = events
            .iter()
            .find(|event| matches!(event, TcpProxyEvent::BufferLimitExceeded { .. }))
            .expect("buffer limit event");
        let TcpProxyEvent::BufferLimitExceeded {
            buffer,
            limit,
            attempted,
            guest_frames,
            ..
        } = limit_event
        else {
            unreachable!();
        };
        assert_eq!(*buffer, TcpProxyBufferKind::PendingUpstreamBytes);
        assert_eq!(*limit, 8);
        assert!(*attempted > *limit);
        assert!(!guest_frames.is_empty());
    }

    #[test]
    fn https_pending_plaintext_limit_fails_closed_when_upstream_not_writable() {
        let ca = TestCa::new("tcp-proxy-https-plaintext-limit");
        let authority = Arc::new(ca.authority());
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let upstream_server_config = test_tls_server_config(&authority, "example.com");
        let mut bridge = TcpProxyBridge::with_tls_mitm_and_client_config(
            TlsServerConnector {
                server_config: upstream_server_config,
            },
            authority.clone(),
            Arc::new(rustls_client_config_with_roots(roots.clone()).expect("proxy client config")),
        )
        .expect("bridge")
        .with_buffer_limits(TcpProxyBufferLimits::new(8));
        let (mut gateway, server_ack) = configured_https_gateway(&ca);
        let mut guest_client = ClientConnection::new(
            guest_client_config(roots),
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("guest client");
        let mut guest_seq = 101_i32;
        let mut clock = 4_i64;

        send_guest_tls_to_proxy(
            &mut guest_client,
            &mut gateway,
            &mut bridge,
            server_ack,
            &mut guest_seq,
            &mut clock,
        );
        assert!(!guest_client.is_handshaking());
        let handle = bridge.session_handles().pop().expect("proxy session");

        guest_client
            .writer()
            .write_all(b"GET /too-large HTTP/1.1\r\nHost: example.com\r\n\r\n")
            .expect("request");
        let request_tls = drain_guest_client_tls(&mut guest_client);
        gateway.handle_guest_frame(
            tcp_frame(
                443,
                TcpControl::Psh,
                TcpSeqNumber(guest_seq),
                Some(server_ack),
                &request_tls,
            ),
            Instant::from_millis(clock),
        );

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(clock + 1),
            TcpProxyReadiness::selected(Vec::new(), Vec::new()),
        );

        let limit_event = events
            .iter()
            .find(|event| matches!(event, TcpProxyEvent::BufferLimitExceeded { .. }))
            .expect("buffer limit event");
        let TcpProxyEvent::BufferLimitExceeded {
            buffer,
            limit,
            attempted,
            guest_frames,
            ..
        } = limit_event
        else {
            unreachable!();
        };
        assert_eq!(*buffer, TcpProxyBufferKind::PendingUpstreamPlaintext);
        assert_eq!(*limit, 8);
        assert!(*attempted > *limit);
        assert!(!guest_frames.is_empty());

        bridge.process_gateway(&mut gateway, Instant::from_millis(clock + 2));
        assert_eq!(bridge.session_interest(handle), None);
    }

    #[test]
    fn https_guest_without_sni_fails_before_upstream_connect() {
        let ca = TestCa::new("tcp-proxy-https-nosni");
        let authority = Arc::new(ca.authority());
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let mut bridge = TcpProxyBridge::with_tls_mitm_and_client_config(
            FakeConnector {
                response: Vec::new(),
                write_would_block_count: 0,
            },
            authority.clone(),
            Arc::new(rustls_client_config_with_roots(roots.clone()).expect("proxy client config")),
        )
        .expect("bridge");
        let (mut gateway, server_ack) = configured_https_gateway(&ca);
        let mut guest_client = ClientConnection::new(
            guest_client_config(roots),
            ServerName::try_from("93.184.216.34")
                .expect("IP server name")
                .to_owned(),
        )
        .expect("guest client");
        let mut guest_seq = 101_i32;
        let mut clock = 4_i64;

        let events = send_guest_tls_to_proxy(
            &mut guest_client,
            &mut gateway,
            &mut bridge,
            server_ack,
            &mut guest_seq,
            &mut clock,
        );

        let failed = events
            .iter()
            .find(|event| {
                matches!(
                    event,
                    TcpProxyEvent::TlsMitmFailed {
                        error: TlsMitmError::Tls(_),
                        ..
                    }
                )
            })
            .expect("TLS MITM failure");
        let TcpProxyEvent::TlsMitmFailed { guest_frames, .. } = failed else {
            unreachable!();
        };
        assert!(
            !guest_frames.is_empty(),
            "fatal guest TLS failure must close/reset the guest session"
        );
        let session = bridge.sessions.values().next().expect("session");
        assert!(
            session.upstream_tls.is_none(),
            "no-SNI guest TLS must not initialize upstream TLS"
        );
        assert!(
            session.connection.written.is_empty(),
            "no-SNI guest TLS must not send bytes upstream"
        );
    }

    #[test]
    fn https_invalid_upstream_tls_is_reported_as_mitm_failure() {
        let ca = TestCa::new("tcp-proxy-https-invalid-upstream");
        let authority = Arc::new(ca.authority());
        let mut roots = RootCertStore::empty();
        roots.add(authority.ca_cert()).expect("root");
        let mut bridge = TcpProxyBridge::with_tls_mitm_and_client_config(
            FakeConnector {
                response: b"not a tls record".to_vec(),
                write_would_block_count: 0,
            },
            authority.clone(),
            Arc::new(rustls_client_config_with_roots(roots.clone()).expect("proxy client config")),
        )
        .expect("bridge");
        let (mut gateway, server_ack) = configured_https_gateway(&ca);
        let mut guest_client = ClientConnection::new(
            guest_client_config(roots),
            ServerName::try_from("example.com")
                .expect("server name")
                .to_owned(),
        )
        .expect("guest client");
        let mut guest_seq = 101_i32;
        let mut clock = 4_i64;

        let events = send_guest_tls_to_proxy(
            &mut guest_client,
            &mut gateway,
            &mut bridge,
            server_ack,
            &mut guest_seq,
            &mut clock,
        );

        let failed = events
            .iter()
            .find(|event| {
                matches!(
                    event,
                    TcpProxyEvent::TlsMitmFailed {
                        error: TlsMitmError::Tls(_) | TlsMitmError::Io(_),
                        ..
                    }
                )
            })
            .expect("TLS MITM failure");
        let TcpProxyEvent::TlsMitmFailed { guest_frames, .. } = failed else {
            unreachable!();
        };
        assert!(
            !guest_frames.is_empty(),
            "fatal upstream TLS failure must close/reset the guest session"
        );
    }

    #[derive(Debug, Clone)]
    struct FakeConnector {
        response: Vec<u8>,
        write_would_block_count: usize,
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
                write_would_block_count: self.write_would_block_count,
            })
        }
    }

    #[derive(Debug, Clone)]
    struct TlsServerConnector {
        server_config: Arc<ServerConfig>,
    }

    impl TcpUpstreamConnector for TlsServerConnector {
        type Connection = TlsServerConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Ok(TlsServerConnection {
                server: ServerConnection::new(self.server_config.clone()).expect("tls server"),
                response_tls: Vec::new(),
                plaintext_buffer: Vec::new(),
                requests: Vec::new(),
                request_count: 0,
            })
        }
    }

    #[derive(Debug)]
    struct TlsServerConnection {
        server: ServerConnection,
        response_tls: Vec<u8>,
        plaintext_buffer: Vec<u8>,
        requests: Vec<String>,
        request_count: usize,
    }

    impl Read for TlsServerConnection {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.response_tls.is_empty() {
                return Err(io::Error::from(ErrorKind::WouldBlock));
            }
            let count = self.response_tls.len().min(buf.len());
            buf[..count].copy_from_slice(&self.response_tls[..count]);
            self.response_tls.drain(..count);
            Ok(count)
        }
    }

    impl Write for TlsServerConnection {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut reader = buf;
            self.server.read_tls(&mut reader)?;
            self.server
                .process_new_packets()
                .map_err(|error| io::Error::new(ErrorKind::InvalidData, error.to_string()))?;
            self.response_tls
                .extend(drain_tls_server(&mut self.server)?);
            read_tls_server_plaintext(&mut self.server, &mut self.plaintext_buffer)?;
            while let Some(end) = http_header_end(&self.plaintext_buffer) {
                let request = self.plaintext_buffer[..end].to_vec();
                self.plaintext_buffer.drain(..end);
                self.request_count += 1;
                self.requests
                    .push(String::from_utf8_lossy(&request).to_string());
                self.server
                    .writer()
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")?;
                self.response_tls
                    .extend(drain_tls_server(&mut self.server)?);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug, Clone)]
    struct PanicConnector;

    impl TcpUpstreamConnector for PanicConnector {
        type Connection = MemoryConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            panic!("connect planning must not call connector")
        }
    }

    #[derive(Debug, Clone)]
    struct FailingConnector;

    impl TcpUpstreamConnector for FailingConnector {
        type Connection = MemoryConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Err(TcpConnectError::UpstreamUnavailable)
        }
    }

    #[derive(Debug, Clone)]
    struct EofConnector;

    impl TcpUpstreamConnector for EofConnector {
        type Connection = EofConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Ok(EofConnection)
        }
    }

    #[derive(Debug, Clone)]
    struct ReadErrorConnector;

    impl TcpUpstreamConnector for ReadErrorConnector {
        type Connection = ReadErrorConnection;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            Ok(ReadErrorConnection)
        }
    }

    #[derive(Debug)]
    struct MemoryConnection {
        response: Vec<u8>,
        written: Vec<u8>,
        write_would_block_count: usize,
    }

    fn test_upstream_session(limits: TcpProxyBufferLimits) -> UpstreamSession<MemoryConnection> {
        UpstreamSession {
            destination: TcpDestination {
                ip: Ipv4Addr::from(PUBLIC_IP.octets()),
                port: 80,
                domain: None,
            },
            decision: TcpDecision {
                action: TcpAction::Connect,
                reason: "test".to_string(),
            },
            connection: MemoryConnection {
                response: Vec::new(),
                written: Vec::new(),
                write_would_block_count: 0,
            },
            http_buffer: Vec::new(),
            guest_tls: None,
            upstream_tls: None,
            upstream_tls_ready: false,
            pending_upstream_bytes: Vec::new(),
            pending_upstream_plaintext: Vec::new(),
            pending_guest_bytes: Vec::new(),
            buffer_limits: limits,
        }
    }

    impl Read for MemoryConnection {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
            if self.write_would_block_count > 0 {
                self.write_would_block_count -= 1;
                return Err(io::Error::from(ErrorKind::WouldBlock));
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct EofConnection;

    impl Read for EofConnection {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for EofConnection {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct ReadErrorConnection;

    impl Read for ReadErrorConnection {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(ErrorKind::ConnectionReset, "upstream reset"))
        }
    }

    impl Write for ReadErrorConnection {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_tls_server_config(authority: &TlsMitmAuthority, host: &str) -> Arc<ServerConfig> {
        let generated = authority
            .generate_server_certificate(host)
            .expect("server cert");
        Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_no_client_auth()
                .with_single_cert(generated.cert_chain, generated.private_key)
                .expect("server config"),
        )
    }

    fn read_tls_server_plaintext(
        server: &mut ServerConnection,
        output: &mut Vec<u8>,
    ) -> io::Result<()> {
        let mut buffer = [0; 16 * 1024];
        loop {
            match server.reader().read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    fn drain_tls_server(server: &mut ServerConnection) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        while server.wants_write() {
            let before = output.len();
            server.write_tls(&mut output)?;
            if output.len() == before {
                break;
            }
        }
        Ok(output)
    }

    fn http_header_end(buffer: &[u8]) -> Option<usize> {
        buffer
            .windows(b"\r\n\r\n".len())
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + b"\r\n\r\n".len())
    }

    fn send_guest_tls_to_proxy<C>(
        client: &mut ClientConnection,
        gateway: &mut VmnetGateway<'_>,
        bridge: &mut TcpProxyBridge<C>,
        ack: TcpSeqNumber,
        seq: &mut i32,
        clock: &mut i64,
    ) -> Vec<TcpProxyEvent>
    where
        C: TcpUpstreamConnector,
        C::Connection: Read + Write,
    {
        let mut collected = Vec::new();
        let tls = drain_guest_client_tls(client);
        if !tls.is_empty() {
            gateway.handle_guest_frame(
                tcp_frame(443, TcpControl::Psh, TcpSeqNumber(*seq), Some(ack), &tls),
                Instant::from_millis(*clock),
            );
            *seq += tls.len() as i32;
            *clock += 1;
        }
        for _ in 0..8 {
            let events = bridge.process_gateway(gateway, Instant::from_millis(*clock));
            *clock += 1;
            feed_guest_from_proxy_events(client, &events);
            let stop = !client.wants_write();
            collected.extend(events);
            if stop {
                break;
            }
        }
        collected
    }

    fn configured_https_gateway(ca: &TestCa) -> (VmnetGateway<'static>, TcpSeqNumber) {
        let network = Box::leak(Box::new(GuestNetwork::default()));
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        policy.tls_mitm.ca_cert_path = Some(ca.cert_path.clone());
        policy.tls_mitm.ca_key_path = Some(ca.key_path.clone());
        policy.tls_mitm.generate_per_host_certs = true;
        let policy = Box::leak(Box::new(policy));
        let mut gateway =
            VmnetGateway::new(policy, network, Instant::from_millis(0)).expect("gateway");
        let server_ack = establish_tcp_session(&mut gateway, 443);
        (gateway, server_ack)
    }

    fn guest_client_config(roots: RootCertStore) -> Arc<ClientConfig> {
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(rustls::DEFAULT_VERSIONS)
                .expect("versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }

    fn drain_guest_client_tls(client: &mut ClientConnection) -> Vec<u8> {
        let mut bytes = Vec::new();
        while client.wants_write() {
            let before = bytes.len();
            client.write_tls(&mut bytes).expect("guest write tls");
            if bytes.len() == before {
                break;
            }
        }
        bytes
    }

    fn feed_guest_from_proxy_events(client: &mut ClientConnection, events: &[TcpProxyEvent]) {
        for payload in tls_payloads_to_guest(events) {
            client
                .read_tls(&mut payload.as_slice())
                .expect("guest read tls");
            client.process_new_packets().expect("guest process tls");
        }
    }

    fn tls_payloads_to_guest(events: &[TcpProxyEvent]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::new();
        for event in events {
            match event {
                TcpProxyEvent::TlsHandshakePayload { guest_frames, .. }
                | TcpProxyEvent::UpstreamPayload { guest_frames, .. } => {
                    for frame in guest_frames {
                        if let Some(payload) = tcp_payload(frame) {
                            if !payload.is_empty() {
                                payloads.push(payload.to_vec());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        payloads
    }

    fn tcp_payload(frame: &[u8]) -> Option<&[u8]> {
        let ethernet = EthernetFrame::new_checked(frame).ok()?;
        let ethernet = EthernetRepr::parse(&ethernet).ok()?;
        if ethernet.ethertype != EthernetProtocol::Ipv4 {
            return None;
        }
        let ip_offset = ethernet.buffer_len();
        let ipv4 = Ipv4Packet::new_checked(&frame[ip_offset..]).ok()?;
        let ipv4 = Ipv4Repr::parse(&ipv4, &ChecksumCapabilities::default()).ok()?;
        if ipv4.next_header != IpProtocol::Tcp {
            return None;
        }
        let tcp_offset = ip_offset + ipv4.buffer_len();
        let tcp = TcpPacket::new_checked(&frame[tcp_offset..]).ok()?;
        let tcp_repr = TcpRepr::parse(
            &tcp,
            &IpAddress::Ipv4(ipv4.src_addr),
            &IpAddress::Ipv4(ipv4.dst_addr),
            &ChecksumCapabilities::default(),
        )
        .ok()?;
        Some(tcp_repr.payload)
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
        let syn_ack = parse_tcp_reply(&syn_ack_result.guest_frames[0]);
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

    fn establish_http_session(gateway: &mut VmnetGateway<'_>) {
        let server_ack = establish_tcp_session(gateway, 80);

        gateway.handle_guest_frame(
            tcp_frame(
                80,
                TcpControl::Psh,
                TcpSeqNumber(101),
                Some(server_ack),
                b"GET /large HTTP/1.1\r\nHost: example.com\r\n\r\n",
            ),
            Instant::from_millis(4),
        );
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
