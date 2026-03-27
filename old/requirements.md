# Wrapper Capabilities Requirements

## 1. Launch and Command Behavior

1. The project shall provide two runnable wrappers: `codex-wrap` and `copilot-wrap`.
2. Each wrapper shall run its target CLI when no explicit command is provided.
3. The Codex wrapper shall automatically include the approvals/sandbox bypass flag unless already provided.
4. The Copilot wrapper shall automatically include `--allow-all` and `--no-auto-update` unless already provided.
5. The wrapper shall run the requested command inside the sandbox.

## 2. Isolation and Runtime Controls

1. The wrapper shall run commands in an isolated Bubblewrap sandbox.
2. The wrapper shall fail with a clear error when Bubblewrap is not available.
3. The wrapper shall provide network access by default and support disabling network access via `--no-net`.
4. The wrapper shall support both ephemeral and persisted sandbox home state via `--persist`.
5. The wrapper shall mount the core host runtime dependencies needed for common CLI execution.

## 3. Mount and Filesystem Capabilities

1. The wrapper shall mount a selected project directory as read-write, with any contained .git folder mounted as read-only.
5. The wrapper shall mount coding agent app home data as read-write by default.
6. The wrapper shall support Docker config and socket mounting (with --no-docker to disable).
7. The wrapper shall support mise data/state mounting and optional read-only mise config mounting.
8. The wrapper shall mount common local toolchain data directories for Cargo, Rustup, and Maven.
9. The wrapper shall expose host `~/bin` scripts and keep symlinked script targets usable inside the sandbox.

## 4. Environment and Credential Handling

1. The wrapper shall set sandbox-local HOME/XDG/app/tooling environment paths.
2. The wrapper shall reduce host credential/config leakage by overriding common credential-related environment paths.
3. The wrapper shall support explicit host environment passthrough via repeatable `--pass-env VAR`.
4. The Copilot wrapper shall pass through required GUI/session runtime environment values when available.
5. The Copilot wrapper shall pass through Copilot/GitHub token environment variables when present.
