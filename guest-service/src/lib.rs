//! Opt-in Rust/Tokio implementation pieces for the AgentVM guest payload service.
//!
//! This is the production AgentVM guest payload service.
//! This crate is built up behind the opt-in Rust guest-service path and shares
//! the frame protocol with the frontend and Python service.

use std::fs::File;
use std::io::{self, Read as _};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use agentvm_payload_protocol::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::process::Command;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time;

pub const DEFAULT_INITIAL_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_SESSION_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_CLIENTS: usize = 16;
pub const DEFAULT_MAX_DIAGNOSTIC_SESSIONS: usize = 4;
pub const DEFAULT_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_DIAGNOSTIC_OUTPUT_BYTES: u64 = 1024 * 1024;
pub const MAX_DIAGNOSTIC_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
pub const DEFAULT_DOCKER_BRIDGE_MAX_SESSIONS: usize = 64;
pub const DEFAULT_DOCKER_BRIDGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_DOCKER_BRIDGE_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const DOCKER_BRIDGE_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestServiceLimits {
    pub max_clients: usize,
    pub max_diagnostics: usize,
    pub initial_timeout: Duration,
    pub io_timeout: Duration,
}

impl Default for GuestServiceLimits {
    fn default() -> Self {
        Self {
            max_clients: DEFAULT_MAX_CLIENTS,
            max_diagnostics: DEFAULT_MAX_DIAGNOSTIC_SESSIONS,
            initial_timeout: DEFAULT_INITIAL_FRAME_TIMEOUT,
            io_timeout: DEFAULT_SESSION_IO_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerBridgeLimits {
    pub max_sessions: usize,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
}

impl Default for DockerBridgeLimits {
    fn default() -> Self {
        Self {
            max_sessions: DEFAULT_DOCKER_BRIDGE_MAX_SESSIONS,
            connect_timeout: DEFAULT_DOCKER_BRIDGE_CONNECT_TIMEOUT,
            io_timeout: DEFAULT_DOCKER_BRIDGE_IO_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockerBridgeStats {
    pub client_to_docker: u64,
    pub docker_to_client: u64,
    pub timed_out: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GuestServiceError {
    #[error("payload frame failed: {source}")]
    Frame {
        source: agentvm_payload_protocol::AsyncFrameError,
    },
    #[error("initial payload frame timed out after {timeout:?}")]
    InitialFrameTimeout { timeout: Duration },
    #[error("payload request JSON failed: {source}")]
    PayloadRequestJson { source: serde_json::Error },
    #[error("diagnostic request JSON failed: {source}")]
    DiagnosticRequestJson { source: serde_json::Error },
    #[error("failed to spawn diagnostic process: {source}")]
    SpawnDiagnostic { source: std::io::Error },
    #[error("diagnostic process IO failed: {source}")]
    DiagnosticIo { source: std::io::Error },
    #[error("failed to spawn primary payload process: {source}")]
    SpawnPrimary { source: std::io::Error },
    #[error("primary payload process IO failed: {source}")]
    PrimaryIo { source: std::io::Error },
    #[error("failed to bind TCP listener {addr}: {source}")]
    BindTcp {
        addr: String,
        source: std::io::Error,
    },
    #[error("failed to accept TCP payload client: {source}")]
    AcceptTcp { source: std::io::Error },
    #[error("payload service client limit reached")]
    ClientLimitReached,
    #[error("docker bridge session limit reached")]
    DockerBridgeSessionLimitReached,
    #[error("failed to connect to Docker socket {path}: {source}")]
    DockerBridgeConnect {
        path: String,
        source: std::io::Error,
    },
    #[error("Docker bridge IO failed: {source}")]
    DockerBridgeIo { source: std::io::Error },
}

impl From<agentvm_payload_protocol::AsyncFrameError> for GuestServiceError {
    fn from(source: agentvm_payload_protocol::AsyncFrameError) -> Self {
        Self::Frame { source }
    }
}

pub async fn serve_tcp(
    tcp_host: &str,
    tcp_port: u16,
    limits: GuestServiceLimits,
) -> Result<(), GuestServiceError> {
    let addr = format!("{tcp_host}:{tcp_port}");
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|source| GuestServiceError::BindTcp {
            addr: addr.clone(),
            source,
        })?;
    serve_listener(listener, GuestServiceState::new(limits)).await
}

pub async fn serve_listener(
    listener: TcpListener,
    state: GuestServiceState,
) -> Result<(), GuestServiceError> {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|source| GuestServiceError::AcceptTcp { source })?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = state.handle_client(stream).await {
                eprintln!("agentvm-guest-service: payload client failed: {error}");
            }
        });
    }
}

pub async fn serve_docker_bridge_tcp(
    tcp_host: &str,
    tcp_port: u16,
    docker_sock: impl AsRef<Path>,
    limits: DockerBridgeLimits,
) -> Result<(), GuestServiceError> {
    let addr = format!("{tcp_host}:{tcp_port}");
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|source| GuestServiceError::BindTcp {
            addr: addr.clone(),
            source,
        })?;
    serve_docker_bridge_listener(listener, docker_sock, limits).await
}

pub async fn serve_docker_bridge_listener(
    listener: TcpListener,
    docker_sock: impl AsRef<Path>,
    limits: DockerBridgeLimits,
) -> Result<(), GuestServiceError> {
    let docker_sock = docker_sock.as_ref().to_path_buf();
    let sessions = Arc::new(Semaphore::new(limits.max_sessions));
    loop {
        let (client, _) = listener
            .accept()
            .await
            .map_err(|source| GuestServiceError::AcceptTcp { source })?;
        let Ok(permit) = sessions.clone().try_acquire_owned() else {
            eprintln!(
                "agentvm-guest-service: rejecting Docker bridge client: session limit reached"
            );
            drop(client);
            continue;
        };
        let docker_sock = docker_sock.clone();
        let limits = limits.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match handle_docker_bridge_client(client, &docker_sock, &limits).await {
                Ok(stats) => eprintln!(
                    "agentvm-guest-service: Docker bridge session closed client_to_docker={} docker_to_client={} timed_out={}",
                    stats.client_to_docker, stats.docker_to_client, stats.timed_out
                ),
                Err(error) => eprintln!("agentvm-guest-service: Docker bridge client failed: {error}"),
            }
        });
    }
}

pub async fn handle_docker_bridge_client(
    client: TcpStream,
    docker_sock: impl AsRef<Path>,
    limits: &DockerBridgeLimits,
) -> Result<DockerBridgeStats, GuestServiceError> {
    let docker_sock = docker_sock.as_ref();
    let upstream = connect_docker_bridge_socket(docker_sock, limits.connect_timeout).await?;
    proxy_docker_bridge(client, upstream, limits.io_timeout).await
}

async fn connect_docker_bridge_socket(
    docker_sock: &Path,
    connect_timeout: Duration,
) -> Result<UnixStream, GuestServiceError> {
    let deadline = time::Instant::now() + connect_timeout;
    loop {
        match UnixStream::connect(docker_sock).await {
            Ok(stream) => return Ok(stream),
            Err(source) if time::Instant::now() >= deadline => {
                return Err(GuestServiceError::DockerBridgeConnect {
                    path: docker_sock.display().to_string(),
                    source,
                });
            }
            Err(_) => {}
        }
        time::sleep(DOCKER_BRIDGE_CONNECT_RETRY_INTERVAL).await;
    }
}

async fn proxy_docker_bridge(
    mut client: TcpStream,
    mut docker: UnixStream,
    io_timeout: Duration,
) -> Result<DockerBridgeStats, GuestServiceError> {
    let mut stats = DockerBridgeStats {
        client_to_docker: 0,
        docker_to_client: 0,
        timed_out: false,
    };
    let mut client_read_closed = false;
    let mut docker_read_closed = false;
    let mut client_buf = [0_u8; 64 * 1024];
    let mut docker_buf = [0_u8; 64 * 1024];

    while !client_read_closed || !docker_read_closed {
        let idle_timeout = time::sleep(io_timeout);
        tokio::pin!(idle_timeout);
        tokio::select! {
            read = client.read(&mut client_buf), if !client_read_closed => {
                let read = read.map_err(|source| GuestServiceError::DockerBridgeIo { source })?;
                if read == 0 {
                    client_read_closed = true;
                    let _ = docker.shutdown().await;
                } else {
                    docker.write_all(&client_buf[..read]).await.map_err(|source| GuestServiceError::DockerBridgeIo { source })?;
                    stats.client_to_docker += read as u64;
                }
            }
            read = docker.read(&mut docker_buf), if !docker_read_closed => {
                let read = read.map_err(|source| GuestServiceError::DockerBridgeIo { source })?;
                if read == 0 {
                    docker_read_closed = true;
                    let _ = client.shutdown().await;
                } else {
                    client.write_all(&docker_buf[..read]).await.map_err(|source| GuestServiceError::DockerBridgeIo { source })?;
                    stats.docker_to_client += read as u64;
                }
            }
            _ = &mut idle_timeout => {
                stats.timed_out = true;
                return Ok(stats);
            }
        }
    }

    Ok(stats)
}

#[derive(Debug, Clone)]
pub struct GuestServiceState {
    primary_active: Arc<Mutex<bool>>,
    diagnostics: Arc<Semaphore>,
    clients: Arc<Semaphore>,
    limits: GuestServiceLimits,
}

impl GuestServiceState {
    pub fn new(limits: GuestServiceLimits) -> Self {
        Self {
            primary_active: Arc::new(Mutex::new(false)),
            diagnostics: Arc::new(Semaphore::new(limits.max_diagnostics)),
            clients: Arc::new(Semaphore::new(limits.max_clients)),
            limits,
        }
    }

    pub fn limits(&self) -> &GuestServiceLimits {
        &self.limits
    }

    pub fn try_acquire_client(&self) -> Result<ClientSession, GuestServiceError> {
        let permit = self
            .clients
            .clone()
            .try_acquire_owned()
            .map_err(|_| GuestServiceError::ClientLimitReached)?;
        Ok(ClientSession { _permit: permit })
    }

    pub fn try_start_primary(&self) -> Option<PrimarySession> {
        let mut active = self
            .primary_active
            .lock()
            .expect("primary payload admission mutex poisoned");
        if *active {
            return None;
        }
        *active = true;
        Some(PrimarySession {
            primary_active: Arc::clone(&self.primary_active),
        })
    }

    pub fn try_start_diagnostic(&self) -> Option<DiagnosticSession> {
        self.diagnostics
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| DiagnosticSession { _permit: permit })
    }

    pub async fn handle_client<S>(&self, mut stream: S) -> Result<(), GuestServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let _client = match self.try_acquire_client() {
            Ok(client) => client,
            Err(GuestServiceError::ClientLimitReached) => return Ok(()),
            Err(error) => return Err(error),
        };
        let frame = match time::timeout(
            self.limits.initial_timeout,
            agentvm_payload_protocol::read_frame_async(&mut stream),
        )
        .await
        {
            Ok(Ok(frame)) => frame,
            Ok(Err(error)) => {
                let _ = send_failure(&mut stream, error.to_string()).await;
                return Ok(());
            }
            Err(_) => {
                return Err(GuestServiceError::InitialFrameTimeout {
                    timeout: self.limits.initial_timeout,
                })
            }
        };

        match frame.kind {
            FrameKind::PING => {
                agentvm_payload_protocol::write_frame_async(
                    &mut stream,
                    &Frame::new(FrameKind::OK, b"ok".to_vec()).expect("static ok frame"),
                )
                .await?
            }
            FrameKind::RUN_PRIMARY => {
                self.handle_primary_request(&mut stream, frame.payload)
                    .await?
            }
            FrameKind::RUN_DIAGNOSTIC => {
                self.handle_diagnostic_request(&mut stream, frame.payload)
                    .await?
            }
            _ => send_failure(&mut stream, "unexpected initial frame").await?,
        }
        Ok(())
    }

    async fn handle_primary_request<S>(
        &self,
        stream: &mut S,
        payload: Vec<u8>,
    ) -> Result<(), GuestServiceError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let request: PayloadRequest = serde_json::from_slice(&payload)
            .map_err(|source| GuestServiceError::PayloadRequestJson { source })?;
        if request.script.is_empty() {
            send_failure(stream, "payload request is missing script").await?;
            return Ok(());
        }
        let Some(_session) = self.try_start_primary() else {
            send_failure(stream, "payload session already active").await?;
            return Ok(());
        };
        run_primary_request(stream, request).await
    }

    async fn handle_diagnostic_request<S>(
        &self,
        stream: &mut S,
        payload: Vec<u8>,
    ) -> Result<(), GuestServiceError>
    where
        S: AsyncWrite + Unpin,
    {
        let request: DiagnosticRequest = serde_json::from_slice(&payload)
            .map_err(|source| GuestServiceError::DiagnosticRequestJson { source })?;
        if request.script.is_empty() {
            send_failure(stream, "diagnostic request is missing script").await?;
            return Ok(());
        }
        let Some(_session) = self.try_start_diagnostic() else {
            send_failure(stream, "too many diagnostic sessions active").await?;
            return Ok(());
        };
        run_diagnostic_request(stream, request).await
    }
}

async fn run_primary_request<S>(
    stream: &mut S,
    request: PayloadRequest,
) -> Result<(), GuestServiceError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let identity = payload_identity(&request.env)?;
    ensure_home(&request.env, identity)?;
    let pty = open_primary_pty(request.rows, request.cols)?;
    let resize_fd = pty
        .master
        .try_clone()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    let master_reader = pty
        .master
        .try_clone()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    let slave_stdin = pty
        .slave
        .try_clone()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    let slave_stdout = pty
        .slave
        .try_clone()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    let slave_stderr = pty
        .slave
        .try_clone()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    let slave_fd = pty.slave.as_raw_fd();

    let mut command = Command::new("/bin/sh");
    configure_process(&mut command, identity, Some(slave_fd));
    command
        .arg("-c")
        .arg(&request.script)
        .stdin(Stdio::from(slave_stdin))
        .stdout(Stdio::from(slave_stdout))
        .stderr(Stdio::from(slave_stderr))
        .current_dir(if request.cwd.is_empty() {
            "/"
        } else {
            request.cwd.as_str()
        })
        .envs(request.env.iter());
    let mut child = command
        .spawn()
        .map_err(|source| GuestServiceError::SpawnPrimary { source })?;
    drop(pty.slave);

    let child_pid = child.id();
    let mut control_closed = false;
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(16);
    let master_write = tokio::fs::File::from_std(pty.master);
    let mut primary_io = PrimaryProcessIo {
        input: master_write,
        resize: resize_fd,
    };
    spawn_pty_output_reader(master_reader, output_tx)?;
    let wait_task = tokio::spawn(async move { child.wait().await });
    tokio::pin!(wait_task);
    let (mut reader, mut writer) = tokio::io::split(stream);

    loop {
        tokio::select! {
            output = output_rx.recv() => {
                if let Some(output) = output {
                    if let Err(error) = send_output(&mut writer, &output).await {
                        signal_child_process_group(child_pid, libc::SIGTERM);
                        return Err(error.into());
                    }
                }
            }
            frame = agentvm_payload_protocol::read_frame_async(&mut reader), if !control_closed => {
                match frame {
                    Ok(frame) => handle_primary_control_frame(frame, &mut primary_io, child_pid).await?,
                    Err(_) => {
                        control_closed = true;
                        signal_child_process_group(child_pid, libc::SIGTERM);
                    }
                }
            }
            status = &mut wait_task => {
                let status = status
                    .map_err(|source| GuestServiceError::PrimaryIo { source: std::io::Error::other(source.to_string()) })?
                    .map_err(|source| GuestServiceError::PrimaryIo { source })?;
                drop(primary_io);
                drain_recent_primary_output(&mut output_rx, &mut writer).await?;
                send_exit(&mut writer, exit_code_from_status(status)).await?;
                return Ok(());
            }
        }
    }
}

struct PrimaryPty {
    master: File,
    slave: File,
}

struct PrimaryProcessIo {
    input: tokio::fs::File,
    resize: File,
}

fn open_primary_pty(rows: u16, cols: u16) -> Result<PrimaryPty, GuestServiceError> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut size = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut size,
        )
    };
    if rc == -1 {
        return Err(GuestServiceError::SpawnPrimary {
            source: io::Error::last_os_error(),
        });
    }
    Ok(PrimaryPty {
        master: unsafe { File::from_raw_fd(master) },
        slave: unsafe { File::from_raw_fd(slave) },
    })
}

async fn handle_primary_control_frame(
    frame: Frame,
    process_io: &mut PrimaryProcessIo,
    child_pid: Option<u32>,
) -> Result<(), GuestServiceError> {
    match frame.kind {
        FrameKind::INPUT => {
            process_io
                .input
                .write_all(&frame.payload)
                .await
                .map_err(|source| GuestServiceError::PrimaryIo { source })?;
        }
        FrameKind::SIGNAL => {
            let signal: SignalFrame = serde_json::from_slice(&frame.payload)
                .map_err(|source| GuestServiceError::PayloadRequestJson { source })?;
            signal_child_process_group(child_pid, signal.signal);
        }
        FrameKind::RESIZE => {
            let resize: ResizeFrame = serde_json::from_slice(&frame.payload)
                .map_err(|source| GuestServiceError::PayloadRequestJson { source })?;
            resize_primary_pty(&process_io.resize, resize.rows, resize.cols)?;
        }
        _ => {}
    }
    Ok(())
}

fn resize_primary_pty(pty: &File, rows: u16, cols: u16) -> Result<(), GuestServiceError> {
    let size = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { libc::ioctl(pty.as_raw_fd(), libc::TIOCSWINSZ, &size) };
    if rc == -1 {
        return Err(GuestServiceError::PrimaryIo {
            source: io::Error::last_os_error(),
        });
    }
    Ok(())
}

async fn drain_recent_primary_output<S>(
    output_rx: &mut mpsc::Receiver<Vec<u8>>,
    writer: &mut S,
) -> Result<(), GuestServiceError>
where
    S: AsyncWrite + Unpin,
{
    loop {
        match time::timeout(Duration::from_millis(50), output_rx.recv()).await {
            Ok(Some(output)) => send_output(writer, &output).await?,
            Ok(None) | Err(_) => return Ok(()),
        }
    }
}

fn spawn_pty_output_reader(
    mut reader: File,
    tx: mpsc::Sender<Vec<u8>>,
) -> Result<(), GuestServiceError> {
    std::thread::Builder::new()
        .name("agentvm-primary-pty-output".to_string())
        .spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.blocking_send(buf[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|source| GuestServiceError::PrimaryIo { source })?;
    Ok(())
}

fn signal_child_process_group(child_pid: Option<u32>, signal: i32) {
    let Some(pid) = child_pid else {
        return;
    };
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PayloadIdentity {
    uid: u32,
    gid: u32,
}

fn payload_identity(
    env: &std::collections::BTreeMap<String, String>,
) -> Result<Option<PayloadIdentity>, GuestServiceError> {
    let uid = env.get("AGENTVM_UID");
    let gid = env.get("AGENTVM_GID");
    match (uid, gid) {
        (None, None) => Ok(None),
        (Some(uid), Some(gid)) => {
            let uid =
                uid.parse::<u32>()
                    .map_err(|source| GuestServiceError::PayloadRequestJson {
                        source: serde_json::Error::io(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            source,
                        )),
                    })?;
            let gid =
                gid.parse::<u32>()
                    .map_err(|source| GuestServiceError::PayloadRequestJson {
                        source: serde_json::Error::io(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            source,
                        )),
                    })?;
            Ok(Some(PayloadIdentity { uid, gid }))
        }
        _ => Err(GuestServiceError::PayloadRequestJson {
            source: serde_json::Error::io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "payload identity requires both AGENTVM_UID and AGENTVM_GID",
            )),
        }),
    }
}

fn ensure_home(
    env: &std::collections::BTreeMap<String, String>,
    identity: Option<PayloadIdentity>,
) -> Result<(), GuestServiceError> {
    let Some(home) = env.get("HOME") else {
        return Ok(());
    };
    let home_path = Path::new(home);
    if !home_path.is_absolute() {
        return Ok(());
    }
    std::fs::create_dir_all(home_path).map_err(|source| GuestServiceError::PrimaryIo { source })?;
    let Some(identity) = identity else {
        return Ok(());
    };
    let metadata =
        std::fs::metadata(home_path).map_err(|source| GuestServiceError::PrimaryIo { source })?;
    if metadata.uid() == identity.uid && metadata.gid() == identity.gid {
        return Ok(());
    }
    let path =
        std::ffi::CString::new(home.as_str()).map_err(|source| GuestServiceError::PrimaryIo {
            source: io::Error::new(io::ErrorKind::InvalidInput, source),
        })?;
    let rc = unsafe { libc::chown(path.as_ptr(), identity.uid, identity.gid) };
    if rc == -1 {
        return Err(GuestServiceError::PrimaryIo {
            source: io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn configure_process(
    command: &mut Command,
    identity: Option<PayloadIdentity>,
    controlling_tty: Option<RawFd>,
) {
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if let Some(fd) = controlling_tty {
                if libc::ioctl(fd, libc::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            let Some(identity) = identity else {
                return Ok(());
            };
            if libc::getuid() == identity.uid && libc::getgid() == identity.gid {
                return Ok(());
            }
            let gid = identity.gid as libc::gid_t;
            if libc::setgroups(1, &gid) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::setgid(gid) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::setuid(identity.uid as libc::uid_t) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

async fn run_diagnostic_request<S>(
    stream: &mut S,
    request: DiagnosticRequest,
) -> Result<(), GuestServiceError>
where
    S: AsyncWrite + Unpin,
{
    let timeout = bounded_diagnostic_timeout(request.timeout_seconds);
    let max_output_bytes = bounded_diagnostic_output(request.max_output_bytes);
    let identity = payload_identity(&request.env)?;
    ensure_home(&request.env, identity)?;
    let mut command = Command::new("/bin/sh");
    configure_process(&mut command, identity, None);
    command
        .arg("-c")
        .arg(&request.script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(if request.cwd.is_empty() {
            "/"
        } else {
            request.cwd.as_str()
        })
        .envs(request.env.iter());
    let mut child = command
        .spawn()
        .map_err(|source| GuestServiceError::SpawnDiagnostic { source })?;
    let mut stdout = child.stdout.take().expect("diagnostic stdout is piped");
    let mut stderr = child.stderr.take().expect("diagnostic stderr is piped");
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut sent = 0_u64;
    let mut timed_out = false;
    let mut truncated = false;
    let mut stdout_buf = [0_u8; 8192];
    let mut stderr_buf = [0_u8; 8192];
    let timeout_sleep = time::sleep(timeout);
    tokio::pin!(timeout_sleep);

    loop {
        if stdout_done && stderr_done {
            break;
        }
        tokio::select! {
            _ = &mut timeout_sleep, if !timed_out && !truncated => {
                timed_out = true;
                signal_child_process_group(child.id(), libc::SIGKILL);
                let _ = child.start_kill();
                break;
            }
            read = stdout.read(&mut stdout_buf), if !stdout_done => {
                let read = read.map_err(|source| GuestServiceError::DiagnosticIo { source })?;
                if read == 0 {
                    stdout_done = true;
                } else if write_diagnostic_output(stream, &stdout_buf[..read], &mut sent, max_output_bytes).await? {
                    truncated = true;
                    signal_child_process_group(child.id(), libc::SIGKILL);
                    let _ = child.start_kill();
                    break;
                }
            }
            read = stderr.read(&mut stderr_buf), if !stderr_done => {
                let read = read.map_err(|source| GuestServiceError::DiagnosticIo { source })?;
                if read == 0 {
                    stderr_done = true;
                } else if write_diagnostic_output(stream, &stderr_buf[..read], &mut sent, max_output_bytes).await? {
                    truncated = true;
                    signal_child_process_group(child.id(), libc::SIGKILL);
                    let _ = child.start_kill();
                    break;
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|source| GuestServiceError::DiagnosticIo { source })?;
    let exit_code = if timed_out {
        send_output(stream, b"\nagentvm diagnostic timed out\n").await?;
        124
    } else if truncated {
        send_output(stream, b"\nagentvm diagnostic output limit exceeded\n").await?;
        125
    } else {
        exit_code_from_status(status)
    };
    send_diagnostic_exit(stream, exit_code, timed_out, truncated).await?;
    Ok(())
}

async fn send_exit<S>(
    stream: &mut S,
    exit_code: i32,
) -> Result<(), agentvm_payload_protocol::AsyncFrameError>
where
    S: AsyncWrite + Unpin,
{
    let frame = Frame::new(
        FrameKind::EXIT,
        serde_json::to_vec(&ExitFrame { exit_code }).expect("exit json"),
    )
    .map_err(agentvm_payload_protocol::AsyncFrameError::from)?;
    agentvm_payload_protocol::write_frame_async(stream, &frame).await
}

async fn send_diagnostic_exit<S>(
    stream: &mut S,
    exit_code: i32,
    timed_out: bool,
    truncated: bool,
) -> Result<(), agentvm_payload_protocol::AsyncFrameError>
where
    S: AsyncWrite + Unpin,
{
    let payload = serde_json::json!({
        "exit_code": exit_code,
        "diagnostic": true,
        "timed_out": timed_out,
        "truncated": truncated,
    });
    let frame = Frame::new(
        FrameKind::EXIT,
        serde_json::to_vec(&payload).expect("diagnostic exit json"),
    )
    .map_err(agentvm_payload_protocol::AsyncFrameError::from)?;
    agentvm_payload_protocol::write_frame_async(stream, &frame).await
}

fn bounded_diagnostic_timeout(requested_seconds: u64) -> Duration {
    let requested = if requested_seconds == 0 {
        DEFAULT_DIAGNOSTIC_TIMEOUT
    } else {
        Duration::from_secs(requested_seconds)
    };
    requested.min(MAX_DIAGNOSTIC_TIMEOUT)
}

fn bounded_diagnostic_output(requested_bytes: u64) -> u64 {
    let requested = if requested_bytes == 0 {
        DEFAULT_DIAGNOSTIC_OUTPUT_BYTES
    } else {
        requested_bytes
    };
    requested.min(MAX_DIAGNOSTIC_OUTPUT_BYTES)
}

async fn write_diagnostic_output<S>(
    stream: &mut S,
    chunk: &[u8],
    sent: &mut u64,
    max_output_bytes: u64,
) -> Result<bool, agentvm_payload_protocol::AsyncFrameError>
where
    S: AsyncWrite + Unpin,
{
    let remaining = max_output_bytes.saturating_sub(*sent) as usize;
    let allowed = remaining.min(chunk.len());
    if allowed > 0 {
        send_output(stream, &chunk[..allowed]).await?;
        *sent += allowed as u64;
    }
    Ok(allowed < chunk.len())
}

async fn send_output<S>(
    stream: &mut S,
    payload: &[u8],
) -> Result<(), agentvm_payload_protocol::AsyncFrameError>
where
    S: AsyncWrite + Unpin,
{
    let frame = Frame::new(FrameKind::OUTPUT, payload.to_vec())?;
    agentvm_payload_protocol::write_frame_async(stream, &frame).await
}

fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

pub struct ClientSession {
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug)]
pub struct DiagnosticSession {
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug)]
pub struct PrimarySession {
    primary_active: Arc<Mutex<bool>>,
}

impl Drop for PrimarySession {
    fn drop(&mut self) {
        *self
            .primary_active
            .lock()
            .expect("primary payload admission mutex poisoned") = false;
    }
}

async fn send_failure<S>(
    stream: &mut S,
    message: impl AsRef<str>,
) -> Result<(), agentvm_payload_protocol::AsyncFrameError>
where
    S: AsyncWrite + Unpin,
{
    let frame = Frame::new(FrameKind::FAILURE, message.as_ref().as_bytes().to_vec())?;
    agentvm_payload_protocol::write_frame_async(stream, &frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::net::UnixListener;

    fn test_state_with_limits(max_clients: usize, max_diagnostics: usize) -> GuestServiceState {
        GuestServiceState::new(GuestServiceLimits {
            max_clients,
            max_diagnostics,
            initial_timeout: Duration::from_millis(100),
            io_timeout: Duration::from_millis(100),
        })
    }

    async fn send_initial_and_read_response(
        state: GuestServiceState,
        frame: Frame,
    ) -> Result<Frame, agentvm_payload_protocol::AsyncFrameError> {
        let (mut client, server) = duplex(64 * 1024);
        agentvm_payload_protocol::write_frame_async(&mut client, &frame).await?;
        client.shutdown().await?;
        let server_task = tokio::spawn(async move { state.handle_client(server).await });
        let response = agentvm_payload_protocol::read_frame_async(&mut client).await?;
        server_task
            .await
            .expect("guest service task joins")
            .expect("guest service handles client");
        Ok(response)
    }

    async fn send_initial_and_read_responses(
        state: GuestServiceState,
        frame: Frame,
    ) -> Result<Vec<Frame>, agentvm_payload_protocol::AsyncFrameError> {
        let (mut client, server) = duplex(64 * 1024);
        agentvm_payload_protocol::write_frame_async(&mut client, &frame).await?;
        client.shutdown().await?;
        let server_task = tokio::spawn(async move { state.handle_client(server).await });
        let mut responses = Vec::new();
        loop {
            let response = agentvm_payload_protocol::read_frame_async(&mut client).await?;
            let terminal = matches!(response.kind, FrameKind::EXIT | FrameKind::FAILURE);
            responses.push(response);
            if terminal {
                break;
            }
        }
        server_task
            .await
            .expect("guest service task joins")
            .expect("guest service handles client");
        Ok(responses)
    }

    fn diagnostic_request(script: impl Into<String>) -> DiagnosticRequest {
        DiagnosticRequest {
            script: script.into(),
            cwd: "/".to_string(),
            env: BTreeMap::new(),
            timeout_seconds: 10,
            max_output_bytes: 1024 * 1024,
        }
    }

    fn output_payloads(frames: &[Frame]) -> Vec<u8> {
        frames
            .iter()
            .filter(|frame| frame.kind == FrameKind::OUTPUT)
            .flat_map(|frame| frame.payload.iter().copied())
            .collect()
    }

    fn exit_payload(frames: &[Frame]) -> serde_json::Value {
        let frame = frames
            .iter()
            .find(|frame| frame.kind == FrameKind::EXIT)
            .expect("exit frame");
        serde_json::from_slice(&frame.payload).expect("exit json")
    }

    fn exit_code_payload(frames: &[Frame]) -> i32 {
        let frame = frames
            .iter()
            .find(|frame| frame.kind == FrameKind::EXIT)
            .expect("exit frame");
        serde_json::from_slice::<ExitFrame>(&frame.payload)
            .expect("exit json")
            .exit_code
    }

    async fn run_primary_frames(
        state: GuestServiceState,
        request: PayloadRequest,
        inputs: &[&[u8]],
    ) -> Vec<Frame> {
        let controls = inputs
            .iter()
            .map(|input| Frame::new(FrameKind::INPUT, input.to_vec()).expect("input frame"))
            .collect::<Vec<_>>();
        run_primary_control_frames(state, request, &controls).await
    }

    async fn run_primary_control_frames(
        state: GuestServiceState,
        request: PayloadRequest,
        controls: &[Frame],
    ) -> Vec<Frame> {
        let (mut client, server) = duplex(64 * 1024);
        let frame = Frame::new(
            FrameKind::RUN_PRIMARY,
            serde_json::to_vec(&request).expect("request json"),
        )
        .expect("primary frame");
        agentvm_payload_protocol::write_frame_async(&mut client, &frame)
            .await
            .expect("send primary request");
        for frame in controls {
            agentvm_payload_protocol::write_frame_async(&mut client, frame)
                .await
                .expect("send control frame");
        }
        let server_task = tokio::spawn(async move { state.handle_client(server).await });
        let mut responses = Vec::new();
        loop {
            let response = agentvm_payload_protocol::read_frame_async(&mut client)
                .await
                .expect("primary response");
            let terminal = response.kind == FrameKind::EXIT;
            responses.push(response);
            if terminal {
                break;
            }
        }
        server_task
            .await
            .expect("guest service task joins")
            .expect("guest service handles primary client");
        responses
    }

    #[tokio::test]
    async fn guest_service_replies_to_ping() {
        let state = test_state_with_limits(1, 1);
        let response = send_initial_and_read_response(
            state,
            Frame::new(FrameKind::PING, Vec::new()).expect("ping frame"),
        )
        .await
        .expect("ping response");

        assert_eq!(response.kind, FrameKind::OK);
        assert_eq!(response.payload, b"ok");
    }

    #[tokio::test]
    async fn guest_service_rejects_unexpected_initial_frame() {
        let state = test_state_with_limits(1, 1);
        let response = send_initial_and_read_response(
            state,
            Frame::new(FrameKind::INPUT, b"stdin".to_vec()).expect("input frame"),
        )
        .await
        .expect("failure response");

        assert_eq!(response.kind, FrameKind::FAILURE);
        assert_eq!(response.payload, b"unexpected initial frame");
    }

    #[test]
    fn primary_admission_allows_only_one_session_until_guard_drops() {
        let state = test_state_with_limits(1, 1);
        let primary = state.try_start_primary().expect("first primary admitted");
        assert!(state.try_start_primary().is_none());
        drop(primary);
        assert!(state.try_start_primary().is_some());
    }

    #[test]
    fn diagnostic_admission_respects_configured_limit() {
        let state = test_state_with_limits(1, 1);
        let diagnostic = state
            .try_start_diagnostic()
            .expect("first diagnostic admitted");
        assert!(state.try_start_diagnostic().is_none());
        drop(diagnostic);
        assert!(state.try_start_diagnostic().is_some());
    }

    #[tokio::test]
    async fn guest_service_rejects_primary_when_session_is_active() {
        let state = test_state_with_limits(1, 1);
        let _primary = state.try_start_primary().expect("hold active primary");
        let payload = serde_json::to_vec(&PayloadRequest::new("echo hello")).expect("json");
        let response = send_initial_and_read_response(
            state,
            Frame::new(FrameKind::RUN_PRIMARY, payload).expect("run frame"),
        )
        .await
        .expect("failure response");

        assert_eq!(response.kind, FrameKind::FAILURE);
        assert_eq!(response.payload, b"payload session already active");
    }

    #[tokio::test]
    async fn guest_service_rejects_diagnostic_when_limit_is_active() {
        let state = test_state_with_limits(1, 1);
        let _diagnostic = state
            .try_start_diagnostic()
            .expect("hold active diagnostic");
        let payload = serde_json::to_vec(&DiagnosticRequest::new("echo hello")).expect("json");
        let response = send_initial_and_read_response(
            state,
            Frame::new(FrameKind::RUN_DIAGNOSTIC, payload).expect("diagnostic frame"),
        )
        .await
        .expect("failure response");

        assert_eq!(response.kind, FrameKind::FAILURE);
        assert_eq!(response.payload, b"too many diagnostic sessions active");
    }

    #[tokio::test]
    async fn guest_service_runs_diagnostic_and_reports_exit() {
        let state = test_state_with_limits(1, 1);
        let request = diagnostic_request("printf stdout; printf stderr >&2; exit 7");
        let responses = send_initial_and_read_responses(
            state,
            Frame::new(
                FrameKind::RUN_DIAGNOSTIC,
                serde_json::to_vec(&request).expect("request json"),
            )
            .expect("diagnostic frame"),
        )
        .await
        .expect("diagnostic responses");

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("stdout"), "{output:?}");
        assert!(output.contains("stderr"), "{output:?}");
        let exit = exit_payload(&responses);
        assert_eq!(exit["exit_code"], 7);
        assert_eq!(exit["diagnostic"], true);
        assert_eq!(exit["timed_out"], false);
        assert_eq!(exit["truncated"], false);
    }

    #[tokio::test]
    async fn guest_service_diagnostic_enforces_output_limit() {
        let state = test_state_with_limits(1, 1);
        let mut request = diagnostic_request("printf abcdef");
        request.max_output_bytes = 3;
        let responses = send_initial_and_read_responses(
            state,
            Frame::new(
                FrameKind::RUN_DIAGNOSTIC,
                serde_json::to_vec(&request).expect("request json"),
            )
            .expect("diagnostic frame"),
        )
        .await
        .expect("diagnostic responses");

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.starts_with("abc"), "{output:?}");
        assert!(output.contains("agentvm diagnostic output limit exceeded"));
        let exit = exit_payload(&responses);
        assert_eq!(exit["exit_code"], 125);
        assert_eq!(exit["timed_out"], false);
        assert_eq!(exit["truncated"], true);
    }

    #[tokio::test]
    async fn guest_service_diagnostic_enforces_timeout() {
        let state = test_state_with_limits(1, 1);
        let mut request = diagnostic_request("sleep 2");
        request.timeout_seconds = 1;
        let responses = send_initial_and_read_responses(
            state,
            Frame::new(
                FrameKind::RUN_DIAGNOSTIC,
                serde_json::to_vec(&request).expect("request json"),
            )
            .expect("diagnostic frame"),
        )
        .await
        .expect("diagnostic responses");

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("agentvm diagnostic timed out"));
        let exit = exit_payload(&responses);
        assert_eq!(exit["exit_code"], 124);
        assert_eq!(exit["timed_out"], true);
        assert_eq!(exit["truncated"], false);
    }

    #[tokio::test]
    async fn docker_bridge_relays_tcp_client_to_unix_socket() {
        let docker_sock = std::env::temp_dir().join(format!(
            "agentvm-guest-service-docker-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&docker_sock);
        let docker_listener = UnixListener::bind(&docker_sock).expect("bind docker unix socket");
        let tcp_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind bridge tcp listener");
        let addr = tcp_listener.local_addr().expect("bridge addr");
        let bridge = tokio::spawn(serve_docker_bridge_listener(
            tcp_listener,
            docker_sock.clone(),
            DockerBridgeLimits {
                max_sessions: 4,
                connect_timeout: Duration::from_secs(1),
                io_timeout: Duration::from_secs(5),
            },
        ));

        let mut client = TcpStream::connect(addr).await.expect("connect bridge");
        let (mut docker, _) = docker_listener
            .accept()
            .await
            .expect("accept docker socket");
        let request_bytes = b"GET /_ping HTTP/1.1\r\n\r\n";
        client
            .write_all(request_bytes)
            .await
            .expect("write request");
        let mut request = vec![0_u8; request_bytes.len()];
        docker.read_exact(&mut request).await.expect("read request");
        assert_eq!(request, request_bytes);
        docker.write_all(b"OK").await.expect("write response");
        let mut response = [0_u8; 2];
        client
            .read_exact(&mut response)
            .await
            .expect("read response");
        assert_eq!(&response, b"OK");

        bridge.abort();
        let _ = std::fs::remove_file(&docker_sock);
    }

    #[tokio::test]
    async fn docker_bridge_closes_idle_session_after_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind tcp listener");
        let addr = listener.local_addr().expect("tcp addr");
        let client = TcpStream::connect(addr).await.expect("connect tcp client");
        let (server, _) = listener.accept().await.expect("accept tcp client");
        let (docker, _docker_peer) = UnixStream::pair().expect("unix pair");

        let stats = proxy_docker_bridge(server, docker, Duration::from_millis(20))
            .await
            .expect("proxy timeout");

        assert_eq!(stats.client_to_docker, 0);
        assert_eq!(stats.docker_to_client, 0);
        assert!(stats.timed_out);
        drop(client);
    }

    #[tokio::test]
    async fn guest_service_tcp_listener_serves_ping() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("local addr");
        let state = test_state_with_limits(1, 1);
        let server = tokio::spawn(async move { serve_listener(listener, state).await });
        let mut stream = TcpStream::connect(addr).await.expect("connect listener");
        agentvm_payload_protocol::write_frame_async(
            &mut stream,
            &Frame::new(FrameKind::PING, Vec::new()).expect("ping frame"),
        )
        .await
        .expect("send ping");
        let response = agentvm_payload_protocol::read_frame_async(&mut stream)
            .await
            .expect("read pong");
        server.abort();

        assert_eq!(response.kind, FrameKind::OK);
        assert_eq!(response.payload, b"ok");
    }

    #[tokio::test]
    async fn guest_service_primary_runs_command_and_reports_exit() {
        let state = test_state_with_limits(1, 1);
        let responses = run_primary_frames(
            state,
            PayloadRequest::new("printf primary-output; printf primary-err >&2; exit 6"),
            &[],
        )
        .await;

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("primary-output"), "{output:?}");
        assert!(output.contains("primary-err"), "{output:?}");
        assert_eq!(exit_code_payload(&responses), 6);
    }

    #[tokio::test]
    async fn guest_service_primary_forwards_stdin() {
        let state = test_state_with_limits(1, 1);
        let responses = run_primary_frames(
            state,
            PayloadRequest::new("IFS= read -r line; printf got:%s \"$line\""),
            &[b"hello from stdin\n"],
        )
        .await;

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("got:hello from stdin"), "{output:?}");
        assert_eq!(exit_code_payload(&responses), 0);
    }

    #[tokio::test]
    async fn guest_service_primary_runs_inside_requested_tty() {
        let state = test_state_with_limits(1, 1);
        let mut request =
            PayloadRequest::new("test -t 0; test -t 1; test -t 2; printf tty-ok; stty size");
        request.rows = 13;
        request.cols = 17;
        let responses = run_primary_frames(state, request, &[]).await;

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("tty-ok"), "{output:?}");
        assert!(output.contains("13 17"), "{output:?}");
        assert_eq!(exit_code_payload(&responses), 0);
    }

    #[tokio::test]
    async fn guest_service_primary_applies_resize_to_tty() {
        let state = test_state_with_limits(1, 1);
        let (mut client, server) = duplex(64 * 1024);
        let mut request = PayloadRequest::new("stty size; IFS= read -r _; stty size");
        request.rows = 5;
        request.cols = 9;
        let frame = Frame::new(
            FrameKind::RUN_PRIMARY,
            serde_json::to_vec(&request).expect("request json"),
        )
        .expect("primary frame");
        agentvm_payload_protocol::write_frame_async(&mut client, &frame)
            .await
            .expect("send primary request");
        let server_task = tokio::spawn(async move { state.handle_client(server).await });
        let first = agentvm_payload_protocol::read_frame_async(&mut client)
            .await
            .expect("first stty output");
        assert_eq!(first.kind, FrameKind::OUTPUT);
        let first_output = String::from_utf8_lossy(&first.payload).to_string();
        assert!(first_output.contains("5 9"), "{first_output:?}");

        agentvm_payload_protocol::write_frame_async(
            &mut client,
            &Frame::new(
                FrameKind::RESIZE,
                resize_payload(12, 34).expect("resize json"),
            )
            .expect("resize frame"),
        )
        .await
        .expect("send resize");
        agentvm_payload_protocol::write_frame_async(
            &mut client,
            &Frame::new(FrameKind::INPUT, b"\n".to_vec()).expect("input frame"),
        )
        .await
        .expect("send input");

        let mut responses = vec![first];
        loop {
            let response = agentvm_payload_protocol::read_frame_async(&mut client)
                .await
                .expect("primary response");
            let terminal = response.kind == FrameKind::EXIT;
            responses.push(response);
            if terminal {
                break;
            }
        }
        server_task
            .await
            .expect("guest service task joins")
            .expect("guest service handles primary client");

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("12 34"), "{output:?}");
        assert_eq!(exit_code_payload(&responses), 0);
    }

    #[tokio::test]
    async fn guest_service_primary_uses_requested_identity_and_home() {
        let state = test_state_with_limits(1, 1);
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let mut request = PayloadRequest::new(
            "test \"$(id -u)\" = \"$AGENTVM_UID\"; test \"$(id -g)\" = \"$AGENTVM_GID\"; test -d \"$HOME\"; printf identity-ok",
        );
        let home = std::env::temp_dir().join(format!(
            "agentvm-guest-service-home-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos()
        ));
        request
            .env
            .insert("AGENTVM_UID".to_string(), uid.to_string());
        request
            .env
            .insert("AGENTVM_GID".to_string(), gid.to_string());
        request
            .env
            .insert("HOME".to_string(), home.display().to_string());
        let responses = run_primary_frames(state, request, &[]).await;
        let _ = std::fs::remove_dir_all(home);

        let output = String::from_utf8_lossy(&output_payloads(&responses)).to_string();
        assert!(output.contains("identity-ok"), "{output:?}");
        assert_eq!(exit_code_payload(&responses), 0);
    }

    #[tokio::test]
    async fn guest_service_primary_forwards_signal_to_process_group() {
        let state = test_state_with_limits(1, 1);
        let (mut client, server) = duplex(64 * 1024);
        let request =
            PayloadRequest::new("trap 'exit 42' TERM; printf ready; while :; do sleep 1; done");
        let frame = Frame::new(
            FrameKind::RUN_PRIMARY,
            serde_json::to_vec(&request).expect("request json"),
        )
        .expect("primary frame");
        agentvm_payload_protocol::write_frame_async(&mut client, &frame)
            .await
            .expect("send primary request");
        let server_task = tokio::spawn(async move { state.handle_client(server).await });
        let ready = agentvm_payload_protocol::read_frame_async(&mut client)
            .await
            .expect("ready output");
        assert_eq!(ready.kind, FrameKind::OUTPUT);
        assert_eq!(ready.payload, b"ready");

        let signal = Frame::new(
            FrameKind::SIGNAL,
            agentvm_payload_protocol::signal_payload(libc::SIGTERM).expect("signal json"),
        )
        .expect("signal frame");
        agentvm_payload_protocol::write_frame_async(&mut client, &signal)
            .await
            .expect("send signal");
        let exit = time::timeout(
            Duration::from_secs(2),
            agentvm_payload_protocol::read_frame_async(&mut client),
        )
        .await
        .expect("primary terminates after signal")
        .expect("exit frame");
        server_task
            .await
            .expect("guest service task joins")
            .expect("guest service handles primary client");

        assert_eq!(exit.kind, FrameKind::EXIT);
        assert_eq!(
            serde_json::from_slice::<ExitFrame>(&exit.payload)
                .expect("exit json")
                .exit_code,
            42
        );
    }

    #[tokio::test]
    async fn guest_service_primary_disconnect_terminates_process() {
        let state = test_state_with_limits(1, 1);
        let (mut client, server) = duplex(64 * 1024);
        let frame = Frame::new(
            FrameKind::RUN_PRIMARY,
            serde_json::to_vec(&PayloadRequest::new("while :; do sleep 1; done"))
                .expect("request json"),
        )
        .expect("primary frame");
        agentvm_payload_protocol::write_frame_async(&mut client, &frame)
            .await
            .expect("send primary request");
        client.shutdown().await.expect("disconnect client");
        let server_task = tokio::spawn(async move { state.handle_client(server).await });

        time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task terminates after disconnect")
            .expect("server task joins")
            .expect("disconnect cleanup succeeds");
    }
}
