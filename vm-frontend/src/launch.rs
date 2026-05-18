use std::fs::{self, File};
use std::future::Future;
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
use thiserror::Error;
use tokio::process::{Child as TokioChild, Command as TokioCommand};
use tokio::sync::watch;
use tracing::{debug, error, info, info_span, warn};
use wait_timeout::ChildExt;

use crate::docker_proxy::{
    run_docker_unix_proxy_async, start_docker_unix_proxy, DockerUnixProxyConfig,
    DockerUnixProxyLimits,
};
use crate::network_policy::{HostListenerPurpose, VmnetPolicy};
use crate::runtime_manifest::{
    write_runtime_manifests, write_runtime_manifests_with_config_mounts, ManifestSourceClass,
    RuntimeManifestSummary, RuntimeMount,
};
use crate::supervisor::{
    LaunchSupervisor, SupervisorShutdown, SupervisorTaskController, SupervisorTaskName,
};
use crate::supervisor_control::{
    bind_control_socket, control_socket_path, serve_control_listener_until_shutdown,
};
use crate::vmnet_runtime::serve_vmnet_gateway;
use crate::{
    FrontendConfig, GuestNetwork, ManagedTask, ProcessSpec, RuntimePaths, ToolPaths, VmArtifacts,
    VmShape,
};

const SOCKET_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WAIT_STEP: Duration = Duration::from_millis(20);
const STATE_DISK_SIZE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("{0}")]
    Io(io::Error),
    #[error("{0}")]
    Json(serde_json::Error),
    #[error("{0}")]
    Artifact(String),
}

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
    info!(
        run_dir = %config.runtime.run_dir.display(),
        qemu_timeout_seconds = qemu_timeout.map(|timeout| timeout.as_secs()),
        "running frontend until qemu exits"
    );
    let running = start_frontend_with_policy(config, mounts, policy)?;
    running.wait(qemu_timeout)
}

pub async fn run_frontend_until_qemu_exit_with_policy_and_timeout_async(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
    policy: VmnetPolicy,
    qemu_timeout: Option<Duration>,
) -> Result<QemuExit, LaunchError> {
    info!(
        run_dir = %config.runtime.run_dir.display(),
        qemu_timeout_seconds = qemu_timeout.map(|timeout| timeout.as_secs()),
        "running frontend asynchronously until qemu exits"
    );
    let state_disk = config.runtime.state_disk.clone();
    tokio::task::spawn_blocking(move || ensure_state_disk(&state_disk))
        .await
        .map_err(join_launch_error)??;
    validate_launch_inputs(&config)?;
    write_launch_state(&config, "starting", None, None, Some(&policy))?;
    prepare_frontend_launch_with_policy(&config, &mounts, &policy)?;
    remove_stale_socket(&config.runtime.composed_fs_sock)?;
    remove_stale_socket(&config.runtime.config_fs_sock)?;
    remove_stale_socket(&config.runtime.vmnet_sock)?;
    remove_stale_socket(&config.runtime.docker_sock)?;
    remove_stale_socket(&config.runtime.vmnet_event_log)?;
    remove_stale_socket(&control_socket_path(&config.runtime))?;

    let docker_tcp_port = docker_listener_tcp_port(&policy);
    let mut supervisor_plan = config.supervisor_plan();
    if docker_tcp_port.is_some() {
        supervisor_plan.docker_proxy = Some(ManagedTask::DockerProxy);
    }
    let supervisor = Arc::new(LaunchSupervisor::new(supervisor_plan));
    let control_socket = control_socket_path(&config.runtime);
    let control_listener =
        bind_control_socket(&control_socket).map_err(supervisor_control_error)?;
    let control_server = tokio::spawn(serve_control_listener_until_shutdown(
        control_listener,
        Arc::clone(&supervisor),
    ));
    let mut services = Vec::new();

    let composed_controller =
        required_task_controller(&supervisor, SupervisorTaskName::ComposedFs)?;
    let composed_config = config.composed_fs_server();
    services.push(
        spawn_supervised_blocking_service_until_ready(
            &config.runtime.composed_fs_sock,
            SOCKET_WAIT_TIMEOUT,
            &composed_controller,
            move || serve_vhost_user_fs(composed_config).map_err(|error| error.to_string()),
        )
        .await?,
    );

    let config_fs_controller = required_task_controller(&supervisor, SupervisorTaskName::ConfigFs)?;
    let config_fs_config = config.config_fs_server();
    services.push(
        spawn_supervised_blocking_service_until_ready(
            &config.runtime.config_fs_sock,
            SOCKET_WAIT_TIMEOUT,
            &config_fs_controller,
            move || serve_vhost_user_fs(config_fs_config).map_err(|error| error.to_string()),
        )
        .await?,
    );

    let vmnet_controller = required_task_controller(&supervisor, SupervisorTaskName::VmnetGateway)?;
    let mut vmnet_config = config.vmnet_gateway_config();
    vmnet_config.policy = policy.clone();
    services.push(
        spawn_supervised_blocking_service_until_ready(
            &config.runtime.vmnet_sock,
            SOCKET_WAIT_TIMEOUT,
            &vmnet_controller,
            move || {
                serve_vmnet_gateway(vmnet_config)
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))
            },
        )
        .await?,
    );

    if let Some(docker_tcp_port) = docker_tcp_port {
        let docker_controller =
            required_task_controller(&supervisor, SupervisorTaskName::DockerProxy)?;
        let docker_config = DockerUnixProxyConfig {
            socket_path: config.runtime.docker_sock.clone(),
            tcp_host: std::net::Ipv4Addr::LOCALHOST,
            tcp_port: docker_tcp_port,
        };
        services.push(
            spawn_supervised_async_service_until_ready(
                &config.runtime.docker_sock,
                SOCKET_WAIT_TIMEOUT,
                &docker_controller,
                move |shutdown| async move {
                    run_docker_unix_proxy_async(
                        docker_config,
                        DockerUnixProxyLimits::default(),
                        shutdown,
                    )
                    .await
                    .map_err(|error| error.to_string())
                },
            )
            .await?,
        );
    }

    let process = match &supervisor.plan().qemu {
        crate::ManagedTask::ChildProcess(process) => process.clone(),
        _ => unreachable!("supervisor qemu task must be a child process"),
    };
    let qemu_controller = required_task_controller(&supervisor, SupervisorTaskName::Qemu)?;
    let qemu_result = run_supervised_qemu_process_with_services_and_shutdown_async(
        &config,
        &policy,
        &process,
        qemu_timeout,
        &qemu_controller,
        services,
        Some(supervisor.subscribe_shutdown()),
    )
    .await;
    if !supervisor.shutdown_requested() {
        supervisor.request_shutdown("launch completed");
    }
    let control_result = control_server.await.map_err(|error| {
        LaunchError::Artifact(format!("supervisor control task failed: {error}"))
    })?;
    control_result.map_err(supervisor_control_error)?;
    qemu_result
}

pub struct RunningFrontend {
    config: FrontendConfig,
    policy: VmnetPolicy,
    child: std::process::Child,
    shutting_down: Arc<AtomicBool>,
    finished: bool,
}

impl RunningFrontend {
    pub fn qemu_pid(&self) -> u32 {
        self.child.id()
    }

    pub fn wait(mut self, qemu_timeout: Option<Duration>) -> Result<QemuExit, LaunchError> {
        info!(
            qemu_pid = self.child.id(),
            qemu_timeout_seconds = qemu_timeout.map(|timeout| timeout.as_secs()),
            "waiting for qemu"
        );
        let qemu_exit = wait_for_qemu(&mut self.child, qemu_timeout)?;
        self.finish(qemu_exit)
    }

    pub fn terminate(mut self) -> Result<QemuExit, LaunchError> {
        self.shutting_down.store(true, Ordering::SeqCst);
        info!(qemu_pid = self.child.id(), "terminating frontend");
        if self.child.try_wait()?.is_none() {
            self.child.kill()?;
        }
        let status = self.child.wait()?;
        self.finish(QemuExit {
            status,
            timed_out: false,
        })
    }

    fn finish(mut self, qemu_exit: QemuExit) -> Result<QemuExit, LaunchError> {
        self.shutting_down.store(true, Ordering::SeqCst);
        let state_status = if qemu_exit.timed_out {
            "timed_out"
        } else {
            "exited"
        };
        let qemu_status = qemu_exit.status.to_string();
        info!(
            qemu_status = %qemu_exit.status,
            timed_out = qemu_exit.timed_out,
            launch_state = state_status,
            "frontend finished"
        );
        write_launch_state(
            &self.config,
            state_status,
            None,
            Some(&qemu_status),
            Some(&self.policy),
        )?;
        self.finished = true;
        Ok(qemu_exit)
    }
}

impl Drop for RunningFrontend {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.shutting_down.store(true, Ordering::SeqCst);
        let status = match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => {
                let _ = self.child.kill();
                self.child.wait().ok().map(|status| status.to_string())
            }
            Err(error) => Some(format!("cleanup status unavailable: {error}")),
        };
        warn!(
            qemu_status = status.as_deref(),
            "dropping unfinished frontend; wrote terminated launch state"
        );
        let _ = write_launch_state(
            &self.config,
            "terminated",
            None,
            status.as_deref(),
            Some(&self.policy),
        );
    }
}

pub fn start_frontend_with_policy(
    config: FrontendConfig,
    mounts: Vec<RuntimeMount>,
    policy: VmnetPolicy,
) -> Result<RunningFrontend, LaunchError> {
    let span = info_span!(
        "frontend.launch",
        run_dir = %config.runtime.run_dir.display(),
        state_disk = %config.runtime.state_disk.display(),
        vmnet_socket = %config.runtime.vmnet_sock.display(),
    );
    let _span_guard = span.enter();
    info!(mount_count = mounts.len(), "starting frontend services");
    ensure_state_disk(&config.runtime.state_disk)?;
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
                error!(service = "composed-fs", %error, "service failed");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.composed_fs_sock, SOCKET_WAIT_TIMEOUT)?;
    info!(
        service = "composed-fs",
        socket = %config.runtime.composed_fs_sock.display(),
        "service ready"
    );

    let config_fs_config = config.config_fs_server();
    let config_fs_shutting_down = shutting_down.clone();
    thread::Builder::new()
        .name("agentvm-config-fs".to_string())
        .spawn(move || {
            if let Err(error) = serve_vhost_user_fs(config_fs_config) {
                if config_fs_shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                error!(service = "config-fs", %error, "service failed");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.config_fs_sock, SOCKET_WAIT_TIMEOUT)?;
    info!(
        service = "config-fs",
        socket = %config.runtime.config_fs_sock.display(),
        "service ready"
    );

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
                error!(service = "vmnet-gateway", ?error, "service failed");
            }
        })
        .map_err(LaunchError::Io)?;
    wait_for_path(&config.runtime.vmnet_sock, SOCKET_WAIT_TIMEOUT)?;
    info!(
        service = "vmnet-gateway",
        socket = %config.runtime.vmnet_sock.display(),
        "service ready"
    );

    if let Some(docker_tcp_port) = docker_listener_tcp_port(&policy) {
        start_docker_unix_proxy(DockerUnixProxyConfig {
            socket_path: config.runtime.docker_sock.clone(),
            tcp_host: std::net::Ipv4Addr::LOCALHOST,
            tcp_port: docker_tcp_port,
        })?;
        wait_for_path(&config.runtime.docker_sock, SOCKET_WAIT_TIMEOUT)?;
        info!(
            service = "docker-unix-proxy",
            socket = %config.runtime.docker_sock.display(),
            tcp_port = docker_tcp_port,
            "service ready"
        );
    }

    let process = match config.supervisor_plan().qemu {
        crate::ManagedTask::ChildProcess(process) => process,
        _ => unreachable!("supervisor qemu task must be a child process"),
    };
    let qemu_log = File::create(&process.stdout_log)?;
    debug!(
        program = %process.program.display(),
        arg_count = process.args.len(),
        stdout_log = %process.stdout_log.display(),
        "spawning qemu"
    );
    let child = Command::new(&process.program)
        .args(&process.args)
        .stdout(Stdio::from(qemu_log.try_clone()?))
        .stderr(Stdio::from(qemu_log))
        .spawn()?;
    info!(qemu_pid = child.id(), "qemu started");
    write_launch_state(&config, "running", Some(child.id()), None, Some(&policy))?;
    Ok(RunningFrontend {
        config,
        policy,
        child,
        shutting_down,
        finished: false,
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

    match child.wait_timeout(timeout).map_err(LaunchError::Io)? {
        Some(status) => {
            info!(%status, "qemu exited");
            Ok(QemuExit {
                status,
                timed_out: false,
            })
        }
        None => {
            warn!(
                timeout_seconds = timeout.as_secs(),
                "qemu timed out; killing process"
            );
            child.kill().map_err(LaunchError::Io)?;
            child
                .wait()
                .map(|status| QemuExit {
                    status,
                    timed_out: true,
                })
                .map_err(LaunchError::Io)
        }
    }
}

pub async fn run_qemu_process_async(
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
) -> Result<QemuExit, LaunchError> {
    let mut child = spawn_qemu_process_async(process).await?;
    wait_for_qemu_async(&mut child, qemu_timeout).await
}

pub async fn run_supervised_qemu_process_async(
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
) -> Result<QemuExit, LaunchError> {
    if controller.name() != SupervisorTaskName::Qemu {
        return Err(qemu_controller_mismatch(controller));
    }
    controller
        .mark_starting()
        .map_err(supervisor_launch_error)?;
    let mut child = match spawn_qemu_process_async(process).await {
        Ok(child) => child,
        Err(error) => {
            let _ = controller.mark_failed(error.to_string());
            return Err(error);
        }
    };
    controller.mark_ready().map_err(supervisor_launch_error)?;
    finish_supervised_qemu_wait(&mut child, qemu_timeout, controller).await
}

pub async fn run_supervised_qemu_process_with_state_async(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
) -> Result<QemuExit, LaunchError> {
    let mut child =
        spawn_supervised_qemu_process_with_state_async(config, policy, process, controller).await?;
    let exit = finish_supervised_qemu_wait(&mut child, qemu_timeout, controller).await?;
    write_qemu_exit_state(config, policy, &exit)?;
    Ok(exit)
}

pub async fn run_supervised_qemu_process_with_service_async(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
    service: SupervisedBlockingService,
) -> Result<QemuExit, LaunchError> {
    run_supervised_qemu_process_with_services_async(
        config,
        policy,
        process,
        qemu_timeout,
        controller,
        vec![service],
    )
    .await
}

pub async fn run_supervised_qemu_process_with_services_async(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
    services: Vec<SupervisedBlockingService>,
) -> Result<QemuExit, LaunchError> {
    run_supervised_qemu_process_with_services_and_shutdown_async(
        config,
        policy,
        process,
        qemu_timeout,
        controller,
        services,
        None,
    )
    .await
}

pub async fn run_supervised_qemu_process_with_services_and_shutdown_async(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    process: &ProcessSpec,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
    services: Vec<SupervisedBlockingService>,
    shutdown_rx: Option<watch::Receiver<SupervisorShutdown>>,
) -> Result<QemuExit, LaunchError> {
    let mut child =
        spawn_supervised_qemu_process_with_state_async(config, policy, process, controller).await?;
    let completion = wait_for_qemu_or_service_failure(
        config,
        policy,
        &mut child,
        qemu_timeout,
        controller,
        services,
        shutdown_rx,
    )
    .await?;
    match completion {
        SupervisedQemuCompletion::Exit(exit) => {
            write_qemu_exit_state(config, policy, &exit)?;
            Ok(exit)
        }
        SupervisedQemuCompletion::SupervisorShutdown { exit, reason } => {
            write_supervisor_shutdown_state(config, policy, &exit, &reason)?;
            Ok(exit)
        }
    }
}

async fn spawn_supervised_qemu_process_with_state_async(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    process: &ProcessSpec,
    controller: &SupervisorTaskController,
) -> Result<TokioChild, LaunchError> {
    if controller.name() != SupervisorTaskName::Qemu {
        return Err(qemu_controller_mismatch(controller));
    }
    controller
        .mark_starting()
        .map_err(supervisor_launch_error)?;
    let child = match spawn_qemu_process_async(process).await {
        Ok(child) => child,
        Err(error) => {
            let _ = controller.mark_failed(error.to_string());
            return Err(error);
        }
    };
    write_launch_state(config, "running", child.id(), None, Some(policy))?;
    controller.mark_ready().map_err(supervisor_launch_error)?;
    Ok(child)
}

async fn finish_supervised_qemu_wait(
    child: &mut TokioChild,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
) -> Result<QemuExit, LaunchError> {
    match wait_for_qemu_async(child, qemu_timeout).await {
        Ok(exit) => {
            publish_supervised_qemu_exit(controller, &exit)?;
            Ok(exit)
        }
        Err(error) => {
            let _ = controller.mark_failed(error.to_string());
            Err(error)
        }
    }
}

enum SupervisedQemuCompletion {
    Exit(QemuExit),
    SupervisorShutdown { exit: QemuExit, reason: String },
}

async fn wait_for_qemu_or_service_failure(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    child: &mut TokioChild,
    qemu_timeout: Option<Duration>,
    controller: &SupervisorTaskController,
    services: Vec<SupervisedBlockingService>,
    shutdown_rx: Option<watch::Receiver<SupervisorShutdown>>,
) -> Result<SupervisedQemuCompletion, LaunchError> {
    let service_controllers = service_controllers(&services);
    let service_completion = wait_for_first_service_completion(services);
    let shutdown = wait_for_optional_supervisor_shutdown(shutdown_rx);
    tokio::pin!(service_completion);
    tokio::pin!(shutdown);
    if let Some(timeout) = qemu_timeout {
        tokio::select! {
            status = child.wait() => {
                let exit = QemuExit { status: status.map_err(LaunchError::Io)?, timed_out: false };
                publish_supervised_qemu_exit(controller, &exit)?;
                cancel_unfinished_services(&service_controllers, None, "qemu exited");
                Ok(SupervisedQemuCompletion::Exit(exit))
            }
            _ = tokio::time::sleep(timeout) => {
                let exit = kill_timed_out_qemu(child).await?;
                publish_supervised_qemu_exit(controller, &exit)?;
                cancel_unfinished_services(&service_controllers, None, "qemu timed out");
                Ok(SupervisedQemuCompletion::Exit(exit))
            }
            service_result = &mut service_completion => {
                let (service_name, service_result) = service_result?;
                cancel_unfinished_services(&service_controllers, Some(service_name), "peer service failed");
                terminate_qemu_after_service_failure(config, policy, child, controller, service_name, service_result).await.map(SupervisedQemuCompletion::Exit)
            }
            shutdown = &mut shutdown => {
                let shutdown = shutdown.expect("shutdown future is pending forever when no receiver is configured");
                terminate_qemu_after_supervisor_shutdown(child, controller, &service_controllers, shutdown).await
            }
        }
    } else {
        tokio::select! {
            status = child.wait() => {
                let exit = QemuExit { status: status.map_err(LaunchError::Io)?, timed_out: false };
                publish_supervised_qemu_exit(controller, &exit)?;
                cancel_unfinished_services(&service_controllers, None, "qemu exited");
                Ok(SupervisedQemuCompletion::Exit(exit))
            }
            service_result = &mut service_completion => {
                let (service_name, service_result) = service_result?;
                cancel_unfinished_services(&service_controllers, Some(service_name), "peer service failed");
                terminate_qemu_after_service_failure(config, policy, child, controller, service_name, service_result).await.map(SupervisedQemuCompletion::Exit)
            }
            shutdown = &mut shutdown => {
                let shutdown = shutdown.expect("shutdown future is pending forever when no receiver is configured");
                terminate_qemu_after_supervisor_shutdown(child, controller, &service_controllers, shutdown).await
            }
        }
    }
}

fn service_controllers(
    services: &[SupervisedBlockingService],
) -> Vec<(
    SupervisorTaskName,
    SupervisorTaskController,
    Option<watch::Sender<bool>>,
)> {
    services
        .iter()
        .map(|service| {
            (
                service.name(),
                service.controller.clone(),
                service.shutdown_tx.clone(),
            )
        })
        .collect()
}

fn cancel_unfinished_services(
    services: &[(
        SupervisorTaskName,
        SupervisorTaskController,
        Option<watch::Sender<bool>>,
    )],
    skip: Option<SupervisorTaskName>,
    reason: &str,
) {
    for (name, controller, shutdown_tx) in services {
        if Some(*name) == skip {
            continue;
        }
        if let Some(shutdown_tx) = shutdown_tx {
            let _ = shutdown_tx.send(true);
        }
        let _ = controller.mark_cancelled(reason);
    }
}

async fn wait_for_first_service_completion(
    services: Vec<SupervisedBlockingService>,
) -> Result<(SupervisorTaskName, Result<(), LaunchError>), LaunchError> {
    if services.is_empty() {
        std::future::pending::<()>().await;
    }
    let mut tasks = tokio::task::JoinSet::new();
    for service in services {
        let name = service.name();
        tasks.spawn(async move { (name, service.wait().await) });
    }
    tasks
        .join_next()
        .await
        .ok_or_else(|| {
            LaunchError::Artifact(
                "all supervised service monitors exited without result".to_string(),
            )
        })?
        .map_err(|error| LaunchError::Artifact(format!("service monitor join failed: {error}")))
}

async fn kill_timed_out_qemu(child: &mut TokioChild) -> Result<QemuExit, LaunchError> {
    child.start_kill().map_err(LaunchError::Io)?;
    child
        .wait()
        .await
        .map(|status| QemuExit {
            status,
            timed_out: true,
        })
        .map_err(LaunchError::Io)
}

async fn wait_for_optional_supervisor_shutdown(
    mut shutdown_rx: Option<watch::Receiver<SupervisorShutdown>>,
) -> Option<SupervisorShutdown> {
    let Some(receiver) = shutdown_rx.as_mut() else {
        std::future::pending::<()>().await;
        return None;
    };
    loop {
        let current = receiver.borrow().clone();
        if current.is_requested() {
            return Some(current);
        }
        if receiver.changed().await.is_err() {
            return None;
        }
    }
}

async fn terminate_qemu_after_supervisor_shutdown(
    child: &mut TokioChild,
    controller: &SupervisorTaskController,
    service_controllers: &[(
        SupervisorTaskName,
        SupervisorTaskController,
        Option<watch::Sender<bool>>,
    )],
    shutdown: SupervisorShutdown,
) -> Result<SupervisedQemuCompletion, LaunchError> {
    let reason = shutdown
        .reason()
        .unwrap_or("supervisor shutdown requested")
        .to_string();
    cancel_unfinished_services(service_controllers, None, &reason);
    let _ = controller.mark_cancelled(reason.clone());
    let status = if let Some(status) = child.try_wait().map_err(LaunchError::Io)? {
        status
    } else {
        child.start_kill().map_err(LaunchError::Io)?;
        child.wait().await.map_err(LaunchError::Io)?
    };
    Ok(SupervisedQemuCompletion::SupervisorShutdown {
        exit: QemuExit {
            status,
            timed_out: false,
        },
        reason,
    })
}

async fn terminate_qemu_after_service_failure(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    child: &mut TokioChild,
    controller: &SupervisorTaskController,
    service_name: SupervisorTaskName,
    service_result: Result<(), LaunchError>,
) -> Result<QemuExit, LaunchError> {
    let cause = match service_result {
        Ok(()) => format!("{service_name} exited while qemu was running"),
        Err(error) => error.to_string(),
    };
    let _ = controller.mark_failed(format!("service failure: {cause}"));
    let status = if let Some(status) = child.try_wait().map_err(LaunchError::Io)? {
        status
    } else {
        child.start_kill().map_err(LaunchError::Io)?;
        child.wait().await.map_err(LaunchError::Io)?
    };
    let qemu_status = format!("terminated after service failure ({cause}); qemu status: {status}");
    write_launch_state(
        config,
        "service_failed",
        None,
        Some(&qemu_status),
        Some(policy),
    )?;
    Err(LaunchError::Artifact(cause))
}

fn publish_supervised_qemu_exit(
    controller: &SupervisorTaskController,
    exit: &QemuExit,
) -> Result<(), LaunchError> {
    if exit.timed_out {
        controller
            .mark_failed("qemu timed out")
            .map_err(supervisor_launch_error)
    } else {
        controller.mark_finished().map_err(supervisor_launch_error)
    }
}

fn write_qemu_exit_state(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    exit: &QemuExit,
) -> Result<(), LaunchError> {
    let state_status = if exit.timed_out {
        "timed_out"
    } else {
        "exited"
    };
    let qemu_status = exit.status.to_string();
    write_launch_state(config, state_status, None, Some(&qemu_status), Some(policy))
}

fn write_supervisor_shutdown_state(
    config: &FrontendConfig,
    policy: &VmnetPolicy,
    exit: &QemuExit,
    reason: &str,
) -> Result<(), LaunchError> {
    let qemu_status = format!(
        "supervisor shutdown requested ({reason}); qemu status: {}",
        exit.status
    );
    write_launch_state(config, "terminated", None, Some(&qemu_status), Some(policy))
}

fn qemu_controller_mismatch(controller: &SupervisorTaskController) -> LaunchError {
    LaunchError::Artifact(format!(
        "qemu process supervisor received {} task controller",
        controller.name()
    ))
}

pub async fn spawn_qemu_process_async(process: &ProcessSpec) -> Result<TokioChild, LaunchError> {
    if let Some(parent) = process.stdout_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let qemu_log = File::create(&process.stdout_log)?;
    debug!(
        program = %process.program.display(),
        arg_count = process.args.len(),
        stdout_log = %process.stdout_log.display(),
        "spawning qemu asynchronously"
    );
    TokioCommand::new(&process.program)
        .args(&process.args)
        .stdout(Stdio::from(qemu_log.try_clone()?))
        .stderr(Stdio::from(qemu_log))
        .spawn()
        .map_err(LaunchError::Io)
}

pub async fn wait_for_qemu_async(
    child: &mut TokioChild,
    qemu_timeout: Option<Duration>,
) -> Result<QemuExit, LaunchError> {
    let Some(timeout) = qemu_timeout else {
        return child
            .wait()
            .await
            .map(|status| QemuExit {
                status,
                timed_out: false,
            })
            .map_err(LaunchError::Io);
    };

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            info!(%status, "qemu exited");
            Ok(QemuExit {
                status,
                timed_out: false,
            })
        }
        Ok(Err(error)) => Err(LaunchError::Io(error)),
        Err(_) => {
            if let Some(status) = child.try_wait().map_err(LaunchError::Io)? {
                info!(%status, "qemu exited after async timeout boundary");
                return Ok(QemuExit {
                    status,
                    timed_out: false,
                });
            }
            warn!(
                timeout_seconds = timeout.as_secs(),
                "qemu timed out asynchronously; killing process"
            );
            child.start_kill().map_err(LaunchError::Io)?;
            child
                .wait()
                .await
                .map(|status| QemuExit {
                    status,
                    timed_out: true,
                })
                .map_err(LaunchError::Io)
        }
    }
}

fn supervisor_launch_error(error: impl std::fmt::Display) -> LaunchError {
    LaunchError::Artifact(format!("launch supervisor status update failed: {error}"))
}

fn supervisor_control_error(error: impl std::fmt::Display) -> LaunchError {
    LaunchError::Artifact(format!("launch supervisor control failed: {error}"))
}

fn join_launch_error(error: tokio::task::JoinError) -> LaunchError {
    LaunchError::Artifact(format!("blocking launch task failed: {error}"))
}

fn required_task_controller(
    supervisor: &LaunchSupervisor,
    name: SupervisorTaskName,
) -> Result<SupervisorTaskController, LaunchError> {
    supervisor
        .task_controller(name)
        .ok_or_else(|| LaunchError::Artifact(format!("missing launch supervisor task: {name}")))
}

fn validate_launch_inputs(config: &FrontendConfig) -> Result<(), LaunchError> {
    if !config.runtime.state_disk.exists() {
        return Err(LaunchError::Artifact(format!(
            "VM state disk is missing: {}",
            config.runtime.state_disk.display()
        )));
    }
    Ok(())
}

fn ensure_state_disk(path: &Path) -> Result<(), LaunchError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let disk = File::create(path)?;
    disk.set_len(STATE_DISK_SIZE_BYTES)?;
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
    state_disk: String,
    qemu_log: String,
    console_log: String,
    vmnet_event_log: String,
    composed_fs_manifest: String,
    config_fs_manifest: String,
    composed_bind_manifest: String,
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
        state_disk: config.runtime.state_disk.display().to_string(),
        qemu_log: config
            .runtime
            .run_dir
            .join("qemu.log")
            .display()
            .to_string(),
        console_log: config.runtime.console_log.display().to_string(),
        vmnet_event_log: config.runtime.vmnet_event_log.display().to_string(),
        composed_fs_manifest: config.runtime.composed_fs_manifest.display().to_string(),
        config_fs_manifest: config.runtime.config_fs_manifest.display().to_string(),
        composed_bind_manifest: config.runtime.composed_bind_manifest.display().to_string(),
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
    Err(path_wait_timeout(path))
}

pub async fn wait_for_path_async(path: &Path, timeout: Duration) -> Result<(), LaunchError> {
    wait_for_path_async_with_step(path, timeout, SOCKET_WAIT_STEP).await
}

async fn wait_for_path_async_with_step(
    path: &Path,
    timeout: Duration,
    step: Duration,
) -> Result<(), LaunchError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.exists() {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(path_wait_timeout(path));
        }
        tokio::time::sleep(std::cmp::min(step, deadline.saturating_duration_since(now))).await;
    }
}

pub async fn wait_for_service_ready_async(
    path: &Path,
    timeout: Duration,
    controller: &SupervisorTaskController,
) -> Result<(), LaunchError> {
    match wait_for_path_async(path, timeout).await {
        Ok(()) => {
            controller.mark_ready().map_err(supervisor_launch_error)?;
            Ok(())
        }
        Err(error) => {
            let _ = controller.mark_failed(error.to_string());
            Err(error)
        }
    }
}

#[derive(Debug)]
pub struct SupervisedBlockingService {
    controller: SupervisorTaskController,
    handle: tokio::task::JoinHandle<Result<(), String>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl SupervisedBlockingService {
    pub fn name(&self) -> SupervisorTaskName {
        self.controller.name()
    }

    pub async fn wait(self) -> Result<(), LaunchError> {
        match self.handle.await {
            Ok(Ok(())) => {
                self.controller
                    .mark_finished()
                    .map_err(supervisor_launch_error)?;
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = self.controller.mark_failed(error.clone());
                Err(LaunchError::Artifact(error))
            }
            Err(error) => {
                let failure = format!("service task join failed: {error}");
                let _ = self.controller.mark_failed(failure.clone());
                Err(LaunchError::Artifact(failure))
            }
        }
    }
}

pub async fn spawn_supervised_blocking_service_until_ready<F>(
    ready_path: &Path,
    timeout: Duration,
    controller: &SupervisorTaskController,
    service: F,
) -> Result<SupervisedBlockingService, LaunchError>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    let handle = tokio::task::spawn_blocking(service);
    wait_for_supervised_service_ready(ready_path, timeout, controller, handle, None).await
}

pub async fn spawn_supervised_async_service_until_ready<F, Fut>(
    ready_path: &Path,
    timeout: Duration,
    controller: &SupervisorTaskController,
    service: F,
) -> Result<SupervisedBlockingService, LaunchError>
where
    F: FnOnce(watch::Receiver<bool>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(service(shutdown_rx));
    wait_for_supervised_service_ready(ready_path, timeout, controller, handle, Some(shutdown_tx))
        .await
}

async fn wait_for_supervised_service_ready(
    ready_path: &Path,
    timeout: Duration,
    controller: &SupervisorTaskController,
    mut handle: tokio::task::JoinHandle<Result<(), String>>,
    shutdown_tx: Option<watch::Sender<bool>>,
) -> Result<SupervisedBlockingService, LaunchError> {
    controller
        .mark_starting()
        .map_err(supervisor_launch_error)?;
    let ready = wait_for_path_async(ready_path, timeout);
    tokio::pin!(ready);

    tokio::select! {
        ready_result = &mut ready => match ready_result {
            Ok(()) => {
                controller.mark_ready().map_err(supervisor_launch_error)?;
                Ok(SupervisedBlockingService {
                    controller: controller.clone(),
                    handle,
                    shutdown_tx,
                })
            }
            Err(error) => {
                if let Some(shutdown_tx) = &shutdown_tx {
                    let _ = shutdown_tx.send(true);
                }
                handle.abort();
                let _ = controller.mark_failed(error.to_string());
                Err(error)
            }
        },
        service_result = &mut handle => {
            let failure = match service_result {
                Ok(Ok(())) => "service exited before readiness".to_string(),
                Ok(Err(error)) => error,
                Err(error) => format!("service task join failed: {error}"),
            };
            let _ = controller.mark_failed(failure.clone());
            Err(LaunchError::Artifact(failure))
        }
    }
}

fn path_wait_timeout(path: &Path) -> LaunchError {
    LaunchError::Artifact(format!("timed out waiting for {}", path.display()))
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
    use std::ops::Deref;

    struct TestTempDir {
        dir: tempfile::TempDir,
    }

    impl TestTempDir {
        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.dir.path().join(path)
        }
    }

    impl Deref for TestTempDir {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.dir.path()
        }
    }

    impl AsRef<Path> for TestTempDir {
        fn as_ref(&self) -> &Path {
            self.dir.path()
        }
    }
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
    fn drop_running_frontend_kills_child_and_records_cleanup_state() {
        let config = minimal_frontend_config("drop-cleanup");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let child = Command::new("sh")
            .arg("-c")
            .arg("while true; do sleep 1; done")
            .spawn()
            .expect("spawn fake qemu");
        let pid = child.id();
        let shutting_down = Arc::new(AtomicBool::new(false));

        drop(RunningFrontend {
            config: config.clone(),
            policy,
            child,
            shutting_down: shutting_down.clone(),
            finished: false,
        });

        assert!(shutting_down.load(Ordering::SeqCst));
        assert_process_exited(pid);
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"terminated\""), "{state}");
    }

    #[test]
    fn waited_running_frontend_is_not_overwritten_by_drop_cleanup() {
        let config = minimal_frontend_config("wait-finished");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("spawn fake qemu");
        let running = RunningFrontend {
            config: config.clone(),
            policy,
            child,
            shutting_down: Arc::new(AtomicBool::new(false)),
            finished: false,
        };

        let exit = running.wait(None).expect("wait fake qemu");

        assert!(!exit.status.success());
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"exited\""), "{state}");
        assert!(!state.contains("\"status\": \"terminated\""), "{state}");
    }

    #[test]
    fn explicit_terminate_marks_finished_and_reaps_child() {
        let config = minimal_frontend_config("explicit-terminate");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let child = Command::new("sh")
            .arg("-c")
            .arg("while true; do sleep 1; done")
            .spawn()
            .expect("spawn fake qemu");
        let pid = child.id();
        let shutting_down = Arc::new(AtomicBool::new(false));
        let running = RunningFrontend {
            config: config.clone(),
            policy,
            child,
            shutting_down: shutting_down.clone(),
            finished: false,
        };

        let _ = running.terminate().expect("terminate fake qemu");

        assert!(shutting_down.load(Ordering::SeqCst));
        assert_process_exited(pid);
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"exited\""), "{state}");
        assert!(!state.contains("\"status\": \"terminated\""), "{state}");
    }

    #[test]
    fn wait_timeout_kills_child_and_records_timed_out_state() {
        let config = minimal_frontend_config("wait-timeout");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let child = Command::new("sh")
            .arg("-c")
            .arg("while true; do sleep 1; done")
            .spawn()
            .expect("spawn fake qemu");
        let pid = child.id();
        let running = RunningFrontend {
            config: config.clone(),
            policy,
            child,
            shutting_down: Arc::new(AtomicBool::new(false)),
            finished: false,
        };

        let exit = running
            .wait(Some(Duration::from_millis(1)))
            .expect("timeout fake qemu");

        assert!(exit.timed_out);
        assert_process_exited(pid);
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"timed_out\""), "{state}");
        assert!(!state.contains("\"status\": \"terminated\""), "{state}");
    }

    #[tokio::test]
    async fn async_qemu_process_exits_with_status() {
        let root = unique_temp_dir();
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "exit 7".to_string()],
            stdout_log: root.join("qemu.log"),
        };

        let exit = run_qemu_process_async(&process, None)
            .await
            .expect("async qemu exit");

        assert_eq!(exit.status.code(), Some(7));
        assert!(!exit.timed_out);
        assert!(process.stdout_log.exists());
    }

    #[tokio::test]
    async fn async_qemu_timeout_kills_child() {
        let root = unique_temp_dir();
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: root.join("qemu.log"),
        };

        let exit = run_qemu_process_async(&process, Some(Duration::from_millis(20)))
            .await
            .expect("async qemu timeout");

        assert!(exit.timed_out);
        assert!(!exit.status.success());
    }

    #[tokio::test]
    async fn supervised_async_qemu_marks_finished_on_exit() {
        let config = minimal_frontend_config("supervised-qemu-exit");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-supervised.log"),
        };

        let exit = run_supervised_qemu_process_async(&process, None, &controller)
            .await
            .expect("supervised qemu exit");

        assert!(exit.status.success());
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Finished)
        );
    }

    #[tokio::test]
    async fn supervised_async_qemu_marks_failed_on_timeout() {
        let config = minimal_frontend_config("supervised-qemu-timeout");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-supervised.log"),
        };

        let exit = run_supervised_qemu_process_async(
            &process,
            Some(Duration::from_millis(20)),
            &controller,
        )
        .await
        .expect("supervised qemu timeout");

        assert!(exit.timed_out);
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "qemu timed out".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_async_qemu_with_state_records_exit() {
        let config = minimal_frontend_config("supervised-qemu-state-exit");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-supervised-state.log"),
        };

        let exit = run_supervised_qemu_process_with_state_async(
            &config,
            &policy,
            &process,
            None,
            &controller,
        )
        .await
        .expect("supervised qemu state exit");

        assert!(exit.status.success());
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"exited\""), "{state}");
        assert!(state.contains("\"qemu_status\":"), "{state}");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Finished)
        );
    }

    #[tokio::test]
    async fn supervised_async_qemu_with_state_records_timeout() {
        let config = minimal_frontend_config("supervised-qemu-state-timeout");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-supervised-state.log"),
        };

        let exit = run_supervised_qemu_process_with_state_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_millis(20)),
            &controller,
        )
        .await
        .expect("supervised qemu state timeout");

        assert!(exit.timed_out);
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"timed_out\""), "{state}");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "qemu timed out".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_async_qemu_shutdown_request_terminates_child_and_records_state() {
        let config = minimal_frontend_config("supervised-qemu-control-shutdown");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let shutdown_rx = supervisor.subscribe_shutdown();
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-supervised-shutdown.log"),
        };
        supervisor.request_shutdown("control socket shutdown");

        let exit = run_supervised_qemu_process_with_services_and_shutdown_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_secs(10)),
            &controller,
            Vec::new(),
            Some(shutdown_rx),
        )
        .await
        .expect("supervised qemu shutdown");

        assert!(!exit.timed_out);
        assert!(!exit.status.success());
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"terminated\""), "{state}");
        assert!(state.contains("control socket shutdown"), "{state}");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Cancelled {
                reason: "control socket shutdown".to_string()
            })
        );
    }

    #[test]
    fn repeated_runner_after_drop_cleanup_can_update_state() {
        let config = minimal_frontend_config("repeat-after-drop");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let first = Command::new("sh")
            .arg("-c")
            .arg("while true; do sleep 1; done")
            .spawn()
            .expect("spawn first fake qemu");
        let first_pid = first.id();
        drop(RunningFrontend {
            config: config.clone(),
            policy: policy.clone(),
            child: first,
            shutting_down: Arc::new(AtomicBool::new(false)),
            finished: false,
        });
        assert_process_exited(first_pid);

        let second = Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("spawn second fake qemu");
        let running = RunningFrontend {
            config: config.clone(),
            policy,
            child: second,
            shutting_down: Arc::new(AtomicBool::new(false)),
            finished: false,
        };

        let exit = running.wait(None).expect("wait second fake qemu");

        assert!(exit.status.success());
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"exited\""), "{state}");
        assert!(!state.contains("\"status\": \"terminated\""), "{state}");
    }

    #[tokio::test]
    async fn async_service_readiness_marks_ready_when_path_exists() {
        let root = unique_temp_dir();
        let ready_path = root.join("ready.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("service-ready");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::ComposedFs)
            .expect("composed-fs controller");
        controller.mark_starting().expect("starting");

        wait_for_service_ready_async(&ready_path, Duration::from_secs(1), &controller)
            .await
            .expect("ready");

        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ComposedFs),
            Some(crate::supervisor::SupervisorTaskStatus::Ready)
        );
    }

    #[tokio::test]
    async fn async_service_readiness_marks_failed_on_timeout() {
        let root = unique_temp_dir();
        let missing_path = root.join("missing.sock");
        let config = minimal_frontend_config("service-timeout");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config-fs controller");
        controller.mark_starting().expect("starting");

        let error =
            wait_for_service_ready_async(&missing_path, Duration::from_millis(20), &controller)
                .await
                .expect_err("timeout");

        assert!(error.to_string().contains("timed out waiting"));
        let Some(crate::supervisor::SupervisorTaskStatus::Failed { cause }) =
            supervisor.task_status(SupervisorTaskName::ConfigFs)
        else {
            panic!("expected failed config-fs status");
        };
        assert!(cause.contains("timed out waiting"));
    }

    #[tokio::test]
    async fn supervised_blocking_service_marks_ready_and_returns_handle() {
        let root = unique_temp_dir();
        let ready_path = root.join("blocking-ready.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("blocking-service-ready");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::ComposedFs)
            .expect("composed-fs controller");

        let handle = spawn_supervised_blocking_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &controller,
            || {
                std::thread::sleep(Duration::from_millis(20));
                Ok(())
            },
        )
        .await
        .expect("service ready");

        assert_eq!(handle.name(), SupervisorTaskName::ComposedFs);
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ComposedFs),
            Some(crate::supervisor::SupervisorTaskStatus::Ready)
        );
        handle.wait().await.expect("service ok");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ComposedFs),
            Some(crate::supervisor::SupervisorTaskStatus::Finished)
        );
    }

    #[tokio::test]
    async fn supervised_blocking_service_marks_failed_after_readiness() {
        let root = unique_temp_dir();
        let ready_path = root.join("blocking-ready-then-fail.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("blocking-service-late-failure");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::VmnetGateway)
            .expect("vmnet controller");

        let handle = spawn_supervised_blocking_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &controller,
            || {
                std::thread::sleep(Duration::from_millis(20));
                Err("vmnet failed after ready".to_string())
            },
        )
        .await
        .expect("service ready before failure");

        let error = handle.wait().await.expect_err("late service failure");
        assert_eq!(error.to_string(), "vmnet failed after ready");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::VmnetGateway),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "vmnet failed after ready".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_qemu_with_service_records_service_failure() {
        let root = unique_temp_dir();
        let ready_path = root.join("service-ready-before-failure.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("qemu-service-failure");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let service_controller = supervisor
            .task_controller(SupervisorTaskName::VmnetGateway)
            .expect("vmnet controller");
        let qemu_controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let service = spawn_supervised_blocking_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &service_controller,
            || {
                std::thread::sleep(Duration::from_millis(20));
                Err("vmnet failed while qemu running".to_string())
            },
        )
        .await
        .expect("service ready");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-service-failure.log"),
        };

        let error = run_supervised_qemu_process_with_service_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_secs(10)),
            &qemu_controller,
            service,
        )
        .await
        .expect_err("service failure stops qemu");

        assert_eq!(error.to_string(), "vmnet failed while qemu running");
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"service_failed\""), "{state}");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "service failure: vmnet failed while qemu running".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_qemu_with_services_records_first_service_failure() {
        let root = unique_temp_dir();
        let config_ready_path = root.join("config-ready.sock");
        let vmnet_ready_path = root.join("vmnet-ready.sock");
        fs::write(&config_ready_path, b"ready").expect("config ready marker");
        fs::write(&vmnet_ready_path, b"ready").expect("vmnet ready marker");
        let config = minimal_frontend_config("qemu-services-failure");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let config_controller = supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config controller");
        let vmnet_controller = supervisor
            .task_controller(SupervisorTaskName::VmnetGateway)
            .expect("vmnet controller");
        let qemu_controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let config_service = spawn_supervised_blocking_service_until_ready(
            &config_ready_path,
            Duration::from_secs(1),
            &config_controller,
            || {
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            },
        )
        .await
        .expect("config ready");
        let vmnet_service = spawn_supervised_blocking_service_until_ready(
            &vmnet_ready_path,
            Duration::from_secs(1),
            &vmnet_controller,
            || {
                std::thread::sleep(Duration::from_millis(20));
                Err("vmnet failed first".to_string())
            },
        )
        .await
        .expect("vmnet ready");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config.runtime.run_dir.join("qemu-services-failure.log"),
        };

        let error = run_supervised_qemu_process_with_services_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_secs(10)),
            &qemu_controller,
            vec![config_service, vmnet_service],
        )
        .await
        .expect_err("first service failure stops qemu");

        assert_eq!(error.to_string(), "vmnet failed first");
        let state = fs::read_to_string(config.runtime.state_json).expect("state json");
        assert!(state.contains("\"status\": \"service_failed\""), "{state}");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::VmnetGateway),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "vmnet failed first".to_string()
            })
        );
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ConfigFs),
            Some(crate::supervisor::SupervisorTaskStatus::Cancelled {
                reason: "peer service failed".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_qemu_exit_cancels_unfinished_services() {
        let root = unique_temp_dir();
        let ready_path = root.join("service-ready-for-qemu-exit.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("qemu-exit-cancels-services");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let service_controller = supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config controller");
        let qemu_controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let service = spawn_supervised_blocking_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &service_controller,
            || {
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            },
        )
        .await
        .expect("service ready");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            stdout_log: config
                .runtime
                .run_dir
                .join("qemu-exit-cancels-services.log"),
        };

        let exit = run_supervised_qemu_process_with_services_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_secs(10)),
            &qemu_controller,
            vec![service],
        )
        .await
        .expect("qemu exits");

        assert!(exit.status.success());
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ConfigFs),
            Some(crate::supervisor::SupervisorTaskStatus::Cancelled {
                reason: "qemu exited".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_qemu_exit_signals_async_service_shutdown() {
        let root = unique_temp_dir();
        let ready_path = root.join("async-service-ready-for-qemu-exit.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("qemu-exit-cancels-async-service");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let mut plan = config.supervisor_plan();
        plan.docker_proxy = Some(crate::ManagedTask::DockerProxy);
        let supervisor = crate::supervisor::LaunchSupervisor::new(plan);
        let service_controller = supervisor
            .task_controller(SupervisorTaskName::DockerProxy)
            .expect("docker proxy controller");
        let qemu_controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let (shutdown_seen_tx, shutdown_seen_rx) = tokio::sync::oneshot::channel();
        let service = spawn_supervised_async_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &service_controller,
            move |mut shutdown| async move {
                let _ = shutdown.changed().await;
                let _ = shutdown_seen_tx.send(*shutdown.borrow());
                Ok(())
            },
        )
        .await
        .expect("async service ready");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            stdout_log: config
                .runtime
                .run_dir
                .join("qemu-exit-cancels-async-service.log"),
        };

        let exit = run_supervised_qemu_process_with_services_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_secs(10)),
            &qemu_controller,
            vec![service],
        )
        .await
        .expect("qemu exits");

        assert!(exit.status.success());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), shutdown_seen_rx)
                .await
                .expect("shutdown signal")
                .expect("shutdown value"),
            true
        );
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::DockerProxy),
            Some(crate::supervisor::SupervisorTaskStatus::Cancelled {
                reason: "qemu exited".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_qemu_timeout_cancels_unfinished_services() {
        let root = unique_temp_dir();
        let ready_path = root.join("service-ready-for-qemu-timeout.sock");
        fs::write(&ready_path, b"ready").expect("ready marker");
        let config = minimal_frontend_config("qemu-timeout-cancels-services");
        let policy = VmnetPolicy::default_sandbox(config.network.clone());
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let service_controller = supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config controller");
        let qemu_controller = supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller");
        let service = spawn_supervised_blocking_service_until_ready(
            &ready_path,
            Duration::from_secs(1),
            &service_controller,
            || {
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            },
        )
        .await
        .expect("service ready");
        let process = ProcessSpec {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), "while true; do sleep 1; done".to_string()],
            stdout_log: config
                .runtime
                .run_dir
                .join("qemu-timeout-cancels-services.log"),
        };

        let exit = run_supervised_qemu_process_with_services_async(
            &config,
            &policy,
            &process,
            Some(Duration::from_millis(20)),
            &qemu_controller,
            vec![service],
        )
        .await
        .expect("qemu times out");

        assert!(exit.timed_out);
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ConfigFs),
            Some(crate::supervisor::SupervisorTaskStatus::Cancelled {
                reason: "qemu timed out".to_string()
            })
        );
    }

    #[tokio::test]
    async fn supervised_blocking_service_marks_failed_when_service_exits_before_ready() {
        let root = unique_temp_dir();
        let missing_path = root.join("missing-ready.sock");
        let config = minimal_frontend_config("blocking-service-failed");
        let supervisor = crate::supervisor::LaunchSupervisor::new(config.supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::ConfigFs)
            .expect("config-fs controller");

        let error = spawn_supervised_blocking_service_until_ready(
            &missing_path,
            Duration::from_secs(1),
            &controller,
            || Err("config-fs failed".to_string()),
        )
        .await
        .expect_err("service failure");

        assert_eq!(error.to_string(), "config-fs failed");
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::ConfigFs),
            Some(crate::supervisor::SupervisorTaskStatus::Failed {
                cause: "config-fs failed".to_string()
            })
        );
    }

    #[test]
    fn stale_socket_cleanup_removes_existing_paths_and_ignores_missing() {
        let root = unique_temp_dir();
        let stale = root.join("stale.sock");
        fs::write(&stale, b"stale").expect("stale file");

        remove_stale_socket(&stale).expect("remove stale file");
        remove_stale_socket(&stale).expect("ignore missing stale file");

        assert!(!stale.exists());
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
        assert!(state.contains("state.raw"));
        assert!(state.contains("qemu.log"));
        assert!(state.contains("console.log"));
        assert!(state.contains("vmnet-events.log"));
        assert!(state.contains("composed-fs-manifest.json"));
        assert!(state.contains("config-fs-manifest.json"));
        assert!(state.contains("composed-binds.json"));
    }

    fn minimal_frontend_config(name: &str) -> FrontendConfig {
        let root = unique_temp_dir();
        let root_path = root.dir.keep();
        fs::create_dir_all(root_path.join("repo")).expect("repo");
        fs::write(root_path.join("vmlinuz"), b"kernel").expect("kernel");
        fs::write(root_path.join("initrd.img"), b"initrd").expect("initrd");
        fs::write(root_path.join("rootfs.raw"), b"rootfs").expect("rootfs");
        ArtifactManifest {
            artifacts: ArtifactPaths {
                kernel: root_path.join("vmlinuz"),
                initrd: root_path.join("initrd.img"),
                rootfs: root_path.join("rootfs.raw"),
            },
            vm: ArtifactVm {
                cpus: 1,
                memory_bytes: 1024 * 1024 * 1024,
                virtiofs_tag: "agentvm".to_string(),
                kernel_cmdline: "console=hvc0 root=/dev/vda".to_string(),
            },
        }
        .into_frontend_config(
            root_path.join("repo"),
            root_path.join(".sandbox/docker-vm").join(name),
            "qemu-system-x86_64",
            &root_path,
        )
        .expect("config")
    }

    fn assert_process_exited(pid: u32) {
        for _ in 0..50 {
            if !process_exists(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("process {pid} is still running");
    }

    fn process_exists(pid: u32) -> bool {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn unique_temp_dir() -> TestTempDir {
        TestTempDir {
            dir: tempfile::Builder::new()
                .prefix("agentvm-frontend-launch-test-")
                .tempdir()
                .expect("temp dir"),
        }
    }
}
