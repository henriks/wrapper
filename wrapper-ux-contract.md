# Agent VM Wrapper UX Contract

This document is the user-facing contract for the project sandbox wrapper. It
supersedes the wrapper-facing CLI/state sections in `requirements.md`,
`docker/runtime-contract.md`, and `vm-frontend/tui-design.md`; those documents
remain implementation and validation references.

## Product Shape

`agentvm` is the primary user command. It starts the configured sandbox for the
current project, launches the configured default command inside the VM, and uses
the TUI when stdin/stdout are interactive terminals.

`agentvm-frontend` is a low-level implementation/debug binary. Its `prepare`,
`launch`, `self-test`, `vmnet-gateway`, and `payload-client` subcommands are not
the normal user workflow.

The wrapper is built around these user jobs:

- set up a project once for a known agent
- start the configured project without repeated prompts
- temporarily override the command, network, auth, or shares for one launch
- inspect or edit project sandbox configuration
- reset project-local sandbox state
- fall back to plain streaming for non-TTY and scripted use

## Project State

AgentVM-owned sandbox state lives under `.sandbox/`; setup-tool installation declarations live in the project-root `mise.toml` so normal mise discovery works. Optional guest-mirrored service logs use `.vmlogs/` only when `--mirror-guest-logs` is passed.

```text
mise.toml
.sandbox/
  config.json
  docker-vm/
.vmlogs/        # optional, only with --mirror-guest-logs
```

`.sandbox/config.json` is the durable source of truth for configurable sandbox
behavior: default command, setup recipe, tool state, network mode, network
allowlists, auth sharing, extra shares, and published guest ports. Its full file
format is documented in `vm-frontend/config-json.md`. Normal launch flags are
one-run overrides and do not persist. The guest workspace hides `.sandbox/` so
implementation state is not exposed inside the VM. Guest service log mirroring is
off by default; `--mirror-guest-logs` enables guest writes under project
`.vmlogs/` for debugging.

The TUI may write `config.json` only after an explicit user action such as
saving edits. A configured project must not ask setup questions again just
because `agentvm` was restarted.

## First Run

Known agent setup is explicit:

```sh
agentvm --setup-tool codex
agentvm --setup-tool pi
```

`--setup-tool codex` is a convenience operation: it writes a Codex-oriented
`config.json` with default command `codex --dangerously-bypass-approvals-and-sandbox`,
explicit writable `~/.codex` share configuration with project-local shadow
backing for volatile children, and writes project-root `mise.toml` declaring
the Codex npm tool.

`--setup-tool pi` likewise writes a Pi-oriented `config.json` with default
command `pi`, explicit writable `~/.pi` share configuration, and writes
project-root `mise.toml` declaring `@mariozechner/pi-coding-agent`.

If no config or command override exists, `agentvm` launches `bash` without
persisting config. Known agent setup remains opt-in through `--setup-tool` or
explicit config editing.

## Normal Launch

Once configured, the primary workflow is:

```sh
agentvm
```

The wrapper reads `.sandbox/config.json`, starts the project VM, applies the
configured network/auth/share policy, and runs the configured default command.
If no config exists and no command override was supplied, it starts `bash`.

`agentvm-frontend wrap` remains a compatibility path for development, but help
and user docs should prefer `agentvm`.

## Command Override

The canonical one-run command override is post-separator:

```sh
agentvm -- bash -l
agentvm -- sh -lc 'npm test'
```

Arguments after `--` are the full payload command for that launch. They do not
modify `config.json`.

When no command follows `--`, the configured default command is used.

## One-Run Overrides

Launch flags override the loaded config for one invocation only:

```sh
agentvm --no-net
agentvm --gh
agentvm --aws dev
agentvm --ro /opt/sdk --rw /tmp/work
agentvm --docker-publish 18080:8080
```

These flags must not persist unless the user saves equivalent changes through a
setup or config-editing flow.

## Network

Network mode is configured as one of:

- `public`: allow normal public egress
- `none`: deny guest egress while preserving wrapper control channels
- `allowlist`: allow only configured domains, hosts, or IP/CIDR entries

Configured allowlists live in `config.json`. `--no-net`, `--allow-domain`, and
`--allow-ip` are one-run overrides.

## Auth And Shares

Auth sharing is explicit. GitHub and AWS support are named config fields and
one-run flags, not arbitrary host environment passthrough.

Extra shares are structured records with host path, guest path, access mode,
whether the source is required, and optional child `shadows`. A shadow names a
relative child path inside a configured share that should be backed by
read-write project-local `.sandbox/root/<full guest path>` state instead of the
corresponding host child path.
CLI `--ro PATH` and `--rw PATH` expose the path at the same absolute guest path
for one launch.

The default guest home is the host user's natural home path inside the guest and
lives on the persistent VM root overlay unless a configured share covers that
path or one of its children. Tool state is shared deliberately by recipe/config,
not by exposing the whole host home.

## TUI Contract

Interactive launches use the TUI by default. `--no-tui` forces plain streaming.
Non-TTY stdin or stdout automatically selects plain mode.

The TUI should eventually view and edit all relevant `config.json` fields:
default command, setup recipe, network mode and allowlists, auth sharing,
additional shares, published ports, and reset/reinitialize choices.

## Reset

```sh
agentvm --reset
```

Reset removes `.sandbox/`, including `config.json`, Docker state, runtime logs,
and project-local share shadows. It does not remove project-root `mise.toml`.
It must refuse while a project VM lock is held.

## Unresolved Decisions

- The exact TUI editor layout is still open, but it must write the same
  structured config schema used by CLI setup recipes.
- Removed wrapper flags `--tool`, `--tool-arg`, and `--command` must stay removed;
  setup belongs in `--setup-tool` or config, and one-run commands belong after `--`.
- The final package manager command for guest tool bootstrap should be verified
  per recipe; current recipes use npm package metadata.
