use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{host_home_dir, toml_basic_string, NODE_HTTP_TOOL, NODE_HTTP_VERSION};

pub(crate) type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("unknown setup tool: {value}")]
    UnknownSetupTool { value: String },
    #[error("HOME must be set to an absolute path for guest state sharing")]
    MissingHostHome,
    #[error("default command must not be empty")]
    EmptyDefaultCommand,
    #[error("share shadow path must not be empty")]
    EmptyShareShadowPath,
    #[error("share shadow path must not contain '='")]
    ShareShadowPathContainsEquals,
    #[error("share shadow path must be relative to the parent share")]
    AbsoluteShareShadowPath,
    #[error("share shadow path must stay under the parent share")]
    EscapingShareShadowPath,
    #[error("unsupported sandbox config schema_version {version}")]
    UnsupportedSandboxConfigSchema { version: u32 },
    #[error("sandbox config share host_path must not be empty")]
    EmptyShareHostPath,
    #[error("sandbox config share guest_path must not be empty")]
    EmptyShareGuestPath,
    #[error("duplicate sandbox config share shadow path: {path}")]
    DuplicateShareShadowPath { path: String },
    #[error("failed to read sandbox config {path}: {source}")]
    ReadSandboxConfig { path: PathBuf, source: io::Error },
    #[error("failed to parse sandbox config {path}: {source}")]
    ParseSandboxConfig {
        path: PathBuf,
        source: Box<ConfigError>,
    },
    #[error("invalid sandbox config {path}: {source}")]
    InvalidSandboxConfig {
        path: PathBuf,
        source: Box<ConfigError>,
    },
    #[error("missing schema_version")]
    MissingSchemaVersion,
    #[error("unsupported schema_version {version}")]
    UnsupportedSchemaVersion { version: u64 },
    #[error("setup_tool is not a schema_version 3 field; run agentvm --setup-tool codex|pi for one-time setup")]
    SetupToolInSchemaV3,
    #[error("{source}")]
    Json { source: serde_json::Error },
    #[error("failed to create sandbox config directory {path}: {source}")]
    CreateSandboxConfigDir { path: PathBuf, source: io::Error },
    #[error("failed to serialize sandbox config: {source}")]
    SerializeSandboxConfig { source: serde_json::Error },
    #[error("failed to write sandbox config {path}: {source}")]
    WriteSandboxConfig { path: PathBuf, source: io::Error },
    #[error("failed to create setup-tool mise config directory {path}: {source}")]
    CreateSetupToolMiseConfigDir { path: PathBuf, source: io::Error },
    #[error("failed to write setup-tool mise config {path}: {source}")]
    WriteSetupToolMiseConfig { path: PathBuf, source: io::Error },
}

impl From<ConfigError> for String {
    fn from(error: ConfigError) -> Self {
        error.to_string()
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(source: serde_json::Error) -> Self {
        Self::Json { source }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SetupTool {
    Codex,
    Pi,
}

impl SetupTool {
    pub(crate) fn parse(value: &str) -> ConfigResult<Self> {
        match value {
            "codex" => Ok(Self::Codex),
            "pi" => Ok(Self::Pi),
            _ => Err(ConfigError::UnknownSetupTool {
                value: value.to_string(),
            }),
        }
    }

    fn default_command(self) -> ConfigCommand {
        ConfigCommand {
            command: self.cli().to_string(),
            args: self
                .auto_flags()
                .iter()
                .map(|flag| flag.to_string())
                .collect(),
        }
    }

    fn package(self) -> &'static str {
        match self {
            Self::Codex => "@openai/codex",
            Self::Pi => "@mariozechner/pi-coding-agent",
        }
    }

    pub(crate) fn cli(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    fn auto_flags(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["--dangerously-bypass-approvals-and-sandbox"],
            Self::Pi => &[],
        }
    }

    fn config_shares(self, host_home: &Path) -> Vec<ConfigShare> {
        let (dir, shadows) = match self {
            Self::Codex => (".codex", vec!["tmp".to_string()]),
            Self::Pi => (".pi", Vec::new()),
        };
        let path = host_home.join(dir).display().to_string();
        vec![ConfigShare {
            host_path: path.clone(),
            guest_path: Some(path),
            access: ConfigShareAccess::Rw,
            required: false,
            shadows,
        }]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigCommand {
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
}

impl ConfigCommand {
    pub(crate) fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
        }
    }

    fn validate(&self) -> ConfigResult<()> {
        if self.command.trim().is_empty() {
            Err(ConfigError::EmptyDefaultCommand)
        } else {
            Ok(())
        }
    }
}

impl Default for ConfigCommand {
    fn default() -> Self {
        Self::new("codex")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConfigNetworkMode {
    Public,
    None,
    Allowlist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigNetwork {
    #[serde(default = "default_network_mode")]
    pub(crate) mode: ConfigNetworkMode,
    #[serde(default)]
    pub(crate) allowed_domains: Vec<String>,
    #[serde(default)]
    pub(crate) allowed_hosts: Vec<String>,
    #[serde(default)]
    pub(crate) allowed_ips: Vec<String>,
}

impl Default for ConfigNetwork {
    fn default() -> Self {
        Self {
            mode: ConfigNetworkMode::Public,
            allowed_domains: Vec::new(),
            allowed_hosts: Vec::new(),
            allowed_ips: Vec::new(),
        }
    }
}

fn default_network_mode() -> ConfigNetworkMode {
    ConfigNetworkMode::Public
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct ConfigAuth {
    #[serde(default)]
    pub(crate) github: bool,
    #[serde(default)]
    pub(crate) aws_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConfigShareAccess {
    Ro,
    Rw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigShare {
    pub(crate) host_path: String,
    #[serde(default)]
    pub(crate) guest_path: Option<String>,
    pub(crate) access: ConfigShareAccess,
    #[serde(default = "default_required_share")]
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) shadows: Vec<String>,
}

fn default_required_share() -> bool {
    true
}

pub(crate) fn validate_share_shadow_path(path: &str) -> ConfigResult<PathBuf> {
    if path.trim().is_empty() {
        return Err(ConfigError::EmptyShareShadowPath);
    }
    if path.contains('=') {
        return Err(ConfigError::ShareShadowPathContainsEquals);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(ConfigError::AbsoluteShareShadowPath);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ConfigError::EscapingShareShadowPath);
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(ConfigError::EmptyShareShadowPath);
    }
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigPort {
    pub(crate) host: u16,
    pub(crate) guest: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WrapperSandboxConfig {
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) default_command: ConfigCommand,
    #[serde(default)]
    pub(crate) network: ConfigNetwork,
    #[serde(default)]
    pub(crate) auth: ConfigAuth,
    #[serde(default)]
    pub(crate) shares: Vec<ConfigShare>,
    #[serde(default)]
    pub(crate) published_ports: Vec<ConfigPort>,
}

impl WrapperSandboxConfig {
    pub(crate) fn setup_tool(tool: SetupTool) -> ConfigResult<Self> {
        let host_home = host_home_dir().map_err(|_| ConfigError::MissingHostHome)?;
        Ok(Self {
            schema_version: 3,
            default_command: tool.default_command(),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: tool.config_shares(&host_home),
            published_ports: Vec::new(),
        })
    }

    pub(crate) fn codex_default() -> ConfigResult<Self> {
        Self::setup_tool(SetupTool::Codex)
    }

    pub(crate) fn validate(&self) -> ConfigResult<()> {
        if self.schema_version != 3 {
            return Err(ConfigError::UnsupportedSandboxConfigSchema {
                version: self.schema_version,
            });
        }
        self.default_command.validate()?;
        for share in &self.shares {
            if share.host_path.trim().is_empty() {
                return Err(ConfigError::EmptyShareHostPath);
            }
            if share.guest_path.as_deref().is_some_and(str::is_empty) {
                return Err(ConfigError::EmptyShareGuestPath);
            }
            let mut shadow_paths = BTreeSet::new();
            for shadow in &share.shadows {
                let path = validate_share_shadow_path(shadow)?;
                if !shadow_paths.insert(path) {
                    return Err(ConfigError::DuplicateShareShadowPath {
                        path: shadow.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn wrapper_sandbox_config_path(project: &PathBuf) -> PathBuf {
    project.join(".sandbox/config.json")
}

pub(crate) fn read_wrapper_sandbox_config(
    project: &PathBuf,
) -> ConfigResult<Option<WrapperSandboxConfig>> {
    let path = wrapper_sandbox_config_path(project);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ConfigError::ReadSandboxConfig { path, source }),
    };
    let config = parse_wrapper_sandbox_config_text(&text).map_err(|source| {
        ConfigError::ParseSandboxConfig {
            path: path.clone(),
            source: Box::new(source),
        }
    })?;
    config
        .validate()
        .map_err(|source| ConfigError::InvalidSandboxConfig {
            path,
            source: Box::new(source),
        })?;
    Ok(Some(config))
}

pub(crate) fn parse_wrapper_sandbox_config_text(text: &str) -> ConfigResult<WrapperSandboxConfig> {
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ConfigError::MissingSchemaVersion)?;
    if schema_version == 1 {
        return parse_legacy_wrapper_sandbox_config(value);
    }
    if schema_version == 2 {
        return parse_v2_wrapper_sandbox_config(value);
    }
    if schema_version != 3 {
        return Err(ConfigError::UnsupportedSchemaVersion {
            version: schema_version,
        });
    }
    if value.get("setup_tool").is_some() {
        return Err(ConfigError::SetupToolInSchemaV3);
    }
    normalize_default_command_value(&mut value)?;
    serde_json::from_value(value).map_err(Into::into)
}

fn parse_v2_wrapper_sandbox_config(
    mut value: serde_json::Value,
) -> ConfigResult<WrapperSandboxConfig> {
    let legacy_setup_tool = value
        .get("setup_tool")
        .and_then(serde_json::Value::as_str)
        .map(SetupTool::parse)
        .transpose()?;
    value
        .as_object_mut()
        .map(|object| object.remove("setup_tool"));
    value["schema_version"] = serde_json::json!(3);
    normalize_default_command_value(&mut value)?;
    if let Some(tool) = legacy_setup_tool {
        add_legacy_setup_auto_flags(&mut value, tool);
    }
    serde_json::from_value(value).map_err(Into::into)
}

fn add_legacy_setup_auto_flags(value: &mut serde_json::Value, tool: SetupTool) {
    let Some(default_command) = value.get_mut("default_command") else {
        return;
    };
    if default_command
        .get("command")
        .and_then(serde_json::Value::as_str)
        != Some(tool.cli())
    {
        return;
    }
    let Some(args) = default_command
        .get_mut("args")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for flag in tool.auto_flags().iter().rev() {
        if !args.iter().any(|arg| arg.as_str() == Some(flag)) {
            args.insert(0, serde_json::json!(flag));
        }
    }
}

fn parse_legacy_wrapper_sandbox_config(
    value: serde_json::Value,
) -> ConfigResult<WrapperSandboxConfig> {
    let codex_enabled = value
        .get("codex_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let command = value
        .get("default_command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("codex")
        .to_string();
    let mut config = if codex_enabled {
        WrapperSandboxConfig::codex_default()?
    } else {
        WrapperSandboxConfig {
            schema_version: 3,
            default_command: ConfigCommand::new(command.clone()),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: Vec::new(),
            published_ports: Vec::new(),
        }
    };
    config.default_command = ConfigCommand::new(command);
    Ok(config)
}

fn normalize_default_command_value(value: &mut serde_json::Value) -> ConfigResult<()> {
    let Some(command_value) = value.get_mut("default_command") else {
        return Ok(());
    };
    if let Some(command) = command_value.as_str() {
        *command_value = serde_json::json!({
            "command": command,
            "args": [],
        });
    }
    Ok(())
}

pub(crate) fn write_wrapper_sandbox_config(
    project: &PathBuf,
    config: &WrapperSandboxConfig,
) -> ConfigResult<()> {
    let path = wrapper_sandbox_config_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::CreateSandboxConfigDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|source| ConfigError::SerializeSandboxConfig { source })?;
    fs::write(&path, format!("{text}\n"))
        .map_err(|source| ConfigError::WriteSandboxConfig { path, source })
}

pub(crate) fn wrapper_mise_config_path(project: &Path) -> PathBuf {
    project.join(".sandbox/mise.toml")
}

pub(crate) fn write_setup_tool_mise_config(project: &Path, tool: SetupTool) -> ConfigResult<()> {
    let path = wrapper_mise_config_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::CreateSetupToolMiseConfigDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(&path, setup_tool_mise_config_text(tool))
        .map_err(|source| ConfigError::WriteSetupToolMiseConfig { path, source })
}

pub(crate) fn setup_tool_mise_config_text(tool: SetupTool) -> String {
    format!(
        "[tools]\n{} = \"{}\"\n{} = \"latest\"\n",
        toml_basic_string(NODE_HTTP_TOOL),
        NODE_HTTP_VERSION,
        toml_basic_string(&format!("npm:{}", tool.package()))
    )
}
