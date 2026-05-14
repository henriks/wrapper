use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
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
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};

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
    match args.first().map(String::as_str) {
        Some("prepare" | "launch" | "vmnet-gateway" | "payload-client" | "self-test")
        | Some("-h" | "--help")
        | None => run(args),
        Some("wrap") => run_wrapper(program, args[1..].to_vec()),
        Some(command) => Err(format!("unknown command: {command}")),
    }
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
        // SAFETY: flock operates on a valid fd owned by file.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(format!(
                "another VM sandbox is already active for this project ({})",
                config.runtime.lock.display()
            ));
        }
        Ok(Self { file })
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // SAFETY: flock operates on a valid fd owned by file.
        unsafe {
            let _ = libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
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
    let ui_mode = wrapper.ui_mode;
    let needs_startup_dialog = ui_mode == WrapperUiMode::Tui && !wrapper.tool_selected;
    let mut launch_args = vec!["launch".to_string()];
    launch_args.extend(wrapper.into_launch_args());
    if needs_startup_dialog {
        let selection = tui::run_startup_dialog().map_err(|error| error.to_string())?;
        if selection.enable_codex {
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
    tls_bootstrap: bool,
    reset: bool,
    help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperUiMode {
    Tui,
    Plain,
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

fn parse_wrapper_args(program: &str, args: &[String]) -> Result<WrapperArgs, String> {
    parse_wrapper_args_with_terminal(
        program,
        args,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
    )
}

fn parse_wrapper_args_with_terminal(
    _program: &str,
    args: &[String],
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> Result<WrapperArgs, String> {
    let mut launch_args = Vec::new();
    let mut project = env::current_dir().map_err(|error| error.to_string())?;
    let mut tool = None;
    let mut reset = false;
    let mut help = false;
    let mut no_net = false;
    let mut no_tui = false;
    let mut tls_bootstrap = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => {
                for arg in &args[index + 1..] {
                    launch_args.extend(["--tool-arg".to_string(), arg.clone()]);
                }
                break;
            }
            "--project" => {
                project = absolute_cli_path(&value(args, &mut index, "--project")?)?;
                launch_args.extend(["--project".to_string(), project.display().to_string()]);
            }
            "--tool" => {
                let selected = value(args, &mut index, "--tool")?;
                selected.parse::<GuestTool>()?;
                tool = Some(selected.clone());
                launch_args.extend(["--tool".to_string(), selected]);
            }
            "--no-net" => {
                no_net = true;
                launch_args.push(args[index].clone());
            }
            "--no-tui" => no_tui = true,
            "--gh" => launch_args.push(args[index].clone()),
            "--aws" | "--ro" | "--rw" | "--qemu" | "--artifact-manifest" => {
                let flag = args[index].clone();
                let val = value(args, &mut index, &flag)?;
                launch_args.extend([flag, val]);
            }
            "--docker-publish" => {
                let val = value(args, &mut index, "--docker-publish")?;
                launch_args.extend(["--publish".to_string(), val]);
            }
            "--reset" => reset = true,
            "--docker" => {
                return Err(
                    "--docker has been removed; the VM is now the default execution model"
                        .to_string(),
                )
            }
            "--docker-machine" => {
                return Err(
                    "--docker-machine has been removed with the legacy QEMU path".to_string(),
                )
            }
            "--pass-env" => return Err(
                "--pass-env has been removed; use explicit VM guest shares/auth options instead"
                    .to_string(),
            ),
            "-h" | "--help" => {
                print_wrapper_usage();
                help = true;
            }
            arg if arg.starts_with('-') => return Err(format!("unknown wrapper option: {arg}")),
            arg => launch_args.extend(["--tool-arg".to_string(), arg.to_string()]),
        }
        index += 1;
    }
    if help {
        return Ok(WrapperArgs {
            project,
            launch_args,
            ui_mode: wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty),
            tool_selected: false,
            tls_bootstrap,
            reset,
            help,
        });
    }
    if !launch_args.iter().any(|arg| arg == "--project") {
        launch_args.extend(["--project".to_string(), project.display().to_string()]);
    }
    let ui_mode = wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty);
    let mut tool_selected = launch_args.iter().any(|arg| arg == "--tool");
    if !tool_selected {
        if let Some(tool) = tool {
            launch_args.extend(["--tool".to_string(), tool]);
            tool_selected = true;
        } else if ui_mode == WrapperUiMode::Plain {
            return Err(
                "no tool selected; use --tool codex|copilot or interactive TUI setup".to_string(),
            );
        }
    }
    if !no_net
        && !launch_args
            .iter()
            .any(|arg| arg == "--allow-public-internet")
    {
        launch_args.push("--allow-public-internet".to_string());
        tls_bootstrap = true;
    }
    Ok(WrapperArgs {
        project,
        launch_args,
        ui_mode,
        tool_selected,
        tls_bootstrap,
        reset,
        help,
    })
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
    // SAFETY: flock operates on a valid fd owned by file.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(format!(
            "--reset refused because a VM sandbox is active for this project ({})",
            runtime.lock.display()
        ));
    }
    if sandbox.exists() {
        fs::remove_dir_all(&sandbox)
            .map_err(|error| format!("failed to remove .sandbox: {error}"))?;
        println!("Removed .sandbox/");
    } else {
        println!(".sandbox/ does not exist, nothing to reset");
    }
    Ok(())
}

fn print_wrapper_usage() {
    eprintln!(
        "usage: agentvm-frontend wrap [--project PATH] [--tool codex|copilot] [--no-net] [--no-tui] [--docker-publish HOST:GUEST] [--ro PATH] [--rw PATH] [--gh] [--aws PROFILE] [--reset] [-- EXTRA_ARGS...]"
    );
}

fn vmnet_gateway_config_from_args(args: &[String]) -> Result<VmnetRuntimeConfig, String> {
    let mut socket_path = None;
    let mut network = GuestNetwork::default();
    let mut allow_ips = Vec::new();
    let mut allow_domains = Vec::new();
    let mut allow_public = false;
    let mut no_net = false;
    let mut host_listeners = Vec::new();
    let mut tls_ca_cert = None;
    let mut tls_ca_key = None;
    let mut tls_generate_per_host_certs = false;
    let mut pcap_path = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--socket" => {
                socket_path = Some(PathBuf::from(value(args, &mut index, "--socket")?));
            }
            "--guest-ip" => network.guest_ip = value(args, &mut index, "--guest-ip")?,
            "--gateway-ip" => network.gateway_ip = value(args, &mut index, "--gateway-ip")?,
            "--dns-ip" => network.dns_ip = value(args, &mut index, "--dns-ip")?,
            "--guest-mac" => network.guest_mac = value(args, &mut index, "--guest-mac")?,
            "--prefix-len" => {
                network.prefix_len = value(args, &mut index, "--prefix-len")?
                    .parse()
                    .map_err(|_| "invalid --prefix-len".to_string())?;
            }
            "--allow-ip" => allow_ips.push(value(args, &mut index, "--allow-ip")?),
            "--allow-domain" => allow_domains.push(value(args, &mut index, "--allow-domain")?),
            "--allow-public-internet" => {
                allow_public = true;
            }
            "--no-net" => {
                no_net = true;
            }
            "--host-docker-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-docker-listener")?)?;
                host_listeners.push(HostListener::docker_api(host_port, guest_port));
            }
            "--host-payload-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-payload-listener")?)?;
                host_listeners.push(HostListener::payload_control(host_port, guest_port));
            }
            "--publish" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--publish")?)?;
                host_listeners.push(HostListener::published_tcp(host_port, guest_port));
            }
            "--pcap" => {
                pcap_path = Some(PathBuf::from(value(args, &mut index, "--pcap")?));
            }
            "--tls-ca-cert" => {
                tls_ca_cert = Some(PathBuf::from(value(args, &mut index, "--tls-ca-cert")?));
            }
            "--tls-ca-key" => {
                tls_ca_key = Some(PathBuf::from(value(args, &mut index, "--tls-ca-key")?));
            }
            "--tls-generate-per-host-certs" => {
                tls_generate_per_host_certs = true;
            }
            "-h" | "--help" => {
                print_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown vmnet-gateway option: {unknown}")),
        }
        index += 1;
    }

    let socket_path = socket_path.ok_or_else(|| "--socket is required".to_string())?;
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
    let mut config = PayloadClientConfig {
        host: "127.0.0.1".to_string(),
        port: 12076,
        ping: false,
        script: None,
        cwd: "/".to_string(),
        env: BTreeMap::new(),
        rows,
        cols,
        no_stdin: false,
    };

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--host" => config.host = value(args, &mut index, "--host")?,
            "--port" => {
                config.port = value(args, &mut index, "--port")?
                    .parse()
                    .map_err(|_| "invalid --port".to_string())?;
            }
            "--ping" => config.ping = true,
            "--script" => config.script = Some(value(args, &mut index, "--script")?),
            "--cwd" => config.cwd = value(args, &mut index, "--cwd")?,
            "--env" => {
                let env = value(args, &mut index, "--env")?;
                let (key, val) = env
                    .split_once('=')
                    .ok_or_else(|| "--env must be KEY=VALUE".to_string())?;
                if key.is_empty() {
                    return Err("--env key must not be empty".to_string());
                }
                config.env.insert(key.to_string(), val.to_string());
            }
            "--rows" => {
                config.rows = value(args, &mut index, "--rows")?
                    .parse()
                    .map_err(|_| "invalid --rows".to_string())?;
            }
            "--cols" => {
                config.cols = value(args, &mut index, "--cols")?
                    .parse()
                    .map_err(|_| "invalid --cols".to_string())?;
            }
            "--no-stdin" => config.no_stdin = true,
            "-h" | "--help" => {
                print_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown payload-client option: {unknown}")),
        }
        index += 1;
    }

    if !config.ping && config.script.is_none() {
        return Err("--script is required unless --ping is set".to_string());
    }

    Ok(config)
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
    tool_args: Vec<String>,
    gh: bool,
    aws_profile: Option<String>,
    extra_ro: Vec<PathBuf>,
    extra_rw: Vec<PathBuf>,
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
    let exit_code = run_payload_tcp_with_control(
        payload_addr,
        &request,
        None,
        &mut io::stdout(),
        PayloadControlOptions::disabled(),
    )
    .map_err(|error| format!("self-test payload failed: {error}"));
    running
        .terminate()
        .map_err(|error| format!("self-test shutdown failed: {error}"))?;
    let exit_code = exit_code?;
    if exit_code != 0 {
        return Err(format!("self-test payload exited with {exit_code}"));
    }
    println!("self-test: ok");
    Ok(())
}

fn self_test_config_from_args(args: &[String]) -> Result<SelfTestConfig, String> {
    let mut config = SelfTestConfig {
        project: env::current_dir().map_err(|error| error.to_string())?,
        run_dir: PathBuf::from(".sandbox/docker-vm/self-test"),
        artifact_manifest: PathBuf::from("docker/out/artifact-manifest.json"),
        qemu: PathBuf::from("qemu-system-x86_64"),
        image: "alpine:3.22".to_string(),
        publish_payload_port: None,
        no_net: false,
        hostile: false,
        tool: GuestTool::Codex,
    };

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--project" => {
                config.project = absolute_cli_path(&value(args, &mut index, "--project")?)?
            }
            "--run-dir" => config.run_dir = PathBuf::from(value(args, &mut index, "--run-dir")?),
            "--artifact-manifest" => {
                config.artifact_manifest =
                    PathBuf::from(value(args, &mut index, "--artifact-manifest")?)
            }
            "--qemu" => config.qemu = PathBuf::from(value(args, &mut index, "--qemu")?),
            "--image" => config.image = value(args, &mut index, "--image")?,
            "--publish-payload-port" => {
                config.publish_payload_port = Some(
                    value(args, &mut index, "--publish-payload-port")?
                        .parse()
                        .map_err(|_| "invalid --publish-payload-port".to_string())?,
                );
            }
            "--no-net" => config.no_net = true,
            "--hostile" => config.hostile = true,
            "--tool" => {
                config.tool = value(args, &mut index, "--tool")?
                    .parse()
                    .map_err(|error: String| error)?;
            }
            "-h" | "--help" => {
                print_self_test_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown self-test option: {unknown}")),
        }
        index += 1;
    }

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

fn self_test_payload_script(config: &FrontendConfig, image: &str, hostile: bool) -> String {
    let mut steps = vec![
        "set -eu".to_string(),
        "echo self-test: payload-start".to_string(),
        format!(
            "test \"$HOME\" = {}",
            shell_quote(&config.project.join(".sandbox/home").display().to_string())
        ),
        "test -d \"$HOME\"".to_string(),
        "test \"$PWD\" = \"$AGENTVM_SELF_TEST_PROJECT\"".to_string(),
        "test -f /run/agentvm-config/mitm-ca.crt".to_string(),
        "test ! -e /run/agentvm-config/mitm-ca.key".to_string(),
        "test -f /run/agentvm-ca-bundle.pem".to_string(),
        "test \"${NODE_EXTRA_CA_CERTS:-}\" = /run/agentvm-ca-bundle.pem".to_string(),
        "test \"${NPM_CONFIG_CAFILE:-}\" = /run/agentvm-ca-bundle.pem".to_string(),
        "if touch /run/agentvm-config/agentvm-self-test-ro 2>/tmp/agentvm-config-ro.err; then echo config-fs-write-unexpected; exit 1; fi".to_string(),
        "mkdir -p \"$HOME/.codex\"".to_string(),
        "printf state-ok > \"$HOME/.codex/agentvm-self-test-state\"".to_string(),
        "test \"$(cat \"$HOME/.codex/agentvm-self-test-state\")\" = state-ok".to_string(),
        "if [ \"${AGENTVM_SELF_TEST_NETWORK:-allow}\" = allow ]; then node -e 'const dns = require(\"dns\"); dns.lookup(\"example.com\", err => { if (err) throw err; });'; fi".to_string(),
        "printf workspace-ok > .agentvm-self-test-workspace".to_string(),
        "test \"$(cat .agentvm-self-test-workspace)\" = workspace-ok".to_string(),
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
    let mut project = PathBuf::from(".");
    let mut run_dir = PathBuf::from(".sandbox/docker-vm/run");
    let mut artifact_manifest = PathBuf::from("docker/out/artifact-manifest.json");
    let mut qemu = PathBuf::from("qemu-system-x86_64");
    let mut policy = PolicyArgs::default();
    let mut guest_http_smoke_url = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--project" => project = PathBuf::from(value(args, &mut index, "--project")?),
            "--run-dir" => run_dir = PathBuf::from(value(args, &mut index, "--run-dir")?),
            "--artifact-manifest" => {
                artifact_manifest = PathBuf::from(value(args, &mut index, "--artifact-manifest")?);
            }
            "--qemu" => qemu = PathBuf::from(value(args, &mut index, "--qemu")?),
            "--tool" => {
                policy.tool = Some(
                    value(args, &mut index, "--tool")?
                        .parse()
                        .map_err(|error: String| error)?,
                );
            }
            "--tool-arg" => {
                policy
                    .tool_args
                    .push(value(args, &mut index, "--tool-arg")?);
            }
            "--gh" => {
                policy.gh = true;
            }
            "--aws" => {
                policy.aws_profile = Some(value(args, &mut index, "--aws")?);
            }
            "--ro" => {
                policy
                    .extra_ro
                    .push(absolute_cli_path(&value(args, &mut index, "--ro")?)?);
            }
            "--rw" => {
                policy
                    .extra_rw
                    .push(absolute_cli_path(&value(args, &mut index, "--rw")?)?);
            }
            "--guest-http-smoke-url" => {
                guest_http_smoke_url = Some(value(args, &mut index, "--guest-http-smoke-url")?);
            }
            "--allow-ip" => policy
                .allow_ips
                .push(value(args, &mut index, "--allow-ip")?),
            "--allow-domain" => {
                policy
                    .allow_domains
                    .push(value(args, &mut index, "--allow-domain")?)
            }
            "--allow-public-internet" => policy.allow_public = true,
            "--no-net" => policy.no_net = true,
            "--qemu-timeout-seconds" => {
                let seconds = value(args, &mut index, "--qemu-timeout-seconds")?
                    .parse::<u64>()
                    .map_err(|_| "invalid --qemu-timeout-seconds".to_string())?;
                policy.qemu_timeout = Some(Duration::from_secs(seconds));
            }
            "--local-http-smoke-upstream" => {
                policy.local_http_smoke_upstream = Some(parse_ip_port(&value(
                    args,
                    &mut index,
                    "--local-http-smoke-upstream",
                )?)?);
            }
            "--host-docker-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-docker-listener")?)?;
                policy
                    .host_listeners
                    .push(HostListener::docker_api(host_port, guest_port));
            }
            "--host-payload-listener" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--host-payload-listener")?)?;
                policy
                    .host_listeners
                    .push(HostListener::payload_control(host_port, guest_port));
            }
            "--publish" => {
                let (host_port, guest_port) =
                    parse_port_pair(&value(args, &mut index, "--publish")?)?;
                policy
                    .host_listeners
                    .push(HostListener::published_tcp(host_port, guest_port));
            }
            "--pcap" => {
                policy.pcap_path = Some(PathBuf::from(value(args, &mut index, "--pcap")?));
            }
            "--payload-script" => {
                payload_launch_args(&mut policy).script =
                    value(args, &mut index, "--payload-script")?;
            }
            "--payload-cwd" => {
                payload_launch_args(&mut policy).cwd = value(args, &mut index, "--payload-cwd")?;
            }
            "--payload-env" => {
                let env = value(args, &mut index, "--payload-env")?;
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
            "--payload-rows" => {
                payload_launch_args(&mut policy).rows = value(args, &mut index, "--payload-rows")?
                    .parse()
                    .map_err(|_| "invalid --payload-rows".to_string())?;
            }
            "--payload-cols" => {
                payload_launch_args(&mut policy).cols = value(args, &mut index, "--payload-cols")?
                    .parse()
                    .map_err(|_| "invalid --payload-cols".to_string())?;
            }
            "--payload-no-stdin" => {
                payload_launch_args(&mut policy).no_stdin = true;
            }
            "--tls-ca-cert" => {
                policy.tls_ca_cert = Some(PathBuf::from(value(args, &mut index, "--tls-ca-cert")?));
            }
            "--tls-ca-key" => {
                policy.tls_ca_key = Some(PathBuf::from(value(args, &mut index, "--tls-ca-key")?));
            }
            "--tls-generate-per-host-certs" => {
                policy.tls_generate_per_host_certs = true;
            }
            "-h" | "--help" => {
                print_usage();
                return Err("help requested".to_string());
            }
            unknown => return Err(format!("unknown frontend option: {unknown}")),
        }
        index += 1;
    }

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
    if policy.tool.is_some() || policy.gh || policy.aws_profile.is_some() {
        std::fs::create_dir_all(config.project.join(".sandbox/home"))
            .map_err(|error| format!("failed to create guest HOME: {error}"))?;
    }
    let host_home = host_home_dir()?;
    Ok(guest_runtime_mounts(
        config.project.clone(),
        &GuestShareSpec {
            tool: policy.tool,
            tool_state: ToolStateMounts::from_guest_tool(policy.tool),
            host_home,
            gh: policy.gh,
            extra_ro: policy.extra_ro.clone(),
            extra_rw: policy.extra_rw.clone(),
        },
    ))
}

fn guest_payload_env(
    config: &FrontendConfig,
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
    if policy.tool.is_none() && !policy.gh && policy.aws_profile.is_none() {
        return Ok(env_vars);
    }
    let user = env::var("USER").unwrap_or_else(|_| "sandbox".to_string());
    let guest_home = config.project.join(".sandbox/home").display().to_string();
    env_vars.insert("HOME".to_string(), guest_home.clone());
    env_vars.insert("USER".to_string(), user.clone());
    env_vars.insert("LOGNAME".to_string(), user);
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
        "unix:///var/run/docker.sock".to_string(),
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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&byte))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
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

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
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
        let guest_home = root.join("repo/.sandbox/home").display().to_string();

        assert_eq!(env.get("HOME"), Some(&guest_home));
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
            Some("unix:///var/run/docker.sock")
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
                "--".to_string(),
                "--model".to_string(),
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
        let args =
            parse_wrapper_args_with_terminal("agentvm-frontend", &[], true, true).expect("wrapper");

        assert_eq!(args.ui_mode, WrapperUiMode::Tui);
        assert!(!args.tool_selected);
        assert!(!args.launch_args.contains(&"--tool".to_string()));
    }

    #[test]
    fn plain_wrap_without_tool_still_requires_explicit_tool() {
        let error = parse_wrapper_args_with_terminal("agentvm-frontend", &[], false, true)
            .expect_err("missing tool");

        assert_eq!(
            error,
            "no tool selected; use --tool codex|copilot or interactive TUI setup"
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
                "--".to_string(),
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

        let args =
            parse_wrapper_args_with_terminal("codex-wrap", &[], true, true).expect("wrapper args");
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
        assert!(script.contains("/run/agentvm-config/mitm-ca.crt"));
        assert!(script.contains("test ! -e /run/agentvm-config/mitm-ca.key"));
        assert!(script.contains("NODE_EXTRA_CA_CERTS"));
        assert!(script.contains("NPM_CONFIG_CAFILE"));
        assert!(script.contains("agentvm-config-ro"));
        assert!(script.contains("agentvm-self-test-state"));
        assert!(script.contains("dns.lookup"));
        assert!(script.contains(".agentvm-self-test-workspace"));
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

    fn unique_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agentvm-frontend-main-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn frontend_test_root() -> PathBuf {
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
