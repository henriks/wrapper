use std::env;
use std::process::ExitCode;
use std::time::Duration;

use agentvm_guest_service::{
    serve_docker_bridge_tcp, serve_tcp, DockerBridgeLimits, GuestServiceLimits,
};

#[tokio::main]
async fn main() -> ExitCode {
    match config_from_args(env::args().skip(1)) {
        Ok(GuestServiceCliConfig::Payload(config)) => {
            match serve_tcp(&config.tcp_host, config.tcp_port, config.limits).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("agentvm-guest-service: {error}");
                    ExitCode::from(2)
                }
            }
        }
        Ok(GuestServiceCliConfig::DockerBridge(config)) => match serve_docker_bridge_tcp(
            &config.tcp_host,
            config.tcp_port,
            &config.docker_sock,
            config.limits,
        )
        .await
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("agentvm-guest-service: {error}");
                ExitCode::from(2)
            }
        },
        Err(error) => {
            eprintln!("agentvm-guest-service: {error}");
            eprintln!("usage: agentvm-guest-service [payload] --tcp-port PORT [--tcp-host HOST] [--max-clients N] [--max-diagnostics N] [--initial-timeout SECONDS] [--io-timeout SECONDS]");
            eprintln!("       agentvm-guest-service docker-bridge --tcp-port PORT --docker-sock PATH [--tcp-host HOST] [--max-sessions N] [--connect-timeout SECONDS] [--io-timeout SECONDS]");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GuestServiceCliConfig {
    Payload(PayloadCliConfig),
    DockerBridge(DockerBridgeCliConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadCliConfig {
    tcp_host: String,
    tcp_port: u16,
    limits: GuestServiceLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DockerBridgeCliConfig {
    tcp_host: String,
    tcp_port: u16,
    docker_sock: String,
    limits: DockerBridgeLimits,
}

fn config_from_args(
    args: impl IntoIterator<Item = String>,
) -> Result<GuestServiceCliConfig, String> {
    let mut args = args.into_iter().collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("payload") {
        args.remove(0);
    }
    if args.first().map(String::as_str) == Some("docker-bridge") {
        args.remove(0);
        return docker_bridge_config_from_args(args).map(GuestServiceCliConfig::DockerBridge);
    }
    payload_config_from_args(args).map(GuestServiceCliConfig::Payload)
}

fn payload_config_from_args(
    args: impl IntoIterator<Item = String>,
) -> Result<PayloadCliConfig, String> {
    let mut tcp_host = "0.0.0.0".to_string();
    let mut tcp_port = None;
    let mut limits = GuestServiceLimits::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tcp-host" => tcp_host = next_value(&mut args, "--tcp-host")?,
            "--tcp-port" => tcp_port = Some(parse_next(&mut args, "--tcp-port")?),
            "--max-clients" => limits.max_clients = parse_next(&mut args, "--max-clients")?,
            "--max-diagnostics" => {
                limits.max_diagnostics = parse_next(&mut args, "--max-diagnostics")?
            }
            "--initial-timeout" => {
                limits.initial_timeout =
                    Duration::from_secs(parse_next(&mut args, "--initial-timeout")?)
            }
            "--io-timeout" => {
                limits.io_timeout = Duration::from_secs(parse_next(&mut args, "--io-timeout")?)
            }
            "-h" | "--help" => return Err("help requested".to_string()),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    let tcp_port = tcp_port.ok_or_else(|| "--tcp-port is required".to_string())?;
    if limits.max_clients == 0 {
        return Err("--max-clients must be positive".to_string());
    }
    if limits.max_diagnostics == 0 {
        return Err("--max-diagnostics must be positive".to_string());
    }
    if limits.initial_timeout.is_zero() {
        return Err("--initial-timeout must be positive".to_string());
    }
    if limits.io_timeout.is_zero() {
        return Err("--io-timeout must be positive".to_string());
    }
    Ok(PayloadCliConfig {
        tcp_host,
        tcp_port,
        limits,
    })
}

fn docker_bridge_config_from_args(
    args: impl IntoIterator<Item = String>,
) -> Result<DockerBridgeCliConfig, String> {
    let mut tcp_host = "0.0.0.0".to_string();
    let mut tcp_port = None;
    let mut docker_sock = None;
    let mut limits = DockerBridgeLimits::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tcp-host" => tcp_host = next_value(&mut args, "--tcp-host")?,
            "--tcp-port" => tcp_port = Some(parse_next(&mut args, "--tcp-port")?),
            "--docker-sock" => docker_sock = Some(next_value(&mut args, "--docker-sock")?),
            "--max-sessions" => limits.max_sessions = parse_next(&mut args, "--max-sessions")?,
            "--connect-timeout" => {
                limits.connect_timeout =
                    Duration::from_secs(parse_next(&mut args, "--connect-timeout")?)
            }
            "--io-timeout" => {
                limits.io_timeout = Duration::from_secs(parse_next(&mut args, "--io-timeout")?)
            }
            "-h" | "--help" => return Err("help requested".to_string()),
            other => return Err(format!("unexpected argument: {other}")),
        }
    }
    let tcp_port = tcp_port.ok_or_else(|| "--tcp-port is required".to_string())?;
    let docker_sock = docker_sock.ok_or_else(|| "--docker-sock is required".to_string())?;
    if limits.max_sessions == 0 {
        return Err("--max-sessions must be positive".to_string());
    }
    if limits.connect_timeout.is_zero() {
        return Err("--connect-timeout must be positive".to_string());
    }
    if limits.io_timeout.is_zero() {
        return Err("--io-timeout must be positive".to_string());
    }
    Ok(DockerBridgeCliConfig {
        tcp_host,
        tcp_port,
        docker_sock,
        limits,
    })
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_next<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String> {
    next_value(args, flag).and_then(|value| {
        value
            .parse()
            .map_err(|_| format!("invalid value for {flag}: {value}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_guest_service_cli_defaults_and_limits() {
        let config = config_from_args([
            "--tcp-port".to_string(),
            "1234".to_string(),
            "--max-clients".to_string(),
            "8".to_string(),
            "--max-diagnostics".to_string(),
            "2".to_string(),
            "--initial-timeout".to_string(),
            "3".to_string(),
            "--io-timeout".to_string(),
            "4".to_string(),
        ])
        .expect("config");

        let GuestServiceCliConfig::Payload(config) = config else {
            panic!("expected payload config");
        };
        assert_eq!(config.tcp_host, "0.0.0.0");
        assert_eq!(config.tcp_port, 1234);
        assert_eq!(config.limits.max_clients, 8);
        assert_eq!(config.limits.max_diagnostics, 2);
        assert_eq!(config.limits.initial_timeout, Duration::from_secs(3));
        assert_eq!(config.limits.io_timeout, Duration::from_secs(4));
    }

    #[test]
    fn parses_docker_bridge_cli_config() {
        let config = config_from_args([
            "docker-bridge".to_string(),
            "--tcp-port".to_string(),
            "1075".to_string(),
            "--docker-sock".to_string(),
            "/var/run/docker.sock".to_string(),
            "--max-sessions".to_string(),
            "8".to_string(),
            "--connect-timeout".to_string(),
            "3".to_string(),
            "--io-timeout".to_string(),
            "4".to_string(),
        ])
        .expect("config");

        let GuestServiceCliConfig::DockerBridge(config) = config else {
            panic!("expected docker bridge config");
        };
        assert_eq!(config.tcp_host, "0.0.0.0");
        assert_eq!(config.tcp_port, 1075);
        assert_eq!(config.docker_sock, "/var/run/docker.sock");
        assert_eq!(config.limits.max_sessions, 8);
        assert_eq!(config.limits.connect_timeout, Duration::from_secs(3));
        assert_eq!(config.limits.io_timeout, Duration::from_secs(4));
    }

    #[test]
    fn guest_service_cli_requires_port_and_positive_limits() {
        assert!(config_from_args(Vec::<String>::new())
            .expect_err("missing port")
            .contains("--tcp-port"));
        assert!(config_from_args([
            "--tcp-port".to_string(),
            "1234".to_string(),
            "--max-clients".to_string(),
            "0".to_string(),
        ])
        .expect_err("zero clients")
        .contains("positive"));
    }
}
