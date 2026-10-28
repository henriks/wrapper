# Sandbox wrapper requirements

The wrapper runs Codex, or an explicitly supplied command, in a persistent
project-specific smolvm machine. Linux hosts require KVM. The guest image is built
from the checked-in Debian slim Dockerfile; host system directories are not shared.

1. `codex-wrap` and `sandbox-wrap` invoke the same Python script. Arguments after
   `--` replace the default `codex --dangerously-bypass-approvals-and-sandbox` command.
   With `--entrypoint PROGRAM`, those arguments are passed to PROGRAM instead.
   The executable and arguments are forwarded literally, without shell parsing.
2. `--project PATH` selects the workspace, defaulting to the current directory.
   It is shared writable at its canonical absolute host path.
3. `.sandbox/` contains smolvm's registry and persistent guest disks, selected
   using `SMOLVM_DATA_DIR`. HOME is `/home/agent` in the persistent root filesystem.
   `/tmp` and `/run` remain ephemeral. A project lifecycle lock serializes
   startup, configuration and reset. Host filesystem views require Linux 5.12+,
   util-linux 2.39+ and permitted user namespaces (Ubuntu may need the shipped
   helper/AppArmor installer).
   Concurrent foreground sessions share the VM, which stops after the last exits.
4. All host directory shares use protected views prepared in a private host
   user/mount namespace. `.sandbox/` is excluded, the project lifecycle lock is
   covered read-only, and Git metadata is read-only unless `--git` is selected,
   including the `.git` file and external metadata of linked worktrees. Read-only
   policy includes nested filesystems and applies through every export/alias.
   Conflicting RW shares inside RO subtrees and exports of control state are
   rejected. Policies are enforced outside the guest and cannot be changed by
   guest root through unmounts, remounts, alternate binds or parent renames.
   The wrapper verifies kernel read-only flags before starting the VM.
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
8. Outbound access defaults to allowlisted CONNECT proxy access.
   `--unfiltered-net` permits any public CONNECT destination; `--raw-net` enables
   direct networking without proxy enforcement; `--no-net` disables external access. `--port HOST:GUEST`
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
12. `--reset` deletes the project VM and runtime state, preserving `.sandbox/config.json`. Existing bwrap state is not
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
    direct networking, proxy mode, git access, and published ports. No credentials or host execution
    methods are exposed. The socket is private, messages are bounded to 64 KiB,
    frame reads have five-second deadlines, and the server is removed after the last session exits.


15. `--entrypoint PROGRAM` selects a program to run in the project VM, for example
    `--entrypoint bash` for a shell. It cannot be combined with reset or image rebuild.
    Concurrent sessions inherit current
    VM settings when options are omitted. Explicit git/network/ports and extra
    shares must agree with the active VM; conflicting requests and reset fail
    without stopping other sessions. Each client retains its own exec credentials.
    Mise bootstrap is locked, then released before foreground execution.


`.sandbox/launch.json` records the last successfully applied mounts, ports, git
mode, network-enabled flag, and host RPC socket path. The wrapper compares these
settings directly to skip unchanged updates and remove obsolete mappings when
changes are needed. It is generated runtime state, not a user configuration file.
For an RPC-enabled VM, a record missing the network flag or effective proxy policy receives one update.
VMs without the RPC socket mapping require explicit recreation because smolvm
1.29.1 cannot add socket bridges through update; no disks are deleted implicitly.
Optional user configuration is documented in [config-json.md](config-json.md).

The `.sandbox/launch.json` fields retain their existing format. Joining sessions
read this record without changing it. Private `owner.lock` and `sessions.lock`
files implement supervisor ownership and shared OS session leases; they contain
no serialized session counters or credentials. `supervisor.log` contains VM
startup/shutdown diagnostics and `supervisor.error` reports owner failures.
`wrapper_pid` in status identifies this supervisor and stays constant while
sessions join/leave, even if the initial foreground wrapper exits.

16. Default proxy mode is implemented by the Python asyncio/socket module
    `network_proxy.py`. The host enforces public-IP checks and per-sandbox domain
    ACLs; VM outbound networking is disabled. Guest applications and dockerd use
    a TCP-to-Unix relay through a private smolvm socket bridge. Policies are read
    from `.sandbox/config.json`, with independent CLI allow/deny overrides and
    immutable snapshots for shared sessions. See [config-json.md](config-json.md)
    for modes, ACL syntax, deadlines, Docker use, and limits.

17. `.sandbox/launch.json` retains logical mount arguments and adds a generated
    `host_mounts` layout marker. smolvm receives stable indexed paths under a
    private `.sandbox/exports` tmpfs; unchanged launches skip updates. The staging
    tmpfs is unbindable so recursive shares cannot copy it into themselves.
    `.sandbox/export-mask` supplies the empty read-only lifecycle-lock mask.
    Existing raw directory exports are replaced once without resetting guest
    disks. Guest preparation streams the current host-owned script over stdin;
    it no longer sets filesystem policy inside the guest. Namespace lifetime
    owns cleanup, including after foreground clients exit or the supervisor fails.
