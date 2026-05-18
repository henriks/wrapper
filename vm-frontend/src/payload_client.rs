use std::io;
#[cfg(test)]
use std::io::{Read, Write};
#[cfg(test)]
use std::net::{Shutdown, TcpStream};
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc, Mutex,
};
#[cfg(test)]
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub use agentvm_payload_protocol::{DiagnosticRequest, PayloadEvent, PayloadRequest};

#[cfg(test)]
use agentvm_payload_protocol::{decode_header, encode_frame, FRAME_HEADER_LEN};
use agentvm_payload_protocol::{
    payload_event_from_frame, read_frame_async, resize_payload, signal_payload, write_frame_async,
    AsyncFrameError, Frame, FrameError, FrameKind, PayloadEventError,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::task::JoinHandle as TokioJoinHandle;

#[cfg(all(test, unix))]
static SIGNAL_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

#[derive(Debug)]
pub enum PayloadClientError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(String),
    Address(String),
    Unsupported(String),
    DeadlineExceeded,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadControlOptions {
    pub forward_signals: bool,
    pub forward_resize: bool,
}

impl PayloadControlOptions {
    pub fn interactive() -> Self {
        Self {
            forward_signals: true,
            forward_resize: true,
        }
    }

    pub fn disabled() -> Self {
        Self {
            forward_signals: false,
            forward_resize: false,
        }
    }

    pub fn policy(self) -> PayloadControlPolicy {
        PayloadControlPolicy {
            forward_signals: self.forward_signals,
            forward_resize: self.forward_resize,
            repeated_interrupt_aborts: self.forward_signals,
            terminate_aborts: self.forward_signals,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadControlPolicy {
    pub forward_signals: bool,
    pub forward_resize: bool,
    pub repeated_interrupt_aborts: bool,
    pub terminate_aborts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadControlAction {
    ForwardSignal(i32),
    ForwardResize,
    LocalAbort,
    Ignore,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadControlLoopDecision {
    Continue,
    Stop,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct PayloadCancelToken {
    done: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<TcpStream>>>,
}

#[cfg(test)]
impl PayloadCancelToken {
    pub fn new() -> Self {
        Self {
            done: Arc::new(AtomicBool::new(false)),
            stream: Arc::new(Mutex::new(None)),
        }
    }

    pub fn cancel(&self) {
        self.done.store(true, Ordering::SeqCst);
        if let Ok(stream) = self.stream.lock() {
            if let Some(stream) = stream.as_ref() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn shared_flag(&self) -> Arc<AtomicBool> {
        self.done.clone()
    }

    fn register_stream(&self, stream: TcpStream) {
        if self.is_cancelled() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Ok(mut current) = self.stream.lock() {
            *current = Some(stream);
        }
    }

    fn unregister_stream(&self) {
        if let Ok(mut current) = self.stream.lock() {
            *current = None;
        }
    }
}

#[cfg(test)]
impl Default for PayloadCancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadSessionOutcome {
    Exit(i32),
    Failure(String),
    Cancelled,
}

#[cfg(test)]
pub struct PayloadSessionRunner {
    control: PayloadControlOptions,
    cancel: PayloadCancelToken,
}

#[cfg(test)]
impl PayloadSessionRunner {
    pub fn new(control: PayloadControlOptions) -> Self {
        Self::with_cancel_token(control, PayloadCancelToken::new())
    }

    pub fn with_cancel_token(control: PayloadControlOptions, cancel: PayloadCancelToken) -> Self {
        Self { control, cancel }
    }

    pub fn cancel_token(&self) -> PayloadCancelToken {
        self.cancel.clone()
    }

    pub fn run_tcp_session(
        &self,
        session: &mut PayloadSession<TcpStream>,
        input: Option<Box<dyn Read + Send>>,
        output: &mut impl Write,
    ) -> Result<PayloadSessionOutcome, PayloadClientError> {
        let done = self.cancel.shared_flag();
        let send_lock = Arc::new(Mutex::new(session.stream.try_clone()?));
        self.cancel.register_stream(session.stream.try_clone()?);

        if let Some(mut input) = input {
            let done = done.clone();
            let send_lock = send_lock.clone();
            thread::Builder::new()
                .name("agentvm-payload-stdin".to_string())
                .spawn(move || {
                    let mut buffer = [0; 64 * 1024];
                    while !done.load(Ordering::SeqCst) {
                        let Ok(count) = input.read(&mut buffer) else {
                            return;
                        };
                        if count == 0 {
                            return;
                        }
                        let Ok(mut writer) = send_lock.lock() else {
                            return;
                        };
                        if send_frame(&mut *writer, b'I', &buffer[..count]).is_err() {
                            return;
                        }
                    }
                })
                .map_err(PayloadClientError::Io)?;
        }
        let signal_forwarder =
            SignalForwarder::install(self.control, send_lock.clone(), done.clone())?;

        let result = (|| -> Result<PayloadSessionOutcome, PayloadClientError> {
            loop {
                match session.recv_event() {
                    Ok(PayloadEvent::Output(payload)) => {
                        output.write_all(&payload)?;
                        output.flush()?;
                    }
                    Ok(PayloadEvent::Exit(exit_code)) => {
                        return Ok(PayloadSessionOutcome::Exit(exit_code));
                    }
                    Ok(PayloadEvent::Failure(message)) => {
                        return Ok(PayloadSessionOutcome::Failure(message));
                    }
                    Err(_error) if self.cancel.is_cancelled() => {
                        return Ok(PayloadSessionOutcome::Cancelled);
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        self.cancel.cancel();
        self.cancel.unregister_stream();
        drop(signal_forwarder);
        result
    }
}

impl PayloadSessionOutcome {
    pub fn into_exit_code(self) -> Result<i32, PayloadClientError> {
        match self {
            PayloadSessionOutcome::Exit(exit_code) => Ok(exit_code),
            PayloadSessionOutcome::Failure(message) => Err(PayloadClientError::Protocol(message)),
            PayloadSessionOutcome::Cancelled => Err(PayloadClientError::Cancelled),
        }
    }
}

#[derive(Debug)]
pub enum AsyncPayloadCommand {
    Input(Vec<u8>),
    Signal(i32),
    Resize { rows: u16, cols: u16 },
}

#[derive(Debug, Clone)]
pub struct AsyncPayloadCommandSender {
    tx: mpsc::Sender<AsyncPayloadCommand>,
}

impl AsyncPayloadCommandSender {
    pub async fn send(&self, command: AsyncPayloadCommand) -> Result<(), PayloadClientError> {
        self.tx
            .send(command)
            .await
            .map_err(|_| PayloadClientError::Cancelled)
    }

    pub async fn send_input(&self, input: impl Into<Vec<u8>>) -> Result<(), PayloadClientError> {
        self.send(AsyncPayloadCommand::Input(input.into())).await
    }

    pub async fn send_signal(&self, signal: i32) -> Result<(), PayloadClientError> {
        self.send(AsyncPayloadCommand::Signal(signal)).await
    }

    pub async fn send_resize(&self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        self.send(AsyncPayloadCommand::Resize { rows, cols }).await
    }

    pub fn try_send(&self, command: AsyncPayloadCommand) -> Result<(), PayloadClientError> {
        self.tx.try_send(command).map_err(command_send_error)
    }

    pub fn try_send_input(&self, input: impl Into<Vec<u8>>) -> Result<(), PayloadClientError> {
        self.try_send(AsyncPayloadCommand::Input(input.into()))
    }

    pub fn try_send_signal(&self, signal: i32) -> Result<(), PayloadClientError> {
        self.try_send(AsyncPayloadCommand::Signal(signal))
    }

    pub fn try_send_resize(&self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        self.try_send(AsyncPayloadCommand::Resize { rows, cols })
    }

    pub fn blocking_send(&self, command: AsyncPayloadCommand) -> Result<(), PayloadClientError> {
        self.tx
            .blocking_send(command)
            .map_err(|_| PayloadClientError::Cancelled)
    }

    pub fn blocking_send_input(&self, input: impl Into<Vec<u8>>) -> Result<(), PayloadClientError> {
        self.blocking_send(AsyncPayloadCommand::Input(input.into()))
    }

    pub fn blocking_send_signal(&self, signal: i32) -> Result<(), PayloadClientError> {
        self.blocking_send(AsyncPayloadCommand::Signal(signal))
    }

    pub fn blocking_send_resize(&self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        self.blocking_send(AsyncPayloadCommand::Resize { rows, cols })
    }
}

fn command_send_error(error: TrySendError<AsyncPayloadCommand>) -> PayloadClientError {
    match error {
        TrySendError::Full(_) => PayloadClientError::Io(io::Error::new(
            io::ErrorKind::WouldBlock,
            "payload command channel is full",
        )),
        TrySendError::Closed(_) => PayloadClientError::Cancelled,
    }
}

pub struct AsyncPayloadSession {
    command_tx: mpsc::Sender<AsyncPayloadCommand>,
    event_rx: mpsc::Receiver<Result<PayloadEvent, PayloadClientError>>,
    reader_task: TokioJoinHandle<()>,
    writer_task: TokioJoinHandle<()>,
}

impl AsyncPayloadSession {
    pub async fn from_stream<S>(
        stream: S,
        request: &PayloadRequest,
    ) -> Result<Self, PayloadClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::from_stream_with_capacity(stream, request, 32, 32).await
    }

    pub async fn from_stream_with_capacity<S>(
        stream: S,
        request: &PayloadRequest,
        command_capacity: usize,
        event_capacity: usize,
    ) -> Result<Self, PayloadClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let request_json = serde_json::to_vec(request)?;
        write_frame_async(
            &mut writer,
            &Frame::new(FrameKind::RUN_PRIMARY, request_json).map_err(frame_protocol_error)?,
        )
        .await?;

        let (command_tx, mut command_rx) = mpsc::channel(command_capacity.max(1));
        let (event_tx, event_rx) = mpsc::channel(event_capacity.max(1));

        let reader_task = tokio::spawn(async move {
            loop {
                let event = match read_frame_async(&mut reader).await {
                    Ok(frame) => payload_event_from_frame(frame).map_err(PayloadClientError::from),
                    Err(error) => Err(PayloadClientError::from(error)),
                };
                let terminal = !matches!(event, Ok(PayloadEvent::Output(_)));
                if event_tx.send(event).await.is_err() || terminal {
                    break;
                }
            }
        });

        let writer_task = tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let frame = match command {
                    AsyncPayloadCommand::Input(input) => Frame::new(FrameKind::INPUT, input),
                    AsyncPayloadCommand::Signal(signal) => Frame::new(
                        FrameKind::SIGNAL,
                        signal_payload(signal)
                            .expect("serializing signal control payload cannot fail"),
                    ),
                    AsyncPayloadCommand::Resize { rows, cols } => Frame::new(
                        FrameKind::RESIZE,
                        resize_payload(rows, cols)
                            .expect("serializing resize control payload cannot fail"),
                    ),
                };
                let Ok(frame) = frame else {
                    break;
                };
                if write_frame_async(&mut writer, &frame).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            command_tx,
            event_rx,
            reader_task,
            writer_task,
        })
    }

    pub fn command_sender(&self) -> AsyncPayloadCommandSender {
        AsyncPayloadCommandSender {
            tx: self.command_tx.clone(),
        }
    }

    pub async fn send_input(&self, input: impl Into<Vec<u8>>) -> Result<(), PayloadClientError> {
        self.command_sender().send_input(input).await
    }

    pub async fn send_signal(&self, signal: i32) -> Result<(), PayloadClientError> {
        self.command_sender().send_signal(signal).await
    }

    pub async fn send_resize(&self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        self.command_sender().send_resize(rows, cols).await
    }

    pub async fn recv_event(&mut self) -> Result<PayloadEvent, PayloadClientError> {
        match self.event_rx.recv().await {
            Some(event) => event,
            None => Err(PayloadClientError::Cancelled),
        }
    }

    pub fn cancel(&self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

impl Drop for AsyncPayloadSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub async fn run_payload_tcp_async_with_control<W>(
    addr: SocketAddr,
    request: &PayloadRequest,
    input: Option<Box<dyn AsyncRead + Send + Unpin>>,
    output: &mut W,
    control: PayloadControlOptions,
) -> Result<PayloadSessionOutcome, PayloadClientError>
where
    W: AsyncWrite + Unpin,
{
    let stream = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| PayloadClientError::DeadlineExceeded)??;
    stream.set_nodelay(true)?;
    let mut session = AsyncPayloadSession::from_stream(stream, request).await?;
    run_payload_session_async(&mut session, input, output, control).await
}

pub async fn run_payload_session_async<W>(
    session: &mut AsyncPayloadSession,
    input: Option<Box<dyn AsyncRead + Send + Unpin>>,
    output: &mut W,
    control: PayloadControlOptions,
) -> Result<PayloadSessionOutcome, PayloadClientError>
where
    W: AsyncWrite + Unpin,
{
    let command_sender = session.command_sender();
    let (abort_tx, mut abort_rx) = mpsc::channel(1);
    let signal_task = spawn_async_signal_forwarder(control, command_sender.clone(), abort_tx)?;
    let mut input = input;
    let mut buffer = [0; 64 * 1024];

    let result = loop {
        tokio::select! {
            _ = abort_rx.recv(), if signal_task.is_some() => {
                session.cancel();
                break Ok(PayloadSessionOutcome::Cancelled);
            }
            read = async {
                let input = input.as_mut().expect("input branch is disabled when input is absent");
                tokio::io::AsyncReadExt::read(input, &mut buffer).await
            }, if input.is_some() => {
                let count = read?;
                if count == 0 {
                    input = None;
                } else {
                    command_sender.send_input(buffer[..count].to_vec()).await?;
                }
            }
            event = session.recv_event() => {
                match event {
                    Ok(PayloadEvent::Output(payload)) => {
                        output.write_all(&payload).await?;
                        output.flush().await?;
                    }
                    Ok(PayloadEvent::Exit(exit_code)) => {
                        break Ok(PayloadSessionOutcome::Exit(exit_code));
                    }
                    Ok(PayloadEvent::Failure(message)) => {
                        break Ok(PayloadSessionOutcome::Failure(message));
                    }
                    Err(error) => break Err(error),
                }
            }
        }
    };

    if let Some(task) = signal_task {
        task.abort();
    }
    session.cancel();
    result
}

fn spawn_async_signal_forwarder(
    control: PayloadControlOptions,
    command_sender: AsyncPayloadCommandSender,
    abort_tx: mpsc::Sender<()>,
) -> Result<Option<TokioJoinHandle<()>>, PayloadClientError> {
    if !control.forward_signals && !control.forward_resize {
        return Ok(None);
    }

    #[cfg(unix)]
    {
        Ok(Some(tokio::spawn(async move {
            async_signal_forward_loop(control.policy(), command_sender, abort_tx).await;
        })))
    }

    #[cfg(not(unix))]
    {
        let _ = (command_sender, abort_tx);
        Err(PayloadClientError::Unsupported(
            "payload signal forwarding is only supported on Unix hosts".to_string(),
        ))
    }
}

#[cfg(unix)]
async fn async_signal_forward_loop(
    policy: PayloadControlPolicy,
    command_sender: AsyncPayloadCommandSender,
    abort_tx: mpsc::Sender<()>,
) {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = if policy.forward_signals {
        signal(SignalKind::interrupt()).ok()
    } else {
        None
    };
    let mut terminate = if policy.forward_signals || policy.terminate_aborts {
        signal(SignalKind::terminate()).ok()
    } else {
        None
    };
    let mut hangup = if policy.forward_signals {
        signal(SignalKind::hangup()).ok()
    } else {
        None
    };
    let mut resize = if policy.forward_resize {
        signal(SignalKind::window_change()).ok()
    } else {
        None
    };
    if interrupt.is_none() && terminate.is_none() && hangup.is_none() && resize.is_none() {
        return;
    }
    let mut forwarded_interrupts = 0;

    loop {
        let signal = tokio::select! {
            _ = async { interrupt.as_mut().expect("interrupt stream").recv().await }, if interrupt.is_some() => libc::SIGINT,
            _ = async { terminate.as_mut().expect("terminate stream").recv().await }, if terminate.is_some() => libc::SIGTERM,
            _ = async { hangup.as_mut().expect("hangup stream").recv().await }, if hangup.is_some() => libc::SIGHUP,
            _ = async { resize.as_mut().expect("resize stream").recv().await }, if resize.is_some() => libc::SIGWINCH,
        };
        let action = policy.host_signal_action(signal, forwarded_interrupts);
        match action {
            PayloadControlAction::ForwardSignal(signal) => {
                if command_sender.send_signal(signal).await.is_err() {
                    return;
                }
                if signal == libc::SIGINT {
                    forwarded_interrupts += 1;
                }
            }
            PayloadControlAction::ForwardResize => {
                let (rows, cols) = terminal_size();
                if command_sender.send_resize(rows, cols).await.is_err() {
                    return;
                }
            }
            PayloadControlAction::LocalAbort => {
                let _ = abort_tx.send(()).await;
                return;
            }
            PayloadControlAction::Ignore => {}
        }
    }
}

pub async fn ping_payload_async_tcp(addr: SocketAddr) -> Result<(), PayloadClientError> {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        ping_payload_async_io(&mut stream).await
    })
    .await
    .map_err(|_| PayloadClientError::DeadlineExceeded)?
}

pub async fn ping_payload_async_io<S>(stream: &mut S) -> Result<(), PayloadClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_frame_async(
        stream,
        &Frame::new(FrameKind::PING, Vec::new()).map_err(frame_protocol_error)?,
    )
    .await?;
    let frame = read_frame_async(stream).await?;
    if frame.kind == FrameKind::OK && frame.payload == b"ok" {
        return Ok(());
    }
    Err(PayloadClientError::Protocol(format!(
        "unexpected ping response: frame={:?} payload={}",
        frame.kind.as_byte(),
        String::from_utf8_lossy(&frame.payload)
    )))
}

pub async fn run_diagnostic_tcp_async<W>(
    addr: SocketAddr,
    request: &DiagnosticRequest,
    output: &mut W,
) -> Result<i32, PayloadClientError>
where
    W: AsyncWrite + Unpin,
{
    run_diagnostic_tcp_async_with_deadline(
        addr,
        request,
        output,
        Duration::from_secs(request.timeout_seconds.max(1)),
    )
    .await
}

pub async fn run_diagnostic_tcp_async_with_deadline<W>(
    addr: SocketAddr,
    request: &DiagnosticRequest,
    output: &mut W,
    deadline: Duration,
) -> Result<i32, PayloadClientError>
where
    W: AsyncWrite + Unpin,
{
    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| PayloadClientError::DeadlineExceeded)??;
    stream.set_nodelay(true)?;
    run_diagnostic_async_io(&mut stream, request, output, deadline).await
}

pub async fn run_diagnostic_async_io<S, W>(
    stream: &mut S,
    request: &DiagnosticRequest,
    output: &mut W,
    deadline: Duration,
) -> Result<i32, PayloadClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(deadline, async {
        let request_json = serde_json::to_vec(request)?;
        write_frame_async(
            stream,
            &Frame::new(FrameKind::RUN_DIAGNOSTIC, request_json).map_err(frame_protocol_error)?,
        )
        .await?;
        loop {
            match payload_event_from_frame(read_frame_async(stream).await?)? {
                PayloadEvent::Output(payload) => {
                    output.write_all(&payload).await?;
                    output.flush().await?;
                }
                PayloadEvent::Exit(exit_code) => return Ok(exit_code),
                PayloadEvent::Failure(message) => {
                    return Err(PayloadClientError::Protocol(message))
                }
            }
        }
    })
    .await
    .map_err(|_| PayloadClientError::DeadlineExceeded)?
}

impl PayloadControlPolicy {
    pub fn disabled() -> Self {
        PayloadControlOptions::disabled().policy()
    }

    pub fn interactive() -> Self {
        PayloadControlOptions::interactive().policy()
    }
}

#[cfg(unix)]
impl PayloadControlPolicy {
    pub fn host_signal_action(
        self,
        signal: i32,
        forwarded_interrupts: usize,
    ) -> PayloadControlAction {
        match signal {
            libc::SIGWINCH if self.forward_resize => PayloadControlAction::ForwardResize,
            libc::SIGINT if self.forward_signals => {
                if self.repeated_interrupt_aborts && forwarded_interrupts > 0 {
                    PayloadControlAction::LocalAbort
                } else {
                    PayloadControlAction::ForwardSignal(signal)
                }
            }
            libc::SIGTERM if self.terminate_aborts => PayloadControlAction::LocalAbort,
            libc::SIGTERM | libc::SIGHUP if self.forward_signals => {
                PayloadControlAction::ForwardSignal(signal)
            }
            _ => PayloadControlAction::Ignore,
        }
    }

    pub fn tui_ctrl_c_action(self, forwarded_interrupts: usize) -> PayloadControlAction {
        self.host_signal_action(libc::SIGINT, forwarded_interrupts)
    }

    #[cfg(test)]
    fn installed_unix_signals(self) -> Vec<i32> {
        let mut signals = Vec::new();
        if self.forward_signals {
            signals.extend([libc::SIGINT, libc::SIGTERM, libc::SIGHUP]);
        }
        if self.forward_resize {
            signals.push(libc::SIGWINCH);
        }
        signals
    }
}

#[cfg(test)]
pub struct PayloadSession<S> {
    stream: S,
}

#[cfg(test)]
impl<S: Read + Write> PayloadSession<S> {
    pub fn from_stream(
        mut stream: S,
        request: &PayloadRequest,
    ) -> Result<Self, PayloadClientError> {
        let request_json = serde_json::to_vec(request)?;
        send_frame(&mut stream, b'R', &request_json)?;
        Ok(Self { stream })
    }

    pub fn send_input(&mut self, input: &[u8]) -> Result<(), PayloadClientError> {
        send_frame(&mut self.stream, b'I', input)?;
        Ok(())
    }

    pub fn send_signal(&mut self, signal: i32) -> Result<(), PayloadClientError> {
        send_signal_frame(&mut self.stream, signal)?;
        Ok(())
    }

    pub fn send_resize(&mut self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        send_resize_frame(&mut self.stream, rows, cols)?;
        Ok(())
    }

    pub fn recv_event(&mut self) -> Result<PayloadEvent, PayloadClientError> {
        payload_event_from_stream(&mut self.stream)
    }
}

#[cfg(test)]
impl PayloadSession<TcpStream> {
    pub fn connect(
        addr: impl ToSocketAddrs,
        request: &PayloadRequest,
    ) -> Result<Self, PayloadClientError> {
        let stream = connect_payload(addr, Duration::from_secs(10))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
        Self::from_stream(stream, request)
    }

    pub fn try_clone_writer(&self) -> Result<PayloadWriter, PayloadClientError> {
        Ok(PayloadWriter {
            stream: self.stream.try_clone()?,
        })
    }
}

#[cfg(test)]
pub struct PayloadWriter {
    stream: TcpStream,
}

#[cfg(test)]
impl PayloadWriter {
    pub fn send_input(&mut self, input: &[u8]) -> Result<(), PayloadClientError> {
        send_frame(&mut self.stream, b'I', input)?;
        Ok(())
    }

    pub fn send_signal(&mut self, signal: i32) -> Result<(), PayloadClientError> {
        send_signal_frame(&mut self.stream, signal)?;
        Ok(())
    }

    pub fn send_resize(&mut self, rows: u16, cols: u16) -> Result<(), PayloadClientError> {
        send_resize_frame(&mut self.stream, rows, cols)?;
        Ok(())
    }
}

#[cfg(test)]
pub fn ping_payload(addr: impl ToSocketAddrs) -> Result<(), PayloadClientError> {
    let mut stream = connect_payload(addr, Duration::from_secs(1))?;
    send_frame(&mut stream, b'P', &[])?;
    let (frame_type, payload) = recv_frame(&mut stream)?;
    if frame_type == b'K' && payload == b"ok" {
        return Ok(());
    }
    Err(PayloadClientError::Protocol(format!(
        "unexpected ping response: frame={frame_type:?} payload={}",
        String::from_utf8_lossy(&payload)
    )))
}

#[cfg(test)]
pub fn run_payload_tcp(
    addr: impl ToSocketAddrs,
    request: &PayloadRequest,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    run_payload_tcp_with_control(
        addr,
        request,
        input,
        output,
        PayloadControlOptions::disabled(),
    )
}

#[cfg(test)]
pub fn run_diagnostic_tcp(
    addr: impl ToSocketAddrs,
    request: &DiagnosticRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    run_diagnostic_tcp_with_deadline(
        addr,
        request,
        output,
        Duration::from_secs(request.timeout_seconds.max(1)),
    )
}

#[cfg(test)]
pub fn run_diagnostic_tcp_with_deadline(
    addr: impl ToSocketAddrs,
    request: &DiagnosticRequest,
    output: &mut impl Write,
    deadline: Duration,
) -> Result<i32, PayloadClientError> {
    let mut stream = connect_payload(addr, Duration::from_secs(10))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(deadline))?;
    stream.set_write_timeout(Some(deadline))?;
    run_diagnostic_io(&mut stream, request, output).map_err(|error| {
        if error.is_timeout() {
            PayloadClientError::DeadlineExceeded
        } else {
            error
        }
    })
}

#[cfg(test)]
pub fn run_payload_tcp_with_control(
    addr: impl ToSocketAddrs,
    request: &PayloadRequest,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
    control: PayloadControlOptions,
) -> Result<i32, PayloadClientError> {
    let mut session = PayloadSession::connect(addr, request)?;
    run_payload_session(&mut session, input, output, control)
}

#[cfg(test)]
fn connect_payload(
    addr: impl ToSocketAddrs,
    timeout: Duration,
) -> Result<TcpStream, PayloadClientError> {
    let mut last_error = None;
    for addr in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(PayloadClientError::Io(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no socket address resolved")
    })))
}

pub fn socket_addr(host: &str, port: u16) -> Result<SocketAddr, PayloadClientError> {
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| PayloadClientError::Address(format!("no socket address for {host}:{port}")))
}

#[cfg(test)]
fn run_payload_session(
    session: &mut PayloadSession<TcpStream>,
    input: Option<Box<dyn Read + Send>>,
    output: &mut impl Write,
    control: PayloadControlOptions,
) -> Result<i32, PayloadClientError> {
    PayloadSessionRunner::new(control)
        .run_tcp_session(session, input, output)?
        .into_exit_code()
}

#[cfg(test)]
fn run_payload_io(
    stream: &mut (impl Read + Write),
    request: &PayloadRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    let mut session = PayloadSession::from_stream(stream, request)?;
    loop {
        match session.recv_event()? {
            PayloadEvent::Output(payload) => {
                output.write_all(&payload)?;
                output.flush()?;
            }
            PayloadEvent::Exit(exit_code) => return Ok(exit_code),
            PayloadEvent::Failure(message) => return Err(PayloadClientError::Protocol(message)),
        }
    }
}

#[cfg(test)]
fn run_diagnostic_io(
    stream: &mut (impl Read + Write),
    request: &DiagnosticRequest,
    output: &mut impl Write,
) -> Result<i32, PayloadClientError> {
    let request_json = serde_json::to_vec(request)?;
    send_frame(stream, b'D', &request_json)?;
    loop {
        match payload_event_from_stream(stream)? {
            PayloadEvent::Output(payload) => {
                output.write_all(&payload)?;
                output.flush()?;
            }
            PayloadEvent::Exit(exit_code) => return Ok(exit_code),
            PayloadEvent::Failure(message) => return Err(PayloadClientError::Protocol(message)),
        }
    }
}

#[cfg(test)]
fn payload_event_from_stream(stream: &mut impl Read) -> Result<PayloadEvent, PayloadClientError> {
    let (frame_type, payload) = recv_frame(stream)?;
    payload_event_from_frame(Frame {
        kind: FrameKind::from_byte(frame_type),
        payload,
    })
    .map_err(PayloadClientError::from)
}

#[cfg(test)]
fn send_signal_frame(writer: &mut impl Write, signal: i32) -> io::Result<()> {
    let payload = signal_payload(signal).map_err(json_io_error)?;
    send_frame(writer, b'S', &payload)
}

#[cfg(test)]
fn send_resize_frame(writer: &mut impl Write, rows: u16, cols: u16) -> io::Result<()> {
    let payload = resize_payload(rows, cols).map_err(json_io_error)?;
    send_frame(writer, b'W', &payload)
}

#[cfg(test)]
fn json_io_error(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
struct SignalForwarder {
    #[cfg(unix)]
    _inner: Option<UnixSignalForwarder>,
}

#[cfg(test)]
impl SignalForwarder {
    fn install(
        control: PayloadControlOptions,
        send_lock: Arc<Mutex<TcpStream>>,
        done: Arc<AtomicBool>,
    ) -> Result<Self, PayloadClientError> {
        if !control.forward_signals && !control.forward_resize {
            return Ok(Self {
                #[cfg(unix)]
                _inner: None,
            });
        }

        #[cfg(unix)]
        {
            return Ok(Self {
                _inner: Some(UnixSignalForwarder::install(control, send_lock, done)?),
            });
        }

        #[cfg(not(unix))]
        {
            let _ = (send_lock, done);
            Err(PayloadClientError::Unsupported(
                "payload signal forwarding is only supported on Unix hosts".to_string(),
            ))
        }
    }
}

#[cfg(all(test, unix))]
struct UnixSignalForwarder {
    read_fd: i32,
    write_fd: i32,
    old_actions: Vec<(i32, libc::sigaction)>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(all(test, unix))]
impl UnixSignalForwarder {
    fn install(
        control: PayloadControlOptions,
        send_lock: Arc<Mutex<TcpStream>>,
        done: Arc<AtomicBool>,
    ) -> Result<Self, PayloadClientError> {
        let (read_fd, write_fd) = create_signal_pipe().map_err(PayloadClientError::Io)?;
        SIGNAL_WRITE_FD.store(write_fd, Ordering::SeqCst);

        let mut old_actions = Vec::new();
        for signal in control.policy().installed_unix_signals() {
            let old = install_signal_handler(signal)?;
            old_actions.push((signal, old));
        }

        let policy = control.policy();
        let thread = thread::Builder::new()
            .name("agentvm-payload-signals".to_string())
            .spawn(move || signal_forward_loop(read_fd, send_lock, done, policy))
            .map_err(PayloadClientError::Io)?;

        Ok(Self {
            read_fd,
            write_fd,
            old_actions,
            thread: Some(thread),
        })
    }
}

#[cfg(all(test, unix))]
impl Drop for UnixSignalForwarder {
    fn drop(&mut self) {
        for (signal, old) in &self.old_actions {
            // SAFETY: old action was returned by sigaction for this signal.
            unsafe {
                libc::sigaction(*signal, old, std::ptr::null_mut());
            }
        }
        SIGNAL_WRITE_FD.store(-1, Ordering::SeqCst);
        // SAFETY: fds are owned by this forwarder and closed exactly once here.
        unsafe {
            let _ = libc::close(self.write_fd);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: fd is owned by this forwarder and closed exactly once here.
        unsafe {
            let _ = libc::close(self.read_fd);
        }
    }
}

#[cfg(all(test, unix))]
fn create_signal_pipe() -> io::Result<(i32, i32)> {
    let mut fds = [0; 2];
    // SAFETY: pipe initializes both fd slots on success.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        if let Err(error) = set_signal_pipe_flags(fd) {
            // SAFETY: fds were returned by pipe and are not otherwise owned yet.
            unsafe {
                let _ = libc::close(fds[0]);
                let _ = libc::close(fds[1]);
            }
            return Err(error);
        }
    }
    Ok((fds[0], fds[1]))
}

#[cfg(all(test, unix))]
fn set_signal_pipe_flags(fd: i32) -> io::Result<()> {
    // SAFETY: fcntl is called with a valid fd and flag command.
    let status = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl is called with a valid fd and updated status flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, status | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl is called with a valid fd and flag command.
    let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if fd_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl is called with a valid fd and updated descriptor flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn install_signal_handler(signal: i32) -> Result<libc::sigaction, PayloadClientError> {
    // SAFETY: zeroed sigaction is immediately initialized before use.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = payload_signal_handler as *const () as usize;
    action.sa_flags = 0;
    // SAFETY: sigemptyset initializes the mask field.
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }

    // SAFETY: zeroed sigaction receives the previous handler from sigaction.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: pointers reference valid sigaction structures.
    if unsafe { libc::sigaction(signal, &action, &mut old) } != 0 {
        return Err(PayloadClientError::Io(io::Error::last_os_error()));
    }
    Ok(old)
}

#[cfg(all(test, unix))]
extern "C" fn payload_signal_handler(signal: i32) {
    let fd = SIGNAL_WRITE_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    let bytes = signal.to_ne_bytes();
    // SAFETY: write is async-signal-safe; fd is managed by UnixSignalForwarder.
    unsafe {
        let _ = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
    }
}

#[cfg(all(test, unix))]
fn signal_forward_loop(
    read_fd: i32,
    send_lock: Arc<Mutex<TcpStream>>,
    done: Arc<AtomicBool>,
    policy: PayloadControlPolicy,
) {
    let mut buffer = [0_u8; std::mem::size_of::<i32>()];
    let mut forwarded_interrupts = 0;
    while !done.load(Ordering::SeqCst) {
        // SAFETY: buffer points to valid writable memory for the requested size.
        let read = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                _ => return,
            }
        }
        if read == 0 {
            return;
        }
        if read as usize != buffer.len() {
            continue;
        }
        let signal = i32::from_ne_bytes(buffer);
        let action = policy.host_signal_action(signal, forwarded_interrupts);
        let Ok(mut writer) = send_lock.lock() else {
            return;
        };
        let decision = apply_payload_control_action(
            action,
            &mut *writer,
            &done,
            &mut forwarded_interrupts,
            terminal_size,
        );
        match decision {
            Ok(PayloadControlLoopDecision::Continue) => {}
            Ok(PayloadControlLoopDecision::Stop) => {
                let _ = writer.shutdown(Shutdown::Both);
                return;
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
fn apply_payload_control_action<W, F>(
    action: PayloadControlAction,
    writer: &mut W,
    done: &AtomicBool,
    forwarded_interrupts: &mut usize,
    terminal_size: F,
) -> io::Result<PayloadControlLoopDecision>
where
    W: Write,
    F: FnOnce() -> (u16, u16),
{
    match action {
        PayloadControlAction::ForwardSignal(signal) => {
            send_signal_frame(writer, signal)?;
            #[cfg(unix)]
            if signal == libc::SIGINT {
                *forwarded_interrupts += 1;
            }
            Ok(PayloadControlLoopDecision::Continue)
        }
        PayloadControlAction::ForwardResize => {
            let (rows, cols) = terminal_size();
            send_resize_frame(writer, rows, cols)?;
            Ok(PayloadControlLoopDecision::Continue)
        }
        PayloadControlAction::LocalAbort => {
            done.store(true, Ordering::SeqCst);
            Ok(PayloadControlLoopDecision::Stop)
        }
        PayloadControlAction::Ignore => Ok(PayloadControlLoopDecision::Continue),
    }
}

#[cfg(unix)]
pub fn terminal_size() -> (u16, u16) {
    // SAFETY: zeroed winsize is filled by ioctl on success.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: ioctl writes to a valid winsize pointer.
    let ok = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
    if ok && size.ws_row > 0 && size.ws_col > 0 {
        (size.ws_row, size.ws_col)
    } else {
        (24, 80)
    }
}

#[cfg(not(unix))]
pub fn terminal_size() -> (u16, u16) {
    (24, 80)
}

#[cfg(test)]
fn send_frame(writer: &mut impl Write, frame_type: u8, payload: &[u8]) -> io::Result<()> {
    let encoded =
        encode_frame(FrameKind::from_byte(frame_type), payload).map_err(frame_io_error)?;
    writer.write_all(&encoded)
}

#[cfg(test)]
fn recv_frame(reader: &mut impl Read) -> Result<(u8, Vec<u8>), PayloadClientError> {
    let mut header = [0; FRAME_HEADER_LEN];
    reader.read_exact(&mut header)?;
    let (kind, len) = decode_header(&header).map_err(frame_protocol_error)?;
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    Ok((kind.as_byte(), payload))
}

#[cfg(test)]
fn frame_io_error(error: FrameError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error)
}

fn frame_protocol_error(error: FrameError) -> PayloadClientError {
    PayloadClientError::Protocol(error.to_string())
}

impl From<io::Error> for PayloadClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PayloadClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<AsyncFrameError> for PayloadClientError {
    fn from(error: AsyncFrameError) -> Self {
        match error {
            AsyncFrameError::Io(error) => Self::Io(error),
            AsyncFrameError::Frame(error) => Self::Protocol(error.to_string()),
        }
    }
}

impl From<PayloadEventError> for PayloadClientError {
    fn from(error: PayloadEventError) -> Self {
        match error {
            PayloadEventError::Json(error) => Self::Json(error),
            PayloadEventError::UnexpectedFrameKind(kind) => Self::Protocol(format!(
                "unexpected payload frame type {:?}",
                kind.as_byte()
            )),
        }
    }
}

#[cfg(test)]
impl PayloadClientError {
    fn is_timeout(&self) -> bool {
        matches!(
            self,
            PayloadClientError::Io(error)
                if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
        )
    }
}

impl std::fmt::Display for PayloadClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadClientError::Io(error) => write!(f, "payload client IO failed: {error}"),
            PayloadClientError::Json(error) => write!(f, "payload client JSON failed: {error}"),
            PayloadClientError::Protocol(error) => write!(f, "payload protocol failed: {error}"),
            PayloadClientError::Address(error) => write!(f, "{error}"),
            PayloadClientError::Unsupported(error) => write!(f, "{error}"),
            PayloadClientError::DeadlineExceeded => {
                write!(f, "payload diagnostic deadline exceeded")
            }
            PayloadClientError::Cancelled => write!(f, "payload session cancelled"),
        }
    }
}

impl std::error::Error for PayloadClientError {}

#[cfg(test)]
mod tests {
    use super::*;
    use agentvm_payload_protocol::MAX_FRAME_PAYLOAD;
    use proptest::prelude::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    #[cfg(unix)]
    static SIGNAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            max_shrink_iters: 32,
            ..ProptestConfig::default()
        })]

        #[test]
        fn proptest_payload_frames_round_trip(frame_type in any::<u8>(), payload in prop::collection::vec(any::<u8>(), 0..=1024)) {
            let mut bytes = Vec::new();
            send_frame(&mut bytes, frame_type, &payload).expect("send");
            let (decoded_type, decoded_payload) = recv_frame(&mut io::Cursor::new(bytes)).expect("recv");

            prop_assert_eq!(decoded_type, frame_type);
            prop_assert_eq!(decoded_payload, payload);
        }

        #[test]
        fn proptest_arbitrary_payload_frame_bytes_stay_bounded(bytes in prop::collection::vec(any::<u8>(), 0..=128)) {
            let _ = recv_frame(&mut io::Cursor::new(bytes));
        }
    }

    #[cfg(unix)]
    #[test]
    fn interactive_control_policy_defines_signal_resize_and_abort_actions() {
        let policy = PayloadControlOptions::interactive().policy();

        assert_eq!(
            policy.installed_unix_signals(),
            vec![libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGWINCH]
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGINT, 0),
            PayloadControlAction::ForwardSignal(libc::SIGINT)
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGINT, 1),
            PayloadControlAction::LocalAbort
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGTERM, 0),
            PayloadControlAction::LocalAbort
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGHUP, 0),
            PayloadControlAction::ForwardSignal(libc::SIGHUP)
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGWINCH, 0),
            PayloadControlAction::ForwardResize
        );
        assert_eq!(
            policy.tui_ctrl_c_action(0),
            PayloadControlAction::ForwardSignal(libc::SIGINT)
        );
        assert_eq!(
            policy.tui_ctrl_c_action(1),
            PayloadControlAction::LocalAbort
        );
    }

    #[cfg(unix)]
    #[test]
    fn disabled_control_policy_ignores_process_signals_and_tui_interrupts() {
        let policy = PayloadControlOptions::disabled().policy();

        assert!(policy.installed_unix_signals().is_empty());
        assert_eq!(
            policy.host_signal_action(libc::SIGINT, 0),
            PayloadControlAction::Ignore
        );
        assert_eq!(
            policy.host_signal_action(libc::SIGWINCH, 0),
            PayloadControlAction::Ignore
        );
        assert_eq!(policy.tui_ctrl_c_action(0), PayloadControlAction::Ignore);
    }

    #[cfg(unix)]
    #[test]
    fn signal_pipe_is_nonblocking_and_close_on_exec() {
        let (read_fd, write_fd) = create_signal_pipe().expect("signal pipe");
        for fd in [read_fd, write_fd] {
            // SAFETY: fd is open for the duration of this assertion.
            let status = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(status >= 0);
            assert_ne!(status & libc::O_NONBLOCK, 0);
            // SAFETY: fd is open for the duration of this assertion.
            let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(fd_flags >= 0);
            assert_ne!(fd_flags & libc::FD_CLOEXEC, 0);
        }
        // SAFETY: fds are owned by this test and closed exactly once here.
        unsafe {
            let _ = libc::close(write_fd);
            let _ = libc::close(read_fd);
        }
    }

    #[cfg(unix)]
    #[test]
    fn signal_handler_ignores_full_nonblocking_pipe() {
        let _guard = SIGNAL_TEST_LOCK.lock().expect("signal test lock");
        let (read_fd, write_fd) = create_signal_pipe().expect("signal pipe");
        let previous = SIGNAL_WRITE_FD.swap(write_fd, Ordering::SeqCst);
        let chunk = [0_u8; 4096];
        loop {
            // SAFETY: write_fd is nonblocking and chunk points to valid memory.
            let written = unsafe { libc::write(write_fd, chunk.as_ptr().cast(), chunk.len()) };
            if written < 0 {
                let error = io::Error::last_os_error();
                let raw = error.raw_os_error();
                assert!(
                    raw == Some(libc::EAGAIN) || raw == Some(libc::EWOULDBLOCK),
                    "expected EAGAIN/EWOULDBLOCK, got {error}"
                );
                break;
            }
            assert!(written > 0);
        }

        payload_signal_handler(libc::SIGINT);

        SIGNAL_WRITE_FD.store(previous, Ordering::SeqCst);
        // SAFETY: fds are owned by this test and closed exactly once here.
        unsafe {
            let _ = libc::close(write_fd);
            let _ = libc::close(read_fd);
        }
    }

    #[cfg(unix)]
    #[test]
    fn signal_forward_loop_forwards_then_aborts_on_repeated_interrupt() {
        let _guard = SIGNAL_TEST_LOCK.lock().expect("signal test lock");
        let (read_fd, write_fd) = create_signal_pipe().expect("signal pipe");
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let client =
            TcpStream::connect(listener.local_addr().expect("listener addr")).expect("client");
        let (mut server, _) = listener.accept().expect("accept");
        let done = Arc::new(AtomicBool::new(false));
        let send_lock = Arc::new(Mutex::new(client));
        let done_for_thread = done.clone();
        let send_lock_for_thread = send_lock.clone();
        let thread = thread::spawn(move || {
            signal_forward_loop(
                read_fd,
                send_lock_for_thread,
                done_for_thread,
                PayloadControlPolicy::interactive(),
            )
        });

        write_signal_number(write_fd, libc::SIGINT).expect("first sigint");
        let (frame_type, payload) = recv_frame(&mut server).expect("forwarded signal frame");
        assert_eq!(frame_type, b'S');
        let signal: serde_json::Value = serde_json::from_slice(&payload).expect("signal json");
        assert_eq!(signal["signal"], libc::SIGINT);

        write_signal_number(write_fd, libc::SIGINT).expect("second sigint");
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut eof = [0_u8; 1];
        assert_eq!(server.read(&mut eof).expect("stream shutdown"), 0);
        assert!(done.load(Ordering::SeqCst));

        // SAFETY: fd is owned by this test and closed exactly once here.
        unsafe {
            let _ = libc::close(write_fd);
        }
        thread.join().expect("signal loop exits");
    }

    #[cfg(unix)]
    #[test]
    fn signal_forwarder_terminates_blocked_payload_receive_on_sigterm() {
        let _guard = SIGNAL_TEST_LOCK.lock().expect("signal test lock");
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let (request_seen_tx, request_seen_rx) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (mut server, _) = listener.accept().expect("accept");
            let (_frame_type, _payload) = recv_frame(&mut server).expect("request frame");
            request_seen_tx.send(()).expect("request seen");
            let mut probe = [0_u8; 1];
            let _ = server.read(&mut probe);
        });
        let signal_thread = thread::spawn(move || {
            request_seen_rx.recv().expect("request seen");
            wait_for_signal_forwarder_installed().expect("signal forwarder installed");
            payload_signal_handler(libc::SIGTERM);
        });

        let request = PayloadRequest::new("sleep forever");
        let mut output = Vec::new();
        let result = run_payload_tcp_with_control(
            addr,
            &request,
            None,
            &mut output,
            PayloadControlOptions::interactive(),
        );

        assert!(
            matches!(result, Err(PayloadClientError::Cancelled)),
            "{result:?}"
        );
        assert!(output.is_empty());
        signal_thread.join().expect("signal thread");
        server_thread.join().expect("server thread");
    }

    #[test]
    fn payload_session_runner_returns_structured_failure_outcome() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server_thread = thread::spawn(move || {
            let (mut server, _) = listener.accept().expect("accept");
            let (_frame_type, _payload) = recv_frame(&mut server).expect("request frame");
            send_frame(&mut server, b'O', b"before failure\n").expect("output");
            send_frame(&mut server, b'F', b"guest failed").expect("failure");
        });

        let request = PayloadRequest::new("fail");
        let mut session = PayloadSession::connect(addr, &request).expect("session");
        let runner = PayloadSessionRunner::new(PayloadControlOptions::disabled());
        let mut output = Vec::new();
        let outcome = runner
            .run_tcp_session(&mut session, None, &mut output)
            .expect("runner outcome");

        assert_eq!(
            outcome,
            PayloadSessionOutcome::Failure("guest failed".to_string())
        );
        assert_eq!(output, b"before failure\n");
        server_thread.join().expect("server thread");
    }

    #[test]
    fn payload_session_runner_cancel_token_interrupts_blocked_receive() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let (request_seen_tx, request_seen_rx) = mpsc::channel();
        let server_thread = thread::spawn(move || {
            let (mut server, _) = listener.accept().expect("accept");
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("server read timeout");
            let (_frame_type, _payload) = recv_frame(&mut server).expect("request frame");
            request_seen_tx.send(()).expect("request seen");
            let mut probe = [0_u8; 1];
            assert_eq!(server.read(&mut probe).expect("client closed"), 0);
        });

        let request = PayloadRequest::new("sleep forever");
        let mut session = PayloadSession::connect(addr, &request).expect("session");
        let cancel = PayloadCancelToken::new();
        let cancel_for_thread = cancel.clone();
        let runner =
            PayloadSessionRunner::with_cancel_token(PayloadControlOptions::disabled(), cancel);
        let cancel_thread = thread::spawn(move || {
            request_seen_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("request seen");
            cancel_for_thread.cancel();
        });
        let mut output = Vec::new();
        let outcome = runner
            .run_tcp_session(&mut session, None, &mut output)
            .expect("runner outcome");

        assert_eq!(outcome, PayloadSessionOutcome::Cancelled);
        assert!(output.is_empty());
        cancel_thread.join().expect("cancel thread");
        server_thread.join().expect("server thread");
    }

    #[test]
    fn payload_session_runner_reports_partial_frame_io_error() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server_thread = thread::spawn(move || {
            let (mut server, _) = listener.accept().expect("accept");
            let (_frame_type, _payload) = recv_frame(&mut server).expect("request frame");
            server
                .write_all(&[b'O', 0, 0, 0, 4, b'a', b'b'])
                .expect("partial output frame");
        });

        let request = PayloadRequest::new("partial");
        let mut session = PayloadSession::connect(addr, &request).expect("session");
        let runner = PayloadSessionRunner::new(PayloadControlOptions::disabled());
        let mut output = Vec::new();
        let result = runner.run_tcp_session(&mut session, None, &mut output);

        assert!(
            matches!(result, Err(PayloadClientError::Io(_))),
            "{result:?}"
        );
        assert!(output.is_empty());
        server_thread.join().expect("server thread");
    }

    #[cfg(unix)]
    fn wait_for_signal_forwarder_installed() -> io::Result<()> {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if SIGNAL_WRITE_FD.load(Ordering::SeqCst) >= 0 {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "signal forwarder was not installed",
        ))
    }

    #[cfg(unix)]
    fn write_signal_number(fd: i32, signal: i32) -> io::Result<()> {
        let bytes = signal.to_ne_bytes();
        // SAFETY: fd is an open nonblocking pipe and bytes points to valid memory.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        assert_eq!(written as usize, bytes.len());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn control_action_application_forwards_resizes_and_sets_local_abort() {
        let done = AtomicBool::new(false);
        let mut forwarded_interrupts = 0;
        let mut bytes = Vec::new();

        assert_eq!(
            apply_payload_control_action(
                PayloadControlAction::ForwardSignal(libc::SIGINT),
                &mut bytes,
                &done,
                &mut forwarded_interrupts,
                || (40, 120),
            )
            .expect("signal"),
            PayloadControlLoopDecision::Continue
        );
        assert_eq!(forwarded_interrupts, 1);
        let (frame_type, payload) =
            recv_frame(&mut io::Cursor::new(bytes.clone())).expect("signal frame");
        assert_eq!(frame_type, b'S');
        let signal: serde_json::Value = serde_json::from_slice(&payload).expect("signal json");
        assert_eq!(signal["signal"], libc::SIGINT);

        bytes.clear();
        assert_eq!(
            apply_payload_control_action(
                PayloadControlAction::ForwardResize,
                &mut bytes,
                &done,
                &mut forwarded_interrupts,
                || (40, 120),
            )
            .expect("resize"),
            PayloadControlLoopDecision::Continue
        );
        let (frame_type, payload) = recv_frame(&mut io::Cursor::new(bytes)).expect("resize frame");
        assert_eq!(frame_type, b'W');
        let resize: serde_json::Value = serde_json::from_slice(&payload).expect("resize json");
        assert_eq!(resize["rows"], 40);
        assert_eq!(resize["cols"], 120);

        let mut ignored = Vec::new();
        assert_eq!(
            apply_payload_control_action(
                PayloadControlAction::LocalAbort,
                &mut ignored,
                &done,
                &mut forwarded_interrupts,
                || (24, 80),
            )
            .expect("abort"),
            PayloadControlLoopDecision::Stop
        );
        assert!(done.load(Ordering::SeqCst));
        assert!(ignored.is_empty());
    }

    #[test]
    fn frame_round_trip_uses_big_endian_length() {
        let mut bytes = Vec::new();

        send_frame(&mut bytes, b'P', b"hello").expect("send");

        assert_eq!(&bytes[..5], &[b'P', 0, 0, 0, 5]);
        let (frame_type, payload) = recv_frame(&mut io::Cursor::new(bytes)).expect("recv");
        assert_eq!(frame_type, b'P');
        assert_eq!(payload, b"hello");
    }

    #[test]
    fn run_payload_sends_request_streams_output_and_returns_exit_code() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"hello\r\n").expect("output");
        send_frame(&mut server_frames, b'X', br#"{"exit_code":7}"#).expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let request = PayloadRequest::new("echo hello");
        let mut output = Vec::new();
        let exit_code = run_payload_io(&mut stream, &request, &mut output).expect("payload run");

        assert_eq!(exit_code, 7);
        assert_eq!(output, b"hello\r\n");
        let (frame_type, payload) =
            recv_frame(&mut io::Cursor::new(stream.written)).expect("request frame");
        assert_eq!(frame_type, b'R');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "echo hello");
    }

    #[test]
    fn run_diagnostic_sends_diagnostic_request_and_returns_exit_code() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"diag\n").expect("output");
        send_frame(
            &mut server_frames,
            b'X',
            br#"{"exit_code":3,"diagnostic":true}"#,
        )
        .expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let mut request = DiagnosticRequest::new("echo diag");
        request.timeout_seconds = 2;
        request.max_output_bytes = 4096;
        let mut output = Vec::new();
        let exit_code =
            run_diagnostic_io(&mut stream, &request, &mut output).expect("diagnostic run");

        assert_eq!(exit_code, 3);
        assert_eq!(output, b"diag\n");
        let (frame_type, payload) =
            recv_frame(&mut io::Cursor::new(stream.written)).expect("request frame");
        assert_eq!(frame_type, b'D');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "echo diag");
        assert_eq!(request["timeout_seconds"], 2);
        assert_eq!(request["max_output_bytes"], 4096);
    }

    #[test]
    fn run_diagnostic_tcp_with_deadline_fails_partial_frame_stall() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let server_thread = thread::spawn(move || {
            let (mut server, _) = listener.accept().expect("accept");
            let (_frame_type, _payload) = recv_frame(&mut server).expect("request frame");
            server
                .write_all(&[b'O', 0, 0, 0, 4, b'a', b'b'])
                .expect("partial output frame");
            let mut probe = [0_u8; 1];
            let _ = server.read(&mut probe);
        });

        let request = DiagnosticRequest::new("partial diagnostic");
        let mut output = Vec::new();
        let result = run_diagnostic_tcp_with_deadline(
            addr,
            &request,
            &mut output,
            Duration::from_millis(50),
        );

        assert!(
            matches!(result, Err(PayloadClientError::DeadlineExceeded)),
            "{result:?}"
        );
        assert!(output.is_empty());
        server_thread.join().expect("server thread");
    }

    #[test]
    fn payload_session_sends_control_frames_and_receives_events() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'O', b"ready").expect("output");
        send_frame(&mut server_frames, b'X', br#"{"exit_code":0}"#).expect("exit");
        let mut stream = ScriptedIo::new(server_frames);

        let request = PayloadRequest::new("agent");
        {
            let mut session = PayloadSession::from_stream(&mut stream, &request).expect("session");
            session.send_input(b"hello").expect("input");
            session.send_resize(33, 101).expect("resize");
            session.send_signal(2).expect("signal");

            assert_eq!(
                session.recv_event().expect("output"),
                PayloadEvent::Output(b"ready".to_vec())
            );
            assert_eq!(session.recv_event().expect("exit"), PayloadEvent::Exit(0));
        }

        let mut written = io::Cursor::new(stream.written);
        let (frame_type, payload) = recv_frame(&mut written).expect("request frame");
        assert_eq!(frame_type, b'R');
        let request: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        assert_eq!(request["script"], "agent");
        let (frame_type, payload) = recv_frame(&mut written).expect("input frame");
        assert_eq!(frame_type, b'I');
        assert_eq!(payload, b"hello");
        let (frame_type, payload) = recv_frame(&mut written).expect("resize frame");
        assert_eq!(frame_type, b'W');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("resize json"),
            serde_json::json!({ "rows": 33, "cols": 101 })
        );
        let (frame_type, payload) = recv_frame(&mut written).expect("signal frame");
        assert_eq!(frame_type, b'S');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("signal json"),
            serde_json::json!({ "signal": 2 })
        );
    }

    #[test]
    fn payload_session_surfaces_failure_events() {
        let mut server_frames = Vec::new();
        send_frame(&mut server_frames, b'F', b"payload failed").expect("failure");
        let mut stream = ScriptedIo::new(server_frames);

        let mut session = PayloadSession::from_stream(&mut stream, &PayloadRequest::new("agent"))
            .expect("session");

        assert_eq!(
            session.recv_event().expect("failure event"),
            PayloadEvent::Failure("payload failed".to_string())
        );
    }

    #[tokio::test]
    async fn async_payload_session_sends_commands_and_receives_events() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut session = AsyncPayloadSession::from_stream(client, &PayloadRequest::new("async"))
            .await
            .expect("async session");

        let request = read_frame_async(&mut server).await.expect("request frame");
        assert_eq!(request.kind, FrameKind::RUN_PRIMARY);
        let request_json: serde_json::Value =
            serde_json::from_slice(&request.payload).expect("request json");
        assert_eq!(request_json["script"], "async");

        let command_sender = session.command_sender();
        command_sender
            .send_input(b"hello".to_vec())
            .await
            .expect("input");
        command_sender.send_resize(44, 120).await.expect("resize");
        command_sender.send_signal(2).await.expect("signal");

        let input = read_frame_async(&mut server).await.expect("input frame");
        assert_eq!(input.kind, FrameKind::INPUT);
        assert_eq!(input.payload, b"hello");
        let resize = read_frame_async(&mut server).await.expect("resize frame");
        assert_eq!(resize.kind, FrameKind::RESIZE);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&resize.payload).expect("resize json"),
            serde_json::json!({ "rows": 44, "cols": 120 })
        );
        let signal = read_frame_async(&mut server).await.expect("signal frame");
        assert_eq!(signal.kind, FrameKind::SIGNAL);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&signal.payload).expect("signal json"),
            serde_json::json!({ "signal": 2 })
        );

        write_frame_async(
            &mut server,
            &Frame::new(FrameKind::OUTPUT, b"ready".to_vec()).expect("output frame"),
        )
        .await
        .expect("write output");
        write_frame_async(
            &mut server,
            &Frame::new(FrameKind::EXIT, br#"{"exit_code":5}"#.to_vec()).expect("exit frame"),
        )
        .await
        .expect("write exit");

        assert_eq!(
            session.recv_event().await.expect("output event"),
            PayloadEvent::Output(b"ready".to_vec())
        );
        assert_eq!(
            session.recv_event().await.expect("exit event"),
            PayloadEvent::Exit(5)
        );
    }

    #[tokio::test]
    async fn async_payload_runner_forwards_input_output_and_exit() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (client, mut server) = tokio::io::duplex(4096);
        let (mut input_writer, input_reader) = tokio::io::duplex(4096);
        let (mut output_writer, mut output_reader) = tokio::io::duplex(4096);
        let mut session = AsyncPayloadSession::from_stream(client, &PayloadRequest::new("async"))
            .await
            .expect("async session");
        input_writer.write_all(b"stdin").await.expect("input write");
        drop(input_writer);

        let client_fut = run_payload_session_async(
            &mut session,
            Some(Box::new(input_reader) as Box<dyn AsyncRead + Send + Unpin>),
            &mut output_writer,
            PayloadControlOptions::disabled(),
        );
        let server_fut = async {
            let request = read_frame_async(&mut server).await.expect("request frame");
            assert_eq!(request.kind, FrameKind::RUN_PRIMARY);
            let input = read_frame_async(&mut server).await.expect("input frame");
            assert_eq!(input.kind, FrameKind::INPUT);
            assert_eq!(input.payload, b"stdin");
            write_frame_async(
                &mut server,
                &Frame::new(FrameKind::OUTPUT, b"stdout".to_vec()).expect("output frame"),
            )
            .await
            .expect("write output");
            write_frame_async(
                &mut server,
                &Frame::new(FrameKind::EXIT, br#"{"exit_code":3}"#.to_vec()).expect("exit frame"),
            )
            .await
            .expect("write exit");
        };

        let (outcome, ()) = tokio::join!(client_fut, server_fut);
        assert_eq!(
            outcome.expect("payload outcome"),
            PayloadSessionOutcome::Exit(3)
        );
        drop(output_writer);
        let mut output = Vec::new();
        output_reader
            .read_to_end(&mut output)
            .await
            .expect("read output");
        assert_eq!(output, b"stdout");
    }

    #[tokio::test]
    async fn async_payload_session_reports_partial_frame_io_error() {
        use tokio::io::AsyncWriteExt;

        let (client, mut server) = tokio::io::duplex(4096);
        let mut session = AsyncPayloadSession::from_stream(client, &PayloadRequest::new("partial"))
            .await
            .expect("async session");
        let _request = read_frame_async(&mut server).await.expect("request frame");

        server
            .write_all(&[FrameKind::OUTPUT.as_byte(), 0, 0, 0, 4, b'a', b'b'])
            .await
            .expect("partial output frame");
        drop(server);

        let error = session.recv_event().await.expect_err("partial frame error");
        assert!(
            matches!(&error, PayloadClientError::Io(io_error) if io_error.kind() == io::ErrorKind::UnexpectedEof),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn async_payload_session_cancel_stops_event_and_command_paths() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut session = AsyncPayloadSession::from_stream(client, &PayloadRequest::new("cancel"))
            .await
            .expect("async session");
        let _request = read_frame_async(&mut server).await.expect("request frame");

        session.cancel();
        assert!(matches!(
            session.recv_event().await,
            Err(PayloadClientError::Cancelled)
        ));
        assert!(matches!(
            session.send_input(b"ignored".to_vec()).await,
            Err(PayloadClientError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn async_payload_command_sender_try_send_reports_backpressure_and_closed() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let sender = AsyncPayloadCommandSender { tx };

        sender
            .try_send_input(b"queued".to_vec())
            .expect("first command");
        let error = sender.try_send_signal(2).expect_err("full command channel");
        assert!(
            matches!(&error, PayloadClientError::Io(io_error) if io_error.kind() == io::ErrorKind::WouldBlock),
            "{error:?}"
        );
        drop(rx);
        assert!(matches!(
            sender.try_send_resize(24, 80),
            Err(PayloadClientError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn async_ping_uses_protocol_frames() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let client_fut = ping_payload_async_io(&mut client);
        let server_fut = async {
            let request = read_frame_async(&mut server).await.expect("ping frame");
            assert_eq!(request.kind, FrameKind::PING);
            assert!(request.payload.is_empty());
            write_frame_async(
                &mut server,
                &Frame::new(FrameKind::OK, b"ok".to_vec()).expect("ok frame"),
            )
            .await
            .expect("write ping response");
        };

        let (client_result, ()) = tokio::join!(client_fut, server_fut);
        client_result.expect("ping ok");
    }

    #[tokio::test]
    async fn async_diagnostic_streams_output_and_enforces_deadline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut client, mut server) = tokio::io::duplex(4096);
        let (mut output_writer, mut output_reader) = tokio::io::duplex(4096);
        let mut request = DiagnosticRequest::new("diag");
        request.timeout_seconds = 1;
        request.max_output_bytes = 64;

        let client_fut = run_diagnostic_async_io(
            &mut client,
            &request,
            &mut output_writer,
            Duration::from_secs(1),
        );
        let server_fut = async {
            let request = read_frame_async(&mut server)
                .await
                .expect("diagnostic frame");
            assert_eq!(request.kind, FrameKind::RUN_DIAGNOSTIC);
            let request_json: serde_json::Value =
                serde_json::from_slice(&request.payload).expect("request json");
            assert_eq!(request_json["script"], "diag");
            write_frame_async(
                &mut server,
                &Frame::new(FrameKind::OUTPUT, b"hello".to_vec()).expect("output frame"),
            )
            .await
            .expect("write output");
            write_frame_async(
                &mut server,
                &Frame::new(FrameKind::EXIT, br#"{"exit_code":9}"#.to_vec()).expect("exit frame"),
            )
            .await
            .expect("write exit");
        };

        let (client_result, ()) = tokio::join!(client_fut, server_fut);
        assert_eq!(client_result.expect("diagnostic exit"), 9);
        drop(output_writer);
        let mut output = Vec::new();
        output_reader
            .read_to_end(&mut output)
            .await
            .expect("read output");
        assert_eq!(output, b"hello");

        let (mut client, mut server) = tokio::io::duplex(4096);
        let (mut output_writer, _output_reader) = tokio::io::duplex(4096);
        let partial_request = DiagnosticRequest::new("partial");
        let timeout_fut = run_diagnostic_async_io(
            &mut client,
            &partial_request,
            &mut output_writer,
            Duration::from_millis(25),
        );
        let partial_server_fut = async {
            let _request = read_frame_async(&mut server)
                .await
                .expect("diagnostic frame");
            server
                .write_all(&[FrameKind::OUTPUT.as_byte(), 0, 0, 0, 4, b'a', b'b'])
                .await
                .expect("partial frame");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let (timeout_result, ()) = tokio::join!(timeout_fut, partial_server_fut);
        assert!(matches!(
            timeout_result,
            Err(PayloadClientError::DeadlineExceeded)
        ));
    }

    #[test]
    fn signal_and_resize_frames_match_guest_protocol() {
        let mut bytes = Vec::new();

        send_signal_frame(&mut bytes, 2).expect("signal");
        send_resize_frame(&mut bytes, 40, 120).expect("resize");

        let mut cursor = io::Cursor::new(bytes);
        let (frame_type, payload) = recv_frame(&mut cursor).expect("signal frame");
        assert_eq!(frame_type, b'S');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("signal json"),
            serde_json::json!({ "signal": 2 })
        );
        let (frame_type, payload) = recv_frame(&mut cursor).expect("resize frame");
        assert_eq!(frame_type, b'W');
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload).expect("resize json"),
            serde_json::json!({ "rows": 40, "cols": 120 })
        );
    }

    #[test]
    fn socket_addr_resolves_host_and_port() {
        let addr = socket_addr("127.0.0.1", 0).expect("loopback");
        assert_eq!(addr.port(), 0);
    }

    #[test]
    fn frame_length_limit_rejects_oversized_send_and_receive() {
        let payload = vec![0; MAX_FRAME_PAYLOAD + 1];
        let error = send_frame(&mut Vec::new(), b'I', &payload).expect_err("oversized send");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("payload frame too large"));

        let mut bytes = vec![b'O'];
        bytes.extend_from_slice(&((MAX_FRAME_PAYLOAD as u32) + 1).to_be_bytes());
        let error = recv_frame(&mut io::Cursor::new(bytes)).expect_err("oversized receive");
        assert!(
            matches!(error, PayloadClientError::Protocol(message) if message.contains("payload frame too large"))
        );
    }

    #[test]
    fn truncated_frame_reports_io_without_allocating_payload() {
        let mut bytes = vec![b'O'];
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(b"ab");

        let error = recv_frame(&mut io::Cursor::new(bytes)).expect_err("truncated");
        assert!(
            matches!(error, PayloadClientError::Io(io_error) if io_error.kind() == io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn exit_code_payload_is_required_json() {
        let error = payload_event_from_frame(Frame {
            kind: FrameKind::EXIT,
            payload: b"not-json".to_vec(),
        })
        .map_err(PayloadClientError::from)
        .expect_err("invalid");
        assert!(matches!(error, PayloadClientError::Json(_)));
    }

    #[derive(Debug)]
    struct ScriptedIo {
        read: io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl ScriptedIo {
        fn new(read: Vec<u8>) -> Self {
            Self {
                read: io::Cursor::new(read),
                written: Vec::new(),
            }
        }
    }

    impl Read for ScriptedIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.read.read(buf)
        }
    }

    impl Write for ScriptedIo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
