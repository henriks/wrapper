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
    #[error("startup dialog did not select a payload")]
    StartupDialogNoPayload,
    #[error("{0}")]
    Path(String),
    #[error("share shadow guest path must be absolute")]
    ShareShadowGuestPathMustBeAbsolute,
    #[error("share shadow guest path must stay under guest root")]
    EscapingShareShadowGuestPath,
}

pub(crate) fn run_wrapper(args: Vec<String>) -> WrapperResult<()> {
    let mut wrapper = parse_wrapper_args(&args).map_err(WrapperError::Parse)?;
    if wrapper.help {
        return Ok(());
    }
    if wrapper.reset {
        reset_project(&wrapper.project).map_err(WrapperError::Reset)?;
        return Ok(());
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
        return Ok(());
    }
    if wrapper.tls_bootstrap {
        let ca = ensure_wrapper_mitm_ca(&wrapper.project)?;
        push_launch_value(
            &mut wrapper.launch_args,
            WrapperLaunchFlag::TlsCaCert,
            ca.cert.display().to_string(),
        );
        push_launch_value(
            &mut wrapper.launch_args,
            WrapperLaunchFlag::TlsCaKey,
            ca.key.display().to_string(),
        );
        push_launch_flag(
            &mut wrapper.launch_args,
            WrapperLaunchFlag::TlsGeneratePerHostCerts,
        );
    }
    let project = wrapper.project.clone();
    let ui_mode = wrapper.ui_mode;
    let needs_startup_dialog = ui_mode == WrapperUiMode::Tui
        && !wrapper.tool_selected
        && wrapper.command_override.is_none();
    let mut launch_args = vec!["launch".to_string()];
    launch_args.extend(wrapper.into_launch_args());
    if needs_startup_dialog {
        let selection =
            tui::run_startup_dialog().map_err(|error| WrapperError::Tui(error.to_string()))?;
        if selection.enable_codex {
            let config = WrapperSandboxConfig::codex_default()?;
            write_wrapper_sandbox_config(&project, &config)?;
            write_setup_tool_mise_config(&project, SetupTool::Codex)?;
            apply_configured_launch_defaults(&mut launch_args, &config, &project, false, false)?;
            apply_payload_script(
                &mut launch_args,
                setup_tool_install_then_exec_script(
                    &project,
                    SetupTool::Codex,
                    payload_script_from_config_command(&config.default_command),
                ),
            );
        } else {
            return Err(WrapperError::StartupDialogNoPayload);
        }
    }
    run_launch(&launch_args[1..], ui_mode).map_err(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperLaunchFlag {
    Project,
    NoNet,
    AllowIp,
    AllowDomain,
    Gh,
    Aws,
    Ro,
    Rw,
    Qemu,
    ArtifactManifest,
    Publish,
    AllowPublicInternet,
    ShareRo,
    ShareRw,
    ShareShadow,
    PayloadScript,
    TlsCaCert,
    TlsCaKey,
    TlsGeneratePerHostCerts,
}

impl WrapperLaunchFlag {
    fn as_str(self) -> &'static str {
        match self {
            Self::Project => "--project",
            Self::NoNet => "--no-net",
            Self::AllowIp => "--allow-ip",
            Self::AllowDomain => "--allow-domain",
            Self::Gh => "--gh",
            Self::Aws => "--aws",
            Self::Ro => "--ro",
            Self::Rw => "--rw",
            Self::Qemu => "--qemu",
            Self::ArtifactManifest => "--artifact-manifest",
            Self::Publish => "--publish",
            Self::AllowPublicInternet => "--allow-public-internet",
            Self::ShareRo => "--share-ro",
            Self::ShareRw => "--share-rw",
            Self::ShareShadow => "--share-shadow",
            Self::PayloadScript => "--payload-script",
            Self::TlsCaCert => "--tls-ca-cert",
            Self::TlsCaKey => "--tls-ca-key",
            Self::TlsGeneratePerHostCerts => "--tls-generate-per-host-certs",
        }
    }
}

#[derive(Debug)]
pub(crate) struct WrapperArgs {
    pub(crate) project: PathBuf,
    pub(crate) launch_args: Vec<String>,
    pub(crate) ui_mode: WrapperUiMode,
    pub(crate) tool_selected: bool,
    pub(crate) command_override: Option<WrapperCommandOverride>,
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

impl WrapperArgs {
    fn into_launch_args(self) -> Vec<String> {
        self.launch_args
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
        push_launch_value(
            &mut launch_args,
            WrapperLaunchFlag::Project,
            project.display().to_string(),
        );
    }
    if no_net {
        push_launch_flag(&mut launch_args, WrapperLaunchFlag::NoNet);
    }
    if let Some(values) = matches.get_many::<String>("allow_ip") {
        for value in values {
            push_launch_value(&mut launch_args, WrapperLaunchFlag::AllowIp, value.clone());
        }
    }
    if let Some(values) = matches.get_many::<String>("allow_domain") {
        for value in values {
            push_launch_value(
                &mut launch_args,
                WrapperLaunchFlag::AllowDomain,
                value.clone(),
            );
        }
    }
    if matches.get_flag("gh") {
        push_launch_flag(&mut launch_args, WrapperLaunchFlag::Gh);
    }
    for (id, flag) in [
        ("aws", WrapperLaunchFlag::Aws),
        ("ro", WrapperLaunchFlag::Ro),
        ("rw", WrapperLaunchFlag::Rw),
        ("qemu", WrapperLaunchFlag::Qemu),
        ("artifact_manifest", WrapperLaunchFlag::ArtifactManifest),
    ] {
        if let Some(values) = matches.get_many::<String>(id) {
            for value in values {
                push_launch_value(&mut launch_args, flag, value.clone());
            }
        }
    }
    if let Some(values) = matches.get_many::<PortPair>("docker_publish") {
        for value in values {
            push_launch_value(
                &mut launch_args,
                WrapperLaunchFlag::Publish,
                value.to_string(),
            );
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
    if !has_launch_flag(&launch_args, WrapperLaunchFlag::Project) {
        push_launch_value(
            &mut launch_args,
            WrapperLaunchFlag::Project,
            project.display().to_string(),
        );
    }
    let ui_mode = wrapper_ui_mode(no_tui, stdin_is_tty, stdout_is_tty);
    let sandbox_config = if let Some(setup_tool) = setup_tool {
        Some(WrapperSandboxConfig::setup_tool(setup_tool)?)
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
        )
        .map_err(|error| error.to_string())?;
    }
    let mut tool_selected = sandbox_config.is_some();
    if ui_mode == WrapperUiMode::Plain && command_override.is_none() && sandbox_config.is_none() {
        return Err(
            "project is not configured; run agentvm --setup-tool codex|pi, use -- COMMAND, or start interactive TUI setup".to_string(),
        );
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
            &mut launch_args,
            setup_tool_install_then_exec_script(&project, tool, final_script),
        );
        tool_selected = true;
    } else if let Some(command) = command_override.as_ref() {
        apply_wrapper_command_override(&mut launch_args, command);
    } else if let Some(config) = sandbox_config.as_ref() {
        apply_configured_default_command(&mut launch_args, config);
        tool_selected = true;
    }
    if setup_tool.is_none() && wrapper_mise_config_path(&project).exists() {
        wrap_payload_script_with_project_mise(&mut launch_args, &project);
    }
    if !no_net
        && !has_launch_flag(&launch_args, WrapperLaunchFlag::NoNet)
        && !has_launch_flag(&launch_args, WrapperLaunchFlag::AllowIp)
        && !has_launch_flag(&launch_args, WrapperLaunchFlag::AllowDomain)
        && !has_launch_flag(&launch_args, WrapperLaunchFlag::AllowPublicInternet)
    {
        push_launch_flag(&mut launch_args, WrapperLaunchFlag::AllowPublicInternet);
        tls_bootstrap = true;
    } else if has_any_launch_flag(
        &launch_args,
        &[
            WrapperLaunchFlag::AllowPublicInternet,
            WrapperLaunchFlag::AllowIp,
            WrapperLaunchFlag::AllowDomain,
        ],
    ) {
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
    launch_args: &mut Vec<String>,
    config: &WrapperSandboxConfig,
    project: &Path,
    saw_network_override: bool,
    cli_no_net: bool,
) -> WrapperResult<()> {
    if !saw_network_override {
        match config.network.mode {
            ConfigNetworkMode::Public => {
                push_launch_flag(launch_args, WrapperLaunchFlag::AllowPublicInternet);
            }
            ConfigNetworkMode::None => {
                push_launch_flag(launch_args, WrapperLaunchFlag::NoNet);
            }
            ConfigNetworkMode::Allowlist => {
                for domain in &config.network.allowed_domains {
                    push_launch_value(launch_args, WrapperLaunchFlag::AllowDomain, domain.clone());
                }
                for host in &config.network.allowed_hosts {
                    push_launch_value(launch_args, WrapperLaunchFlag::AllowDomain, host.clone());
                }
                for ip in &config.network.allowed_ips {
                    push_launch_value(launch_args, WrapperLaunchFlag::AllowIp, ip.clone());
                }
            }
        }
    }
    if config.auth.github && !has_launch_flag(launch_args, WrapperLaunchFlag::Gh) {
        push_launch_flag(launch_args, WrapperLaunchFlag::Gh);
    }
    if let Some(profile) = &config.auth.aws_profile {
        if !has_launch_flag(launch_args, WrapperLaunchFlag::Aws) {
            push_launch_value(launch_args, WrapperLaunchFlag::Aws, profile.clone());
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
        let flag = match share.access {
            ConfigShareAccess::Ro => WrapperLaunchFlag::ShareRo,
            ConfigShareAccess::Rw => WrapperLaunchFlag::ShareRw,
        };
        let required = if share.required {
            "required"
        } else {
            "optional"
        };
        push_launch_value(
            launch_args,
            flag,
            format!("{}={}={required}", host.display(), guest.display()),
        );
        for shadow in &share.shadows {
            let relative_path = validate_share_shadow_path(shadow)?;
            let shadow_guest_path = guest.join(&relative_path);
            let backing = config_share_shadow_backing_path(project, &shadow_guest_path)?;
            push_launch_value(
                launch_args,
                WrapperLaunchFlag::ShareShadow,
                format!(
                    "{}={}={}",
                    guest.display(),
                    relative_path.display(),
                    backing.display()
                ),
            );
        }
    }
    let config_no_net = !saw_network_override && config.network.mode == ConfigNetworkMode::None;
    if !cli_no_net && !config_no_net {
        for port in &config.published_ports {
            push_launch_value(
                launch_args,
                WrapperLaunchFlag::Publish,
                format!("{}:{}", port.host, port.guest),
            );
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

fn apply_configured_default_command(launch_args: &mut Vec<String>, config: &WrapperSandboxConfig) {
    apply_payload_script(
        launch_args,
        payload_script_from_config_command(&config.default_command),
    );
}

fn apply_wrapper_command_override(launch_args: &mut Vec<String>, command: &WrapperCommandOverride) {
    apply_payload_script(launch_args, command.script());
}

fn apply_payload_script(launch_args: &mut Vec<String>, script: String) {
    upsert_launch_value(launch_args, WrapperLaunchFlag::PayloadScript, script);
}

fn wrap_payload_script_with_project_mise(launch_args: &mut Vec<String>, project: &Path) {
    let mut index = 0;
    while index + 1 < launch_args.len() {
        if launch_args[index] == WrapperLaunchFlag::PayloadScript.as_str() {
            let script = launch_args[index + 1].clone();
            launch_args[index + 1] = mise_install_then_exec_script(
                &wrapper_mise_config_path(project),
                "agentvm: mise is required to install tools from .sandbox/mise.toml",
                script,
            );
            return;
        }
        index += 1;
    }
}

pub(crate) fn payload_script_from_config_command(command: &ConfigCommand) -> String {
    let mut script = format!("exec {}", shell_quote(&command.command));
    for arg in &command.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script
}

fn push_launch_flag(launch_args: &mut Vec<String>, flag: WrapperLaunchFlag) {
    launch_args.push(flag.as_str().to_string());
}

fn push_launch_value(launch_args: &mut Vec<String>, flag: WrapperLaunchFlag, value: String) {
    launch_args.extend([flag.as_str().to_string(), value]);
}

fn has_launch_flag(launch_args: &[String], flag: WrapperLaunchFlag) -> bool {
    launch_args.iter().any(|arg| arg == flag.as_str())
}

fn has_any_launch_flag(launch_args: &[String], flags: &[WrapperLaunchFlag]) -> bool {
    flags.iter().any(|flag| has_launch_flag(launch_args, *flag))
}

fn upsert_launch_value(launch_args: &mut Vec<String>, flag: WrapperLaunchFlag, value: String) {
    let mut index = 0;
    while index < launch_args.len() {
        if launch_args[index] == flag.as_str() && index + 1 < launch_args.len() {
            launch_args[index + 1] = value;
            return;
        }
        index += 1;
    }
    push_launch_value(launch_args, flag, value);
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
