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

All durable project state lives under `.sandbox/`.

```text
.sandbox/
  config.json
  home/
  docker-vm/
```

`.sandbox/config.json` is the durable source of truth for configurable sandbox
behavior: default command, setup recipe, tool state, network mode, network
allowlists, auth sharing, extra shares, and published guest ports. Normal launch
flags are one-run overrides and do not persist.

The TUI may write `config.json` only after an explicit user action such as
accepting setup or saving edits. A configured project must not ask setup
questions again just because `agentvm` was restarted.

## First Run

Known agent setup is explicit:

```sh
agentvm --setup-tool codex
agentvm --setup-tool pi
```

`--setup-tool codex` writes a Codex-oriented `config.json`: default command
`codex`, Codex writable state sharing, the package/bootstrap metadata needed to
install the CLI in the guest, and the normal project workspace and guest home
shares.

`--setup-tool pi` writes a Pi-oriented `config.json`: default command `pi`,
package metadata for `@mariozechner/pi-coding-agent`, and the normal sandbox
shares.

Interactive setup can be offered by the TUI when no config exists. Non-TTY mode
must fail with an actionable message rather than prompting.

## Normal Launch

Once configured, the primary workflow is:

```sh
agentvm
```

The wrapper reads `.sandbox/config.json`, starts the project VM, applies the
configured network/auth/share policy, and runs the configured default command.

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

Extra shares are structured records with host path, guest path, access mode, and
whether the source is required. CLI `--ro PATH` and `--rw PATH` expose the path
at the same absolute guest path for one launch.

The default guest home is project-local `.sandbox/home/` mounted at the host
user's natural home path inside the guest. Tool state is shared deliberately by
recipe/config, not by exposing the whole host home.

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

Reset removes `.sandbox/`, including `config.json`, guest home, Docker state,
runtime logs, and tool state. It must refuse while a project VM lock is held.

## Unresolved Decisions

- The exact TUI editor layout is still open, but it must write the same
  structured config schema used by CLI setup recipes.
- Removed wrapper flags `--tool`, `--tool-arg`, and `--command` must stay removed;
  setup belongs in `--setup-tool` or config, and one-run commands belong after `--`.
- The final package manager command for guest tool bootstrap should be verified
  per recipe; current recipes use npm package metadata.
