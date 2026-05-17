//! Async service-IO boundary primitives for the vmnet runtime.
//!
//! The vmnet owner remains the only code that owns smoltcp state, QEMU frame
//! ordering, pcap capture, policy decisions tied to guest frames, and
//! guest-visible close/reset semantics. Service IO workers may only receive
//! bounded requests from the owner and return bounded completions back to the
//! owner. This module keeps the boundary explicit before individual DNS,
//! connect, host-ingress, or upstream-session slices choose a concrete worker
//! implementation.

use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};

use crate::dns_proxy::{DnsForwardRequest, DnsUpstream, DnsUpstreamError};
use crate::tcp_gateway::{TcpConnectError, TcpDestination, TcpUpstreamConnector};

use hickory_proto::op::Message;

/// Service-IO work classes that are allowed to live outside the vmnet owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmnetServiceIoKind {
    DnsLookup,
    TcpConnect,
    HostIngress,
    UpstreamSession,
    CertificateWorker,
}

/// Opaque owner-assigned correlation token for service IO requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VmnetServiceToken(u64);

/// Work submitted by the vmnet owner to service IO.
#[derive(Debug, Clone)]
pub enum VmnetServiceCommand {
    DnsLookup(VmnetDnsLookupCommand),
    TcpConnect(VmnetTcpConnectCommand),
    ByteIo(VmnetByteIoCommand),
    Cancel(VmnetServiceCancel),
}

impl VmnetServiceCommand {
    pub fn token(&self) -> VmnetServiceToken {
        match self {
            Self::DnsLookup(command) => command.token,
            Self::TcpConnect(command) => command.token,
            Self::ByteIo(command) => command.token,
            Self::Cancel(command) => command.token,
        }
    }

    pub fn kind(&self) -> VmnetServiceIoKind {
        match self {
            Self::DnsLookup(_) => VmnetServiceIoKind::DnsLookup,
            Self::TcpConnect(_) => VmnetServiceIoKind::TcpConnect,
            Self::ByteIo(command) => command.kind,
            Self::Cancel(command) => command.kind,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VmnetDnsLookupCommand {
    pub token: VmnetServiceToken,
    pub request: DnsForwardRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetTcpConnectCommand {
    pub token: VmnetServiceToken,
    pub destination: TcpDestination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetByteIoCommand {
    pub token: VmnetServiceToken,
    pub kind: VmnetServiceIoKind,
    pub operation: VmnetByteIoOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetByteIoOperation {
    Read { max_bytes: usize },
    Write { bytes: Vec<u8>, max_bytes: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmnetServiceCancel {
    pub token: VmnetServiceToken,
    pub kind: VmnetServiceIoKind,
}

/// Work returned by service IO to the vmnet owner.
///
/// The owner remains responsible for applying completions to smoltcp/QEMU state
/// in token order appropriate for that subsystem. In particular, DNS response
/// frame construction and TCP session close/reset behavior stay owner-side.
#[derive(Debug)]
pub enum VmnetServiceCompletion<C> {
    DnsLookup(VmnetDnsLookupCompletion),
    TcpConnect(VmnetTcpConnectCompletion<C>),
    ByteIo(VmnetByteIoCompletion),
    Cancelled(VmnetServiceCancel),
}

impl<C> VmnetServiceCompletion<C> {
    pub fn token(&self) -> VmnetServiceToken {
        match self {
            Self::DnsLookup(completion) => completion.token,
            Self::TcpConnect(completion) => completion.token,
            Self::ByteIo(completion) => completion.token,
            Self::Cancelled(completion) => completion.token,
        }
    }

    pub fn kind(&self) -> VmnetServiceIoKind {
        match self {
            Self::DnsLookup(_) => VmnetServiceIoKind::DnsLookup,
            Self::TcpConnect(_) => VmnetServiceIoKind::TcpConnect,
            Self::ByteIo(completion) => completion.kind,
            Self::Cancelled(completion) => completion.kind,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VmnetDnsLookupCompletion {
    pub token: VmnetServiceToken,
    pub result: Result<Message, DnsUpstreamError>,
}

#[derive(Debug)]
pub struct VmnetTcpConnectCompletion<C> {
    pub token: VmnetServiceToken,
    pub result: Result<C, TcpConnectError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetByteIoCompletion {
    pub token: VmnetServiceToken,
    pub kind: VmnetServiceIoKind,
    pub result: VmnetByteIoResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetByteIoResult {
    Read(VmnetByteReadResult),
    Write(VmnetByteWriteResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetByteReadResult {
    Data(Vec<u8>),
    Closed,
    WouldBlock,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmnetByteWriteResult {
    Written(usize),
    WouldBlock,
    Failed(String),
}

impl VmnetServiceToken {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Owner-side monotonic token allocator for correlating service completions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetServiceTokenSource {
    next: u64,
}

impl VmnetServiceTokenSource {
    pub fn new() -> Self {
        Self { next: 1 }
    }

    pub fn next_token(&mut self) -> VmnetServiceToken {
        let token = VmnetServiceToken::new(self.next);
        self.next += 1;
        token
    }
}

impl Default for VmnetServiceTokenSource {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when owner pending state already contains a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetServicePendingDuplicate<T> {
    pub token: VmnetServiceToken,
    pub item: T,
}

/// Owner-side pending state keyed by service correlation token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetServicePending<T> {
    items: HashMap<VmnetServiceToken, T>,
}

impl<T> VmnetServicePending<T> {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    pub fn insert(
        &mut self,
        token: VmnetServiceToken,
        item: T,
    ) -> Result<(), VmnetServicePendingDuplicate<T>> {
        if self.items.contains_key(&token) {
            return Err(VmnetServicePendingDuplicate { token, item });
        }
        self.items.insert(token, item);
        Ok(())
    }

    pub fn remove(&mut self, token: VmnetServiceToken) -> Option<T> {
        self.items.remove(&token)
    }

    pub fn remove_for_completion<C>(
        &mut self,
        completion: &VmnetServiceCompletion<C>,
    ) -> Option<T> {
        self.remove(completion.token())
    }

    pub fn contains(&self, token: VmnetServiceToken) -> bool {
        self.items.contains_key(&token)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn drain(&mut self) -> Vec<(VmnetServiceToken, T)> {
        self.items.drain().collect()
    }
}

impl<T> Default for VmnetServicePending<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pollable wakeup used by service workers to notify the vmnet owner.
#[derive(Debug)]
pub struct VmnetServiceWakeup {
    reader: UnixStream,
    writer: UnixStream,
}

impl VmnetServiceWakeup {
    pub fn new() -> io::Result<Self> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        Ok(Self { reader, writer })
    }

    pub fn reader_fd(&self) -> RawFd {
        self.reader.as_raw_fd()
    }

    pub fn notifier(&self) -> io::Result<VmnetServiceNotifier> {
        Ok(VmnetServiceNotifier {
            writer: self.writer.try_clone()?,
        })
    }

    pub fn drain(&mut self) -> io::Result<usize> {
        let mut total = 0;
        let mut buffer = [0_u8; 64];
        loop {
            match self.reader.read(&mut buffer) {
                Ok(0) => return Ok(total),
                Ok(count) => total += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(total),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

/// Cloneable service-side notifier half for `VmnetServiceWakeup`.
#[derive(Debug)]
pub struct VmnetServiceNotifier {
    writer: UnixStream,
}

impl VmnetServiceNotifier {
    pub fn notify(&mut self) -> io::Result<()> {
        loop {
            match self.writer.write(&[1]) {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

/// Result of executing one command in a DNS-capable service worker.
#[derive(Debug)]
pub enum VmnetDnsServiceExecution<C> {
    Completed(VmnetServiceCompletion<C>),
    Unsupported(VmnetServiceCommand),
}

pub fn execute_dns_service_command<C>(
    command: VmnetServiceCommand,
    upstream: &impl DnsUpstream,
) -> VmnetDnsServiceExecution<C> {
    match command {
        VmnetServiceCommand::DnsLookup(command) => {
            let result = upstream.exchange(&command.request.query);
            VmnetDnsServiceExecution::Completed(VmnetServiceCompletion::DnsLookup(
                VmnetDnsLookupCompletion {
                    token: command.token,
                    result,
                },
            ))
        }
        VmnetServiceCommand::Cancel(cancel) if cancel.kind == VmnetServiceIoKind::DnsLookup => {
            VmnetDnsServiceExecution::Completed(VmnetServiceCompletion::Cancelled(cancel))
        }
        command => VmnetDnsServiceExecution::Unsupported(command),
    }
}

/// Result of executing one command in a TCP-connect-capable service worker.
#[derive(Debug)]
pub enum VmnetTcpConnectServiceExecution<C> {
    Completed(VmnetServiceCompletion<C>),
    Unsupported(VmnetServiceCommand),
}

pub fn execute_tcp_connect_service_command<T>(
    command: VmnetServiceCommand,
    connector: &T,
) -> VmnetTcpConnectServiceExecution<T::Connection>
where
    T: TcpUpstreamConnector,
{
    match command {
        VmnetServiceCommand::TcpConnect(command) => {
            let result = connector.connect(&command.destination);
            VmnetTcpConnectServiceExecution::Completed(VmnetServiceCompletion::TcpConnect(
                VmnetTcpConnectCompletion {
                    token: command.token,
                    result,
                },
            ))
        }
        VmnetServiceCommand::Cancel(cancel) if cancel.kind == VmnetServiceIoKind::TcpConnect => {
            VmnetTcpConnectServiceExecution::Completed(VmnetServiceCompletion::Cancelled(cancel))
        }
        command => VmnetTcpConnectServiceExecution::Unsupported(command),
    }
}

/// Result of executing one command in a byte-IO-capable service worker.
#[derive(Debug)]
pub enum VmnetByteIoServiceExecution<C> {
    Completed(VmnetServiceCompletion<C>),
    Unsupported(VmnetServiceCommand),
}

pub fn execute_byte_io_service_command<C>(
    command: VmnetServiceCommand,
    connection: &mut (impl Read + Write),
) -> VmnetByteIoServiceExecution<C> {
    match command {
        VmnetServiceCommand::ByteIo(command) => {
            let result = match command.operation {
                VmnetByteIoOperation::Read { max_bytes } => {
                    VmnetByteIoResult::Read(execute_byte_io_read(connection, max_bytes))
                }
                VmnetByteIoOperation::Write { bytes, max_bytes } => {
                    VmnetByteIoResult::Write(execute_byte_io_write(connection, &bytes, max_bytes))
                }
            };
            VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(
                VmnetByteIoCompletion {
                    token: command.token,
                    kind: command.kind,
                    result,
                },
            ))
        }
        VmnetServiceCommand::Cancel(cancel)
            if matches!(
                cancel.kind,
                VmnetServiceIoKind::HostIngress | VmnetServiceIoKind::UpstreamSession
            ) =>
        {
            VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::Cancelled(cancel))
        }
        command => VmnetByteIoServiceExecution::Unsupported(command),
    }
}

fn execute_byte_io_read(connection: &mut impl Read, max_bytes: usize) -> VmnetByteReadResult {
    if max_bytes == 0 {
        return VmnetByteReadResult::WouldBlock;
    }
    let mut buffer = vec![0; max_bytes.min(64 * 1024)];
    match connection.read(&mut buffer) {
        Ok(0) => VmnetByteReadResult::Closed,
        Ok(count) => {
            buffer.truncate(count);
            VmnetByteReadResult::Data(buffer)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => VmnetByteReadResult::WouldBlock,
        Err(error) => VmnetByteReadResult::Failed(error.to_string()),
    }
}

fn execute_byte_io_write(
    connection: &mut impl Write,
    bytes: &[u8],
    max_bytes: usize,
) -> VmnetByteWriteResult {
    if max_bytes == 0 || bytes.is_empty() {
        return VmnetByteWriteResult::Written(0);
    }
    match connection.write(&bytes[..bytes.len().min(max_bytes)]) {
        Ok(count) => VmnetByteWriteResult::Written(count),
        Err(error) if error.kind() == ErrorKind::WouldBlock => VmnetByteWriteResult::WouldBlock,
        Err(error) => VmnetByteWriteResult::Failed(error.to_string()),
    }
}

/// Result of one byte-IO service task step over bounded owner/service queues.
#[derive(Debug)]
pub enum VmnetByteIoServiceStep<C> {
    Idle,
    Completed,
    Unsupported(VmnetServiceCommand),
    CompletionQueueFull(VmnetServiceQueueFull<VmnetServiceCompletion<C>>),
}

pub fn run_byte_io_service_owner_step<Pending, C>(
    service: &mut VmnetServiceOwner<Pending, C>,
    connection: &mut (impl Read + Write),
) -> VmnetByteIoServiceStep<C> {
    let Some(command) = service.service_recv_command() else {
        return VmnetByteIoServiceStep::Idle;
    };
    match execute_byte_io_service_command(command, connection) {
        VmnetByteIoServiceExecution::Completed(completion) => {
            match service.service_complete(completion) {
                Ok(()) => VmnetByteIoServiceStep::Completed,
                Err(full) => VmnetByteIoServiceStep::CompletionQueueFull(full),
            }
        }
        VmnetByteIoServiceExecution::Unsupported(command) => {
            VmnetByteIoServiceStep::Unsupported(command)
        }
    }
}

/// Result of one DNS service task step over the bounded owner/service queues.
#[derive(Debug)]
pub enum VmnetDnsServiceStep<C> {
    Idle,
    Completed,
    Unsupported(VmnetServiceCommand),
    CompletionQueueFull(VmnetServiceQueueFull<VmnetServiceCompletion<C>>),
}

pub fn run_dns_service_owner_step<Pending, C>(
    service: &mut VmnetServiceOwner<Pending, C>,
    upstream: &impl DnsUpstream,
) -> VmnetDnsServiceStep<C> {
    let Some(command) = service.service_recv_command() else {
        return VmnetDnsServiceStep::Idle;
    };
    match execute_dns_service_command(command, upstream) {
        VmnetDnsServiceExecution::Completed(completion) => {
            match service.service_complete(completion) {
                Ok(()) => VmnetDnsServiceStep::Completed,
                Err(full) => VmnetDnsServiceStep::CompletionQueueFull(full),
            }
        }
        VmnetDnsServiceExecution::Unsupported(command) => VmnetDnsServiceStep::Unsupported(command),
    }
}

pub struct VmnetServiceWorkerHandle<C> {
    command_tx: SyncSender<VmnetServiceCommand>,
    completion_rx: Receiver<VmnetServiceCompletion<C>>,
    join: JoinHandle<()>,
}

pub type VmnetDnsWorkerHandle<C> = VmnetServiceWorkerHandle<C>;
pub type VmnetTcpConnectWorkerHandle<C> = VmnetServiceWorkerHandle<C>;
pub type VmnetByteIoWorkerHandle<C = ()> = VmnetServiceWorkerHandle<C>;

impl<C> VmnetServiceWorkerHandle<C> {
    pub fn try_send_command(
        &self,
        command: VmnetServiceCommand,
    ) -> Result<(), TrySendError<VmnetServiceCommand>> {
        self.command_tx.try_send(command)
    }

    pub fn try_recv_completion(&self) -> Result<VmnetServiceCompletion<C>, TryRecvError> {
        self.completion_rx.try_recv()
    }

    pub fn shutdown(self) -> thread::Result<()> {
        let Self {
            command_tx,
            completion_rx: _,
            join,
        } = self;
        drop(command_tx);
        join.join()
    }
}

pub fn spawn_dns_service_worker<C, U>(
    upstream: U,
    mut notifier: VmnetServiceNotifier,
    limits: VmnetServiceIoLimits,
) -> Result<VmnetDnsWorkerHandle<C>, VmnetServiceIoLimitError>
where
    C: Send + 'static,
    U: DnsUpstream + Send + 'static,
{
    let limits = limits.validate()?;
    let (command_tx, command_rx) = mpsc::sync_channel(limits.command_capacity);
    let (completion_tx, completion_rx) = mpsc::sync_channel(limits.completion_capacity);
    let join = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            let VmnetDnsServiceExecution::Completed(completion) =
                execute_dns_service_command::<C>(command, &upstream)
            else {
                continue;
            };
            if completion_tx.try_send(completion).is_err() {
                break;
            }
            if notifier.notify().is_err() {
                break;
            }
        }
    });
    Ok(VmnetServiceWorkerHandle {
        command_tx,
        completion_rx,
        join,
    })
}

pub fn spawn_tcp_connect_service_worker<T>(
    connector: T,
    mut notifier: VmnetServiceNotifier,
    limits: VmnetServiceIoLimits,
) -> Result<VmnetTcpConnectWorkerHandle<T::Connection>, VmnetServiceIoLimitError>
where
    T: TcpUpstreamConnector + Send + 'static,
    T::Connection: Send + 'static,
{
    let limits = limits.validate()?;
    let (command_tx, command_rx) = mpsc::sync_channel(limits.command_capacity);
    let (completion_tx, completion_rx) = mpsc::sync_channel(limits.completion_capacity);
    let join = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            let VmnetTcpConnectServiceExecution::Completed(completion) =
                execute_tcp_connect_service_command(command, &connector)
            else {
                continue;
            };
            if completion_tx.try_send(completion).is_err() {
                break;
            }
            if notifier.notify().is_err() {
                break;
            }
        }
    });
    Ok(VmnetServiceWorkerHandle {
        command_tx,
        completion_rx,
        join,
    })
}

pub fn spawn_byte_io_service_worker<C, R>(
    mut connection: R,
    mut notifier: VmnetServiceNotifier,
    limits: VmnetServiceIoLimits,
) -> Result<VmnetByteIoWorkerHandle<C>, VmnetServiceIoLimitError>
where
    C: Send + 'static,
    R: Read + Write + Send + 'static,
{
    let limits = limits.validate()?;
    let (command_tx, command_rx) = mpsc::sync_channel(limits.command_capacity);
    let (completion_tx, completion_rx) = mpsc::sync_channel(limits.completion_capacity);
    let join = thread::spawn(move || {
        while let Ok(command) = command_rx.recv() {
            let VmnetByteIoServiceExecution::Completed(completion) =
                execute_byte_io_service_command::<C>(command, &mut connection)
            else {
                continue;
            };
            if completion_tx.try_send(completion).is_err() {
                break;
            }
            if notifier.notify().is_err() {
                break;
            }
        }
    });
    Ok(VmnetServiceWorkerHandle {
        command_tx,
        completion_rx,
        join,
    })
}

/// Error returned when owner submission to an arbitrary service sink fails.
#[derive(Debug)]
pub struct VmnetServiceSubmitError<T, E> {
    pub pending: T,
    pub command: VmnetServiceCommand,
    pub error: E,
}

/// Error returned when owner submission finds the command queue full.
#[derive(Debug)]
pub struct VmnetServiceSubmitFull<T> {
    pub capacity: usize,
    pub pending: T,
    pub command: VmnetServiceCommand,
}

/// Error returned when cancelling owner-pending service work fails.
#[derive(Debug)]
pub enum VmnetServiceCancelError {
    NotFound {
        token: VmnetServiceToken,
    },
    QueueFull {
        capacity: usize,
        command: VmnetServiceCommand,
    },
}

/// Owner-side service IO state: token allocation, pending context, and queues.
#[derive(Debug)]
pub struct VmnetServiceOwner<Pending, C> {
    tokens: VmnetServiceTokenSource,
    pending: VmnetServicePending<Pending>,
    queues: VmnetServiceIoQueues<VmnetServiceCommand, VmnetServiceCompletion<C>>,
}

impl<Pending, C> VmnetServiceOwner<Pending, C> {
    pub fn new(limits: VmnetServiceIoLimits) -> Result<Self, VmnetServiceIoLimitError> {
        Ok(Self {
            tokens: VmnetServiceTokenSource::new(),
            pending: VmnetServicePending::new(),
            queues: VmnetServiceIoQueues::new(limits)?,
        })
    }

    pub fn submit(
        &mut self,
        pending: Pending,
        build_command: impl FnOnce(&Pending, VmnetServiceToken) -> VmnetServiceCommand,
    ) -> Result<VmnetServiceToken, VmnetServiceSubmitFull<Pending>> {
        let token = self.tokens.next_token();
        let command = build_command(&pending, token);
        match self.queues.owner_send(command) {
            Ok(()) => {
                if self.pending.insert(token, pending).is_err() {
                    unreachable!("fresh service token unexpectedly duplicated pending state");
                }
                Ok(token)
            }
            Err(full) => Err(VmnetServiceSubmitFull {
                capacity: full.capacity,
                pending,
                command: full.item,
            }),
        }
    }

    pub fn submit_to<E>(
        &mut self,
        pending: Pending,
        build_command: impl FnOnce(&Pending, VmnetServiceToken) -> VmnetServiceCommand,
        enqueue: impl FnOnce(&VmnetServiceCommand) -> Result<(), E>,
    ) -> Result<VmnetServiceToken, VmnetServiceSubmitError<Pending, E>> {
        let token = self.tokens.next_token();
        let command = build_command(&pending, token);
        match enqueue(&command) {
            Ok(()) => {
                if self.pending.insert(token, pending).is_err() {
                    unreachable!("fresh service token unexpectedly duplicated pending state");
                }
                Ok(token)
            }
            Err(error) => Err(VmnetServiceSubmitError {
                pending,
                command,
                error,
            }),
        }
    }

    pub fn cancel_pending(
        &mut self,
        token: VmnetServiceToken,
        kind: VmnetServiceIoKind,
    ) -> Result<Pending, VmnetServiceCancelError> {
        let Some(pending) = self.pending.remove(token) else {
            return Err(VmnetServiceCancelError::NotFound { token });
        };
        let command = VmnetServiceCommand::Cancel(VmnetServiceCancel { token, kind });
        match self.queues.owner_send(command) {
            Ok(()) => Ok(pending),
            Err(full) => {
                if self.pending.insert(token, pending).is_err() {
                    unreachable!("removed service token unexpectedly duplicated on cancel restore");
                }
                Err(VmnetServiceCancelError::QueueFull {
                    capacity: full.capacity,
                    command: full.item,
                })
            }
        }
    }

    pub fn service_recv_command(&mut self) -> Option<VmnetServiceCommand> {
        self.queues.service_recv_command()
    }

    pub fn service_complete(
        &mut self,
        completion: VmnetServiceCompletion<C>,
    ) -> Result<(), VmnetServiceQueueFull<VmnetServiceCompletion<C>>> {
        self.queues.service_complete(completion)
    }

    pub fn owner_recv_completion(&mut self) -> Option<VmnetServiceCompletion<C>> {
        self.queues.owner_recv_completion()
    }

    pub fn remove_pending_for_completion(
        &mut self,
        completion: &VmnetServiceCompletion<C>,
    ) -> Option<Pending> {
        self.pending.remove_for_completion(completion)
    }

    pub fn drain_pending(&mut self) -> Vec<(VmnetServiceToken, Pending)> {
        self.pending.drain()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn command_len(&self) -> usize {
        self.queues.command_len()
    }

    pub fn completion_len(&self) -> usize {
        self.queues.completion_len()
    }
}

/// Bounded queue sizes for both directions across the vmnet owner boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmnetServiceIoLimits {
    pub command_capacity: usize,
    pub completion_capacity: usize,
}

impl VmnetServiceIoLimits {
    pub const fn new(command_capacity: usize, completion_capacity: usize) -> Self {
        Self {
            command_capacity,
            completion_capacity,
        }
    }

    pub fn validate(self) -> Result<Self, VmnetServiceIoLimitError> {
        if self.command_capacity == 0 {
            return Err(VmnetServiceIoLimitError::ZeroCommandCapacity);
        }
        if self.completion_capacity == 0 {
            return Err(VmnetServiceIoLimitError::ZeroCompletionCapacity);
        }
        Ok(self)
    }
}

impl Default for VmnetServiceIoLimits {
    fn default() -> Self {
        Self {
            command_capacity: 128,
            completion_capacity: 128,
        }
    }
}

/// A rejected queue item plus the capacity that rejected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetServiceQueueFull<T> {
    pub capacity: usize,
    pub item: T,
}

/// Construction errors for service IO boundary queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmnetServiceIoLimitError {
    ZeroCommandCapacity,
    ZeroCompletionCapacity,
}

/// In-process model of the owner/service boundary.
///
/// Tokio-backed implementations can replace the internal storage later, but
/// they must preserve these semantics: bounded FIFO in both directions and
/// explicit rejection when either queue is full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmnetServiceIoQueues<Command, Completion> {
    command_capacity: usize,
    completion_capacity: usize,
    commands: VecDeque<Command>,
    completions: VecDeque<Completion>,
}

impl<Command, Completion> VmnetServiceIoQueues<Command, Completion> {
    pub fn new(limits: VmnetServiceIoLimits) -> Result<Self, VmnetServiceIoLimitError> {
        let limits = limits.validate()?;
        Ok(Self {
            command_capacity: limits.command_capacity,
            completion_capacity: limits.completion_capacity,
            commands: VecDeque::with_capacity(limits.command_capacity),
            completions: VecDeque::with_capacity(limits.completion_capacity),
        })
    }

    pub fn owner_send(&mut self, command: Command) -> Result<(), VmnetServiceQueueFull<Command>> {
        if self.commands.len() >= self.command_capacity {
            return Err(VmnetServiceQueueFull {
                capacity: self.command_capacity,
                item: command,
            });
        }
        self.commands.push_back(command);
        Ok(())
    }

    pub fn service_recv_command(&mut self) -> Option<Command> {
        self.commands.pop_front()
    }

    pub fn service_complete(
        &mut self,
        completion: Completion,
    ) -> Result<(), VmnetServiceQueueFull<Completion>> {
        if self.completions.len() >= self.completion_capacity {
            return Err(VmnetServiceQueueFull {
                capacity: self.completion_capacity,
                item: completion,
            });
        }
        self.completions.push_back(completion);
        Ok(())
    }

    pub fn owner_recv_completion(&mut self) -> Option<Completion> {
        self.completions.pop_front()
    }

    pub fn command_len(&self) -> usize {
        self.commands.len()
    }

    pub fn completion_len(&self) -> usize {
        self.completions.len()
    }

    pub fn command_capacity(&self) -> usize {
        self.command_capacity
    }

    pub fn completion_capacity(&self) -> usize {
        self.completion_capacity
    }

    pub fn commands_are_full(&self) -> bool {
        self.commands.len() >= self.command_capacity
    }

    pub fn completions_are_full(&self) -> bool {
        self.completions.len() >= self.completion_capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Command {
        Dns(VmnetServiceToken),
        Connect(VmnetServiceToken),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Completion {
        Ready(VmnetServiceToken),
        Failed(VmnetServiceToken),
    }

    #[test]
    fn rejects_zero_capacity_in_either_direction() {
        assert_eq!(
            VmnetServiceIoQueues::<Command, Completion>::new(VmnetServiceIoLimits::new(0, 1)),
            Err(VmnetServiceIoLimitError::ZeroCommandCapacity)
        );
        assert_eq!(
            VmnetServiceIoQueues::<Command, Completion>::new(VmnetServiceIoLimits::new(1, 0)),
            Err(VmnetServiceIoLimitError::ZeroCompletionCapacity)
        );
    }

    #[test]
    fn owner_commands_are_fifo_and_do_not_emit_completions() {
        let mut queues =
            VmnetServiceIoQueues::<Command, Completion>::new(VmnetServiceIoLimits::new(4, 4))
                .expect("queues");

        queues
            .owner_send(Command::Dns(VmnetServiceToken::new(10)))
            .expect("first command");
        queues
            .owner_send(Command::Connect(VmnetServiceToken::new(11)))
            .expect("second command");

        assert_eq!(queues.command_len(), 2);
        assert_eq!(queues.completion_len(), 0);
        assert_eq!(
            queues.service_recv_command(),
            Some(Command::Dns(VmnetServiceToken::new(10)))
        );
        assert_eq!(
            queues.service_recv_command(),
            Some(Command::Connect(VmnetServiceToken::new(11)))
        );
        assert_eq!(queues.service_recv_command(), None);
        assert_eq!(queues.owner_recv_completion(), None);
    }

    #[test]
    fn full_command_queue_rejects_new_item_and_preserves_order() {
        let mut queues =
            VmnetServiceIoQueues::<Command, Completion>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("queues");

        queues
            .owner_send(Command::Dns(VmnetServiceToken::new(1)))
            .expect("first command");
        queues
            .owner_send(Command::Connect(VmnetServiceToken::new(2)))
            .expect("second command");

        let rejected = queues
            .owner_send(Command::Dns(VmnetServiceToken::new(3)))
            .expect_err("command queue is full");
        assert_eq!(rejected.capacity, 2);
        assert_eq!(rejected.item, Command::Dns(VmnetServiceToken::new(3)));
        assert!(queues.commands_are_full());
        assert_eq!(
            queues.service_recv_command(),
            Some(Command::Dns(VmnetServiceToken::new(1)))
        );
        assert_eq!(
            queues.service_recv_command(),
            Some(Command::Connect(VmnetServiceToken::new(2)))
        );
        assert_eq!(queues.service_recv_command(), None);
    }

    #[test]
    fn full_completion_queue_rejects_new_item_and_preserves_order() {
        let mut queues =
            VmnetServiceIoQueues::<Command, Completion>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("queues");

        queues
            .service_complete(Completion::Ready(VmnetServiceToken::new(1)))
            .expect("first completion");
        queues
            .service_complete(Completion::Failed(VmnetServiceToken::new(2)))
            .expect("second completion");

        let rejected = queues
            .service_complete(Completion::Ready(VmnetServiceToken::new(3)))
            .expect_err("completion queue is full");
        assert_eq!(rejected.capacity, 2);
        assert_eq!(rejected.item, Completion::Ready(VmnetServiceToken::new(3)));
        assert!(queues.completions_are_full());
        assert_eq!(
            queues.owner_recv_completion(),
            Some(Completion::Ready(VmnetServiceToken::new(1)))
        );
        assert_eq!(
            queues.owner_recv_completion(),
            Some(Completion::Failed(VmnetServiceToken::new(2)))
        );
        assert_eq!(queues.owner_recv_completion(), None);
    }

    #[test]
    fn service_metadata_identifies_allowed_off_owner_work() {
        assert_eq!(VmnetServiceToken::new(42).get(), 42);
        assert_eq!(VmnetServiceIoKind::DnsLookup, VmnetServiceIoKind::DnsLookup);
        assert_ne!(
            VmnetServiceIoKind::TcpConnect,
            VmnetServiceIoKind::HostIngress
        );
    }

    #[test]
    fn token_source_allocates_monotonic_owner_tokens() {
        let mut source = VmnetServiceTokenSource::new();

        assert_eq!(source.next_token(), VmnetServiceToken::new(1));
        assert_eq!(source.next_token(), VmnetServiceToken::new(2));
        assert_eq!(source.next_token(), VmnetServiceToken::new(3));
    }

    #[test]
    fn pending_state_rejects_duplicates_and_removes_by_token() {
        let mut pending = VmnetServicePending::new();
        let token = VmnetServiceToken::new(11);

        pending.insert(token, "first").expect("insert pending");
        let duplicate = pending
            .insert(token, "second")
            .expect_err("duplicate token rejected");

        assert_eq!(duplicate.token, token);
        assert_eq!(duplicate.item, "second");
        assert!(pending.contains(token));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.remove(token), Some("first"));
        assert!(pending.is_empty());
        assert_eq!(pending.remove(token), None);
    }

    #[test]
    fn pending_state_can_be_drained_for_owner_fail_closed_cleanup() {
        let mut pending = VmnetServicePending::new();
        pending
            .insert(VmnetServiceToken::new(1), "one")
            .expect("first insert");
        pending
            .insert(VmnetServiceToken::new(2), "two")
            .expect("second insert");

        let drained = pending.drain();

        assert_eq!(drained.len(), 2);
        assert!(drained.contains(&(VmnetServiceToken::new(1), "one")));
        assert!(drained.contains(&(VmnetServiceToken::new(2), "two")));
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_state_removes_matching_completion_and_ignores_stale_completion() {
        let mut pending = VmnetServicePending::new();
        let matching = VmnetServiceToken::new(21);
        let stale = VmnetServiceToken::new(22);
        pending.insert(matching, "dns-context").expect("insert");

        let stale_completion = VmnetServiceCompletion::<()>::DnsLookup(VmnetDnsLookupCompletion {
            token: stale,
            result: Err(DnsUpstreamError::Unavailable),
        });
        assert_eq!(pending.remove_for_completion(&stale_completion), None);
        assert!(pending.contains(matching));

        let matching_completion =
            VmnetServiceCompletion::<()>::DnsLookup(VmnetDnsLookupCompletion {
                token: matching,
                result: Err(DnsUpstreamError::Unavailable),
            });
        assert_eq!(
            pending.remove_for_completion(&matching_completion),
            Some("dns-context")
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn service_wakeup_is_pollable_and_drainable() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut notifier = wakeup.notifier().expect("notifier");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");

        notifier.notify().expect("notify");
        let events = poller
            .poll(Some(Duration::from_millis(100)))
            .expect("poll wakeup");

        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain") > 0);
        assert_eq!(wakeup.drain().expect("second drain"), 0);
    }

    #[test]
    fn owner_state_submits_pending_work_and_matches_completion() {
        use hickory_proto::op::Query;
        use hickory_proto::rr::{Name, RecordType};

        let mut owner =
            VmnetServiceOwner::<DnsForwardRequest, ()>::new(VmnetServiceIoLimits::new(2, 2))
                .expect("owner");
        let mut query = Message::query();
        query.add_query(Query::query(
            Name::from_ascii("example.com").expect("name"),
            RecordType::A,
        ));
        let pending = DnsForwardRequest {
            query,
            domain: "example.com".to_string(),
        };

        let token = owner
            .submit(pending, |request, token| {
                VmnetServiceCommand::DnsLookup(VmnetDnsLookupCommand {
                    token,
                    request: request.clone(),
                })
            })
            .expect("submit");

        assert_eq!(token, VmnetServiceToken::new(1));
        assert_eq!(owner.pending_len(), 1);
        assert_eq!(owner.command_len(), 1);
        let command = owner.service_recv_command().expect("command");
        assert_eq!(command.token(), token);
        owner
            .service_complete(VmnetServiceCompletion::DnsLookup(
                VmnetDnsLookupCompletion {
                    token,
                    result: Err(DnsUpstreamError::Unavailable),
                },
            ))
            .expect("completion");
        let completion = owner.owner_recv_completion().expect("owner completion");
        let pending = owner
            .remove_pending_for_completion(&completion)
            .expect("pending context");
        assert_eq!(pending.domain, "example.com");
        assert_eq!(owner.pending_len(), 0);
    }

    #[test]
    fn owner_state_rejects_submission_when_command_queue_is_full() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");
        owner
            .submit("first".to_string(), |item, token| {
                VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                    token,
                    destination: TcpDestination {
                        ip: "198.51.100.10".parse().expect("ip"),
                        port: item.len() as u16,
                        domain: None,
                    },
                })
            })
            .expect("first submit");

        let rejected = owner
            .submit("second".to_string(), |item, token| {
                VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                    token,
                    destination: TcpDestination {
                        ip: "198.51.100.11".parse().expect("ip"),
                        port: item.len() as u16,
                        domain: None,
                    },
                })
            })
            .expect_err("command queue full");

        assert_eq!(rejected.capacity, 1);
        assert_eq!(rejected.pending, "second");
        assert_eq!(rejected.command.token(), VmnetServiceToken::new(2));
        assert_eq!(owner.pending_len(), 1);
        assert_eq!(owner.command_len(), 1);
    }

    #[test]
    fn owner_state_can_submit_to_external_bounded_sink() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");
        let mut sent = Vec::new();

        let token = owner
            .submit_to(
                "pending".to_string(),
                |item, token| {
                    VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                        token,
                        destination: TcpDestination {
                            ip: "198.51.100.21".parse().expect("ip"),
                            port: item.len() as u16,
                            domain: None,
                        },
                    })
                },
                |command| {
                    sent.push(command.clone());
                    Ok::<(), &'static str>(())
                },
            )
            .expect("submit to external sink");

        assert_eq!(token, VmnetServiceToken::new(1));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].token(), token);
        assert_eq!(owner.pending_len(), 1);
        assert_eq!(owner.command_len(), 0);
    }

    #[test]
    fn owner_state_does_not_insert_pending_when_external_sink_rejects() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");

        let error = owner
            .submit_to(
                "pending".to_string(),
                |item, token| {
                    VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                        token,
                        destination: TcpDestination {
                            ip: "198.51.100.22".parse().expect("ip"),
                            port: item.len() as u16,
                            domain: None,
                        },
                    })
                },
                |_command| Err::<(), _>("full"),
            )
            .expect_err("external sink rejected");

        assert_eq!(error.pending, "pending");
        assert_eq!(error.command.token(), VmnetServiceToken::new(1));
        assert_eq!(error.error, "full");
        assert_eq!(owner.pending_len(), 0);
        assert_eq!(owner.command_len(), 0);
    }

    #[test]
    fn owner_state_cancels_pending_work_with_bounded_cancel_command() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(2, 1)).expect("owner");
        let token = owner
            .submit("pending".to_string(), |item, token| {
                VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                    token,
                    destination: TcpDestination {
                        ip: "198.51.100.12".parse().expect("ip"),
                        port: item.len() as u16,
                        domain: None,
                    },
                })
            })
            .expect("submit");
        assert!(owner.service_recv_command().is_some());

        let cancelled = owner
            .cancel_pending(token, VmnetServiceIoKind::TcpConnect)
            .expect("cancel pending");

        assert_eq!(cancelled, "pending");
        assert_eq!(owner.pending_len(), 0);
        let command = owner.service_recv_command().expect("cancel command");
        assert_eq!(command.token(), token);
        assert_eq!(command.kind(), VmnetServiceIoKind::TcpConnect);
        assert!(matches!(command, VmnetServiceCommand::Cancel(_)));
    }

    #[test]
    fn owner_state_keeps_pending_when_cancel_command_queue_is_full() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");
        let token = owner
            .submit("pending".to_string(), |item, token| {
                VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
                    token,
                    destination: TcpDestination {
                        ip: "198.51.100.13".parse().expect("ip"),
                        port: item.len() as u16,
                        domain: None,
                    },
                })
            })
            .expect("submit");

        let error = owner
            .cancel_pending(token, VmnetServiceIoKind::TcpConnect)
            .expect_err("cancel queue full");

        let VmnetServiceCancelError::QueueFull { capacity, command } = error else {
            panic!("expected full cancel queue");
        };
        assert_eq!(capacity, 1);
        assert_eq!(command.token(), token);
        assert_eq!(owner.pending_len(), 1);
        assert_eq!(owner.command_len(), 1);
    }

    #[test]
    fn typed_dns_and_connect_commands_expose_tokens_without_owner_state() {
        use hickory_proto::op::Query;
        use hickory_proto::rr::{Name, RecordType};
        use std::net::Ipv4Addr;

        let mut query = Message::query();
        query.add_query(Query::query(
            Name::from_ascii("example.com").expect("name"),
            RecordType::A,
        ));
        let dns = VmnetServiceCommand::DnsLookup(VmnetDnsLookupCommand {
            token: VmnetServiceToken::new(7),
            request: DnsForwardRequest {
                query,
                domain: "example.com".to_string(),
            },
        });
        let connect = VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
            token: VmnetServiceToken::new(8),
            destination: TcpDestination {
                ip: Ipv4Addr::new(198, 51, 100, 10),
                port: 443,
                domain: Some("example.com".to_string()),
            },
        });
        let byte_io = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
            token: VmnetServiceToken::new(9),
            kind: VmnetServiceIoKind::HostIngress,
            operation: VmnetByteIoOperation::Read { max_bytes: 1024 },
        });

        assert_eq!(dns.token(), VmnetServiceToken::new(7));
        assert_eq!(dns.kind(), VmnetServiceIoKind::DnsLookup);
        assert_eq!(connect.token(), VmnetServiceToken::new(8));
        assert_eq!(connect.kind(), VmnetServiceIoKind::TcpConnect);
        assert_eq!(byte_io.token(), VmnetServiceToken::new(9));
        assert_eq!(byte_io.kind(), VmnetServiceIoKind::HostIngress);
    }

    #[test]
    fn byte_io_executor_caps_read_and_write_sizes() {
        let mut connection = ByteIoConnection::new(b"abcdef".to_vec());
        let read = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
            token: VmnetServiceToken::new(51),
            kind: VmnetServiceIoKind::HostIngress,
            operation: VmnetByteIoOperation::Read { max_bytes: 3 },
        });

        let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(read_completion)) =
            execute_byte_io_service_command::<()>(read, &mut connection)
        else {
            panic!("expected byte-io read completion");
        };
        assert_eq!(read_completion.token, VmnetServiceToken::new(51));
        assert_eq!(read_completion.kind, VmnetServiceIoKind::HostIngress);
        assert_eq!(
            read_completion.result,
            VmnetByteIoResult::Read(VmnetByteReadResult::Data(b"abc".to_vec()))
        );

        let write = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
            token: VmnetServiceToken::new(52),
            kind: VmnetServiceIoKind::UpstreamSession,
            operation: VmnetByteIoOperation::Write {
                bytes: b"012345".to_vec(),
                max_bytes: 4,
            },
        });
        let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(
            write_completion,
        )) = execute_byte_io_service_command::<()>(write, &mut connection)
        else {
            panic!("expected byte-io write completion");
        };
        assert_eq!(write_completion.token, VmnetServiceToken::new(52));
        assert_eq!(write_completion.kind, VmnetServiceIoKind::UpstreamSession);
        assert_eq!(
            write_completion.result,
            VmnetByteIoResult::Write(VmnetByteWriteResult::Written(4))
        );
        assert_eq!(connection.written, b"0123");
    }

    #[test]
    fn byte_io_executor_reports_would_block_and_cancel_without_io() {
        let mut connection = WouldBlockByteIoConnection;
        let read = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
            token: VmnetServiceToken::new(53),
            kind: VmnetServiceIoKind::HostIngress,
            operation: VmnetByteIoOperation::Read { max_bytes: 8 },
        });
        let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(completion)) =
            execute_byte_io_service_command::<()>(read, &mut connection)
        else {
            panic!("expected byte-io completion");
        };
        assert_eq!(
            completion.result,
            VmnetByteIoResult::Read(VmnetByteReadResult::WouldBlock)
        );

        let cancel = VmnetServiceCommand::Cancel(VmnetServiceCancel {
            token: VmnetServiceToken::new(54),
            kind: VmnetServiceIoKind::UpstreamSession,
        });
        let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::Cancelled(cancelled)) =
            execute_byte_io_service_command::<()>(cancel, &mut connection)
        else {
            panic!("expected byte-io cancel completion");
        };
        assert_eq!(cancelled.token, VmnetServiceToken::new(54));
        assert_eq!(cancelled.kind, VmnetServiceIoKind::UpstreamSession);
    }

    #[derive(Debug)]
    struct ByteIoConnection {
        readable: Vec<u8>,
        written: Vec<u8>,
    }

    impl ByteIoConnection {
        fn new(readable: Vec<u8>) -> Self {
            Self {
                readable,
                written: Vec::new(),
            }
        }
    }

    impl Read for ByteIoConnection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.readable.is_empty() {
                return Ok(0);
            }
            let count = buffer.len().min(self.readable.len());
            buffer[..count].copy_from_slice(&self.readable[..count]);
            self.readable.drain(..count);
            Ok(count)
        }
    }

    impl Write for ByteIoConnection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct WouldBlockByteIoConnection;

    impl Read for WouldBlockByteIoConnection {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }
    }

    impl Write for WouldBlockByteIoConnection {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(ErrorKind::WouldBlock))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    proptest! {
        #[test]
        fn proptest_byte_io_executor_caps_arbitrary_read_write_lengths(
            data in proptest::collection::vec(any::<u8>(), 0..8192),
            write_bytes in proptest::collection::vec(any::<u8>(), 0..8192),
            read_max in 0usize..9000,
            write_max in 0usize..9000,
        ) {
            let mut read_connection = ByteIoConnection::new(data.clone());
            let read = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
                token: VmnetServiceToken::new(71),
                kind: VmnetServiceIoKind::HostIngress,
                operation: VmnetByteIoOperation::Read { max_bytes: read_max },
            });
            let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(read_completion)) =
                execute_byte_io_service_command::<()>(read, &mut read_connection)
            else {
                panic!("expected byte-io read completion");
            };
            let expected_read = data
                .iter()
                .copied()
                .take(read_max.min(64 * 1024))
                .collect::<Vec<_>>();
            let expected_read_result = if read_max == 0 {
                VmnetByteIoResult::Read(VmnetByteReadResult::WouldBlock)
            } else if expected_read.is_empty() {
                VmnetByteIoResult::Read(VmnetByteReadResult::Closed)
            } else {
                VmnetByteIoResult::Read(VmnetByteReadResult::Data(expected_read))
            };
            prop_assert_eq!(read_completion.result, expected_read_result);

            let mut write_connection = ByteIoConnection::new(Vec::new());
            let write = VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
                token: VmnetServiceToken::new(72),
                kind: VmnetServiceIoKind::UpstreamSession,
                operation: VmnetByteIoOperation::Write {
                    bytes: write_bytes.clone(),
                    max_bytes: write_max,
                },
            });
            let VmnetByteIoServiceExecution::Completed(VmnetServiceCompletion::ByteIo(write_completion)) =
                execute_byte_io_service_command::<()>(write, &mut write_connection)
            else {
                panic!("expected byte-io write completion");
            };
            let expected_write_len = write_bytes.len().min(write_max);
            prop_assert_eq!(
                write_completion.result,
                VmnetByteIoResult::Write(VmnetByteWriteResult::Written(expected_write_len))
            );
            prop_assert_eq!(&write_connection.written, &write_bytes[..expected_write_len]);
        }
    }

    #[derive(Debug)]
    struct StaticDnsUpstream {
        response: Message,
    }

    impl DnsUpstream for StaticDnsUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            Ok(self.response.clone())
        }
    }

    #[derive(Debug)]
    struct PanicDnsUpstream;

    impl DnsUpstream for PanicDnsUpstream {
        fn exchange(&self, _query: &Message) -> Result<Message, DnsUpstreamError> {
            panic!("unexpected DNS upstream call")
        }
    }

    fn example_dns_command(token: VmnetServiceToken) -> VmnetServiceCommand {
        use hickory_proto::op::Query;
        use hickory_proto::rr::{Name, RecordType};

        let mut query = Message::query();
        query.add_query(Query::query(
            Name::from_ascii("example.com").expect("name"),
            RecordType::A,
        ));
        VmnetServiceCommand::DnsLookup(VmnetDnsLookupCommand {
            token,
            request: DnsForwardRequest {
                query,
                domain: "example.com".to_string(),
            },
        })
    }

    #[test]
    fn dns_service_executor_runs_lookup_command() {
        let token = VmnetServiceToken::new(31);
        let response = Message::query();

        let execution = execute_dns_service_command::<()>(
            example_dns_command(token),
            &StaticDnsUpstream {
                response: response.clone(),
            },
        );

        let VmnetDnsServiceExecution::Completed(VmnetServiceCompletion::DnsLookup(completion)) =
            execution
        else {
            panic!("expected DNS completion");
        };
        assert_eq!(completion.token, token);
        assert_eq!(completion.result.expect("dns response"), response);
    }

    #[test]
    fn dns_service_executor_converts_dns_cancel_without_upstream_call() {
        let token = VmnetServiceToken::new(32);
        let execution = execute_dns_service_command::<()>(
            VmnetServiceCommand::Cancel(VmnetServiceCancel {
                token,
                kind: VmnetServiceIoKind::DnsLookup,
            }),
            &PanicDnsUpstream,
        );

        let VmnetDnsServiceExecution::Completed(VmnetServiceCompletion::Cancelled(cancelled)) =
            execution
        else {
            panic!("expected cancelled completion");
        };
        assert_eq!(cancelled.token, token);
        assert_eq!(cancelled.kind, VmnetServiceIoKind::DnsLookup);
    }

    #[test]
    fn dns_service_executor_leaves_non_dns_commands_for_other_workers() {
        let command = VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
            token: VmnetServiceToken::new(33),
            destination: TcpDestination {
                ip: "198.51.100.33".parse().expect("ip"),
                port: 443,
                domain: None,
            },
        });

        let execution = execute_dns_service_command::<()>(command, &PanicDnsUpstream);

        let VmnetDnsServiceExecution::Unsupported(command) = execution else {
            panic!("expected unsupported TCP command");
        };
        assert_eq!(command.token(), VmnetServiceToken::new(33));
        assert_eq!(command.kind(), VmnetServiceIoKind::TcpConnect);
    }

    #[derive(Debug)]
    struct StaticTcpConnector {
        result: Result<&'static str, TcpConnectError>,
    }

    impl TcpUpstreamConnector for StaticTcpConnector {
        type Connection = &'static str;

        fn connect(
            &self,
            _destination: &TcpDestination,
        ) -> Result<Self::Connection, TcpConnectError> {
            self.result.clone()
        }
    }

    fn example_tcp_connect_command(token: VmnetServiceToken) -> VmnetServiceCommand {
        VmnetServiceCommand::TcpConnect(VmnetTcpConnectCommand {
            token,
            destination: TcpDestination {
                ip: "198.51.100.44".parse().expect("ip"),
                port: 443,
                domain: Some("example.com".to_string()),
            },
        })
    }

    fn example_byte_read_command(token: VmnetServiceToken) -> VmnetServiceCommand {
        VmnetServiceCommand::ByteIo(VmnetByteIoCommand {
            token,
            kind: VmnetServiceIoKind::HostIngress,
            operation: VmnetByteIoOperation::Read { max_bytes: 4 },
        })
    }

    #[test]
    fn tcp_connect_service_executor_runs_connect_command() {
        let token = VmnetServiceToken::new(41);
        let execution = execute_tcp_connect_service_command(
            example_tcp_connect_command(token),
            &StaticTcpConnector { result: Ok("conn") },
        );

        let VmnetTcpConnectServiceExecution::Completed(VmnetServiceCompletion::TcpConnect(
            completion,
        )) = execution
        else {
            panic!("expected TCP connect completion");
        };
        assert_eq!(completion.token, token);
        assert_eq!(completion.result.expect("connection"), "conn");
    }

    #[test]
    fn tcp_connect_service_executor_reports_connect_failure() {
        let token = VmnetServiceToken::new(42);
        let execution = execute_tcp_connect_service_command(
            example_tcp_connect_command(token),
            &StaticTcpConnector {
                result: Err(TcpConnectError::UpstreamUnavailable),
            },
        );

        let VmnetTcpConnectServiceExecution::Completed(VmnetServiceCompletion::TcpConnect(
            completion,
        )) = execution
        else {
            panic!("expected TCP connect completion");
        };
        assert_eq!(completion.token, token);
        assert_eq!(completion.result, Err(TcpConnectError::UpstreamUnavailable));
    }

    #[test]
    fn tcp_connect_service_executor_converts_cancel_without_connect_call() {
        let token = VmnetServiceToken::new(43);
        let execution = execute_tcp_connect_service_command(
            VmnetServiceCommand::Cancel(VmnetServiceCancel {
                token,
                kind: VmnetServiceIoKind::TcpConnect,
            }),
            &StaticTcpConnector { result: Ok("conn") },
        );

        let VmnetTcpConnectServiceExecution::Completed(VmnetServiceCompletion::Cancelled(
            cancelled,
        )) = execution
        else {
            panic!("expected cancelled completion");
        };
        assert_eq!(cancelled.token, token);
        assert_eq!(cancelled.kind, VmnetServiceIoKind::TcpConnect);
    }

    #[test]
    fn tcp_connect_service_executor_leaves_dns_for_dns_worker() {
        let command = example_dns_command(VmnetServiceToken::new(44));

        let execution = execute_tcp_connect_service_command(
            command,
            &StaticTcpConnector { result: Ok("conn") },
        );

        let VmnetTcpConnectServiceExecution::Unsupported(command) = execution else {
            panic!("expected unsupported DNS command");
        };
        assert_eq!(command.token(), VmnetServiceToken::new(44));
        assert_eq!(command.kind(), VmnetServiceIoKind::DnsLookup);
    }

    #[test]
    fn dns_service_owner_step_moves_command_to_completion_queue() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(2, 2)).expect("owner");
        let token = owner
            .submit("pending".to_string(), |_item, token| {
                example_dns_command(token)
            })
            .expect("submit");

        let response = Message::query();
        let step = run_dns_service_owner_step(
            &mut owner,
            &StaticDnsUpstream {
                response: response.clone(),
            },
        );

        assert!(matches!(step, VmnetDnsServiceStep::Completed));
        assert_eq!(owner.command_len(), 0);
        assert_eq!(owner.completion_len(), 1);
        let completion = owner.owner_recv_completion().expect("completion");
        assert_eq!(completion.token(), token);
        let VmnetServiceCompletion::DnsLookup(completion) = completion else {
            panic!("expected dns completion");
        };
        assert_eq!(completion.result.expect("response"), response);
    }

    #[test]
    fn dns_service_owner_step_reports_full_completion_queue() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(2, 1)).expect("owner");
        owner
            .service_complete(VmnetServiceCompletion::Cancelled(VmnetServiceCancel {
                token: VmnetServiceToken::new(99),
                kind: VmnetServiceIoKind::DnsLookup,
            }))
            .expect("pre-fill completion queue");
        let token = owner
            .submit("pending".to_string(), |_item, token| {
                example_dns_command(token)
            })
            .expect("submit");

        let step = run_dns_service_owner_step(
            &mut owner,
            &StaticDnsUpstream {
                response: Message::query(),
            },
        );

        let VmnetDnsServiceStep::CompletionQueueFull(full) = step else {
            panic!("expected full completion queue");
        };
        assert_eq!(full.capacity, 1);
        assert_eq!(full.item.token(), token);
        assert_eq!(owner.command_len(), 0);
        assert_eq!(owner.completion_len(), 1);
        assert_eq!(owner.pending_len(), 1);
    }

    #[test]
    fn dns_service_owner_step_idles_without_upstream_call() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");

        let step = run_dns_service_owner_step(&mut owner, &PanicDnsUpstream);

        assert!(matches!(step, VmnetDnsServiceStep::Idle));
    }

    #[test]
    fn spawned_dns_worker_completes_and_wakes_owner() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let response = Message::query();
        let worker = spawn_dns_service_worker::<(), _>(
            StaticDnsUpstream {
                response: response.clone(),
            },
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("spawn worker");
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");
        let token = owner
            .submit_to(
                "pending".to_string(),
                |_item, token| example_dns_command(token),
                |command| worker.try_send_command(command.clone()),
            )
            .expect("submit to worker");

        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll service wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        let completion = worker.try_recv_completion().expect("completion");
        assert_eq!(completion.token(), token);
        let VmnetServiceCompletion::DnsLookup(completion) = completion else {
            panic!("expected DNS completion");
        };
        assert_eq!(completion.result.expect("response"), response);
        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn spawned_dns_worker_rejects_zero_capacity_limits() {
        let wakeup = VmnetServiceWakeup::new().expect("wakeup");
        assert!(matches!(
            spawn_dns_service_worker::<(), _>(
                StaticDnsUpstream {
                    response: Message::query(),
                },
                wakeup.notifier().expect("notifier"),
                VmnetServiceIoLimits::new(0, 1),
            ),
            Err(VmnetServiceIoLimitError::ZeroCommandCapacity)
        ));
    }

    #[test]
    fn spawned_tcp_connect_worker_completes_and_wakes_owner() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let worker = spawn_tcp_connect_service_worker(
            StaticTcpConnector { result: Ok("conn") },
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("spawn worker");
        let mut owner =
            VmnetServiceOwner::<String, &'static str>::new(VmnetServiceIoLimits::new(1, 1))
                .expect("owner");
        let token = owner
            .submit_to(
                "pending".to_string(),
                |_item, token| example_tcp_connect_command(token),
                |command| worker.try_send_command(command.clone()),
            )
            .expect("submit to worker");

        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll service wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        let completion = worker.try_recv_completion().expect("completion");
        assert_eq!(completion.token(), token);
        let VmnetServiceCompletion::TcpConnect(completion) = completion else {
            panic!("expected TCP connect completion");
        };
        assert_eq!(completion.result.expect("connection"), "conn");
        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn spawned_tcp_connect_worker_shutdown_does_not_block_on_full_completion_queue() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let worker = spawn_tcp_connect_service_worker(
            StaticTcpConnector { result: Ok("conn") },
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(1, 1),
        )
        .expect("spawn worker");

        worker
            .try_send_command(example_tcp_connect_command(VmnetServiceToken::new(1)))
            .expect("first command");
        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll first completion wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        worker
            .try_send_command(example_tcp_connect_command(VmnetServiceToken::new(2)))
            .expect("second command");

        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn byte_io_service_owner_step_moves_command_to_completion_queue() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(2, 2)).expect("owner");
        let token = owner
            .submit("pending".to_string(), |_item, token| {
                example_byte_read_command(token)
            })
            .expect("submit");
        let mut connection = ByteIoConnection::new(b"abcdef".to_vec());

        let step = run_byte_io_service_owner_step(&mut owner, &mut connection);

        assert!(matches!(step, VmnetByteIoServiceStep::Completed));
        assert_eq!(owner.command_len(), 0);
        assert_eq!(owner.completion_len(), 1);
        let completion = owner.owner_recv_completion().expect("completion");
        assert_eq!(completion.token(), token);
        let VmnetServiceCompletion::ByteIo(completion) = completion else {
            panic!("expected byte-io completion");
        };
        assert_eq!(completion.kind, VmnetServiceIoKind::HostIngress);
        assert_eq!(
            completion.result,
            VmnetByteIoResult::Read(VmnetByteReadResult::Data(b"abcd".to_vec()))
        );
    }

    #[test]
    fn byte_io_service_owner_step_reports_full_completion_queue() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(2, 1)).expect("owner");
        owner
            .service_complete(VmnetServiceCompletion::Cancelled(VmnetServiceCancel {
                token: VmnetServiceToken::new(98),
                kind: VmnetServiceIoKind::HostIngress,
            }))
            .expect("pre-fill completion queue");
        let token = owner
            .submit("pending".to_string(), |_item, token| {
                example_byte_read_command(token)
            })
            .expect("submit");
        let mut connection = ByteIoConnection::new(b"abcdef".to_vec());

        let step = run_byte_io_service_owner_step(&mut owner, &mut connection);

        let VmnetByteIoServiceStep::CompletionQueueFull(full) = step else {
            panic!("expected full completion queue");
        };
        assert_eq!(full.capacity, 1);
        assert_eq!(full.item.token(), token);
        assert_eq!(owner.command_len(), 0);
        assert_eq!(owner.completion_len(), 1);
        assert_eq!(owner.pending_len(), 1);
    }

    #[test]
    fn byte_io_service_owner_step_idles_without_io() {
        let mut owner =
            VmnetServiceOwner::<String, ()>::new(VmnetServiceIoLimits::new(1, 1)).expect("owner");
        let mut connection = WouldBlockByteIoConnection;

        let step = run_byte_io_service_owner_step(&mut owner, &mut connection);

        assert!(matches!(step, VmnetByteIoServiceStep::Idle));
    }

    #[test]
    fn spawned_byte_io_worker_completes_and_wakes_owner() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let worker = spawn_byte_io_service_worker::<(), _>(
            ByteIoConnection::new(b"service-bytes".to_vec()),
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("spawn worker");
        worker
            .try_send_command(example_byte_read_command(VmnetServiceToken::new(61)))
            .expect("send read command");

        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll service wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        let completion = worker.try_recv_completion().expect("completion");
        assert_eq!(completion.token(), VmnetServiceToken::new(61));
        let VmnetServiceCompletion::ByteIo(completion) = completion else {
            panic!("expected byte-io completion");
        };
        assert_eq!(completion.kind, VmnetServiceIoKind::HostIngress);
        assert_eq!(
            completion.result,
            VmnetByteIoResult::Read(VmnetByteReadResult::Data(b"serv".to_vec()))
        );
        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn spawned_byte_io_worker_ignores_unsupported_commands_and_keeps_running() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let worker = spawn_byte_io_service_worker::<(), _>(
            ByteIoConnection::new(b"still-running".to_vec()),
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(2, 2),
        )
        .expect("spawn worker");
        worker
            .try_send_command(example_dns_command(VmnetServiceToken::new(64)))
            .expect("send unsupported dns command");
        worker
            .try_send_command(example_byte_read_command(VmnetServiceToken::new(65)))
            .expect("send byte read command");

        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll service wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        let completion = worker.try_recv_completion().expect("completion");
        assert_eq!(completion.token(), VmnetServiceToken::new(65));
        assert!(matches!(completion, VmnetServiceCompletion::ByteIo(_)));
        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn spawned_byte_io_worker_shutdown_does_not_block_on_full_completion_queue() {
        use std::time::Duration;

        let mut wakeup = VmnetServiceWakeup::new().expect("wakeup");
        let mut poller = crate::vmnet_poller::RuntimePoller::new().expect("poller");
        poller
            .register_fd(
                crate::vmnet_poller::VmnetEventSource::ServiceIo,
                wakeup.reader_fd(),
                crate::vmnet_poller::VmnetInterest::READABLE,
            )
            .expect("register wakeup");
        let worker = spawn_byte_io_service_worker::<(), _>(
            ByteIoConnection::new(b"abcdefgh".to_vec()),
            wakeup.notifier().expect("notifier"),
            VmnetServiceIoLimits::new(1, 1),
        )
        .expect("spawn worker");

        worker
            .try_send_command(example_byte_read_command(VmnetServiceToken::new(62)))
            .expect("first command");
        let events = poller
            .poll(Some(Duration::from_millis(500)))
            .expect("poll first completion wakeup");
        assert!(events.iter().any(|event| {
            event.source == crate::vmnet_poller::VmnetEventSource::ServiceIo && event.readable
        }));
        assert!(wakeup.drain().expect("drain wakeup") > 0);
        worker
            .try_send_command(example_byte_read_command(VmnetServiceToken::new(63)))
            .expect("second command");

        worker.shutdown().expect("worker shutdown");
    }

    #[test]
    fn spawned_byte_io_worker_rejects_zero_capacity_limits() {
        let wakeup = VmnetServiceWakeup::new().expect("wakeup");
        assert!(matches!(
            spawn_byte_io_service_worker::<(), _>(
                ByteIoConnection::new(Vec::new()),
                wakeup.notifier().expect("notifier"),
                VmnetServiceIoLimits::new(0, 1),
            ),
            Err(VmnetServiceIoLimitError::ZeroCommandCapacity)
        ));
    }

    #[test]
    fn spawned_tcp_connect_worker_rejects_zero_capacity_limits() {
        let wakeup = VmnetServiceWakeup::new().expect("wakeup");
        assert!(matches!(
            spawn_tcp_connect_service_worker(
                StaticTcpConnector { result: Ok("conn") },
                wakeup.notifier().expect("notifier"),
                VmnetServiceIoLimits::new(1, 0),
            ),
            Err(VmnetServiceIoLimitError::ZeroCompletionCapacity)
        ));
    }

    #[test]
    fn typed_completion_metadata_keeps_application_owner_side() {
        let dns = VmnetServiceCompletion::<()>::DnsLookup(VmnetDnsLookupCompletion {
            token: VmnetServiceToken::new(9),
            result: Err(DnsUpstreamError::Unavailable),
        });
        let cancelled = VmnetServiceCompletion::<()>::Cancelled(VmnetServiceCancel {
            token: VmnetServiceToken::new(10),
            kind: VmnetServiceIoKind::TcpConnect,
        });
        let byte_io = VmnetServiceCompletion::<()>::ByteIo(VmnetByteIoCompletion {
            token: VmnetServiceToken::new(11),
            kind: VmnetServiceIoKind::UpstreamSession,
            result: VmnetByteIoResult::Write(VmnetByteWriteResult::Written(7)),
        });

        assert_eq!(dns.token(), VmnetServiceToken::new(9));
        assert_eq!(dns.kind(), VmnetServiceIoKind::DnsLookup);
        assert_eq!(cancelled.token(), VmnetServiceToken::new(10));
        assert_eq!(cancelled.kind(), VmnetServiceIoKind::TcpConnect);
        assert_eq!(byte_io.token(), VmnetServiceToken::new(11));
        assert_eq!(byte_io.kind(), VmnetServiceIoKind::UpstreamSession);
    }
}
