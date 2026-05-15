use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use agentvm_frontend::launch::{
    prepare_frontend_launch_with_policy, run_frontend_until_qemu_exit_with_policy_and_timeout,
    start_frontend_with_policy,
};
use agentvm_frontend::network_policy::{
    EgressAction, EgressReason, HostListener, HostListenerPurpose, VmnetPolicy,
};
use agentvm_frontend::payload_client::{
    ping_payload, run_payload_tcp_with_control, socket_addr, terminal_size, PayloadControlOptions,
    PayloadRequest,
};
use agentvm_frontend::runtime_manifest::{
    guest_runtime_mounts, GuestShareSpec, RuntimeMount, ToolStateMounts,
};
use agentvm_frontend::tcp_gateway::UpstreamMapping;
use agentvm_frontend::vmnet_runtime::{serve_vmnet_gateway, VmnetRuntimeConfig};
use agentvm_frontend::{FrontendConfig, GuestNetwork, RuntimePaths};
use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand, ValueHint};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod tui;

fn main() {
    if let Err(error) = run_cli(env::args().collect()) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run_cli(argv: Vec<String>) -> Result<(), String> {
    let program = argv
        .first()
        .cloned()
        .unwrap_or_else(|| "agentvm-frontend".to_string());
    let args: Vec<String> = argv.into_iter().skip(1).collect();
    if is_agentvm_program(&program) {
        return match args.first().map(String::as_str) {
            Some("prepare" | "launch" | "vmnet-gateway" | "payload-client" | "self-test") => {
                run(args)
            }
            Some("wrap") => run_wrapper(program, args[1..].to_vec()),
            _ => run_wrapper(program, args),
        };
    }
    match args.first().map(String::as_str) {
        Some("prepare" | "launch" | "vmnet-gateway" | "payload-client" | "self-test")
        | Some("-h" | "--help")
        | None => run(args),
        Some("wrap") => run_wrapper(program, args[1..].to_vec()),
        Some(command) => Err(format!("unknown command: {command}")),
    }
}

fn is_agentvm_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("agentvm")
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.first().map(String::as_str) {
        Some("vmnet-gateway") => {
            let config = vmnet_gateway_config_from_args(&args[1..])?;
            serve_vmnet_gateway(config)
                .map_err(|error| format!("vmnet gateway failed: {error:?}"))?;
            Ok(())
        }
        Some("payload-client") => {
            let config = payload_client_config_from_args(&args[1..])?;
            let addr = socket_addr(&config.host, config.port).map_err(|error| error.to_string())?;
            if config.ping {
                ping_payload(addr).map_err(|error| error.to_string())?;
                return Ok(());
            }
            let script = config
                .script
                .ok_or_else(|| "--script is required unless --ping is set".to_string())?;
            let request = PayloadRequest {
                script,
                cwd: config.cwd,
                env: config.env,
                rows: config.rows,
                cols: config.cols,
            };
            let input =
                (!config.no_stdin).then(|| Box::new(io::stdin()) as Box<dyn io::Read + Send>);
            let exit_code = run_payload_tcp_with_control(
                addr,
                &request,
                input,
                &mut io::stdout(),
                PayloadControlOptions::interactive(),
            )
            .map_err(|error| error.to_string())?;
            std::process::exit(payload_exit_status(exit_code));
        }
        Some("self-test") => run_self_test(&args[1..]),
        Some("prepare") => {
            let (config, policy_args) = frontend_config_from_args(&args[1..])?;
            let mounts = runtime_mounts(&config, &policy_args)?;
            let policy = policy_from_args(config.network.clone(), policy_args);
            let prep = prepare_frontend_launch_with_policy(&config, &mounts, &policy)
                .map_err(|error| format!("prepare failed: {error}"))?;
            println!("{}", prep.qemu_command.join(" "));
            Ok(())
        }
        Some("launch") => run_launch(&args[1..], WrapperUiMode::Plain),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}")),
    }
}

fn run_launch(args: &[String], ui_mode: WrapperUiMode) -> Result<(), String> {
    let (config, policy_args) = frontend_config_from_args(args)?;
    let artifacts = frontend_artifact_summary(&config);
    let _lock = ProjectLock::acquire(&config)?;
    let qemu_timeout = policy_args.qemu_timeout;
    let local_http_smoke_upstream = policy_args.local_http_smoke_upstream;
    let payload = launch_payload_args(&config, &policy_args)?;
    let mounts = runtime_mounts(&config, &policy_args)?;
    let guest_env = guest_payload_env(&config, &policy_args)?;
    let mut policy = policy_from_args(config.network.clone(), policy_args);
    let mut config = config;
    if let Some(destination) = local_http_smoke_upstream {
        config
            .upstream_mappings
            .push(start_local_http_smoke_upstream(destination)?);
    }
    if let Some(payload) = payload {
        let host_port = ensure_payload_listener(&mut policy);
        println!("launch: phase=starting-frontend");
        let running = start_frontend_with_policy(config.clone(), mounts, policy)
            .map_err(|error| format!("launch failed: {error}\n{artifacts}"))?;
        let addr = socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
        println!("launch: phase=waiting-for-payload-ready timeout=120s");
        wait_for_payload_ready(addr, Duration::from_secs(120))
            .map_err(|error| format!("launch payload readiness failed: {error}\n{artifacts}"))?;
        let request = PayloadRequest {
            script: payload.script,
            cwd: payload.cwd,
            env: merged_payload_env(guest_env, payload.env),
            rows: payload.rows,
            cols: payload.cols,
        };
        let payload_result = match ui_mode {
            WrapperUiMode::Tui => {
                tui::run_payload_viewport(addr, &request).map_err(|error| error.to_string())
            }
            WrapperUiMode::Plain => {
                let input =
                    (!payload.no_stdin).then(|| Box::new(io::stdin()) as Box<dyn io::Read + Send>);
                run_payload_tcp_with_control(
                    addr,
                    &request,
                    input,
                    &mut io::stdout(),
                    PayloadControlOptions::interactive(),
                )
                .map_err(|error| error.to_string())
            }
        };
        running
            .terminate()
            .map_err(|error| format!("launch shutdown failed: {error}"))?;
        let exit_code = payload_result?;
        std::process::exit(payload_exit_status(exit_code));
    }
    println!("launch: phase=starting-frontend");
    let qemu_exit = run_frontend_until_qemu_exit_with_policy_and_timeout(
        config.clone(),
        mounts,
        policy,
        qemu_timeout,
    )
    .map_err(|error| format!("launch failed: {error}\n{artifacts}"))?;
    if qemu_exit.status.success() {
        Ok(())
    } else if qemu_exit.timed_out {
        Err(format!(
            "qemu timed out after {} seconds and was terminated with status: {}\n{}",
            qemu_timeout.map_or(0, |timeout| timeout.as_secs()),
            qemu_exit.status,
            artifacts
        ))
    } else {
        Err(format!(
            "qemu exited with status: {}\n{}",
            qemu_exit.status, artifacts
        ))
    }
}

fn frontend_artifact_summary(config: &FrontendConfig) -> String {
    format!(
        "artifacts: run_dir={} state={} qemu_log={} console_log={} vmnet_event_log={}",
        config.runtime.run_dir.display(),
        config.runtime.state_json.display(),
        config.runtime.run_dir.join("qemu.log").display(),
        config.runtime.console_log.display(),
        config.runtime.vmnet_event_log.display()
    )
}

struct ProjectLock {
    file: File,
}

impl ProjectLock {
    fn acquire(config: &FrontendConfig) -> Result<Self, String> {
        if let Some(parent) = config.runtime.lock.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create VM runtime root: {error}"))?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&config.runtime.lock)
            .map_err(|error| {
                format!(
                    "failed to open project VM lock {}: {error}",
                    config.runtime.lock.display()
                )
            })?;
        acquire_project_file_lock(
            &file,
            format!(
                "another VM sandbox is already active for this project ({})",
                config.runtime.lock.display()
            ),
        )?;
        Ok(Self { file })
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

fn run_wrapper(program: String, args: Vec<String>) -> Result<(), String> {
    let mut wrapper = parse_wrapper_args(&program, &args)?;
    if wrapper.help {
        return Ok(());
    }
    if wrapper.reset {
        reset_project(&wrapper.project)?;
        return Ok(());
    }
    if let Some(setup_tool) = wrapper.setup_tool {
        write_wrapper_sandbox_config(
            &wrapper.project,
            &WrapperSandboxConfig::setup_tool(setup_tool),
        )?;
    }
    if wrapper.edit_config {
        let config = read_wrapper_sandbox_config(&wrapper.project)?
            .unwrap_or_else(WrapperSandboxConfig::codex_default);
        let edited = tui::run_config_editor(config).map_err(|error| error.to_string())?;
        write_wrapper_sandbox_config(&wrapper.project, &edited)?;
        return Ok(());
    }
    if wrapper.tls_bootstrap {
        let ca = ensure_wrapper_mitm_ca(&wrapper.project)?;
        wrapper.launch_args.extend([
            "--tls-ca-cert".to_string(),
            ca.cert.display().to_string(),
            "--tls-ca-key".to_string(),
            ca.key.display().to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ]);
    }
    let project = wrapper.project.clone();
    let ui_mode = wrapper.ui_mode;
    let needs_startup_dialog = ui_mode == WrapperUiMode::Tui
        && !wrapper.tool_selected
        && wrapper.command_override.is_none();
    let mut launch_args = vec!["launch".to_string()];
    launch_args.extend(wrapper.into_launch_args());
    if needs_startup_dialog {
        let selection = tui::run_startup_dialog().map_err(|error| error.to_string())?;
        if selection.enable_codex {
            let config = WrapperSandboxConfig::codex_default();
            write_wrapper_sandbox_config(&project, &config)?;
            apply_configured_launch_defaults(&mut launch_args, &config, &project, false, false)?;
            apply_configured_default_command(&mut launch_args, &config);
        } else {
            return Err("startup dialog did not select a payload".to_string());
        }
    }
    match ui_mode {
        WrapperUiMode::Tui => return run_launch(&launch_args[1..], ui_mode),
        WrapperUiMode::Plain => {}
    }
    run(launch_args)
}

#[derive(Debug)]
struct WrapperArgs {
    project: PathBuf,
    launch_args: Vec<String>,
    ui_mode: WrapperUiMode,
    tool_selected: bool,
    command_override: Option<WrapperCommandOverride>,
    setup_tool: Option<SetupTool>,
    tls_bootstrap: bool,
    reset: bool,
    edit_config: bool,
    help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperUiMode {
    Tui,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WrapperCommandOverride {
    Argv(ConfigCommand),
}

impl WrapperCommandOverride {
    fn script(&self) -> String {
        match self {
            Self::Argv(command) => payload_script_from_config_command(command),
        }
    }
}

#[derive(Debug)]
struct MitmCaPaths {
    cert: PathBuf,
    key: PathBuf,
}

impl WrapperArgs {
    fn into_launch_args(self) -> Vec<String> {
        self.launch_args
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SetupTool {
    Codex,
    Pi,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct CliParseError(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortPair {
    host: u16,
    guest: u16,
}

impl fmt::Display for PortPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.guest)
    }
}

impl std::str::FromStr for PortPair {
    type Err = CliParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (host, guest) = parse_port_pair(value).map_err(CliParseError)?;
        Ok(Self { host, guest })
    }
}

impl SetupTool {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "codex" => Ok(Self::Codex),
            "pi" => Ok(Self::Pi),
            _ => Err(format!("unknown setup tool: {value}")),
        }
    }

    fn default_command(self) -> ConfigCommand {
        match self {
            Self::Codex => ConfigCommand::new("codex"),
            Self::Pi => ConfigCommand::new("pi"),
        }
    }

    fn package(self) -> &'static str {
        match self {
            Self::Codex => "@openai/codex",
            Self::Pi => "@mariozechner/pi-coding-agent",
        }
    }

    fn cli(self) -> &'static str {
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

    fn tool_state(self) -> ConfigToolState {
        match self {
            Self::Codex => ConfigToolState {
                codex: true,
                pi: false,
            },
            Self::Pi => ConfigToolState {
                codex: false,
                pi: true,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConfigCommand {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

impl ConfigCommand {
    fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            Err("default command must not be empty".to_string())
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
enum ConfigNetworkMode {
    Public,
    None,
    Allowlist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConfigNetwork {
    #[serde(default = "default_network_mode")]
    mode: ConfigNetworkMode,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_ips: Vec<String>,
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
struct ConfigAuth {
    #[serde(default)]
    github: bool,
    #[serde(default)]
    aws_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ConfigShareAccess {
    Ro,
    Rw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConfigShare {
    host_path: String,
    #[serde(default)]
    guest_path: Option<String>,
    access: ConfigShareAccess,
    #[serde(default = "default_required_share")]
    required: bool,
    #[serde(default)]
    shadows: Vec<ConfigShareShadow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConfigShareShadow {
    path: String,
}

fn default_required_share() -> bool {
    true
}

fn validate_share_shadow_path(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("share shadow path must not be empty".to_string());
    }
    if path.contains('=') {
        return Err("share shadow path must not contain '='".to_string());
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err("share shadow path must be relative to the parent share".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("share shadow path must stay under the parent share".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("share shadow path must not be empty".to_string());
    }
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConfigPort {
    host: u16,
    guest: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
struct ConfigToolState {
    #[serde(default)]
    codex: bool,
    #[serde(default)]
    pi: bool,
}

impl From<ConfigToolState> for ToolStateMounts {
    fn from(value: ConfigToolState) -> Self {
        Self {
            codex: value.codex,
            pi: value.pi,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WrapperSandboxConfig {
    schema_version: u32,
    #[serde(default)]
    setup_tool: Option<SetupTool>,
    #[serde(default)]
    default_command: ConfigCommand,
    #[serde(default)]
    tool_state: ConfigToolState,
    #[serde(default)]
    network: ConfigNetwork,
    #[serde(default)]
    auth: ConfigAuth,
    #[serde(default)]
    shares: Vec<ConfigShare>,
    #[serde(default)]
    published_ports: Vec<ConfigPort>,
}

impl WrapperSandboxConfig {
    fn setup_tool(tool: SetupTool) -> Self {
        Self {
            schema_version: 2,
            setup_tool: Some(tool),
            default_command: tool.default_command(),
            tool_state: tool.tool_state(),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: Vec::new(),
            published_ports: Vec::new(),
        }
    }

    fn codex_default() -> Self {
        Self::setup_tool(SetupTool::Codex)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2 {
            return Err(format!(
                "unsupported sandbox config schema_version {}",
                self.schema_version
            ));
        }
        self.default_command.validate()?;
        for share in &self.shares {
            if share.host_path.trim().is_empty() {
                return Err("sandbox config share host_path must not be empty".to_string());
            }
            if share.guest_path.as_deref().is_some_and(str::is_empty) {
                return Err("sandbox config share guest_path must not be empty".to_string());
            }
            if !share.shadows.is_empty() && share.access != ConfigShareAccess::Rw {
                return Err("sandbox config share shadows require rw access".to_string());
            }
            let mut shadow_paths = BTreeSet::new();
            for shadow in &share.shadows {
                let path = validate_share_shadow_path(&shadow.path)?;
                if !shadow_paths.insert(path) {
                    return Err(format!(
                        "duplicate sandbox config share shadow path: {}",
                        shadow.path
                    ));
                }
            }
        }
        Ok(())
    }
}

fn wrapper_sandbox_config_path(project: &PathBuf) -> PathBuf {
    project.join(".sandbox/config.json")
}

fn read_wrapper_sandbox_config(project: &PathBuf) -> Result<Option<WrapperSandboxConfig>, String> {
    let path = wrapper_sandbox_config_path(project);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to read sandbox config {}: {error}",
                path.display()
            ));
        }
    };
    let config = parse_wrapper_sandbox_config_text(&text)
        .map_err(|error| format!("failed to parse sandbox config {}: {error}", path.display()))?;
    config
        .validate()
        .map_err(|error| format!("invalid sandbox config {}: {error}", path.display()))?;
    Ok(Some(config))
}

fn parse_wrapper_sandbox_config_text(text: &str) -> Result<WrapperSandboxConfig, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| error.to_string())?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "missing schema_version".to_string())?;
    if schema_version == 1 {
        return parse_legacy_wrapper_sandbox_config(value);
    }
    if schema_version != 2 {
        return Err(format!("unsupported schema_version {schema_version}"));
    }
    normalize_default_command_value(&mut value)?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn parse_legacy_wrapper_sandbox_config(
    value: serde_json::Value,
) -> Result<WrapperSandboxConfig, String> {
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
        WrapperSandboxConfig::codex_default()
    } else {
        WrapperSandboxConfig {
            schema_version: 2,
            setup_tool: None,
            default_command: ConfigCommand::new(command.clone()),
            tool_state: ConfigToolState::default(),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: Vec::new(),
            published_ports: Vec::new(),
        }
    };
    config.default_command = ConfigCommand::new(command);
    Ok(config)
}

fn normalize_default_command_value(value: &mut serde_json::Value) -> Result<(), String> {
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

fn write_wrapper_sandbox_config(
    project: &PathBuf,
    config: &WrapperSandboxConfig,
) -> Result<(), String> {
    let path = wrapper_sandbox_config_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create sandbox config directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| format!("failed to serialize sandbox config: {error}"))?;
    fs::write(&path, format!("{text}\n"))
        .map_err(|error| format!("failed to write sandbox config {}: {error}", path.display()))
}

fn parse_wrapper_args(program: &str, args: &[String]) -> Result<WrapperArgs, String> {
    parse_wrapper_args_with_terminal(
        program,
        args,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    )
}

fn parse_wrapper_args_with_terminal(
    program: &str,
    args: &[String],
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> Result<WrapperArgs, String> {
    for arg in args {
        match arg.as_str() {
            "--docker" => {
                return Err(
                    "--docker has been removed; the VM is now the default execution model"
                        .to_string(),
                );
            }
            "--docker-machine" => {
                return Err(
                    "--docker-machine has been removed with the legacy QEMU path".to_string(),
                );
            }
            "--pass-env" => {
                return Err(
                    "--pass-env has been removed; use explicit VM guest shares/auth options instead"
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    let separator_index = args.iter().position(|arg| arg == "--");
    let (parse_args, payload_command_values) = if let Some(index) = separator_index {
        (&args[..index], args[index + 1..].to_vec())
    } else {
        (args, Vec::new())
    };

    let mut argv = vec![wrapper_program_name(program).to_string()];
    argv.extend(parse_args.iter().cloned());
    let matches = match wrapper_clap_command().try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            error.print().map_err(|error| error.to_string())?;
            let project = env::current_dir().map_err(|error| error.to_string())?;
            return Ok(WrapperArgs {
                project,
                launch_args: Vec::new(),
                ui_mode: wrapper_ui_mode(false, stdin_is_tty, stdout_is_tty),
                tool_selected: false,
                command_override: None,
                setup_tool: None,
                tls_bootstrap: false,
                reset: false,
                edit_config: false,
                help: true,
            });
        }
        Err(error) => return Err(error.to_string().trim().to_string()),
    };

    let mut launch_args = Vec::new();
    let mut project = matches
        .get_one::<PathBuf>("project")
        .map(|path| absolute_cli_path(&path.display().to_string()))
        .transpose()?
        .unwrap_or(env::current_dir().map_err(|error| error.to_string())?);
    let mut command_override: Option<WrapperCommandOverride> = None;
    let setup_tool = matches
        .get_one::<String>("setup_tool")
        .map(|tool| SetupTool::parse(tool))
        .transpose()?;
    let reset = matches.get_flag("reset");
    let help = false;
    let no_net = matches.get_flag("no_net");
    let no_tui = matches.get_flag("no_tui");
    let mut tls_bootstrap = false;
    let edit_config = matches.get_flag("config");
    let saw_network_override =
        no_net || matches.contains_id("allow_ip") || matches.contains_id("allow_domain");

    if let Some((command, command_args)) = payload_command_values.split_first() {
        command_override = Some(WrapperCommandOverride::Argv(ConfigCommand {
            command: command.clone(),
            args: command_args.to_vec(),
        }));
    }

    if let Some(path) = matches.get_one::<PathBuf>("project") {
        project = absolute_cli_path(&path.display().to_string())?;
        launch_args.extend(["--project".to_string(), project.display().to_string()]);
    }
    if no_net {
        launch_args.push("--no-net".to_string());
    }
    if let Some(values) = matches.get_many::<String>("allow_ip") {
        for value in values {
            launch_args.extend(["--allow-ip".to_string(), value.clone()]);
        }
    }
    if let Some(values) = matches.get_many::<String>("allow_domain") {
        for value in values {
            launch_args.extend(["--allow-domain".to_string(), value.clone()]);
        }
    }
    if matches.get_flag("gh") {
        launch_args.push("--gh".to_string());
    }
    for (id, flag) in [
        ("aws", "--aws"),
        ("ro", "--ro"),
        ("rw", "--rw"),
        ("qemu", "--qemu"),
        ("artifact_manifest", "--artifact-manifest"),
    ] {
        if let Some(values) = matches.get_many::<String>(id) {
            for value in values {
                launch_args.extend([flag.to_string(), value.clone()]);
            }
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("docker_publish") {
        for value in values {
            launch_args.extend(["--publish".to_string(), value.to_string()]);
        }
    }
    if help {
        return Ok(WrapperArgs {
            project,
            launch_args,
            ui_mode: wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty),
            tool_selected: false,
            command_override,
            setup_tool,
            tls_bootstrap,
            reset,
            edit_config,
            help,
        });
    }
    if reset {
        return Ok(WrapperArgs {
            project,
            launch_args,
            ui_mode: wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty),
            tool_selected: false,
            command_override,
            setup_tool,
            tls_bootstrap,
            reset,
            edit_config,
            help,
        });
    }
    if !launch_args.iter().any(|arg| arg == "--project") {
        launch_args.extend(["--project".to_string(), project.display().to_string()]);
    }
    let ui_mode = wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty);
    let sandbox_config = if let Some(setup_tool) = setup_tool {
        Some(WrapperSandboxConfig::setup_tool(setup_tool))
    } else {
        read_wrapper_sandbox_config(&project)?
    };
    if let Some(config) = sandbox_config.as_ref() {
        apply_configured_launch_defaults(
            &mut launch_args,
            config,
            &project,
            saw_network_override,
            no_net,
        )?;
    }
    let mut tool_selected = sandbox_config.is_some();
    if ui_mode == WrapperUiMode::Plain && command_override.is_none() && sandbox_config.is_none() {
        return Err(
            "project is not configured; run agentvm --setup-tool codex|pi, use -- COMMAND, or start interactive TUI setup".to_string(),
        );
    }
    if let Some(command) = command_override.as_ref() {
        apply_wrapper_command_override(&mut launch_args, command);
    } else if let Some(config) = sandbox_config.as_ref() {
        apply_configured_default_command(&mut launch_args, config);
        tool_selected = true;
    }
    if !no_net
        && !launch_args.iter().any(|arg| arg == "--no-net")
        && !launch_args.iter().any(|arg| arg == "--allow-ip")
        && !launch_args.iter().any(|arg| arg == "--allow-domain")
        && !launch_args
            .iter()
            .any(|arg| arg == "--allow-public-internet")
    {
        launch_args.push("--allow-public-internet".to_string());
        tls_bootstrap = true;
    } else if launch_args.iter().any(|arg| {
        arg == "--allow-public-internet" || arg == "--allow-ip" || arg == "--allow-domain"
    }) {
        tls_bootstrap = true;
    }
    Ok(WrapperArgs {
        project,
        launch_args,
        ui_mode,
        tool_selected,
        command_override,
        setup_tool,
        tls_bootstrap,
        reset,
        edit_config,
        help,
    })
}

fn wrapper_program_name(program: &str) -> &'static str {
    if is_agentvm_program(program) {
        "agentvm"
    } else {
        "agentvm-frontend wrap"
    }
}

fn wrapper_clap_command() -> ClapCommand {
    ClapCommand::new("agentvm")
        .after_help("Command override: agentvm -- COMMAND [ARG...]\nCompatibility: agentvm-frontend wrap [same options]")
        .arg(
            Arg::new("project")
                .long("project")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf))
                .value_hint(ValueHint::DirPath),
        )
        .arg(
            Arg::new("setup_tool")
                .long("setup-tool")
                .value_name("codex|pi"),
        )
        .arg(Arg::new("config").long("config").action(ArgAction::SetTrue))
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("allow_ip")
                .long("allow-ip")
                .value_name("IP_OR_CIDR")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_domain")
                .long("allow-domain")
                .value_name("DOMAIN")
                .action(ArgAction::Append),
        )
        .arg(Arg::new("no_tui").long("no-tui").action(ArgAction::SetTrue))
        .arg(Arg::new("gh").long("gh").action(ArgAction::SetTrue))
        .arg(Arg::new("aws").long("aws").value_name("PROFILE"))
        .arg(
            Arg::new("ro")
                .long("ro")
                .value_name("PATH")
                .action(ArgAction::Append)
                .value_hint(ValueHint::AnyPath),
        )
        .arg(
            Arg::new("rw")
                .long("rw")
                .value_name("PATH")
                .action(ArgAction::Append)
                .value_hint(ValueHint::AnyPath),
        )
        .arg(
            Arg::new("qemu")
                .long("qemu")
                .value_name("PATH")
                .value_hint(ValueHint::AnyPath),
        )
        .arg(
            Arg::new("artifact_manifest")
                .long("artifact-manifest")
                .value_name("PATH")
                .value_hint(ValueHint::FilePath),
        )
        .arg(
            Arg::new("docker_publish")
                .long("docker-publish")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(Arg::new("reset").long("reset").action(ArgAction::SetTrue))
}

fn parse_clap_matches(command: ClapCommand, args: &[String]) -> Result<ArgMatches, String> {
    let name = command.get_name().to_string();
    let mut argv = vec![name];
    argv.extend(args.iter().cloned());
    command
        .try_get_matches_from(argv)
        .map_err(|error| error.to_string().trim().to_string())
}

fn append_many(matches: &ArgMatches, id: &str) -> Vec<String> {
    matches
        .get_many::<String>(id)
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

fn apply_configured_launch_defaults(
    launch_args: &mut Vec<String>,
    config: &WrapperSandboxConfig,
    project: &Path,
    saw_network_override: bool,
    cli_no_net: bool,
) -> Result<(), String> {
    if !launch_args.iter().any(|arg| arg == "--tool-state") {
        if config.tool_state.codex {
            launch_args.extend(["--tool-state".to_string(), "codex".to_string()]);
        }
        if config.tool_state.pi {
            launch_args.extend(["--tool-state".to_string(), "pi".to_string()]);
        }
    }
    if !saw_network_override {
        match config.network.mode {
            ConfigNetworkMode::Public => {
                launch_args.push("--allow-public-internet".to_string());
            }
            ConfigNetworkMode::None => {
                launch_args.push("--no-net".to_string());
            }
            ConfigNetworkMode::Allowlist => {
                for domain in &config.network.allowed_domains {
                    launch_args.extend(["--allow-domain".to_string(), domain.clone()]);
                }
                for host in &config.network.allowed_hosts {
                    launch_args.extend(["--allow-domain".to_string(), host.clone()]);
                }
                for ip in &config.network.allowed_ips {
                    launch_args.extend(["--allow-ip".to_string(), ip.clone()]);
                }
            }
        }
    }
    if config.auth.github && !launch_args.iter().any(|arg| arg == "--gh") {
        launch_args.push("--gh".to_string());
    }
    if let Some(profile) = &config.auth.aws_profile {
        if !launch_args.iter().any(|arg| arg == "--aws") {
            launch_args.extend(["--aws".to_string(), profile.clone()]);
        }
    }
    for (share_index, share) in config.shares.iter().enumerate() {
        let host = absolute_cli_path(&share.host_path)?;
        let guest = share
            .guest_path
            .as_deref()
            .map(absolute_cli_path)
            .transpose()?
            .unwrap_or_else(|| host.clone());
        let flag = match share.access {
            ConfigShareAccess::Ro => "--share-ro",
            ConfigShareAccess::Rw => "--share-rw",
        };
        let required = if share.required {
            "required"
        } else {
            "optional"
        };
        launch_args.extend([
            flag.to_string(),
            format!("{}={}={required}", host.display(), guest.display()),
        ]);
        for shadow in &share.shadows {
            let relative_path = validate_share_shadow_path(&shadow.path)?;
            let backing = config_share_shadow_backing_path(project, share_index, &relative_path);
            launch_args.extend([
                "--share-shadow".to_string(),
                format!(
                    "{}={}={}",
                    guest.display(),
                    relative_path.display(),
                    backing.display()
                ),
            ]);
        }
    }
    let config_no_net = !saw_network_override && config.network.mode == ConfigNetworkMode::None;
    if !cli_no_net && !config_no_net {
        for port in &config.published_ports {
            launch_args.extend([
                "--publish".to_string(),
                format!("{}:{}", port.host, port.guest),
            ]);
        }
    }
    Ok(())
}

fn config_share_shadow_backing_path(
    project: &Path,
    share_index: usize,
    relative_path: &Path,
) -> PathBuf {
    project
        .join(".sandbox/share-shadows")
        .join(format!("share-{share_index:04}"))
        .join(relative_path)
}

fn apply_configured_default_command(launch_args: &mut Vec<String>, config: &WrapperSandboxConfig) {
    if !launch_args.iter().any(|arg| arg == "--payload-script") {
        if let Some(tool) = config.setup_tool {
            if config.default_command.command == tool.cli() {
                apply_payload_script(
                    launch_args,
                    setup_tool_payload_script(tool, &config.default_command.args),
                );
                return;
            }
        }
    }
    apply_payload_script(
        launch_args,
        payload_script_from_config_command(&config.default_command),
    );
}

fn apply_wrapper_command_override(launch_args: &mut Vec<String>, command: &WrapperCommandOverride) {
    apply_payload_script(launch_args, command.script());
}

fn apply_payload_script(launch_args: &mut Vec<String>, script: String) {
    upsert_launch_arg(launch_args, "--payload-script", script);
}

fn payload_script_from_config_command(command: &ConfigCommand) -> String {
    let mut script = format!("exec {}", shell_quote(&command.command));
    for arg in &command.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script
}

fn upsert_launch_arg(launch_args: &mut Vec<String>, flag: &str, value: String) {
    let mut index = 0;
    while index < launch_args.len() {
        if launch_args[index] == flag && index + 1 < launch_args.len() {
            launch_args[index + 1] = value;
            return;
        }
        index += 1;
    }
    launch_args.extend([flag.to_string(), value]);
}

fn wrapper_ui_mode(no_tui: bool, stdin_is_tty: bool, stdout_is_tty: bool) -> WrapperUiMode {
    if no_tui || !stdin_is_tty || !stdout_is_tty {
        WrapperUiMode::Plain
    } else {
        WrapperUiMode::Tui
    }
}

fn ensure_wrapper_mitm_ca(project: &PathBuf) -> Result<MitmCaPaths, String> {
    let dir = project.join(".sandbox/docker-vm/ca");
    let cert = dir.join("mitm-ca.crt");
    let key = dir.join("mitm-ca.key");
    if cert.exists() && key.exists() {
        return Ok(MitmCaPaths { cert, key });
    }

    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "failed to create wrapper MITM CA directory {}: {error}",
            dir.display()
        )
    })?;

    let key_pair =
        KeyPair::generate().map_err(|error| format!("failed to generate MITM CA key: {error}"))?;
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|error| format!("failed to create MITM CA params: {error}"))?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "agentvm project MITM CA");
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    params.key_usages.push(KeyUsagePurpose::CrlSign);

    let ca_cert = params
        .self_signed(&key_pair)
        .map_err(|error| format!("failed to generate MITM CA certificate: {error}"))?;
    fs::write(&cert, ca_cert.pem()).map_err(|error| {
        format!(
            "failed to write wrapper MITM CA certificate {}: {error}",
            cert.display()
        )
    })?;
    write_private_key(&key, &key_pair.serialize_pem())?;
    Ok(MitmCaPaths { cert, key })
}

fn write_private_key(path: &PathBuf, pem: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("failed to write private key {}: {error}", path.display()))?;
        file.write_all(pem.as_bytes())
            .map_err(|error| format!("failed to write private key {}: {error}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            format!(
                "failed to restrict private key permissions {}: {error}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, pem)
            .map_err(|error| format!("failed to write private key {}: {error}", path.display()))?;
    }
    Ok(())
}

fn reset_project(project: &PathBuf) -> Result<(), String> {
    let project = if project.is_absolute() {
        project.clone()
    } else {
        absolute_cli_path(&project.display().to_string())?
    };
    let sandbox = project.join(".sandbox");
    let runtime = RuntimePaths::under(project.join(".sandbox/docker-vm/run"));
    if let Some(parent) = runtime.lock.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create VM runtime root: {error}"))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&runtime.lock)
        .map_err(|error| format!("failed to open project VM lock: {error}"))?;
    acquire_project_file_lock(
        &file,
        format!(
            "--reset refused because a VM sandbox is active for this project ({})",
            runtime.lock.display()
        ),
    )?;
    if sandbox.exists() {
        fs::remove_dir_all(&sandbox)
            .map_err(|error| format!("failed to remove .sandbox: {error}"))?;
        println!("Removed .sandbox/");
    } else {
        println!(".sandbox/ does not exist, nothing to reset");
    }
    Ok(())
}

fn acquire_project_file_lock(file: &File, busy_message: String) -> Result<(), String> {
    match fs4::FileExt::try_lock(file) {
        Ok(()) => Ok(()),
        Err(fs4::TryLockError::WouldBlock) => Err(busy_message),
        Err(fs4::TryLockError::Error(error)) => {
            Err(format!("failed to lock project VM state: {error}"))
        }
    }
}

fn vmnet_gateway_config_from_args(args: &[String]) -> Result<VmnetRuntimeConfig, String> {
    let matches = parse_clap_matches(vmnet_gateway_clap_command(), args)?;
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .cloned()
        .ok_or_else(|| "--socket is required".to_string())?;
    let mut network = GuestNetwork::default();
    if let Some(value) = matches.get_one::<String>("guest_ip") {
        network.guest_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("gateway_ip") {
        network.gateway_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("dns_ip") {
        network.dns_ip = value.clone();
    }
    if let Some(value) = matches.get_one::<String>("guest_mac") {
        network.guest_mac = value.clone();
    }
    if let Some(value) = matches.get_one::<u8>("prefix_len") {
        network.prefix_len = *value;
    }
    let allow_ips = append_many(&matches, "allow_ip");
    let allow_domains = append_many(&matches, "allow_domain");
    let allow_public = matches.get_flag("allow_public_internet");
    let no_net = matches.get_flag("no_net");
    let mut host_listeners = Vec::new();
    if let Some(values) = matches.get_many::<PortPair>("host_docker_listener") {
        for value in values {
            host_listeners.push(HostListener::docker_api(value.host, value.guest));
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("host_payload_listener") {
        for value in values {
            host_listeners.push(HostListener::payload_control(value.host, value.guest));
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("publish") {
        for value in values {
            host_listeners.push(HostListener::published_tcp(value.host, value.guest));
        }
    }
    let pcap_path = matches.get_one::<String>("pcap").map(PathBuf::from);
    let tls_ca_cert = matches.get_one::<String>("tls_ca_cert").map(PathBuf::from);
    let tls_ca_key = matches.get_one::<String>("tls_ca_key").map(PathBuf::from);
    let tls_generate_per_host_certs = matches.get_flag("tls_generate_per_host_certs");
    validate_no_net_args(
        no_net,
        allow_public,
        &allow_ips,
        &allow_domains,
        &host_listeners,
    )?;
    let mut policy = VmnetPolicy::default_sandbox(network.clone());
    policy.egress.allow_ips = allow_ips;
    policy.egress.allow_domains = allow_domains;
    if allow_public {
        policy.egress.default_action = EgressAction::AllowPublicInternet;
        policy.egress.reason = EgressReason::ExplicitAllowProfile;
    } else if no_net {
        policy.egress.default_action = EgressAction::Deny;
        policy.egress.reason = EgressReason::NoNetFlag;
    }
    policy.host_listeners = host_listeners;
    policy.tls_mitm.ca_cert_path = tls_ca_cert;
    policy.tls_mitm.ca_key_path = tls_ca_key;
    policy.tls_mitm.generate_per_host_certs = tls_generate_per_host_certs;
    policy.capture.pcap_path = pcap_path;
    policy.capture.capture_guest_side_frames = policy.capture.pcap_path.is_some();

    Ok(VmnetRuntimeConfig::new(socket_path, network, policy))
}

fn vmnet_gateway_clap_command() -> ClapCommand {
    ClapCommand::new("vmnet-gateway")
        .arg(
            Arg::new("socket")
                .long("socket")
                .value_name("PATH")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(Arg::new("guest_ip").long("guest-ip").value_name("IP"))
        .arg(Arg::new("gateway_ip").long("gateway-ip").value_name("IP"))
        .arg(Arg::new("dns_ip").long("dns-ip").value_name("IP"))
        .arg(Arg::new("guest_mac").long("guest-mac").value_name("MAC"))
        .arg(
            Arg::new("prefix_len")
                .long("prefix-len")
                .value_name("N")
                .value_parser(clap::value_parser!(u8)),
        )
        .arg(
            Arg::new("allow_ip")
                .long("allow-ip")
                .value_name("IP_OR_CIDR")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_domain")
                .long("allow-domain")
                .value_name("DOMAIN")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_public_internet")
                .long("allow-public-internet")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("host_docker_listener")
                .long("host-docker-listener")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("host_payload_listener")
                .long("host-payload-listener")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("publish")
                .long("publish")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair))
                .action(ArgAction::Append),
        )
        .arg(Arg::new("pcap").long("pcap").value_name("PATH"))
        .arg(
            Arg::new("tls_ca_cert")
                .long("tls-ca-cert")
                .value_name("PATH"),
        )
        .arg(Arg::new("tls_ca_key").long("tls-ca-key").value_name("PATH"))
        .arg(
            Arg::new("tls_generate_per_host_certs")
                .long("tls-generate-per-host-certs")
                .action(ArgAction::SetTrue),
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadClientConfig {
    host: String,
    port: u16,
    ping: bool,
    script: Option<String>,
    cwd: String,
    env: BTreeMap<String, String>,
    rows: u16,
    cols: u16,
    no_stdin: bool,
}

fn payload_client_config_from_args(args: &[String]) -> Result<PayloadClientConfig, String> {
    let (rows, cols) = terminal_size();
    let matches = parse_clap_matches(payload_client_clap_command(), args)?;
    let mut config = PayloadClientConfig {
        host: matches
            .get_one::<String>("host")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        port: matches
            .get_one::<String>("port")
            .map(|value| value.parse().map_err(|_| "invalid --port".to_string()))
            .transpose()?
            .unwrap_or(12076),
        ping: matches.get_flag("ping"),
        script: matches.get_one::<String>("script").cloned(),
        cwd: matches
            .get_one::<String>("cwd")
            .cloned()
            .unwrap_or_else(|| "/".to_string()),
        env: BTreeMap::new(),
        rows: matches
            .get_one::<String>("rows")
            .map(|value| value.parse().map_err(|_| "invalid --rows".to_string()))
            .transpose()?
            .unwrap_or(rows),
        cols: matches
            .get_one::<String>("cols")
            .map(|value| value.parse().map_err(|_| "invalid --cols".to_string()))
            .transpose()?
            .unwrap_or(cols),
        no_stdin: matches.get_flag("no_stdin"),
    };
    if let Some(values) = matches.get_many::<String>("env") {
        for env in values {
            let (key, val) = env
                .split_once('=')
                .ok_or_else(|| "--env must be KEY=VALUE".to_string())?;
            if key.is_empty() {
                return Err("--env key must not be empty".to_string());
            }
            config.env.insert(key.to_string(), val.to_string());
        }
    }

    if !config.ping && config.script.is_none() {
        return Err("--script is required unless --ping is set".to_string());
    }

    Ok(config)
}

fn payload_client_clap_command() -> ClapCommand {
    ClapCommand::new("payload-client")
        .arg(Arg::new("host").long("host").value_name("HOST"))
        .arg(Arg::new("port").long("port").value_name("PORT"))
        .arg(Arg::new("ping").long("ping").action(ArgAction::SetTrue))
        .arg(Arg::new("script").long("script").value_name("SCRIPT"))
        .arg(Arg::new("cwd").long("cwd").value_name("PATH"))
        .arg(
            Arg::new("env")
                .long("env")
                .value_name("KEY=VALUE")
                .action(ArgAction::Append),
        )
        .arg(Arg::new("rows").long("rows").value_name("N"))
        .arg(Arg::new("cols").long("cols").value_name("N"))
        .arg(
            Arg::new("no_stdin")
                .long("no-stdin")
                .action(ArgAction::SetTrue),
        )
}

fn payload_exit_status(exit_code: i32) -> i32 {
    if exit_code < 0 {
        128 + (-exit_code)
    } else {
        exit_code
    }
}

#[derive(Debug, Default)]
struct PolicyArgs {
    allow_ips: Vec<String>,
    allow_domains: Vec<String>,
    allow_public: bool,
    no_net: bool,
    qemu_timeout: Option<Duration>,
    local_http_smoke_upstream: Option<(Ipv4Addr, u16)>,
    host_listeners: Vec<HostListener>,
    tls_ca_cert: Option<PathBuf>,
    tls_ca_key: Option<PathBuf>,
    tls_generate_per_host_certs: bool,
    pcap_path: Option<PathBuf>,
    payload: Option<PayloadLaunchArgs>,
    tool_state: ToolStateMounts,
    gh: bool,
    aws_profile: Option<String>,
    extra_ro: Vec<PathBuf>,
    extra_rw: Vec<PathBuf>,
    extra_shares: Vec<GuestPathShare>,
    extra_share_shadows: Vec<GuestPathShareShadow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadLaunchArgs {
    script: String,
    cwd: String,
    env: BTreeMap<String, String>,
    rows: u16,
    cols: u16,
    no_stdin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GuestPathShare {
    host_path: PathBuf,
    guest_path: PathBuf,
    readonly: bool,
    required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GuestPathShareShadow {
    parent_guest_path: PathBuf,
    relative_path: PathBuf,
    backing_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelfTestConfig {
    project: PathBuf,
    run_dir: PathBuf,
    artifact_manifest: PathBuf,
    qemu: PathBuf,
    image: String,
    publish_payload_port: Option<u16>,
    publish_container_port: Option<PortPair>,
    no_net: bool,
    hostile: bool,
    payload_stress: bool,
    dns_check: bool,
    docker_net_check: bool,
    fs_check: bool,
}

fn run_self_test(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_self_test_usage();
        return Ok(());
    }
    let self_test = self_test_config_from_args(args)?;
    ensure_appliance_sources_fresh(&self_test.artifact_manifest)?;
    let config = FrontendConfig::from_artifact_manifest_file(
        self_test.project.clone(),
        self_test.run_dir.clone(),
        self_test.qemu,
        &self_test.artifact_manifest,
    )
    .map_err(|error| format!("failed to load frontend config: {error}"))?;
    let artifacts = frontend_artifact_summary(&config);
    println!("self-test: {artifacts}");
    let mut policy_args = PolicyArgs {
        allow_public: !self_test.no_net,
        no_net: self_test.no_net,
        ..PolicyArgs::default()
    };
    let ca = ensure_wrapper_mitm_ca(&self_test.project)?;
    policy_args.tls_ca_cert = Some(ca.cert);
    policy_args.tls_ca_key = Some(ca.key);
    policy_args.tls_generate_per_host_certs = true;
    if let Some(host_port) = self_test.publish_payload_port {
        policy_args
            .host_listeners
            .push(HostListener::published_tcp(host_port, 1076));
    }
    if let Some(port) = self_test.publish_container_port {
        policy_args
            .host_listeners
            .push(HostListener::published_tcp(port.host, port.guest));
    }
    let mounts = runtime_mounts(&config, &policy_args)?;
    let mut guest_env = guest_payload_env(&config, &policy_args)?;
    guest_env.insert(
        "AGENTVM_SELF_TEST_PROJECT".to_string(),
        config.project.display().to_string(),
    );
    guest_env.insert(
        "AGENTVM_SELF_TEST_NETWORK".to_string(),
        if self_test.no_net { "deny" } else { "allow" }.to_string(),
    );
    if let Some(home) = guest_env.get("HOME").cloned() {
        guest_env.insert("AGENTVM_SELF_TEST_HOME".to_string(), home);
    }
    if self_test.payload_stress {
        guest_env.insert(
            "AGENTVM_PAYLOAD_STRESS_BLOB".to_string(),
            "r".repeat(64 * 1024),
        );
        guest_env.insert(
            "AGENTVM_PAYLOAD_STRESS_BYTES".to_string(),
            (192 * 1024).to_string(),
        );
    }
    let sqlite_db_name = self_test
        .run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            name.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "self-test".to_string());
    let skip_sqlite_concurrency = self_test.docker_net_check && self_test.no_net;
    let sqlite_concurrency_host_db = config
        .project
        .join(format!(".agentvm-self-test-sqlite/{sqlite_db_name}.sqlite"));
    if !skip_sqlite_concurrency {
        if let Some(parent) = sqlite_concurrency_host_db.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create sqlite concurrency dir: {error}"))?;
        }
        guest_env.insert(
            "AGENTVM_SQLITE_CONCURRENCY_DB".to_string(),
            sqlite_concurrency_host_db.display().to_string(),
        );
    }
    let mut policy = policy_from_args(config.network.clone(), policy_args);
    let _lock = ProjectLock::acquire(&config)?;
    let host_port = ensure_payload_listener(&mut policy);
    println!("self-test: phase=starting-frontend");
    let running = start_frontend_with_policy(config.clone(), mounts, policy)
        .map_err(|error| format!("self-test launch failed: {error}\n{artifacts}"))?;
    let payload_addr = socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
    println!("self-test: phase=waiting-for-payload-ready timeout=120s");
    wait_for_payload_ready(payload_addr, Duration::from_secs(120))
        .map_err(|error| format!("self-test payload readiness failed: {error}\n{artifacts}"))?;

    if let Some(host_port) = self_test.publish_payload_port {
        let publish_addr =
            socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
        println!("self-test: phase=checking-published-payload-port port={host_port}");
        ping_payload(publish_addr).map_err(|error| {
            format!("published payload-port check failed: {error}\n{artifacts}")
        })?;
        println!("self-test: published payload port {host_port} ok");
    }

    let (rows, cols) = terminal_size();
    let request = PayloadRequest {
        script: self_test_payload_script(
            &config,
            &self_test.image,
            self_test.hostile,
            self_test.payload_stress,
            self_test.dns_check,
            self_test.docker_net_check,
            self_test.publish_container_port,
            self_test.fs_check,
            skip_sqlite_concurrency,
        ),
        cwd: config.project.display().to_string(),
        env: guest_env,
        rows,
        cols,
    };
    println!("self-test: phase=running-payload");
    let mut host_sqlite = if skip_sqlite_concurrency {
        None
    } else {
        Some(
            spawn_host_sqlite_concurrency(&sqlite_concurrency_host_db).map_err(|error| {
                format!("self-test host sqlite setup failed: {error}\n{artifacts}")
            })?,
        )
    };
    let exit_code = if let Some(port) = self_test.publish_container_port {
        run_payload_with_published_container_check(payload_addr, request, port, &artifacts)
    } else {
        run_payload_tcp_with_control(
            payload_addr,
            &request,
            None,
            &mut io::stdout(),
            PayloadControlOptions::disabled(),
        )
        .map_err(|error| format!("self-test payload failed: {error}\n{artifacts}"))
    };
    let host_sqlite_result = if let Some(host_sqlite) = host_sqlite.as_mut() {
        wait_host_sqlite_concurrency(host_sqlite)
            .and_then(|_| run_host_sqlite_integrity_check(&sqlite_concurrency_host_db))
    } else {
        Ok(())
    };
    println!("self-test: phase=shutting-down-frontend");
    running
        .terminate()
        .map_err(|error| format!("self-test shutdown failed: {error}\n{artifacts}"))?;
    let exit_code = exit_code?;
    if exit_code != 0 {
        return Err(format!(
            "self-test payload exited with {exit_code}\n{artifacts}"
        ));
    }
    host_sqlite_result
        .map_err(|error| format!("self-test host sqlite failed: {error}\n{artifacts}"))?;
    if self_test.fs_check {
        verify_self_test_fs_check(&config.project)
            .map_err(|error| format!("self-test fs check failed: {error}\n{artifacts}"))?;
    }
    println!("self-test: ok");
    Ok(())
}

fn verify_self_test_fs_check(project: &Path) -> Result<(), String> {
    let root = project.join(".agentvm-fs-live");
    let host_visible = root.join("host-visible.txt");
    let contents = fs::read_to_string(&host_visible)
        .map_err(|error| format!("failed to read {}: {error}", host_visible.display()))?;
    if contents != "host-visible-ok\n" {
        return Err(format!(
            "unexpected host-visible file contents in {}: {contents:?}",
            host_visible.display()
        ));
    }
    let removed = root.join("dir/file.txt");
    if removed.exists() {
        return Err(format!(
            "guest unlink did not remove {} from host view",
            removed.display()
        ));
    }
    fs::remove_dir_all(&root)
        .map_err(|error| format!("failed to clean fs check dir {}: {error}", root.display()))?;
    Ok(())
}

fn run_payload_with_published_container_check(
    payload_addr: std::net::SocketAddr,
    request: PayloadRequest,
    port: PortPair,
    artifacts: &str,
) -> Result<i32, String> {
    let marker = "self-test: docker-publish-ready".to_string();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_artifacts = artifacts.to_string();
    let handle = thread::Builder::new()
        .name("agentvm-self-test-published-container".to_string())
        .spawn(move || {
            let mut output = ReadyMarkerWriter::new(marker, ready_tx);
            run_payload_tcp_with_control(
                payload_addr,
                &request,
                None,
                &mut output,
                PayloadControlOptions::disabled(),
            )
            .map_err(|error| format!("self-test payload failed: {error}\n{thread_artifacts}"))
        })
        .map_err(|error| format!("failed to spawn published-container payload: {error}"))?;

    ready_rx
        .recv_timeout(Duration::from_secs(90))
        .map_err(|error| {
            format!("published container did not become ready: {error}\n{artifacts}")
        })?;
    check_published_container_port(port.host)
        .map_err(|error| format!("published container check failed: {error}\n{artifacts}"))?;
    handle
        .join()
        .map_err(|_| format!("published-container payload thread panicked\n{artifacts}"))?
}

struct ReadyMarkerWriter {
    marker: String,
    ready_tx: Option<mpsc::Sender<()>>,
    recent: Vec<u8>,
}

impl ReadyMarkerWriter {
    fn new(marker: String, ready_tx: mpsc::Sender<()>) -> Self {
        Self {
            marker,
            ready_tx: Some(ready_tx),
            recent: Vec::new(),
        }
    }
}

impl Write for ReadyMarkerWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::stdout().write_all(bytes)?;
        self.recent.extend_from_slice(bytes);
        let max_len = self.marker.len().saturating_mul(2).max(1024);
        if self.recent.len() > max_len {
            let drop = self.recent.len() - max_len;
            self.recent.drain(..drop);
        }
        if self.ready_tx.is_some() && String::from_utf8_lossy(&self.recent).contains(&self.marker) {
            if let Some(tx) = self.ready_tx.take() {
                let _ = tx.send(());
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
}

fn check_published_container_port(host_port: u16) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let addr = socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
    let mut last_error = None;
    while Instant::now() < deadline {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(1)) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                stream
                    .write_all(b"GET / HTTP/1.1\r\nHost: container\r\nConnection: close\r\n\r\n")
                    .map_err(|error| error.to_string())?;
                let mut response = String::new();
                stream
                    .read_to_string(&mut response)
                    .map_err(|error| error.to_string())?;
                if response.contains("agentvm-container-publish-ok") {
                    println!(
                        "self-test: published container port {host_port} ok phase=host-to-container"
                    );
                    return Ok(());
                }
                last_error = Some(format!(
                    "unexpected response from published container port {host_port}: {response:?}"
                ));
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(last_error.unwrap_or_else(|| {
        format!("published container port {host_port} did not accept connections")
    }))
}

fn self_test_config_from_args(args: &[String]) -> Result<SelfTestConfig, String> {
    let matches = parse_clap_matches(self_test_clap_command(), args)?;
    let mut config = SelfTestConfig {
        project: matches
            .get_one::<String>("project")
            .map(|value| absolute_cli_path(value))
            .transpose()?
            .unwrap_or(env::current_dir().map_err(|error| error.to_string())?),
        run_dir: matches
            .get_one::<String>("run_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".sandbox/docker-vm/self-test")),
        artifact_manifest: matches
            .get_one::<String>("artifact_manifest")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("docker/out/artifact-manifest.json")),
        qemu: matches
            .get_one::<String>("qemu")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("qemu-system-x86_64")),
        image: matches
            .get_one::<String>("image")
            .cloned()
            .unwrap_or_else(|| "alpine:3.22".to_string()),
        publish_payload_port: matches
            .get_one::<String>("publish_payload_port")
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| "invalid --publish-payload-port".to_string())
            })
            .transpose()?,
        publish_container_port: matches
            .get_one::<PortPair>("publish_container_port")
            .copied(),
        no_net: matches.get_flag("no_net"),
        hostile: matches.get_flag("hostile"),
        payload_stress: matches.get_flag("payload_stress"),
        dns_check: matches.get_flag("dns_check"),
        docker_net_check: matches.get_flag("docker_net_check"),
        fs_check: matches.get_flag("fs_check"),
    };

    if !config.project.is_absolute() {
        config.project = absolute_cli_path(&config.project.display().to_string())?;
    }
    if !config.run_dir.is_absolute() {
        config.run_dir = config.project.join(&config.run_dir);
    }
    if !config.artifact_manifest.is_absolute() {
        config.artifact_manifest = env::current_dir()
            .map_err(|error| error.to_string())?
            .join(&config.artifact_manifest);
    }
    Ok(config)
}

fn self_test_clap_command() -> ClapCommand {
    ClapCommand::new("self-test")
        .arg(Arg::new("project").long("project").value_name("PATH"))
        .arg(Arg::new("run_dir").long("run-dir").value_name("PATH"))
        .arg(
            Arg::new("artifact_manifest")
                .long("artifact-manifest")
                .value_name("PATH"),
        )
        .arg(Arg::new("qemu").long("qemu").value_name("PATH"))
        .arg(Arg::new("image").long("image").value_name("IMAGE"))
        .arg(
            Arg::new("publish_payload_port")
                .long("publish-payload-port")
                .value_name("PORT"),
        )
        .arg(
            Arg::new("publish_container_port")
                .long("publish-container-port")
                .value_name("HOST:GUEST")
                .value_parser(clap::value_parser!(PortPair)),
        )
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("hostile")
                .long("hostile")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("payload_stress")
                .long("payload-stress")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dns_check")
                .long("dns-check")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("docker_net_check")
                .long("docker-net-check")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("fs_check")
                .long("fs-check")
                .action(ArgAction::SetTrue),
        )
}

fn self_test_payload_script(
    _config: &FrontendConfig,
    image: &str,
    hostile: bool,
    payload_stress: bool,
    dns_check: bool,
    docker_net_check: bool,
    publish_container_port: Option<PortPair>,
    fs_check: bool,
    skip_sqlite_concurrency: bool,
) -> String {
    let sqlite_smoke = r#"import os, sqlite3
root = os.path.join(os.environ["HOME"], ".cache", "agentvm-sqlite-smoke")
os.makedirs(root, exist_ok=True)
db = os.path.join(root, "state.sqlite")
conn = sqlite3.connect(db, timeout=1.0)
mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0].lower()
assert mode == "wal", mode
conn.execute("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)")
with conn:
    conn.execute("INSERT INTO kv(k, v) VALUES('key', 'value') ON CONFLICT(k) DO UPDATE SET v=excluded.v")
conn.close()
conn = sqlite3.connect(db, timeout=1.0)
value = conn.execute("SELECT v FROM kv WHERE k='key'").fetchone()[0]
assert value == "value", value
conn.close()
assert os.path.exists(db)
"#;
    let sqlite_concurrency = r#"import os, sqlite3, time
db = os.environ["AGENTVM_SQLITE_CONCURRENCY_DB"]
os.makedirs(os.path.dirname(db), exist_ok=True)
conn = sqlite3.connect(db, timeout=30.0, isolation_level=None)
conn.execute("PRAGMA busy_timeout=30000")
mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0].lower()
assert mode == "wal", mode
conn.execute("CREATE TABLE IF NOT EXISTS concurrent_writes (source TEXT NOT NULL, n INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY(source, n))")
for i in range(200):
    with conn:
        conn.execute("INSERT OR REPLACE INTO concurrent_writes(source, n, value) VALUES('guest', ?, ?)", (i, f"guest-{i}"))
    if i % 10 == 0:
        time.sleep(0.005)
assert conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
conn.close()
"#;
    let payload_stress_script = r#"import os, sys
blob = os.environ.get("AGENTVM_PAYLOAD_STRESS_BLOB", "")
assert len(blob) == 65536, len(blob)
size = int(os.environ["AGENTVM_PAYLOAD_STRESS_BYTES"])
assert size > 65536, size
sys.stdout.write("self-test: payload-stress-start\n")
sys.stdout.write("X" * size)
sys.stdout.write("\nself-test: payload-stress-ok\n")
sys.stdout.flush()
"#;
    let dns_allow_script = r#"const dns = require("dns");
dns.lookup("example.com", (err, address) => {
  if (err) {
    console.error(`dns-allow-failed example.com ${err.code || err.message}`);
    process.exit(1);
  }
  if (!address) {
    console.error("dns-allow-failed example.com empty-address");
    process.exit(1);
  }
  console.log(`self-test: dns-allow-ok example.com ${address}`);
});
"#;
    let docker_egress_ok =
        format!("self-test: docker-egress-ok image={image} policy=allow phase=container-egress");
    let docker_egress_failed =
        format!("docker-egress-failed image={image} policy=allow phase=container-egress");
    let docker_deny_ok =
        format!("self-test: docker-deny-ok image={image} policy=deny phase=container-egress");
    let docker_deny_unexpected =
        format!("docker-deny-unexpected image={image} policy=deny phase=container-egress");
    let docker_egress_allow_script = format!(
        "if wget -qO- -T 10 http://example.com >/dev/null; then printf '%s\\n' {}; else printf '%s\\n' {} >&2; exit 1; fi",
        shell_quote(&docker_egress_ok),
        shell_quote(&docker_egress_failed),
    );
    let docker_egress_deny_script = format!(
        "if wget -qO- -T 5 http://example.com >/tmp/agentvm-docker-egress 2>/tmp/agentvm-docker-egress.err; then printf '%s\\n' {} >&2; exit 1; else printf '%s\\n' {}; fi",
        shell_quote(&docker_deny_unexpected),
        shell_quote(&docker_deny_ok),
    );
    let docker_net_check_step = format!(
        "if [ \"${{AGENTVM_SELF_TEST_NETWORK:-allow}}\" = allow ]; then docker run --rm {} sh -c {}; else docker run --rm {} sh -c {}; fi",
        shell_quote(image),
        shell_quote(&docker_egress_allow_script),
        shell_quote(image),
        shell_quote(&docker_egress_deny_script),
    );
    let docker_publish_step = publish_container_port.map(|port| {
        let container = "agentvm-self-test-published";
        let ok = format!(
            "self-test: docker-publish-ok image={image} host_port={} guest_port={} phase=container-publish",
            port.host, port.guest
        );
        let ready = format!(
            "self-test: docker-publish-ready image={image} host_port={} guest_port={} phase=container-publish",
            port.host, port.guest
        );
        let server = "{ printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 28\\r\\n\\r\\nagentvm-container-publish-ok'; } | nc -l -p 8080";
        format!(
            "docker rm -f {container} >/dev/null 2>&1 || true; docker run -d --name {container} -p {}:8080 {} sh -c {}; echo {}; sleep 10; docker rm -f {container} >/dev/null 2>&1 || true; echo {}",
            port.guest,
            shell_quote(image),
            shell_quote(server),
            shell_quote(&ready),
            shell_quote(&ok),
        )
    });
    let fs_check_script = r#"import os, shutil, subprocess, time
root = ".agentvm-fs-live"
def read_text_eventually(path):
    last = None
    for _ in range(50):
        try:
            with open(path, encoding="utf-8") as handle:
                return handle.read()
        except OSError as exc:
            last = exc
            time.sleep(0.05)
    raise last
shutil.rmtree(root, ignore_errors=True)
os.makedirs(os.path.join(root, "dir"), exist_ok=True)
with open(os.path.join(root, "dir", "file.txt"), "w", encoding="utf-8") as handle:
    handle.write("one\n")
with open(os.path.join(root, "dir", "file.txt"), "a", encoding="utf-8") as handle:
    handle.write("two\n")
with open(os.path.join(root, "dir", "file.txt"), "r+", encoding="utf-8") as handle:
    handle.truncate(4)
assert read_text_eventually(os.path.join(root, "dir", "file.txt")) == "one\n"
os.rename(os.path.join(root, "dir", "file.txt"), os.path.join(root, "renamed.txt"))
assert read_text_eventually(os.path.join(root, "renamed.txt")) == "one\n"
with open(os.path.join(root, "dir", "delete-me.txt"), "w", encoding="utf-8") as handle:
    handle.write("delete-me\n")
os.unlink(os.path.join(root, "dir", "delete-me.txt"))
assert not os.path.exists(os.path.join(root, "dir", "delete-me.txt"))
entries = sorted(os.listdir(root))
assert entries == ["dir", "renamed.txt"], entries
os.symlink("/run/agentvm-config/mitm-ca.key", os.path.join(root, "key-link"))
try:
    open(os.path.join(root, "key-link"), "rb").read(1)
except OSError:
    pass
else:
    raise AssertionError("config private key readable through workspace symlink")
with open(os.path.join(root, "host-visible.txt"), "w", encoding="utf-8") as handle:
    handle.write("host-visible-ok\n")
uid_gid = subprocess.check_output(["stat", "-c", "%u:%g", os.path.join(root, "host-visible.txt")], text=True).strip()
expected = f"{os.getuid()}:{os.getgid()}"
assert uid_gid == expected, (uid_gid, expected)
print("self-test: fs-live-ok")
"#;
    let mut steps = vec![
        "set -eu".to_string(),
        "echo self-test: payload-start".to_string(),
        "test \"$HOME\" = \"${AGENTVM_SELF_TEST_HOME:?}\"".to_string(),
        "echo self-test: home-ok".to_string(),
        "test \"$(id -u)\" = \"${AGENTVM_UID:?}\"".to_string(),
        "echo self-test: uid-ok".to_string(),
        "test \"$(id -g)\" = \"${AGENTVM_GID:?}\"".to_string(),
        "echo self-test: gid-ok".to_string(),
        "test -d \"$HOME\"".to_string(),
        "echo self-test: home-dir-ok".to_string(),
        "test \"$PWD\" = \"$AGENTVM_SELF_TEST_PROJECT\"".to_string(),
        "echo self-test: cwd-ok".to_string(),
        "test -f /run/agentvm-config/mitm-ca.crt".to_string(),
        "echo self-test: ca-cert-ok".to_string(),
        "test ! -e /run/agentvm-config/mitm-ca.key".to_string(),
        "echo self-test: ca-key-hidden-ok".to_string(),
        "test -f /run/agentvm-ca-bundle.pem".to_string(),
        "echo self-test: ca-bundle-ok".to_string(),
        "test \"${NODE_EXTRA_CA_CERTS:-}\" = /run/agentvm-ca-bundle.pem".to_string(),
        "test \"${NPM_CONFIG_CAFILE:-}\" = /run/agentvm-ca-bundle.pem".to_string(),
        "if touch /run/agentvm-config/agentvm-self-test-ro 2>/tmp/agentvm-config-ro.err; then echo config-fs-write-unexpected; exit 1; fi".to_string(),
        "mkdir -p \"$HOME/.codex\"".to_string(),
        "printf state-ok > \"$HOME/.codex/agentvm-self-test-state\"".to_string(),
        "test \"$(cat \"$HOME/.codex/agentvm-self-test-state\")\" = state-ok".to_string(),
        "if [ \"${AGENTVM_SELF_TEST_NETWORK:-allow}\" = allow ]; then node -e 'const dns = require(\"dns\"); dns.lookup(\"example.com\", err => { if (err) throw err; });'; fi".to_string(),
        "printf workspace-ok > .agentvm-self-test-workspace".to_string(),
        "test \"$(cat .agentvm-self-test-workspace)\" = workspace-ok".to_string(),
        "echo self-test: sqlite-home-smoke".to_string(),
        format!("python3 -c {}", shell_quote(sqlite_smoke)),
        "printf bind-ok > .agentvm-self-test-bind".to_string(),
        "docker version >/tmp/agentvm-docker-version".to_string(),
        "docker info >/tmp/agentvm-docker-info".to_string(),
        format!(
            "docker run --rm -v \"$PWD:/work:ro\" {} sh -c {}",
            shell_quote(image),
            shell_quote("echo docker-run-ok; cat /work/.agentvm-self-test-bind")
        ),
        "rm -f .agentvm-self-test-workspace .agentvm-self-test-bind".to_string(),
        "echo self-test: payload-ok".to_string(),
    ];
    if hostile {
        steps.splice(12..12, hostile_self_test_payload_steps());
    }
    if !skip_sqlite_concurrency {
        let bind_index = steps
            .iter()
            .position(|step| step == "printf bind-ok > .agentvm-self-test-bind")
            .expect("bind step");
        steps.splice(
            bind_index..bind_index,
            [
                "echo self-test: sqlite-concurrency-smoke".to_string(),
                format!("python3 -c {}", shell_quote(sqlite_concurrency)),
            ],
        );
    }
    if dns_check {
        steps.insert(
            steps.len() - 1,
            format!("node -e {}", shell_quote(dns_allow_script)),
        );
    }
    if docker_net_check {
        steps.insert(steps.len() - 1, docker_net_check_step);
    }
    if let Some(docker_publish_step) = docker_publish_step {
        steps.insert(steps.len() - 1, docker_publish_step);
    }
    if fs_check {
        steps.insert(
            steps.len() - 1,
            format!("python3 -c {}", shell_quote(fs_check_script)),
        );
    }
    if payload_stress {
        steps.insert(
            steps.len() - 1,
            format!("python3 -c {}", shell_quote(payload_stress_script)),
        );
    }
    steps.join("; ")
}

fn spawn_host_sqlite_concurrency(db: &PathBuf) -> Result<Child, String> {
    let script = r#"import os, sqlite3, sys, time
db = sys.argv[1]
os.makedirs(os.path.dirname(db), exist_ok=True)
conn = sqlite3.connect(db, timeout=30.0, isolation_level=None)
conn.execute("PRAGMA busy_timeout=30000")
mode = conn.execute("PRAGMA journal_mode=WAL").fetchone()[0].lower()
assert mode == "wal", mode
conn.execute("CREATE TABLE IF NOT EXISTS concurrent_writes (source TEXT NOT NULL, n INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY(source, n))")
for i in range(200):
    with conn:
        conn.execute("INSERT OR REPLACE INTO concurrent_writes(source, n, value) VALUES('host', ?, ?)", (i, f"host-{i}"))
    if i % 10 == 0:
        time.sleep(0.005)
assert conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
conn.close()
"#;
    Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(db)
        .spawn()
        .map_err(|error| format!("failed to start host sqlite concurrency worker: {error}"))
}

fn wait_host_sqlite_concurrency(child: &mut Child) -> Result<(), String> {
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for host sqlite worker: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("host sqlite worker exited with {status}"))
    }
}

fn run_host_sqlite_integrity_check(db: &PathBuf) -> Result<(), String> {
    let script = r#"import sqlite3, sys
conn = sqlite3.connect(sys.argv[1], timeout=30.0)
conn.execute("PRAGMA busy_timeout=30000")
assert conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
host_count = conn.execute("SELECT COUNT(*) FROM concurrent_writes WHERE source='host'").fetchone()[0]
guest_count = conn.execute("SELECT COUNT(*) FROM concurrent_writes WHERE source='guest'").fetchone()[0]
assert host_count == 200, host_count
assert guest_count == 200, guest_count
conn.close()
"#;
    let status = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(db)
        .status()
        .map_err(|error| format!("failed to run host sqlite integrity check: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("host sqlite integrity check exited with {status}"))
    }
}

fn hostile_self_test_payload_steps() -> Vec<String> {
    vec![
        "echo self-test: hostile-start".to_string(),
        "if cat /run/agentvm-config/mitm-ca.key >/tmp/agentvm-key-leak 2>/tmp/agentvm-key-leak.err; then echo config-key-readable-unexpected; exit 1; fi".to_string(),
        "ln -sf /run/agentvm-config/mitm-ca.key .agentvm-self-test-key-link".to_string(),
        "if cat .agentvm-self-test-key-link >/tmp/agentvm-workspace-link-leak 2>/tmp/agentvm-workspace-link-leak.err; then echo workspace-symlink-escape-unexpected; exit 1; fi".to_string(),
        "rm -f .agentvm-self-test-key-link".to_string(),
        "node -e 'const net=require(\"net\"); const s=net.connect({host:\"169.254.169.254\",port:80,timeout:750},()=>{console.error(\"metadata-connect-unexpected\"); process.exit(1);}); s.on(\"timeout\",()=>process.exit(0)); s.on(\"error\",()=>process.exit(0));'".to_string(),
        "node -e 'const net=require(\"net\"); const s=net.connect({host:\"127.0.0.1\",port:22,timeout:750},()=>{console.error(\"loopback-connect-unexpected\"); process.exit(1);}); s.on(\"timeout\",()=>process.exit(0)); s.on(\"error\",()=>process.exit(0));'".to_string(),
        "if [ \"${AGENTVM_SELF_TEST_NETWORK:-allow}\" = deny ]; then node -e 'const dns=require(\"dns\"); dns.lookup(\"example.com\", err => { if (err) { console.log(\"self-test: dns-deny-ok example.com \" + (err.code || err.message)); process.exit(0); } console.error(\"dns-deny-unexpected example.com\"); process.exit(1); });'; fi".to_string(),
        "echo self-test: hostile-ok".to_string(),
    ]
}

fn frontend_config_from_args(args: &[String]) -> Result<(FrontendConfig, PolicyArgs), String> {
    let matches = parse_clap_matches(frontend_clap_command(), args)?;
    let mut project = matches
        .get_one::<String>("project")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut run_dir = matches
        .get_one::<String>("run_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".sandbox/docker-vm/run"));
    let artifact_manifest = matches
        .get_one::<String>("artifact_manifest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docker/out/artifact-manifest.json"));
    let qemu = matches
        .get_one::<String>("qemu")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("qemu-system-x86_64"));
    let mut policy = PolicyArgs::default();
    for tool_state in append_many(&matches, "tool_state") {
        match tool_state.as_str() {
            "codex" => policy.tool_state.codex = true,
            "pi" => policy.tool_state.pi = true,
            value => return Err(format!("unknown --tool-state: {value}")),
        }
    }
    policy.gh = matches.get_flag("gh");
    policy.aws_profile = matches.get_one::<String>("aws").cloned();
    for path in append_many(&matches, "ro") {
        policy.extra_ro.push(absolute_cli_path(&path)?);
    }
    for path in append_many(&matches, "rw") {
        policy.extra_rw.push(absolute_cli_path(&path)?);
    }
    for share in append_many(&matches, "share_ro") {
        policy
            .extra_shares
            .push(parse_guest_path_share(&share, true)?);
    }
    for share in append_many(&matches, "share_rw") {
        policy
            .extra_shares
            .push(parse_guest_path_share(&share, false)?);
    }
    for shadow in append_many(&matches, "share_shadow") {
        policy
            .extra_share_shadows
            .push(parse_guest_path_share_shadow(&shadow)?);
    }
    let guest_http_smoke_url = matches.get_one::<String>("guest_http_smoke_url").cloned();
    policy.allow_ips = append_many(&matches, "allow_ip");
    policy.allow_domains = append_many(&matches, "allow_domain");
    policy.allow_public = matches.get_flag("allow_public_internet");
    policy.no_net = matches.get_flag("no_net");
    if let Some(seconds) = matches.get_one::<String>("qemu_timeout_seconds") {
        policy.qemu_timeout = Some(Duration::from_secs(
            seconds
                .parse::<u64>()
                .map_err(|_| "invalid --qemu-timeout-seconds".to_string())?,
        ));
    }
    if let Some(value) = matches.get_one::<String>("local_http_smoke_upstream") {
        policy.local_http_smoke_upstream = Some(parse_ip_port(value)?);
    }
    for value in append_many(&matches, "host_docker_listener") {
        let (host_port, guest_port) = parse_port_pair(&value)?;
        policy
            .host_listeners
            .push(HostListener::docker_api(host_port, guest_port));
    }
    for value in append_many(&matches, "host_payload_listener") {
        let (host_port, guest_port) = parse_port_pair(&value)?;
        policy
            .host_listeners
            .push(HostListener::payload_control(host_port, guest_port));
    }
    for value in append_many(&matches, "publish") {
        let (host_port, guest_port) = parse_port_pair(&value)?;
        policy
            .host_listeners
            .push(HostListener::published_tcp(host_port, guest_port));
    }
    policy.pcap_path = matches.get_one::<String>("pcap").map(PathBuf::from);
    if let Some(script) = matches.get_one::<String>("payload_script") {
        payload_launch_args(&mut policy).script = script.clone();
    }
    if let Some(cwd) = matches.get_one::<String>("payload_cwd") {
        payload_launch_args(&mut policy).cwd = cwd.clone();
    }
    for env in append_many(&matches, "payload_env") {
        let (key, val) = env
            .split_once('=')
            .ok_or_else(|| "--payload-env must be KEY=VALUE".to_string())?;
        if key.is_empty() {
            return Err("--payload-env key must not be empty".to_string());
        }
        payload_launch_args(&mut policy)
            .env
            .insert(key.to_string(), val.to_string());
    }
    if let Some(rows) = matches.get_one::<String>("payload_rows") {
        payload_launch_args(&mut policy).rows = rows
            .parse()
            .map_err(|_| "invalid --payload-rows".to_string())?;
    }
    if let Some(cols) = matches.get_one::<String>("payload_cols") {
        payload_launch_args(&mut policy).cols = cols
            .parse()
            .map_err(|_| "invalid --payload-cols".to_string())?;
    }
    if matches.get_flag("payload_no_stdin") {
        payload_launch_args(&mut policy).no_stdin = true;
    }
    policy.tls_ca_cert = matches.get_one::<String>("tls_ca_cert").map(PathBuf::from);
    policy.tls_ca_key = matches.get_one::<String>("tls_ca_key").map(PathBuf::from);
    policy.tls_generate_per_host_certs = matches.get_flag("tls_generate_per_host_certs");

    if !project.is_absolute() {
        project = absolute_cli_path(&project.display().to_string())?;
    }
    if !run_dir.is_absolute() {
        run_dir = project.join(&run_dir);
    }

    let mut config =
        FrontendConfig::from_artifact_manifest_file(project, run_dir, qemu, &artifact_manifest)
            .map_err(|error| format!("failed to load frontend config: {error}"))?;
    config.guest_http_smoke_url = guest_http_smoke_url;
    validate_no_net_args(
        policy.no_net,
        policy.allow_public,
        &policy.allow_ips,
        &policy.allow_domains,
        &policy.host_listeners,
    )?;
    if config.guest_http_smoke_url.is_some() {
        ensure_appliance_sources_fresh(&artifact_manifest)?;
    }
    if policy
        .payload
        .as_ref()
        .is_some_and(|payload| payload.script.is_empty())
    {
        return Err("--payload-script must not be empty".to_string());
    }
    Ok((config, policy))
}

fn frontend_clap_command() -> ClapCommand {
    ClapCommand::new("launch")
        .arg(Arg::new("project").long("project").value_name("PATH"))
        .arg(Arg::new("run_dir").long("run-dir").value_name("PATH"))
        .arg(
            Arg::new("artifact_manifest")
                .long("artifact-manifest")
                .value_name("PATH"),
        )
        .arg(Arg::new("qemu").long("qemu").value_name("PATH"))
        .arg(
            Arg::new("tool_state")
                .long("tool-state")
                .value_name("codex|pi")
                .action(ArgAction::Append),
        )
        .arg(Arg::new("gh").long("gh").action(ArgAction::SetTrue))
        .arg(Arg::new("aws").long("aws").value_name("PROFILE"))
        .arg(
            Arg::new("ro")
                .long("ro")
                .value_name("PATH")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("rw")
                .long("rw")
                .value_name("PATH")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("share_ro")
                .long("share-ro")
                .value_name("HOST=GUEST[=required|optional]")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("share_rw")
                .long("share-rw")
                .value_name("HOST=GUEST[=required|optional]")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("share_shadow")
                .long("share-shadow")
                .value_name("PARENT_GUEST=RELATIVE_PATH=BACKING_PATH")
                .hide(true)
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("guest_http_smoke_url")
                .long("guest-http-smoke-url")
                .value_name("URL"),
        )
        .arg(
            Arg::new("allow_ip")
                .long("allow-ip")
                .value_name("IP_OR_CIDR")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_domain")
                .long("allow-domain")
                .value_name("DOMAIN")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("allow_public_internet")
                .long("allow-public-internet")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("qemu_timeout_seconds")
                .long("qemu-timeout-seconds")
                .value_name("N"),
        )
        .arg(
            Arg::new("local_http_smoke_upstream")
                .long("local-http-smoke-upstream")
                .value_name("IP:PORT"),
        )
        .arg(
            Arg::new("host_docker_listener")
                .long("host-docker-listener")
                .value_name("HOST:GUEST")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("host_payload_listener")
                .long("host-payload-listener")
                .value_name("HOST:GUEST")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("publish")
                .long("publish")
                .value_name("HOST:GUEST")
                .action(ArgAction::Append),
        )
        .arg(Arg::new("pcap").long("pcap").value_name("PATH"))
        .arg(
            Arg::new("payload_script")
                .long("payload-script")
                .value_name("SCRIPT"),
        )
        .arg(
            Arg::new("payload_cwd")
                .long("payload-cwd")
                .value_name("PATH"),
        )
        .arg(
            Arg::new("payload_env")
                .long("payload-env")
                .value_name("KEY=VALUE")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("payload_rows")
                .long("payload-rows")
                .value_name("N"),
        )
        .arg(
            Arg::new("payload_cols")
                .long("payload-cols")
                .value_name("N"),
        )
        .arg(
            Arg::new("payload_no_stdin")
                .long("payload-no-stdin")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("tls_ca_cert")
                .long("tls-ca-cert")
                .value_name("PATH"),
        )
        .arg(Arg::new("tls_ca_key").long("tls-ca-key").value_name("PATH"))
        .arg(
            Arg::new("tls_generate_per_host_certs")
                .long("tls-generate-per-host-certs")
                .action(ArgAction::SetTrue),
        )
}

fn payload_launch_args(policy: &mut PolicyArgs) -> &mut PayloadLaunchArgs {
    let (rows, cols) = terminal_size();
    policy.payload.get_or_insert_with(|| PayloadLaunchArgs {
        script: String::new(),
        cwd: "/".to_string(),
        env: BTreeMap::new(),
        rows,
        cols,
        no_stdin: false,
    })
}

fn launch_payload_args(
    _config: &FrontendConfig,
    policy: &PolicyArgs,
) -> Result<Option<PayloadLaunchArgs>, String> {
    Ok(policy.payload.clone())
}

fn runtime_mounts(
    config: &FrontendConfig,
    policy: &PolicyArgs,
) -> Result<Vec<RuntimeMount>, String> {
    std::fs::create_dir_all(config.project.join(".sandbox/home"))
        .map_err(|error| format!("failed to create persistent guest HOME backing dir: {error}"))?;
    let host_home = host_home_dir()?;
    let mut mounts = guest_runtime_mounts(
        config.project.clone(),
        &GuestShareSpec {
            tool_state: policy.tool_state,
            host_home,
            gh: policy.gh,
            extra_ro: policy.extra_ro.clone(),
            extra_rw: policy.extra_rw.clone(),
        },
    );
    let mut next_id = mounts.len() + 1;
    for share in &policy.extra_shares {
        mounts.push(RuntimeMount {
            id: format!("m{next_id:04}_config_share"),
            host_path: share.host_path.clone(),
            guest_path: share.guest_path.clone(),
            readonly: share.readonly,
            source_class: if share.readonly {
                agentvm_frontend::runtime_manifest::ManifestSourceClass::UserRo
            } else {
                agentvm_frontend::runtime_manifest::ManifestSourceClass::UserRw
            },
            required: share.required,
            bind: true,
        });
        next_id += 1;
    }
    for shadow in &policy.extra_share_shadows {
        let parent = policy
            .extra_shares
            .iter()
            .find(|share| !share.readonly && share.guest_path == shadow.parent_guest_path)
            .ok_or_else(|| {
                format!(
                    "share shadow parent must match a configured rw share: {}",
                    shadow.parent_guest_path.display()
                )
            })?;
        if !shadow_backing_is_project_local(config, &shadow.backing_path) {
            return Err(format!(
                "share shadow backing path must stay under project .sandbox: {}",
                shadow.backing_path.display()
            ));
        }
        let guest_path = parent.guest_path.join(&shadow.relative_path);
        std::fs::create_dir_all(&shadow.backing_path).map_err(|error| {
            format!(
                "failed to create share shadow backing dir {}: {error}",
                shadow.backing_path.display()
            )
        })?;
        mounts.push(RuntimeMount {
            id: format!("m{next_id:04}_share_shadow"),
            host_path: shadow.backing_path.clone(),
            guest_path,
            readonly: false,
            source_class: agentvm_frontend::runtime_manifest::ManifestSourceClass::UserRw,
            required: true,
            bind: true,
        });
        next_id += 1;
    }
    Ok(mounts)
}

fn shadow_backing_is_project_local(config: &FrontendConfig, backing_path: &Path) -> bool {
    let sandbox = config.project.join(".sandbox");
    backing_path == sandbox || backing_path.starts_with(&sandbox)
}

fn guest_payload_env(
    _config: &FrontendConfig,
    policy: &PolicyArgs,
) -> Result<BTreeMap<String, String>, String> {
    let mut env_vars = BTreeMap::new();
    if policy.tls_ca_cert.is_some() {
        env_vars.insert(
            "SSL_CERT_FILE".to_string(),
            "/run/agentvm-ca-bundle.pem".to_string(),
        );
        env_vars.insert(
            "REQUESTS_CA_BUNDLE".to_string(),
            "/run/agentvm-ca-bundle.pem".to_string(),
        );
        env_vars.insert(
            "NODE_EXTRA_CA_CERTS".to_string(),
            "/run/agentvm-ca-bundle.pem".to_string(),
        );
        env_vars.insert(
            "NPM_CONFIG_CAFILE".to_string(),
            "/run/agentvm-ca-bundle.pem".to_string(),
        );
    }
    if policy.payload.is_none()
        && !policy.gh
        && policy.aws_profile.is_none()
        && policy.tls_ca_cert.is_none()
    {
        return Ok(env_vars);
    }
    let user = env::var("USER").unwrap_or_else(|_| "sandbox".to_string());
    let guest_home = host_home_dir()?.display().to_string();
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    env_vars.insert("HOME".to_string(), guest_home.clone());
    env_vars.insert("USER".to_string(), user.clone());
    env_vars.insert("LOGNAME".to_string(), user);
    env_vars.insert("AGENTVM_UID".to_string(), uid.to_string());
    env_vars.insert("AGENTVM_GID".to_string(), gid.to_string());
    env_vars.insert(
        "TERM".to_string(),
        env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()),
    );
    env_vars.insert(
        "LANG".to_string(),
        env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string()),
    );
    env_vars.insert("XDG_CACHE_HOME".to_string(), format!("{guest_home}/.cache"));
    env_vars.insert(
        "XDG_CONFIG_HOME".to_string(),
        format!("{guest_home}/.config"),
    );
    env_vars.insert(
        "XDG_DATA_HOME".to_string(),
        format!("{guest_home}/.local/share"),
    );
    env_vars.insert(
        "XDG_STATE_HOME".to_string(),
        format!("{guest_home}/.local/state"),
    );
    env_vars.insert(
        "PATH".to_string(),
        [
            format!("{guest_home}/.local/share/mise/shims"),
            format!("{guest_home}/.local/bin"),
            format!("{guest_home}/bin"),
            "/usr/local/sbin".to_string(),
            "/usr/local/bin".to_string(),
            "/usr/sbin".to_string(),
            "/usr/bin".to_string(),
            "/sbin".to_string(),
            "/bin".to_string(),
        ]
        .join(":"),
    );
    env_vars.insert("TMPDIR".to_string(), "/tmp".to_string());
    env_vars.insert(
        "DOCKER_HOST".to_string(),
        "tcp://127.0.0.1:1075".to_string(),
    );
    env_vars.insert("SSH_AUTH_SOCK".to_string(), String::new());
    env_vars.insert("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string());
    env_vars.insert(
        "AWS_SHARED_CREDENTIALS_FILE".to_string(),
        "/dev/null".to_string(),
    );
    env_vars.insert("AWS_CONFIG_FILE".to_string(), "/dev/null".to_string());
    env_vars.insert("GOOGLE_APPLICATION_CREDENTIALS".to_string(), String::new());
    env_vars.insert("KUBECONFIG".to_string(), "/dev/null".to_string());

    if policy.gh {
        if let Some(token) = acquire_gh_token() {
            env_vars.insert("GH_TOKEN".to_string(), token);
        } else {
            eprintln!(
                "warning: --gh specified but could not extract token (run 'gh auth login' on the host)"
            );
        }
    }
    if let Some(profile) = &policy.aws_profile {
        env_vars.extend(acquire_aws_credentials(profile)?);
    }

    Ok(env_vars)
}

fn merged_payload_env(
    mut base: BTreeMap<String, String>,
    overrides: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    base.extend(overrides);
    base
}

fn setup_tool_payload_script(tool: SetupTool, tool_args: &[String]) -> String {
    tool_bootstrap_payload_script(tool.cli(), tool.package(), tool.auto_flags(), tool_args)
}

fn tool_bootstrap_payload_script(
    cli: &str,
    npm_package: &str,
    auto_flags: &[&str],
    tool_args: &[String],
) -> String {
    let mut command = Vec::new();
    command.push(shell_quote(cli));
    command.extend(auto_flags.iter().map(|flag| shell_quote(flag)));
    command.extend(tool_args.iter().map(|arg| shell_quote(arg)));
    let npm_package = format!("{npm_package}@latest");
    let install_message =
        format!("agentvm: installing {cli} CLI in guest HOME (first run only)...");
    [
        r#"export NPM_CONFIG_PREFIX="$HOME/.local""#.to_string(),
        r#"mkdir -p "$NPM_CONFIG_PREFIX""#.to_string(),
        format!(
            "if ! command -v {} >/dev/null 2>&1; then if ! command -v npm >/dev/null 2>&1; then printf '%s\\n' {} >&2; exit 127; fi; printf '%s\\n' {} >&2; npm install --global --no-progress {}; fi",
            shell_quote(cli),
            shell_quote(&format!("agentvm: npm is required to install {cli} CLI")),
            shell_quote(&install_message),
            shell_quote(&npm_package)
        ),
        "hash -r 2>/dev/null || true".to_string(),
        r#"export MISE_TRUSTED_CONFIG_PATHS="$PWD""#.to_string(),
        format!("exec {}", command.join(" ")),
    ]
    .join(" && ")
}

fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .expect("shell argument contains an embedded NUL byte")
        .into_owned()
}

fn host_home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME must be set to an absolute path for guest state sharing".to_string())
}

fn absolute_cli_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Ok(path);
    }
    env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|error| format!("failed to resolve {value}: {error}"))
}

fn acquire_gh_token() -> Option<String> {
    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn acquire_aws_credentials(profile: &str) -> Result<BTreeMap<String, String>, String> {
    let output = Command::new("aws")
        .args([
            "configure",
            "export-credentials",
            "--profile",
            profile,
            "--format",
            "process",
        ])
        .output()
        .map_err(|error| format!("failed to run aws CLI: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to export AWS credentials for profile '{profile}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let credentials: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse AWS credentials: {error}"))?;
    let mut env_vars = BTreeMap::new();
    if let Some(value) = credentials
        .get("AccessKeyId")
        .and_then(|value| value.as_str())
    {
        env_vars.insert("AWS_ACCESS_KEY_ID".to_string(), value.to_string());
    }
    if let Some(value) = credentials
        .get("SecretAccessKey")
        .and_then(|value| value.as_str())
    {
        env_vars.insert("AWS_SECRET_ACCESS_KEY".to_string(), value.to_string());
    }
    if let Some(value) = credentials
        .get("SessionToken")
        .and_then(|value| value.as_str())
    {
        env_vars.insert("AWS_SESSION_TOKEN".to_string(), value.to_string());
    }
    if !env_vars.contains_key("AWS_ACCESS_KEY_ID") {
        return Err(format!("no credentials found for AWS profile '{profile}'"));
    }

    if let Ok(region) = Command::new("aws")
        .args(["configure", "get", "region", "--profile", profile])
        .output()
    {
        if region.status.success() {
            let region = String::from_utf8_lossy(&region.stdout).trim().to_string();
            if !region.is_empty() {
                env_vars.insert("AWS_DEFAULT_REGION".to_string(), region);
            }
        }
    }

    Ok(env_vars)
}

fn parse_ip_port(value: &str) -> Result<(Ipv4Addr, u16), String> {
    let (ip, port) = value
        .rsplit_once(':')
        .ok_or_else(|| "expected IP:PORT".to_string())?;
    Ok((
        ip.parse()
            .map_err(|_| format!("invalid IPv4 address in {value}"))?,
        port.parse()
            .map_err(|_| format!("invalid port in {value}"))?,
    ))
}

fn parse_port_pair(value: &str) -> Result<(u16, u16), String> {
    let (host_port, guest_port) = value
        .split_once(':')
        .ok_or_else(|| "expected HOST_PORT:GUEST_PORT".to_string())?;
    Ok((
        host_port
            .parse()
            .map_err(|_| format!("invalid host port in {value}"))?,
        guest_port
            .parse()
            .map_err(|_| format!("invalid guest port in {value}"))?,
    ))
}

fn parse_guest_path_share(value: &str, readonly: bool) -> Result<GuestPathShare, String> {
    let mut parts = value.splitn(3, '=');
    let host = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "share host path must not be empty".to_string())?;
    let guest = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "share must be HOST=GUEST[=required|optional]".to_string())?;
    let required = match parts.next() {
        None | Some("required") => true,
        Some("optional") => false,
        Some(value) => return Err(format!("invalid share requirement: {value}")),
    };
    Ok(GuestPathShare {
        host_path: absolute_cli_path(host)?,
        guest_path: absolute_cli_path(guest)?,
        readonly,
        required,
    })
}

fn parse_guest_path_share_shadow(value: &str) -> Result<GuestPathShareShadow, String> {
    let mut parts = value.splitn(3, '=');
    let parent_guest = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| "share shadow parent guest path must not be empty".to_string())?;
    let relative_path = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| {
            "share shadow must be PARENT_GUEST=RELATIVE_PATH=BACKING_PATH".to_string()
        })?;
    let backing_path = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| {
            "share shadow must be PARENT_GUEST=RELATIVE_PATH=BACKING_PATH".to_string()
        })?;
    Ok(GuestPathShareShadow {
        parent_guest_path: absolute_cli_path(parent_guest)?,
        relative_path: validate_share_shadow_path(relative_path)?,
        backing_path: absolute_cli_path(backing_path)?,
    })
}

fn start_local_http_smoke_upstream(
    destination: (Ipv4Addr, u16),
) -> Result<UpstreamMapping, String> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| error.to_string())?;
    let host_port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    thread::Builder::new()
        .name("agentvm-local-http-smoke".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                );
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(UpstreamMapping {
        guest_ip: destination.0,
        guest_port: destination.1,
        host_ip: Ipv4Addr::LOCALHOST,
        host_port,
    })
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

const REQUIRED_APPLIANCE_SOURCE_INPUTS: &[&str] = &[
    "docker/appliance.env",
    "docker/build-appliance.sh",
    "docker/guest-init.sh",
    "docker/guest-payload-server.py",
    "docker/guest-socket-bridge.py",
];

fn ensure_appliance_sources_fresh(artifact_manifest: &Path) -> Result<(), String> {
    let repo_root = artifact_manifest
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            format!(
                "stale appliance artifacts: cannot infer repository root from {}; rerun sudo ./docker/build-appliance.sh",
                artifact_manifest.display()
            )
        })?;
    let text = fs::read_to_string(artifact_manifest)
        .map_err(|error| format!("failed to read {}: {error}", artifact_manifest.display()))?;
    let manifest: ApplianceFreshnessManifest = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", artifact_manifest.display()))?;
    let inputs = manifest
        .source_inputs
        .as_deref()
        .filter(|inputs| !inputs.is_empty())
        .ok_or_else(|| {
            format!(
                "stale appliance artifacts: {} does not record appliance source hashes; rerun sudo ./docker/build-appliance.sh",
                artifact_manifest.display()
            )
        })?;

    let mut recorded_paths = BTreeSet::new();
    for input in inputs {
        if input.path.is_absolute()
            || input
                .path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!(
                "stale appliance artifacts: {} contains invalid source path {}; rerun sudo ./docker/build-appliance.sh",
                artifact_manifest.display(),
                input.path.display()
            ));
        }
        recorded_paths.insert(input.path.to_string_lossy().into_owned());
        let source_path = repo_root.join(&input.path);
        let actual_hash = sha256_file_hex(&source_path)?;
        if !input.sha256.eq_ignore_ascii_case(&actual_hash) {
            return Err(format!(
                "stale appliance artifacts: {} changed since {} was written (expected sha256 {}, current {}); rerun sudo ./docker/build-appliance.sh",
                input.path.display(),
                artifact_manifest.display(),
                input.sha256,
                actual_hash
            ));
        }
    }
    for required in REQUIRED_APPLIANCE_SOURCE_INPUTS {
        if !recorded_paths.contains(*required) {
            return Err(format!(
                "stale appliance artifacts: {} does not record required source hash for {}; rerun sudo ./docker/build-appliance.sh",
                artifact_manifest.display(),
                required
            ));
        }
    }
    Ok(())
}

fn sha256_file_hex(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

fn policy_from_args(network: GuestNetwork, args: PolicyArgs) -> VmnetPolicy {
    let mut policy = VmnetPolicy::default_sandbox(network);
    policy.egress.allow_ips = args.allow_ips;
    policy.egress.allow_domains = args.allow_domains;
    if args.allow_public {
        policy.egress.default_action = EgressAction::AllowPublicInternet;
        policy.egress.reason = EgressReason::ExplicitAllowProfile;
    } else if args.no_net {
        policy.egress.default_action = EgressAction::Deny;
        policy.egress.reason = EgressReason::NoNetFlag;
    }
    policy.host_listeners = args.host_listeners;
    policy.tls_mitm.ca_cert_path = args.tls_ca_cert;
    policy.tls_mitm.ca_key_path = args.tls_ca_key;
    policy.tls_mitm.generate_per_host_certs = args.tls_generate_per_host_certs;
    policy.capture.pcap_path = args.pcap_path;
    policy.capture.capture_guest_side_frames = policy.capture.pcap_path.is_some();
    policy
}

fn ensure_payload_listener(policy: &mut VmnetPolicy) -> u16 {
    if let Some(listener) = policy
        .host_listeners
        .iter()
        .find(|listener| listener.purpose == HostListenerPurpose::PayloadControl)
    {
        return listener.host_port;
    }
    const DEFAULT_HOST_PAYLOAD_PORT: u16 = 12076;
    const DEFAULT_GUEST_PAYLOAD_PORT: u16 = 1076;
    policy.host_listeners.push(HostListener::payload_control(
        DEFAULT_HOST_PAYLOAD_PORT,
        DEFAULT_GUEST_PAYLOAD_PORT,
    ));
    DEFAULT_HOST_PAYLOAD_PORT
}

fn wait_for_payload_ready(addr: std::net::SocketAddr, timeout: Duration) -> Result<(), String> {
    wait_for_payload_ready_with_probe(timeout, Duration::from_millis(250), || {
        ping_payload(addr).map_err(|error| error.to_string())
    })
}

fn wait_for_payload_ready_with_probe(
    timeout: Duration,
    interval: Duration,
    mut probe: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < timeout {
        match probe() {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if interval.is_zero() {
                    thread::yield_now();
                } else {
                    thread::sleep(interval);
                }
            }
        }
    }
    Err(format!(
        "timed out waiting for guest payload control path{}",
        last_error
            .map(|error| format!("; last error: {error}"))
            .unwrap_or_default()
    ))
}

fn validate_no_net_args(
    no_net: bool,
    allow_public: bool,
    allow_ips: &[String],
    allow_domains: &[String],
    host_listeners: &[HostListener],
) -> Result<(), String> {
    if !no_net {
        return Ok(());
    }
    if allow_public || !allow_ips.is_empty() || !allow_domains.is_empty() {
        return Err("--no-net cannot be combined with egress allow options".to_string());
    }
    if host_listeners
        .iter()
        .any(|listener| listener.purpose == HostListenerPurpose::PublishedTcp)
    {
        return Err("--no-net cannot be combined with --publish".to_string());
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: agentvm-frontend <prepare|launch|self-test|vmnet-gateway|payload-client> [options]\n\
         prepare/launch options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--gh] [--aws PROFILE] [--ro PATH] [--rw PATH] [--guest-http-smoke-url URL] [--allow-public-internet|--no-net] [--qemu-timeout-seconds N] [--local-http-smoke-upstream IP:PORT] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--payload-script SCRIPT] [--payload-cwd PATH] [--payload-env KEY=VALUE] [--payload-no-stdin] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         self-test options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--image IMAGE] [--publish-payload-port PORT] [--no-net] [--hostile]\n\
         vmnet-gateway options: --socket PATH [--allow-ip IP_OR_CIDR] [--allow-domain DOMAIN] [--allow-public-internet|--no-net] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         payload-client options: --port PORT [--host HOST] [--ping|--script SCRIPT] [--cwd PATH] [--env KEY=VALUE] [--rows N] [--cols N] [--no-stdin]"
    );
}

fn print_self_test_usage() {
    eprintln!(
        "usage: agentvm-frontend self-test [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--image IMAGE] [--publish-payload-port PORT] [--no-net] [--hostile]"
    );
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

    #[test]
    fn parses_vmnet_gateway_runtime_config() {
        let config = vmnet_gateway_config_from_args(&[
            "--socket".to_string(),
            "/tmp/vmnet.sock".to_string(),
            "--guest-ip".to_string(),
            "10.0.2.20".to_string(),
            "--allow-ip".to_string(),
            "93.184.216.34".to_string(),
            "--allow-domain".to_string(),
            "example.com".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
            "--publish".to_string(),
            "18080:8080".to_string(),
            "--tls-ca-cert".to_string(),
            "/tmp/ca.pem".to_string(),
            "--tls-ca-key".to_string(),
            "/tmp/ca-key.pem".to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ])
        .expect("config");

        assert_eq!(config.socket_path, PathBuf::from("/tmp/vmnet.sock"));
        assert_eq!(config.network.guest_ip, "10.0.2.20");
        assert_eq!(config.policy.egress.allow_ips, vec!["93.184.216.34"]);
        assert_eq!(config.policy.egress.allow_domains, vec!["example.com"]);
        assert_eq!(
            config.policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
                HostListener::published_tcp(18080, 8080),
            ]
        );
        assert_eq!(
            config.policy.tls_mitm.ca_cert_path,
            Some(PathBuf::from("/tmp/ca.pem"))
        );
        assert_eq!(
            config.policy.tls_mitm.ca_key_path,
            Some(PathBuf::from("/tmp/ca-key.pem"))
        );
        assert!(config.policy.tls_mitm.generate_per_host_certs);
    }

    #[test]
    fn vmnet_gateway_runtime_config_requires_socket() {
        assert_eq!(
            vmnet_gateway_config_from_args(&[]).expect_err("missing socket"),
            "--socket is required"
        );
    }

    #[test]
    fn parses_frontend_prepare_defaults_and_policy() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::create_dir_all(root.join("repo")).expect("repo");
        std::fs::write(root.join("docker/out/vmlinuz"), b"kernel").expect("kernel");
        std::fs::write(root.join("docker/out/initrd.img"), b"initrd").expect("initrd");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        write_frontend_manifest(&root);

        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--allow-public-internet".to_string(),
            "--guest-http-smoke-url".to_string(),
            "http://93.184.216.34/".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
            "--publish".to_string(),
            "18080:8080".to_string(),
            "--tls-ca-cert".to_string(),
            "/tmp/ca.pem".to_string(),
            "--tls-ca-key".to_string(),
            "/tmp/ca-key.pem".to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ])
        .expect("config");

        assert_eq!(config.vm.cpus, 2);
        assert_eq!(
            config.guest_http_smoke_url.as_deref(),
            Some("http://93.184.216.34/")
        );
        assert!(policy.allow_public);
        assert_eq!(
            policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
                HostListener::published_tcp(18080, 8080),
            ]
        );
        let runtime_policy = policy_from_args(config.network.clone(), policy);
        assert_eq!(
            runtime_policy.tls_mitm.ca_cert_path,
            Some(PathBuf::from("/tmp/ca.pem"))
        );
        assert_eq!(
            runtime_policy.tls_mitm.ca_key_path,
            Some(PathBuf::from("/tmp/ca-key.pem"))
        );
        assert!(runtime_policy.tls_mitm.generate_per_host_certs);
    }

    #[test]
    fn frontend_no_net_sets_policy_reason_and_allows_control_listeners() {
        let root = frontend_test_root();

        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--no-net".to_string(),
            "--host-docker-listener".to_string(),
            "23750:2375".to_string(),
            "--host-payload-listener".to_string(),
            "12076:1076".to_string(),
        ])
        .expect("config");

        let runtime_policy = policy_from_args(config.network.clone(), policy);
        assert_eq!(runtime_policy.egress.default_action, EgressAction::Deny);
        assert_eq!(runtime_policy.egress.reason, EgressReason::NoNetFlag);
        assert_eq!(
            runtime_policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
            ]
        );
    }

    #[test]
    fn frontend_no_net_rejects_published_ports_and_allow_rules() {
        let root = frontend_test_root();
        let base_args = [
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--no-net".to_string(),
        ];

        let mut publish_args = base_args.to_vec();
        publish_args.extend(["--publish".to_string(), "18080:8080".to_string()]);
        assert_eq!(
            frontend_config_from_args(&publish_args).expect_err("publish rejected"),
            "--no-net cannot be combined with --publish"
        );

        let mut allow_args = base_args.to_vec();
        allow_args.extend(["--allow-public-internet".to_string()]);
        assert_eq!(
            frontend_config_from_args(&allow_args).expect_err("allow rejected"),
            "--no-net cannot be combined with egress allow options"
        );
    }

    #[test]
    fn frontend_parses_payload_guest_share_options() {
        let root = frontend_test_root();
        let ro = root.join("readonly");
        let rw = root.join("writable");
        std::fs::create_dir_all(&ro).expect("ro");
        std::fs::create_dir_all(&rw).expect("rw");

        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--tool-state".to_string(),
            "codex".to_string(),
            "--payload-script".to_string(),
            "exec codex --model gpt-5".to_string(),
            "--gh".to_string(),
            "--aws".to_string(),
            "dev".to_string(),
            "--ro".to_string(),
            ro.display().to_string(),
            "--rw".to_string(),
            rw.display().to_string(),
        ])
        .expect("config");

        assert_eq!(config.project, root.join("repo"));
        assert!(policy.tool_state.codex);
        assert!(policy.gh);
        assert_eq!(policy.aws_profile.as_deref(), Some("dev"));
        assert_eq!(policy.extra_ro, vec![ro]);
        assert_eq!(policy.extra_rw, vec![rw]);
        let payload = launch_payload_args(&config, &policy)
            .expect("payload")
            .expect("payload script");
        assert_eq!(payload.script, "exec codex --model gpt-5");
    }

    #[test]
    fn frontend_rejects_removed_tool_flags() {
        let root = frontend_test_root();

        for flag in ["--tool", "--tool-arg"] {
            let error = frontend_config_from_args(&[
                "--project".to_string(),
                root.join("repo").display().to_string(),
                "--run-dir".to_string(),
                root.join(".sandbox/docker-vm/run").display().to_string(),
                "--artifact-manifest".to_string(),
                root.join("docker/out/artifact-manifest.json")
                    .display()
                    .to_string(),
                flag.to_string(),
                "codex".to_string(),
            ])
            .expect_err("removed flag rejected");
            assert!(
                error.contains(&format!("unexpected argument '{flag}'")),
                "{error}"
            );
        }
    }

    #[test]
    fn payload_env_sets_guest_ca_bundle_for_node_and_npm() {
        let root = frontend_test_root();
        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--tls-ca-cert".to_string(),
            root.join("repo/.sandbox/docker-vm/ca/mitm-ca.crt")
                .display()
                .to_string(),
        ])
        .expect("config");

        let env = guest_payload_env(&config, &policy).expect("env");

        assert_eq!(
            env.get("SSL_CERT_FILE").map(String::as_str),
            Some("/run/agentvm-ca-bundle.pem")
        );
        assert_eq!(
            env.get("REQUESTS_CA_BUNDLE").map(String::as_str),
            Some("/run/agentvm-ca-bundle.pem")
        );
        assert_eq!(
            env.get("NODE_EXTRA_CA_CERTS").map(String::as_str),
            Some("/run/agentvm-ca-bundle.pem")
        );
        assert_eq!(
            env.get("NPM_CONFIG_CAFILE").map(String::as_str),
            Some("/run/agentvm-ca-bundle.pem")
        );
    }

    #[test]
    fn payload_env_sets_home_xdg_docker_and_host_secret_defaults() {
        let root = frontend_test_root();
        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--tls-ca-cert".to_string(),
            root.join("repo/.sandbox/docker-vm/ca/mitm-ca.crt")
                .display()
                .to_string(),
        ])
        .expect("config");

        let env = guest_payload_env(&config, &policy).expect("env");
        let guest_home = host_home_dir().expect("host home").display().to_string();

        assert_eq!(env.get("HOME"), Some(&guest_home));
        assert!(!env.get("HOME").expect("HOME").contains(".sandbox/home"));
        assert_eq!(
            env.get("XDG_CACHE_HOME"),
            Some(&format!("{guest_home}/.cache"))
        );
        assert_eq!(
            env.get("XDG_CONFIG_HOME"),
            Some(&format!("{guest_home}/.config"))
        );
        assert_eq!(
            env.get("DOCKER_HOST").map(String::as_str),
            Some("tcp://127.0.0.1:1075")
        );
        let uid = unsafe { libc::geteuid() }.to_string();
        let gid = unsafe { libc::getegid() }.to_string();
        assert_eq!(
            env.get("AGENTVM_UID").map(String::as_str),
            Some(uid.as_str())
        );
        assert_eq!(
            env.get("AGENTVM_GID").map(String::as_str),
            Some(gid.as_str())
        );
        assert_eq!(env.get("SSH_AUTH_SOCK").map(String::as_str), Some(""));
        assert_eq!(
            env.get("GIT_CONFIG_GLOBAL").map(String::as_str),
            Some("/dev/null")
        );
        assert_eq!(
            env.get("AWS_SHARED_CREDENTIALS_FILE").map(String::as_str),
            Some("/dev/null")
        );
        assert!(env
            .get("PATH")
            .expect("PATH")
            .starts_with(&format!("{guest_home}/.local/share/mise/shims:")));
        assert!(!env.get("PATH").expect("PATH").contains(".sandbox/home"));
        assert_eq!(
            env.get("NODE_EXTRA_CA_CERTS").map(String::as_str),
            Some("/run/agentvm-ca-bundle.pem")
        );
    }

    #[test]
    fn wrapper_args_translate_to_launch_args() {
        let args = parse_wrapper_args(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                "/tmp/project".to_string(),
                "--no-net".to_string(),
                "--docker-publish".to_string(),
                "18080:8080".to_string(),
                "--".to_string(),
                "true".to_string(),
            ],
        )
        .expect("wrapper args");

        assert!(!args.reset);
        assert!(args.launch_args.contains(&"--no-net".to_string()));
        assert!(!args
            .launch_args
            .contains(&"--allow-public-internet".to_string()));
        assert!(!args.tls_bootstrap);
        assert!(args.launch_args.contains(&"--publish".to_string()));
        assert!(args.launch_args.contains(&"18080:8080".to_string()));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec true"));
    }

    #[test]
    fn setup_tool_defaults_to_public_egress_for_tool_install() {
        let root = frontend_test_root();
        let args = parse_wrapper_args(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
                "--setup-tool".to_string(),
                "codex".to_string(),
            ],
        )
        .expect("wrapper args");

        assert!(args
            .launch_args
            .contains(&"--allow-public-internet".to_string()));
        assert!(args.tls_bootstrap);
    }

    #[test]
    fn wrapper_selects_tui_for_interactive_terminals_by_default() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");
        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--project".to_string(), project.display().to_string()],
            true,
            true,
        )
        .expect("wrapper args");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
    }

    #[test]
    fn wrapper_no_tui_selects_plain_mode() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");
        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper args");

        assert_eq!(args.ui_mode, WrapperUiMode::Plain);
        assert!(!args.launch_args.contains(&"--no-tui".to_string()));
    }

    #[test]
    fn wrapper_non_tty_selects_plain_mode() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");
        let stdin_plain = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--project".to_string(), project.display().to_string()],
            false,
            true,
        )
        .expect("stdin");
        let stdout_plain = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--project".to_string(), project.display().to_string()],
            true,
            false,
        )
        .expect("stdout");

        assert_eq!(stdin_plain.ui_mode, WrapperUiMode::Plain);
        assert_eq!(stdout_plain.ui_mode, WrapperUiMode::Plain);
    }

    #[test]
    fn interactive_wrap_without_tool_defers_to_startup_dialog() {
        let root = frontend_test_root();
        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
        assert!(!args.tool_selected);
        assert!(!args.launch_args.contains(&"--tool".to_string()));
    }

    #[test]
    fn configured_codex_project_skips_startup_dialog() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--artifact-manifest".to_string(),
                root.join("docker/out/artifact-manifest.json")
                    .display()
                    .to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
        assert!(args.tool_selected);
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
        assert!(args.launch_args.windows(2).any(|window| {
            window[0] == "--payload-script"
                && window[1].contains("@openai/codex@latest")
                && window[1].contains("exec codex --dangerously-bypass-approvals-and-sandbox")
        }));
    }

    #[test]
    fn setup_tool_pi_config_runs_pi_payload_with_recipe_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--setup-tool".to_string(),
                "pi".to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.setup_tool, Some(SetupTool::Pi));
        assert!(args.tool_selected);
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "pi"));
        assert!(args.launch_args.windows(2).any(|window| {
            window[0] == "--payload-script"
                && window[1].contains("@mariozechner/pi-coding-agent@latest")
                && window[1].contains("exec pi")
        }));
    }

    #[test]
    fn setup_tool_pi_bootstrap_script_is_idempotent_and_quotes_args() {
        let script = setup_tool_payload_script(
            SetupTool::Pi,
            &["--model".to_string(), "claude 3.5".to_string()],
        );

        assert!(script.contains("command -v pi >/dev/null 2>&1"));
        assert!(script
            .contains("npm install --global --no-progress @mariozechner/pi-coding-agent@latest"));
        assert!(script.contains("agentvm: npm is required to install pi CLI"));
        assert!(script.contains("agentvm: installing pi CLI in guest HOME (first run only)"));
        assert!(script.contains("exec pi --model 'claude 3.5'"));
        assert!(script.contains("MISE_TRUSTED_CONFIG_PATHS=\"$PWD\""));
    }

    #[test]
    fn codex_setup_tool_bootstrap_script_uses_expected_package_flags_and_args() {
        let script = setup_tool_payload_script(
            SetupTool::Codex,
            &["--profile".to_string(), "work account".to_string()],
        );

        assert!(script.contains("command -v codex >/dev/null 2>&1"));
        assert!(script.contains("npm install --global --no-progress @openai/codex@latest"));
        assert!(script.contains("agentvm: npm is required to install codex CLI"));
        assert!(script.contains("agentvm: installing codex CLI in guest HOME (first run only)"));
        assert!(script.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(script.contains("--profile 'work account'"));
    }

    #[test]
    fn setup_tool_rejects_unsupported_recipe_with_actionable_diagnostic() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let error = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--setup-tool".to_string(),
                "emacs".to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect_err("unsupported setup tool");

        assert!(error.contains("unknown setup tool: emacs"), "{error}");
    }

    #[test]
    fn setup_tool_config_writes_are_idempotent() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let config = WrapperSandboxConfig::setup_tool(SetupTool::Codex);

        write_wrapper_sandbox_config(&project, &config).expect("first write");
        let first = std::fs::read_to_string(wrapper_sandbox_config_path(&project)).expect("first");
        write_wrapper_sandbox_config(&project, &config).expect("second write");
        let second =
            std::fs::read_to_string(wrapper_sandbox_config_path(&project)).expect("second");

        assert_eq!(first, second);
    }

    #[test]
    fn post_separator_overrides_configured_command_for_one_launch() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(
            args.command_override,
            Some(WrapperCommandOverride::Argv(ConfigCommand {
                command: "bash".to_string(),
                args: vec!["-l".to_string()],
            }))
        );
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec bash -l"));
    }

    #[test]
    fn config_schema_applies_network_auth_shares_and_ports() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let share = root.join("share");
        std::fs::create_dir_all(&share).expect("share");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig {
                schema_version: 2,
                setup_tool: None,
                default_command: ConfigCommand::new("bash"),
                tool_state: ConfigToolState {
                    codex: true,
                    pi: false,
                },
                network: ConfigNetwork {
                    mode: ConfigNetworkMode::Allowlist,
                    allowed_domains: vec!["example.com".to_string()],
                    allowed_hosts: vec!["api.example.com".to_string()],
                    allowed_ips: vec!["93.184.216.34".to_string()],
                },
                auth: ConfigAuth {
                    github: true,
                    aws_profile: Some("dev".to_string()),
                },
                shares: vec![ConfigShare {
                    host_path: share.display().to_string(),
                    guest_path: Some("/opt/share".to_string()),
                    access: ConfigShareAccess::Ro,
                    required: false,
                    shadows: Vec::new(),
                }],
                published_ports: vec![ConfigPort {
                    host: 18080,
                    guest: 8080,
                }],
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--allow-domain" && window[1] == "example.com"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--allow-domain" && window[1] == "api.example.com"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--allow-ip" && window[1] == "93.184.216.34"));
        assert!(args.launch_args.contains(&"--gh".to_string()));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--aws" && window[1] == "dev"));
        assert!(args.launch_args.windows(2).any(|window| {
            window[0] == "--share-ro" && window[1].ends_with("=/opt/share=optional")
        }));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--publish" && window[1] == "18080:8080"));
    }

    #[test]
    fn config_share_shadow_generates_nested_project_local_mount() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let share = root.join("host-codex");
        std::fs::create_dir_all(&share).expect("share");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig {
                schema_version: 2,
                setup_tool: None,
                default_command: ConfigCommand::new("bash"),
                tool_state: ConfigToolState::default(),
                network: ConfigNetwork::default(),
                auth: ConfigAuth::default(),
                shares: vec![ConfigShare {
                    host_path: share.display().to_string(),
                    guest_path: Some("/home/test/.codex".to_string()),
                    access: ConfigShareAccess::Rw,
                    required: true,
                    shadows: vec![ConfigShareShadow {
                        path: "tmp/arg0".to_string(),
                    }],
                }],
                published_ports: Vec::new(),
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--artifact-manifest".to_string(),
                root.join("docker/out/artifact-manifest.json")
                    .display()
                    .to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args.launch_args.windows(2).any(|window| {
            window[0] == "--share-rw" && window[1].ends_with("=/home/test/.codex=required")
        }));
        assert!(args.launch_args.windows(2).any(|window| {
            window[0] == "--share-shadow"
                && window[1].contains("/home/test/.codex=tmp/arg0=")
                && window[1].contains(".sandbox/share-shadows/share-0000/tmp/arg0")
        }));

        let (frontend, policy) = frontend_config_from_args(&args.launch_args).expect("frontend");
        let mounts = runtime_mounts(&frontend, &policy).expect("mounts");
        let shadow_backing = project.join(".sandbox/share-shadows/share-0000/tmp/arg0");
        assert!(shadow_backing.is_dir());
        let parent_index = mounts
            .iter()
            .position(|mount| mount.guest_path == PathBuf::from("/home/test/.codex"))
            .expect("parent mount");
        let shadow_index = mounts
            .iter()
            .position(|mount| mount.guest_path == PathBuf::from("/home/test/.codex/tmp/arg0"))
            .expect("shadow mount");
        assert!(parent_index < shadow_index);
        assert_eq!(mounts[shadow_index].host_path, shadow_backing);
        assert!(!mounts[shadow_index].readonly);
    }

    #[test]
    fn config_share_shadow_validation_rejects_readonly_and_escaping_paths() {
        let readonly_shadow = WrapperSandboxConfig {
            schema_version: 2,
            setup_tool: None,
            default_command: ConfigCommand::new("bash"),
            tool_state: ConfigToolState::default(),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: vec![ConfigShare {
                host_path: "/tmp/host".to_string(),
                guest_path: Some("/tmp/guest".to_string()),
                access: ConfigShareAccess::Ro,
                required: true,
                shadows: vec![ConfigShareShadow {
                    path: "tmp".to_string(),
                }],
            }],
            published_ports: Vec::new(),
        };
        assert_eq!(
            readonly_shadow.validate().expect_err("readonly shadow"),
            "sandbox config share shadows require rw access"
        );

        let escaping_shadow = WrapperSandboxConfig {
            shares: vec![ConfigShare {
                access: ConfigShareAccess::Rw,
                shadows: vec![ConfigShareShadow {
                    path: "../tmp".to_string(),
                }],
                ..readonly_shadow.shares[0].clone()
            }],
            ..readonly_shadow
        };
        assert_eq!(
            escaping_shadow.validate().expect_err("escaping shadow"),
            "share shadow path must stay under the parent share"
        );
    }

    #[test]
    fn shell_command_override_allows_unconfigured_plain_project() {
        let root = frontend_test_root();
        let project = root.join("repo");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string(),
            ],
            false,
            false,
        )
        .expect("shell override");

        assert_eq!(args.ui_mode, WrapperUiMode::Plain);
        assert!(!args.tool_selected);
        assert_eq!(
            args.command_override,
            Some(WrapperCommandOverride::Argv(ConfigCommand {
                command: "bash".to_string(),
                args: vec!["-l".to_string()],
            }))
        );
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec bash -l"));
    }

    #[test]
    fn legacy_config_from_disk_migrates_to_current_launch_defaults() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let path = wrapper_sandbox_config_path(&project);
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config dir");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"codex_enabled":true,"default_command":"codex"}"#,
        )
        .expect("legacy config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("legacy wrapper");

        assert!(args.tool_selected);
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
        assert!(args.launch_args.contains(&"--payload-script".to_string()));
    }

    #[test]
    fn setup_tool_codex_writes_expected_config_json() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--setup-tool".to_string(),
                "codex".to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");
        write_wrapper_sandbox_config(
            &args.project,
            &WrapperSandboxConfig::setup_tool(args.setup_tool.expect("setup tool")),
        )
        .expect("write config");

        let text = std::fs::read_to_string(wrapper_sandbox_config_path(&project)).expect("config");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["setup_tool"], "codex");
        assert_eq!(value["default_command"]["command"], "codex");
        assert_eq!(value["tool_state"]["codex"], true);
        assert_eq!(value["network"]["mode"], "public");
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
    }

    #[test]
    fn invalid_config_from_disk_reports_path_and_reason() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let path = wrapper_sandbox_config_path(&project);
        std::fs::create_dir_all(path.parent().expect("config parent")).expect("config dir");
        std::fs::write(
            &path,
            r#"{"schema_version":2,"default_command":{"command":""}}"#,
        )
        .expect("config");

        let error = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect_err("invalid config");

        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(
            error.contains("default command must not be empty"),
            "{error}"
        );
    }

    #[test]
    fn cli_network_overrides_take_precedence_over_config_no_net() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let mut config = WrapperSandboxConfig::codex_default();
        config.network.mode = ConfigNetworkMode::None;
        config.published_ports.push(ConfigPort {
            host: 18080,
            guest: 8080,
        });
        write_wrapper_sandbox_config(&project, &config).expect("sandbox config");

        let config_default = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("config default");
        assert!(config_default.launch_args.contains(&"--no-net".to_string()));
        assert!(!config_default
            .launch_args
            .contains(&"--publish".to_string()));

        let cli_override = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
                "--allow-domain".to_string(),
                "example.com".to_string(),
            ],
            true,
            true,
        )
        .expect("cli override");
        assert!(!cli_override.launch_args.contains(&"--no-net".to_string()));
        assert!(cli_override
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--allow-domain" && window[1] == "example.com"));
        assert!(cli_override
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--publish" && window[1] == "18080:8080"));
    }

    #[test]
    fn agentvm_argv0_uses_wrapper_without_wrap_subcommand() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args.tool_selected);
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
    }

    #[test]
    fn wrapper_command_override_runs_payload_script_and_keeps_configured_codex_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())
            .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-l".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args.tool_selected);
        assert_eq!(
            args.command_override,
            Some(WrapperCommandOverride::Argv(ConfigCommand {
                command: "bash".to_string(),
                args: vec!["-l".to_string()],
            }))
        );
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool-state" && window[1] == "codex"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec bash -l"));
    }

    #[test]
    fn configured_default_command_can_be_arbitrary_payload() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig {
                schema_version: 2,
                setup_tool: None,
                default_command: ConfigCommand {
                    command: "bash".to_string(),
                    args: vec!["-l".to_string()],
                },
                tool_state: ConfigToolState::default(),
                network: ConfigNetwork::default(),
                auth: ConfigAuth::default(),
                shares: Vec::new(),
                published_ports: Vec::new(),
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Plain);
        assert!(args.tool_selected);
        assert!(!args.launch_args.contains(&"--tool".to_string()));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec bash -l"));
    }

    #[test]
    fn plain_wrap_without_tool_still_requires_explicit_tool() {
        let root = frontend_test_root();
        let error = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            false,
            true,
        )
        .expect_err("missing tool");

        assert_eq!(
            error,
            "project is not configured; run agentvm --setup-tool codex|pi, use -- COMMAND, or start interactive TUI setup"
        );
    }

    #[test]
    fn wrapper_mitm_ca_is_generated_and_loadable() {
        let root = frontend_test_root();
        let paths = ensure_wrapper_mitm_ca(&root.join("repo")).expect("ca");

        assert!(paths.cert.is_file());
        assert!(paths.key.is_file());
        agentvm_frontend::tls_mitm::TlsMitmAuthority::from_files(&paths.cert, &paths.key)
            .expect("load ca");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&paths.key)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn payload_readiness_timeout_reports_last_error() {
        let mut attempts = 0;
        let error =
            wait_for_payload_ready_with_probe(Duration::from_millis(1), Duration::ZERO, || {
                attempts += 1;
                Err(format!("probe-{attempts}"))
            })
            .expect_err("timeout");

        assert!(attempts > 0);
        assert!(error.contains("timed out waiting for guest payload control path"));
        assert!(error.contains("last error: probe-"));
    }

    #[test]
    fn frontend_artifact_summary_names_key_run_artifacts() {
        let root = frontend_test_root();
        let (config, _) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
        ])
        .expect("config");

        let summary = frontend_artifact_summary(&config);

        assert!(summary.contains("artifacts: run_dir="));
        assert!(summary.contains("state="));
        assert!(summary.contains("qemu_log="));
        assert!(summary.contains("console_log="));
        assert!(summary.contains("vmnet_event_log="));
        assert!(summary.contains("state.json"));
        assert!(summary.contains("qemu.log"));
    }

    #[test]
    fn frontend_defaults_runtime_under_absolute_project() {
        let root = frontend_test_root();
        let (config, _) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
        ])
        .expect("config");

        assert_eq!(
            config.runtime.composed_bind_manifest,
            root.join("repo/.sandbox/docker-vm/run/guest-config/composed-binds.json")
        );
        assert!(config.runtime.composed_bind_manifest.is_absolute());
    }

    #[test]
    fn reset_project_removes_project_local_sandbox_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let codex_state = project.join(".sandbox/home/.codex/auth.json");
        let runtime_state = project.join(".sandbox/docker-vm/run/state.json");
        std::fs::create_dir_all(codex_state.parent().expect("codex parent")).expect("codex dir");
        std::fs::create_dir_all(runtime_state.parent().expect("runtime parent"))
            .expect("runtime dir");
        std::fs::write(&codex_state, "{}").expect("codex state");
        std::fs::write(&runtime_state, "{}").expect("runtime state");

        reset_project(&project).expect("reset");

        assert!(!project.join(".sandbox").exists());
    }

    #[test]
    fn wrapper_rejects_removed_bubblewrap_flags() {
        assert_eq!(
            parse_wrapper_args("agentvm-frontend", &["--docker".to_string()]).expect_err("docker"),
            "--docker has been removed; the VM is now the default execution model"
        );
        assert_eq!(
            parse_wrapper_args(
                "agentvm-frontend",
                &["--pass-env".to_string(), "TOKEN".to_string()]
            )
            .expect_err("pass env"),
            "--pass-env has been removed; use explicit VM guest shares/auth options instead"
        );
    }

    #[test]
    fn wrapper_rejects_removed_tool_and_command_flags() {
        for flag in ["--tool", "--tool-arg", "--command"] {
            let error =
                parse_wrapper_args("agentvm-frontend", &[flag.to_string(), "codex".to_string()])
                    .expect_err("removed wrapper flag");
            assert!(
                error.contains(&format!("unexpected argument '{flag}'")),
                "{error}"
            );
        }
    }

    #[test]
    fn argv0_no_longer_selects_wrapper_or_tool_behavior() {
        assert_eq!(
            run_cli(vec![
                "codex-wrap".to_string(),
                "--tool".to_string(),
                "codex".to_string(),
            ])
            .expect_err("argv0 wrapper removed"),
            "unknown command: --tool"
        );

        let root = frontend_test_root();
        let args = parse_wrapper_args_with_terminal(
            "codex-wrap",
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper args");
        assert!(!args.tool_selected);
        assert!(!args.launch_args.contains(&"--tool".to_string()));
    }

    #[test]
    fn parses_self_test_config_defaults_and_options() {
        let root = frontend_test_root();
        let config = self_test_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/self-test")
                .display()
                .to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--qemu".to_string(),
            "/usr/bin/qemu-system-x86_64".to_string(),
            "--image".to_string(),
            "alpine:3.22".to_string(),
            "--publish-payload-port".to_string(),
            "12079".to_string(),
            "--publish-container-port".to_string(),
            "18080:8080".to_string(),
            "--hostile".to_string(),
            "--payload-stress".to_string(),
            "--dns-check".to_string(),
            "--docker-net-check".to_string(),
            "--fs-check".to_string(),
        ])
        .expect("self-test config");

        assert_eq!(config.project, root.join("repo"));
        assert_eq!(config.run_dir, root.join(".sandbox/docker-vm/self-test"));
        assert_eq!(config.image, "alpine:3.22");
        assert_eq!(config.publish_payload_port, Some(12079));
        assert_eq!(
            config.publish_container_port,
            Some(PortPair {
                host: 18080,
                guest: 8080
            })
        );
        assert!(config.hostile);
        assert!(config.payload_stress);
        assert!(config.dns_check);
        assert!(config.docker_net_check);
        assert!(config.fs_check);
    }

    #[test]
    fn self_test_payload_covers_workspace_docker_and_bind_mount() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            false,
            None,
            false,
            false,
        );

        assert!(script.contains("self-test: payload-start"));
        assert!(script.contains("id -u"));
        assert!(script.contains("AGENTVM_UID"));
        assert!(script.contains("/run/agentvm-config/mitm-ca.crt"));
        assert!(script.contains("test ! -e /run/agentvm-config/mitm-ca.key"));
        assert!(script.contains("NODE_EXTRA_CA_CERTS"));
        assert!(script.contains("NPM_CONFIG_CAFILE"));
        assert!(script.contains("agentvm-config-ro"));
        assert!(script.contains("agentvm-self-test-state"));
        assert!(script.contains("dns.lookup"));
        assert!(script.contains(".agentvm-self-test-workspace"));
        assert!(script.contains("sqlite3.connect"));
        assert!(script.contains("PRAGMA journal_mode=WAL"));
        assert!(script.contains("AGENTVM_SQLITE_CONCURRENCY_DB"));
        assert!(script.contains("concurrent_writes"));
        assert!(script.contains("integrity_check"));
        assert!(script.contains("docker info"));
        assert!(script.contains("docker run --rm -v \"$PWD:/work:ro\" alpine:3.22"));
        assert!(script.contains(".agentvm-self-test-bind"));
        assert!(script.contains("self-test: payload-ok"));
    }

    #[test]
    fn self_test_payload_stress_covers_large_request_and_response() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            true,
            false,
            false,
            None,
            false,
            false,
        );

        assert!(script.contains("AGENTVM_PAYLOAD_STRESS_BLOB"));
        assert!(script.contains("AGENTVM_PAYLOAD_STRESS_BYTES"));
        assert!(script.contains("payload-stress-start"));
        assert!(script.contains("payload-stress-ok"));
    }

    #[test]
    fn self_test_payload_dns_check_reports_allowed_resolution() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            true,
            false,
            None,
            false,
            false,
        );

        assert!(script.contains("dns.lookup"));
        assert!(script.contains("dns-allow-ok example.com"));
        assert!(script.contains("dns-allow-failed example.com"));
    }

    #[test]
    fn self_test_payload_docker_net_check_reports_allow_and_deny_policy() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            true,
            None,
            false,
            false,
        );

        assert!(script.contains("docker-egress-ok image=alpine:3.22 policy=allow"));
        assert!(script.contains("docker-egress-failed image=alpine:3.22 policy=allow"));
        assert!(script.contains("docker-deny-ok image=alpine:3.22 policy=deny"));
        assert!(script.contains("docker-deny-unexpected image=alpine:3.22 policy=deny"));
        assert!(script.contains("phase=container-egress"));
    }

    #[test]
    fn self_test_payload_can_skip_sqlite_concurrency_for_specialized_network_checks() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            true,
            None,
            false,
            true,
        );

        assert!(script.contains("sqlite-home-smoke"));
        assert!(!script.contains("sqlite-concurrency-smoke"));
        assert!(script.contains("docker-deny-ok image=alpine:3.22 policy=deny"));
    }

    #[test]
    fn self_test_payload_fs_check_covers_live_composed_fs_contracts() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            false,
            None,
            true,
            false,
        );

        assert!(script.contains("self-test: fs-live-ok"));
        assert!(script.contains("delete-me.txt"));
        assert!(script.contains("os.rename"));
        assert!(script.contains("os.unlink"));
        assert!(script.contains("host-visible-ok"));
        assert!(script.contains("key-link"));
        assert!(script.contains("stat"));
    }

    #[test]
    fn self_test_payload_published_container_reports_ready_and_hit() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            false,
            Some(PortPair {
                host: 18080,
                guest: 8080,
            }),
            false,
            false,
        );

        assert!(script
            .contains("docker-publish-ready image=alpine:3.22 host_port=18080 guest_port=8080"));
        assert!(
            script.contains("docker-publish-ok image=alpine:3.22 host_port=18080 guest_port=8080")
        );
        assert!(script.contains("agentvm-container-publish-ok"));
        assert!(script.contains("-p 8080:8080"));
    }

    #[test]
    fn hostile_self_test_payload_covers_escape_and_denied_network_probes() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(
            &config,
            "alpine:3.22",
            true,
            false,
            false,
            false,
            None,
            false,
            false,
        );

        assert!(script.contains("self-test: hostile-start"));
        assert!(script.contains("mitm-ca.key"));
        assert!(script.contains(".agentvm-self-test-key-link"));
        assert!(script.contains("169.254.169.254"));
        assert!(script.contains("127.0.0.1"));
        assert!(script.contains("dns-deny-ok example.com"));
        assert!(script.contains("dns-deny-unexpected example.com"));
        assert!(script.contains("self-test: hostile-ok"));
        assert!(script.contains("self-test: payload-ok"));
    }

    #[test]
    #[ignore = "slow hostile VM smoke: run with `AGENTVM_HOSTILE_SELF_TEST_RUN=1 cargo test --manifest-path vm-frontend/Cargo.toml --offline hostile_guest_self_test_profile -- --ignored --nocapture`; requires rebuilt docker/out artifacts, QEMU, and guest payload readiness"]
    fn hostile_guest_self_test_profile() {
        if std::env::var("AGENTVM_HOSTILE_SELF_TEST_RUN").as_deref() != Ok("1") {
            eprintln!(
                "set AGENTVM_HOSTILE_SELF_TEST_RUN=1 to run `agentvm-frontend self-test --hostile --no-net` from this ignored test"
            );
            return;
        }
        run_self_test(&["--hostile".to_string(), "--no-net".to_string()])
            .expect("hostile self-test");
    }

    #[test]
    fn appliance_source_hashes_accept_current_files() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        write_frontend_manifest(&root);

        ensure_appliance_sources_fresh(&root.join("docker/out/artifact-manifest.json"))
            .expect("fresh sources");
    }

    #[test]
    fn appliance_source_hashes_reject_changed_guest_assets() {
        for source in [
            "docker/guest-init.sh",
            "docker/guest-payload-server.py",
            "docker/guest-socket-bridge.py",
        ] {
            let root = unique_temp_dir();
            std::fs::create_dir_all(root.join("docker/out")).expect("out");
            write_frontend_manifest(&root);
            std::fs::write(root.join(source), b"changed\n").expect("change source");

            let error =
                ensure_appliance_sources_fresh(&root.join("docker/out/artifact-manifest.json"))
                    .expect_err("stale artifact");

            assert!(error.contains("stale appliance artifacts"), "{error}");
            assert!(error.contains(source), "{error}");
            assert!(
                error.contains("rerun sudo ./docker/build-appliance.sh"),
                "{error}"
            );
        }
    }

    #[test]
    fn appliance_source_hashes_require_manifest_entries_before_live_boot() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::write(root.join("docker/out/artifact-manifest.json"), b"{}").expect("manifest");

        let error = ensure_appliance_sources_fresh(&root.join("docker/out/artifact-manifest.json"))
            .expect_err("missing source hashes");

        assert!(error.contains("does not record appliance source hashes"));
        assert!(error.contains("rerun sudo ./docker/build-appliance.sh"));
    }

    #[test]
    fn parses_ip_port() {
        assert_eq!(
            parse_ip_port("198.51.100.10:80").expect("ip port"),
            (Ipv4Addr::new(198, 51, 100, 10), 80)
        );
        assert!(parse_ip_port("198.51.100.10").is_err());
    }

    #[test]
    fn parses_port_pair() {
        assert_eq!(
            parse_port_pair("18080:8080").expect("port pair"),
            (18080, 8080)
        );
        assert!(parse_port_pair("18080").is_err());
        assert!(parse_port_pair("localhost:8080").is_err());
    }

    fn unique_temp_dir() -> TestTempDir {
        TestTempDir {
            dir: tempfile::Builder::new()
                .prefix("agentvm-frontend-main-test-")
                .tempdir()
                .expect("temp dir"),
        }
    }

    fn write_frontend_manifest(root: &Path) {
        let mut source_json = String::new();
        for (index, source) in REQUIRED_APPLIANCE_SOURCE_INPUTS.iter().enumerate() {
            let path = root.join(source);
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("source parent");
                }
                std::fs::write(&path, format!("test source {source}\n")).expect("source");
            }
            let hash = sha256_file_hex(&path).expect("source hash");
            if index > 0 {
                source_json.push_str(",\n");
            }
            source_json.push_str(&format!(
                "                {{ \"path\": \"{source}\", \"sha256\": \"{hash}\" }}"
            ));
        }
        std::fs::write(
            root.join("docker/out/artifact-manifest.json"),
            format!(
                r#"{{
              "schema_version": 1,
              "artifacts": {{
                "kernel": "docker/out/vmlinuz",
                "initrd": "docker/out/initrd.img",
                "rootfs": "docker/out/rootfs.raw"
              }},
              "source_inputs": [
{source_json}
              ],
              "vm": {{
                "cpus": 2,
                "memory_bytes": 2147483648,
                "virtiofs_tag": "agentvm",
                "kernel_cmdline": "console=hvc0 root=/dev/vda"
              }}
            }}"#
            ),
        )
        .expect("manifest");
    }

    fn frontend_test_root() -> TestTempDir {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::create_dir_all(root.join("repo")).expect("repo");
        std::fs::write(root.join("docker/out/vmlinuz"), b"kernel").expect("kernel");
        std::fs::write(root.join("docker/out/initrd.img"), b"initrd").expect("initrd");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        std::fs::write(
            root.join("docker/out/artifact-manifest.json"),
            r#"{
              "schema_version": 1,
              "artifacts": {
                "kernel": "docker/out/vmlinuz",
                "initrd": "docker/out/initrd.img",
                "rootfs": "docker/out/rootfs.raw"
              },
              "vm": {
                "cpus": 2,
                "memory_bytes": 2147483648,
                "virtiofs_tag": "agentvm",
                "kernel_cmdline": "console=hvc0 root=/dev/vda"
              }
            }"#,
        )
        .expect("manifest");
        root
    }
}
