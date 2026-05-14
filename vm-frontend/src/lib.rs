use std::path::PathBuf;

use agentvm_composed_fs::ServeConfig;

use crate::network_policy::VmnetPolicy;
use crate::tcp_gateway::UpstreamMapping;
use crate::vmnet_runtime::{
    VmnetRuntimeConfig, DEFAULT_UPSTREAM_CONNECT_TIMEOUT, DEFAULT_VMNET_IDLE_SLEEP,
};

pub mod dns_proxy;
pub mod docker_proxy;
pub mod guest_tcp;
pub mod host_ingress;
pub mod l2_gateway;
pub mod launch;
pub mod network_policy;
pub mod payload_client;
pub mod runtime_manifest;
pub mod tcp_gateway;
pub mod tcp_proxy;
pub mod tls_mitm;
pub mod vmnet_gateway;
pub mod vmnet_runtime;
pub mod vmnet_stream;

#[cfg(test)]
pub(crate) mod test_support;

pub const MACHINE_MICROVM: &str = "microvm";
pub const COMPOSED_FS_TAG: &str = "agentvm";
pub const CONFIG_FS_TAG: &str = "agentvm-config";
pub const COMPOSED_FS_MOUNTPOINT: &str = "/run/agentvm-host";
pub const VIRTIOFS_QUEUE_SIZE: u16 = 128;
pub const VIRTIOFS_NUM_QUEUES: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub run_dir: PathBuf,
    pub console_log: PathBuf,
    pub state_json: PathBuf,
    pub lock: PathBuf,
    pub data_disk: PathBuf,
    pub composed_fs_manifest: PathBuf,
    pub composed_fs_sock: PathBuf,
    pub config_fs_manifest: PathBuf,
    pub guest_config_dir: PathBuf,
    pub composed_bind_manifest: PathBuf,
    pub config_fs_sock: PathBuf,
    pub vmnet_sock: PathBuf,
    pub vmnet_event_log: PathBuf,
    pub docker_sock: PathBuf,
}

impl RuntimePaths {
    pub fn under(run_dir: impl Into<PathBuf>) -> Self {
        let run_dir = run_dir.into();
        let root_dir = run_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".sandbox/docker-vm"));
        Self {
            console_log: run_dir.join("console.log"),
            state_json: run_dir.join("state.json"),
            lock: root_dir.join("lock"),
            data_disk: root_dir.join("docker-data.raw"),
            composed_fs_manifest: run_dir.join("composed-fs-manifest.json"),
            composed_fs_sock: run_dir.join("virtiofs.sock"),
            config_fs_manifest: run_dir.join("config-fs-manifest.json"),
            guest_config_dir: run_dir.join("guest-config"),
            composed_bind_manifest: run_dir.join("guest-config").join("composed-binds.json"),
            config_fs_sock: run_dir.join("guest-config.sock"),
            vmnet_sock: run_dir.join("vmnet.sock"),
            vmnet_event_log: run_dir.join("vmnet-events.log"),
            docker_sock: run_dir.join("docker.sock"),
            run_dir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPaths {
    pub qemu_system_x86_64: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmArtifacts {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub rootfs: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestNetwork {
    pub guest_ip: String,
    pub gateway_ip: String,
    pub prefix_len: u8,
    pub dns_ip: String,
    pub guest_mac: String,
    pub vmnet_reconnect_ms: u32,
}

impl Default for GuestNetwork {
    fn default() -> Self {
        Self {
            guest_ip: "10.0.2.15".to_string(),
            gateway_ip: "10.0.2.2".to_string(),
            prefix_len: 24,
            dns_ip: "10.0.2.3".to_string(),
            guest_mac: "02:fc:12:34:56:78".to_string(),
            vmnet_reconnect_ms: 250,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmShape {
    pub memory_bytes: u64,
    pub cpus: u16,
    pub kernel_cmdline: String,
    pub virtiofs_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendConfig {
    pub project: PathBuf,
    pub tools: ToolPaths,
    pub artifacts: VmArtifacts,
    pub runtime: RuntimePaths,
    pub vm: VmShape,
    pub network: GuestNetwork,
    pub guest_http_smoke_url: Option<String>,
    pub upstream_mappings: Vec<UpstreamMapping>,
}

impl FrontendConfig {
    pub fn from_artifact_manifest_file(
        project: impl Into<PathBuf>,
        run_dir: impl Into<PathBuf>,
        qemu_system_x86_64: impl Into<PathBuf>,
        artifact_manifest: impl AsRef<std::path::Path>,
    ) -> Result<Self, launch::LaunchError> {
        let project = project.into();
        let manifest_path = artifact_manifest.as_ref();
        let text = std::fs::read_to_string(manifest_path).map_err(launch::LaunchError::Io)?;
        let manifest: launch::ArtifactManifest =
            serde_json::from_str(&text).map_err(launch::LaunchError::Json)?;
        let repo_root = manifest_path
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::parent)
            .unwrap_or_else(|| std::path::Path::new("."));
        manifest.into_frontend_config(project, run_dir, qemu_system_x86_64, repo_root)
    }

    pub fn supervisor_plan(&self) -> SupervisorPlan {
        SupervisorPlan {
            composed_fs: ManagedTask::EmbeddedComposedFs {
                config: self.composed_fs_server(),
            },
            config_fs: ManagedTask::EmbeddedComposedFs {
                config: self.config_fs_server(),
            },
            vmnet: ManagedTask::VmnetGateway(self.vmnet_gateway_config()),
            qemu: ManagedTask::ChildProcess(ProcessSpec {
                program: self.tools.qemu_system_x86_64.clone(),
                args: self
                    .build_microvm_qemu_command()
                    .into_iter()
                    .skip(1)
                    .collect(),
                stdout_log: self.runtime.run_dir.join("qemu.log"),
            }),
            state_path: self.runtime.state_json.clone(),
        }
    }

    pub fn composed_fs_server(&self) -> ServeConfig {
        ServeConfig {
            manifest: self.runtime.composed_fs_manifest.clone(),
            socket_path: self.runtime.composed_fs_sock.clone(),
            tag: self.vm.virtiofs_tag.clone(),
            thread_pool_size: agentvm_composed_fs::DEFAULT_THREAD_POOL_SIZE,
        }
    }

    pub fn config_fs_server(&self) -> ServeConfig {
        ServeConfig {
            manifest: self.runtime.config_fs_manifest.clone(),
            socket_path: self.runtime.config_fs_sock.clone(),
            tag: CONFIG_FS_TAG.to_string(),
            thread_pool_size: agentvm_composed_fs::DEFAULT_THREAD_POOL_SIZE,
        }
    }

    pub fn vmnet_gateway_config(&self) -> VmnetRuntimeConfig {
        let policy = VmnetPolicy::default_sandbox(self.network.clone());
        VmnetRuntimeConfig {
            socket_path: self.runtime.vmnet_sock.clone(),
            event_log_path: Some(self.runtime.vmnet_event_log.clone()),
            upstream_mappings: self.upstream_mappings.clone(),
            network: self.network.clone(),
            policy,
            upstream_connect_timeout: DEFAULT_UPSTREAM_CONNECT_TIMEOUT,
            idle_sleep: DEFAULT_VMNET_IDLE_SLEEP,
        }
    }

    pub fn state_snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            machine_type: MACHINE_MICROVM.to_string(),
            network_backend: "stream".to_string(),
            composed_fs: "embedded".to_string(),
            qemu_device_count: 6,
        }
    }

    pub fn build_microvm_qemu_command(&self) -> Vec<String> {
        let memory_mib = std::cmp::max(1, self.vm.memory_bytes / (1024 * 1024));
        let mut args = vec![
            self.tools.qemu_system_x86_64.display().to_string(),
            "-enable-kvm".to_string(),
            "-cpu".to_string(),
            "host".to_string(),
            "-nodefaults".to_string(),
            "-no-user-config".to_string(),
            "-no-reboot".to_string(),
            "-display".to_string(),
            "none".to_string(),
            "-serial".to_string(),
            format!("file:{}", self.runtime.console_log.display()),
            "-monitor".to_string(),
            "none".to_string(),
            "-machine".to_string(),
            "microvm,acpi=off,memory-backend=mem,isa-serial=on".to_string(),
            "-object".to_string(),
            format!(
                "memory-backend-memfd,id=mem,size={},share=on",
                self.vm.memory_bytes
            ),
            "-m".to_string(),
            memory_mib.to_string(),
            "-smp".to_string(),
            self.vm.cpus.to_string(),
            "-kernel".to_string(),
            self.artifacts.kernel.display().to_string(),
            "-initrd".to_string(),
            self.artifacts.initrd.display().to_string(),
            "-append".to_string(),
            self.kernel_cmdline(),
            "-drive".to_string(),
            format!(
                "if=none,file={},format=raw,readonly=on,id=rootfs",
                self.artifacts.rootfs.display()
            ),
            "-device".to_string(),
            "virtio-blk-device,drive=rootfs".to_string(),
            "-drive".to_string(),
            format!(
                "if=none,file={},format=raw,id=dockerdata",
                self.runtime.data_disk.display()
            ),
            "-device".to_string(),
            "virtio-blk-device,drive=dockerdata".to_string(),
            "-object".to_string(),
            "rng-random,id=rng0,filename=/dev/urandom".to_string(),
            "-device".to_string(),
            "virtio-rng-device,rng=rng0".to_string(),
        ];

        self.push_virtiofs_device(
            &mut args,
            "charfs",
            &self.runtime.composed_fs_sock,
            &self.vm.virtiofs_tag,
        );
        self.push_virtiofs_device(
            &mut args,
            "charcfg",
            &self.runtime.config_fs_sock,
            CONFIG_FS_TAG,
        );

        args.extend([
            "-netdev".to_string(),
            self.stream_netdev_arg(),
            "-device".to_string(),
            format!(
                "virtio-net-device,netdev=net0,mac={}",
                self.network.guest_mac
            ),
        ]);
        args
    }

    fn push_virtiofs_device(
        &self,
        args: &mut Vec<String>,
        chardev_id: &str,
        socket_path: &PathBuf,
        tag: &str,
    ) {
        args.extend([
            "-chardev".to_string(),
            format!("socket,id={chardev_id},path={}", socket_path.display()),
            "-device".to_string(),
            format!(
                "vhost-user-fs-device,chardev={chardev_id},tag={tag},queue-size={VIRTIOFS_QUEUE_SIZE},num-request-queues={VIRTIOFS_NUM_QUEUES}"
            ),
        ]);
    }

    fn kernel_cmdline(&self) -> String {
        let mut cmdline = self.vm.kernel_cmdline.clone();
        append_kernel_arg(
            &mut cmdline,
            "agentvm_project",
            &self.project.display().to_string(),
        );
        append_kernel_arg(&mut cmdline, "agentvm_guest_ip", &self.network.guest_ip);
        append_kernel_arg(&mut cmdline, "agentvm_gateway_ip", &self.network.gateway_ip);
        append_kernel_arg(
            &mut cmdline,
            "agentvm_prefix_len",
            &self.network.prefix_len.to_string(),
        );
        append_kernel_arg(&mut cmdline, "agentvm_dns", &self.network.dns_ip);
        append_kernel_arg(&mut cmdline, "agentvm_guest_mac", &self.network.guest_mac);
        if let Some(url) = &self.guest_http_smoke_url {
            append_kernel_arg(&mut cmdline, "agentvm_http_smoke_url", url);
        }
        cmdline
    }

    fn stream_netdev_arg(&self) -> String {
        format!(
            "stream,id=net0,server=off,addr.type=unix,addr.path={},reconnect-ms={}",
            self.runtime.vmnet_sock.display(),
            self.network.vmnet_reconnect_ms
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSnapshot {
    pub machine_type: String,
    pub network_backend: String,
    pub composed_fs: String,
    pub qemu_device_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorPlan {
    pub composed_fs: ManagedTask,
    pub config_fs: ManagedTask,
    pub vmnet: ManagedTask,
    pub qemu: ManagedTask,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedTask {
    EmbeddedComposedFs { config: ServeConfig },
    VmnetGateway(VmnetRuntimeConfig),
    ChildProcess(ProcessSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub stdout_log: PathBuf,
}

fn append_kernel_arg(cmdline: &mut String, key: &str, value: &str) {
    if !cmdline.is_empty() {
        cmdline.push(' ');
    }
    cmdline.push_str(key);
    cmdline.push('=');
    cmdline.push_str(value);
}

#[cfg(test)]
mod tests {
    use super::*;

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
                kernel_cmdline: "console=ttyS0,115200n8 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet".to_string(),
                virtiofs_tag: COMPOSED_FS_TAG.to_string(),
            },
            network: GuestNetwork::default(),
            guest_http_smoke_url: None,
            upstream_mappings: Vec::new(),
        }
    }

    #[test]
    fn builds_microvm_stream_command_without_usernet_or_hostfwd() {
        let command = config().build_microvm_qemu_command();
        let joined = command.join(" ");

        assert!(joined.contains("-machine microvm,acpi=off,memory-backend=mem,isa-serial=on"));
        assert!(joined.contains("-device virtio-net-device,netdev=net0,mac=02:fc:12:34:56:78"));
        assert!(joined.contains("-netdev stream,id=net0,server=off,addr.type=unix,addr.path=/repo/.sandbox/docker-vm/run/vmnet.sock,reconnect-ms=250"));
        assert!(joined.contains("vhost-user-fs-device,chardev=charfs,tag=agentvm"));
        assert!(joined.contains("vhost-user-fs-device,chardev=charcfg,tag=agentvm-config"));
        assert!(!joined.contains("hostfwd="));
        assert!(!joined.contains("-netdev user"));
    }

    #[test]
    fn can_add_guest_http_smoke_url_to_kernel_cmdline() {
        let mut config = config();
        config.guest_http_smoke_url = Some("http://93.184.216.34/".to_string());

        let command = config.build_microvm_qemu_command();
        let joined = command.join(" ");

        assert!(joined.contains("agentvm_http_smoke_url=http://93.184.216.34/"));
    }

    #[test]
    fn exposes_composed_fs_embedding_configs() {
        let config = config();

        assert_eq!(config.composed_fs_server().tag, "agentvm");
        assert_eq!(
            config.composed_fs_server().socket_path,
            PathBuf::from("/repo/.sandbox/docker-vm/run/virtiofs.sock")
        );
        assert_eq!(config.config_fs_server().tag, "agentvm-config");
        assert_eq!(
            config.config_fs_server().socket_path,
            PathBuf::from("/repo/.sandbox/docker-vm/run/guest-config.sock")
        );
    }

    #[test]
    fn exposes_vmnet_runtime_config_for_supervisor() {
        let vmnet = config().vmnet_gateway_config();

        assert_eq!(
            vmnet.socket_path,
            PathBuf::from("/repo/.sandbox/docker-vm/run/vmnet.sock")
        );
        assert_eq!(vmnet.network, GuestNetwork::default());
        assert_eq!(vmnet.policy.assignment, GuestNetwork::default());
        assert_eq!(
            vmnet.upstream_connect_timeout,
            DEFAULT_UPSTREAM_CONNECT_TIMEOUT
        );
        assert_eq!(vmnet.idle_sleep, DEFAULT_VMNET_IDLE_SLEEP);
    }

    #[test]
    fn state_snapshot_marks_stream_and_embedded_fs() {
        let snapshot = config().state_snapshot();

        assert_eq!(snapshot.machine_type, "microvm");
        assert_eq!(snapshot.network_backend, "stream");
        assert_eq!(snapshot.composed_fs, "embedded");
        assert_eq!(snapshot.qemu_device_count, 6);
    }

    #[test]
    fn supervisor_plan_has_explicit_managed_tasks_and_logs() {
        let plan = config().supervisor_plan();

        assert!(matches!(
            plan.composed_fs,
            ManagedTask::EmbeddedComposedFs { .. }
        ));
        match plan.vmnet {
            ManagedTask::VmnetGateway(vmnet) => {
                assert_eq!(
                    vmnet.socket_path,
                    PathBuf::from("/repo/.sandbox/docker-vm/run/vmnet.sock")
                );
            }
            _ => panic!("vmnet should be a gateway runtime task"),
        }
        match plan.qemu {
            ManagedTask::ChildProcess(process) => {
                assert_eq!(process.program, PathBuf::from("qemu-system-x86_64"));
                assert_eq!(
                    process.stdout_log,
                    PathBuf::from("/repo/.sandbox/docker-vm/run/qemu.log")
                );
                assert!(process.args.iter().any(|arg| arg == "-enable-kvm"));
            }
            _ => panic!("qemu should be a child process task"),
        }
        assert_eq!(
            plan.state_path,
            PathBuf::from("/repo/.sandbox/docker-vm/run/state.json")
        );
    }
}
