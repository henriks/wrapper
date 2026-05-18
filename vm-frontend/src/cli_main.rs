use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
#[cfg(feature = "validation-self-test")]
use std::net::TcpStream;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Component, Path, PathBuf};
#[cfg(feature = "validation-self-test")]
use std::process::Child;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use agentvm_frontend::launch::{
    prepare_frontend_launch_with_policy,
    run_frontend_until_qemu_exit_with_policy_and_timeout_async,
    run_frontend_until_qemu_exit_with_policy_and_timeout_reserving_host_ports_async, LaunchError,
    QemuExit,
};
use agentvm_frontend::network_policy::{
    EgressAction, EgressReason, HostListener, HostListenerPurpose, Ipv4RangeParseError, VmnetPolicy,
};
use agentvm_frontend::payload_client::{
    ping_payload_async_tcp, run_diagnostic_tcp_async, run_payload_tcp_async_with_control,
    socket_addr, terminal_size, DiagnosticRequest, PayloadClientError, PayloadControlOptions,
    PayloadRequest,
};
use agentvm_frontend::runtime_manifest::{guest_runtime_mounts, GuestShareSpec, RuntimeMount};
use agentvm_frontend::supervisor_control::{
    control_socket_path, SupervisorControlClient, SupervisorControlIoError,
};
use agentvm_frontend::tcp_gateway::UpstreamMapping;
use agentvm_frontend::vmnet_runtime::{serve_vmnet_gateway_async, VmnetRuntimeError};
use agentvm_frontend::{FrontendConfig, GuestNetwork, RuntimePaths};
use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand, ValueHint};
use tracing::{debug, info};
mod appliance;
mod config;
mod launch_cli;
mod payload_cli;
#[cfg(feature = "validation-self-test")]
mod self_test;
#[cfg(feature = "validation-self-test")]
mod self_test_payload;
mod tls_bootstrap;
mod tui;
mod vmnet_cli;
mod wrapper;

use appliance::ensure_appliance_sources_fresh;
#[cfg(test)]
use appliance::{sha256_file_hex, ApplianceError, REQUIRED_APPLIANCE_SOURCE_INPUTS};
use config::{
    project_mise_config_path, read_wrapper_sandbox_config, validate_share_shadow_path,
    write_setup_tool_mise_config, write_wrapper_sandbox_config, ConfigCommand, ConfigNetworkMode,
    ConfigPort, ConfigShare, ConfigShareAccess, SetupTool, WrapperSandboxConfig,
};
#[cfg(test)]
use config::{wrapper_sandbox_config_path, ConfigAuth, ConfigError, ConfigNetwork};
#[cfg(any(test, feature = "validation-self-test"))]
use launch_cli::ensure_payload_listener;
#[cfg(any(test, feature = "validation-self-test"))]
use launch_cli::{frontend_artifact_summary, guest_payload_env};
use launch_cli::{
    frontend_config_from_args, policy_from_args, reset_project, run_launch_async,
    run_launch_request_async, runtime_mounts, validate_no_net_args, FrontendLaunchRequest,
    GuestPathShare, GuestPathShareShadow, LaunchCliError, PayloadLaunchArgs, PolicyArgs,
};
#[cfg(test)]
use launch_cli::{
    frontend_config_from_launch_request, launch_payload_args, wait_for_payload_ready_with_probe,
};
#[cfg(feature = "validation-self-test")]
use launch_cli::{wait_for_payload_ready_async, ProjectLock};
#[cfg(test)]
use payload_cli::guest_sync_diagnostic_request;
use payload_cli::{
    flush_guest_filesystems, payload_client_config_from_args, payload_exit_status, PayloadCliError,
};
#[cfg(feature = "validation-self-test")]
use self_test::run_self_test;
use tls_bootstrap::ensure_wrapper_mitm_ca;
use vmnet_cli::{vmnet_gateway_config_from_args, VmnetCliError};
#[cfg(test)]
use wrapper::{
    parse_wrapper_args, parse_wrapper_args_with_terminal, payload_script_from_config_command,
};
use wrapper::{run_wrapper_async, WrapperError, WrapperUiMode};

pub(crate) fn main() {
    init_tracing_from_env();
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("agentvm")
        .build()
        .map_err(|source| CliError::TokioRuntime { source })
        .and_then(|runtime| runtime.block_on(run_cli_async(env::args().collect())));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

type CliResult<T> = Result<T, CliError>;

async fn run_cli_async(argv: Vec<String>) -> CliResult<()> {
    debug!(argc = argv.len(), "dispatching async CLI argv");
    let mut iter = argv.into_iter();
    let program = iter.next().unwrap_or_else(|| "agentvm".to_string());
    let args: Vec<String> = iter.collect();
    #[cfg(feature = "validation-self-test")]
    if is_agentvm_self_test_program(&program) {
        return run_self_test(&args).await.map_err(CliError::SelfTest);
    }
    if is_agentvm_program(&program) {
        return match args.first().map(String::as_str) {
            Some("prepare") => run(args),
            Some("self-test") => Err(CliError::UnknownCommand {
                command: "self-test".to_string(),
            }),
            Some("launch" | "control" | "vmnet-gateway" | "payload-client") => {
                run_async(args).await
            }
            _ => run_wrapper_async(args).await.map_err(Into::into),
        };
    }
    match args.first().map(String::as_str) {
        Some("launch" | "control" | "vmnet-gateway" | "payload-client") => run_async(args).await,
        Some("prepare") | Some("-h" | "--help") | None => run(args),
        Some(command) => Err(CliError::UnknownCommand {
            command: command.to_string(),
        }),
    }
}

fn init_tracing_from_env() {
    let Ok(level) = env::var("AGENTVM_LOG") else {
        return;
    };
    let Some(max_level) = parse_trace_level(&level) else {
        return;
    };
    let _ = tracing::subscriber::set_global_default(AgentvmTraceSubscriber { max_level });
}

fn parse_trace_level(level: &str) -> Option<tracing::Level> {
    match level.trim().to_ascii_lowercase().as_str() {
        "error" => Some(tracing::Level::ERROR),
        "warn" | "warning" => Some(tracing::Level::WARN),
        "info" => Some(tracing::Level::INFO),
        "debug" => Some(tracing::Level::DEBUG),
        "trace" => Some(tracing::Level::TRACE),
        _ => None,
    }
}

struct AgentvmTraceSubscriber {
    max_level: tracing::Level,
}

impl tracing::Subscriber for AgentvmTraceSubscriber {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= self.max_level
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut visitor = TraceFieldVisitor::default();
        event.record(&mut visitor);
        let fields = visitor.fields.join(" ");
        eprintln!(
            "agentvm {} {} {}",
            event.metadata().level(),
            event.metadata().target(),
            fields
        );
    }

    fn enter(&self, _span: &tracing::Id) {}

    fn exit(&self, _span: &tracing::Id) {}
}

#[derive(Default)]
struct TraceFieldVisitor {
    fields: Vec<String>,
}

impl tracing::field::Visit for TraceFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.fields.push(format!("{value:?}"));
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("failed to start Tokio runtime: {source}")]
    TokioRuntime { source: io::Error },
    #[error("unknown command: {command}")]
    UnknownCommand { command: String },
    #[error(transparent)]
    VmnetCli(#[from] VmnetCliError),
    #[error("vmnet gateway failed: {error:?}")]
    VmnetGateway { error: VmnetRuntimeError },
    #[error(transparent)]
    PayloadCli(#[from] PayloadCliError),
    #[error(transparent)]
    PayloadClient(#[from] PayloadClientError),
    #[error(transparent)]
    SupervisorControl(#[from] SupervisorControlIoError),
    #[error("failed to format supervisor control JSON: {source}")]
    SupervisorControlJson { source: serde_json::Error },
    #[error("prepare failed: {source}")]
    Prepare { source: LaunchError },
    #[error(transparent)]
    Launch(#[from] LaunchCliError),
    #[error(transparent)]
    Wrapper(#[from] WrapperError),
    #[cfg(feature = "validation-self-test")]
    #[error("{0}")]
    SelfTest(String),
}

#[cfg(feature = "validation-self-test")]
fn is_agentvm_self_test_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("agentvm-self-test")
}

fn is_agentvm_program(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("agentvm")
}

async fn run_async(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let args: Vec<String> = args.into_iter().collect();
    match args.first().map(String::as_str) {
        Some("launch") => {
            info!(ui_mode = "plain", "starting async launch command");
            run_launch_async(&args[1..], WrapperUiMode::Plain)
                .await
                .map_err(Into::into)
        }
        Some("control") => run_control_async(&args[1..]).await,
        Some("vmnet-gateway") => {
            let config = vmnet_gateway_config_from_args(&args[1..])?;
            serve_vmnet_gateway_async(config)
                .await
                .map_err(|error| CliError::VmnetGateway { error })?;
            Ok(())
        }
        Some("payload-client") => run_payload_client_async(&args[1..]).await,
        _ => run(args),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlCliConfig {
    socket_path: PathBuf,
    command: ControlCliCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlCliCommand {
    Status,
    Shutdown { reason: String },
}

async fn run_control_async(args: &[String]) -> CliResult<()> {
    let config = control_cli_config_from_args(args).map_err(LaunchCliError::Message)?;
    print!("{}", run_control_command_async(config).await?);
    Ok(())
}

async fn run_payload_client_async(args: &[String]) -> CliResult<()> {
    let config = payload_client_config_from_args(args)?;
    let addr = socket_addr(&config.host, config.port)?;
    if config.ping {
        ping_payload_async_tcp(addr).await?;
        return Ok(());
    }
    let script = config.script.ok_or(PayloadCliError::MissingScript)?;
    let exit_code = if config.diagnostic {
        let request = DiagnosticRequest {
            script,
            cwd: config.cwd,
            env: config.env,
            timeout_seconds: config.timeout_seconds,
            max_output_bytes: config.max_output_bytes,
        };
        let mut output = tokio::io::stdout();
        run_diagnostic_tcp_async(addr, &request, &mut output).await?
    } else {
        let request = PayloadRequest {
            script,
            cwd: config.cwd,
            env: config.env,
            rows: config.rows,
            cols: config.cols,
        };
        let input = (!config.no_stdin)
            .then(|| Box::new(tokio::io::stdin()) as Box<dyn tokio::io::AsyncRead + Send + Unpin>);
        let mut output = tokio::io::stdout();
        run_payload_tcp_async_with_control(
            addr,
            &request,
            input,
            &mut output,
            PayloadControlOptions::interactive(),
        )
        .await?
        .into_exit_code()?
    };
    std::process::exit(payload_exit_status(exit_code));
}

async fn run_control_command_async(config: ControlCliConfig) -> CliResult<String> {
    let client = SupervisorControlClient::new(config.socket_path);
    match config.command {
        ControlCliCommand::Status => {
            let snapshot = client.status_snapshot().await?;
            let json = serde_json::to_string_pretty(&snapshot)
                .map_err(|source| CliError::SupervisorControlJson { source })?;
            Ok(format!("{json}\n"))
        }
        ControlCliCommand::Shutdown { reason } => {
            client.request_shutdown(reason).await?;
            Ok("shutdown requested\n".to_string())
        }
    }
}

fn control_cli_config_from_args(args: &[String]) -> Result<ControlCliConfig, String> {
    let matches = parse_clap_matches(control_clap_command(), args)?;
    let mut project = matches
        .get_one::<String>("project")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut run_dir = matches
        .get_one::<String>("run_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".sandbox/docker-vm/run"));
    if !project.is_absolute() {
        project = absolute_cli_path(&project.display().to_string())?;
    }
    if !run_dir.is_absolute() {
        run_dir = project.join(&run_dir);
    }
    let socket_path = matches
        .get_one::<String>("socket")
        .map(PathBuf::from)
        .unwrap_or_else(|| control_socket_path(&RuntimePaths::under(run_dir)));
    let command = match matches.subcommand() {
        Some(("status", _)) => ControlCliCommand::Status,
        Some(("shutdown", subcommand)) => ControlCliCommand::Shutdown {
            reason: subcommand
                .get_one::<String>("reason")
                .cloned()
                .unwrap_or_else(|| "control client requested shutdown".to_string()),
        },
        _ => return Err("control command must be status or shutdown".to_string()),
    };
    Ok(ControlCliConfig {
        socket_path,
        command,
    })
}

fn control_clap_command() -> ClapCommand {
    ClapCommand::new("control")
        .arg(
            Arg::new("project")
                .long("project")
                .value_name("PATH")
                .global(true),
        )
        .arg(
            Arg::new("run_dir")
                .long("run-dir")
                .value_name("PATH")
                .global(true),
        )
        .arg(
            Arg::new("socket")
                .long("socket")
                .value_name("PATH")
                .global(true),
        )
        .subcommand_required(true)
        .subcommand(ClapCommand::new("status"))
        .subcommand(
            ClapCommand::new("shutdown").arg(Arg::new("reason").long("reason").value_name("TEXT")),
        )
}

fn run(args: impl IntoIterator<Item = String>) -> CliResult<()> {
    let args: Vec<String> = args.into_iter().collect();
    let command = args.first().map(String::as_str).unwrap_or("help");
    info!(
        command,
        argc = args.len().saturating_sub(1),
        "running command"
    );
    match args.first().map(String::as_str) {
        Some("payload-client") => Err(CliError::UnknownCommand {
            command: "payload-client".to_string(),
        }),
        Some("self-test") => Err(CliError::UnknownCommand {
            command: "self-test".to_string(),
        }),
        Some("prepare") => {
            let (config, policy_args) = frontend_config_from_args(&args[1..])?;
            let mounts = runtime_mounts(&config, &policy_args)?;
            let policy = policy_from_args(config.network.clone(), policy_args)?;
            let prep = prepare_frontend_launch_with_policy(&config, &mounts, &policy)
                .map_err(|source| CliError::Prepare { source })?;
            println!("{}", prep.qemu_command.join(" "));
            Ok(())
        }
        Some("launch") => Err(CliError::UnknownCommand {
            command: "launch".to_string(),
        }),
        Some("-h" | "--help") | None => {
            print_usage();
            Ok(())
        }
        Some(command) => Err(CliError::UnknownCommand {
            command: command.to_string(),
        }),
    }
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

const NODE_HTTP_TOOL: &str = "http:node[url=https://unofficial-builds.nodejs.org/download/release/v24.15.0/node-v24.15.0-linux-x64-musl.tar.gz]";
const NODE_HTTP_VERSION: &str = "24.15.0";

fn setup_tool_mise_exec_script(project: &Path, tool: SetupTool, final_script: String) -> String {
    mise_exec_script(
        &project_mise_config_path(project),
        &format!("agentvm: mise is required to install {} CLI", tool.cli()),
        final_script,
    )
}

fn mise_exec_script(
    mise_config_path: &Path,
    missing_mise_message: &str,
    final_script: String,
) -> String {
    let mise_config = mise_config_path.display().to_string();
    let mise_dir = mise_config_path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .display()
        .to_string();
    [
        format!("export AGENTVM_MISE_CONFIG={}", shell_quote(&mise_config)),
        format!("export AGENTVM_MISE_DIR={}", shell_quote(&mise_dir)),
        r#"export MISE_TRUSTED_CONFIG_PATHS="$AGENTVM_MISE_CONFIG${MISE_TRUSTED_CONFIG_PATHS:+:$MISE_TRUSTED_CONFIG_PATHS}""#.to_string(),
        "export MISE_YES=true MISE_TERMINAL_PROGRESS=false NPM_CONFIG_PROGRESS=false NPM_CONFIG_AUDIT=false NPM_CONFIG_FUND=false NPM_CONFIG_MAXSOCKETS=1 NPM_CONFIG_FETCH_RETRIES=5 NPM_CONFIG_FETCH_RETRY_MINTIMEOUT=2000 NPM_CONFIG_FETCH_RETRY_MAXTIMEOUT=20000".to_string(),
        format!(
            "if ! command -v mise >/dev/null 2>&1; then printf '%s\\n' {} >&2; exit 127; fi",
            shell_quote(missing_mise_message)
        ),
        r#"if [ ! -f "$AGENTVM_MISE_CONFIG" ]; then printf '%s\n' "agentvm: missing mise config $AGENTVM_MISE_CONFIG" >&2; exit 1; fi"#.to_string(),
        "hash -r 2>/dev/null || true".to_string(),
        format!(
            "exec mise -C \"$AGENTVM_MISE_DIR\" exec -- sh -c {} sh \"$PWD\" {}",
            shell_quote("cd \"$1\" && shift && exec /bin/sh -c \"$1\""),
            shell_quote(&final_script),
        ),
    ]
    .join(" && ")
}

fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .expect("shell argument contains an embedded NUL byte")
        .into_owned()
}

fn toml_basic_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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

fn print_usage() {
    eprintln!(
        "usage: agentvm-frontend <prepare|launch|control|vmnet-gateway|payload-client> [options]\n\
         prepare/launch options: [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--gh] [--aws PROFILE] [--ro PATH] [--rw PATH] [--guest-http-smoke-url URL] [--mirror-guest-logs] [--allow-public-internet|--no-net] [--qemu-timeout-seconds N] [--local-http-smoke-upstream IP:PORT] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--payload-script SCRIPT] [--payload-cwd PATH] [--payload-env KEY=VALUE] [--payload-no-stdin] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         vmnet-gateway options: --socket PATH [--allow-ip IP_OR_CIDR] [--allow-domain DOMAIN] [--allow-public-internet|--no-net] [--host-docker-listener HOST:GUEST] [--host-payload-listener HOST:GUEST] [--publish HOST:GUEST] [--pcap PATH] [--tls-ca-cert PATH --tls-ca-key PATH --tls-generate-per-host-certs]\n\
         payload-client options: --port PORT [--host HOST] [--ping|--script SCRIPT] [--diagnostic] [--cwd PATH] [--env KEY=VALUE] [--rows N] [--cols N] [--no-stdin] [--timeout-seconds N] [--max-output-bytes N]\n\
         control options: [--project PATH] [--run-dir PATH] [--socket PATH] <status|shutdown [--reason TEXT]>"
    );
}

#[cfg(feature = "validation-self-test")]
fn print_self_test_usage() {
    eprintln!(
        "usage: agentvm-self-test [--project PATH] [--run-dir PATH] [--artifact-manifest PATH] [--qemu PATH] [--image IMAGE] [--publish-payload-port PORT] [--skip-sqlite-concurrency] [--no-net] [--hostile]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentvm_frontend::supervisor::{
        LaunchSupervisor, SupervisorShutdown, SupervisorTaskName, SupervisorTaskStatus,
    };
    use agentvm_frontend::supervisor_control::{
        bind_control_socket, serve_control_listener_until_shutdown, SupervisorControlSnapshot,
    };
    use std::ops::Deref;
    use std::sync::Arc;

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
        assert_eq!(
            config.policy.egress.allow_ip_ranges[0].as_net().to_string(),
            "93.184.216.34/32"
        );
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
        let error = vmnet_gateway_config_from_args(&[]).expect_err("missing socket");

        assert!(matches!(error, VmnetCliError::MissingSocket));
        assert_eq!(error.to_string(), "--socket is required");
    }

    #[test]
    fn parses_control_status_defaults_to_project_runtime_socket() {
        let config = control_cli_config_from_args(&["status".to_string()]).expect("control config");
        let expected = env::current_dir()
            .expect("cwd")
            .join(".sandbox/docker-vm/run/agentvm-control.sock");

        assert_eq!(config.socket_path, expected);
        assert_eq!(config.command, ControlCliCommand::Status);
    }

    #[test]
    fn parses_control_shutdown_with_socket_override_and_reason() {
        let config = control_cli_config_from_args(&[
            "--socket".to_string(),
            "/tmp/agentvm-control.sock".to_string(),
            "shutdown".to_string(),
            "--reason".to_string(),
            "operator requested shutdown".to_string(),
        ])
        .expect("control config");

        assert_eq!(
            config.socket_path,
            PathBuf::from("/tmp/agentvm-control.sock")
        );
        assert_eq!(
            config.command,
            ControlCliCommand::Shutdown {
                reason: "operator requested shutdown".to_string(),
            }
        );
    }

    fn test_frontend_config(project: impl Into<PathBuf>) -> FrontendConfig {
        let project = project.into();
        FrontendConfig {
            project: project.clone(),
            tools: agentvm_frontend::ToolPaths {
                qemu_system_x86_64: PathBuf::from("qemu-system-x86_64"),
            },
            artifacts: agentvm_frontend::VmArtifacts {
                kernel: project.join("docker/out/vmlinuz"),
                initrd: project.join("docker/out/initrd.img"),
                rootfs: project.join("docker/out/rootfs.raw"),
            },
            runtime: RuntimePaths::under(project.join(".sandbox/docker-vm/run")),
            vm: agentvm_frontend::VmShape {
                memory_bytes: 2 * 1024 * 1024 * 1024,
                cpus: 2,
                kernel_cmdline: "console=ttyS0".to_string(),
                virtiofs_tag: agentvm_frontend::COMPOSED_FS_TAG.to_string(),
            },
            network: GuestNetwork::default(),
            guest_http_smoke_url: None,
            guest_log_dir: None,
            upstream_mappings: Vec::new(),
        }
    }

    #[tokio::test]
    async fn control_status_command_reads_snapshot_from_socket() {
        let root = unique_temp_dir();
        let socket = root.join("agentvm-control.sock");
        let listener = bind_control_socket(&socket).expect("bind control socket");
        let supervisor = Arc::new(LaunchSupervisor::new(
            test_frontend_config(root.as_ref()).supervisor_plan(),
        ));
        supervisor
            .task_controller(SupervisorTaskName::Qemu)
            .expect("qemu controller")
            .mark_ready()
            .expect("mark qemu ready");
        let server_supervisor = Arc::clone(&supervisor);
        let server = tokio::spawn(async move {
            serve_control_listener_until_shutdown(listener, server_supervisor).await
        });

        let output = run_control_command_async(ControlCliConfig {
            socket_path: socket,
            command: ControlCliCommand::Status,
        })
        .await
        .expect("control status");
        let snapshot: SupervisorControlSnapshot =
            serde_json::from_str(&output).expect("snapshot json");
        assert!(snapshot.tasks.iter().any(|task| {
            task.name == SupervisorTaskName::Qemu && task.status == SupervisorTaskStatus::Ready
        }));

        supervisor.request_shutdown("status test finished");
        server.await.expect("server join").expect("server result");
    }

    #[tokio::test]
    async fn control_shutdown_command_requests_supervisor_shutdown() {
        let root = unique_temp_dir();
        let socket = root.join("agentvm-control.sock");
        let listener = bind_control_socket(&socket).expect("bind control socket");
        let supervisor = Arc::new(LaunchSupervisor::new(
            test_frontend_config(root.as_ref()).supervisor_plan(),
        ));
        let server_supervisor = Arc::clone(&supervisor);
        let server = tokio::spawn(async move {
            serve_control_listener_until_shutdown(listener, server_supervisor).await
        });

        let output = run_control_command_async(ControlCliConfig {
            socket_path: socket,
            command: ControlCliCommand::Shutdown {
                reason: "operator requested shutdown".to_string(),
            },
        })
        .await
        .expect("control shutdown");

        assert_eq!(output, "shutdown requested\n");
        assert_eq!(
            supervisor.current_shutdown(),
            SupervisorShutdown::Requested {
                reason: "operator requested shutdown".to_string(),
            }
        );
        server.await.expect("server join").expect("server result");
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
        let ca_cert = root.join("ca.pem");
        let ca_key = root.join("ca-key.pem");
        std::fs::write(&ca_cert, "test ca").expect("ca cert");
        std::fs::write(&ca_key, "test key").expect("ca key");

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
            ca_cert.display().to_string(),
            "--tls-ca-key".to_string(),
            ca_key.display().to_string(),
            "--tls-generate-per-host-certs".to_string(),
        ])
        .expect("config");

        assert_eq!(config.vm.cpus, 2);
        assert_eq!(
            config.guest_http_smoke_url.as_deref(),
            Some("http://93.184.216.34/")
        );
        assert_eq!(config.guest_log_dir, None);
        assert!(policy.allow_public);
        assert_eq!(
            policy.host_listeners,
            vec![
                HostListener::docker_api(23750, 2375),
                HostListener::payload_control(12076, 1076),
                HostListener::published_tcp(18080, 8080),
            ]
        );
        let runtime_policy =
            policy_from_args(config.network.clone(), policy.clone()).expect("policy");
        assert_eq!(runtime_policy.tls_mitm.ca_cert_path, Some(ca_cert));
        assert_eq!(runtime_policy.tls_mitm.ca_key_path, Some(ca_key));
        assert!(runtime_policy.tls_mitm.generate_per_host_certs);

        let mounts = runtime_mounts(&config, &policy).expect("runtime mounts");
        let prep = prepare_frontend_launch_with_policy(&config, &mounts, &runtime_policy)
            .expect("prepare launch");
        let qemu_command = prep.qemu_command.join(" ");
        assert!(!qemu_command.contains("agentvm_project="));
        assert!(!qemu_command.contains("agentvm_guest_ip="));
        assert!(!qemu_command.contains("agentvm_http_smoke_url="));
        let launch_json = std::fs::read_to_string(&config.runtime.guest_launch_config)
            .expect("guest launch config");
        assert!(launch_json.contains("\"project_path\":"));
        assert!(launch_json.contains("\"guest_ip\": \"10.0.2.15\""));
        assert!(launch_json.contains("\"http_smoke_url\": \"http://93.184.216.34/\""));
        assert!(launch_json.contains("\"guest_log_dir\": null"));
    }

    #[test]
    fn mirror_guest_logs_sets_project_vmlogs_guest_dir() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            project.display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--mirror-guest-logs".to_string(),
        ])
        .expect("config");

        let expected = project.join(".vmlogs").display().to_string();
        assert_eq!(config.guest_log_dir.as_deref(), Some(expected.as_str()));
        let runtime_policy = policy_from_args(config.network.clone(), policy).expect("policy");
        let mounts = runtime_mounts(&config, &PolicyArgs::default()).expect("runtime mounts");
        prepare_frontend_launch_with_policy(&config, &mounts, &runtime_policy)
            .expect("prepare launch");
        let launch_json = std::fs::read_to_string(&config.runtime.guest_launch_config)
            .expect("guest launch config");
        assert!(launch_json.contains(&format!("\"guest_log_dir\": \"{expected}\"")));
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

        let runtime_policy = policy_from_args(config.network.clone(), policy).expect("policy");
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
    fn payload_control_listener_default_uses_reserved_ephemeral_port() {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());

        let reservation = ensure_payload_listener(&mut policy).expect("payload listener");
        let host_port = reservation.host_port();

        assert_ne!(host_port, 0);
        assert_eq!(
            policy.host_listeners,
            vec![HostListener::payload_control(host_port, 1076)]
        );
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, host_port)).is_err());
        drop(reservation);
        let rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, host_port))
            .expect("reserved port should be released on drop");
        drop(rebound);
    }

    #[test]
    fn payload_control_listener_allocates_distinct_ports_for_concurrent_projects() {
        let mut first = VmnetPolicy::default_sandbox(GuestNetwork::default());
        let mut second = VmnetPolicy::default_sandbox(GuestNetwork::default());

        let first_reservation = ensure_payload_listener(&mut first).expect("first listener");
        let second_reservation = ensure_payload_listener(&mut second).expect("second listener");

        assert_ne!(
            first_reservation.host_port(),
            second_reservation.host_port()
        );
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, first_reservation.host_port())).is_err());
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, second_reservation.host_port())).is_err());
    }

    #[test]
    fn payload_control_listener_keeps_explicit_host_port() {
        let mut policy = VmnetPolicy::default_sandbox(GuestNetwork::default());
        policy
            .host_listeners
            .push(HostListener::payload_control(12076, 1076));

        let reservation = ensure_payload_listener(&mut policy).expect("payload listener");

        assert_eq!(reservation.host_port(), 12076);
        assert_eq!(
            policy.host_listeners,
            vec![HostListener::payload_control(12076, 1076)]
        );
    }

    #[test]
    fn frontend_policy_rejects_invalid_allow_ip_at_boundary() {
        let root = frontend_test_root();
        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--allow-ip".to_string(),
            "not-an-ip".to_string(),
        ])
        .expect("config args parse before typed policy conversion");

        let error = policy_from_args(config.network.clone(), policy).expect_err("invalid range");
        assert_eq!(
            error.to_string(),
            "invalid --allow-ip: invalid IPv4 address or CIDR range: not-an-ip"
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
            frontend_config_from_args(&publish_args)
                .expect_err("publish rejected")
                .to_string(),
            "--no-net cannot be combined with --publish"
        );

        let mut allow_args = base_args.to_vec();
        allow_args.extend(["--allow-public-internet".to_string()]);
        assert_eq!(
            frontend_config_from_args(&allow_args)
                .expect_err("allow rejected")
                .to_string(),
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
        assert!(policy.gh);
        assert_eq!(policy.aws_profile.as_deref(), Some("dev"));
        assert_eq!(policy.extra_ro, vec![ro]);
        assert_eq!(policy.extra_rw, vec![rw]);
        let payload = launch_payload_args(&config, &policy)
            .expect("payload")
            .expect("payload script");
        assert_eq!(payload.script, "exec codex --model gpt-5");
        assert_eq!(payload.cwd, config.project.display().to_string());
    }

    #[test]
    fn payload_cwd_override_is_preserved() {
        let root = frontend_test_root();
        let cwd = root.join("repo/subdir");
        let (config, policy) = frontend_config_from_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--run-dir".to_string(),
            root.join(".sandbox/docker-vm/run").display().to_string(),
            "--artifact-manifest".to_string(),
            root.join("docker/out/artifact-manifest.json")
                .display()
                .to_string(),
            "--payload-script".to_string(),
            "pwd".to_string(),
            "--payload-cwd".to_string(),
            cwd.display().to_string(),
        ])
        .expect("config");

        let payload = launch_payload_args(&config, &policy)
            .expect("payload")
            .expect("payload script");
        assert_eq!(payload.cwd, cwd.display().to_string());
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
            .expect_err("removed flag rejected")
            .to_string();
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
    fn wrapper_args_build_typed_launch_request() {
        let args = parse_wrapper_args(&[
            "--project".to_string(),
            "/tmp/project".to_string(),
            "--no-net".to_string(),
            "--docker-publish".to_string(),
            "18080:8080".to_string(),
            "--mirror-guest-logs".to_string(),
            "--".to_string(),
            "true".to_string(),
        ])
        .expect("wrapper args");

        assert!(!args.reset);
        assert!(args.launch.policy.no_net);
        assert!(!args.launch.policy.allow_public);
        assert!(!args.tls_bootstrap);
        assert_eq!(
            args.launch.guest_log_dir,
            Some(PathBuf::from("/tmp/project/.vmlogs"))
        );
        assert!(args
            .launch
            .policy
            .host_listeners
            .contains(&HostListener::published_tcp(18080, 8080)));
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec true")
        );
    }

    #[test]
    fn setup_tool_defaults_to_public_egress_for_tool_install() {
        let root = frontend_test_root();
        let args = parse_wrapper_args(&[
            "--project".to_string(),
            root.join("repo").display().to_string(),
            "--setup-tool".to_string(),
            "codex".to_string(),
        ])
        .expect("wrapper args");

        assert!(args.launch.policy.allow_public);
        assert!(args.tls_bootstrap);
    }

    #[test]
    fn wrapper_selects_tui_for_interactive_terminals_by_default() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");
        let args = parse_wrapper_args_with_terminal(
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
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");
        let args = parse_wrapper_args_with_terminal(
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
    }

    #[test]
    fn wrapper_non_tty_selects_plain_mode() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");
        let stdin_plain = parse_wrapper_args_with_terminal(
            &["--project".to_string(), project.display().to_string()],
            false,
            true,
        )
        .expect("stdin");
        let stdout_plain = parse_wrapper_args_with_terminal(
            &["--project".to_string(), project.display().to_string()],
            true,
            false,
        )
        .expect("stdout");

        assert_eq!(stdin_plain.ui_mode, WrapperUiMode::Plain);
        assert_eq!(stdout_plain.ui_mode, WrapperUiMode::Plain);
    }

    #[test]
    fn interactive_wrap_without_tool_defaults_to_bash() {
        let root = frontend_test_root();
        let args = parse_wrapper_args_with_terminal(
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash")
        );
    }

    #[test]
    fn configured_codex_project_runs_configured_payload() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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
        let host_home = host_home_dir().expect("host home");
        assert!(args
            .launch
            .policy
            .extra_shares
            .iter()
            .any(|share| { !share.readonly && share.host_path == host_home.join(".codex") }));
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec codex --dangerously-bypass-approvals-and-sandbox")
        );
    }

    #[test]
    fn setup_tool_pi_config_runs_pi_payload_with_recipe_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let args = parse_wrapper_args_with_terminal(
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
        let host_home = host_home_dir().expect("host home");
        assert!(args
            .launch
            .policy
            .extra_shares
            .iter()
            .any(|share| { !share.readonly && share.host_path == host_home.join(".pi") }));
        let payload_script = &args.launch.policy.payload.as_ref().expect("payload").script;
        assert!(payload_script.contains("/mise.toml"));
        assert!(payload_script.contains("exec mise -C \"$AGENTVM_MISE_DIR\" exec --"));
        assert!(payload_script.contains("MISE_TRUSTED_CONFIG_PATHS"));
        assert!(!payload_script.contains("mise --no-config"));
        assert!(payload_script.contains("exec pi"));
    }

    #[test]
    fn setup_tool_pi_script_execs_project_mise_tools_then_runs_plain_command() {
        let final_script = payload_script_from_config_command(&ConfigCommand {
            command: "pi".to_string(),
            args: vec!["--model".to_string(), "claude 3.5".to_string()],
        });
        let script =
            setup_tool_mise_exec_script(Path::new("/project"), SetupTool::Pi, final_script);

        assert!(script.contains("command -v mise >/dev/null 2>&1"));
        assert!(script.contains("AGENTVM_MISE_CONFIG=/project/mise.toml"));
        assert!(script.contains("exec mise -C \"$AGENTVM_MISE_DIR\" exec --"));
        assert!(script.contains("MISE_TRUSTED_CONFIG_PATHS"));
        assert!(!script.contains("mise --no-config"));
        assert!(script.contains("MISE_TERMINAL_PROGRESS=false"));
        assert!(script.contains("NPM_CONFIG_PROGRESS=false"));
        assert!(script.contains("NPM_CONFIG_MAXSOCKETS=1"));
        assert!(script.contains("agentvm: mise is required to install pi CLI"));
        assert!(!script.contains("agentvm: installing pi CLI with mise"));
        assert!(script.contains("exec /bin/sh -c"));
        assert!(script.contains("exec pi --model 'claude 3.5'"));
    }

    #[test]
    fn codex_setup_tool_script_uses_project_mise_and_persists_auto_flags() {
        let final_script = payload_script_from_config_command(&ConfigCommand {
            command: "codex".to_string(),
            args: vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "--profile".to_string(),
                "work account".to_string(),
            ],
        });
        let script =
            setup_tool_mise_exec_script(Path::new("/project"), SetupTool::Codex, final_script);

        assert!(script.contains("command -v mise >/dev/null 2>&1"));
        assert!(script.contains("AGENTVM_MISE_CONFIG=/project/mise.toml"));
        assert!(script.contains("exec mise -C \"$AGENTVM_MISE_DIR\" exec --"));
        assert!(script.contains("MISE_TRUSTED_CONFIG_PATHS"));
        assert!(!script.contains("mise --no-config"));
        assert!(script.contains("agentvm: mise is required to install codex CLI"));
        assert!(!script.contains("agentvm: installing codex CLI with mise"));
        assert!(script.contains(
            "exec codex --dangerously-bypass-approvals-and-sandbox --profile 'work account'"
        ));
    }

    #[test]
    fn setup_tool_rejects_unsupported_recipe_with_actionable_diagnostic() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let error = parse_wrapper_args_with_terminal(
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
        let config = WrapperSandboxConfig::setup_tool(SetupTool::Codex).expect("setup tool");

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
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash -l")
        );
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
                schema_version: 3,
                default_command: ConfigCommand::new("bash"),
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
            .launch
            .policy
            .allow_domains
            .contains(&"example.com".to_string()));
        assert!(args
            .launch
            .policy
            .allow_domains
            .contains(&"api.example.com".to_string()));
        assert!(args
            .launch
            .policy
            .allow_ips
            .contains(&"93.184.216.34".to_string()));
        assert!(args.launch.policy.gh);
        assert_eq!(args.launch.policy.aws_profile.as_deref(), Some("dev"));
        assert!(args.launch.policy.extra_shares.iter().any(|share| {
            share.readonly && share.guest_path == PathBuf::from("/opt/share") && !share.required
        }));
        assert!(args
            .launch
            .policy
            .host_listeners
            .contains(&HostListener::published_tcp(18080, 8080)));
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
                schema_version: 3,
                default_command: ConfigCommand::new("bash"),
                network: ConfigNetwork::default(),
                auth: ConfigAuth::default(),
                shares: vec![ConfigShare {
                    host_path: share.display().to_string(),
                    guest_path: Some("/home/test/.codex".to_string()),
                    access: ConfigShareAccess::Rw,
                    required: true,
                    shadows: vec!["tmp/arg0".to_string()],
                }],
                published_ports: Vec::new(),
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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

        assert!(args.launch.policy.extra_shares.iter().any(|share| {
            !share.readonly && share.guest_path == PathBuf::from("/home/test/.codex")
        }));
        assert!(args.launch.policy.extra_share_shadows.iter().any(|shadow| {
            shadow.parent_guest_path == PathBuf::from("/home/test/.codex")
                && shadow.relative_path == PathBuf::from("tmp/arg0")
                && shadow
                    .backing_path
                    .ends_with(".sandbox/root/home/test/.codex/tmp/arg0")
        }));

        let (frontend, policy) =
            frontend_config_from_launch_request(&args.launch).expect("frontend");
        let mounts = runtime_mounts(&frontend, &policy).expect("mounts");
        let shadow_backing = project.join(".sandbox/root/home/test/.codex/tmp/arg0");
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
    fn config_share_shadow_allows_readonly_parent_with_writable_shadow() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let share = root.join("host-readonly");
        std::fs::create_dir_all(&share).expect("share");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig {
                schema_version: 3,
                default_command: ConfigCommand::new("bash"),
                network: ConfigNetwork::default(),
                auth: ConfigAuth::default(),
                shares: vec![ConfigShare {
                    host_path: share.display().to_string(),
                    guest_path: Some("/mnt/share".to_string()),
                    access: ConfigShareAccess::Ro,
                    required: true,
                    shadows: vec!["cache".to_string()],
                }],
                published_ports: Vec::new(),
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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

        assert!(args.launch.policy.extra_shares.iter().any(|share| {
            share.readonly && share.guest_path == PathBuf::from("/mnt/share") && share.required
        }));
        let (frontend, policy) =
            frontend_config_from_launch_request(&args.launch).expect("frontend");
        let mounts = runtime_mounts(&frontend, &policy).expect("mounts");
        let parent_index = mounts
            .iter()
            .position(|mount| mount.guest_path == PathBuf::from("/mnt/share"))
            .expect("parent mount");
        let shadow_index = mounts
            .iter()
            .position(|mount| mount.guest_path == PathBuf::from("/mnt/share/cache"))
            .expect("shadow mount");
        assert!(parent_index < shadow_index);
        assert!(mounts[parent_index].readonly);
        assert!(!mounts[shadow_index].readonly);
        assert_eq!(
            mounts[shadow_index].host_path,
            project.join(".sandbox/root/mnt/share/cache")
        );
    }

    #[test]
    fn config_share_shadow_validation_accepts_readonly_and_rejects_escaping_paths() {
        let readonly_shadow = WrapperSandboxConfig {
            schema_version: 3,
            default_command: ConfigCommand::new("bash"),
            network: ConfigNetwork::default(),
            auth: ConfigAuth::default(),
            shares: vec![ConfigShare {
                host_path: "/tmp/host".to_string(),
                guest_path: Some("/tmp/guest".to_string()),
                access: ConfigShareAccess::Ro,
                required: true,
                shadows: vec!["tmp".to_string()],
            }],
            published_ports: Vec::new(),
        };
        readonly_shadow.validate().expect("readonly parent shadow");

        let escaping_shadow = WrapperSandboxConfig {
            shares: vec![ConfigShare {
                access: ConfigShareAccess::Rw,
                shadows: vec!["../tmp".to_string()],
                ..readonly_shadow.shares[0].clone()
            }],
            ..readonly_shadow
        };
        let escaping_error = escaping_shadow.validate().expect_err("escaping shadow");
        assert!(matches!(
            escaping_error,
            ConfigError::EscapingShareShadowPath
        ));
        assert_eq!(
            escaping_error.to_string(),
            "share shadow path must stay under the parent share"
        );

        let duplicate_shadow = WrapperSandboxConfig {
            shares: vec![ConfigShare {
                access: ConfigShareAccess::Rw,
                shadows: vec!["tmp".to_string(), "./tmp".to_string()],
                ..escaping_shadow.shares[0].clone()
            }],
            ..escaping_shadow
        };
        let duplicate_error = duplicate_shadow.validate().expect_err("duplicate shadow");
        assert!(matches!(
            duplicate_error,
            ConfigError::DuplicateShareShadowPath { ref path } if path == "./tmp"
        ));
        assert_eq!(
            duplicate_error.to_string(),
            "duplicate sandbox config share shadow path: ./tmp"
        );
    }

    #[test]
    fn config_share_shadow_json_uses_string_array_not_objects() {
        let config: WrapperSandboxConfig = serde_json::from_str(
            r#"{
                "schema_version": 3,
                "default_command": { "command": "bash", "args": [] },
                "shares": [{
                    "host_path": "/tmp/host",
                    "guest_path": "/tmp/guest",
                    "access": "rw",
                    "shadows": ["tmp"]
                }]
            }"#,
        )
        .expect("string shadows");
        assert_eq!(config.shares[0].shadows, vec!["tmp".to_string()]);

        let object_form = r#"{
            "schema_version": 3,
            "default_command": { "command": "bash", "args": [] },
            "shares": [{
                "host_path": "/tmp/host",
                "guest_path": "/tmp/guest",
                "access": "rw",
                "shadows": [{ "path": "tmp" }]
            }]
        }"#;
        serde_json::from_str::<WrapperSandboxConfig>(object_form)
            .expect_err("object-form shadows are obsolete");
    }

    #[test]
    fn shell_command_override_allows_unconfigured_plain_project() {
        let root = frontend_test_root();
        let project = root.join("repo");

        let args = parse_wrapper_args_with_terminal(
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
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash -l")
        );
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
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("legacy wrapper");

        let host_home = host_home_dir().expect("host home");
        assert!(args
            .launch
            .policy
            .extra_shares
            .iter()
            .any(|share| { !share.readonly && share.host_path == host_home.join(".codex") }));
        assert!(args.launch.policy.payload.is_some());
    }

    #[test]
    fn setup_tool_codex_writes_expected_config_json() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let args = parse_wrapper_args_with_terminal(
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
            &WrapperSandboxConfig::setup_tool(args.setup_tool.expect("setup tool"))
                .expect("setup config"),
        )
        .expect("write config");

        let text = std::fs::read_to_string(wrapper_sandbox_config_path(&project)).expect("config");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["schema_version"], 3);
        assert!(value.get("setup_tool").is_none());
        assert_eq!(value["default_command"]["command"], "codex");
        assert_eq!(
            value["default_command"]["args"],
            serde_json::json!(["--dangerously-bypass-approvals-and-sandbox"])
        );
        assert!(value.get("tool_state").is_none());
        assert_eq!(value["network"]["mode"], "public");
        let host_home = host_home_dir().expect("host home");
        assert_eq!(
            value["shares"][0]["host_path"],
            host_home.join(".codex").display().to_string()
        );
        assert_eq!(
            value["shares"][0]["guest_path"],
            host_home.join(".codex").display().to_string()
        );
        assert_eq!(value["shares"][0]["access"], "rw");
        assert_eq!(value["shares"][0]["required"], false);
        assert_eq!(value["shares"][0]["shadows"], serde_json::json!(["tmp"]));
        assert!(args
            .launch
            .policy
            .extra_shares
            .iter()
            .any(|share| { !share.readonly && share.host_path == host_home.join(".codex") }));
        assert!(args.launch.policy.extra_share_shadows.iter().any(|shadow| {
            shadow.parent_guest_path.ends_with(".codex")
                && shadow.relative_path == PathBuf::from("tmp")
        }));

        write_setup_tool_mise_config(&project, SetupTool::Codex).expect("write mise config");
        let mise_text = std::fs::read_to_string(project_mise_config_path(&project)).expect("mise");
        assert!(mise_text.contains("\"npm:@openai/codex\""));
        assert!(!project.join(".sandbox/mise.toml").exists());
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
        let mut config = WrapperSandboxConfig::codex_default().expect("codex default");
        config.network.mode = ConfigNetworkMode::None;
        config.published_ports.push(ConfigPort {
            host: 18080,
            guest: 8080,
        });
        write_wrapper_sandbox_config(&project, &config).expect("sandbox config");

        let config_default = parse_wrapper_args_with_terminal(
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("config default");
        assert!(config_default.launch.policy.no_net);
        assert!(config_default.launch.policy.host_listeners.is_empty());

        let cli_override = parse_wrapper_args_with_terminal(
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
        assert!(!cli_override.launch.policy.no_net);
        assert!(cli_override
            .launch
            .policy
            .allow_domains
            .contains(&"example.com".to_string()));
        assert!(cli_override
            .launch
            .policy
            .host_listeners
            .contains(&HostListener::published_tcp(18080, 8080)));
    }

    #[test]
    fn agentvm_argv0_uses_wrapper_without_wrap_subcommand() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
            &[
                "--project".to_string(),
                project.display().to_string(),
                "--no-tui".to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper");

        assert!(args.launch.policy.payload.is_some());
    }

    #[tokio::test]
    async fn agentvm_argv0_does_not_expose_self_test_command() {
        assert_eq!(
            run_cli_async(vec!["agentvm".to_string(), "self-test".to_string()])
                .await
                .expect_err("production wrapper should not expose self-test")
                .to_string(),
            "unknown command: self-test"
        );
    }

    #[tokio::test]
    async fn frontend_argv0_does_not_expose_self_test_command() {
        assert_eq!(
            run_cli_async(vec![
                "agentvm-frontend".to_string(),
                "self-test".to_string()
            ])
            .await
            .expect_err("production frontend should not expose self-test")
            .to_string(),
            "unknown command: self-test"
        );
    }

    #[test]
    fn wrapper_command_override_runs_payload_script_and_keeps_configured_codex_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig::codex_default().expect("codex default"),
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash -l")
        );
    }

    #[test]
    fn configured_default_command_can_be_arbitrary_payload() {
        let root = frontend_test_root();
        let project = root.join("repo");
        write_wrapper_sandbox_config(
            &project,
            &WrapperSandboxConfig {
                schema_version: 3,
                default_command: ConfigCommand {
                    command: "bash".to_string(),
                    args: vec!["-l".to_string()],
                },
                network: ConfigNetwork::default(),
                auth: ConfigAuth::default(),
                shares: Vec::new(),
                published_ports: Vec::new(),
            },
        )
        .expect("sandbox config");

        let args = parse_wrapper_args_with_terminal(
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
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash -l")
        );
    }

    #[test]
    fn plain_wrap_without_tool_defaults_to_bash() {
        let root = frontend_test_root();
        let args = parse_wrapper_args_with_terminal(
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            false,
            true,
        )
        .expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Plain);
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash")
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
    fn payload_client_config_reports_structured_parse_errors() {
        let error = payload_client_config_from_args(&[
            "--port".to_string(),
            "12076".to_string(),
            "--ping".to_string(),
            "--env".to_string(),
            "=value".to_string(),
        ])
        .expect_err("empty env key");

        assert!(matches!(error, PayloadCliError::EmptyEnvKey));
        assert_eq!(error.to_string(), "--env key must not be empty");
    }

    #[test]
    fn guest_sync_diagnostic_request_is_bounded_sync() {
        let request = guest_sync_diagnostic_request();

        assert_eq!(request.script, "sync");
        assert_eq!(request.cwd, "/");
        assert_eq!(request.timeout_seconds, 10);
        assert_eq!(request.max_output_bytes, 4096);
        assert!(request.env.is_empty());
    }

    #[tokio::test]
    async fn flush_guest_filesystems_sends_sync_diagnostic() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake payload server");
        let addr = listener.local_addr().expect("listener addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept diagnostic");
            let mut header = [0_u8; 5];
            stream.read_exact(&mut header).expect("read frame header");
            assert_eq!(header[0], b'D');
            let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
            let mut payload = vec![0_u8; len];
            stream.read_exact(&mut payload).expect("read frame payload");
            let request: DiagnosticRequest =
                serde_json::from_slice(&payload).expect("request json");
            assert_eq!(request, guest_sync_diagnostic_request());

            let response = br#"{"exit_code":0}"#;
            stream.write_all(&[b'X']).expect("write frame type");
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .expect("write frame length");
            stream.write_all(response).expect("write frame payload");
        });

        flush_guest_filesystems(addr)
            .await
            .expect("guest sync succeeds");
        server.join().expect("fake server completes");
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
        assert!(summary.contains("state_disk="));
        assert!(summary.contains("qemu_log="));
        assert!(summary.contains("console_log="));
        assert!(summary.contains("vmnet_event_log="));
        assert!(summary.contains("state.json"));
        assert!(summary.contains("state.raw"));
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
        assert_eq!(
            config.runtime.state_disk,
            root.join("repo/.sandbox/docker-vm/state.raw")
        );
        assert!(config.runtime.composed_bind_manifest.is_absolute());
        assert!(config.runtime.state_disk.is_absolute());
    }

    #[test]
    fn reset_project_removes_project_local_sandbox_state() {
        let root = frontend_test_root();
        let project = root.join("repo");
        let overlay_state = project.join(".sandbox/docker-vm/state.raw");
        let runtime_state = project.join(".sandbox/docker-vm/run/state.json");
        std::fs::create_dir_all(runtime_state.parent().expect("runtime parent"))
            .expect("runtime dir");
        std::fs::write(&overlay_state, "state").expect("overlay state");
        std::fs::write(&runtime_state, "{}").expect("runtime state");

        reset_project(&project).expect("reset");

        assert!(!project.join(".sandbox").exists());
    }

    #[test]
    fn wrapper_rejects_removed_flags_as_unknown_arguments() {
        for flag in [
            "--docker",
            "--docker-machine",
            "--pass-env",
            "--tool",
            "--tool-arg",
            "--command",
        ] {
            let error = parse_wrapper_args(&[flag.to_string(), "codex".to_string()])
                .expect_err("removed wrapper flag");
            assert!(
                error.contains(&format!("unexpected argument '{flag}'")),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn wrap_subcommand_is_not_a_compatibility_entrypoint() {
        assert_eq!(
            run_cli_async(vec!["agentvm-frontend".to_string(), "wrap".to_string()])
                .await
                .expect_err("agentvm-frontend wrap removed")
                .to_string(),
            "unknown command: wrap"
        );
        let error = run_cli_async(vec!["agentvm".to_string(), "wrap".to_string()])
            .await
            .expect_err("agentvm wrap removed")
            .to_string();
        assert!(error.contains("unexpected argument 'wrap'"), "{error}");
    }

    #[tokio::test]
    async fn argv0_no_longer_selects_wrapper_or_tool_behavior() {
        assert_eq!(
            run_cli_async(vec![
                "codex-wrap".to_string(),
                "--tool".to_string(),
                "codex".to_string(),
            ])
            .await
            .expect_err("argv0 wrapper removed")
            .to_string(),
            "unknown command: --tool"
        );

        let root = frontend_test_root();
        let args = parse_wrapper_args_with_terminal(
            &[
                "--project".to_string(),
                root.join("repo").display().to_string(),
            ],
            true,
            true,
        )
        .expect("wrapper args");
        assert_eq!(
            args.launch
                .policy
                .payload
                .as_ref()
                .map(|payload| payload.script.as_str()),
            Some("exec bash")
        );
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
        for source in ["docker/build-appliance.sh", "docker/guest-init.sh"] {
            let root = unique_temp_dir();
            std::fs::create_dir_all(root.join("docker/out")).expect("out");
            write_frontend_manifest(&root);
            std::fs::write(root.join(source), b"changed\n").expect("change source");

            let error =
                ensure_appliance_sources_fresh(&root.join("docker/out/artifact-manifest.json"))
                    .expect_err("stale artifact");

            assert!(matches!(
                error,
                ApplianceError::ChangedSourceHash { ref source_path, .. } if source_path == source
            ));
            let error = error.to_string();
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

        assert!(matches!(error, ApplianceError::MissingSourceHashes { .. }));
        let error = error.to_string();
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
