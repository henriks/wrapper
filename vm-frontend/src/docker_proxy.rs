use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::thread;

use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerUnixProxyConfig {
    pub socket_path: PathBuf,
    pub tcp_host: Ipv4Addr,
    pub tcp_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockerUnixProxyLimits {
    pub max_connections: usize,
}

impl Default for DockerUnixProxyLimits {
    fn default() -> Self {
        Self {
            max_connections: 32,
        }
    }
}

pub fn start_docker_unix_proxy(config: DockerUnixProxyConfig) -> io::Result<()> {
    let listener = bind_docker_unix_proxy_listener(&config.socket_path)?;
    info!(
        socket = %config.socket_path.display(),
        tcp_host = %config.tcp_host,
        tcp_port = config.tcp_port,
        "starting docker unix proxy"
    );
    thread::Builder::new()
        .name("agentvm-docker-unix-proxy".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    error!(%error, "failed to start docker unix proxy runtime");
                    return;
                }
            };
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);
            if let Err(error) = runtime.block_on(serve_docker_unix_proxy_listener_async(
                listener,
                config.tcp_host,
                config.tcp_port,
                DockerUnixProxyLimits::default(),
                shutdown_rx,
            )) {
                error!(%error, "docker unix proxy service failed");
            }
        })
        .map(|_| ())
}

pub async fn run_docker_unix_proxy_async(
    config: DockerUnixProxyConfig,
    limits: DockerUnixProxyLimits,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    if limits.max_connections == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "docker proxy max_connections must be greater than zero",
        ));
    }
    remove_stale_socket(&config.socket_path)?;
    if let Some(parent) = config.socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = tokio::net::UnixListener::bind(&config.socket_path)?;
    info!(
        socket = %config.socket_path.display(),
        tcp_host = %config.tcp_host,
        tcp_port = config.tcp_port,
        max_connections = limits.max_connections,
        "starting async docker unix proxy"
    );
    serve_docker_unix_proxy_async(listener, config.tcp_host, config.tcp_port, limits, shutdown)
        .await
}

async fn serve_docker_unix_proxy_listener_async(
    listener: UnixListener,
    tcp_host: Ipv4Addr,
    tcp_port: u16,
    limits: DockerUnixProxyLimits,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener)?;
    serve_docker_unix_proxy_async(listener, tcp_host, tcp_port, limits, shutdown).await
}

async fn serve_docker_unix_proxy_async(
    listener: tokio::net::UnixListener,
    tcp_host: Ipv4Addr,
    tcp_port: u16,
    limits: DockerUnixProxyLimits,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let permits = std::sync::Arc::new(Semaphore::new(limits.max_connections));
    let mut clients = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                match changed {
                    Ok(()) if *shutdown.borrow() => break,
                    Ok(()) => continue,
                    Err(_) => break,
                }
            }
            Some(result) = clients.join_next() => {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => error!(%error, %tcp_host, tcp_port, "async docker unix proxy client failed"),
                    Err(error) => error!(%error, %tcp_host, tcp_port, "async docker unix proxy client task failed"),
                }
            }
            accepted = listener.accept() => {
                let (client, _) = accepted?;
                let permit = match permits.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!(%tcp_host, tcp_port, max_connections = limits.max_connections, "rejecting docker unix proxy client: connection limit reached");
                        continue;
                    }
                };
                debug!(%tcp_host, tcp_port, "accepted async docker unix proxy client");
                clients.spawn(async move {
                    let _permit = permit;
                    proxy_client_async(client, tcp_host, tcp_port).await
                });
            }
        }
    }

    clients.abort_all();
    while clients.join_next().await.is_some() {}
    Ok(())
}

async fn proxy_client_async(
    mut client: tokio::net::UnixStream,
    tcp_host: Ipv4Addr,
    tcp_port: u16,
) -> io::Result<()> {
    debug!(%tcp_host, tcp_port, "connecting async docker unix proxy upstream");
    let mut upstream = tokio::net::TcpStream::connect((tcp_host, tcp_port)).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map(|_| ())
}

fn bind_docker_unix_proxy_listener(path: &Path) -> io::Result<UnixListener> {
    remove_stale_socket(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn async_docker_proxy_bridges_unix_client_to_tcp_upstream() {
        let tempdir = tempfile::tempdir().unwrap();
        let socket_path = tempdir.path().join("docker.sock");
        let upstream = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = [0; 4];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let service_config = DockerUnixProxyConfig {
            socket_path: socket_path.clone(),
            tcp_host: Ipv4Addr::LOCALHOST,
            tcp_port: upstream_port,
        };
        let service = tokio::spawn(run_docker_unix_proxy_async(
            service_config,
            DockerUnixProxyLimits { max_connections: 4 },
            shutdown_rx,
        ));
        wait_for_socket(&socket_path).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"pong");

        shutdown_tx.send(true).unwrap();
        service.await.unwrap().unwrap();
        upstream_task.await.unwrap();
    }

    #[tokio::test]
    async fn async_docker_proxy_shutdown_cancels_open_clients() {
        let tempdir = tempfile::tempdir().unwrap();
        let socket_path = tempdir.path().join("docker.sock");
        let upstream = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        let upstream_task = tokio::spawn(async move {
            let (_stream, _) = upstream.accept().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let service = tokio::spawn(run_docker_unix_proxy_async(
            DockerUnixProxyConfig {
                socket_path: socket_path.clone(),
                tcp_host: Ipv4Addr::LOCALHOST,
                tcp_port: upstream_port,
            },
            DockerUnixProxyLimits { max_connections: 1 },
            shutdown_rx,
        ));
        wait_for_socket(&socket_path).await;
        let _client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        shutdown_tx.send(true).unwrap();
        service.await.unwrap().unwrap();
        upstream_task.abort();
    }

    #[tokio::test]
    async fn async_docker_proxy_rejects_zero_connection_limit() {
        let tempdir = tempfile::tempdir().unwrap();
        let error = run_docker_unix_proxy_async(
            DockerUnixProxyConfig {
                socket_path: tempdir.path().join("docker.sock"),
                tcp_host: Ipv4Addr::LOCALHOST,
                tcp_port: 1,
            },
            DockerUnixProxyLimits { max_connections: 0 },
            watch::channel(false).1,
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    async fn wait_for_socket(path: &Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("socket {} was not created", path.display());
    }
}
