use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use vhost::vhost_user::Listener;
use vhost_user_backend::VhostUserDaemon;
use virtiofsd::vhost_user::VhostUserFsBackendBuilder;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};

use crate::{
    invalid_input, ComposedFs, Manifest, Namespace, DEFAULT_TAG, DEFAULT_THREAD_POOL_SIZE,
};

#[derive(Debug, Parser)]
#[command(
    name = "agentvm-composed-fs",
    about = "Serve an AgentVM composed filesystem over vhost-user"
)]
struct Args {
    #[arg(long, value_name = "PATH")]
    manifest: PathBuf,
    #[arg(long, value_name = "PATH")]
    socket_path: PathBuf,
    #[arg(long, default_value = DEFAULT_TAG)]
    tag: String,
    #[arg(long, default_value_t = DEFAULT_THREAD_POOL_SIZE)]
    thread_pool_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub manifest: PathBuf,
    pub socket_path: PathBuf,
    pub tag: String,
    pub thread_pool_size: usize,
}

impl ServeConfig {
    pub fn new(manifest: PathBuf, socket_path: PathBuf) -> Self {
        Self {
            manifest,
            socket_path,
            tag: String::from(DEFAULT_TAG),
            thread_pool_size: DEFAULT_THREAD_POOL_SIZE,
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.thread_pool_size == 0 {
            return Err(invalid_input(
                "thread_pool_size must be at least 1 for bounded blocking filesystem workers",
            ));
        }
        Ok(())
    }
}

pub fn run_cli() -> io::Result<()> {
    let args = Args::parse();
    serve_vhost_user_fs(ServeConfig {
        manifest: args.manifest,
        socket_path: args.socket_path,
        tag: args.tag,
        thread_pool_size: args.thread_pool_size,
    })
}

pub fn serve_vhost_user_fs(config: ServeConfig) -> io::Result<()> {
    config.validate()?;
    let manifest_text = fs::read_to_string(&config.manifest)?;
    let manifest: Manifest = serde_json::from_str(&manifest_text)
        .map_err(|error| invalid_input(format!("invalid manifest JSON: {error}")))?;
    let namespace = Namespace::from_manifest(&manifest)?;
    let fs = ComposedFs::new(namespace);

    if let Some(parent) = config.socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let backend = Arc::new(
        VhostUserFsBackendBuilder::default()
            .set_thread_pool_size(config.thread_pool_size)
            .set_tag(Some(config.tag.clone()))
            .build(fs)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?,
    );
    let listener = Listener::new(&config.socket_path, true)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
    let mut daemon = VhostUserDaemon::new(
        String::from("agentvm-composed-fs"),
        backend,
        GuestMemoryAtomic::new(GuestMemoryMmap::new()),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;

    eprintln!(
        "agentvm-composed-fs: serving tag {:?} on {}",
        config.tag,
        config.socket_path.display()
    );
    daemon
        .start(listener)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, format!("{error:?}")))?;
    daemon
        .wait()
        .map_err(|error| io::Error::new(io::ErrorKind::Other, format!("{error:?}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_config_defaults_to_one_bounded_blocking_worker() {
        let config = ServeConfig::new(PathBuf::from("manifest.json"), PathBuf::from("fs.sock"));

        assert_eq!(config.thread_pool_size, 1);
        config.validate().expect("default worker count is valid");
    }

    #[test]
    fn serve_config_rejects_zero_bounded_blocking_workers() {
        let mut config = ServeConfig::new(PathBuf::from("manifest.json"), PathBuf::from("fs.sock"));
        config.thread_pool_size = 0;

        let error = config.validate().expect_err("zero workers rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("thread_pool_size"));
    }
}
