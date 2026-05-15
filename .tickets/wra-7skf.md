---
id: wra-7skf
status: closed
deps: []
links: []
created: 2026-05-15T21:45:29Z
type: bug
priority: 0
assignee: Henrik Saksela
tags: [vm, guest, persistence, codex]
---
# Flush guest filesystem before killing VM after payload exit

In planb, Codex reportedly installed and launched successfully on the first run, but subsequent starts found a broken persisted install with zero-byte @openai/codex-linux-x64/package.json and qemu_status signal: 9. The wrapper currently calls RunningFrontend::terminate() after the primary payload returns, and terminate() immediately Child::kill()s QEMU. A successful npm install followed by SIGKILL can leave the persistent state.raw ext4 filesystem with recently-created tool files truncated or missing after reboot. This better explains 'worked once, broken next run' than assuming the initial install was bad.

## Design

Before killing QEMU after payload completion, ask the guest to flush filesystem state via the payload diagnostic side-channel (e.g. bounded 'sync') and only then terminate QEMU. Keep failure non-fatal but visible because a broken diagnostic channel should not hide the user's payload exit status. Consider a later graceful poweroff path, but the immediate fix should be minimal and testable.

## Acceptance Criteria

Wrapper launch with a payload runs a bounded guest sync before terminate. Unit tests cover that launch invokes the sync hook before terminate and tolerates sync failure. Live Codex/Pi setup-tool smoke should no longer leave zero-byte npm optional package metadata after payload exit and restart. ./vm-frontend/validate.sh required passes.


## Notes

**2026-05-15T21:47:51Z**

Implemented bounded guest filesystem flush before VM termination after primary payload completion. run_launch now sends a diagnostic 'sync' request over the payload side channel before RunningFrontend::terminate() SIGKILLs QEMU; failures warn but do not mask the payload exit status. Added tests for sync diagnostic request shape and fake-server verification. Live persistence smoke: fresh /tmp/agentvm-codex-persist-smoke installed @openai/codex, payload exited, VM was terminated, restart with --no-net ran codex --version successfully and @openai/codex-linux-x64/package.json remained 511 bytes.

**2026-05-15T21:48:12Z**

Validation passed: ./vm-frontend/validate.sh required completed successfully, including live-smoke. The live Codex persistence smoke also passed: install run printed codex-cli 0.130.0 and package.json size 511; restart without network printed the same.
