use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use agentvm_composed_fs::serve_vhost_user_fs;
use serde::{Deserialize, Serialize};

use crate::docker_proxy::{start_docker_unix_proxy, DockerUnixProxyConfig};
use crate::network_policy::{HostListenerPurpose, VmnetPolicy};
use crate::runtime_manifest::{
    write_runtime_manifests, write_runtime_manifests_with_config_mounts, ManifestSourceClass,
    RuntimeManifestSummary, RuntimeMount,
};
use crate::vmnet_runtime::serve_vmnet_gateway;
use crate::{FrontendConfig, GuestNetwork, RuntimePaths, ToolPaths, VmArtifacts, VmShape};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WAIT_STEP: Duration = Duration::from_millis(20);
const DATA_DISK_SIZE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug)]
pub enum LaunchError {
    Io(io::Error),
    Json(serde_json::Error),
    Artifact(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Io(error) => write!(f, "{error}"),
            LaunchError::Json(error) => write!(f, "{error}"),
            LaunchError::Artifact(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

impl From<io::Error> for LaunchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactManifest {
    pub artifacts: ArtifactPaths,
    pub vm: ArtifactVm,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactPaths {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub rootfs: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactVm {
    pub cpus: u16,
    pub memory_bytes: u64,
    pub virtiofs_tag: String,
    pub kernel_cmdline: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPreparation {
    pub manifests: RuntimeManifestSummary,
    pub qemu_command: Vec<String>,
}

impl ArtifactManifest {
    pub fn into_frontend_config(
        self,
        project: impl Into<PathBuf>,
        run_dir: impl Into<PathBuf>,
        qemu_system_x86_64: impl Into<PathBuf>,
        repo_root: impl AsRef<Path>,
    ) -> Result<FrontendConfig, LaunchError> {
        let repo_root = repo_root.as_ref();
        Ok(FrontendConfig {
            project: absolute_path(project.into())?,
            tools: ToolPaths {
                qemu_system_x86_64: qemu_system_x86_64.into(),
            },
            artifacts: VmArtifacts {
                kernel: resolve_artifact(repo_root, self.artifacts.kernel)?,
                initrd: resolve_artifact(repo_root, self.artifacts.initrd)?,
                rootfs: resolve_artifact(repo_root, self.artifacts.rootfs)?,
            },
            runtime: RuntimePaths::under(run_dir.into()),
            vm: VmShape {
                memory_bytes: self.vm.memory_bytes,
                cpus: self.vm.cpus,
                kernel_cmdline: ensure_ttys0_console(&self.vm.kernel_cmdline),
                virtiofs_tag: self.vm.virtiofs_tag,
            },
            network: computed_guest_network(repo_root),
            guest_http_smoke_url: None,
            upstream_mappings: Vec::new(),
        })
    }
}

pub fn prepare_frontend_launch(
    config: &FrontendConfig,
    mounts: &[RuntimeMount],
) -> Result<LaunchPreparation, LaunchError> {
    let manifests = write_runtime_manifests(config, mounts)?;
    Ok(LaunchPreparation {
        manifests,
        qemu_command: config.build_microvm_qemu_command(),
    })
}

pub fn prepare_frontend_launch_with_policy(
    config: &FrontendConfig,
    mounts: &[RuntimeMount],
    policy: &VmnetPolicy,
) -> Result<LaunchPreparation, LaunchError> {
    let config_mounts = policy_config_mounts(policy);
    let manifests = write_runtime_manifests_with_config_mounts(config, mounts, &config_mounts)?;
    Ok(LaunchPreparation {
        manifests,
        qemu_command: config.build_microvm_qemu_command(),
    })
}

pub fn run_frontend_until_qemu_exit(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
) -> Result<QemuExit, LaunchError> {
    let policy = VmnetPolicy::default_sandbox(config.network.clone());
    run_frontend_until_qemu_exit_with_policy(config, mounts, policy)
}

pub fn run_frontend_until_qemu_exit_with_policy(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
    policy: VmnetPolicy,
) -> Result<QemuExit, LaunchError> {
    run_frontend_until_qemu_exit_with_policy_and_timeout(config, mounts, policy, None)
}

pub fn run_frontend_until_qemu_exit_with_policy_and_timeout(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
    policy: VmnetPolicy,
    qemu_timeout: Option<Duration>,
) -> Result<QemuExit, LaunchError> {
    let running = start_frontend_with_policy(config, mounts, policy)?;
    running.wait(qemu_timeout)
}

pub struct RunningFrontend {
    config: FrontendConfig,
    policy: VmnetPolicy,
    child: std::process::Child,
    shutting_down: Arc<AtomicBool>,
}

impl RunningFrontend {
    pub fn qemu_pid(&self) -> u32 {
        self.child.id()
    }

    pub fn wait(mut self, qemu_timeout: Option<Duration>) -> Result<QemuExit, LaunchError> {
        let qemu_exit = wait_for_qemu(&mut self.child, qemu_timeout)?;
        self.finish(qemu_exit)
    }

    pub fn terminate(mut self) -> Result<QemuExit, LaunchError> {
        self.shutting_down.store(true, Ordering::SeqCst);
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let status = self.child.wait()?;
        self.finish(QemuExit {
            status,
            timed_out: false,
        })
    }

    fn finish(self, qemu_exit: QemuExit) -> Result<QemuExit, LaunchError> {
        self.shutting_down.store(true, Ordering::SeqCst);
        let state_status = if qemu_exit.timed_out {
            "timed_out"
        } else {
            "exited"
        };
        let qemu_status = qemu_exit.status.to_string();
        write_launch_state(
            &self.config,
            state_status,
            None,
            Some(&qemu_status),
            Some(&self.policy),
        )?;
        Ok(qemu_exit)
    }
}

pub fn start_frontend_with_policy(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
    policy: VmnetPolicy,
) -> Result<RunningFrontend, LaunchError> {
    ensure_data_disk(&config.runtime.data_disk)?;
    validate_launch_inputs(&config)?;
    write_launch_state(&config, "starting", None, None, Some(&policy))?;
    prepare_frontend_launch_with_policy(&config, &mounts, &policy)?;
    remove_stale_socket(&config.runtime.composed_fs_sock)?;
    remove_stale_socket(&config.runtime.config_fs_sock)?;
    remove_stale_socket(&config.runtime.vmnet_sock)?;
    remove_stale_socket(&config.runtime.docker_sock)?;
    remove_stale_socket(&config.runtime.vmnet_event_log)?;

    let shutting_down = Arc::new(AtomicBool::new(false));

    let composed_config = config.composed_fs_server();
    let composed_shutting_down = shutting_down.clone();
    thread::Builder::new()
        .name("agentvm-composed-fs".to_string())
        .spawn(move || {
            if let Err(error) = serve_vhost_user_fs(composed_config) {
                if composed_shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                eprintln!("agentvm composed fs failed: {error}");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.composed_fs_sock, SOCKET_WAIT_TIMEOUT)?;

    let config_fs_config = config.config_fs_server();
    let config_fs_shutting_down = shutting_down.clone();
    thread::Builder::new()
        .name("agentvm-config-fs".to_string())
        .spawn(move || {
            if let Err(error) = serve_vhost_user_fs(config_fs_config) {
                if config_fs_shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                eprintln!("agentvm config fs failed: {error}");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.config_fs_sock, SOCKET_WAIT_TIMEOUT)?;

    let mut vmnet_config = config.vmnet_gateway_config();
    vmnet_config.policy = policy.clone();
    let vmnet_shutting_down = shutting_down.clone();
    thread::Builder::new()
        .name("agentvm-vmnet".to_string())
        .spawn(move || {
            if let Err(error) = serve_vmnet_gateway(vmnet_config) {
                if vmnet_shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                eprintln!("agentvm vmnet gateway failed: {error:?}");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.vmnet_sock, SOCKET_WAIT_TIMEOUT)?;

    if let Some(docker_tcp_port) = docker_listener_tcp_port(&policy) {
        start_docker_unix_proxy(DockerUnixProxyConfig {
            socket_path: config.runtime.docker_sock.clone(),
            tcp_host: std::net::Ipv4Addr::LOCALHOST,
            tcp_port: docker_tcp_port,
        })?;
        wait_for_path(&config.runtime.docker_sock, SOCKET_WAIT_TIMEOUT)?;
    }

    let process = match config.supervisor_plan().qemu {
        crate::ManagedTask::ChildProcess(process) => process,
        _ => unreachable!("supervisor qemu task must be a child process"),
    };
    let qemu_log = File::create(&process.stdout_log)?;
    let child = Command::new(&process.program)
        .args(&process.args)
        .stdout(Stdio::from(qemu_log.try_clone()?))
        .stderr(Stdio::from(qemu_log))
        .spawn()?;
    write_launch_state(&config, "running", Some(child.id()), None, Some(&policy))?;
    Ok(RunningFrontend {
        config,
        policy,
        child,
        shutting_down,
    })
}

#[derive(Debug)]
pub struct QemuExit {
    pub status: ExitStatus,
    pub timed_out: bool,
}

fn wait_for_qemu(
    child: &mut std::process::Child,
    qemu_timeout: Option<Duration>,
) -> Result<QemuExit, LaunchError> {
    let Some(timeout) = qemu_timeout else {
        return child
            .wait()
            .map(|status| QemuExit {
                status,
                timed_out: false,
            })
            .map_err(LaunchError::Io);
    };

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(QemuExit {
                status,
                timed_out: false,
            });
        }
        if started.elapsed() >= timeout {
            child.kill()?;
            return child
                .wait()
                .map(|status| QemuExit {
                    status,
                    timed_out: true,
                })
                .map_err(LaunchError::Io);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn validate_launch_inputs(config: &FrontendConfig) -> Result<(), LaunchError> {
    if !config.runtime.data_disk.exists() {
        return Err(LaunchError::Artifact(format!(
            "Docker data disk is missing: {}",
            config.runtime.data_disk.display()
        )));
    }
    Ok(())
}

fn ensure_data_disk(path: &Path) -> Result<(), LaunchError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let disk = File::create(path)?;
    disk.set_len(DATA_DISK_SIZE_BYTES)?;
    let status = Command::new("mkfs.ext4")
        .arg("-F")
        .arg(path)
        .status()
        .map_err(LaunchError::Io)?;
    if !status.success() {
        return Err(LaunchError::Artifact(format!(
            "mkfs.ext4 failed for {} with status {status}",
            path.display()
        )));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct LaunchState<'a> {
    status: &'a str,
    machine_type: &'a str,
    network_backend: &'a str,
    composed_fs: &'a str,
    qemu_pid: Option<u32>,
    qemu_status: Option<&'a str>,
    egress_default_action: Option<String>,
    egress_reason: Option<String>,
    allow_ip_count: usize,
    allow_domain_count: usize,
    vmnet_socket: String,
    composed_fs_socket: String,
    config_fs_socket: String,
    docker_socket: Option<String>,
}

fn write_launch_state(
    config: &FrontendConfig,
    status: &str,
    qemu_pid: Option<u32>,
    qemu_status: Option<&str>,
    policy: Option<&VmnetPolicy>,
) -> Result<(), LaunchError> {
    fs::create_dir_all(&config.runtime.run_dir)?;
    let snapshot = config.state_snapshot();
    let state = LaunchState {
        status,
        machine_type: &snapshot.machine_type,
        network_backend: &snapshot.network_backend,
        composed_fs: &snapshot.composed_fs,
        qemu_pid,
        qemu_status,
        egress_default_action: policy.map(|policy| format!("{:?}", policy.egress.default_action)),
        egress_reason: policy.map(|policy| format!("{:?}", policy.egress.reason)),
        allow_ip_count: policy.map_or(0, |policy| policy.egress.allow_ips.len()),
        allow_domain_count: policy.map_or(0, |policy| policy.egress.allow_domains.len()),
        vmnet_socket: config.runtime.vmnet_sock.display().to_string(),
        composed_fs_socket: config.runtime.composed_fs_sock.display().to_string(),
        config_fs_socket: config.runtime.config_fs_sock.display().to_string(),
        docker_socket: policy
            .and_then(|policy| docker_listener_tcp_port(policy))
            .map(|_| config.runtime.docker_sock.display().to_string()),
    };
    let bytes = serde_json::to_vec_pretty(&state).map_err(LaunchError::Json)?;
    fs::write(
        &config.runtime.state_json,
        [bytes.as_slice(), b"\n"].concat(),
    )?;
    Ok(())
}

pub fn wait_for_path(path: &Path, timeout: Duration) -> Result<(), LaunchError> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() {
            return Ok(());
        }
        thread::sleep(SOCKET_WAIT_STEP);
    }
    Err(LaunchError::Artifact(format!(
        "timed out waiting for {}",
        path.display()
    )))
}

fn docker_listener_tcp_port(policy: &VmnetPolicy) -> Option<u16> {
    policy
        .host_listeners
        .iter()
        .find(|listener| listener.purpose == HostListenerPurpose::DockerApi)
        .map(|listener| listener.host_port)
}

fn policy_config_mounts(policy: &VmnetPolicy) -> Vec<RuntimeMount> {
    let Some(ca_cert_path) = policy.tls_mitm.ca_cert_path.as_ref() else {
        return Vec::new();
    };
    vec![RuntimeMount {
        id: "m0002_mitm_ca_cert".to_string(),
        host_path: ca_cert_path.clone(),
        guest_path: PathBuf::from("/mitm-ca.crt"),
        readonly: true,
        source_class: ManifestSourceClass::SystemRo,
        required: true,
        bind: false,
    }]
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn resolve_artifact(repo_root: &Path, path: PathBuf) -> Result<PathBuf, LaunchError> {
    let path = if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    };
    if !path.exists() {
        return Err(LaunchError::Artifact(format!(
            "Docker VM artifact is missing: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, LaunchError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(LaunchError::Io)
    }
}

fn computed_guest_network(repo_root: &Path) -> GuestNetwork {
    let digest = fnv1a64(repo_root.display().to_string().as_bytes());
    GuestNetwork {
        guest_mac: format!(
            "02:fc:{:02x}:{:02x}:{:02x}:{:02x}",
            (digest >> 24) & 0xff,
            (digest >> 16) & 0xff,
            (digest >> 8) & 0xff,
            digest & 0xff
        ),
        ..GuestNetwork::default()
    }
}

fn ensure_ttys0_console(cmdline: &str) -> String {
    let mut args = cmdline
        .split_whitespace()
        .filter(|arg| !arg.starts_with("console="))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    args.insert(0, "console=ttyS0,115200n8".to_string());
    args.join(" ")
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_manifest::workspace_mounts;
    use crate::test_support::FrontendFixture;

    #[test]
    fn loads_frontend_config_from_artifact_manifest() {
        let fixture = FrontendFixture::new("artifact-manifest");
        let config = fixture.frontend_config().expect("config");

        assert_eq!(config.vm.cpus, 2);
        assert!(config
            .vm
            .kernel_cmdline
            .starts_with("console=ttyS0,115200n8 "));
        assert!(!config.vm.kernel_cmdline.contains("console=hvc0"));
        assert_eq!(config.vm.virtiofs_tag, "agentvm");
        assert_eq!(
            config.artifacts.kernel,
            fixture.root.join("docker/out/vmlinuz")
        );
        assert_eq!(
            config.runtime.composed_bind_manifest,
            fixture
                .run_dir
                .join("guest-config")
                .join("composed-binds.json")
        );
        assert_eq!(config.guest_http_smoke_url, None);
    }

    #[test]
    fn prepares_launch_without_usernet_or_hostfwd() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        fs::write(root.join("vmlinuz"), b"kernel").expect("kernel");
        fs::write(root.join("initrd.img"), b"initrd").expect("initrd");
        fs::write(root.join("rootfs.raw"), b"rootfs").expect("rootfs");
        let manifest = ArtifactManifest {
            artifacts: ArtifactPaths {
                kernel: root.join("vmlinuz"),
                initrd: root.join("initrd.img"),
                rootfs: root.join("rootfs.raw"),
            },
            vm: ArtifactVm {
                cpus: 2,
                memory_bytes: 2147483648,
                virtiofs_tag: "agentvm".to_string(),
                kernel_cmdline: "console=hvc0 root=/dev/vda".to_string(),
            },
        };
        let config = manifest
            .into_frontend_config(
                root.join("repo"),
                root.join(".sandbox/docker-vm/run"),
                "qemu-system-x86_64",
                &root,
            )
            .expect("config");

        let prep = prepare_frontend_launch(&config, &workspace_mounts(config.project.clone()))
            .expect("prepare");
        let joined = prep.qemu_command.join(" ");

        assert!(prep.manifests.composed_bind_manifest.exists());
        assert!(joined.contains("-netdev stream,"));
        assert!(!joined.contains("-netdev user"));
        assert!(!joined.contains("hostfwd="));
    }

    #[test]
    fn writes_launch_state_snapshot() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        fs::write(root.join("vmlinuz"), b"kernel").expect("kernel");
        fs::write(root.join("initrd.img"), b"initrd").expect("initrd");
        fs::write(root.join("rootfs.raw"), b"rootfs").expect("rootfs");
        let manifest = ArtifactManifest {
            artifacts: ArtifactPaths {
                kernel: root.join("vmlinuz"),
                initrd: root.join("initrd.img"),
                rootfs: root.join("rootfs.raw"),
            },
            vm: ArtifactVm {
                cpus: 1,
                memory_bytes: 1024 * 1024 * 1024,
                virtiofs_tag: "agentvm".to_string(),
                kernel_cmdline: "console=hvc0 root=/dev/vda".to_string(),
            },
        };
        let config = manifest
            .into_frontend_config(
                root.join("repo"),
                root.join(".sandbox/docker-vm/run"),
                "qemu-system-x86_64",
                &root,
            )
            .expect("config");

        let mut policy = VmnetPolicy::default_sandbox(config.network.clone());
        policy
            .host_listeners
            .push(crate::network_policy::HostListener::docker_api(23750, 1075));
        write_launch_state(&config, "running", Some(1234), None, Some(&policy)).expect("state");
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");

        assert!(state.contains("\"status\": \"running\""));
        assert!(state.contains("\"qemu_pid\": 1234"));
        assert!(state.contains("\"network_backend\": \"stream\""));
        assert!(state.contains("\"egress_default_action\": \"Deny\""));
        assert!(state.contains("guest-config.sock"));
        assert!(state.contains("docker.sock"));
    }

    fn unique_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentvm-frontend-launch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }
}
