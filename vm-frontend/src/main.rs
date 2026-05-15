use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
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
    guest_runtime_mounts, GuestShareSpec, GuestTool, RuntimeMount, ToolStateMounts,
};
use agentvm_frontend::tcp_gateway::UpstreamMapping;
use agentvm_frontend::vmnet_runtime::{serve_vmnet_gateway, VmnetRuntimeConfig};
use agentvm_frontend::{FrontendConfig, GuestNetwork, RuntimePaths};
use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand, ValueHint};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use serde::{Deserialize, Serialize};

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
        let running = start_frontend_with_policy(config.clone(), mounts, policy)
            .map_err(|error| format!("launch failed: {error}"))?;
        let addr = socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
        wait_for_payload_ready(addr, Duration::from_secs(120))?;
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
    let qemu_exit = run_frontend_until_qemu_exit_with_policy_and_timeout(
        config.clone(),
        mounts,
        policy,
        qemu_timeout,
    )
    .map_err(|error| format!("launch failed: {error}"))?;
    if qemu_exit.status.success() {
        Ok(())
    } else if qemu_exit.timed_out {
        Err(format!(
            "qemu timed out after {} seconds and was terminated with status: {}",
            qemu_timeout.map_or(0, |timeout| timeout.as_secs()),
            qemu_exit.status
        ))
    } else {
        Err(format!("qemu exited with status: {}", qemu_exit.status))
    }
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
            write_wrapper_sandbox_config(&project, &WrapperSandboxConfig::codex_default())?;
            launch_args.extend(["--tool".to_string(), "codex".to_string()]);
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
    Shell { command: String, args: Vec<String> },
}

impl WrapperCommandOverride {
    fn append_args(&mut self, args: &[String]) {
        match self {
            Self::Argv(command) => command.args.extend(args.iter().cloned()),
            Self::Shell {
                args: shell_args, ..
            } => shell_args.extend(args.iter().cloned()),
        }
    }

    fn script(&self) -> String {
        match self {
            Self::Argv(command) => payload_script_from_config_command(command),
            Self::Shell { command, args } => payload_script_from_shell_command(command, args),
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
}

fn default_required_share() -> bool {
    true
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
    let tool = matches.get_one::<String>("tool").cloned();
    if let Some(selected) = tool.as_ref() {
        selected.parse::<GuestTool>()?;
        launch_args.extend(["--tool".to_string(), selected.clone()]);
    }
    if let Some(args) = matches.get_many::<String>("tool_arg") {
        for arg in args {
            launch_args.extend(["--tool-arg".to_string(), arg.clone()]);
        }
    }
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

    if let Some(command) = matches.get_one::<String>("command") {
        if command.trim().is_empty() {
            return Err("--command must not be empty".to_string());
        }
        command_override = Some(WrapperCommandOverride::Shell {
            command: command.clone(),
            args: Vec::new(),
        });
    }
    if let Some((command, command_args)) = payload_command_values.split_first() {
        if let Some(existing) = command_override.as_mut() {
            let mut all_args = Vec::with_capacity(1 + command_args.len());
            all_args.push(command.clone());
            all_args.extend(command_args.iter().cloned());
            existing.append_args(&all_args);
        } else {
            command_override = Some(WrapperCommandOverride::Argv(ConfigCommand {
                command: command.clone(),
                args: command_args.to_vec(),
            }));
        }
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
        apply_configured_launch_defaults(&mut launch_args, config, saw_network_override, no_net)?;
    }
    let mut tool_selected = launch_args.iter().any(|arg| arg == "--tool");
    if !tool_selected {
        if let Some(tool) = tool.clone() {
            launch_args.extend(["--tool".to_string(), tool]);
            tool_selected = true;
        } else if sandbox_config.as_ref().is_some_and(config_uses_codex_tool) {
            launch_args.extend(["--tool".to_string(), "codex".to_string()]);
            tool_selected = true;
        } else if ui_mode == WrapperUiMode::Plain
            && command_override.is_none()
            && sandbox_config.is_none()
        {
            return Err(
                "project is not configured; run agentvm --setup-tool codex|pi, use -- COMMAND, or start interactive TUI setup".to_string(),
            );
        }
    }
    if let Some(command) = command_override.as_ref() {
        apply_wrapper_command_override(&mut launch_args, command);
    } else if tool.is_none() {
        if let Some(config) = sandbox_config.as_ref() {
            apply_configured_default_command(&mut launch_args, config);
            tool_selected = true;
        }
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
        .arg(
            Arg::new("tool")
                .long("tool")
                .value_name("codex|copilot")
                .help("Compatibility: select a launch-time tool without persisting config"),
        )
        .arg(
            Arg::new("tool_arg")
                .long("tool-arg")
                .value_name("ARG")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .help("Compatibility: pass one argument to --tool"),
        )
        .arg(
            Arg::new("command")
                .long("command")
                .value_name("CMD")
                .help("Compatibility: one-run shell command override"),
        )
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

fn config_uses_codex_tool(config: &WrapperSandboxConfig) -> bool {
    config.setup_tool == Some(SetupTool::Codex)
        && config.default_command.command == "codex"
        && config.default_command.args.is_empty()
}

fn apply_configured_launch_defaults(
    launch_args: &mut Vec<String>,
    config: &WrapperSandboxConfig,
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
    for share in &config.shares {
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

fn apply_configured_default_command(launch_args: &mut Vec<String>, config: &WrapperSandboxConfig) {
    if config_uses_codex_tool(config) {
        return;
    }
    if config.setup_tool == Some(SetupTool::Pi)
        && config.default_command.command == SetupTool::Pi.cli()
        && !launch_args.iter().any(|arg| arg == "--payload-script")
    {
        apply_payload_script(
            launch_args,
            setup_tool_payload_script(SetupTool::Pi, &config.default_command.args),
        );
        return;
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
    let mut tool_args = Vec::new();
    let mut filtered = Vec::with_capacity(launch_args.len());
    let mut index = 0;
    while index < launch_args.len() {
        if launch_args[index] == "--tool-arg" && index + 1 < launch_args.len() {
            tool_args.push(launch_args[index + 1].clone());
            index += 2;
        } else {
            filtered.push(launch_args[index].clone());
            index += 1;
        }
    }
    *launch_args = filtered;

    let script = if tool_args.is_empty() {
        script
    } else {
        format!(
            "{} {}",
            script.trim_end(),
            tool_args
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
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

fn payload_script_from_shell_command(command: &str, args: &[String]) -> String {
    let mut script = format!("exec {}", command.trim());
    for arg in args {
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
    tool: Option<GuestTool>,
    tool_state: ToolStateMounts,
    tool_args: Vec<String>,
    gh: bool,
    aws_profile: Option<String>,
    extra_ro: Vec<PathBuf>,
    extra_rw: Vec<PathBuf>,
    extra_shares: Vec<GuestPathShare>,
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
struct SelfTestConfig {
    project: PathBuf,
    run_dir: PathBuf,
    artifact_manifest: PathBuf,
    qemu: PathBuf,
    image: String,
    publish_payload_port: Option<u16>,
    no_net: bool,
    hostile: bool,
    tool: GuestTool,
}

fn run_self_test(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_self_test_usage();
        return Ok(());
    }
    let self_test = self_test_config_from_args(args)?;
    let config = FrontendConfig::from_artifact_manifest_file(
        self_test.project.clone(),
        self_test.run_dir,
        self_test.qemu,
        &self_test.artifact_manifest,
    )
    .map_err(|error| format!("failed to load frontend config: {error}"))?;
    let mut policy_args = PolicyArgs {
        allow_public: !self_test.no_net,
        no_net: self_test.no_net,
        tool: Some(self_test.tool),
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
    let sqlite_concurrency_host_db = config
        .project
        .join(".agentvm-self-test-sqlite/state.sqlite");
    if let Some(parent) = sqlite_concurrency_host_db.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create sqlite concurrency dir: {error}"))?;
    }
    guest_env.insert(
        "AGENTVM_SQLITE_CONCURRENCY_DB".to_string(),
        sqlite_concurrency_host_db.display().to_string(),
    );
    let mut policy = policy_from_args(config.network.clone(), policy_args);
    let _lock = ProjectLock::acquire(&config)?;
    let host_port = ensure_payload_listener(&mut policy);
    let running = start_frontend_with_policy(config.clone(), mounts, policy)
        .map_err(|error| format!("self-test launch failed: {error}"))?;
    let payload_addr = socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
    wait_for_payload_ready(payload_addr, Duration::from_secs(120))?;

    if let Some(host_port) = self_test.publish_payload_port {
        let publish_addr =
            socket_addr("127.0.0.1", host_port).map_err(|error| error.to_string())?;
        ping_payload(publish_addr)
            .map_err(|error| format!("published payload-port check failed: {error}"))?;
        println!("self-test: published payload port {host_port} ok");
    }

    let (rows, cols) = terminal_size();
    let request = PayloadRequest {
        script: self_test_payload_script(&config, &self_test.image, self_test.hostile),
        cwd: config.project.display().to_string(),
        env: guest_env,
        rows,
        cols,
    };
    let mut host_sqlite = spawn_host_sqlite_concurrency(&sqlite_concurrency_host_db)?;
    let exit_code = run_payload_tcp_with_control(
        payload_addr,
        &request,
        None,
        &mut io::stdout(),
        PayloadControlOptions::disabled(),
    )
    .map_err(|error| format!("self-test payload failed: {error}"));
    let host_sqlite_result = wait_host_sqlite_concurrency(&mut host_sqlite)
        .and_then(|_| run_host_sqlite_integrity_check(&sqlite_concurrency_host_db));
    running
        .terminate()
        .map_err(|error| format!("self-test shutdown failed: {error}"))?;
    let exit_code = exit_code?;
    if exit_code != 0 {
        return Err(format!("self-test payload exited with {exit_code}"));
    }
    host_sqlite_result?;
    println!("self-test: ok");
    Ok(())
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
        no_net: matches.get_flag("no_net"),
        hostile: matches.get_flag("hostile"),
        tool: matches
            .get_one::<String>("tool")
            .map(|value| value.parse().map_err(|error: String| error))
            .transpose()?
            .unwrap_or(GuestTool::Codex),
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
        .arg(Arg::new("no_net").long("no-net").action(ArgAction::SetTrue))
        .arg(
            Arg::new("hostile")
                .long("hostile")
                .action(ArgAction::SetTrue),
        )
        .arg(Arg::new("tool").long("tool").value_name("codex|copilot"))
}

fn self_test_payload_script(_config: &FrontendConfig, image: &str, hostile: bool) -> String {
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
        "echo self-test: sqlite-concurrency-smoke".to_string(),
        format!("python3 -c {}", shell_quote(sqlite_concurrency)),
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
        "if [ \"${AGENTVM_SELF_TEST_NETWORK:-allow}\" = deny ]; then node -e 'const dns=require(\"dns\"); dns.lookup(\"example.com\", err => { if (err) process.exit(0); console.error(\"dns-deny-unexpected\"); process.exit(1); });'; fi".to_string(),
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
    if let Some(tool) = matches.get_one::<String>("tool") {
        policy.tool = Some(tool.parse().map_err(|error: String| error)?);
    }
    for tool_state in append_many(&matches, "tool_state") {
        match tool_state.as_str() {
            "codex" => policy.tool_state.codex = true,
            "pi" => policy.tool_state.pi = true,
            value => return Err(format!("unknown --tool-state: {value}")),
        }
    }
    policy.tool_args = append_many(&matches, "tool_arg");
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
        ensure_smoke_hook_artifact_fresh(&artifact_manifest)?;
    }
    if policy
        .payload
        .as_ref()
        .is_some_and(|payload| payload.script.is_empty())
    {
        return Err("--payload-script must not be empty".to_string());
    }
    if !policy.tool_args.is_empty() && policy.tool.is_none() {
        return Err("--tool-arg requires --tool".to_string());
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
        .arg(Arg::new("tool").long("tool").value_name("codex|copilot"))
        .arg(
            Arg::new("tool_state")
                .long("tool-state")
                .value_name("codex|pi")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("tool_arg")
                .long("tool-arg")
                .value_name("ARG")
                .action(ArgAction::Append)
                .allow_hyphen_values(true),
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
    config: &FrontendConfig,
    policy: &PolicyArgs,
) -> Result<Option<PayloadLaunchArgs>, String> {
    if let Some(payload) = policy.payload.clone() {
        return Ok(Some(payload));
    }
    let Some(tool) = policy.tool else {
        return Ok(None);
    };
    let (rows, cols) = terminal_size();
    Ok(Some(PayloadLaunchArgs {
        script: tool_payload_script(tool, &policy.tool_args),
        cwd: config.project.display().to_string(),
        env: BTreeMap::new(),
        rows,
        cols,
        no_stdin: false,
    }))
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
            tool: policy.tool,
            tool_state: ToolStateMounts::from_guest_tool(policy.tool).union(policy.tool_state),
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
    Ok(mounts)
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
    if policy.tool.is_none()
        && policy.payload.is_none()
        && !policy.gh
        && policy.aws_profile.is_none()
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

fn tool_payload_script(tool: GuestTool, tool_args: &[String]) -> String {
    let mut command = Vec::new();
    command.push(shell_quote(tool.cli()));
    command.extend(tool.auto_flags().iter().map(|flag| shell_quote(flag)));
    command.extend(tool_args.iter().map(|arg| shell_quote(arg)));
    let npm_package = format!("{}@latest", tool.npm_package());
    let install_message = format!(
        "agentvm: installing {} CLI in guest HOME (first run only)...",
        tool.cli()
    );
    [
        r#"export NPM_CONFIG_PREFIX="$HOME/.local""#.to_string(),
        r#"mkdir -p "$NPM_CONFIG_PREFIX""#.to_string(),
        format!(
            "if ! command -v {} >/dev/null 2>&1; then printf '%s\\n' {} >&2; npm install --global --no-progress {}; fi",
            shell_quote(tool.cli()),
            shell_quote(&install_message),
            shell_quote(&npm_package)
        ),
        "hash -r 2>/dev/null || true".to_string(),
        r#"export MISE_TRUSTED_CONFIG_PATHS="$PWD""#.to_string(),
        format!("exec {}", command.join(" ")),
    ]
    .join(" && ")
}

fn setup_tool_payload_script(tool: SetupTool, tool_args: &[String]) -> String {
    tool_bootstrap_payload_script(tool.cli(), tool.package(), &[], tool_args)
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
            "if ! command -v {} >/dev/null 2>&1; then printf '%s\\n' {} >&2; npm install --global --no-progress {}; fi",
            shell_quote(cli),
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

fn ensure_smoke_hook_artifact_fresh(artifact_manifest: &std::path::Path) -> Result<(), String> {
    let Some(repo_root) = artifact_manifest
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
    else {
        return Ok(());
    };
    let guest_init = repo_root.join("docker/guest-init.sh");
    let rootfs = repo_root.join("docker/out/rootfs.raw");
    if !guest_init.exists() || !rootfs.exists() {
        return Ok(());
    }
    let guest_init_modified = guest_init
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("failed to stat {}: {error}", guest_init.display()))?;
    let rootfs_modified = rootfs
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("failed to stat {}: {error}", rootfs.display()))?;
    if guest_init_modified >= rootfs_modified {
        return Err(format!(
            "--guest-http-smoke-url requires rebuilt appliance artifacts; {} is newer than {}",
            guest_init.display(),
            rootfs.display()
        ));
    }
    Ok(())
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
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < timeout {
        match ping_payload(addr) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error.to_string());
                thread::sleep(Duration::from_millis(250));
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
         prepare/launch options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--tool codex|copilot] [--tool-arg ARG] [--gh] [--aws PROFILE] [--ro PATH] [--rw PATH] [--guest-http-smoke-url URL] [--allow-public-internet|--no-net] [--qemu-timeout-seconds N] [--local-http-smoke-upstream IP:PORT] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--payload-script SCRIPT] [--payload-cwd PATH] [--payload-env KEY=VALUE] [--payload-no-stdin] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         self-test options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--image IMAGE] [--publish-payload-port PORT] [--tool codex|copilot] [--no-net] [--hostile]\n\
         vmnet-gateway options: --socket PATH [--allow-ip IP_OR_CIDR] [--allow-domain DOMAIN] [--allow-public-internet|--no-net] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         payload-client options: --port PORT [--host HOST] [--ping|--script SCRIPT] [--cwd PATH] [--env KEY=VALUE] [--rows N] [--cols N] [--no-stdin]"
    );
}

fn print_self_test_usage() {
    eprintln!(
        "usage: agentvm-frontend self-test [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--image IMAGE] [--publish-payload-port PORT] [--tool codex|copilot] [--no-net] [--hostile]"
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
    fn frontend_parses_tool_guest_share_options() {
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
            "--tool".to_string(),
            "codex".to_string(),
            "--tool-arg".to_string(),
            "--model".to_string(),
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
        assert_eq!(policy.tool, Some(GuestTool::Codex));
        assert_eq!(policy.tool_args, vec!["--model"]);
        assert!(policy.gh);
        assert_eq!(policy.aws_profile.as_deref(), Some("dev"));
        assert_eq!(policy.extra_ro, vec![ro]);
        assert_eq!(policy.extra_rw, vec![rw]);
        let payload = launch_payload_args(&config, &policy)
            .expect("payload")
            .expect("tool payload");
        assert!(payload
            .script
            .contains("agentvm: installing codex CLI in guest HOME"));
        assert!(payload
            .script
            .contains("npm install --global --no-progress @openai/codex@latest"));
        assert!(payload
            .script
            .contains("codex --dangerously-bypass-approvals-and-sandbox --model"));
    }

    #[test]
    fn tool_arg_requires_tool() {
        let root = frontend_test_root();

        let error = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--tool-arg".to_string(),
            "--model".to_string(),
        ])
        .expect_err("tool arg rejected");

        assert_eq!(error, "--tool-arg requires --tool");
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
            "--tool".to_string(),
            "codex".to_string(),
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
            "--tool".to_string(),
            "copilot".to_string(),
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
                "--tool".to_string(),
                "codex".to_string(),
                "--project".to_string(),
                "/tmp/project".to_string(),
                "--no-net".to_string(),
                "--docker-publish".to_string(),
                "18080:8080".to_string(),
                "--tool-arg".to_string(),
                "--model".to_string(),
                "--tool-arg".to_string(),
                "gpt-5".to_string(),
            ],
        )
        .expect("wrapper args");

        assert!(!args.reset);
        assert!(args.launch_args.contains(&"--tool".to_string()));
        assert!(args.launch_args.contains(&"codex".to_string()));
        assert!(args.launch_args.contains(&"--no-net".to_string()));
        assert!(!args
            .launch_args
            .contains(&"--allow-public-internet".to_string()));
        assert!(!args.tls_bootstrap);
        assert!(args.launch_args.contains(&"--publish".to_string()));
        assert!(args.launch_args.contains(&"18080:8080".to_string()));
        assert_eq!(
            args.launch_args
                .windows(2)
                .filter(|window| window[0] == "--tool-arg")
                .map(|window| window[1].clone())
                .collect::<Vec<_>>(),
            vec!["--model".to_string(), "gpt-5".to_string()]
        );
    }

    #[test]
    fn wrapper_defaults_to_public_egress_for_tool_install() {
        let args = parse_wrapper_args(
            "agentvm-frontend",
            &["--tool".to_string(), "codex".to_string()],
        )
        .expect("wrapper args");

        assert!(args
            .launch_args
            .contains(&"--allow-public-internet".to_string()));
        assert!(args.tls_bootstrap);
    }

    #[test]
    fn wrapper_selects_tui_for_interactive_terminals_by_default() {
        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--tool".to_string(), "codex".to_string()],
            true,
            true,
        )
        .expect("wrapper args");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
    }

    #[test]
    fn wrapper_no_tui_selects_plain_mode() {
        let args = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &[
                "--tool".to_string(),
                "codex".to_string(),
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
        let stdin_plain = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--tool".to_string(), "codex".to_string()],
            false,
            true,
        )
        .expect("stdin");
        let stdout_plain = parse_wrapper_args_with_terminal(
            "agentvm-frontend",
            &["--tool".to_string(), "codex".to_string()],
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
            .any(|window| window[0] == "--tool" && window[1] == "codex"));
        assert!(!args.launch_args.contains(&"--payload-script".to_string()));
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
            .any(|window| window[0] == "--tool" && window[1] == "codex"));
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
            .any(|window| window[0] == "--tool" && window[1] == "codex"));
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
                "--command".to_string(),
                "bash".to_string(),
                "--".to_string(),
                "-l".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args.tool_selected);
        assert_eq!(
            args.command_override,
            Some(WrapperCommandOverride::Shell {
                command: "bash".to_string(),
                args: vec!["-l".to_string()],
            })
        );
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool" && window[1] == "codex"));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--payload-script" && window[1] == "exec bash -l"));
        assert!(!args.launch_args.contains(&"--tool-arg".to_string()));
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
    fn explicit_copilot_tool_defaults_to_vm_tool_install_and_public_egress() {
        let root = frontend_test_root();
        let args = parse_wrapper_args(
            "agentvm-frontend",
            &[
                "--tool".to_string(),
                "copilot".to_string(),
                "--project".to_string(),
                root.join("repo").display().to_string(),
                "--artifact-manifest".to_string(),
                root.join("docker/out/artifact-manifest.json")
                    .display()
                    .to_string(),
                "--tool-arg".to_string(),
                "suggest".to_string(),
            ],
        )
        .expect("wrapper args");

        assert!(args.tls_bootstrap);
        assert!(args
            .launch_args
            .contains(&"--allow-public-internet".to_string()));
        assert!(args
            .launch_args
            .windows(2)
            .any(|window| window[0] == "--tool" && window[1] == "copilot"));
        let (config, policy) = frontend_config_from_args(&args.launch_args).expect("config");
        let payload = launch_payload_args(&config, &policy)
            .expect("payload")
            .expect("tool payload");
        assert!(payload
            .script
            .contains("agentvm: installing github-copilot-cli CLI in guest HOME"));
        assert!(payload
            .script
            .contains("npm install --global --no-progress @github/copilot@latest"));
        assert!(payload
            .script
            .contains("github-copilot-cli --allow-all --no-auto-update suggest"));
        assert_eq!(payload.cwd, root.join("repo").display().to_string());
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
    fn frontend_defaults_runtime_under_absolute_project() {
        let root = frontend_test_root();
        let (config, _) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--tool".to_string(),
            "codex".to_string(),
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
            "--hostile".to_string(),
            "--tool".to_string(),
            "copilot".to_string(),
        ])
        .expect("self-test config");

        assert_eq!(config.project, root.join("repo"));
        assert_eq!(config.run_dir, root.join(".sandbox/docker-vm/self-test"));
        assert_eq!(config.image, "alpine:3.22");
        assert_eq!(config.publish_payload_port, Some(12079));
        assert!(config.hostile);
        assert_eq!(config.tool, GuestTool::Copilot);
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

        let script = self_test_payload_script(&config, "alpine:3.22", false);

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
    fn hostile_self_test_payload_covers_escape_and_denied_network_probes() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let script = self_test_payload_script(&config, "alpine:3.22", true);

        assert!(script.contains("self-test: hostile-start"));
        assert!(script.contains("mitm-ca.key"));
        assert!(script.contains(".agentvm-self-test-key-link"));
        assert!(script.contains("169.254.169.254"));
        assert!(script.contains("127.0.0.1"));
        assert!(script.contains("dns-deny-unexpected"));
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
    fn guest_http_smoke_requires_fresh_artifact() {
        let root = unique_temp_dir();
        std::fs::create_dir_all(root.join("docker/out")).expect("out");
        std::fs::write(root.join("docker/out/artifact-manifest.json"), b"{}").expect("manifest");
        std::fs::write(root.join("docker/out/rootfs.raw"), b"rootfs").expect("rootfs");
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(root.join("docker/guest-init.sh"), b"#!/bin/sh\n").expect("guest init");

        let error =
            ensure_smoke_hook_artifact_fresh(&root.join("docker/out/artifact-manifest.json"))
                .expect_err("stale artifact");

        assert!(error.contains("requires rebuilt appliance artifacts"));
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
