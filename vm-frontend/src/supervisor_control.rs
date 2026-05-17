use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::supervisor::{LaunchSupervisor, SupervisorShutdown, SupervisorTaskResult};
use crate::RuntimePaths;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_SOCKET_FILE_NAME: &str = "agentvm-control.sock";
pub const MAX_CONTROL_MESSAGE_BYTES: u64 = 64 * 1024;

pub fn control_socket_path(runtime: &RuntimePaths) -> PathBuf {
    control_socket_path_under(&runtime.run_dir)
}

pub fn control_socket_path_under(run_dir: &Path) -> PathBuf {
    run_dir.join(CONTROL_SOCKET_FILE_NAME)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorControlClient {
    socket_path: PathBuf,
}

impl SupervisorControlClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn for_runtime(runtime: &RuntimePaths) -> Self {
        Self::new(control_socket_path(runtime))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn status_snapshot(
        &self,
    ) -> Result<SupervisorControlSnapshot, SupervisorControlIoError> {
        match control_client_request(&self.socket_path, SupervisorControlRequest::StatusSnapshot)
            .await?
        {
            SupervisorControlResponse::StatusSnapshot { snapshot } => Ok(snapshot),
            SupervisorControlResponse::Error { message } => {
                Err(SupervisorControlIoError::RemoteError { message })
            }
            response => Err(SupervisorControlIoError::UnexpectedResponse {
                response: format!("{response:?}"),
            }),
        }
    }

    pub async fn request_shutdown(
        &self,
        reason: impl Into<String>,
    ) -> Result<(), SupervisorControlIoError> {
        match control_client_request(
            &self.socket_path,
            SupervisorControlRequest::RequestShutdown {
                reason: reason.into(),
            },
        )
        .await?
        {
            SupervisorControlResponse::Ack => Ok(()),
            SupervisorControlResponse::Error { message } => {
                Err(SupervisorControlIoError::RemoteError { message })
            }
            response => Err(SupervisorControlIoError::UnexpectedResponse {
                response: format!("{response:?}"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorControlEnvelope<T> {
    pub version: u32,
    pub message: T,
}

impl<T> SupervisorControlEnvelope<T> {
    pub fn new(message: T) -> Self {
        Self {
            version: CONTROL_PROTOCOL_VERSION,
            message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SupervisorControlRequest {
    StatusSnapshot,
    SubscribeStatus,
    RequestShutdown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SupervisorControlResponse {
    StatusSnapshot { snapshot: SupervisorControlSnapshot },
    Ack,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorControlSnapshot {
    pub shutdown: SupervisorShutdown,
    pub tasks: Vec<SupervisorTaskResult>,
}

impl SupervisorControlSnapshot {
    pub fn from_supervisor(supervisor: &LaunchSupervisor) -> Self {
        Self {
            shutdown: supervisor.current_shutdown(),
            tasks: supervisor.task_statuses(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorControlProtocolError {
    #[error("invalid supervisor control JSON: {source}")]
    InvalidJson { source: serde_json::Error },
    #[error("unsupported supervisor control protocol version {actual}, expected {expected}")]
    UnsupportedVersion { actual: u32, expected: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorControlIoError {
    #[error("supervisor control I/O failed: {source}")]
    Io { source: std::io::Error },
    #[error("failed to encode supervisor control message: {source}")]
    EncodeJson { source: serde_json::Error },
    #[error(transparent)]
    Protocol(#[from] SupervisorControlProtocolError),
    #[error("supervisor control message exceeded {limit} bytes")]
    MessageTooLarge { limit: u64 },
    #[error("supervisor control returned error: {message}")]
    RemoteError { message: String },
    #[error("unexpected supervisor control response: {response}")]
    UnexpectedResponse { response: String },
}

impl From<std::io::Error> for SupervisorControlIoError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<serde_json::Error> for SupervisorControlIoError {
    fn from(source: serde_json::Error) -> Self {
        Self::EncodeJson { source }
    }
}

pub fn encode_control_message<T: Serialize>(message: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&SupervisorControlEnvelope::new(message))
}

pub fn decode_control_request(
    bytes: &[u8],
) -> Result<SupervisorControlRequest, SupervisorControlProtocolError> {
    decode_control_envelope(bytes)
}

pub fn decode_control_response(
    bytes: &[u8],
) -> Result<SupervisorControlResponse, SupervisorControlProtocolError> {
    decode_control_envelope(bytes)
}

pub fn bind_control_socket(path: &Path) -> Result<UnixListener, SupervisorControlIoError> {
    UnixListener::bind(path).map_err(Into::into)
}

pub async fn control_client_request(
    socket_path: &Path,
    request: SupervisorControlRequest,
) -> Result<SupervisorControlResponse, SupervisorControlIoError> {
    let mut stream = UnixStream::connect(socket_path).await?;
    write_control_message(&mut stream, &request).await?;
    stream.shutdown().await?;
    let bytes = read_bounded_control_message(&mut stream).await?;
    decode_control_response(&bytes).map_err(Into::into)
}

pub async fn serve_control_listener_once(
    listener: &UnixListener,
    supervisor: &LaunchSupervisor,
) -> Result<(), SupervisorControlIoError> {
    let (stream, _addr) = listener.accept().await?;
    handle_control_connection(stream, supervisor).await
}

pub async fn serve_control_listener_until_shutdown(
    listener: UnixListener,
    supervisor: Arc<LaunchSupervisor>,
) -> Result<(), SupervisorControlIoError> {
    let mut shutdown_rx = supervisor.subscribe_shutdown();
    loop {
        if shutdown_rx.borrow().is_requested() {
            return Ok(());
        }
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let supervisor = Arc::clone(&supervisor);
                tokio::spawn(async move {
                    let _ = handle_control_connection(stream, &supervisor).await;
                });
            }
            changed = shutdown_rx.changed() => {
                changed.map_err(|source| SupervisorControlIoError::Io { source: std::io::Error::other(source) })?;
            }
        }
    }
}

pub async fn handle_control_connection<S>(
    mut stream: S,
    supervisor: &LaunchSupervisor,
) -> Result<(), SupervisorControlIoError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let response = match read_bounded_control_message(&mut stream)
        .await
        .and_then(|bytes| decode_control_request(&bytes).map_err(Into::into))
    {
        Ok(request) => apply_control_request(supervisor, request),
        Err(error) => SupervisorControlResponse::Error {
            message: error.to_string(),
        },
    };
    write_control_message(&mut stream, &response).await?;
    stream.shutdown().await?;
    Ok(())
}

pub fn apply_control_request(
    supervisor: &LaunchSupervisor,
    request: SupervisorControlRequest,
) -> SupervisorControlResponse {
    match request {
        SupervisorControlRequest::StatusSnapshot => SupervisorControlResponse::StatusSnapshot {
            snapshot: SupervisorControlSnapshot::from_supervisor(supervisor),
        },
        SupervisorControlRequest::RequestShutdown { reason } => {
            supervisor.request_shutdown(reason);
            SupervisorControlResponse::Ack
        }
        SupervisorControlRequest::SubscribeStatus => SupervisorControlResponse::Error {
            message: "status subscriptions are not implemented yet".to_string(),
        },
    }
}

async fn write_control_message<W, T>(
    writer: &mut W,
    message: &T,
) -> Result<(), SupervisorControlIoError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = encode_control_message(message)?;
    writer.write_all(&bytes).await?;
    Ok(())
}

async fn read_bounded_control_message<R>(
    reader: &mut R,
) -> Result<Vec<u8>, SupervisorControlIoError>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut limited = reader.take(MAX_CONTROL_MESSAGE_BYTES + 1);
    limited.read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > MAX_CONTROL_MESSAGE_BYTES {
        return Err(SupervisorControlIoError::MessageTooLarge {
            limit: MAX_CONTROL_MESSAGE_BYTES,
        });
    }
    Ok(bytes)
}

fn decode_control_envelope<T>(bytes: &[u8]) -> Result<T, SupervisorControlProtocolError>
where
    T: for<'de> Deserialize<'de>,
{
    let envelope: SupervisorControlEnvelope<T> = serde_json::from_slice(bytes)
        .map_err(|source| SupervisorControlProtocolError::InvalidJson { source })?;
    if envelope.version != CONTROL_PROTOCOL_VERSION {
        return Err(SupervisorControlProtocolError::UnsupportedVersion {
            actual: envelope.version,
            expected: CONTROL_PROTOCOL_VERSION,
        });
    }
    Ok(envelope.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::{LaunchSupervisor, SupervisorTaskName, SupervisorTaskStatus};
    use crate::{
        FrontendConfig, GuestNetwork, RuntimePaths, ToolPaths, VmArtifacts, VmShape,
        COMPOSED_FS_TAG,
    };
    use std::path::PathBuf;
    use tokio::io::AsyncWriteExt;

    fn config() -> FrontendConfig {
        FrontendConfig {
            project: PathBuf::from("/repo"),
            tools: ToolPaths {
                qemu_system_x86_64: PathBuf::from("qemu-system-x86_64"),
            },
            artifacts: VmArtifacts {
                kernel: PathBuf::from("/repo/docker/out/vmlinuz"),
                initrd: PathBuf::from("/repo/docker/out/initrd.img"),
                rootfs: PathBuf::from("/repo/docker/out/rootfs.raw"),
            },
            runtime: RuntimePaths::under("/repo/.sandbox/docker-vm/run"),
            vm: VmShape {
                memory_bytes: 2 * 1024 * 1024 * 1024,
                cpus: 2,
                kernel_cmdline: "console=ttyS0".to_string(),
                virtiofs_tag: COMPOSED_FS_TAG.to_string(),
            },
            network: GuestNetwork::default(),
            guest_http_smoke_url: None,
            upstream_mappings: Vec::new(),
        }
    }

    #[test]
    fn control_socket_lives_under_runtime_run_dir() {
        let runtime = RuntimePaths::under("/repo/.sandbox/docker-vm/run");
        assert_eq!(
            control_socket_path(&runtime),
            PathBuf::from("/repo/.sandbox/docker-vm/run/agentvm-control.sock")
        );
    }

    #[test]
    fn control_request_round_trips_through_versioned_json_envelope() {
        let request = SupervisorControlRequest::RequestShutdown {
            reason: "tui requested shutdown".to_string(),
        };
        let encoded = encode_control_message(&request).expect("encode request");
        let decoded = decode_control_request(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn control_snapshot_reflects_existing_supervisor_state() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        supervisor
            .task_controller(SupervisorTaskName::VmnetGateway)
            .expect("vmnet controller")
            .mark_ready()
            .expect("ready");
        supervisor.request_shutdown("test shutdown");

        let snapshot = SupervisorControlSnapshot::from_supervisor(&supervisor);

        assert_eq!(
            snapshot.shutdown,
            SupervisorShutdown::Requested {
                reason: "test shutdown".to_string(),
            }
        );
        assert!(snapshot.tasks.iter().any(|task| {
            task.name == SupervisorTaskName::VmnetGateway
                && task.status == SupervisorTaskStatus::Ready
        }));
    }

    #[test]
    fn control_decoder_rejects_invalid_json_and_wrong_version() {
        assert!(matches!(
            decode_control_request(b"not json"),
            Err(SupervisorControlProtocolError::InvalidJson { .. })
        ));

        let wrong_version = br#"{"version":2,"message":{"type":"status-snapshot"}}"#;
        assert!(matches!(
            decode_control_request(wrong_version),
            Err(SupervisorControlProtocolError::UnsupportedVersion {
                actual: 2,
                expected: CONTROL_PROTOCOL_VERSION,
            })
        ));
    }

    #[tokio::test]
    async fn control_listener_serves_status_snapshot_over_unix_socket() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let socket = tempdir.path().join(CONTROL_SOCKET_FILE_NAME);
        let listener = bind_control_socket(&socket).expect("bind control socket");
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config controller")
            .mark_ready()
            .expect("ready");

        let server = serve_control_listener_once(&listener, &supervisor);
        let client = control_client_request(&socket, SupervisorControlRequest::StatusSnapshot);
        let (server_result, client_result) = tokio::join!(server, client);

        server_result.expect("server result");
        let response = client_result.expect("client response");
        let SupervisorControlResponse::StatusSnapshot { snapshot } = response else {
            panic!("unexpected control response: {response:?}");
        };
        assert!(snapshot.tasks.iter().any(|task| {
            task.name == SupervisorTaskName::ConfigFs && task.status == SupervisorTaskStatus::Ready
        }));
    }

    #[tokio::test]
    async fn control_client_can_request_supervisor_shutdown() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let socket = tempdir.path().join(CONTROL_SOCKET_FILE_NAME);
        let listener = bind_control_socket(&socket).expect("bind control socket");
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());

        let server = serve_control_listener_once(&listener, &supervisor);
        let client = control_client_request(
            &socket,
            SupervisorControlRequest::RequestShutdown {
                reason: "tui requested shutdown".to_string(),
            },
        );
        let (server_result, client_result) = tokio::join!(server, client);

        server_result.expect("server result");
        assert_eq!(
            client_result.expect("client response"),
            SupervisorControlResponse::Ack
        );
        assert_eq!(
            supervisor.current_shutdown(),
            SupervisorShutdown::Requested {
                reason: "tui requested shutdown".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn control_client_adapter_reads_status_and_stops_server_loop() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let socket = tempdir.path().join(CONTROL_SOCKET_FILE_NAME);
        let listener = bind_control_socket(&socket).expect("bind control socket");
        let supervisor = Arc::new(LaunchSupervisor::new(config().supervisor_plan()));
        supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller")
            .mark_starting()
            .expect("starting");
        let server_supervisor = Arc::clone(&supervisor);
        let server = tokio::spawn(async move {
            serve_control_listener_until_shutdown(listener, server_supervisor).await
        });
        let client = SupervisorControlClient::new(socket);

        let snapshot = client.status_snapshot().await.expect("snapshot");
        assert!(snapshot.tasks.iter().any(|task| {
            task.name == SupervisorTaskName::Qemu && task.status == SupervisorTaskStatus::Starting
        }));

        client
            .request_shutdown("adapter requested shutdown")
            .await
            .expect("request shutdown");
        assert_eq!(
            supervisor.current_shutdown(),
            SupervisorShutdown::Requested {
                reason: "adapter requested shutdown".to_string(),
            }
        );
        server.await.expect("server join").expect("server result");
    }

    #[tokio::test]
    async fn control_connection_returns_error_response_for_malformed_request() {
        let (mut client, server) = tokio::io::duplex(1024);
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let server = handle_control_connection(server, &supervisor);
        let client = async {
            client.write_all(b"not-json").await.expect("write request");
            client.shutdown().await.expect("shutdown request");
            let bytes = read_bounded_control_message(&mut client)
                .await
                .expect("read response");
            decode_control_response(&bytes).expect("decode response")
        };

        let (server_result, response) = tokio::join!(server, client);

        server_result.expect("server result");
        let SupervisorControlResponse::Error { message } = response else {
            panic!("unexpected control response: {response:?}");
        };
        assert!(message.contains("invalid supervisor control JSON"));
    }

    #[tokio::test]
    async fn control_connection_rejects_oversized_request() {
        let (mut client, server) = tokio::io::duplex((MAX_CONTROL_MESSAGE_BYTES + 128) as usize);
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let server = handle_control_connection(server, &supervisor);
        let client = async {
            let oversized = vec![b' '; (MAX_CONTROL_MESSAGE_BYTES + 1) as usize];
            client.write_all(&oversized).await.expect("write request");
            client.shutdown().await.expect("shutdown request");
            let bytes = read_bounded_control_message(&mut client)
                .await
                .expect("read response");
            decode_control_response(&bytes).expect("decode response")
        };

        let (server_result, response) = tokio::join!(server, client);

        server_result.expect("server result");
        let SupervisorControlResponse::Error { message } = response else {
            panic!("unexpected control response: {response:?}");
        };
        assert!(message.contains("exceeded"));
    }
}
