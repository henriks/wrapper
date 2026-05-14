use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;

use crate::{FrontendConfig, COMPOSED_FS_MOUNTPOINT};

pub const CODEX_TOOL_STATE_DIRS: &[&str] = &[".codex"];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestTool {
    Codex,
    Copilot,
}

impl GuestTool {
    pub fn cli(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Copilot => "github-copilot-cli",
        }
    }

    pub fn auto_flags(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
            Self::Copilot => &["--allow-all", "--no-auto-update"],
        }
    }

    pub fn npm_package(self) -> &'static str {
        match self {
            Self::Codex => "@openai/codex",
            Self::Copilot => "@github/copilot",
        }
    }

    fn state_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Codex => CODEX_TOOL_STATE_DIRS,
            Self::Copilot => &[
                ".copilot",
                ".config/github-copilot",
                ".cache/github-copilot",
            ],
        }
    }
}

impl FromStr for GuestTool {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Self::Codex),
            "copilot" => Ok(Self::Copilot),
            _ => Err(format!("unknown tool: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolStateMounts {
    pub codex: bool,
}

impl ToolStateMounts {
    pub fn codex() -> Self {
        Self { codex: true }
    }

    pub fn from_guest_tool(tool: Option<GuestTool>) -> Self {
        Self {
            codex: tool == Some(GuestTool::Codex),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestShareSpec {
    pub tool: Option<GuestTool>,
    pub tool_state: ToolStateMounts,
    pub host_home: PathBuf,
    pub gh: bool,
    pub extra_ro: Vec<PathBuf>,
    pub extra_rw: Vec<PathBuf>,
}

impl GuestShareSpec {
    pub fn minimal(host_home: impl Into<PathBuf>) -> Self {
        Self {
            tool: None,
            tool_state: ToolStateMounts::default(),
            host_home: host_home.into(),
            gh: false,
            extra_ro: Vec::new(),
            extra_rw: Vec::new(),
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

pub fn guest_runtime_mounts(
    project: impl Into<PathBuf>,
    spec: &GuestShareSpec,
) -> Vec<RuntimeMount> {
    let project = project.into();
    let guest_home = project.join(".sandbox/home");
    let mut mounts = vec![RuntimeMount::workspace(project)];
    let mut next_id = 2;

    if spec.tool_state.codex || spec.tool == Some(GuestTool::Codex) {
        for rel_dir in CODEX_TOOL_STATE_DIRS {
            mounts.push(home_mount(
                next_id,
                &spec.host_home,
                &guest_home,
                rel_dir,
                false,
                ManifestSourceClass::ToolState,
            ));
            next_id += 1;
        }
    }
    if spec.tool == Some(GuestTool::Copilot) {
        for rel_dir in GuestTool::Copilot.state_dirs() {
            mounts.push(home_mount(
                next_id,
                &spec.host_home,
                &guest_home,
                rel_dir,
                false,
                ManifestSourceClass::ToolState,
            ));
            next_id += 1;
        }
    }

    mounts.push(home_mount(
        next_id,
        &spec.host_home,
        &guest_home,
        ".docker",
        false,
        ManifestSourceClass::ToolState,
    ));
    next_id += 1;

    if spec.gh {
        mounts.push(home_mount(
            next_id,
            &spec.host_home,
            &guest_home,
            ".config/gh",
            true,
            ManifestSourceClass::AuthConfig,
        ));
        next_id += 1;
    }

    for path in &spec.extra_ro {
        mounts.push(user_mount(next_id, path, true, ManifestSourceClass::UserRo));
        next_id += 1;
    }
    for path in &spec.extra_rw {
        mounts.push(user_mount(
            next_id,
            path,
            false,
            ManifestSourceClass::UserRw,
        ));
        next_id += 1;
    }

    mounts
}

fn home_mount(
    index: usize,
    host_home: &Path,
    guest_home: &Path,
    rel_path: &str,
    readonly: bool,
    source_class: ManifestSourceClass,
) -> RuntimeMount {
    RuntimeMount {
        id: format!("m{index:04}_{}", mount_id_suffix(rel_path)),
        host_path: host_home.join(rel_path),
        guest_path: guest_home.join(rel_path),
        readonly,
        source_class,
        required: false,
        bind: true,
    }
}

fn user_mount(
    index: usize,
    path: &Path,
    readonly: bool,
    source_class: ManifestSourceClass,
) -> RuntimeMount {
    RuntimeMount {
        id: format!(
            "m{index:04}_{}",
            mount_id_suffix(&path.display().to_string())
        ),
        host_path: path.to_path_buf(),
        guest_path: path.to_path_buf(),
        readonly,
        source_class,
        required: true,
        bind: true,
    }
}

fn mount_id_suffix(value: &str) -> String {
    let suffix: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let suffix = suffix.trim_matches('_');
    if suffix.is_empty() {
        "root".to_string()
    } else {
        suffix.to_string()
    }
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
    fn config_fs_mounts_expose_mitm_ca_cert_without_private_key() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        let ca_cert = root.join("mitm-ca.crt");
        let ca_key = root.join("mitm-ca.key");
        fs::write(&ca_cert, "test ca").expect("ca cert");
        fs::write(&ca_key, "private key").expect("ca key");
        let config = config(&root);
        let extra = RuntimeMount {
            id: "m0002_mitm_ca_cert".to_string(),
            host_path: ca_cert.clone(),
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
        assert!(config_manifest.contains(&ca_cert.display().to_string()));
        assert!(config_manifest.contains("\"access\": \"ro\""));
        assert!(!config_manifest.contains(&ca_key.display().to_string()));
        assert!(!config_manifest.contains("mitm-ca.key"));
        assert!(!config_manifest.contains("private key"));
    }

    #[test]
    fn guest_runtime_mounts_add_tool_auth_docker_and_user_shares() {
        let root = unique_temp_dir();
        let project = root.join("repo");
        let home = root.join("host-home");
        let extra_ro = root.join("extra-ro");
        let extra_rw = root.join("extra-rw");
        fs::create_dir_all(&project).expect("repo");
        fs::create_dir_all(home.join(".codex")).expect("codex");
        fs::create_dir_all(home.join(".docker")).expect("docker");
        fs::create_dir_all(home.join(".config/gh")).expect("gh");
        fs::create_dir_all(&extra_ro).expect("extra ro");
        fs::create_dir_all(&extra_rw).expect("extra rw");
        let config = config(&root);
        let mounts = guest_runtime_mounts(
            project,
            &GuestShareSpec {
                tool: Some(GuestTool::Codex),
                tool_state: ToolStateMounts::from_guest_tool(Some(GuestTool::Codex)),
                host_home: home,
                gh: true,
                extra_ro: vec![extra_ro.clone()],
                extra_rw: vec![extra_rw.clone()],
            },
        );

        write_runtime_manifests(&config, &mounts).expect("write manifests");

        let host_manifest =
            fs::read_to_string(&config.runtime.composed_fs_manifest).expect("host manifest");
        assert!(host_manifest.contains("\"guest_path\": \""));
        assert!(host_manifest.contains(".sandbox/home/.codex"));
        assert!(host_manifest.contains(".sandbox/home/.docker"));
        assert!(host_manifest.contains(".sandbox/home/.config/gh"));
        assert!(host_manifest.contains("\"source_class\": \"tool-state\""));
        assert!(host_manifest.contains("\"source_class\": \"auth-config\""));
        assert!(host_manifest.contains("\"source_class\": \"user-ro\""));
        assert!(host_manifest.contains("\"source_class\": \"user-rw\""));
        assert!(host_manifest.contains(&extra_ro.display().to_string()));
        assert!(host_manifest.contains(&extra_rw.display().to_string()));
    }

    #[test]
    fn codex_tool_state_mounts_do_not_require_tool_selection() {
        let root = unique_temp_dir();
        let project = root.join("repo");
        let home = root.join("host-home");
        fs::create_dir_all(&project).expect("repo");
        fs::create_dir_all(home.join(".codex")).expect("codex");
        let mounts = guest_runtime_mounts(
            project.clone(),
            &GuestShareSpec {
                tool: None,
                tool_state: ToolStateMounts::codex(),
                host_home: home.clone(),
                gh: false,
                extra_ro: Vec::new(),
                extra_rw: Vec::new(),
            },
        );

        assert!(mounts.iter().any(|mount| {
            mount.host_path == home.join(".codex")
                && mount.guest_path == project.join(".sandbox/home/.codex")
                && !mount.readonly
                && mount.source_class == ManifestSourceClass::ToolState
        }));
    }

    #[test]
    fn guest_runtime_mounts_cover_copilot_state_and_gh_opt_in() {
        let root = unique_temp_dir();
        let project = root.join("repo");
        let home = root.join("host-home");
        let mounts = guest_runtime_mounts(
            project.clone(),
            &GuestShareSpec {
                tool: Some(GuestTool::Copilot),
                tool_state: ToolStateMounts::from_guest_tool(Some(GuestTool::Copilot)),
                host_home: home.clone(),
                gh: false,
                extra_ro: Vec::new(),
                extra_rw: Vec::new(),
            },
        );

        assert!(mounts.iter().any(|mount| mount.guest_path == project));
        for rel in [
            ".copilot",
            ".config/github-copilot",
            ".cache/github-copilot",
            ".docker",
        ] {
            assert!(
                mounts.iter().any(|mount| {
                    mount.host_path == home.join(rel)
                        && mount.guest_path == project.join(".sandbox/home").join(rel)
                        && !mount.readonly
                        && mount.source_class == ManifestSourceClass::ToolState
                }),
                "missing copilot/tool-state mount for {rel}"
            );
        }
        assert!(!mounts
            .iter()
            .any(|mount| mount.guest_path.ends_with(".config/gh")));
    }

    #[test]
    fn validation_rejects_relative_dotdot_duplicate_and_missing_required_mounts() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        fs::create_dir_all(root.join("source")).expect("source");
        let config = config(&root);

        let cases = [
            (
                RuntimeMount {
                    id: "relative-host".to_string(),
                    host_path: PathBuf::from("relative"),
                    guest_path: PathBuf::from("/guest"),
                    readonly: false,
                    source_class: ManifestSourceClass::UserRw,
                    required: true,
                    bind: true,
                },
                "host_path must be absolute",
            ),
            (
                RuntimeMount {
                    id: "dotdot-guest".to_string(),
                    host_path: root.join("source"),
                    guest_path: PathBuf::from("/guest/../escape"),
                    readonly: false,
                    source_class: ManifestSourceClass::UserRw,
                    required: true,
                    bind: true,
                },
                "guest_path must not contain . or ..",
            ),
            (
                RuntimeMount {
                    id: "missing-required".to_string(),
                    host_path: root.join("missing"),
                    guest_path: PathBuf::from("/missing"),
                    readonly: false,
                    source_class: ManifestSourceClass::UserRw,
                    required: true,
                    bind: true,
                },
                "required composed filesystem source is missing",
            ),
        ];

        for (mount, expected) in cases {
            let error = write_runtime_manifests(&config, &[mount]).expect_err(expected);
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
        }

        let duplicate_id = RuntimeMount {
            id: "dup".to_string(),
            host_path: root.join("source"),
            guest_path: PathBuf::from("/one"),
            readonly: false,
            source_class: ManifestSourceClass::UserRw,
            required: true,
            bind: true,
        };
        let error = write_runtime_manifests(&config, &[duplicate_id.clone(), duplicate_id])
            .expect_err("duplicate id");
        assert!(error.to_string().contains("duplicate mount id"));
        let duplicate_guest_left = RuntimeMount {
            id: "dup1".to_string(),
            host_path: root.join("source"),
            guest_path: PathBuf::from("/one"),
            readonly: false,
            source_class: ManifestSourceClass::UserRw,
            required: true,
            bind: true,
        };
        let duplicate_guest_right = RuntimeMount {
            id: "dup2".to_string(),
            host_path: root.join("source"),
            guest_path: PathBuf::from("/one"),
            readonly: false,
            source_class: ManifestSourceClass::UserRw,
            required: true,
            bind: true,
        };
        let error =
            write_runtime_manifests(&config, &[duplicate_guest_left, duplicate_guest_right])
                .expect_err("duplicate guest path");
        assert!(error
            .to_string()
            .contains("duplicate composed filesystem guest path"));
    }

    #[test]
    fn optional_missing_mounts_are_skipped_without_bind_entries() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("repo")).expect("repo");
        let config = config(&root);
        let optional = RuntimeMount {
            id: "optional".to_string(),
            host_path: root.join("missing-optional"),
            guest_path: PathBuf::from("/optional"),
            readonly: true,
            source_class: ManifestSourceClass::UserRo,
            required: false,
            bind: true,
        };

        write_runtime_manifests(
            &config,
            &[RuntimeMount::workspace(config.project.clone()), optional],
        )
        .expect("write manifests");

        let host_manifest =
            fs::read_to_string(&config.runtime.composed_fs_manifest).expect("host manifest");
        let bind_manifest =
            fs::read_to_string(&config.runtime.composed_bind_manifest).expect("bind manifest");
        assert!(!host_manifest.contains("/optional"));
        assert!(!bind_manifest.contains("/optional"));
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
