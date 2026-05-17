use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub(crate) type ApplianceResult<T> = Result<T, ApplianceError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ApplianceError {
    #[error("stale appliance artifacts: cannot infer repository root from {artifact_manifest}; rerun sudo ./docker/build-appliance.sh")]
    CannotInferRepositoryRoot { artifact_manifest: String },
    #[error("failed to read {path}: {source}")]
    Read { path: String, source: io::Error },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
    #[error("stale appliance artifacts: {artifact_manifest} does not record appliance source hashes; rerun sudo ./docker/build-appliance.sh")]
    MissingSourceHashes { artifact_manifest: String },
    #[error("stale appliance artifacts: {artifact_manifest} contains invalid source path {source_path}; rerun sudo ./docker/build-appliance.sh")]
    InvalidSourcePath {
        artifact_manifest: String,
        source_path: String,
    },
    #[error("stale appliance artifacts: {source_path} changed since {artifact_manifest} was written (expected sha256 {expected}, current {actual}); rerun sudo ./docker/build-appliance.sh")]
    ChangedSourceHash {
        source_path: String,
        artifact_manifest: String,
        expected: String,
        actual: String,
    },
    #[error("stale appliance artifacts: {artifact_manifest} does not record required source hash for {required}; rerun sudo ./docker/build-appliance.sh")]
    MissingRequiredSourceHash {
        artifact_manifest: String,
        required: &'static str,
    },
}

impl From<ApplianceError> for String {
    fn from(error: ApplianceError) -> Self {
        error.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct ApplianceFreshnessManifest {
    source_inputs: Option<Vec<ApplianceSourceInput>>,
}

#[derive(Debug, Deserialize)]
struct ApplianceSourceInput {
    path: PathBuf,
    sha256: String,
}

pub(crate) const REQUIRED_APPLIANCE_SOURCE_INPUTS: &[&str] = &[
    "docker/appliance.env",
    "docker/build-appliance.sh",
    "docker/guest-init.sh",
    "docker/guest-payload-server.py",
    "docker/guest-socket-bridge.py",
];

pub(crate) fn ensure_appliance_sources_fresh(artifact_manifest: &Path) -> ApplianceResult<()> {
    let artifact_manifest_display = artifact_manifest.display().to_string();
    let repo_root = artifact_manifest
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| ApplianceError::CannotInferRepositoryRoot {
            artifact_manifest: artifact_manifest_display.clone(),
        })?;
    let text = fs::read_to_string(artifact_manifest).map_err(|source| ApplianceError::Read {
        path: artifact_manifest_display.clone(),
        source,
    })?;
    let manifest: ApplianceFreshnessManifest =
        serde_json::from_str(&text).map_err(|source| ApplianceError::Parse {
            path: artifact_manifest_display.clone(),
            source,
        })?;
    let inputs = manifest
        .source_inputs
        .as_deref()
        .filter(|inputs| !inputs.is_empty())
        .ok_or_else(|| ApplianceError::MissingSourceHashes {
            artifact_manifest: artifact_manifest_display.clone(),
        })?;

    let mut recorded_paths = BTreeSet::new();
    for input in inputs {
        if input.path.is_absolute()
            || input
                .path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ApplianceError::InvalidSourcePath {
                artifact_manifest: artifact_manifest_display.clone(),
                source_path: input.path.display().to_string(),
            });
        }
        recorded_paths.insert(input.path.to_string_lossy().into_owned());
        let source_path = repo_root.join(&input.path);
        let actual_hash = sha256_file_hex(&source_path)?;
        if !input.sha256.eq_ignore_ascii_case(&actual_hash) {
            return Err(ApplianceError::ChangedSourceHash {
                source_path: input.path.display().to_string(),
                artifact_manifest: artifact_manifest_display.clone(),
                expected: input.sha256.clone(),
                actual: actual_hash,
            });
        }
    }
    for required in REQUIRED_APPLIANCE_SOURCE_INPUTS {
        if !recorded_paths.contains(*required) {
            return Err(ApplianceError::MissingRequiredSourceHash {
                artifact_manifest: artifact_manifest_display.clone(),
                required,
            });
        }
    }
    Ok(())
}

pub(crate) fn sha256_file_hex(path: &Path) -> ApplianceResult<String> {
    let bytes = fs::read(path).map_err(|source| ApplianceError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}
