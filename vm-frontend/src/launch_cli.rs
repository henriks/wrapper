use super::*;

use agentvm_frontend::payload_client::ping_payload_async_tcp;

const DEFAULT_ARTIFACT_MANIFEST_RELATIVE: &str = "docker/out/artifact-manifest.json";

pub(crate) type LaunchCliResult<T> = Result<T, LaunchCliError>;

pub(crate) fn default_artifact_manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_ARTIFACT_MANIFEST_RELATIVE)
}

pub(crate) fn artifact_manifest_arg(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<String>("artifact_manifest")
        .map(PathBuf::from)
        .unwrap_or_else(default_artifact_manifest_path)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchCliError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    PayloadClient(#[from] PayloadClientError),
    #[error(transparent)]
    PayloadCli(#[from] PayloadCliError),
    #[error(transparent)]
    Appliance(#[from] appliance::ApplianceError),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("failed to load frontend config: {source}")]
    LoadFrontendConfig { source: LaunchError },
    #[error("invalid --qemu-timeout-seconds")]
    InvalidQemuTimeoutSeconds,
    #[error("--payload-env must be KEY=VALUE")]
    InvalidPayloadEnvFormat,
    #[error("--payload-env key must not be empty")]
    EmptyPayloadEnvKey,
    #[error("invalid --payload-rows")]
    InvalidPayloadRows,
    #[error("invalid --payload-cols")]
    InvalidPayloadCols,
    #[error("--payload-script must not be empty")]
    EmptyPayloadScript,
    #[error("share shadow parent must match a configured share: {parent}")]
    MissingShareShadowParent { parent: String },
    #[error("share shadow backing path must stay under project .sandbox: {path}")]
    ShareShadowBackingOutsideProject { path: String },
    #[error("failed to create share shadow backing dir {path}: {source}")]
    CreateShareShadowBackingDir { path: String, source: io::Error },
    #[error("failed to create VM runtime root: {source}")]
    CreateVmRuntimeRoot { source: io::Error },
    #[error("failed to open project VM lock {path}: {source}")]
    OpenProjectVmLock { path: String, source: io::Error },
    #[error("{message}")]
    ProjectVmBusy { message: String },
    #[error("failed to lock project VM state: {source}")]
    LockProjectVmState { source: io::Error },
    #[error("failed to remove .sandbox: {source}")]
    RemoveSandbox { source: io::Error },
    #[error("failed to bind payload control listener on {host_addr}:0: {source}")]
    BindPayloadControlPort {
        host_addr: String,
        source: io::Error,
    },
    #[error("failed to inspect payload control listener address: {source}")]
    PayloadControlPortAddr { source: io::Error },
    #[error("failed to bind local HTTP smoke upstream: {source}")]
    BindLocalHttpSmokeUpstream { source: io::Error },
    #[error("failed to inspect local HTTP smoke upstream address: {source}")]
    LocalHttpSmokeUpstreamAddr { source: io::Error },
    #[error("failed to spawn local HTTP smoke upstream thread: {source}")]
    SpawnLocalHttpSmokeUpstream { source: io::Error },
    #[error("--no-net cannot be combined with egress allow options")]
    NoNetWithEgressAllow,
    #[error("invalid --allow-ip: {source}")]
    InvalidAllowIpRange { source: Ipv4RangeParseError },
    #[error("--no-net cannot be combined with --publish")]
    NoNetWithPublish,
}

impl From<LaunchCliError> for String {
    fn from(error: LaunchCliError) -> Self {
        error.to_string()
    }
}

impl From<String> for LaunchCliError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for LaunchCliError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

pub(crate) async fn run_launch_async(
    args: &[String],
    ui_mode: WrapperUiMode,
) -> LaunchCliResult<()> {
    let request = frontend_launch_request_from_args(args)?;
    run_launch_request_async(request, ui_mode).await
}

pub(crate) async fn run_launch_request_async(
    request: FrontendLaunchRequest,
    ui_mode: WrapperUiMode,
) -> LaunchCliResult<()> {
    let (config, policy_args) = frontend_config_from_launch_request(&request)?;
    let payload = launch_payload_args(&config, &policy_args)?;
    if let Some(payload) = payload {
        return run_payload_launch_request_async(config, policy_args, payload, ui_mode).await;
    }
    let span = tracing::info_span!(
        "launch.cli.async",
        project = %config.project.display(),
        run_dir = %config.runtime.run_dir.display(),
        ui_mode = ?ui_mode,
    );
    let _span_guard = span.enter();
    tracing::info!("launch request configured for async run");
    let artifacts = frontend_artifact_summary(&config);
    let _lock = ProjectLock::acquire(&config)?;
    let qemu_timeout = policy_args.qemu_timeout;
    let local_http_smoke_upstream = policy_args.local_http_smoke_upstream;
    let mounts = runtime_mounts(&config, &policy_args)?;
    let policy = policy_from_args(config.network.clone(), policy_args)?;
    let mut config = config;
    if let Some(destination) = local_http_smoke_upstream {
        tracing::info!(?destination, "starting local HTTP smoke upstream");
        config
            .upstream_mappings
            .push(start_local_http_smoke_upstream(destination)?);
    }
    tracing::info!(
        qemu_timeout_seconds = qemu_timeout.map(|timeout| timeout.as_secs()),
        "starting frontend without payload client through async supervisor"
    );
    println!("launch: phase=starting-frontend");
    let qemu_exit = run_frontend_until_qemu_exit_with_policy_and_timeout_async(
        config.clone(),
        mounts,
        policy,
        qemu_timeout,
    )
    .await
    .map_err(|error| format!("launch failed: {error}\n{artifacts}"))?;
    handle_qemu_exit(qemu_exit, qemu_timeout, &artifacts)
}

async fn run_payload_launch_request_async(
    config: FrontendConfig,
    policy_args: PolicyArgs,
    payload: PayloadLaunchArgs,
    ui_mode: WrapperUiMode,
) -> LaunchCliResult<()> {
    let span = tracing::info_span!(
        "launch.cli.async.payload",
        project = %config.project.display(),
        run_dir = %config.runtime.run_dir.display(),
        ui_mode = ?ui_mode,
    );
    let _span_guard = span.enter();
    tracing::info!("payload launch configured for async supervisor run");
    let artifacts = frontend_artifact_summary(&config);
    let _lock = ProjectLock::acquire(&config)?;
    let qemu_timeout = policy_args.qemu_timeout;
    let local_http_smoke_upstream = policy_args.local_http_smoke_upstream;
    let mounts = runtime_mounts(&config, &policy_args)?;
    let guest_env = guest_payload_env(&config, &policy_args)?;
    let mut policy = policy_from_args(config.network.clone(), policy_args)?;
    let mut config = config;
    if let Some(destination) = local_http_smoke_upstream {
        tracing::info!(?destination, "starting local HTTP smoke upstream");
        config
            .upstream_mappings
            .push(start_local_http_smoke_upstream(destination)?);
    }
    let mut payload_listener = ensure_payload_listener(&mut policy)?;
    let host_port = payload_listener.host_port();
    tracing::info!(host_port, "starting async frontend for payload launch");
    println!("launch: phase=starting-frontend");
    let control_client = SupervisorControlClient::for_runtime(&config.runtime);
    let launch_config = config.clone();
    let reserved_host_ports = payload_listener.take_listener().into_iter().collect();
    let mut launch_task = tokio::spawn(async move {
        run_frontend_until_qemu_exit_with_policy_and_timeout_reserving_host_ports_async(
            launch_config,
            mounts,
            policy,
            qemu_timeout,
            reserved_host_ports,
        )
        .await
    });

    let endpoint = wait_for_supervisor_payload_endpoint(&control_client, Duration::from_secs(30));
    let addr = tokio::select! {
        endpoint = endpoint => match endpoint {
            Ok(addr) => addr,
            Err(error) => {
                let _ = control_client
                    .request_shutdown("payload control discovery failed")
                    .await;
                let _ = launch_task.await;
                return Err(
                    format!("launch payload control discovery failed: {error}\n{artifacts}").into(),
                );
            }
        },
        launch = &mut launch_task => {
            let launch_result = launch.map_err(|error| {
                LaunchCliError::Message(format!("async launch task failed: {error}"))
            })?;
            return match launch_result {
                Ok(qemu_exit) => Err(format!(
                    "launch failed: qemu exited before payload endpoint was published: status={} timed_out={}\n{artifacts}",
                    qemu_exit.status,
                    qemu_exit.timed_out,
                ).into()),
                Err(error) => Err(format!("launch failed: {error}\n{artifacts}").into()),
            };
        }
    };
    println!("launch: phase=waiting-for-payload-ready timeout=120s");
    let readiness_task = tokio::spawn(wait_for_payload_ready_async(addr, Duration::from_secs(120)));
    let readiness = tokio::select! {
        readiness = readiness_task => readiness,
        launch = &mut launch_task => {
            let launch_result = launch.map_err(|error| {
                LaunchCliError::Message(format!("async launch task failed: {error}"))
            })?;
            return match launch_result {
                Ok(qemu_exit) => Err(format!(
                    "launch failed: qemu exited before payload ready: status={} timed_out={}\n{artifacts}",
                    qemu_exit.status,
                    qemu_exit.timed_out,
                ).into()),
                Err(error) => Err(format!("launch failed: {error}\n{artifacts}").into()),
            };
        }
    };
    if let Err(error) = readiness.map_err(|error| {
        LaunchCliError::Message(format!("payload readiness task failed: {error}"))
    })? {
        let _ = control_client
            .request_shutdown("payload readiness failed")
            .await;
        let _ = launch_task.await;
        return Err(format!("launch payload readiness failed: {error}\n{artifacts}").into());
    }
    tracing::info!(%addr, "payload listener ready through supervisor control");
    let request = PayloadRequest {
        script: payload.script,
        cwd: payload.cwd,
        env: merged_payload_env(guest_env, payload.env),
        rows: payload.rows,
        cols: payload.cols,
    };
    let payload_result = match ui_mode {
        WrapperUiMode::Plain => {
            let input = (!payload.no_stdin).then(|| {
                Box::new(tokio::io::stdin()) as Box<dyn tokio::io::AsyncRead + Send + Unpin>
            });
            let mut output = tokio::io::stdout();
            run_payload_tcp_async_with_control(
                addr,
                &request,
                input,
                &mut output,
                PayloadControlOptions::interactive(),
            )
            .await
        }
        WrapperUiMode::Tui => {
            tracing::info!("running payload in TUI viewport through supervisor endpoint");
            tui::run_payload_viewport(addr, &request).await
        }
    };

    if let Err(error) = flush_guest_filesystems(addr).await {
        tracing::warn!(%error, "failed to flush guest filesystem before VM shutdown");
        eprintln!("warning: failed to flush guest filesystem before VM shutdown: {error}");
    }
    if let Err(error) = control_client.request_shutdown("payload finished").await {
        tracing::warn!(%error, "failed to request supervisor shutdown after payload");
        eprintln!("warning: failed to request supervisor shutdown after payload: {error}");
    }
    let launch_result = launch_task
        .await
        .map_err(|error| LaunchCliError::Message(format!("async launch task failed: {error}")))?;
    if let Err(error) = launch_result {
        return Err(format!("launch failed: {error}\n{artifacts}").into());
    }
    let exit_code = payload_result?.into_exit_code()?;
    tracing::info!(exit_code, "payload finished");
    std::process::exit(payload_exit_status(exit_code));
}

async fn wait_for_supervisor_payload_endpoint(
    client: &SupervisorControlClient,
    timeout: Duration,
) -> LaunchCliResult<std::net::SocketAddr> {
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    loop {
        match client.payload_control_endpoint().await {
            Ok(Some(endpoint)) => {
                return socket_addr(&endpoint.host_addr, endpoint.host_port).map_err(Into::into)
            }
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
        if Instant::now() >= deadline {
            let detail = last_error
                .map(|error| format!("; last control error: {error}"))
                .unwrap_or_default();
            return Err(LaunchCliError::Message(format!(
                "timed out waiting for supervisor payload endpoint{detail}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn handle_qemu_exit(
    qemu_exit: QemuExit,
    qemu_timeout: Option<Duration>,
    artifacts: &str,
) -> LaunchCliResult<()> {
    if qemu_exit.status.success() {
        tracing::info!(status = %qemu_exit.status, "qemu completed successfully");
        Ok(())
    } else if qemu_exit.timed_out {
        tracing::warn!(status = %qemu_exit.status, "qemu timed out");
        Err(LaunchCliError::Message(format!(
            "qemu timed out after {} seconds and was terminated with status: {}\n{}",
            qemu_timeout.map_or(0, |timeout| timeout.as_secs()),
            qemu_exit.status,
            artifacts
        )))
    } else {
        tracing::warn!(status = %qemu_exit.status, "qemu exited unsuccessfully");
        Err(LaunchCliError::Message(format!(
            "qemu exited with status: {}\n{}",
            qemu_exit.status, artifacts
        )))
    }
}

pub(crate) fn frontend_artifact_summary(config: &FrontendConfig) -> String {
    format!(
        "artifacts: run_dir={} state={} state_disk={} qemu_log={} console_log={} vmnet_event_log={}",
        config.runtime.run_dir.display(),
        config.runtime.state_json.display(),
        config.runtime.state_disk.display(),
        config.runtime.run_dir.join("qemu.log").display(),
        config.runtime.console_log.display(),
        config.runtime.vmnet_event_log.display()
    )
}

pub(crate) struct ProjectLock {
    file: File,
}

impl ProjectLock {
    pub(crate) fn acquire(config: &FrontendConfig) -> LaunchCliResult<Self> {
        if let Some(parent) = config.runtime.lock.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| LaunchCliError::CreateVmRuntimeRoot { source })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&config.runtime.lock)
            .map_err(|source| LaunchCliError::OpenProjectVmLock {
                path: config.runtime.lock.display().to_string(),
                source,
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

pub(crate) fn reset_project(project: &PathBuf) -> LaunchCliResult<()> {
    let project = if project.is_absolute() {
        project.clone()
    } else {
        absolute_cli_path(&project.display().to_string())?
    };
    let sandbox = project.join(".sandbox");
    let runtime = RuntimePaths::under(project.join(".sandbox/docker-vm/run"));
    if let Some(parent) = runtime.lock.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| LaunchCliError::CreateVmRuntimeRoot { source })?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&runtime.lock)
        .map_err(|source| LaunchCliError::OpenProjectVmLock {
            path: runtime.lock.display().to_string(),
            source,
        })?;
    acquire_project_file_lock(
        &file,
        format!(
            "--reset refused because a VM sandbox is active for this project ({})",
            runtime.lock.display()
        ),
    )?;
    if sandbox.exists() {
        fs::remove_dir_all(&sandbox).map_err(|source| LaunchCliError::RemoveSandbox { source })?;
        println!("Removed .sandbox/");
    } else {
        println!(".sandbox/ does not exist, nothing to reset");
    }
    Ok(())
}

fn acquire_project_file_lock(file: &File, busy_message: String) -> LaunchCliResult<()> {
    match fs4::FileExt::try_lock(file) {
        Ok(()) => Ok(()),
        Err(fs4::TryLockError::WouldBlock) => Err(LaunchCliError::ProjectVmBusy {
            message: busy_message,
        }),
        Err(fs4::TryLockError::Error(source)) => Err(LaunchCliError::LockProjectVmState { source }),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PolicyArgs {
    pub(crate) allow_ips: Vec<String>,
    pub(crate) allow_domains: Vec<String>,
    pub(crate) allow_public: bool,
    pub(crate) no_net: bool,
    pub(crate) qemu_timeout: Option<Duration>,
    pub(crate) local_http_smoke_upstream: Option<(Ipv4Addr, u16)>,
    pub(crate) host_listeners: Vec<HostListener>,
    pub(crate) tls_ca_cert: Option<PathBuf>,
    pub(crate) tls_ca_key: Option<PathBuf>,
    pub(crate) tls_generate_per_host_certs: bool,
    pub(crate) pcap_path: Option<PathBuf>,
    pub(crate) payload: Option<PayloadLaunchArgs>,
    pub(crate) gh: bool,
    pub(crate) aws_profile: Option<String>,
    pub(crate) extra_ro: Vec<PathBuf>,
    pub(crate) extra_rw: Vec<PathBuf>,
    pub(crate) extra_shares: Vec<GuestPathShare>,
    pub(crate) extra_share_shadows: Vec<GuestPathShareShadow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PayloadLaunchArgs {
    pub(crate) script: String,
    pub(crate) cwd: String,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) no_stdin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuestPathShare {
    pub(crate) host_path: PathBuf,
    pub(crate) guest_path: PathBuf,
    pub(crate) readonly: bool,
    pub(crate) required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuestPathShareShadow {
    pub(crate) parent_guest_path: PathBuf,
    pub(crate) relative_path: PathBuf,
    pub(crate) backing_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct FrontendLaunchRequest {
    pub(crate) project: PathBuf,
    pub(crate) run_dir: PathBuf,
    pub(crate) artifact_manifest: PathBuf,
    pub(crate) qemu: PathBuf,
    pub(crate) guest_http_smoke_url: Option<String>,
    pub(crate) guest_log_dir: Option<PathBuf>,
    pub(crate) policy: PolicyArgs,
}

impl FrontendLaunchRequest {
    pub(crate) fn for_project(project: PathBuf) -> Self {
        Self {
            project: project.clone(),
            run_dir: project.join(".sandbox/docker-vm/run"),
            artifact_manifest: default_artifact_manifest_path(),
            qemu: PathBuf::from("qemu-system-x86_64"),
            guest_http_smoke_url: None,
            guest_log_dir: None,
            policy: PolicyArgs::default(),
        }
    }
}

pub(crate) fn frontend_launch_request_from_args(
    args: &[String],
) -> LaunchCliResult<FrontendLaunchRequest> {
    let matches = parse_clap_matches(frontend_clap_command(), args)?;
    let mut project = matches
        .get_one::<String>("project")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut run_dir = matches
        .get_one::<String>("run_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".sandbox/docker-vm/run"));
    let artifact_manifest = artifact_manifest_arg(&matches);
    let qemu = matches
        .get_one::<String>("qemu")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("qemu-system-x86_64"));
    let mut policy = PolicyArgs::default();
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
    let mirror_guest_logs = matches.get_flag("mirror_guest_logs");
    policy.allow_ips = append_many(&matches, "allow_ip");
    policy.allow_domains = append_many(&matches, "allow_domain");
    policy.allow_public = matches.get_flag("allow_public_internet");
    policy.no_net = matches.get_flag("no_net");
    if let Some(seconds) = matches.get_one::<String>("qemu_timeout_seconds") {
        policy.qemu_timeout = Some(Duration::from_secs(
            seconds
                .parse::<u64>()
                .map_err(|_| LaunchCliError::InvalidQemuTimeoutSeconds)?,
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
            .ok_or(LaunchCliError::InvalidPayloadEnvFormat)?;
        if key.is_empty() {
            return Err(LaunchCliError::EmptyPayloadEnvKey);
        }
        payload_launch_args(&mut policy)
            .env
            .insert(key.to_string(), val.to_string());
    }
    if let Some(rows) = matches.get_one::<String>("payload_rows") {
        payload_launch_args(&mut policy).rows = rows
            .parse()
            .map_err(|_| LaunchCliError::InvalidPayloadRows)?;
    }
    if let Some(cols) = matches.get_one::<String>("payload_cols") {
        payload_launch_args(&mut policy).cols = cols
            .parse()
            .map_err(|_| LaunchCliError::InvalidPayloadCols)?;
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

    validate_launch_policy_args(&policy)?;
    let guest_log_dir = mirror_guest_logs.then(|| project.join(".vmlogs"));

    Ok(FrontendLaunchRequest {
        project,
        run_dir,
        artifact_manifest,
        qemu,
        guest_http_smoke_url,
        guest_log_dir,
        policy,
    })
}

pub(crate) fn frontend_config_from_args(
    args: &[String],
) -> LaunchCliResult<(FrontendConfig, PolicyArgs)> {
    let request = frontend_launch_request_from_args(args)?;
    frontend_config_from_launch_request(&request)
}

pub(crate) fn frontend_config_from_launch_request(
    request: &FrontendLaunchRequest,
) -> LaunchCliResult<(FrontendConfig, PolicyArgs)> {
    let mut config = FrontendConfig::from_artifact_manifest_file(
        request.project.clone(),
        request.run_dir.clone(),
        request.qemu.clone(),
        &request.artifact_manifest,
    )
    .map_err(|source| LaunchCliError::LoadFrontendConfig { source })?;
    config.guest_http_smoke_url = request.guest_http_smoke_url.clone();
    config.guest_log_dir = request
        .guest_log_dir
        .as_ref()
        .map(|path| path.display().to_string());
    validate_launch_policy_args(&request.policy)?;
    if config.guest_http_smoke_url.is_some() {
        ensure_appliance_sources_fresh(&request.artifact_manifest)?;
    }
    Ok((config, request.policy.clone()))
}

fn validate_launch_policy_args(policy: &PolicyArgs) -> LaunchCliResult<()> {
    validate_no_net_args(
        policy.no_net,
        policy.allow_public,
        &policy.allow_ips,
        &policy.allow_domains,
        &policy.host_listeners,
    )?;
    if policy
        .payload
        .as_ref()
        .is_some_and(|payload| payload.script.is_empty())
    {
        return Err(LaunchCliError::EmptyPayloadScript);
    }
    Ok(())
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
            Arg::new("mirror_guest_logs")
                .long("mirror-guest-logs")
                .action(ArgAction::SetTrue),
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
        cwd: String::new(),
        env: BTreeMap::new(),
        rows,
        cols,
        no_stdin: false,
    })
}

pub(crate) fn launch_payload_args(
    config: &FrontendConfig,
    policy: &PolicyArgs,
) -> LaunchCliResult<Option<PayloadLaunchArgs>> {
    let mut payload = match policy.payload.clone() {
        Some(payload) => payload,
        None => return Ok(None),
    };
    if payload.cwd.is_empty() {
        payload.cwd = config.project.display().to_string();
    }
    Ok(Some(payload))
}

pub(crate) fn runtime_mounts(
    config: &FrontendConfig,
    policy: &PolicyArgs,
) -> LaunchCliResult<Vec<RuntimeMount>> {
    let host_home = host_home_dir()?;
    let mut mounts = guest_runtime_mounts(
        config.project.clone(),
        &GuestShareSpec {
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
            .find(|share| share.guest_path == shadow.parent_guest_path)
            .ok_or_else(|| LaunchCliError::MissingShareShadowParent {
                parent: shadow.parent_guest_path.display().to_string(),
            })?;
        if !shadow_backing_is_project_local(config, &shadow.backing_path) {
            return Err(LaunchCliError::ShareShadowBackingOutsideProject {
                path: shadow.backing_path.display().to_string(),
            });
        }
        let guest_path = parent.guest_path.join(&shadow.relative_path);
        std::fs::create_dir_all(&shadow.backing_path).map_err(|source| {
            LaunchCliError::CreateShareShadowBackingDir {
                path: shadow.backing_path.display().to_string(),
                source,
            }
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
    let root = config.project.join(".sandbox/root");
    backing_path == root || backing_path.starts_with(&root)
}

pub(crate) fn guest_payload_env(
    _config: &FrontendConfig,
    policy: &PolicyArgs,
) -> LaunchCliResult<BTreeMap<String, String>> {
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

fn parse_guest_path_share(value: &str, readonly: bool) -> LaunchCliResult<GuestPathShare> {
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
        Some(value) => return Err(format!("invalid share requirement: {value}").into()),
    };
    Ok(GuestPathShare {
        host_path: absolute_cli_path(host)?,
        guest_path: absolute_cli_path(guest)?,
        readonly,
        required,
    })
}

fn parse_guest_path_share_shadow(value: &str) -> LaunchCliResult<GuestPathShareShadow> {
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
) -> LaunchCliResult<UpstreamMapping> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|source| LaunchCliError::BindLocalHttpSmokeUpstream { source })?;
    let host_port = listener
        .local_addr()
        .map_err(|source| LaunchCliError::LocalHttpSmokeUpstreamAddr { source })?
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
        .map_err(|source| LaunchCliError::SpawnLocalHttpSmokeUpstream { source })?;
    Ok(UpstreamMapping {
        guest_ip: destination.0,
        guest_port: destination.1,
        host_ip: Ipv4Addr::LOCALHOST,
        host_port,
    })
}

pub(crate) fn policy_from_args(
    network: GuestNetwork,
    args: PolicyArgs,
) -> LaunchCliResult<VmnetPolicy> {
    let mut policy = VmnetPolicy::default_sandbox(network);
    policy.egress.allow_ip_ranges =
        agentvm_frontend::network_policy::parse_ipv4_ranges(&args.allow_ips)
            .map_err(|source| LaunchCliError::InvalidAllowIpRange { source })?;
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
    Ok(policy)
}

pub(crate) struct PayloadListenerReservation {
    host_port: u16,
    listener: Option<TcpListener>,
}

impl PayloadListenerReservation {
    pub(crate) fn host_port(&self) -> u16 {
        self.host_port
    }

    pub(crate) fn take_listener(&mut self) -> Option<TcpListener> {
        self.listener.take()
    }
}

pub(crate) fn ensure_payload_listener(
    policy: &mut VmnetPolicy,
) -> LaunchCliResult<PayloadListenerReservation> {
    if let Some(listener) = policy
        .host_listeners
        .iter()
        .find(|listener| listener.purpose == HostListenerPurpose::PayloadControl)
    {
        return Ok(PayloadListenerReservation {
            host_port: listener.host_port,
            listener: None,
        });
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|source| {
        LaunchCliError::BindPayloadControlPort {
            host_addr: Ipv4Addr::LOCALHOST.to_string(),
            source,
        }
    })?;
    let host_port = listener
        .local_addr()
        .map_err(|source| LaunchCliError::PayloadControlPortAddr { source })?
        .port();
    const GUEST_PAYLOAD_PORT: u16 = 1076;
    policy
        .host_listeners
        .push(HostListener::payload_control(host_port, GUEST_PAYLOAD_PORT));
    Ok(PayloadListenerReservation {
        host_port,
        listener: Some(listener),
    })
}

pub(crate) async fn wait_for_payload_ready_async(
    addr: std::net::SocketAddr,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match ping_payload_async_tcp(addr).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "timed out waiting for guest payload control path after {} seconds; last error: {error}",
                        timeout.as_secs()
                    ));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(test)]
pub(crate) fn wait_for_payload_ready_with_probe(
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

pub(crate) fn validate_no_net_args(
    no_net: bool,
    allow_public: bool,
    allow_ips: &[String],
    allow_domains: &[String],
    host_listeners: &[HostListener],
) -> LaunchCliResult<()> {
    if !no_net {
        return Ok(());
    }
    if allow_public || !allow_ips.is_empty() || !allow_domains.is_empty() {
        return Err(LaunchCliError::NoNetWithEgressAllow);
    }
    if host_listeners
        .iter()
        .any(|listener| listener.purpose == HostListenerPurpose::PublishedTcp)
    {
        return Err(LaunchCliError::NoNetWithPublish);
    }
    Ok(())
}
