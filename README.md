# Persistent Codex microVM wrapper

Requirements: Python 3, git, smolvm 1.29.1 or newer, Linux 5.12+ with KVM,
and util-linux 2.39+ with unprivileged user namespaces enabled.
A missing default `sandbox-wrap.tar` is built from the Dockerfile at launch
and exported automatically using host Docker. Later launches reuse it. Host
Docker is used only to build/export the image; the workload uses its own
Docker daemon inside the VM.
Outbound access defaults to an allowlisted CONNECT proxy.
The wrapper reuses its recorded VM configuration on unchanged launches; it runs
`smolvm machine update` only when mounts, ports, git mode, or networking change.

```sh
./codex-wrap --project /path/to/project
./codex-wrap --project /path/to/project --entrypoint bash  # attach a shell alongside Codex
./codex-wrap --entrypoint bash -- -lc 'pwd'  # pass arguments to the chosen program
./codex-wrap --port 8080:8080 --aws my-profile --gh
./codex-wrap --rebuild-image  # rebuild/export the default image, then exit
./codex-wrap --reset
```

Multiple wrapper invocations for the same project share one running VM. Start
Codex normally, then run `codex-wrap --entrypoint bash` from another terminal. Plain
`-- COMMAND` sessions share it too. Joining sessions inherit active machine
settings; explicit mount, git, port or network options must agree. Each session
keeps its own credentials and command. The VM stops only after the last session
exits; `--reset` is rejected while any session is active.

A small host supervisor owns the VM and status socket independently of the first
client. OS locks track active sessions, including clients killed with SIGKILL.
Startup/shutdown diagnostics are in `.sandbox/supervisor.log`; supervisor failures
also print the log to the client's terminal. Guest tool
installation is serialized; sessions with installed tools proceed directly
to their commands, which run concurrently.

The guest starts from Debian bookworm-slim with mise and Docker. Engine, CLI,
Buildx and the `docker compose` plugin come from Docker's official stable apt
repository. Fresh image builds install the available stable packages. Node and Codex
are installed through mise when launching `codex` and reused on later launches;
put project toolchains in `mise.toml`. Generic commands and shells do not install
Node or Codex, so they work offline when their requested project tools are already
available. To use Codex from a shell on a fresh VM, first run
`codex-wrap --entrypoint codex -- --version`.
Mise, HOME, installed packages, and Docker data persist in project-local
`.sandbox/` VM storage. The directory is hidden from the guest workspace.
Host `~/.codex` is shared writable so existing Codex authentication can be reused.
`--codex-home PATH` selects another host Codex directory.
Use `--image /absolute/path/to/image.tar` to override the default image archive;
custom archives and registry references are not built by the wrapper. To rebuild
the default image, run `codex-wrap --rebuild-image`. This command uses host Docker
and exits without starting or modifying a project VM. Existing VMs retain their
filesystem until `--reset`; reset itself does not build an image.
If an older image lacks the guest proxy, startup reports it before launching
tools. Run `codex-wrap --rebuild-image`, then `codex-wrap --reset` to recreate
the VM from the updated image. Reset deletes the guest filesystem and Docker data.
This also applies to changes in the guest runner: rebuilding updates the archive
for new VMs, while existing VMs retain their installed runner and tools.
Existing bwrap `.sandbox/` directories must be moved aside before first launch.

Every shared directory is exported through a private host filesystem view. Host
mounts hide `.sandbox/`, protect the lifecycle lock and enforce read-only Git
metadata unless `--git` is selected. The same policy applies through overlapping
or bind-aliased exports, including extra shares and linked worktrees. Conflicting
writable shares inside a read-only subtree are rejected. Read-only `.git`
symlinks are rejected; ordinary Git directories and worktree files are supported. Docker grants guest
root, but guest remounts do not change these host mount restrictions. Host files
explicitly shared writable, including Codex state, remain writable by the workload. These are path-level
mount controls: host-created hard links and deliberately shared writable files
remain trusted.

Install the command symlink from the checkout:

```sh
./setup
```

This creates `codex-wrap` in `~/.local/bin`, or `/usr/local/bin`
when run as root. Use `--bin-dir PATH` to choose another directory. The symlink
points to this checkout, which must stay in place. Setup reports if the directory
needs adding to `PATH`.

Setup checks the host's AppArmor settings rather than its distribution. When
AppArmor restricts unprivileged user namespaces, it locates the host's `unshare`,
copies it to `/usr/local/libexec/sandbox-wrap-unshare`, installs the dedicated
profile in `/etc/apparmor.d/`, and loads it with `apparmor_parser`. Only these
system changes use `sudo` when needed; missing tools produce an actionable error.
It does not install packages or disable host restrictions. Re-run setup after
updating util-linux to refresh the helper binary. Hosts without this AppArmor
restriction need only the command symlink and use `unshare` directly when no
dedicated helper is installed.
Normal launches run without sudo. Ubuntu profile behavior still needs validation
on Ubuntu; the implementation and live scenarios were exercised on Debian 13.

The supervisor and VMM own the private mount namespace. Attached clients use the
same running VM; their exit cannot remove another session's views. Kernel
namespace teardown cleans up the views after the last owning process exits.
Do not start protected machines directly with `smolvm machine start`: their
export views are prepared by the wrapper. Existing stopped machines receive a
single mount update without losing their guest disks; old active sessions must
finish before host protection can be enabled.

`--ro`/`--rw` accept directories. `--no-net` disables outbound networking;
`--port HOST:GUEST` publishes TCP ports. Guest localhost is separate from the host.
AWS/GitHub tokens and `--pass-env` variables use per-session secret injection.
`--aws` and `--gh` also provision the corresponding CLI through mise.
Guest-owned Git, AWS and Kubernetes configuration uses the persistent guest HOME.
Host configuration and the host SSH agent are not forwarded automatically;
`--pass-env` remains an explicit environment override.
See [requirements.md](requirements.md) for the full behavior contract.

Validation:

```sh
./validate.sh offline
python3 tests/live_host_mounts.py --mount-only  # real host mounts, without a VM
./validate.sh required  # includes image build and live VM scenarios
```

Live validation creates disposable test projects, avoids sharing real Codex state,
and checks filesystem persistence, Docker, git protection, masking, networking,
exit status, and terminal resize behavior. A real interactive Codex session and
AWS/GitHub authentication should also be checked with your accounts.

Older guest filesystems missing `procps` can be repaired without resetting.
Start `codex-wrap --raw-net --entrypoint bash` and keep the shell open. In another host
terminal in the same project, run:

```sh
SMOLVM_DATA_DIR="$PWD/.sandbox" smolvm machine exec --name sandbox --user root -- bash -ec 'apt-get update; apt-get install -y --no-install-recommends procps'
```

Exit the shell and launch Codex normally. New guest images already include this
package; rebuilding an archive does not replace an existing guest filesystem.

Inside the guest, `hostctl status` queries the running host wrapper through
`/run/sandbox-wrap/host.sock` and prints JSON with `state`, `machine`, `project`,
`wrapper_pid`, `network`, `proxy`, `git_writable`, and `published_ports`. The PID belongs to
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

## Network access

By default, outbound access uses the built-in CONNECT proxy with a domain
allowlist for Codex/OpenAI, mise/Node/npm, GitHub downloads, PyPI and Docker Hub.
Direct VM egress is disabled. Choose broader access explicitly:

```sh
codex-wrap --unfiltered-net  # any public CONNECT destination; IP checks remain
codex-wrap --raw-net         # direct networking, without proxy enforcement
codex-wrap --no-net          # no external access
codex-wrap --help-config     # config format, defaults and override rules
```

Persist per-project rules in `.sandbox/config.json`:

```json
{"network": {"allow": ["api.openai.com", "chatgpt.com"], "deny": []}}
```

`allow` replaces the built-in list, so include bootstrap endpoints when using a
fresh VM. Repeatable `--allow-domain` / `--deny-domain` options replace only their
corresponding configured list. CLI options never change the config file.
Configuration can also explicitly persist unfiltered or raw access.

Deny wins. `--reset` preserves configuration. Existing VMs need a rebuilt image
and explicit reset to add the proxy bridge, or an explicit `--raw-net` override.
See [config-json.md](config-json.md) for the exact default domains, shared-session
behavior, and container proxy configuration.
