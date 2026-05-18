use super::*;

pub(crate) type WrapperResult<T> = Result<T, WrapperError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum WrapperError {
    #[error("{0}")]
    Parse(String),
    #[error(transparent)]
    Reset(LaunchCliError),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("{0}")]
    Tui(String),
    #[error(transparent)]
    TlsBootstrap(#[from] tls_bootstrap::TlsBootstrapError),
    #[error(transparent)]
    Launch(#[from] LaunchCliError),
    #[error("{0}")]
    Path(String),
    #[error("share shadow guest path must be absolute")]
    ShareShadowGuestPathMustBeAbsolute,
    #[error("share shadow guest path must stay under guest root")]
    EscapingShareShadowGuestPath,
}

pub(crate) async fn run_wrapper_async(args: Vec<String>) -> WrapperResult<()> {
    let Some((launch, ui_mode)) = prepare_wrapper_launch(args)? else {
        return Ok(());
    };
    run_launch_request_async(launch, ui_mode)
        .await
        .map_err(Into::into)
}

fn prepare_wrapper_launch(
    args: Vec<String>,
) -> WrapperResult<Option<(FrontendLaunchRequest, WrapperUiMode)>> {
    let mut wrapper = parse_wrapper_args(&args).map_err(WrapperError::Parse)?;
    if wrapper.help {
        return Ok(None);
    }
    if wrapper.reset {
        reset_project(&wrapper.project).map_err(WrapperError::Reset)?;
        return Ok(None);
    }
    if let Some(setup_tool) = wrapper.setup_tool {
        write_wrapper_sandbox_config(
            &wrapper.project,
            &WrapperSandboxConfig::setup_tool(setup_tool)?,
        )?;
        write_setup_tool_mise_config(&wrapper.project, setup_tool)?;
    }
    if wrapper.edit_config {
        let config = match read_wrapper_sandbox_config(&wrapper.project)? {
            Some(config) => config,
            None => WrapperSandboxConfig::codex_default()?,
        };
        let edited =
            tui::run_config_editor(config).map_err(|error| WrapperError::Tui(error.to_string()))?;
        write_wrapper_sandbox_config(&wrapper.project, &edited)?;
        return Ok(None);
    }
    if wrapper.tls_bootstrap {
        let ca = ensure_wrapper_mitm_ca(&wrapper.project)?;
        wrapper.launch.policy.tls_ca_cert = Some(ca.cert);
        wrapper.launch.policy.tls_ca_key = Some(ca.key);
        wrapper.launch.policy.tls_generate_per_host_certs = true;
    }
    Ok(Some((wrapper.launch, wrapper.ui_mode)))
}

#[derive(Debug)]
pub(crate) struct WrapperArgs {
    pub(crate) project: PathBuf,
    pub(crate) launch: FrontendLaunchRequest,
    pub(crate) ui_mode: WrapperUiMode,
    pub(crate) setup_tool: Option<SetupTool>,
    pub(crate) tls_bootstrap: bool,
    pub(crate) reset: bool,
    pub(crate) edit_config: bool,
    pub(crate) help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WrapperUiMode {
    Tui,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WrapperCommandOverride {
    Argv(ConfigCommand),
}

impl WrapperCommandOverride {
    pub(crate) fn script(&self) -> String {
        match self {
            Self::Argv(command) => payload_script_from_config_command(command),
        }
    }
}

pub(crate) fn parse_wrapper_args(args: &[String]) -> Result<WrapperArgs, String> {
    parse_wrapper_args_with_terminal(args, io::stdin().is_terminal(), io::stdout().is_terminal())
}

pub(crate) fn parse_wrapper_args_with_terminal(
    args: &[String],
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> Result<WrapperArgs, String> {
    let separator_index = args.iter().position(|arg| arg == "--");
    let (parse_args, payload_command_values) = if let Some(index) = separator_index {
        (&args[..index], args[index + 1..].to_vec())
    } else {
        (args, Vec::new())
    };

    let mut argv = vec!["agentvm".to_string()];
    argv.extend(parse_args.iter().cloned());
    let matches = match wrapper_clap_command().try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            error.print().map_err(|error| error.to_string())?;
            let project = env::current_dir().map_err(|error| error.to_string())?;
            let launch = FrontendLaunchRequest::for_project(project.clone());
            return Ok(WrapperArgs {
                launch,
                project,
                ui_mode: wrapper_ui_mode(false, stdin_is_tty, stdout_is_tty),
                setup_tool: None,
                tls_bootstrap: false,
                reset: false,
                edit_config: false,
                help: true,
            });
        }
        Err(error) => return Err(error.to_string().trim().to_string()),
    };

    let mut project = matches
        .get_one::<PathBuf>("project")
        .map(|path| absolute_cli_path(&path.display().to_string()))
        .transpose()?
        .unwrap_or(env::current_dir().map_err(|error| error.to_string())?);
    let mut launch = FrontendLaunchRequest::for_project(project.clone());
    let mut command_override: Option<WrapperCommandOverride> = None;
    let setup_tool = matches
        .get_one::<String>("setup_tool")
        .map(|tool| SetupTool::parse(tool))
        .transpose()?;
    let reset = matches.get_flag("reset");
    let help = false;
    let no_net = matches.get_flag("no_net");
    let no_tui = matches.get_flag("no_tui");
    let mirror_guest_logs = matches.get_flag("mirror_guest_logs");
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
        launch.project = project.clone();
        launch.run_dir = project.join(".sandbox/docker-vm/run");
    }
    if no_net {
        launch.policy.no_net = true;
    }
    if let Some(values) = matches.get_many::<String>("allow_ip") {
        launch.policy.allow_ips.extend(values.cloned());
    }
    if let Some(values) = matches.get_many::<String>("allow_domain") {
        launch.policy.allow_domains.extend(values.cloned());
    }
    if matches.get_flag("gh") {
        launch.policy.gh = true;
    }
    if let Some(value) = matches.get_one::<String>("aws") {
        launch.policy.aws_profile = Some(value.clone());
    }
    if let Some(values) = matches.get_many::<String>("ro") {
        for value in values {
            launch.policy.extra_ro.push(absolute_cli_path(value)?);
        }
    }
    if let Some(values) = matches.get_many::<String>("rw") {
        for value in values {
            launch.policy.extra_rw.push(absolute_cli_path(value)?);
        }
    }
    if let Some(value) = matches.get_one::<String>("qemu") {
        launch.qemu = PathBuf::from(value);
    }
    if let Some(value) = matches.get_one::<String>("artifact_manifest") {
        launch.artifact_manifest = PathBuf::from(value);
    }
    if mirror_guest_logs {
        launch.guest_log_dir = Some(project.join(".vmlogs"));
    }
    if let Some(values) = matches.get_many::<PortPair>("docker_publish") {
        for value in values {
            launch
                .policy
                .host_listeners
                .push(HostListener::published_tcp(value.host, value.guest));
        }
    }
    if help {
        return Ok(WrapperArgs {
            project,
            launch,
            ui_mode: wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty),
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
            launch,
            ui_mode: wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty),
            setup_tool,
            tls_bootstrap,
            reset,
            edit_config,
            help,
        });
    }
    let ui_mode = wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty);
    let sandbox_config = if let Some(setup_tool) = setup_tool {
        Some(WrapperSandboxConfig::setup_tool(setup_tool)?)
    } else {
        read_wrapper_sandbox_config(&project)?
    };
    if let Some(config) = sandbox_config.as_ref() {
        apply_configured_launch_defaults(
            &mut launch,
            config,
            &project,
            saw_network_override,
            no_net,
        )
        .map_err(|error| error.to_string())?;
    }
    if let Some(tool) = setup_tool {
        let final_script = if let Some(command) = command_override.as_ref() {
            command.script()
        } else {
            payload_script_from_config_command(
                &sandbox_config
                    .as_ref()
                    .expect("setup tool config is present")
                    .default_command,
            )
        };
        apply_payload_script(
            &mut launch,
            setup_tool_mise_exec_script(&project, tool, final_script),
        );
    } else if let Some(command) = command_override.as_ref() {
        apply_wrapper_command_override(&mut launch, command);
    } else if let Some(config) = sandbox_config.as_ref() {
        apply_configured_default_command(&mut launch, config);
    } else {
        apply_default_bash_payload(&mut launch);
    }
    if setup_tool.is_none() && project_mise_config_path(&project).exists() {
        wrap_payload_script_with_project_mise(&mut launch, &project);
    }
    if !no_net
        && !launch.policy.no_net
        && launch.policy.allow_ips.is_empty()
        && launch.policy.allow_domains.is_empty()
        && !launch.policy.allow_public
    {
        launch.policy.allow_public = true;
        tls_bootstrap = true;
    } else if launch.policy.allow_public
        || !launch.policy.allow_ips.is_empty()
        || !launch.policy.allow_domains.is_empty()
    {
        tls_bootstrap = true;
    }
    Ok(WrapperArgs {
        project,
        launch,
        ui_mode,
        setup_tool,
        tls_bootstrap,
        reset,
        edit_config,
        help,
    })
}

fn wrapper_clap_command() -> ClapCommand {
    ClapCommand::new("agentvm")
        .after_help("Command override: agentvm -- COMMAND [ARG...]")
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
        .arg(
            Arg::new("mirror_guest_logs")
                .long("mirror-guest-logs")
                .action(ArgAction::SetTrue),
        )
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

pub(crate) fn apply_configured_launch_defaults(
    launch: &mut FrontendLaunchRequest,
    config: &WrapperSandboxConfig,
    project: &Path,
    saw_network_override: bool,
    cli_no_net: bool,
) -> WrapperResult<()> {
    if !saw_network_override {
        match config.network.mode {
            ConfigNetworkMode::Public => {
                launch.policy.allow_public = true;
            }
            ConfigNetworkMode::None => {
                launch.policy.no_net = true;
            }
            ConfigNetworkMode::Allowlist => {
                launch
                    .policy
                    .allow_domains
                    .extend(config.network.allowed_domains.iter().cloned());
                launch
                    .policy
                    .allow_domains
                    .extend(config.network.allowed_hosts.iter().cloned());
                launch
                    .policy
                    .allow_ips
                    .extend(config.network.allowed_ips.iter().cloned());
            }
        }
    }
    if config.auth.github {
        launch.policy.gh = true;
    }
    if let Some(profile) = &config.auth.aws_profile {
        if launch.policy.aws_profile.is_none() {
            launch.policy.aws_profile = Some(profile.clone());
        }
    }
    for share in &config.shares {
        let host = absolute_cli_path(&share.host_path).map_err(WrapperError::Path)?;
        let guest = share
            .guest_path
            .as_deref()
            .map(|path| absolute_cli_path(path).map_err(WrapperError::Path))
            .transpose()?
            .unwrap_or_else(|| host.clone());
        launch.policy.extra_shares.push(GuestPathShare {
            host_path: host,
            guest_path: guest.clone(),
            readonly: share.access == ConfigShareAccess::Ro,
            required: share.required,
        });
        for shadow in &share.shadows {
            let relative_path = validate_share_shadow_path(shadow)?;
            let shadow_guest_path = guest.join(&relative_path);
            let backing_path = config_share_shadow_backing_path(project, &shadow_guest_path)?;
            launch
                .policy
                .extra_share_shadows
                .push(GuestPathShareShadow {
                    parent_guest_path: guest.clone(),
                    relative_path,
                    backing_path,
                });
        }
    }
    let config_no_net = !saw_network_override && config.network.mode == ConfigNetworkMode::None;
    if !cli_no_net && !config_no_net {
        for port in &config.published_ports {
            launch
                .policy
                .host_listeners
                .push(HostListener::published_tcp(port.host, port.guest));
        }
    }
    Ok(())
}

fn config_share_shadow_backing_path(project: &Path, guest_path: &Path) -> WrapperResult<PathBuf> {
    if !guest_path.is_absolute() {
        return Err(WrapperError::ShareShadowGuestPathMustBeAbsolute);
    }
    let mut backing = project.join(".sandbox/root");
    for component in guest_path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => backing.push(part),
            Component::ParentDir | Component::Prefix(_) => {
                return Err(WrapperError::EscapingShareShadowGuestPath);
            }
        }
    }
    Ok(backing)
}

fn apply_configured_default_command(
    launch: &mut FrontendLaunchRequest,
    config: &WrapperSandboxConfig,
) {
    apply_payload_script(
        launch,
        payload_script_from_config_command(&config.default_command),
    );
}

fn apply_default_bash_payload(launch: &mut FrontendLaunchRequest) {
    apply_payload_script(
        launch,
        payload_script_from_config_command(&ConfigCommand::new("bash")),
    );
}

fn apply_wrapper_command_override(
    launch: &mut FrontendLaunchRequest,
    command: &WrapperCommandOverride,
) {
    apply_payload_script(launch, command.script());
}

fn apply_payload_script(launch: &mut FrontendLaunchRequest, script: String) {
    payload_launch_args(&mut launch.policy).script = script;
}

fn wrap_payload_script_with_project_mise(launch: &mut FrontendLaunchRequest, project: &Path) {
    let Some(payload) = launch.policy.payload.as_mut() else {
        return;
    };
    payload.script = mise_exec_script(
        &project_mise_config_path(project),
        "agentvm: mise is required to run tools from project mise.toml",
        payload.script.clone(),
    );
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

pub(crate) fn payload_script_from_config_command(command: &ConfigCommand) -> String {
    let mut script = format!("exec {}", shell_quote(&command.command));
    for arg in &command.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script
}

pub(crate) fn wrapper_ui_mode(
    no_tui: bool,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> WrapperUiMode {
    if no_tui || !stdin_is_tty || !stdout_is_tty {
        WrapperUiMode::Plain
    } else {
        WrapperUiMode::Tui
    }
}
