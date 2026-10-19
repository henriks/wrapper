# Sandbox wrapper requirements

The wrapper runs Codex, or an explicitly supplied command, in a persistent
project-specific smolvm machine. Linux hosts require KVM. The guest image is built
from the checked-in Debian slim Dockerfile; host system directories are not shared.

1. `codex-wrap` and `sandbox-wrap` invoke the same Python script. Arguments after
   `--` replace the default `codex --dangerously-bypass-approvals-and-sandbox` command.
2. `--project PATH` selects the workspace, defaulting to the current directory.
   It is shared writable at its canonical absolute host path.
3. `.sandbox/` contains smolvm's registry and persistent guest disks, selected
   using `SMOLVM_DATA_DIR`. HOME is `/home/agent` in the persistent root filesystem.
   `/tmp` and `/run` remain ephemeral. A project lifecycle lock serializes
   startup, configuration and reset.
   Concurrent foreground sessions share the VM, which stops after the last exits.
4. `.sandbox/` is masked within the guest workspace. Git metadata is read-only
   unless `--git` is selected, including external metadata for linked worktrees.
   Git protection is an ordinary-operation guard: Docker access grants guest root
   privileges, so it does not prevent deliberate bypass by a hostile workload.
5. Host `~/.codex`, when present, is shared writable at guest `~/.codex`. No other
   agent's host configuration, host toolchain directories, or host Docker socket
   is shared. No host AWS configuration is shared.
6. Mise is installed in the image. Node 24 and Codex are installed through mise
   on launch; project mise configurations are used without being rewritten.
   AWS CLI and GitHub CLI are provisioned through mise when their credentials are
   injected. Downloads and other guest filesystem changes persist across stop/start.
7. Docker Engine, CLI, Buildx and the `docker compose` plugin are installed from
   Docker's official stable apt repository. Its daemon starts on each VM boot.
   `/var/lib/docker` is a persistent guest-local volume on smolvm's storage disk,
   separate from the container's root overlay and host workspace. Docker readiness
   is required before launching the workload.
8. Outbound networking defaults on; `--no-net` disables it. `--port HOST:GUEST`
   publishes TCP ports. VM localhost is distinct from host localhost; this wrapper
   does not implicitly expose host loopback services.
9. `--ro PATH` and `--rw PATH` share extra directories at their absolute paths.
   File mounts are unsupported. Extra mounts cannot overlap the project or guest
   HOME. Paths containing colon/newline are rejected because smolvm uses delimiters.
10. `--aws PROFILE` exports resolved credentials on the host. `--gh` resolves a
    host GitHub token. `--pass-env NAME` explicitly forwards other variables.
    Values are passed through smolvm's per-exec secret references, not stored in
    machine configuration or included as plaintext command arguments. Guest Git
    global config, AWS config/credential files, SSH agent and Kubernetes config
    are disabled by default. Shared Codex state remains intentionally accessible.
11. Interactive sessions use a guest PTY. Nonterminal sessions stream output and
    stdin. Exit codes are propagated; normal exit, Ctrl-C, SIGTERM and SIGHUP
    end that session. The last session triggers VM shutdown. OS leases also
    release on SIGKILL; a separate host supervisor keeps other sessions alive.
    Supervisor/host failure can leave the VM running; after old clients exit,
    the next launch stops it before reconfiguration. Guest disks persist in either case.
12. `--reset` deletes the project VM and `.sandbox/`. Existing bwrap state is not
    automatically migrated. Move old `.sandbox/` aside before first use.
13. `--image` selects a registry reference or local archive for first creation.
    The default is `sandbox-wrap.tar` beside the wrapper, built and exported
    automatically using host Docker when absent at launch.
    `--rebuild-image` forces a build/export of the default archive and exits,
    without requiring smolvm or touching project VM state. It cannot be combined
    with `--reset`, a guest command, or a custom `--image`.
    Concurrent builds are serialized and the archive is published atomically.
    Existing archives are reused; explicit images are never built automatically.
    Reset does not build an image. An existing machine keeps
    its original image and filesystem; rebuild plus `--reset` is needed to replace
    the base image. Mounts, git mode, network access, and published ports are
    updated only when the effective configuration changes, without discarding
    disks. Per-session credentials and commands are applied on every launch.

14. `hostctl status` inside the guest queries the running wrapper over JSON-RPC
    through `/run/sandbox-wrap/host.sock`, forwarded from `.sandbox/host.sock`.
    Status contains lifecycle state, VM name, project path, host supervisor PID,
    networking, git access, and published ports. No credentials or host execution
    methods are exposed. The socket is private, messages are bounded to 64 KiB,
    frame reads have five-second deadlines, and the server is removed after the last session exits.


15. `--terminal` opens bash in the project VM and cannot be combined with an
    explicit command, reset or image rebuild. Concurrent sessions inherit current
    VM settings when options are omitted. Explicit git/network/ports and extra
    shares must agree with the active VM; conflicting requests and reset fail
    without stopping other sessions. Each client retains its own exec credentials.
    Mise bootstrap is locked, then released before foreground execution.


`.sandbox/launch.json` records the last successfully applied mounts, ports, git
mode, network-enabled flag, and host RPC socket path. The wrapper compares these
settings directly to skip unchanged updates and remove obsolete mappings when
changes are needed. It is generated runtime state, not a user configuration file.
For an RPC-enabled VM, a record missing the network flag receives one update.
VMs without the RPC socket mapping require explicit recreation because smolvm
1.29.1 cannot add socket bridges through update; no disks are deleted implicitly.
This checkout has no `.sandbox/config.json` parser or `vm-frontend/config-json.md`.

The `.sandbox/launch.json` fields retain their existing format. Joining sessions
read this record without changing it. Private `owner.lock` and `sessions.lock`
files implement supervisor ownership and shared OS session leases; they contain
no serialized session counters or credentials. `supervisor.log` contains VM
startup/shutdown diagnostics and `supervisor.error` reports owner failures.
`wrapper_pid` in status identifies this supervisor and stays constant while
sessions join/leave, even if the initial foreground wrapper exits.
