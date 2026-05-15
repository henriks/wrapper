---
id: wra-7xi3
status: closed
deps: []
links: [wra-8xjs, wra-13x7]
created: 2026-05-15T21:13:38Z
type: feature
priority: 1
assignee: Henrik Saksela
tags: [vm, guest, payload, debuggability]
---
# Support concurrent diagnostic exec sessions in running VM

Current guest payload server explicitly supports only one payload at a time. docker/guest-payload-server.py says 'Run one guest payload at a time' and handle_client uses a single session_lock; if a user-facing payload is stuck, payload-client diagnostics fail with 'payload session already active'. This blocks debugging of real setup-tool stalls such as /home/hsaksela/Code/planb agentvm --setup-tool codex, where the Codex bootstrap appeared stuck after npm registry metadata fetches and a diagnostic payload could not be started. Relevant code: docker/guest-payload-server.py, vm-frontend/src/payload_client.rs, vm-frontend/src/main.rs integrated launch/payload flow, vmnet host-ingress payload listener.

## Design

Clarify semantics before implementing broad concurrency. Preferred minimal design: keep one primary interactive payload session for the user, but add a separate bounded diagnostic exec channel/session class that can run non-interactive commands concurrently for observability (ps, logs, env, npm/mise state), with clear timeouts and output limits. Alternative/full design: make payload protocol multi-session with session IDs, independent PTYs/process groups, per-session lifecycle/signal/resize, and policy limiting concurrent sessions. Avoid letting diagnostics mutate user state unless explicitly requested. Expose this through an agentvm debug/exec command or payload-client flag that reports active primary session status instead of failing opaquely.

## Acceptance Criteria

While a long-running primary payload is active, a host command can execute a bounded diagnostic command in the same guest and return output. Existing single-session interactive behavior remains stable. Diagnostics have configurable but safe timeout/output limits and cannot accidentally steal stdin/stdout or signals from the primary session. Logs identify session type/id and active payload PID/command. Tests cover concurrent primary+diagnostic sessions, active-session reporting, timeout cleanup, and rejection/limits for too many diagnostics. Live validation demonstrates diagnosing a stuck setup-tool bootstrap without killing the user payload.


## Notes

**2026-05-15T21:19:08Z**

Implemented first cut of concurrent diagnostics rather than full multi-interactive sessions. docker/guest-payload-server.py now accepts D diagnostic request frames alongside the existing single primary R payload; primary remains locked to one session, diagnostics run concurrently under a bounded semaphore with timeout/output caps and session-id logging. vm-frontend payload client gained DiagnosticRequest/run_diagnostic_tcp and payload-client --diagnostic with --timeout-seconds/--max-output-bytes. Added Rust/Python integration coverage proving a diagnostic command can run while a primary payload is active. Validation so far: python3 -m py_compile docker/guest-payload-server.py; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_client -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-15T21:20:18Z**

Required validation rerun reached live-smoke but failed before boot because docker/out artifacts are stale after changing docker/guest-payload-server.py: artifact-manifest expected old sha256 and requested rerun of sudo ./docker/build-appliance.sh. Attempted sudo -n docker/build-appliance.sh, but sudo requires a password in this environment. Ticket remains open until appliance is rebuilt and ./vm-frontend/validate.sh required passes live-smoke.

**2026-05-15T21:22:23Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end, including live-smoke with rebuilt guest-payload-server.py. This validates the diagnostic protocol changes in offline tests and confirms the rebuilt appliance boots/runs the required live self-test.
