use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{FrontendConfig, COMPOSED_FS_MOUNTPOINT};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestSourceClass {
    Workspace,
    ToolState,
    AuthConfig,
    SystemRo,
    UserRo,
    UserRw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMount {
    pub id: String,
    pub host_path: PathBuf,
    pub guest_path: PathBuf,
    pub readonly: bool,
    pub source_class: ManifestSourceClass,
    pub required: bool,
    pub bind: bool,
}

impl RuntimeMount {
    pub fn workspace(project: impl Into<PathBuf>) -> Self {
        let project = project.into();
        Self {
            id: "m0001_workspace".to_string(),
            host_path: project.clone(),
            guest_path: project,
            readonly: false,
            source_class: ManifestSourceClass::Workspace,
            required: true,
            bind: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeManifestSummary {
    pub composed_fs_manifest: PathBuf,
    pub composed_bind_manifest: PathBuf,
    pub config_fs_manifest: PathBuf,
    pub mount_count: usize,
}

#[derive(Debug, Serialize)]
struct HostManifest {
    schema_version: u32,
    export_tag: String,
    created_by: String,
    mounts: Vec<HostMount>,
    synthetic: SyntheticSpec,
    protected_guest_paths: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct HostMount {
    id: String,
    guest_path: String,
    host_path: String,
    kind: &'static str,
    access: &'static str,
    source_class: ManifestSourceClass,
    required: bool,
    bind: bool,
    metadata: MetadataSpec,
}

#[derive(Debug, Serialize)]
struct MetadataSpec {
    uid_gid: &'static str,
    permissions: &'static str,
}

#[derive(Debug, Serialize)]
struct SyntheticSpec {
    uid: u32,
    gid: u32,
    dir_mode: &'static str,
}

#[derive(Debug, Serialize)]
struct BindManifest {
    schema_version: u32,
    composed_mountpoint: &'static str,
    entries: Vec<BindEntry>,
}

#[derive(Debug, Serialize)]
struct BindEntry {
    mount_id: String,
    kind: &'static str,
    source: String,
    target: String,
    required: bool,
    create_parent: bool,
}

pub fn workspace_mounts(project: impl Into<PathBuf>) -> Vec<RuntimeMount> {
    vec![RuntimeMount::workspace(project)]
}

pub fn write_runtime_manifests(
    config: &FrontendConfig,
    mounts: &[RuntimeMount],
) -> io::Result<RuntimeManifestSummary> {
    write_runtime_manifests_with_config_mounts(config, mounts, &[])
}

pub fn write_runtime_manifests_with_config_mounts(
    config: &FrontendConfig,
    mounts: &[RuntimeMount],
    config_mounts: &[RuntimeMount],
) -> io::Result<RuntimeManifestSummary> {
    fs::create_dir_all(&config.runtime.run_dir)?;
    fs::create_dir_all(&config.runtime.guest_config_dir)?;

    let host_mounts = validated_host_mounts(mounts)?;
    if host_mounts.is_empty() {
        return Err(invalid_input(
            "runtime manifest must expose at least one mount",
        ));
    }

    let bind_entries = host_mounts
        .iter()
        .filter(|mount| mount.bind)
        .map(|mount| BindEntry {
            mount_id: mount.id.clone(),
            kind: mount.kind,
            source: format!("{COMPOSED_FS_MOUNTPOINT}{}", mount.guest_path),
            target: mount.guest_path.clone(),
            required: mount.required,
            create_parent: true,
        })
        .collect();

    let host_manifest = HostManifest {
        schema_version: 1,
        export_tag: config.vm.virtiofs_tag.clone(),
        created_by: "agentvm-frontend".to_string(),
        mounts: host_mounts,
        synthetic: SyntheticSpec {
            uid: 0,
            gid: 0,
            dir_mode: "0555",
        },
        protected_guest_paths: protected_guest_paths(),
    };
    let bind_manifest = BindManifest {
        schema_version: 1,
        composed_mountpoint: COMPOSED_FS_MOUNTPOINT,
        entries: bind_entries,
    };

    write_json(&config.runtime.composed_fs_manifest, &host_manifest)?;
    write_json(&config.runtime.composed_bind_manifest, &bind_manifest)?;
    write_config_fs_manifest(config, config_mounts)?;

    Ok(RuntimeManifestSummary {
        composed_fs_manifest: config.runtime.composed_fs_manifest.clone(),
        composed_bind_manifest: config.runtime.composed_bind_manifest.clone(),
        config_fs_manifest: config.runtime.config_fs_manifest.clone(),
        mount_count: mounts.len(),
    })
}

fn validated_host_mounts(mounts: &[RuntimeMount]) -> io::Result<Vec<HostMount>> {
    let mut ids = HashSet::new();
    let mut guest_paths = HashSet::new();
    let mut host_mounts = Vec::with_capacity(mounts.len());
    for mount in mounts {
        if mount.id.is_empty() {
            return Err(invalid_input("mount id cannot be empty"));
        }
        if !ids.insert(mount.id.clone()) {
            return Err(invalid_input(format!("duplicate mount id: {}", mount.id)));
        }
        let guest_path = normalize_absolute_path(&mount.guest_path, "guest_path")?;
        if protected_guest_paths()
            .iter()
            .any(|path| path == &guest_path)
        {
            return Err(invalid_input(format!(
                "mount targets protected guest path: {guest_path}"
            )));
        }
        if !guest_paths.insert(guest_path.clone()) {
            return Err(invalid_input(format!(
                "duplicate composed filesystem guest path: {guest_path}"
            )));
        }
        let host_path = normalize_absolute_path(&mount.host_path, "host_path")?;
        if !mount.host_path.exists() {
            if mount.required {
                return Err(invalid_input(format!(
                    "required composed filesystem source is missing: {host_path}"
                )));
            }
            continue;
        }
        let metadata = mount.host_path.metadata()?;
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        host_mounts.push(HostMount {
            id: mount.id.clone(),
            guest_path,
            host_path,
            kind,
            access: if mount.readonly { "ro" } else { "rw" },
            source_class: mount.source_class,
            required: mount.required,
            bind: mount.bind,
            metadata: MetadataSpec {
                uid_gid: "host",
                permissions: "host",
            },
        });
    }
    Ok(host_mounts)
}

fn write_config_fs_manifest(
    config: &FrontendConfig,
    extra_mounts: &[RuntimeMount],
) -> io::Result<()> {
    let mount = RuntimeMount {
        id: "m0001_composed_binds".to_string(),
        host_path: config.runtime.composed_bind_manifest.clone(),
        guest_path: PathBuf::from("/composed-binds.json"),
        readonly: true,
        source_class: ManifestSourceClass::SystemRo,
        required: true,
        bind: false,
    };
    let mut config_mounts = Vec::with_capacity(1 + extra_mounts.len());
    config_mounts.push(mount);
    config_mounts.extend(extra_mounts.iter().cloned());
    let mounts = validated_host_mounts(&config_mounts)?;
    let manifest = HostManifest {
        schema_version: 1,
        export_tag: crate::CONFIG_FS_TAG.to_string(),
        created_by: "agentvm-frontend".to_string(),
        mounts,
        synthetic: SyntheticSpec {
            uid: 0,
            gid: 0,
            dir_mode: "0555",
        },
        protected_guest_paths: Vec::new(),
    };
    write_json(&config.runtime.config_fs_manifest, &manifest)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| invalid_input(format!("failed to encode JSON: {error}")))?;
    let mut bytes = bytes;
    bytes.push(b'\n');
    fs::write(path, bytes)
}

fn normalize_absolute_path(path: &Path, field: &str) -> io::Result<String> {
    if !path.is_absolute() {
        return Err(invalid_input(format!(
            "{field} must be absolute: {}",
            path.display()
        )));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => normalized.push("/"),
            std::path::Component::Normal(part) => normalized.push(part),
            _ => {
                return Err(invalid_input(format!(
                    "{field} must not contain . or ..: {}",
                    path.display()
                )))
            }
        }
    }
    Ok(normalized.display().to_string())
}

fn protected_guest_paths() -> Vec<&'static str> {
    vec![
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/tmp",
        "/var/lib/docker",
        "/var/run/docker.sock",
        "/var/run",
    ]
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FrontendConfig, GuestNetwork, RuntimePaths, ToolPaths, VmArtifacts, VmShape,
        COMPOSED_FS_TAG,
    };

    fn config(root: &Path) -> FrontendConfig {
        FrontendConfig {
            project: root.join("repo"),
            tools: ToolPaths {
                qemu_system_x86_64: PathBuf::from("qemu-system-x86_64"),
            },
            artifacts: VmArtifacts {
                kernel: root.join("docker/out/vmlinuz"),
                initrd: root.join("docker/out/initrd.img"),
                rootfs: root.join("docker/out/rootfs.raw"),
            },
            runtime: RuntimePaths::under(root.join(".sandbox/docker-vm/run")),
            vm: VmShape {
                memory_bytes: 1024 * 1024 * 1024,
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
    fn writes_composed_and_config_fs_manifests() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        let config = config(&root);

        let summary = write_runtime_manifests(&config, &workspace_mounts(config.project.clone()))
            .expect("write manifests");

        assert_eq!(summary.mount_count, 1);
        let host_manifest =
            fs::read_to_string(&config.runtime.composed_fs_manifest).expect("host manifest");
        assert!(host_manifest.contains("\"export_tag\": \"agentvm\""));
        assert!(host_manifest.contains("\"guest_path\": \""));
        assert!(host_manifest.contains("\"source_class\": \"workspace\""));

        let bind_manifest =
            fs::read_to_string(&config.runtime.composed_bind_manifest).expect("bind manifest");
        assert!(bind_manifest.contains("\"composed_mountpoint\": \"/run/agentvm-host\""));
        assert!(bind_manifest.contains("\"target\": \""));

        let config_manifest =
            fs::read_to_string(&config.runtime.config_fs_manifest).expect("config manifest");
        assert!(config_manifest.contains("\"guest_path\": \"/composed-binds.json\""));
        assert!(config_manifest.contains("\"host_path\": \""));
    }

    #[test]
    fn writes_extra_config_fs_mounts() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        let ca_cert = root.join("mitm-ca.crt");
        fs::write(&ca_cert, "test ca").expect("ca");
        let config = config(&root);
        let extra = RuntimeMount {
            id: "m0002_mitm_ca_cert".to_string(),
            host_path: ca_cert,
            guest_path: PathBuf::from("/mitm-ca.crt"),
            readonly: true,
            source_class: ManifestSourceClass::SystemRo,
            required: true,
            bind: false,
        };

        write_runtime_manifests_with_config_mounts(
            &config,
            &workspace_mounts(config.project.clone()),
            &[extra],
        )
        .expect("write manifests");

        let config_manifest =
            fs::read_to_string(&config.runtime.config_fs_manifest).expect("config manifest");
        assert!(config_manifest.contains("\"guest_path\": \"/composed-binds.json\""));
        assert!(config_manifest.contains("\"guest_path\": \"/mitm-ca.crt\""));
    }

    #[test]
    fn rejects_protected_guest_path() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("source")).expect("source");
        let config = config(&root);
        let mount = RuntimeMount {
            id: "bad".to_string(),
            host_path: root.join("source"),
            guest_path: PathBuf::from("/run"),
            readonly: false,
            source_class: ManifestSourceClass::Workspace,
            required: true,
            bind: true,
        };

        let error = write_runtime_manifests(&config, &[mount]).expect_err("protected");
        assert!(error.to_string().contains("protected guest path"));
    }

    fn unique_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentvm-frontend-manifest-test-{}-{}",
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
