use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::{AsRawFd, RawFd};

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;

use crate::guest_tcp::GuestTcpConnectError;
use crate::network_policy::{HostListener, HostListenerPurpose};
use crate::vmnet_gateway::VmnetGateway;

pub const DEFAULT_HOST_INGRESS_FIRST_LOCAL_PORT: u16 = 40_000;

pub struct HostIngressBridge<C> {
    sessions: HashMap<SocketHandle, HostIngressSession<C>>,
    next_local_port: u16,
}

pub struct HostIngressListenerSet {
    listeners: Vec<HostIngressListener>,
}

struct HostIngressListener {
    config: HostListener,
    listener: TcpListener,
}

pub struct AcceptedHostConnection {
    pub guest_port: u16,
    pub purpose: HostListenerPurpose,
    pub connection: TcpStream,
}

impl HostIngressListenerSet {
    pub fn bind(configs: &[HostListener]) -> std::io::Result<Self> {
        let mut listeners = Vec::with_capacity(configs.len());
        for config in configs {
            let addr: SocketAddr = format!("{}:{}", config.host_addr, config.host_port)
                .parse()
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
            let listener = TcpListener::bind(addr)?;
            listener.set_nonblocking(true)?;
            listeners.push(HostIngressListener {
                config: config.clone(),
                listener,
            });
        }
        Ok(Self { listeners })
    }

    pub fn accept_pending(&self) -> Vec<std::io::Result<AcceptedHostConnection>> {
        let mut accepted = Vec::new();
        for index in 0..self.listeners.len() {
            accepted.extend(self.accept_ready(index));
        }
        accepted
    }

    pub fn accept_ready(
        &self,
        listener_index: usize,
    ) -> Vec<std::io::Result<AcceptedHostConnection>> {
        let mut accepted = Vec::new();
        let Some(listener) = self.listeners.get(listener_index) else {
            return accepted;
        };
        loop {
            match listener.listener.accept() {
                Ok((stream, _addr)) => {
                    if let Err(error) = stream.set_nonblocking(true) {
                        accepted.push(Err(error));
                        continue;
                    }
                    accepted.push(Ok(AcceptedHostConnection {
                        guest_port: listener.config.guest_port,
                        purpose: listener.config.purpose,
                        connection: stream,
                    }));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    accepted.push(Err(error));
                    break;
                }
            }
        }
        accepted
    }

    pub fn listener_fds(&self) -> Vec<RawFd> {
        self.listeners
            .iter()
            .map(|listener| listener.listener.as_raw_fd())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }

    #[cfg(test)]
    fn local_addrs(&self) -> std::io::Result<Vec<SocketAddr>> {
        self.listeners
            .iter()
            .map(|listener| listener.listener.local_addr())
            .collect()
    }
}

impl<C> HostIngressBridge<C> {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_local_port: DEFAULT_HOST_INGRESS_FIRST_LOCAL_PORT,
        }
    }

    pub fn open_session(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        guest_port: u16,
        connection: C,
        now: Instant,
    ) -> Result<HostIngressOpen, GuestTcpConnectError> {
        let local_port = self.allocate_local_port();
        let connect = gateway.connect_host_to_guest(guest_port, local_port, now)?;
        self.sessions.insert(
            connect.handle,
            HostIngressSession {
                guest_port,
                connection,
                pending_host_write: Vec::new(),
                pending_guest_write: Vec::new(),
                guest_closed: None,
            },
        );
        Ok(HostIngressOpen {
            handle: connect.handle,
            guest_port,
            local_port,
            guest_frames: connect.guest_frames,
        })
    }

    fn allocate_local_port(&mut self) -> u16 {
        let port = self.next_local_port;
        self.next_local_port = self.next_local_port.wrapping_add(1).max(1024);
        port
    }
}

impl<C> Default for HostIngressBridge<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> HostIngressBridge<C>
where
    C: Read + Write,
{
    pub fn process_gateway(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
    ) -> Vec<HostIngressEvent> {
        self.process_gateway_with_readiness(gateway, now, HostIngressReadiness::all())
    }

    pub fn process_gateway_with_readiness(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
        readiness: HostIngressReadiness,
    ) -> Vec<HostIngressEvent> {
        let mut events = Vec::new();
        let mut closed = Vec::new();
        for active in gateway.host_ingress_sessions() {
            let Some(session) = self.sessions.get_mut(&active.handle) else {
                continue;
            };
            let guest_port = session.guest_port;

            if active.session.state == tcp::State::Established {
                if !session.pending_guest_write.is_empty() {
                    match send_pending_guest_write(
                        gateway,
                        active.handle,
                        &mut session.pending_guest_write,
                        now,
                    ) {
                        Ok(guest_frames) => events.push(HostIngressEvent::HostPayload {
                            handle: active.handle,
                            guest_port,
                            bytes: 0,
                            guest_frames,
                        }),
                        Err(error) => events.push(HostIngressEvent::GuestWriteFailed {
                            handle: active.handle,
                            error,
                        }),
                    }
                }
                if readiness.readable(active.handle) && session.pending_guest_write.is_empty() {
                    loop {
                        match read_available(&mut session.connection) {
                            HostRead::Payload(host_bytes) => {
                                let bytes = host_bytes.len();
                                session.pending_guest_write.extend_from_slice(&host_bytes);
                                match send_pending_guest_write(
                                    gateway,
                                    active.handle,
                                    &mut session.pending_guest_write,
                                    now,
                                ) {
                                    Ok(guest_frames) => {
                                        events.push(HostIngressEvent::HostPayload {
                                            handle: active.handle,
                                            guest_port,
                                            bytes,
                                            guest_frames,
                                        })
                                    }
                                    Err(error) => events.push(HostIngressEvent::GuestWriteFailed {
                                        handle: active.handle,
                                        error,
                                    }),
                                }
                                if !session.pending_guest_write.is_empty() {
                                    break;
                                }
                            }
                            HostRead::Closed => {
                                let guest_frames = gateway.close_tcp_session(active.handle, now);
                                events.push(HostIngressEvent::HostClosed {
                                    handle: active.handle,
                                    guest_port,
                                    guest_frames,
                                });
                                closed.push(active.handle);
                                break;
                            }
                            HostRead::WouldBlock => break,
                            HostRead::Failed(error) => {
                                let guest_frames = gateway.close_tcp_session(active.handle, now);
                                events.push(HostIngressEvent::HostReadFailed {
                                    handle: active.handle,
                                    error,
                                });
                                events.push(HostIngressEvent::HostClosed {
                                    handle: active.handle,
                                    guest_port,
                                    guest_frames,
                                });
                                closed.push(active.handle);
                                break;
                            }
                        }
                    }
                }
            }
            if active.session.state != tcp::State::Established
                && !guest_closed_state(active.session.state)
            {
                continue;
            }

            match gateway.recv_tcp_session(active.handle) {
                Ok(guest_bytes) if !guest_bytes.is_empty() => {
                    session.pending_host_write.extend_from_slice(&guest_bytes);
                    match write_all_best_effort(
                        &mut session.connection,
                        &mut session.pending_host_write,
                    ) {
                        Ok(bytes) if bytes > 0 => events.push(HostIngressEvent::GuestPayload {
                            handle: active.handle,
                            guest_port,
                            bytes,
                        }),
                        Ok(_) => {}
                        Err(error) => {
                            let guest_frames = gateway.close_tcp_session(active.handle, now);
                            events.push(HostIngressEvent::HostWriteFailed {
                                handle: active.handle,
                                error,
                            });
                            events.push(HostIngressEvent::HostClosed {
                                handle: active.handle,
                                guest_port,
                                guest_frames,
                            });
                            closed.push(active.handle);
                        }
                    }
                }
                Ok(_) => {
                    if readiness.writable(active.handle) && !session.pending_host_write.is_empty() {
                        match write_all_best_effort(
                            &mut session.connection,
                            &mut session.pending_host_write,
                        ) {
                            Ok(bytes) if bytes > 0 => events.push(HostIngressEvent::GuestPayload {
                                handle: active.handle,
                                guest_port,
                                bytes,
                            }),
                            Ok(_) => {}
                            Err(error) => {
                                let guest_frames = gateway.close_tcp_session(active.handle, now);
                                events.push(HostIngressEvent::HostWriteFailed {
                                    handle: active.handle,
                                    error,
                                });
                                events.push(HostIngressEvent::HostClosed {
                                    handle: active.handle,
                                    guest_port,
                                    guest_frames,
                                });
                                closed.push(active.handle);
                            }
                        }
                    }
                }
                Err(error) => events.push(HostIngressEvent::GuestReadFailed {
                    handle: active.handle,
                    error,
                }),
            }
            if guest_closed_state(active.session.state) {
                session.guest_closed = Some(active.session.state);
            }
            if let Some(state) = session.guest_closed {
                if session.pending_host_write.is_empty() {
                    events.push(HostIngressEvent::GuestClosed {
                        handle: active.handle,
                        guest_port,
                        state,
                    });
                    closed.push(active.handle);
                }
            }
        }
        for handle in closed {
            self.sessions.remove(&handle);
        }
        events
    }

    pub fn session_connection(&self, handle: SocketHandle) -> Option<&C> {
        self.sessions
            .get(&handle)
            .map(|session| &session.connection)
    }

    pub fn session_interest(&self, handle: SocketHandle) -> Option<HostSessionInterest> {
        let session = self.sessions.get(&handle)?;
        Some(HostSessionInterest {
            readable: session.guest_closed.is_none() && session.pending_guest_write.is_empty(),
            writable: !session.pending_host_write.is_empty(),
        })
    }

    pub fn session_handles(&self) -> Vec<SocketHandle> {
        self.sessions.keys().copied().collect()
    }
}

struct HostIngressSession<C> {
    guest_port: u16,
    connection: C,
    pending_host_write: Vec<u8>,
    pending_guest_write: Vec<u8>,
    guest_closed: Option<tcp::State>,
}

impl HostIngressBridge<TcpStream> {
    pub fn session_raw_fd(&self, handle: SocketHandle) -> Option<RawFd> {
        self.sessions
            .get(&handle)
            .map(|session| session.connection.as_raw_fd())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSessionInterest {
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIngressReadiness {
    poll_all: bool,
    readable: Vec<SocketHandle>,
    writable: Vec<SocketHandle>,
}

impl HostIngressReadiness {
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

#[derive(Debug, PartialEq, Eq)]
pub struct HostIngressOpen {
    pub handle: SocketHandle,
    pub guest_port: u16,
    pub local_port: u16,
    pub guest_frames: Vec<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HostIngressEvent {
    Opened {
        handle: SocketHandle,
        guest_port: u16,
        purpose: HostListenerPurpose,
    },
    OpenFailed {
        guest_port: u16,
        purpose: HostListenerPurpose,
        error: String,
    },
    HostPayload {
        handle: SocketHandle,
        guest_port: u16,
        bytes: usize,
        guest_frames: Vec<Vec<u8>>,
    },
    GuestPayload {
        handle: SocketHandle,
        guest_port: u16,
        bytes: usize,
    },
    HostClosed {
        handle: SocketHandle,
        guest_port: u16,
        guest_frames: Vec<Vec<u8>>,
    },
    GuestClosed {
        handle: SocketHandle,
        guest_port: u16,
        state: tcp::State,
    },
    GuestReadFailed {
        handle: SocketHandle,
        error: tcp::RecvError,
    },
    GuestWriteFailed {
        handle: SocketHandle,
        error: tcp::SendError,
    },
    HostReadFailed {
        handle: SocketHandle,
        error: String,
    },
    HostWriteFailed {
        handle: SocketHandle,
        error: String,
    },
}

fn guest_closed_state(state: tcp::State) -> bool {
    matches!(
        state,
        tcp::State::CloseWait
            | tcp::State::Closing
            | tcp::State::LastAck
            | tcp::State::TimeWait
            | tcp::State::Closed
    )
}

enum HostRead {
    Payload(Vec<u8>),
    Closed,
    WouldBlock,
    Failed(String),
}

fn send_pending_guest_write(
    gateway: &mut VmnetGateway<'_>,
    handle: SocketHandle,
    pending: &mut Vec<u8>,
    now: Instant,
) -> Result<Vec<Vec<u8>>, tcp::SendError> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let (written, guest_frames) = gateway.send_tcp_session_partial(handle, pending, now)?;
    pending.drain(..written);
    Ok(guest_frames)
}

fn read_available(connection: &mut impl Read) -> HostRead {
    let mut buffer = vec![0; 64 * 1024];
    match connection.read(&mut buffer) {
        Ok(0) => HostRead::Closed,
        Ok(count) => {
            buffer.truncate(count);
            HostRead::Payload(buffer)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => HostRead::WouldBlock,
        Err(error) => HostRead::Failed(error.to_string()),
    }
}

fn write_all_best_effort(
    connection: &mut impl Write,
    bytes: &mut Vec<u8>,
) -> Result<usize, String> {
    let mut written = 0;
    while !bytes.is_empty() {
        match connection.write(bytes) {
            Ok(0) => break,
            Ok(count) => {
                written += count;
                bytes.drain(..count);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_policy::VmnetPolicy;
    use crate::vmnet_gateway::VmnetGateway;
    use crate::GuestNetwork;
    use smoltcp::phy::ChecksumCapabilities;
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, EthernetRepr, IpAddress, IpProtocol,
        Ipv4Address, Ipv4Packet, Ipv4Repr, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber,
    };
    use std::collections::VecDeque;
    use std::io;

    const GUEST_MAC: EthernetAddress = EthernetAddress([0x02, 0xfc, 0x12, 0x34, 0x56, 0x78]);
    const GATEWAY_MAC: EthernetAddress = EthernetAddress(crate::guest_tcp::DEFAULT_GATEWAY_MAC);
    const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
    const GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

    #[test]
    fn bridges_host_payload_into_guest_session_and_guest_response_back() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                MemoryConnection::new(vec![b"GET /_ping HTTP/1.1\r\n\r\n".to_vec()]),
                Instant::from_millis(1),
            )
            .expect("open");
        assert_eq!(open.local_port, DEFAULT_HOST_INGRESS_FIRST_LOCAL_PORT);
        assert_eq!(open.guest_frames.len(), 1);
        assert_eq!(
            &open.guest_frames[0][0..6],
            EthernetAddress::BROADCAST.as_bytes()
        );

        let result = gateway.handle_guest_frame(arp_reply_frame(), Instant::from_millis(2));
        let syn = parse_tcp_frame(&result.guest_frames[0]).expect("syn");
        let syn_ack = tcp_frame(
            1075,
            open.local_port,
            TcpControl::Syn,
            TcpSeqNumber(200),
            Some(syn.seq_number + 1),
            &[],
        );
        let result = gateway.handle_guest_frame(syn_ack, Instant::from_millis(3));
        assert!(!result.guest_frames.is_empty());

        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(4));
        let guest_frames = events
            .iter()
            .find_map(|event| match event {
                HostIngressEvent::HostPayload { guest_frames, .. } => Some(guest_frames),
                _ => None,
            })
            .expect("host payload frames");
        assert!(!guest_frames.is_empty());
        let host_payload = parse_tcp_frame(guest_frames.last().expect("host payload frame"))
            .expect("host payload");

        gateway.handle_guest_frame(
            tcp_frame(
                1075,
                open.local_port,
                TcpControl::Psh,
                TcpSeqNumber(201),
                Some(host_payload.seq_number + host_payload.payload.len()),
                b"OK",
            ),
            Instant::from_millis(5),
        );
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(6));
        assert!(events
            .iter()
            .any(|event| matches!(event, HostIngressEvent::GuestPayload { bytes: 2, .. })));
        let connection = bridge
            .session_connection(open.handle)
            .expect("session connection");
        assert_eq!(connection.written, b"OK");

        gateway.handle_guest_frame(
            tcp_frame(
                1075,
                open.local_port,
                TcpControl::Fin,
                TcpSeqNumber(203),
                Some(host_payload.seq_number + host_payload.payload.len()),
                &[],
            ),
            Instant::from_millis(7),
        );
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(8));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::GuestClosed {
                    handle,
                    guest_port: 1075,
                    ..
                } if *handle == open.handle
            )
        }));
        assert!(bridge.session_connection(open.handle).is_none());
    }

    #[test]
    fn host_eof_closes_guest_session_and_removes_bridge_session() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                MemoryConnection::new(vec![Vec::new()]),
                Instant::from_millis(1),
            )
            .expect("open");
        let result = gateway.handle_guest_frame(arp_reply_frame(), Instant::from_millis(2));
        let syn = parse_tcp_frame(&result.guest_frames[0]).expect("syn");
        let syn_ack = tcp_frame(
            1075,
            open.local_port,
            TcpControl::Syn,
            TcpSeqNumber(200),
            Some(syn.seq_number + 1),
            &[],
        );
        gateway.handle_guest_frame(syn_ack, Instant::from_millis(3));

        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(4));

        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::HostClosed {
                    handle,
                    guest_port: 1075,
                    ..
                } if *handle == open.handle
            )
        }));
        assert!(bridge.session_connection(open.handle).is_none());
    }

    #[test]
    fn multiple_host_sessions_allocate_distinct_guest_side_ports() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let first = bridge
            .open_session(
                &mut gateway,
                1075,
                MemoryConnection::new(vec![]),
                Instant::from_millis(1),
            )
            .expect("first open");
        let second = bridge
            .open_session(
                &mut gateway,
                8080,
                MemoryConnection::new(vec![]),
                Instant::from_millis(2),
            )
            .expect("second open");

        assert_eq!(first.local_port, DEFAULT_HOST_INGRESS_FIRST_LOCAL_PORT);
        assert_eq!(second.local_port, DEFAULT_HOST_INGRESS_FIRST_LOCAL_PORT + 1);
        assert_ne!(first.handle, second.handle);
        assert_eq!(first.guest_port, 1075);
        assert_eq!(second.guest_port, 8080);
        assert_eq!(gateway.host_ingress_sessions().len(), 2);
    }

    #[test]
    fn host_read_failure_closes_guest_session_and_removes_bridge_session() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                FailingReadConnection,
                Instant::from_millis(1),
            )
            .expect("open");
        complete_guest_accept(&mut gateway, &open);

        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(4));

        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::HostReadFailed { handle, .. } if *handle == open.handle
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::HostClosed {
                    handle,
                    guest_port: 1075,
                    ..
                } if *handle == open.handle
            )
        }));
        assert!(bridge.session_connection(open.handle).is_none());
    }

    #[test]
    fn host_write_failure_closes_guest_session_and_removes_bridge_session() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                FailingWriteConnection,
                Instant::from_millis(1),
            )
            .expect("open");
        let gateway_ack = complete_guest_accept(&mut gateway, &open);
        gateway.handle_guest_frame(
            tcp_frame(
                1075,
                open.local_port,
                TcpControl::Psh,
                TcpSeqNumber(201),
                Some(gateway_ack),
                b"guest payload",
            ),
            Instant::from_millis(4),
        );

        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::HostWriteFailed { handle, .. } if *handle == open.handle
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                HostIngressEvent::HostClosed {
                    handle,
                    guest_port: 1075,
                    ..
                } if *handle == open.handle
            )
        }));
        assert!(bridge.session_connection(open.handle).is_none());
    }

    #[test]
    fn readiness_writes_guest_payload_immediately_without_waiting_for_writable_event() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                MemoryConnection::new(vec![]),
                Instant::from_millis(1),
            )
            .expect("open");
        let gateway_ack = complete_guest_accept(&mut gateway, &open);
        gateway.handle_guest_frame(
            tcp_frame(
                1075,
                open.local_port,
                TcpControl::Psh,
                TcpSeqNumber(201),
                Some(gateway_ack),
                b"guest payload",
            ),
            Instant::from_millis(4),
        );

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            HostIngressReadiness::selected(Vec::new(), Vec::new()),
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, HostIngressEvent::GuestPayload { bytes: 13, .. })));
        assert_eq!(
            bridge.session_interest(open.handle),
            Some(HostSessionInterest {
                readable: true,
                writable: false
            })
        );
        assert_eq!(
            bridge
                .session_connection(open.handle)
                .expect("session")
                .written,
            b"guest payload"
        );
    }

    #[test]
    fn readiness_buffers_guest_payload_when_immediate_host_write_would_block() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                WouldBlockWriteConnection,
                Instant::from_millis(1),
            )
            .expect("open");
        let gateway_ack = complete_guest_accept(&mut gateway, &open);
        gateway.handle_guest_frame(
            tcp_frame(
                1075,
                open.local_port,
                TcpControl::Psh,
                TcpSeqNumber(201),
                Some(gateway_ack),
                b"guest payload",
            ),
            Instant::from_millis(4),
        );

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            HostIngressReadiness::selected(Vec::new(), Vec::new()),
        );

        assert!(!events
            .iter()
            .any(|event| matches!(event, HostIngressEvent::GuestPayload { .. })));
        assert_eq!(
            bridge.session_interest(open.handle),
            Some(HostSessionInterest {
                readable: true,
                writable: true
            })
        );
    }

    #[test]
    fn readiness_drains_multiple_host_reads_until_would_block() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();

        let open = bridge
            .open_session(
                &mut gateway,
                1075,
                MemoryConnection::new(vec![b"frame".to_vec(), b"-body".to_vec()]),
                Instant::from_millis(1),
            )
            .expect("open");
        complete_guest_accept(&mut gateway, &open);

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            HostIngressReadiness::selected(vec![open.handle], Vec::new()),
        );

        let host_payload_bytes = events
            .iter()
            .filter_map(|event| match event {
                HostIngressEvent::HostPayload { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(host_payload_bytes, vec![5, 5]);
    }

    #[test]
    fn large_host_payload_backpressures_instead_of_discarding_unsent_bytes() {
        let network = GuestNetwork::default();
        let policy = VmnetPolicy::default_sandbox(network.clone());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = HostIngressBridge::new();
        let overflow = vec![b'b'; 5_020];

        let open = bridge
            .open_session(
                &mut gateway,
                1076,
                MemoryConnection::new(vec![vec![b'a'; 64 * 1024], overflow.clone()]),
                Instant::from_millis(1),
            )
            .expect("open");
        complete_guest_accept(&mut gateway, &open);

        let events = bridge.process_gateway_with_readiness(
            &mut gateway,
            Instant::from_millis(5),
            HostIngressReadiness::selected(vec![open.handle], Vec::new()),
        );

        let host_payload_bytes = events
            .iter()
            .filter_map(|event| match event {
                HostIngressEvent::HostPayload { bytes, .. } => Some(*bytes),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(host_payload_bytes, vec![64 * 1024, 5_020]);
        let session = bridge.sessions.get(&open.handle).expect("session remains");
        assert_eq!(session.pending_guest_write, overflow);
        assert_eq!(
            bridge.session_interest(open.handle),
            Some(HostSessionInterest {
                readable: false,
                writable: false
            })
        );
    }

    #[test]
    #[ignore = "requires loopback TCP bind/connect outside the command sandbox"]
    fn listener_set_accepts_each_configured_listener_with_purpose() {
        let configs = vec![
            HostListener::docker_api(0, 1075),
            HostListener::payload_control(0, 1076),
            HostListener::published_tcp(0, 8080),
        ];
        let listeners = HostIngressListenerSet::bind(&configs).expect("bind listeners");
        let addrs = listeners.local_addrs().expect("local addrs");
        for addr in addrs {
            let _stream = TcpStream::connect(addr).expect("connect listener");
        }

        let accepted = listeners
            .accept_pending()
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("accepted");

        assert_eq!(accepted.len(), 3);
        assert_eq!(accepted[0].guest_port, 1075);
        assert_eq!(accepted[0].purpose, HostListenerPurpose::DockerApi);
        assert_eq!(accepted[1].guest_port, 1076);
        assert_eq!(accepted[1].purpose, HostListenerPurpose::PayloadControl);
        assert_eq!(accepted[2].guest_port, 8080);
        assert_eq!(accepted[2].purpose, HostListenerPurpose::PublishedTcp);
    }

    #[derive(Debug)]
    struct MemoryConnection {
        reads: VecDeque<Vec<u8>>,
        written: Vec<u8>,
    }

    impl MemoryConnection {
        fn new(reads: Vec<Vec<u8>>) -> Self {
            Self {
                reads: reads.into(),
                written: Vec::new(),
            }
        }
    }

    impl Read for MemoryConnection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let Some(read) = self.reads.pop_front() else {
                return Err(io::Error::from(ErrorKind::WouldBlock));
            };
            let count = read.len().min(buffer.len());
            buffer[..count].copy_from_slice(&read[..count]);
            Ok(count)
        }
    }

    impl Write for MemoryConnection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingReadConnection;

    impl Read for FailingReadConnection {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::ConnectionReset))
        }
    }

    impl Write for FailingReadConnection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingWriteConnection;

    impl Read for FailingWriteConnection {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
    }

    impl Write for FailingWriteConnection {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct WouldBlockWriteConnection;

    impl Read for WouldBlockWriteConnection {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
    }

    impl Write for WouldBlockWriteConnection {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn complete_guest_accept(
        gateway: &mut VmnetGateway<'_>,
        open: &HostIngressOpen,
    ) -> TcpSeqNumber {
        let result = gateway.handle_guest_frame(arp_reply_frame(), Instant::from_millis(2));
        let syn = parse_tcp_frame(&result.guest_frames[0]).expect("syn");
        let gateway_ack = syn.seq_number + 1;
        let syn_ack = tcp_frame(
            open.guest_port,
            open.local_port,
            TcpControl::Syn,
            TcpSeqNumber(200),
            Some(gateway_ack),
            &[],
        );
        gateway.handle_guest_frame(syn_ack, Instant::from_millis(3));
        gateway_ack
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
            dst_addr: GATEWAY_IP,
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
            &IpAddress::Ipv4(GATEWAY_IP),
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

    fn parse_tcp_frame(frame: &[u8]) -> Option<TcpRepr<'_>> {
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
