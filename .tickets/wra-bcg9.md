---
id: wra-bcg9
status: closed
deps: []
links: []
created: 2026-05-15T21:42:44Z
type: bug
priority: 1
assignee: Henrik Saksela
tags: [vm, guest, tooling, codex, npm]
---
# Repair broken cached agent CLI installs on bootstrap

After a successful first Codex install, subsequent starts in /home/hsaksela/Code/planb skipped bootstrap because command -v codex succeeded, then codex immediately failed and the wrapper returned to shell. Inspection showed /home/hsaksela/.local/bin/codex existed, but codex --version failed with 'Missing optional dependency @openai/codex-linux-x64' because the optional package's package.json/README were zero bytes. A forced reinstall reported npm tar cache corruption, refreshed the cache, restored package.json to 511 bytes, and codex --version worked. Current setup_tool_payload_script only checks command existence, so it cannot repair broken/incomplete cached tool installs.

## Design

Change setup tool bootstrap to validate an existing CLI with a cheap health command before skipping install. For npm-backed agent CLIs, use --version as the health check. If command is absent or health check fails, run npm install --global --force --no-progress <package>@latest so corrupt npm cache/package state is refreshed. Keep the script narrow and deterministic; do not add broad defensive machinery.

## Acceptance Criteria

A stale/broken codex binary on PATH triggers reinstall instead of exec failure. A healthy codex install skips reinstall. Tests assert bootstrap script checks command health and uses forced npm reinstall on failed health. Live validation in planb or a temp project demonstrates a repaired codex --version. ./vm-frontend/validate.sh required passes.


## Notes

**2026-05-15T21:43:24Z**

Implemented bootstrap health check in setup_tool_payload_script: existing CLI is accepted only if command -v succeeds and '<cli> --version' exits successfully. Otherwise bootstrap runs npm install --global --force --no-progress <package>@latest, which repairs corrupt npm cache/incomplete optional package installs. Updated setup_tool tests to assert version health check and forced npm install. Live diagnosis in /home/hsaksela/Code/planb: codex existed but codex --version failed because @openai/codex-linux-x64/package.json was zero bytes; forced reinstall refreshed corrupt tar cache and restored codex-cli 0.130.0.

**2026-05-15T21:43:46Z**

Final validation passed: ./vm-frontend/validate.sh required completed successfully, including live-smoke. Note: planb state was repaired manually during diagnosis with npm install --global --force --no-progress @openai/codex@latest; future starts now also run the health-check bootstrap and would repair the same broken state automatically.

**2026-05-15T21:45:33Z**

User correctly pointed out Codex launched successfully on first run. That makes the zero-byte optional dependency observed later evidence of post-run persistence/state corruption, not a failed initial install. Root-cause candidate: run_launch calls RunningFrontend::terminate(), which SIGKILLs QEMU immediately after payload exit; qemu_status in planb was signal: 9. A successful npm install followed by QEMU SIGKILL can plausibly leave state.raw with truncated recently-created files. Health check reinstall is useful recovery but not the primary fix; tracking root fix in wra-7skf.

**2026-05-15T21:48:12Z**

Closing again as recovery hardening only. Root cause for the user's 'worked first time, broken after restart' is now tracked/fixed in wra-7skf: flush guest filesystems before SIGKILL shutdown. The health-check reinstall remains as a repair path for already-broken cached installs.
