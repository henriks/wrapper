use std::collections::BTreeMap;

use agentvm_frontend::payload_client::{
    run_diagnostic_tcp_async, terminal_size, DiagnosticRequest, PayloadClientError,
};
use clap::{Arg, ArgAction, Command as ClapCommand};

use super::parse_clap_matches;

pub(crate) type PayloadCliResult<T> = Result<T, PayloadCliError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum PayloadCliError {
    #[error("{message}")]
    Clap { message: String },
    #[error("--port is required")]
    MissingPort,
    #[error("invalid --port")]
    InvalidPort,
    #[error("invalid --rows")]
    InvalidRows,
    #[error("invalid --cols")]
    InvalidCols,
    #[error("invalid --timeout-seconds")]
    InvalidTimeoutSeconds,
    #[error("invalid --max-output-bytes")]
    InvalidMaxOutputBytes,
    #[error("--env must be KEY=VALUE")]
    InvalidEnvFormat,
    #[error("--env key must not be empty")]
    EmptyEnvKey,
    #[error("--script is required unless --ping is set")]
    MissingScript,
    #[error("--timeout-seconds must be positive")]
    NonPositiveTimeoutSeconds,
    #[error("--max-output-bytes must be positive")]
    NonPositiveMaxOutputBytes,
    #[error("sync diagnostic exited with {exit_code}: {output}")]
    GuestSyncDiagnosticFailed { exit_code: i32, output: String },
    #[error(transparent)]
    PayloadClient { source: PayloadClientError },
}

impl From<PayloadCliError> for String {
    fn from(error: PayloadCliError) -> Self {
        error.to_string()
    }
}

impl From<PayloadClientError> for PayloadCliError {
    fn from(source: PayloadClientError) -> Self {
        Self::PayloadClient { source }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PayloadClientConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) ping: bool,
    pub(crate) script: Option<String>,
    pub(crate) cwd: String,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) no_stdin: bool,
    pub(crate) diagnostic: bool,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_output_bytes: u64,
}

pub(crate) fn payload_client_config_from_args(
    args: &[String],
) -> PayloadCliResult<PayloadClientConfig> {
    let (rows, cols) = terminal_size();
    let matches = parse_clap_matches(payload_client_clap_command(), args)
        .map_err(|message| PayloadCliError::Clap { message })?;
    let mut config = PayloadClientConfig {
        host: matches
            .get_one::<String>("host")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        port: matches
            .get_one::<String>("port")
            .ok_or(PayloadCliError::MissingPort)?
            .parse()
            .map_err(|_| PayloadCliError::InvalidPort)?,
        ping: matches.get_flag("ping"),
        script: matches.get_one::<String>("script").cloned(),
        cwd: matches
            .get_one::<String>("cwd")
            .cloned()
            .unwrap_or_else(|| "/".to_string()),
        env: BTreeMap::new(),
        rows: matches
            .get_one::<String>("rows")
            .map(|value| value.parse().map_err(|_| PayloadCliError::InvalidRows))
            .transpose()?
            .unwrap_or(rows),
        cols: matches
            .get_one::<String>("cols")
            .map(|value| value.parse().map_err(|_| PayloadCliError::InvalidCols))
            .transpose()?
            .unwrap_or(cols),
        no_stdin: matches.get_flag("no_stdin"),
        diagnostic: matches.get_flag("diagnostic"),
        timeout_seconds: matches
            .get_one::<String>("timeout_seconds")
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| PayloadCliError::InvalidTimeoutSeconds)
            })
            .transpose()?
            .unwrap_or(10),
        max_output_bytes: matches
            .get_one::<String>("max_output_bytes")
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| PayloadCliError::InvalidMaxOutputBytes)
            })
            .transpose()?
            .unwrap_or(1024 * 1024),
    };
    if let Some(values) = matches.get_many::<String>("env") {
        for env in values {
            let (key, val) = env
                .split_once('=')
                .ok_or(PayloadCliError::InvalidEnvFormat)?;
            if key.is_empty() {
                return Err(PayloadCliError::EmptyEnvKey);
            }
            config.env.insert(key.to_string(), val.to_string());
        }
    }

    if !config.ping && config.script.is_none() {
        return Err(PayloadCliError::MissingScript);
    }
    if config.timeout_seconds == 0 {
        return Err(PayloadCliError::NonPositiveTimeoutSeconds);
    }
    if config.max_output_bytes == 0 {
        return Err(PayloadCliError::NonPositiveMaxOutputBytes);
    }

    Ok(config)
}

fn payload_client_clap_command() -> ClapCommand {
    ClapCommand::new("payload-client")
        .arg(Arg::new("host").long("host").value_name("HOST"))
        .arg(Arg::new("port").long("port").value_name("PORT"))
        .arg(Arg::new("ping").long("ping").action(ArgAction::SetTrue))
        .arg(Arg::new("script").long("script").value_name("SCRIPT"))
        .arg(
            Arg::new("diagnostic")
                .long("diagnostic")
                .action(ArgAction::SetTrue),
        )
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
            Arg::new("timeout_seconds")
                .long("timeout-seconds")
                .value_name("N"),
        )
        .arg(
            Arg::new("max_output_bytes")
                .long("max-output-bytes")
                .value_name("N"),
        )
        .arg(
            Arg::new("no_stdin")
                .long("no-stdin")
                .action(ArgAction::SetTrue),
        )
}

pub(crate) fn payload_exit_status(exit_code: i32) -> i32 {
    if exit_code < 0 {
        128 + (-exit_code)
    } else {
        exit_code
    }
}

pub(crate) async fn flush_guest_filesystems(addr: std::net::SocketAddr) -> PayloadCliResult<()> {
    let request = guest_sync_diagnostic_request();
    let mut output = Vec::new();
    match run_diagnostic_tcp_async(addr, &request, &mut output).await {
        Ok(0) => Ok(()),
        Ok(exit_code) => Err(PayloadCliError::GuestSyncDiagnosticFailed {
            exit_code,
            output: String::from_utf8_lossy(&output).trim().to_string(),
        }),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn guest_sync_diagnostic_request() -> DiagnosticRequest {
    let mut request = DiagnosticRequest::new("sync");
    request.timeout_seconds = 10;
    request.max_output_bytes = 4096;
    request
}
