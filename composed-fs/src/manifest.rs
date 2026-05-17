use std::io;

use serde::Deserialize;

use crate::{
    invalid_input, normalize_guest_path, parse_octal_mode, validate_single_path_component,
    SCHEMA_VERSION,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) export_tag: Option<String>,
    #[serde(default)]
    pub(crate) created_by: Option<String>,
    pub(crate) mounts: Vec<MountSpec>,
    #[serde(default)]
    pub(crate) synthetic: Option<SyntheticSpec>,
    #[serde(default)]
    pub(crate) protected_guest_paths: Vec<String>,
    #[serde(default)]
    pub(crate) shadow_root: Option<String>,
    #[serde(default)]
    pub(crate) filters: Vec<FilterSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FilterSpec {
    #[serde(default)]
    pub(crate) mount_id: Option<String>,
    pub(crate) suffixes: Vec<String>,
    pub(crate) action: FilterAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FilterAction {
    HideAndShadow,
}

impl FilterSpec {
    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.suffixes.is_empty() {
            return Err(invalid_input("filter suffixes cannot be empty"));
        }
        for suffix in &self.suffixes {
            if suffix.is_empty() || suffix.contains('/') || suffix.contains('\0') {
                return Err(invalid_input(format!(
                    "filter suffix must be a non-empty path-component suffix: {suffix:?}"
                )));
            }
        }
        match self.action {
            FilterAction::HideAndShadow => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MountSpec {
    pub(crate) id: String,
    pub(crate) guest_path: String,
    pub(crate) host_path: String,
    pub(crate) kind: MountKind,
    pub(crate) access: AccessMode,
    pub(crate) source_class: SourceClass,
    pub(crate) required: bool,
    pub(crate) bind: bool,
    pub(crate) metadata: MetadataSpec,
}

impl MountSpec {
    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.id.is_empty() {
            return Err(invalid_input("mount id cannot be empty"));
        }
        validate_single_path_component(&self.id).map_err(|_| {
            invalid_input(format!(
                "mount id must be a safe path component: {:?}",
                self.id
            ))
        })?;
        if !self.host_path.starts_with('/') {
            return Err(invalid_input(format!(
                "host_path must be absolute for mount {}: {:?}",
                self.id, self.host_path
            )));
        }
        let _access = match self.access {
            AccessMode::Ro => "ro",
            AccessMode::Rw => "rw",
        };
        let _source_class = match self.source_class {
            SourceClass::Workspace => "workspace",
            SourceClass::PersistentHome => "persistent-home",
            SourceClass::ToolState => "tool-state",
            SourceClass::AuthConfig => "auth-config",
            SourceClass::SystemRo => "system-ro",
            SourceClass::UserRo => "user-ro",
            SourceClass::UserRw => "user-rw",
        };
        let _required = self.required;
        let _bind = self.bind;
        self.metadata.validate();
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MountKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AccessMode {
    Ro,
    Rw,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SourceClass {
    Workspace,
    PersistentHome,
    ToolState,
    AuthConfig,
    SystemRo,
    UserRo,
    UserRw,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataSpec {
    pub(crate) uid_gid: MetadataPolicy,
    pub(crate) permissions: MetadataPolicy,
}

impl MetadataSpec {
    pub(crate) fn validate(&self) {
        match self.uid_gid {
            MetadataPolicy::Host => {}
        }
        match self.permissions {
            MetadataPolicy::Host => {}
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MetadataPolicy {
    Host,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyntheticSpec {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) dir_mode: String,
}

pub fn validate_manifest_json_shape(manifest_text: &str) -> io::Result<()> {
    let manifest: Manifest = serde_json::from_str(manifest_text)
        .map_err(|error| invalid_input(format!("invalid manifest JSON: {error}")))?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(invalid_input(format!(
            "unsupported schema_version {}, expected {}",
            manifest.schema_version, SCHEMA_VERSION
        )));
    }
    if manifest.mounts.is_empty() {
        return Err(invalid_input("manifest must contain at least one mount"));
    }
    if let Some(synthetic) = &manifest.synthetic {
        parse_octal_mode(&synthetic.dir_mode)?;
    }

    let mut normalized_paths = Vec::new();
    for mount in &manifest.mounts {
        mount.validate()?;
        let guest_path = normalize_guest_path(&mount.guest_path)?;
        if manifest
            .protected_guest_paths
            .iter()
            .any(|protected| protected == &guest_path)
        {
            return Err(invalid_input(format!(
                "mount targets protected guest path: {guest_path}"
            )));
        }
        if guest_path == "/" {
            return Err(invalid_input("mount guest_path cannot be /"));
        }
        if normalized_paths.iter().any(|path| path == &guest_path) {
            return Err(invalid_input(format!(
                "duplicate guest_path in manifest: {guest_path}"
            )));
        }
        normalized_paths.push(guest_path);
    }
    Ok(())
}
