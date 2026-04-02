# Sandbox Wrapper Requirements (v2)

## Overview

A Python-based wrapper that runs AI coding agents in a project-scoped VM.
Sandbox state is persistent and project-local, stored in a `.sandbox/`
directory within the project. Toolchains are managed by mise and installed
within project-local sandbox state.

## Current Target Architecture

The implementation is being rewritten toward a VM-only execution model.

Current target rules:

1. The wrapper's primary execution model is a project-scoped VM, not
   Bubblewrap.
2. The selected tool command runs inside the guest.
3. Docker runs inside the same guest and is available there by default.
4. The host wrapper is a lifecycle/orchestration layer only.
5. Where older sections in this document still reflect the Bubblewrap-era
   design, this section and `docker/runtime-contract.md` take precedence until
   the docs cleanup ticket completes.

---

## 1. Entry Points and Invocation

1. The project shall consist of a single Python script.
2. Two symlinks shall point to the script: `codex-wrap` and `copilot-wrap`.
3. The script shall determine its mode from `argv[0]`:
   - Invoked as `codex-wrap` → Codex mode.
   - Invoked as `copilot-wrap` → Copilot mode.
4. A `--tool codex|copilot` flag shall override the invocation-name detection.
5. In Codex mode, the script shall run the Codex CLI and automatically include
   `--dangerously-bypass-approvals-and-sandbox` unless already present in the
   user-supplied arguments.
6. In Copilot mode, the script shall run the Copilot CLI with appropriate
   default flags (e.g. `--allow-all`, `--no-auto-update`).
7. Arguments after `--` or unrecognized positional arguments shall be passed
   through to the target CLI.
8. `--project PATH` shall set the project directory (default: `$PWD`).
9. Copilot support is secondary; if it proves difficult the initial release may
   be Codex-only.

---

## 2. Sandbox State and Persistence

1. All mutable sandbox state shall be stored in `.sandbox/` within the project
   directory.
2. `.sandbox/` shall be created automatically on first run.
3. `.sandbox/home/` shall serve as the persistent guest `$HOME`. Tool-local
   state (mise toolchains, caches, configs, and similar per-user data) shall
   live under that path.
4. `.sandbox/docker-vm/` shall contain the VM runtime state, lock file, logs,
   and persistent Docker data disk.
5. `--reset` shall remove the entire `.sandbox/` directory so the next run
   starts fresh, except that it must fail if an active Docker VM lock is held
   for the project.
6. On first run, the script shall append `.sandbox/` to the project's
   `.gitignore` if not already present.

---

## 3. Isolation and Namespace Setup

1. The wrapper shall use the VM as the primary isolation boundary.
2. The wrapper shall fail with a clear error if required VM host tools are not
   available.
3. Network access shall be enabled for the guest by default; `--no-net` shall
   disable guest egress.
4. The selected agent payload shall run inside the guest, not under a separate
   host-side namespace sandbox.

---

## 4. Filesystem Layout Inside the Sandbox

### 4.1 Host System Directories (Read-Only)

1. The following shall be bind-mounted read-only from the host:
   `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64` (if it exists).
2. Selective `/etc` entries shall be mounted read-only: `resolv.conf`, `hosts`,
   `passwd`, `group`, `ssl`, `ca-certificates`, `pki`,
   `java*` directories, `mavenrc`.
3. `/proc` shall be mounted via `--proc`.
4. `/dev` shall be mounted via `--dev`.

### 4.2 Project Directory

1. The project directory shall be mounted read-write at its real host path.
2. `.git` within the project shall be mounted read-only (overriding the
   read-write mount), preventing the agent from modifying git state.
3. `.sandbox` within the project shall be hidden by a tmpfs overlay.

### 4.3 Persistent Sandbox State (from `.sandbox/`)

1. `$HOME` inside the guest shall map to `.sandbox/home/` on the host.
2. Mise directories use their default XDG-relative paths under that guest
   home, so they naturally reside within `.sandbox/home/`.

### 4.4 Ephemeral Mounts

1. `/tmp` inside the sandbox shall map to a project-specific directory under
   the host's `/tmp` (e.g. `/tmp/sandbox-<hash>/`). This is ephemeral
   (cleared on host reboot) but persists across sandbox restarts within a
   session.
2. `/var` shall be a tmpfs.

### 4.5 Optional / User-Specified Mounts

1. The VM shall always provide the project workspace inside the guest.
2. The VM shall provide Docker inside the guest as part of the default model.
3. `--docker-publish HOST:GUEST` shall expose a guest TCP port on
   `127.0.0.1:HOST` while the VM is running.
4. `--ro PATH` shall expose an arbitrary host path read-only inside the guest
   at the same absolute path.
5. `--rw PATH` shall expose an arbitrary host path read-write inside the guest
   at the same absolute path.
6. `--pass-env` is still being removed as part of the VM-only rewrite.

---

## 5. Toolchain Management via Mise

1. Before launching the sandbox, the script shall check for `mise.toml` or
   `.mise.toml` in the project directory.
2. If no mise config exists in the project, the script shall create
   `mise.toml` with default tool entries:
   ```toml
   [tools]
   node = "24"
   python = "latest"
   ```
3. The script shall maintain a **separate** mise config for sandbox-specific
   tool entries (e.g. the AI tool npm package) at
   `.sandbox/home/.config/mise/config.toml` (the guest HOME's standard mise
   config path). The project's own mise config is never modified by the
   script for these entries.
4. The sandbox mise config shall contain the selected AI tool's entry:
   - Codex: `"npm:@openai/codex" = "latest"`
   - Copilot: equivalent entry (TBD)
5. Mise shall be configured to read **both** the project's mise config and
   the guest HOME's mise config. Since mise reads configs from both the
   project directory and `$MISE_CONFIG_DIR` by default, this should work
   naturally with `$HOME` set to the persistent guest home.
6. Since `$HOME` maps to `.sandbox/home/`, mise's default XDG-relative
   data/state directories (`$HOME/.local/share/mise`, etc.) naturally reside
   within `.sandbox/home/` — no explicit `MISE_DATA_DIR`/`MISE_STATE_DIR`
   overrides are needed.
7. On each sandbox start, `mise install` shall be run inside the guest before
   launching the target CLI. This is idempotent and typically a no-op after
   the first run.
8. The guest `$PATH` shall include mise shims / tool binary paths so
   installed tools are available to the agent.
9. The mise binary itself must be accessible inside the guest.

---

## 6. AWS Credential Handling

1. When `--aws PROFILE` is provided, the script shall acquire temporary
   credentials before launching the sandbox by invoking
   `aws sts get-session-token` (or `assume-role` as appropriate) using the
   named profile on the host.
2. The resulting temporary credentials shall be injected as environment
   variables inside the sandbox:
   - `AWS_ACCESS_KEY_ID`
   - `AWS_SECRET_ACCESS_KEY`
   - `AWS_SESSION_TOKEN`
   - `AWS_DEFAULT_REGION` (from the profile config, if available)
3. Host AWS configuration (`~/.aws`) shall NOT be mounted into the sandbox.
4. Without `--aws`, AWS credential environment variables and config paths
   shall be explicitly nullified to prevent leakage.

---

## 7. Environment Handling

1. The following environment variables shall be set inside the sandbox:
   - `HOME` — sandbox home path
   - `USER`, `LOGNAME` — current username
   - `TERM` — inherited from host
   - `LANG` — inherited from host (or a sane default)
   - `PATH` — constructed to include: mise shims, sandbox home bin, and
     standard system paths.
2. XDG base directories shall point to sandbox-local paths:
   - `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`
3. The following shall be explicitly nullified to prevent host credential
   leakage:
   - `SSH_AUTH_SOCK`
   - `GIT_CONFIG_GLOBAL` → `/dev/null`
   - `AWS_SHARED_CREDENTIALS_FILE` → `/dev/null`
   - `AWS_CONFIG_FILE` → `/dev/null`
   - `GOOGLE_APPLICATION_CREDENTIALS`
   - `KUBECONFIG` → `/dev/null`
4. Arbitrary host environment passthrough is not part of the VM-only contract.
   Any future passthrough support should return as an explicit VM-era feature.

---

## 8. Git Access

1. `.git` shall be mounted read-only, allowing the agent to read repository
   state (log, diff, status, blame, etc.).
2. Git write operations (commit, push, checkout, etc.) shall fail due to the
   read-only mount.
3. `GIT_CONFIG_GLOBAL` shall be set to `/dev/null` to prevent host git config
   leakage.

---

## 9. CLI Summary

```
codex-wrap [OPTIONS] [-- EXTRA_ARGS...]
copilot-wrap [OPTIONS] [-- EXTRA_ARGS...]

Options:
  --project PATH        Project directory (default: $PWD)
  --tool codex|copilot  Explicit tool selection (overrides argv[0])
  --no-net              Disable network access
  --docker-publish H:G  Expose a guest TCP port on localhost
  --aws PROFILE         Acquire temporary AWS credentials via STS
  --ro PATH             Extra read-only guest share (repeatable)
  --rw PATH             Extra read-write guest share (repeatable)
  --reset               Remove .sandbox/ and start fresh
  --help                Show usage
```

---

## 10. Non-Goals (Current Scope)

1. GUI/display passthrough (DISPLAY, WAYLAND, DBUS) is not required.
2. Persistent sandbox home at an arbitrary path (`--persist PATH`) is replaced
   by the fixed project-local `.sandbox/` approach.
3. Host `~/.cargo`, `~/.rustup`, `~/.m2` are NOT directly mounted; these
   toolchains are managed by mise inside the guest instead.
4. Arbitrary environment passthrough is not included by default.
