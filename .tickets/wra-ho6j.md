---
id: wra-ho6j
status: in_progress
deps: []
links: []
created: 2026-05-18T19:15:39Z
type: bug
priority: 0
assignee: Henrik Saksela
tags: [tui, payload, live]
---
# BUG: interactive payloads run without a guest TTY

Running a configured interactive CLI such as Codex through agentvm exits immediately with 'Error: stdin is not a terminal'. Reproduction from /home/hsaksela/Code/planb: .sandbox/config.json default_command is codex; launch agentvm in a PTY (for example script -qefc 'stty rows 24 cols 80; /home/hsaksela/ai/wrapper/target/debug/agentvm --project /home/hsaksela/Code/planb --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json' /tmp/agentvm-planb-script.log). The TUI briefly opens, Codex prints 'Error: stdin is not a terminal', and the wrapper exits with status 1. guest-service/src/lib.rs currently spawns the primary payload with Stdio::piped for stdin/stdout/stderr, so the guest process does not see a TTY even though the host wrapper is in TUI mode. Required validation did not catch this because live setup-tool tests run codex --version/plain commands and offline TUI tests use fake launch/no-qemu or shell commands; no live test asserts test -t 0 or runs an interactive TTY-requiring payload.

## Acceptance Criteria

Primary interactive payloads run under a guest PTY with requested rows/cols; Codex no-arg launch reaches an interactive UI instead of 'stdin is not a terminal'; add required/offline or live coverage that fails if the guest payload stdin is not a TTY (for example test -t 0/test -t 1 through the primary payload path, plus a TUI/PTY regression where practical).


## Notes

**2026-05-18T19:36:21Z**

Implemented guest-service primary payload PTY support in guest-service/src/lib.rs: primary sessions now open a pty, attach child stdin/stdout/stderr to the slave, set the controlling tty in pre_exec, stream master output, forward input to the pty, and apply RESIZE via TIOCSWINSZ. Added regression tests for test -t 0/1/2, requested stty size, and resize updates. Also added vm-frontend TUI tiny/zero size regressions for vt100 underflow. Offline guest-service and targeted vm-frontend tests pass. Full required gate currently fails before live because appliance artifacts are stale after guest-service/src/lib.rs changes; make release-appliance failed in this non-interactive harness because sudo requires a password/TTY. Need rerun sudo ./docker/build-appliance.sh (or make release-appliance) and then ./vm-frontend/validate.sh required on host.

**2026-05-18T19:37:26Z**

Added live self-test coverage in vm-frontend/src/self_test_payload.rs: required live-smoke payload now runs test -t 0/1/2 and emits self-test: tty-ok, so rebuilt-appliance live validation will fail on the old piped-primary behavior.

**2026-05-18T20:37:07Z**

Implemented guest-service primary payload PTY support earlier and added live self-test TTY assertions. Offline guest-service and vm-frontend validation-self-test suites pass. Required validation cannot reach live TTY assertion after current source changes until appliance is rebuilt; make release-appliance fails in this harness because sudo requires a TTY/password.
