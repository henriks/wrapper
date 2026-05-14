use std::collections::HashMap;
use std::io::{self, ErrorKind, Read, Write};
use std::net::Ipv4Addr;
use std::sync::Arc;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::IpAddress;

use crate::guest_tcp::GuestTcpSession;
use crate::tcp_gateway::{
    evaluate_tcp_destination, parse_http_request, HttpParseError, HttpRequestSummary, TcpAction,
    TcpConnectError, TcpDecision, TcpDestination, TcpUpstreamConnector,
};
use crate::tls_mitm::{
    rustls_client_config_with_native_roots, GuestTlsSession, TlsMitmAuthority, TlsMitmError,
    TlsUpstreamSession,
};
use crate::vmnet_gateway::VmnetGateway;

pub struct TcpProxyBridge<C>
where
    C: TcpUpstreamConnector,
{
    connector: C,
    sessions: HashMap<SocketHandle, UpstreamSession<C::Connection>>,
    tls_server_config: Option<Arc<rustls::ServerConfig>>,
    tls_client_config: Option<Arc<rustls::ClientConfig>>,
}

impl<C> TcpProxyBridge<C>
where
    C: TcpUpstreamConnector,
{
    pub fn new(connector: C) -> Self {
        Self {
            connector,
            sessions: HashMap::new(),
            tls_server_config: None,
            tls_client_config: None,
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
            tls_server_config: Some(Arc::new(authority.rustls_server_config()?)),
            tls_client_config: Some(tls_client_config),
        })
    }

    pub fn process_gateway(
        &mut self,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
    ) -> Vec<TcpProxyEvent>
    where
        C::Connection: Read + Write,
    {
        let mut events = Vec::new();
        for active in gateway.active_tcp_sessions() {
            if active.session.state != tcp::State::Established {
                continue;
            }
            let Some(destination) = destination_from_session(&active.session) else {
                continue;
            };

            if !self.sessions.contains_key(&active.handle) {
                let decision = evaluate_tcp_destination(gateway.policy(), &destination);
                if decision.action == TcpAction::Deny {
                    events.push(TcpProxyEvent::Denied {
                        handle: active.handle,
                        destination,
                        decision,
                    });
                    continue;
                }
                let guest_tls = if decision.action == TcpAction::InterceptHttps {
                    let Some(config) = &self.tls_server_config else {
                        events.push(TcpProxyEvent::TlsMitmUnavailable {
                            handle: active.handle,
                            destination,
                        });
                        continue;
                    };
                    match GuestTlsSession::new(config.clone()) {
                        Ok(session) => Some(session),
                        Err(error) => {
                            events.push(TcpProxyEvent::TlsMitmFailed {
                                handle: active.handle,
                                error,
                            });
                            continue;
                        }
                    }
                } else {
                    None
                };
                match self.connector.connect(&destination) {
                    Ok(connection) => {
                        events.push(TcpProxyEvent::Connected {
                            handle: active.handle,
                            destination: destination.clone(),
                            decision: decision.clone(),
                        });
                        self.sessions.insert(
                            active.handle,
                            UpstreamSession {
                                destination,
                                decision,
                                connection,
                                http_buffer: Vec::new(),
                                guest_tls,
                                upstream_tls: None,
                                upstream_tls_ready: false,
                                pending_upstream_bytes: Vec::new(),
                                pending_upstream_plaintext: Vec::new(),
                                pending_guest_bytes: Vec::new(),
                            },
                        );
                    }
                    Err(error) => {
                        events.push(TcpProxyEvent::ConnectFailed {
                            handle: active.handle,
                            destination,
                            error,
                        });
                        continue;
                    }
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
                self.process_guest_payload(active.handle, guest_bytes, gateway, now, &mut events);
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
            drain_upstream_tls_writes(session, active.handle, &mut events);
            flush_pending_upstream_bytes(session, active.handle, &mut events);
            if let Some(upstream_bytes) =
                read_available(&mut session.connection, &mut events, active.handle)
            {
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
                            events.push(TcpProxyEvent::TlsMitmFailed {
                                handle: active.handle,
                                error,
                            });
                            continue;
                        }
                    };
                    if !upstream_read.tls_to_upstream.is_empty() {
                        match write_buffered_best_effort(
                            &mut session.connection,
                            &mut session.pending_upstream_bytes,
                            &upstream_read.tls_to_upstream,
                        ) {
                            Ok(bytes) => {
                                if bytes > 0 {
                                    events.push(TcpProxyEvent::TlsUpstreamPayload {
                                        handle: active.handle,
                                        bytes,
                                    });
                                }
                            }
                            Err(error) => {
                                events.push(TcpProxyEvent::UpstreamWriteFailed {
                                    handle: active.handle,
                                    error,
                                });
                                continue;
                            }
                        }
                    }
                    if !session.upstream_tls_ready {
                        session.upstream_tls_ready = session
                            .upstream_tls
                            .as_ref()
                            .is_some_and(|upstream_tls| !upstream_tls.is_handshaking());
                    }
                    flush_pending_https_plaintext(session, active.handle, &mut events);
                    drain_upstream_tls_writes(session, active.handle, &mut events);
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
                            events.push(TcpProxyEvent::TlsMitmFailed {
                                handle: active.handle,
                                error,
                            });
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
        events
    }

    fn process_guest_payload(
        &mut self,
        handle: SocketHandle,
        guest_bytes: Vec<u8>,
        gateway: &mut VmnetGateway<'_>,
        now: Instant,
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
                    events.push(TcpProxyEvent::TlsMitmFailed { handle, error });
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
            if !ensure_https_upstream_tls(session, handle, self.tls_client_config.clone(), events) {
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
                session
                    .pending_upstream_plaintext
                    .extend_from_slice(&payload);
                return;
            }
            write_https_plaintext_upstream(session, handle, &payload, events);
        } else {
            match write_buffered_best_effort(
                &mut session.connection,
                &mut session.pending_upstream_bytes,
                &payload,
            ) {
                Ok(bytes) => events.push(TcpProxyEvent::GuestPayload { handle, bytes }),
                Err(error) => events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error }),
            }
        }
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestSendEvent {
    UpstreamPayload,
    TlsHandshakePayload,
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
    session.pending_guest_bytes.extend_from_slice(bytes);
    flush_pending_guest_bytes(session, handle, gateway, now, event, events);
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
        }
        Err(error) => events.push(TcpProxyEvent::GuestWriteFailed { handle, error }),
    }
}

fn ensure_https_upstream_tls<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    tls_client_config: Option<Arc<rustls::ClientConfig>>,
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
            events.push(TcpProxyEvent::TlsMitmFailed {
                handle,
                error: TlsMitmError::InvalidHost(
                    "guest TLS connection did not provide SNI".to_string(),
                ),
            });
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
            events.push(TcpProxyEvent::TlsMitmFailed { handle, error });
            return false;
        }
    };
    let client_hello = match upstream_tls.drain_tls_to_upstream() {
        Ok(client_hello) => client_hello,
        Err(error) => {
            events.push(TcpProxyEvent::TlsMitmFailed { handle, error });
            return false;
        }
    };
    if !client_hello.is_empty() {
        match write_buffered_best_effort(
            &mut session.connection,
            &mut session.pending_upstream_bytes,
            &client_hello,
        ) {
            Ok(bytes) => {
                if bytes > 0 {
                    events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
                }
            }
            Err(error) => {
                events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error });
                return false;
            }
        }
    }
    session.upstream_tls = Some(upstream_tls);
    true
}

fn flush_pending_https_plaintext<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    events: &mut Vec<TcpProxyEvent>,
) {
    if session.pending_upstream_plaintext.is_empty() {
        return;
    }
    if !session.upstream_tls_ready {
        return;
    }
    let pending = std::mem::take(&mut session.pending_upstream_plaintext);
    write_https_plaintext_upstream(session, handle, &pending, events);
}

fn write_https_plaintext_upstream<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
    plaintext: &[u8],
    events: &mut Vec<TcpProxyEvent>,
) {
    let Some(upstream_tls) = session.upstream_tls.as_mut() else {
        session
            .pending_upstream_plaintext
            .extend_from_slice(plaintext);
        return;
    };
    let tls_bytes = match upstream_tls.write_upstream_plaintext(plaintext) {
        Ok(tls_bytes) => tls_bytes,
        Err(error) => {
            events.push(TcpProxyEvent::TlsMitmFailed { handle, error });
            return;
        }
    };
    match write_buffered_best_effort(
        &mut session.connection,
        &mut session.pending_upstream_bytes,
        &tls_bytes,
    ) {
        Ok(bytes) => {
            events.push(TcpProxyEvent::GuestPayload {
                handle,
                bytes: plaintext.len(),
            });
            if bytes > 0 {
                events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
            }
        }
        Err(error) => events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error }),
    }
}

fn drain_upstream_tls_writes<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
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
            events.push(TcpProxyEvent::TlsMitmFailed { handle, error });
            return;
        }
    };
    if tls_bytes.is_empty() {
        return;
    }
    match write_buffered_best_effort(
        &mut session.connection,
        &mut session.pending_upstream_bytes,
        &tls_bytes,
    ) {
        Ok(bytes) if bytes > 0 => {
            events.push(TcpProxyEvent::TlsUpstreamPayload { handle, bytes });
        }
        Ok(_) => {}
        Err(error) => events.push(TcpProxyEvent::UpstreamWriteFailed { handle, error }),
    }
}

fn flush_pending_upstream_bytes<T: Write>(
    session: &mut UpstreamSession<T>,
    handle: SocketHandle,
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
        flush_pending_https_plaintext(session, handle, events);
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

fn read_available(
    connection: &mut impl Read,
    events: &mut Vec<TcpProxyEvent>,
    handle: SocketHandle,
) -> Option<Vec<u8>> {
    let mut buffer = vec![0; 64 * 1024];
    match connection.read(&mut buffer) {
        Ok(0) => None,
        Ok(count) => {
            buffer.truncate(count);
            Some(buffer)
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
        Err(error) => {
            events.push(TcpProxyEvent::UpstreamReadFailed {
                handle,
                error: error.to_string(),
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_policy::VmnetPolicy;
    use crate::test_support::TestCa;
    use crate::tls_mitm::rustls_client_config_with_roots;
    use crate::vmnet_gateway::{GuestFrameOutcome, VmnetGateway};
    use crate::GuestNetwork;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore};
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
    fn upstream_connect_failure_is_reported_without_creating_session() {
        let network = GuestNetwork::default();
        let mut policy = VmnetPolicy::default_sandbox(network.clone());
        policy.egress.allow_ips.push(PUBLIC_IP.to_string());
        let mut gateway =
            VmnetGateway::new(&policy, &network, Instant::from_millis(0)).expect("gateway");
        let mut bridge = TcpProxyBridge::new(FailingConnector);

        establish_http_session(&mut gateway);
        let events = bridge.process_gateway(&mut gateway, Instant::from_millis(5));

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::ConnectFailed {
                error: TcpConnectError::UpstreamUnavailable,
                ..
            }
        )));
        assert!(bridge.sessions.is_empty());
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

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::TlsMitmFailed {
                error: TlsMitmError::Tls(_),
                ..
            }
        )));
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

        assert!(events.iter().any(|event| matches!(
            event,
            TcpProxyEvent::TlsMitmFailed {
                error: TlsMitmError::Tls(_) | TlsMitmError::Io(_),
                ..
            }
        )));
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

    #[derive(Debug)]
    struct MemoryConnection {
        response: Vec<u8>,
        written: Vec<u8>,
        write_would_block_count: usize,
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
