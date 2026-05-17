use super::*;

pub(crate) fn self_test_payload_script(
    _config: &FrontendConfig,
    image: &str,
    hostile: bool,
    payload_stress: bool,
    dns_check: bool,
    docker_net_check: bool,
    publish_container_port: Option<PortPair>,
    fs_check: bool,
    root_persistence_check: bool,
    expect_root_persistence: bool,
    skip_sqlite_concurrency: bool,
) -> String {
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
    let payload_stress_script = r#"import os, sys
blob = os.environ.get("AGENTVM_PAYLOAD_STRESS_BLOB", "")
assert len(blob) == 65536, len(blob)
size = int(os.environ["AGENTVM_PAYLOAD_STRESS_BYTES"])
assert size > 65536, size
sys.stdout.write("self-test: payload-stress-start\n")
sys.stdout.write("X" * size)
sys.stdout.write("\nself-test: payload-stress-ok\n")
sys.stdout.flush()
"#;
    let dns_allow_script = r#"const dns = require("dns");
dns.lookup("example.com", (err, address) => {
  if (err) {
    console.error(`dns-allow-failed example.com ${err.code || err.message}`);
    process.exit(1);
  }
  if (!address) {
    console.error("dns-allow-failed example.com empty-address");
    process.exit(1);
  }
  console.log(`self-test: dns-allow-ok example.com ${address}`);
});
"#;
    let docker_egress_ok =
        format!("self-test: docker-egress-ok image={image} policy=allow phase=container-egress");
    let docker_egress_failed =
        format!("docker-egress-failed image={image} policy=allow phase=container-egress");
    let docker_deny_ok =
        format!("self-test: docker-deny-ok image={image} policy=deny phase=container-egress");
    let docker_deny_unexpected =
        format!("docker-deny-unexpected image={image} policy=deny phase=container-egress");
    let docker_egress_allow_script = format!(
        "if wget -qO- -T 10 http://example.com >/dev/null; then printf '%s\\n' {}; else printf '%s\\n' {} >&2; exit 1; fi",
        shell_quote(&docker_egress_ok),
        shell_quote(&docker_egress_failed),
    );
    let docker_egress_deny_script = format!(
        "if wget -qO- -T 5 http://example.com >/tmp/agentvm-docker-egress 2>/tmp/agentvm-docker-egress.err; then printf '%s\\n' {} >&2; exit 1; else printf '%s\\n' {}; fi",
        shell_quote(&docker_deny_unexpected),
        shell_quote(&docker_deny_ok),
    );
    let docker_net_check_step = format!(
        "if [ \"${{AGENTVM_SELF_TEST_NETWORK:-allow}}\" = allow ]; then docker run --rm {} sh -c {}; else docker run --rm {} sh -c {}; fi",
        shell_quote(image),
        shell_quote(&docker_egress_allow_script),
        shell_quote(image),
        shell_quote(&docker_egress_deny_script),
    );
    let docker_publish_step = publish_container_port.map(|port| {
        let container = "agentvm-self-test-published";
        let ok = format!(
            "self-test: docker-publish-ok image={image} host_port={} guest_port={} phase=container-publish",
            port.host, port.guest
        );
        let ready = format!(
            "self-test: docker-publish-ready image={image} host_port={} guest_port={} phase=container-publish",
            port.host, port.guest
        );
        let server = "{ printf 'HTTP/1.1 200 OK\\r\\nContent-Length: 28\\r\\n\\r\\nagentvm-container-publish-ok'; } | nc -l -p 8080";
        format!(
            "docker rm -f {container} >/dev/null 2>&1 || true; docker run -d --name {container} -p {}:8080 {} sh -c {}; echo {}; sleep 10; docker rm -f {container} >/dev/null 2>&1 || true; echo {}",
            port.guest,
            shell_quote(image),
            shell_quote(server),
            shell_quote(&ready),
            shell_quote(&ok),
        )
    });
    let fs_check_script = r#"import os, shutil, subprocess, time
root = ".agentvm-fs-live"
def read_text_eventually(path):
    last = None
    for _ in range(50):
        try:
            with open(path, encoding="utf-8") as handle:
                return handle.read()
        except OSError as exc:
            last = exc
            time.sleep(0.05)
    raise last
shutil.rmtree(root, ignore_errors=True)
os.makedirs(os.path.join(root, "dir"), exist_ok=True)
with open(os.path.join(root, "dir", "file.txt"), "w", encoding="utf-8") as handle:
    handle.write("one\n")
with open(os.path.join(root, "dir", "file.txt"), "a", encoding="utf-8") as handle:
    handle.write("two\n")
with open(os.path.join(root, "dir", "file.txt"), "r+", encoding="utf-8") as handle:
    handle.truncate(4)
assert read_text_eventually(os.path.join(root, "dir", "file.txt")) == "one\n"
os.rename(os.path.join(root, "dir", "file.txt"), os.path.join(root, "renamed.txt"))
assert read_text_eventually(os.path.join(root, "renamed.txt")) == "one\n"
with open(os.path.join(root, "dir", "delete-me.txt"), "w", encoding="utf-8") as handle:
    handle.write("delete-me\n")
os.unlink(os.path.join(root, "dir", "delete-me.txt"))
assert not os.path.exists(os.path.join(root, "dir", "delete-me.txt"))
entries = sorted(os.listdir(root))
assert entries == ["dir", "renamed.txt"], entries
os.symlink("/run/agentvm-config/mitm-ca.key", os.path.join(root, "key-link"))
try:
    open(os.path.join(root, "key-link"), "rb").read(1)
except OSError:
    pass
else:
    raise AssertionError("config private key readable through workspace symlink")
with open(os.path.join(root, "host-visible.txt"), "w", encoding="utf-8") as handle:
    handle.write("host-visible-ok\n")
uid_gid = subprocess.check_output(["stat", "-c", "%u:%g", os.path.join(root, "host-visible.txt")], text=True).strip()
expected = f"{os.getuid()}:{os.getgid()}"
assert uid_gid == expected, (uid_gid, expected)
print("self-test: fs-live-ok")
"#;
    let persistence_marker = "/var/tmp/agentvm-root-persistence/marker";
    let root_persistence_script = if expect_root_persistence {
        format!(
            "test \"$(cat {marker})\" = root-persistence-ok; echo self-test: root-persistence-present",
            marker = shell_quote(persistence_marker)
        )
    } else {
        format!(
            "mkdir -p {dir}; printf root-persistence-ok > {marker}; test \"$(cat {marker})\" = root-persistence-ok; sync; echo self-test: root-persistence-written",
            dir = shell_quote("/var/tmp/agentvm-root-persistence"),
            marker = shell_quote(persistence_marker)
        )
    };
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
        "printf home-write-ok > \"$HOME/.agentvm-self-test-home-write\"".to_string(),
        "test \"$(cat \"$HOME/.agentvm-self-test-home-write\")\" = home-write-ok".to_string(),
        "rm -f \"$HOME/.agentvm-self-test-home-write\"".to_string(),
        "echo self-test: home-write-ok".to_string(),
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
        "printf bind-ok > .agentvm-self-test-bind".to_string(),
        "rm -f .agentvm-self-test-workspace .agentvm-self-test-bind".to_string(),
        "sync".to_string(),
        "echo self-test: payload-ok".to_string(),
    ];
    if hostile {
        steps.splice(12..12, hostile_self_test_payload_steps());
    }
    if !skip_sqlite_concurrency {
        let bind_index = steps
            .iter()
            .position(|step| step == "printf bind-ok > .agentvm-self-test-bind")
            .expect("bind step");
        steps.splice(
            bind_index..bind_index,
            [
                "echo self-test: sqlite-concurrency-smoke".to_string(),
                format!("python3 -c {}", shell_quote(sqlite_concurrency)),
            ],
        );
    }
    if !root_persistence_check {
        let cleanup_index = steps
            .iter()
            .position(|step| step == "rm -f .agentvm-self-test-workspace .agentvm-self-test-bind")
            .expect("cleanup step");
        steps.splice(
            cleanup_index..cleanup_index,
            [
                "docker version >/tmp/agentvm-docker-version".to_string(),
                "docker info >/tmp/agentvm-docker-info".to_string(),
                format!(
                    "docker run --rm -v \"$PWD:/work:ro\" {} sh -c {}",
                    shell_quote(image),
                    shell_quote("echo docker-run-ok; cat /work/.agentvm-self-test-bind")
                ),
            ],
        );
    }
    if dns_check {
        steps.insert(
            steps.len() - 1,
            format!("node -e {}", shell_quote(dns_allow_script)),
        );
    }
    if docker_net_check {
        steps.insert(steps.len() - 1, docker_net_check_step);
    }
    if let Some(docker_publish_step) = docker_publish_step {
        steps.insert(steps.len() - 1, docker_publish_step);
    }
    if fs_check {
        steps.insert(
            steps.len() - 1,
            format!("python3 -c {}", shell_quote(fs_check_script)),
        );
    }
    if root_persistence_check {
        steps.insert(steps.len() - 1, root_persistence_script);
    }
    if payload_stress {
        steps.insert(
            steps.len() - 1,
            format!("python3 -c {}", shell_quote(payload_stress_script)),
        );
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
        "if [ \"${AGENTVM_SELF_TEST_NETWORK:-allow}\" = deny ]; then node -e 'const dns=require(\"dns\"); dns.lookup(\"example.com\", err => { if (err) { console.log(\"self-test: dns-deny-ok example.com \" + (err.code || err.message)); process.exit(0); } console.error(\"dns-deny-unexpected example.com\"); process.exit(1); });'; fi".to_string(),
        "echo self-test: hostile-ok".to_string(),
    ]
}
