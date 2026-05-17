use super::*;

pub(crate) use crate::self_test_payload::self_test_payload_script;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfTestConfig {
    pub(crate) project: PathBuf,
    pub(crate) run_dir: PathBuf,
    pub(crate) artifact_manifest: PathBuf,
    pub(crate) qemu: PathBuf,
    pub(crate) image: String,
    pub(crate) publish_payload_port: Option<u16>,
    pub(crate) publish_container_port: Option<PortPair>,
    pub(crate) no_net: bool,
    pub(crate) hostile: bool,
    pub(crate) payload_stress: bool,
    pub(crate) dns_check: bool,
    pub(crate) docker_net_check: bool,
    pub(crate) skip_sqlite_concurrency: bool,
    pub(crate) fs_check: bool,
    pub(crate) root_persistence_check: bool,
    pub(crate) expect_root_persistence: bool,
}

pub(crate) fn run_self_test(args: &[String]) -> Result<(), String> {
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
    let skip_sqlite_concurrency = self_test.skip_sqlite_concurrency
        || self_test.docker_net_check
        || self_test.publish_container_port.is_some();
    let sqlite_concurrency_host_db = config
        .project
        .join(format!(".agentvm-self-test-sqlite/{sqlite_db_name}.sqlite"));
    if !skip_sqlite_concurrency {
        if let Some(parent) = sqlite_concurrency_host_db.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create sqlite concurrency dir: {error}"))?;
        }
        reset_sqlite_concurrency_db(&sqlite_concurrency_host_db)?;
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
            self_test.root_persistence_check,
            self_test.expect_root_persistence,
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
    let host_sqlite_wait_result = if let Some(host_sqlite) = host_sqlite.as_mut() {
        wait_host_sqlite_concurrency(host_sqlite)
    } else {
        Ok(())
    };
    println!("self-test: phase=shutting-down-frontend");
    flush_guest_filesystems(payload_addr)
        .map_err(|error| format!("self-test guest sync failed: {error}\n{artifacts}"))?;
    running
        .terminate()
        .map_err(|error| format!("self-test shutdown failed: {error}\n{artifacts}"))?;
    let exit_code = exit_code?;
    if exit_code != 0 {
        return Err(format!(
            "self-test payload exited with {exit_code}\n{artifacts}"
        ));
    }
    host_sqlite_wait_result
        .map_err(|error| format!("self-test host sqlite failed: {error}\n{artifacts}"))?;
    if !skip_sqlite_concurrency {
        run_host_sqlite_integrity_check(&sqlite_concurrency_host_db)
            .map_err(|error| format!("self-test host sqlite failed: {error}\n{artifacts}"))?;
    }
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

pub(crate) fn self_test_config_from_args(args: &[String]) -> Result<SelfTestConfig, String> {
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
        artifact_manifest: launch_cli::artifact_manifest_arg(&matches),
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
        skip_sqlite_concurrency: matches.get_flag("skip_sqlite_concurrency"),
        fs_check: matches.get_flag("fs_check"),
        root_persistence_check: matches.get_flag("root_persistence_check"),
        expect_root_persistence: matches.get_flag("expect_root_persistence"),
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
            Arg::new("skip_sqlite_concurrency")
                .long("skip-sqlite-concurrency")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("fs_check")
                .long("fs-check")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("root_persistence_check")
                .long("root-persistence-check")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("expect_root_persistence")
                .long("expect-root-persistence")
                .action(ArgAction::SetTrue),
        )
}
pub(crate) fn reset_sqlite_concurrency_db(db: &Path) -> Result<(), String> {
    let mut paths = vec![db.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut path = db.as_os_str().to_os_string();
        path.push(suffix);
        paths.push(PathBuf::from(path));
    }
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to remove stale sqlite self-test file {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
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

    fn unique_temp_dir() -> TestTempDir {
        TestTempDir {
            dir: tempfile::Builder::new()
                .prefix("agentvm-self-test-")
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

    #[test]
    fn default_artifact_manifest_is_repo_relative_not_cwd_relative() {
        assert_eq!(
            launch_cli::default_artifact_manifest_path(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("vm-frontend parent")
                .join("docker/out/artifact-manifest.json")
        );
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
            "--skip-sqlite-concurrency".to_string(),
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
        assert!(config.skip_sqlite_concurrency);
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
        assert!(script.contains("agentvm-self-test-home-write"));
        assert!(script.contains("self-test: home-write-ok"));
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
    fn reset_sqlite_concurrency_db_removes_stale_db_wal_and_shm() {
        let root = frontend_test_root();
        let db = root.join("state.sqlite");
        std::fs::write(&db, "db").expect("db");
        std::fs::write(root.join("state.sqlite-wal"), "wal").expect("wal");
        std::fs::write(root.join("state.sqlite-shm"), "shm").expect("shm");

        reset_sqlite_concurrency_db(&db).expect("reset sqlite db");

        assert!(!db.exists());
        assert!(!root.join("state.sqlite-wal").exists());
        assert!(!root.join("state.sqlite-shm").exists());
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
            false,
            false,
            true,
        );

        assert!(script.contains("sqlite-home-smoke"));
        assert!(!script.contains("sqlite-concurrency-smoke"));
        assert!(script.contains("docker-deny-ok image=alpine:3.22 policy=deny"));
    }

    #[test]
    fn self_test_payload_skips_sqlite_concurrency_for_docker_egress_check() {
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
            false,
            true,
        );

        assert!(script.contains("sqlite-home-smoke"));
        assert!(!script.contains("sqlite-concurrency-smoke"));
        assert!(script.contains("docker-egress-ok image=alpine:3.22 policy=allow"));
    }

    #[test]
    fn self_test_payload_skips_sqlite_concurrency_for_published_port_check() {
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
                host: 12080,
                guest: 18080,
            }),
            false,
            false,
            false,
            true,
        );

        assert!(script.contains("sqlite-home-smoke"));
        assert!(!script.contains("sqlite-concurrency-smoke"));
        assert!(script.contains("docker-publish-ok image=alpine:3.22"));
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
            false,
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
    fn self_test_payload_root_persistence_check_writes_and_verifies_marker() {
        let root = frontend_test_root();
        let config = FrontendConfig::from_artifact_manifest_file(
            root.join("repo"),
            root.join(".sandbox/docker-vm/self-test"),
            "qemu-system-x86_64",
            root.join("docker/out/artifact-manifest.json"),
        )
        .expect("config");

        let write_script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            false,
            None,
            false,
            true,
            false,
            false,
        );
        assert!(write_script.contains("root-persistence-written"));
        assert!(write_script.contains("/var/tmp/agentvm-root-persistence/marker"));
        assert!(!write_script.contains("docker run --rm"));

        let verify_script = self_test_payload_script(
            &config,
            "alpine:3.22",
            false,
            false,
            false,
            false,
            None,
            false,
            true,
            true,
            false,
        );
        assert!(verify_script.contains("root-persistence-present"));
        assert!(!verify_script.contains("root-persistence-written"));
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
}
