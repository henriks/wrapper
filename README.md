# Persistent Codex microVM wrapper

Requirements: Python 3, git, smolvm 1.29.1 or newer, and a Linux host with KVM.
A missing default `sandbox-wrap.tar` is built from the Dockerfile at launch
and exported automatically using host Docker. Later launches reuse it. Host
Docker is used only to build/export the image; the workload uses its own
Docker daemon inside the VM.
The wrapper reuses its recorded VM configuration on unchanged launches; it runs
`smolvm machine update` only when mounts, ports, git mode, or networking change.

```sh
./codex-wrap --project /path/to/project
./codex-wrap --project /path/to/project --terminal  # attach a shell alongside Codex
./codex-wrap --port 8080:8080 --aws my-profile --gh
./codex-wrap --rebuild-image  # rebuild/export the default image, then exit
./codex-wrap --reset
```

Multiple wrapper invocations for the same project share one running VM. Start
Codex normally, then run `codex-wrap --terminal` from another terminal. Plain
`-- COMMAND` sessions share it too. Joining sessions inherit active machine
settings; explicit mount, git, port or network options must agree. Each session
keeps its own credentials and command. The VM stops only after the last session
exits; `--reset` is rejected while any session is active.

A small host supervisor owns the VM and status socket independently of the first
client. OS locks track active sessions, including clients killed with SIGKILL.
Startup/shutdown diagnostics are in `.sandbox/supervisor.log`. Guest tool
bootstrap is serialized; foreground commands run concurrently.

The guest starts from Debian bookworm-slim with mise and Docker. Engine, CLI,
Buildx and the `docker compose` plugin come from Docker's official stable apt
repository. Fresh image builds install the available stable packages. Node and Codex
are installed through mise on launch; put project toolchains in `mise.toml`.
Mise, HOME, installed packages, and Docker data persist in project-local
`.sandbox/` VM storage. The directory is hidden from the guest workspace.
Host `~/.codex` is shared writable so existing Codex authentication can be reused.
`--codex-home PATH` selects another host Codex directory.
Use `--image /absolute/path/to/image.tar` to override the default image archive;
custom archives and registry references are not built by the wrapper. To rebuild
the default image, run `codex-wrap --rebuild-image`. This command uses host Docker
and exits without starting or modifying a project VM. Existing VMs retain their
filesystem until `--reset`; reset itself does not build an image.
Existing bwrap `.sandbox/` directories must be moved aside before first launch.

Git metadata is read-only by default; `--git` enables writes. These mounts guard
against ordinary accidental writes. Guest Docker access grants guest root and
can bypass guest mount restrictions. Host directories explicitly shared writable,
including Codex state, are writable by the workload. The VM is not given the host
Docker socket or general host HOME access.

`--ro`/`--rw` accept directories. `--no-net` disables outbound networking;
`--port HOST:GUEST` publishes TCP ports. Guest localhost is separate from the host.
AWS/GitHub tokens and `--pass-env` variables use per-session secret injection.
`--aws` and `--gh` also provision the corresponding CLI through mise.
See [requirements.md](requirements.md) for the full behavior contract.

Validation:

```sh
./validate.sh offline
./validate.sh required  # includes image build and live VM scenarios
```

The historical `./vm-frontend/validate.sh required` mentioned in repository guidance
is absent in this checkout. `validate.sh` provides the gate for this Python port.
Live validation creates disposable test projects, avoids sharing real Codex state,
and checks filesystem persistence, Docker, git protection, masking, networking,
exit status, and terminal resize behavior. A real interactive Codex session and
AWS/GitHub authentication should also be checked with your accounts.

If an existing VM reports `failed to invoke ps` during Codex daemon startup, its
image predates the `procps` dependency. Updating the Dockerfile or rebuilding the
archive does not change that VM's persistent filesystem. Install the package in
the existing guest to preserve its HOME, toolchains, and Docker data. From the
project directory, with no wrapper session running:

```sh
SMOLVM_DATA_DIR="$PWD/.sandbox" smolvm machine start --name sandbox
SMOLVM_DATA_DIR="$PWD/.sandbox" smolvm machine exec --name sandbox --user root -- bash -ec 'apt-get update; apt-get install -y --no-install-recommends procps'
SMOLVM_DATA_DIR="$PWD/.sandbox" smolvm machine stop --name sandbox
codex-wrap
```

New VMs need an archive rebuilt from the updated Dockerfile. Run
`codex-wrap --rebuild-image` to rebuild/export it. `--reset` discards the entire project VM; it is
not needed to repair the missing package in an existing VM.

Inside the guest, `hostctl status` queries the running host wrapper through
`/run/sandbox-wrap/host.sock` and prints JSON with `state`, `machine`, `project`,
`wrapper_pid`, `network`, `git_writable`, and `published_ports`. The PID belongs to
the host supervisor, not a guest process. No credential values are returned. Status
works with `--no-net` and does not open a TCP port.

The wrapper listens on the project's `.sandbox/host.sock` (mode 0600 inside the
private state directory); smolvm forwards it into the guest. The server exists
while project sessions are active, reports `starting`, `running`, or `stopping`, and
removes its socket after the last session stops the VM. A later launch recreates
the server at the same path.

The wire protocol is JSON-RPC 2.0 with one UTF-8, newline-terminated request object
per connection. Send `{"jsonrpc":"2.0","id":1,"method":"status"}` followed by a
newline. `params` may be omitted or an empty object/array. Replies contain
`result` with the status object, or a JSON-RPC `error`. Notifications get no reply;
batches and methods other than `status` are unsupported. Messages are limited to
64 KiB, frame reads have a five-second deadline, socket writes time out after
five seconds, and connections are handled
sequentially so clients cannot create unbounded host threads.

To enable RPC in a pre-RPC VM, run `codex-wrap --rebuild-image`, then recreate the
project VM with `codex-wrap --reset` and `codex-wrap`. **Reset deletes that VM's
persisted guest data.** Smolvm 1.29.1 can configure socket forwarding at creation
but cannot add it through `machine update`; the wrapper reports this rather than
silently recreating a VM. Native updates are tracked in `wra-adhj`.
