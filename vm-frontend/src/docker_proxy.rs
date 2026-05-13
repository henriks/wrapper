use std::fs;
use std::io;
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerUnixProxyConfig {
    pub socket_path: PathBuf,
    pub tcp_host: Ipv4Addr,
    pub tcp_port: u16,
}

pub fn start_docker_unix_proxy(config: DockerUnixProxyConfig) -> io::Result<()> {
    remove_stale_socket(&config.socket_path)?;
    if let Some(parent) = config.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&config.socket_path)?;
    thread::Builder::new()
        .name("agentvm-docker-unix-proxy".to_string())
        .spawn(move || serve_docker_unix_proxy(listener, config.tcp_host, config.tcp_port))
        .map(|_| ())
}

fn serve_docker_unix_proxy(listener: UnixListener, tcp_host: Ipv4Addr, tcp_port: u16) {
    for client in listener.incoming() {
        match client {
            Ok(client) => {
                thread::Builder::new()
                    .name("agentvm-docker-unix-proxy-client".to_string())
                    .spawn(move || {
                        if let Err(error) = proxy_client(client, tcp_host, tcp_port) {
                            eprintln!("agentvm docker unix proxy client failed: {error}");
                        }
                    })
                    .ok();
            }
            Err(error) => {
                eprintln!("agentvm docker unix proxy accept failed: {error}");
                break;
            }
        }
    }
}

fn proxy_client(mut client: UnixStream, tcp_host: Ipv4Addr, tcp_port: u16) -> io::Result<()> {
    let mut upstream = TcpStream::connect((tcp_host, tcp_port))?;
    let mut upstream_for_client = upstream.try_clone()?;
    let mut client_for_upstream = client.try_clone()?;

    let upstream_to_client = thread::Builder::new()
        .name("agentvm-docker-proxy-upstream".to_string())
        .spawn(move || io::copy(&mut upstream_for_client, &mut client));

    let client_to_upstream = io::copy(&mut client_for_upstream, &mut upstream);
    match upstream_to_client {
        Ok(handle) => {
            let upstream_to_client = handle
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("docker proxy thread panicked")));
            client_to_upstream.and(upstream_to_client).map(|_| ())
        }
        Err(error) => Err(error),
    }
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
