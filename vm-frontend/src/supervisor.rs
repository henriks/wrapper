use std::fmt;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::{ManagedTask, SupervisorPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupervisorTaskName {
    ComposedFs,
    ConfigFs,
    VmnetGateway,
    DockerProxy,
    Qemu,
}

impl fmt::Display for SupervisorTaskName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ComposedFs => "composed-fs",
            Self::ConfigFs => "config-fs",
            Self::VmnetGateway => "vmnet-gateway",
            Self::DockerProxy => "docker-proxy",
            Self::Qemu => "qemu",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupervisorTaskKind {
    BlockingComposedFs,
    BlockingConfigFs,
    VmnetGateway,
    AsyncDockerProxy,
    ChildProcess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorTaskSpec {
    pub name: SupervisorTaskName,
    pub kind: SupervisorTaskKind,
}

#[derive(Debug)]
struct SupervisorTaskState {
    spec: SupervisorTaskSpec,
    status_tx: watch::Sender<SupervisorTaskStatus>,
    status_rx: watch::Receiver<SupervisorTaskStatus>,
}

#[derive(Debug, Clone)]
pub struct SupervisorTaskController {
    spec: SupervisorTaskSpec,
    status_tx: watch::Sender<SupervisorTaskStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum SupervisorTaskStatus {
    Planned,
    Starting,
    Ready,
    Finished,
    Failed { cause: String },
    Cancelled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorTaskResult {
    pub name: SupervisorTaskName,
    pub status: SupervisorTaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRunSummary {
    pub shutdown: SupervisorShutdown,
    pub task_results: Vec<SupervisorTaskResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shutdown", rename_all = "kebab-case")]
pub enum SupervisorShutdown {
    Running,
    Requested { reason: String },
}

impl SupervisorTaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }
}

impl SupervisorShutdown {
    pub fn is_requested(&self) -> bool {
        matches!(self, Self::Requested { .. })
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Running => None,
            Self::Requested { reason } => Some(reason),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("supervisor shutdown channel closed")]
    ShutdownChannelClosed,
    #[error("task status channel closed for {task}")]
    TaskStatusChannelClosed { task: SupervisorTaskName },
}

pub struct LaunchSupervisor {
    plan: SupervisorPlan,
    tasks: Vec<SupervisorTaskState>,
    shutdown_tx: watch::Sender<SupervisorShutdown>,
    shutdown_rx: watch::Receiver<SupervisorShutdown>,
}

impl LaunchSupervisor {
    pub fn new(plan: SupervisorPlan) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(SupervisorShutdown::Running);
        let specs = task_specs_from_plan(&plan);
        let tasks = specs
            .into_iter()
            .map(|spec| {
                let (status_tx, status_rx) = watch::channel(SupervisorTaskStatus::Planned);
                SupervisorTaskState {
                    spec,
                    status_tx,
                    status_rx,
                }
            })
            .collect();
        Self {
            plan,
            tasks,
            shutdown_tx,
            shutdown_rx,
        }
    }

    pub fn plan(&self) -> &SupervisorPlan {
        &self.plan
    }

    pub fn task_specs(&self) -> Vec<SupervisorTaskSpec> {
        self.tasks.iter().map(|task| task.spec.clone()).collect()
    }

    pub fn task_statuses(&self) -> Vec<SupervisorTaskResult> {
        self.tasks
            .iter()
            .map(|task| SupervisorTaskResult {
                name: task.spec.name,
                status: task.status_rx.borrow().clone(),
            })
            .collect()
    }

    pub fn task_status(&self, name: SupervisorTaskName) -> Option<SupervisorTaskStatus> {
        self.tasks
            .iter()
            .find(|task| task.spec.name == name)
            .map(|task| task.status_rx.borrow().clone())
    }

    pub fn subscribe_task_status(
        &self,
        name: SupervisorTaskName,
    ) -> Option<watch::Receiver<SupervisorTaskStatus>> {
        self.tasks
            .iter()
            .find(|task| task.spec.name == name)
            .map(|task| task.status_rx.clone())
    }

    pub fn task_controller(&self, name: SupervisorTaskName) -> Option<SupervisorTaskController> {
        self.tasks
            .iter()
            .find(|task| task.spec.name == name)
            .map(|task| SupervisorTaskController {
                spec: task.spec.clone(),
                status_tx: task.status_tx.clone(),
            })
    }

    pub fn task_controllers(&self) -> Vec<SupervisorTaskController> {
        self.tasks
            .iter()
            .map(|task| SupervisorTaskController {
                spec: task.spec.clone(),
                status_tx: task.status_tx.clone(),
            })
            .collect()
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<SupervisorShutdown> {
        self.shutdown_rx.clone()
    }

    pub fn current_shutdown(&self) -> SupervisorShutdown {
        self.shutdown_rx.borrow().clone()
    }

    pub fn request_shutdown(&self, reason: impl Into<String>) {
        let _ = self.shutdown_tx.send(SupervisorShutdown::Requested {
            reason: reason.into(),
        });
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_rx.borrow().is_requested()
    }

    pub async fn wait_for_shutdown(&self) -> Result<SupervisorShutdown, SupervisorError> {
        let mut receiver = self.subscribe_shutdown();
        loop {
            let current = receiver.borrow().clone();
            if current.is_requested() {
                return Ok(current);
            }
            receiver
                .changed()
                .await
                .map_err(|_| SupervisorError::ShutdownChannelClosed)?;
        }
    }

    pub async fn planned_task_results(&self) -> Vec<SupervisorTaskResult> {
        self.task_statuses()
    }

    pub async fn run_until_shutdown(&self) -> Result<SupervisorRunSummary, SupervisorError> {
        let shutdown = self.wait_for_shutdown().await?;
        let reason = shutdown.reason().unwrap_or("supervisor shutdown");
        for controller in self.task_controllers() {
            let status = self
                .task_status(controller.name())
                .unwrap_or(SupervisorTaskStatus::Planned);
            if !status.is_terminal() {
                controller.mark_cancelled(reason)?;
            }
        }
        Ok(SupervisorRunSummary {
            shutdown,
            task_results: self.task_statuses(),
        })
    }
}

impl SupervisorTaskController {
    pub fn spec(&self) -> &SupervisorTaskSpec {
        &self.spec
    }

    pub fn name(&self) -> SupervisorTaskName {
        self.spec.name
    }

    pub fn mark_starting(&self) -> Result<(), SupervisorError> {
        self.set_status(SupervisorTaskStatus::Starting)
    }

    pub fn mark_ready(&self) -> Result<(), SupervisorError> {
        self.set_status(SupervisorTaskStatus::Ready)
    }

    pub fn mark_finished(&self) -> Result<(), SupervisorError> {
        self.set_status(SupervisorTaskStatus::Finished)
    }

    pub fn mark_failed(&self, cause: impl Into<String>) -> Result<(), SupervisorError> {
        self.set_status(SupervisorTaskStatus::Failed {
            cause: cause.into(),
        })
    }

    pub fn mark_cancelled(&self, reason: impl Into<String>) -> Result<(), SupervisorError> {
        self.set_status(SupervisorTaskStatus::Cancelled {
            reason: reason.into(),
        })
    }

    pub fn set_status(&self, status: SupervisorTaskStatus) -> Result<(), SupervisorError> {
        self.status_tx
            .send(status)
            .map_err(|_| SupervisorError::TaskStatusChannelClosed {
                task: self.spec.name,
            })
    }
}

fn task_specs_from_plan(plan: &SupervisorPlan) -> Vec<SupervisorTaskSpec> {
    let mut specs = vec![
        task_spec(SupervisorTaskName::ComposedFs, &plan.composed_fs),
        task_spec(SupervisorTaskName::ConfigFs, &plan.config_fs),
        task_spec(SupervisorTaskName::VmnetGateway, &plan.vmnet),
    ];
    if let Some(docker_proxy) = &plan.docker_proxy {
        specs.push(task_spec(SupervisorTaskName::DockerProxy, docker_proxy));
    }
    specs.push(task_spec(SupervisorTaskName::Qemu, &plan.qemu));
    specs
}

fn task_spec(name: SupervisorTaskName, task: &ManagedTask) -> SupervisorTaskSpec {
    let kind = match (name, task) {
        (SupervisorTaskName::ComposedFs, ManagedTask::EmbeddedComposedFs { .. }) => {
            SupervisorTaskKind::BlockingComposedFs
        }
        (SupervisorTaskName::ConfigFs, ManagedTask::EmbeddedComposedFs { .. }) => {
            SupervisorTaskKind::BlockingConfigFs
        }
        (_, ManagedTask::VmnetGateway(_)) => SupervisorTaskKind::VmnetGateway,
        (_, ManagedTask::DockerProxy) => SupervisorTaskKind::AsyncDockerProxy,
        (_, ManagedTask::ChildProcess(_)) => SupervisorTaskKind::ChildProcess,
        (_, ManagedTask::EmbeddedComposedFs { .. }) => SupervisorTaskKind::BlockingComposedFs,
    };
    SupervisorTaskSpec { name, kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FrontendConfig, GuestNetwork, RuntimePaths, ToolPaths, VmArtifacts, VmShape,
        COMPOSED_FS_TAG,
    };
    use std::path::PathBuf;

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
            guest_log_dir: None,
            upstream_mappings: Vec::new(),
        }
    }

    #[test]
    fn supervisor_task_specs_preserve_blocking_service_boundaries() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let specs = supervisor.task_specs();

        assert_eq!(
            specs,
            vec![
                SupervisorTaskSpec {
                    name: SupervisorTaskName::ComposedFs,
                    kind: SupervisorTaskKind::BlockingComposedFs,
                },
                SupervisorTaskSpec {
                    name: SupervisorTaskName::ConfigFs,
                    kind: SupervisorTaskKind::BlockingConfigFs,
                },
                SupervisorTaskSpec {
                    name: SupervisorTaskName::VmnetGateway,
                    kind: SupervisorTaskKind::VmnetGateway,
                },
                SupervisorTaskSpec {
                    name: SupervisorTaskName::Qemu,
                    kind: SupervisorTaskKind::ChildProcess,
                },
            ]
        );
    }

    #[tokio::test]
    async fn supervisor_shutdown_is_observable_by_async_tasks() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());

        assert!(!supervisor.shutdown_requested());
        supervisor.request_shutdown("test shutdown");

        let shutdown = supervisor.wait_for_shutdown().await.expect("shutdown");
        assert_eq!(
            shutdown,
            SupervisorShutdown::Requested {
                reason: "test shutdown".to_string(),
            }
        );
        assert!(supervisor.shutdown_requested());
    }

    #[tokio::test]
    async fn supervisor_reports_planned_task_results_without_spawning_services() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let results = supervisor.planned_task_results().await;

        assert_eq!(results.len(), 4);
        assert!(results
            .iter()
            .all(|result| result.status == SupervisorTaskStatus::Planned));
    }

    #[tokio::test]
    async fn task_controller_publishes_readiness_and_failure() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let controller = supervisor
            .task_controller(SupervisorTaskName::VmnetGateway)
            .expect("vmnet controller");
        let mut status_rx = supervisor
            .subscribe_task_status(SupervisorTaskName::VmnetGateway)
            .expect("vmnet status");

        assert_eq!(controller.name(), SupervisorTaskName::VmnetGateway);
        controller.mark_starting().expect("starting");
        status_rx.changed().await.expect("starting update");
        assert_eq!(*status_rx.borrow(), SupervisorTaskStatus::Starting);

        controller.mark_ready().expect("ready");
        status_rx.changed().await.expect("ready update");
        assert_eq!(*status_rx.borrow(), SupervisorTaskStatus::Ready);
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::VmnetGateway),
            Some(SupervisorTaskStatus::Ready)
        );

        controller.mark_failed("vmnet failed").expect("failed");
        status_rx.changed().await.expect("failed update");
        assert_eq!(
            *status_rx.borrow(),
            SupervisorTaskStatus::Failed {
                cause: "vmnet failed".to_string(),
            }
        );
    }

    #[test]
    fn supervisor_exposes_one_controller_per_planned_task() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        let controllers = supervisor.task_controllers();

        assert_eq!(controllers.len(), 4);
        assert_eq!(
            controllers[0].spec().kind,
            SupervisorTaskKind::BlockingComposedFs
        );
        assert_eq!(
            controllers[1].spec().kind,
            SupervisorTaskKind::BlockingConfigFs
        );
        assert_eq!(controllers[2].spec().kind, SupervisorTaskKind::VmnetGateway);
        assert_eq!(controllers[3].spec().kind, SupervisorTaskKind::ChildProcess);
    }

    #[tokio::test]
    async fn run_until_shutdown_cancels_unfinished_tasks_without_spawning_services() {
        let supervisor = LaunchSupervisor::new(config().supervisor_plan());
        supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller")
            .mark_finished()
            .expect("qemu finished");
        supervisor.request_shutdown("test cancellation");

        let summary = supervisor.run_until_shutdown().await.expect("summary");

        assert_eq!(
            summary.shutdown,
            SupervisorShutdown::Requested {
                reason: "test cancellation".to_string(),
            }
        );
        assert_eq!(summary.task_results.len(), 4);
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::Qemu),
            Some(SupervisorTaskStatus::Finished)
        );
        assert_eq!(
            supervisor.task_status(SupervisorTaskName::VmnetGateway),
            Some(SupervisorTaskStatus::Cancelled {
                reason: "test cancellation".to_string(),
            })
        );
    }
}
