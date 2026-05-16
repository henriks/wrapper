use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::TempDir;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct PtyRun {
    status: std::process::ExitStatus,
    output: String,
}

fn run_agentvm_in_pty(project: &TempDir, args: &[impl AsRef<str>], input: &str) -> PtyRun {
    run_agentvm_in_pty_inner(project, args, input, None)
}

fn run_agentvm_in_pty_with_timeout(
    project: &TempDir,
    args: &[impl AsRef<str>],
    input: &str,
    timeout_seconds: u64,
) -> PtyRun {
    run_agentvm_in_pty_inner(project, args, input, Some(timeout_seconds))
}

fn run_agentvm_in_pty_inner(
    project: &TempDir,
    args: &[impl AsRef<str>],
    input: &str,
    timeout_seconds: Option<u64>,
) -> PtyRun {
    let agentvm = env!("CARGO_BIN_EXE_agentvm");
    let mut command = Vec::new();
    if let Some(timeout_seconds) = timeout_seconds {
        command.extend(["timeout".to_string(), format!("{timeout_seconds}s")]);
    }
    command.extend([
        shell_quote(agentvm),
        "--project".to_string(),
        shell_quote(project.path().to_str().expect("utf-8 project path")),
    ]);
    command.extend(args.iter().map(|arg| shell_quote(arg.as_ref())));
    let command = command.join(" ");

    let mut child = Command::new("script")
        .arg("-qefc")
        .arg(command)
        .arg("/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn script pty harness");
    child
        .stdin
        .as_mut()
        .expect("script stdin")
        .write_all(input.as_bytes())
        .expect("write scripted input");
    let output = child.wait_with_output().expect("wait for script");
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    PtyRun {
        status: output.status,
        output: combined,
    }
}

fn read_config(project: &TempDir) -> serde_json::Value {
    let text = std::fs::read_to_string(project.path().join(".sandbox/config.json"))
        .expect("read generated config");
    serde_json::from_str(&text).expect("parse generated config")
}

fn artifact_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("docker/out/artifact-manifest.json")
}

fn write_codex_config(project: &TempDir) {
    let sandbox = project.path().join(".sandbox");
    std::fs::create_dir_all(&sandbox).expect("create sandbox config dir");
    std::fs::write(
        sandbox.join("config.json"),
        serde_json::json!({
            "schema_version": 3,
            "default_command": { "command": "codex", "args": ["--dangerously-bypass-approvals-and-sandbox"] },
            "network": {
                "mode": "public",
                "allowed_domains": [],
                "allowed_hosts": [],
                "allowed_ips": []
            },
            "auth": { "github": false, "aws_profile": null },
            "shares": [],
            "published_ports": []
        })
        .to_string(),
    )
    .expect("write codex config");
}

#[test]
fn startup_dialog_terminal_cancel_reports_no_payload_without_config() {
    let project = TempDir::new().expect("temp project");

    let run = run_agentvm_in_pty(&project, &[] as &[&str], "n");

    assert!(!run.status.success(), "unexpected success: {}", run.output);
    assert!(
        run.output
            .contains("startup dialog did not select a payload"),
        "missing startup cancel diagnostic in output: {}",
        run.output
    );
    assert!(
        !project.path().join(".sandbox/config.json").exists(),
        "cancelled first-run dialog should not persist config"
    );
}

#[test]
fn config_editor_terminal_keys_update_and_save_config() {
    let project = TempDir::new().expect("temp project");

    let run = run_agentvm_in_pty(&project, &["--config"], "cngdps");

    assert!(run.status.success(), "config editor failed: {}", run.output);
    let config = read_config(&project);
    assert!(config.get("setup_tool").is_none());
    assert_eq!(config["default_command"]["command"], "pi");
    assert_eq!(config["network"]["mode"], "none");
    assert_eq!(config["auth"]["github"], true);
    assert_eq!(config["shares"].as_array().expect("shares array").len(), 2);
    assert_eq!(
        config["published_ports"]
            .as_array()
            .expect("published ports array")
            .len(),
        1
    );
}

#[test]
fn configured_startup_terminal_skips_dialog_and_reports_launch_artifacts() {
    let project = TempDir::new().expect("temp project");
    write_codex_config(&project);
    let artifact_manifest = artifact_manifest();
    let args = vec![
        "--qemu".to_string(),
        "/tmp/agentvm-no-qemu-for-tui-test".to_string(),
        "--artifact-manifest".to_string(),
        artifact_manifest.display().to_string(),
    ];

    let run = run_agentvm_in_pty_with_timeout(&project, &args, "", 20);

    assert!(!run.status.success(), "unexpected success: {}", run.output);
    assert!(
        !run.output
            .contains("startup dialog did not select a payload"),
        "configured project should not reprompt: {}",
        run.output
    );
    assert!(
        run.output.contains("launch: phase=starting-frontend"),
        "missing launch phase diagnostic: {}",
        run.output
    );
    assert!(
        run.output.contains("launch failed:") && run.output.contains("artifacts: run_dir="),
        "missing launch artifact diagnostic: {}",
        run.output
    );
}
