---
id: wra-662v
status: in_progress
deps: [wra-cvmy, wra-n0fe]
links: [wra-y335]
created: 2026-05-17T10:19:50Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, guest-service, payload]
---
# Option 1: implement opt-in Rust Tokio guest payload service

Build guest-service into the real Rust payload server using the shared payload protocol and make it usable through the existing opt-in appliance path. guest-service/src/main.rs is currently a skeleton. Do not remove or replace the Python guest payload server in the default runtime path in this ticket; wra-y335 owns parity/live validation before any default switch.

## Design

Implement a Tokio TCP server that owns child process lifecycle, stdout/stderr streaming, stdin forwarding, resize/signal handling, diagnostic timeouts, output limits, cancellation, and failure reporting. Reuse the shared protocol crate and preserve the bounded Python baseline semantics documented by wra-y335/wra-lcbk: PTY/session/process-group behavior, quiet long-running primary payloads, signal/resize/stdin frames, diagnostic limits/timeouts, client/session caps, and slow-writer cleanup. Leave Python as the default/fallback until the explicit validation/default-switch path completes.

## Acceptance Criteria

The appliance can use the Rust guest service for payload execution in opt-in mode, protocol tests are shared with frontend, focused payload tests pass for the Rust service, and wra-y335 remains the blocking parity/live validation ticket before any default switch or Python removal.


## Notes

**2026-05-17T11:03:02Z**

Relationship to existing hardening ticket wra-y335: this ticket should implement the real Rust/Tokio guest payload service and make it usable in opt-in appliance mode. wra-y335 is the parity/live validation gate before any default switch. Scope correction: do not remove the Python guest service from the production/default path in this ticket; that should happen only after wra-y335 passes and a separate default-switch/removal ticket is created or selected.

**2026-05-17T16:50:43Z**

Starting after wra-zqci closed with required validation. Scope reminder: implement opt-in Rust/Tokio guest payload service using shared payload-protocol; do not switch default production runtime away from Python in this ticket. wra-y335 remains the parity/live validation and default-switch gate. First pass should inspect current guest-service skeleton, Python guest service behavior/tests, and existing payload-protocol/frontend async session APIs before making changes.

**2026-05-17T16:53:49Z**

Continuation iteration 14: planned smallest safe Rust guest-service slice as protocol/session-admission core before process execution. Implemented GuestServiceLimits and GuestServiceState in guest-service/src/lib.rs with Python-baseline defaults for max clients, max diagnostics, initial frame timeout, and session I/O timeout. Added bounded admission guards for client slots, the single primary payload session, and diagnostic sessions. Added async handle_client for initial-frame handling over AsyncRead/AsyncWrite: ping returns OK, oversized/protocol failures attempt FAILURE, unexpected initial frames get FAILURE, valid primary/diagnostic requests are parsed and admitted but currently return explicit not-implemented FAILURE messages so the service remains opt-in/non-default. Added focused Tokio duplex tests for ping, unexpected initial frame, primary admission guard, diagnostic limit, and busy primary/diagnostic failure frames. Validation passed: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture; cargo fmt --manifest-path guest-service/Cargo.toml -- --check.

**2026-05-17T16:57:48Z**

Continuation iteration 15: added real Rust guest-service diagnostic command execution. Diagnostic requests now spawn /bin/sh -c with stdin closed and stdout/stderr piped, stream bounded OUTPUT frames, cap timeout to 60s, cap output to 4MiB, kill on timeout/truncation, and send diagnostic EXIT JSON compatible with the frontend/Python schema (exit_code, diagnostic, timed_out, truncated). Primary payload execution remains explicitly not implemented, so the Rust service is still opt-in/non-default. Added focused tests for diagnostic stdout/stderr plus exit code, output-limit truncation (exit 125), and timeout (exit 124), in addition to existing admission/protocol tests. Validation passed: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture; cargo fmt --manifest-path guest-service/Cargo.toml -- --check.

**2026-05-17T17:02:42Z**

Continuation iteration 16 reflection: progress is good when wra-662v is split into small opt-in Rust guest-service surfaces: protocol/admission first, diagnostics second, now TCP serving/CLI third. What's working: shared payload-protocol lets tests use the same frames as the frontend; Tokio duplex and loopback listener tests are fast enough for focused validation; keeping primary payload execution explicit not-implemented prevents accidental default-path behavior changes. Remaining blocker/risk: primary payload parity is still the hard part (PTY, controlling terminal, process groups, stdin/signal/resize, slow client cleanup, UID/GID HOME handling). Approach adjustment: keep making the Rust binary reachable and testable before taking on PTY parity; do not touch appliance default Python path in this ticket. Iteration 16 added serve_tcp/serve_listener, TCP bind/accept errors, the agentvm-guest-service --tcp-port CLI with Python-compatible flags, CLI parse tests, and a loopback ping listener test. Focused validation passed: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture; cargo fmt --manifest-path guest-service/Cargo.toml -- --check.

**2026-05-17T17:05:48Z**

Continuation iteration 17: chose opt-in appliance wiring before primary PTY/control parity. Rationale: Rust guest-service is now reachable and diagnostic-capable, so making the opt-in path explicit lets future parity/live validation target the real appliance plumbing while leaving Python default untouched. Added docker/guest-init.sh payload_server_command/start_payload_server selection: default/empty/python runs python3 -u agentvm-payload-server; rust requires executable /usr/local/libexec/agentvm-guest-service and runs it with the same --tcp-host/--tcp-port flags; unsupported values fail closed. Added docker/build-appliance.sh AGENTVM_PAYLOAD_SERVICE=rust opt-in requiring AGENTVM_GUEST_SERVICE_BIN and adding agentvm_payload_service=rust to the manifest kernel cmdline only for the rust opt-in. Updated source-only shell tests for guest-init service selection and build-appliance kernel cmdline/required-binary behavior. Focused validation passed: sh docker/tests/test_guest_init.sh; bash docker/tests/test_build_appliance.sh; cargo fmt --manifest-path guest-service/Cargo.toml -- --check; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture. Note: this changes appliance source inputs (docker/guest-init.sh and docker/build-appliance.sh), so required validation/closure will likely require rebuilding appliance artifacts with the updated source hashes before wra-662v can be closed.

**2026-05-17T17:08:54Z**

Continuation iteration 18: chose to start primary payload execution parity rather than stop before PTY work. Implemented an initial non-PTY Tokio primary runner: spawns /bin/sh -c with piped stdin/stdout/stderr, streams stdout/stderr as OUTPUT frames through a bounded channel, sends normal EXIT JSON, forwards INPUT frames into child stdin, parses SIGNAL/RESIZE frames (SIGNAL sends to child pid; RESIZE parsed but is a no-op until PTY support), and terminates the child on client disconnect/output write failure. Added libc dependency for signal delivery. Added focused tests that primary command stdout/stderr plus exit code are reported and that stdin frames reach the child. Caveat: this is not full Python parity yet: PTY, controlling terminal/session/process-group behavior, UID/GID/HOME setup, real resize handling, and slow-writer cleanup remain. Focused validation passed: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture; cargo fmt --manifest-path guest-service/Cargo.toml -- --check.

**2026-05-17T17:13:32Z**

Continuation iteration 19: continued primary parity with process-session/group cleanup. Primary and diagnostic child commands now run in a new session via setsid() pre_exec, and signal forwarding/cleanup target the child process group rather than just the shell PID. The primary control loop now marks the control side closed after read-frame failure so disconnect cleanup does not spin on repeated EOF; it sends SIGTERM to the process group and waits for the child. Diagnostic timeout/truncation now also SIGKILLs the process group before the child fallback kill. Added focused tests that SIGTERM control reaches the primary process group after the child installs a trap, and that client disconnect terminates a long-running primary payload promptly. Remaining parity gaps: PTY allocation/controlling terminal, real resize ioctl handling, UID/GID/HOME setup, graceful TERM-then-KILL escalation for primary processes that ignore TERM, and slow-writer deadlines/backpressure. Focused validation passed: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml --offline guest_service_primary -- --nocapture; cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture; cargo fmt --manifest-path guest-service/Cargo.toml -- --check.

**2026-05-17T17:36:12Z**

Continuation iteration 20: ran broad validation after primary/process-group work. Broad offline suite passed before required validation: sh docker/tests/test_guest_init.sh; bash docker/tests/test_build_appliance.sh; cargo test --workspace --offline; cargo fmt --all -- --check. Required validation was attempted twice. First attempt exposed a flaky vm-frontend supervised_blocking_service_marks_failed_after_readiness test; documented as wra-kolb and fixed the test race by delaying the fake late-failing service after readiness. Focused validation for the fix passed. Second required validation attempt progressed through formatting, composed-fs, guest-service, payload-protocol, and much of vm-frontend offline tests, but timed out at the command timeout after reporting payload_client::tests::payload_session_runner_cancel_token_interrupts_blocked_receive running for over 60 seconds. Focused rerun of that payload test passed immediately; documented follow-up bug wra-cqjc. No appliance rebuild request was observed because required validation did not get far enough to live/appliance freshness checks after the timeout. wra-662v remains in progress and should not be closed.

**2026-05-17T17:48:41Z**

Continuation-2 iteration 1: triaged the required-validation timeout from wra-cqjc and hardened payload cancellation against a pre-registration cancellation race. PayloadCancelToken::register_stream now shuts down the stream immediately if already cancelled; the blocking-receive cancellation test now has bounded waits/timeouts so a failure cannot hang validation indefinitely. Focused payload runner tests passed. Broad validation passed: cargo test --workspace --offline; cargo fmt --all -- --check. Closed wra-kolb after its deterministic test fix had focused and broad validation. Required validation still needs to be rerun; wra-cqjc remains open until that gate completes.

**2026-05-17T17:51:55Z**

Continuation-2 iteration 2 required validation: ./vm-frontend/validate.sh required progressed past the previous wra-cqjc payload cancel-token hang, through offline guest-service tests and fuzz target compilation, then stopped at live-smoke appliance freshness. Validation reported stale appliance artifacts because docker/build-appliance.sh changed since docker/out/artifact-manifest.json was written (manifest expected sha256 3fe137f74ba1650bb130b54def3f7a3af055d30d17a7ee2e561b5813aad436bb, current sha256 55d90dca2ad53114b69dc6797a4be4378ca0b187c513906e158b44616481d2a1). Required before wra-662v can close: rebuild the appliance artifacts with sudo ./docker/build-appliance.sh so docker/out/artifact-manifest.json, docker/out/rootfs.raw, docker/out/initrd.img, and docker/out/vmlinuz reflect the changed appliance scripts/source hashes. If validating the Rust opt-in appliance path specifically, rebuild with AGENTVM_PAYLOAD_SERVICE=rust and AGENTVM_GUEST_SERVICE_BIN pointing at the built agentvm-guest-service binary; Python remains the default appliance mode.

**2026-05-17T17:55:06Z**

Continuation-2 iteration 3: no further wra-662v changes while waiting for appliance rebuild/freshness. Switched to wra-gq8e as an independent Tokio-boundary ticket per the rebuild-blocker instruction.

**2026-05-17T17:59:25Z**

Continuation-2 iteration 4: no wra-662v changes; remains blocked on appliance rebuild/freshness before required validation can close it. Continued independent wra-gq8e async Docker proxy work instead.

**2026-05-17T18:01:17Z**

Continuation-2 iteration 5 reflection: wra-662v remains blocked on stale appliance artifacts and should not be retried/closed until the appliance is rebuilt. Work continued on independent wra-gq8e instead.

**2026-05-17T18:05:58Z**

Continuation-2 iteration 6: still no wra-662v changes; appliance rebuild/freshness remains the blocker for required validation and closure. Continued independent wra-73tn work instead.

**2026-05-17T18:10:45Z**

Continuation-2 iteration 7: unchanged; still blocked on appliance rebuild/freshness before required validation can close it. Continued independent wra-73tn work.

**2026-05-17T18:14:55Z**

Continuation-2 iteration 8: unchanged; still blocked on appliance rebuild/freshness before required validation can close it. Continued independent wra-73tn work.

**2026-05-17T18:18:18Z**

Continuation-2 iteration 9 reflection: still blocked on appliance rebuild/freshness. Required validation should not be retried for closure until docker/out artifacts are rebuilt from changed appliance scripts. Continued independent wra-73tn work.

**2026-05-17T18:20:01Z**

Continuation-2 iteration 10: unchanged; required validation/closure remains blocked on appliance rebuild/freshness. Continued independent wra-73tn work.

**2026-05-17T18:21:25Z**

Continuation-2 iteration 11 broad offline validation for the current workspace passed: cargo test --workspace --offline; cargo fmt --all -- --check. Required validation/closure remains blocked on appliance rebuild/freshness; do not retry required validation for closure until docker/out artifacts are rebuilt.

**2026-05-17T18:29:49Z**

Continuation-2 iteration 12: after the user rebuilt default appliance artifacts, ./vm-frontend/validate.sh required passed. The manifest kernel_cmdline does not include agentvm_payload_service=rust, so this validates the default Python guest payload service path plus offline Rust guest-service tests, not the opt-in Rust appliance path. wra-662v remains open until an opt-in Rust appliance is rebuilt and live-smoked, or until the ticket is explicitly scoped to stop at offline/default-required validation with wra-y335 owning all Rust live parity.

**2026-05-17T18:32:20Z**

Continuation-2 iteration 13 reflection: default required validation and Docker live validation now pass, so the remaining blocker for this ticket is specifically the Rust opt-in appliance path/parity decision. The rebuilt docker/out manifest is default Python (no agentvm_payload_service=rust), so do not treat the passing required/live-docker runs as Rust guest-service live validation. Next useful step is either build an opt-in Rust appliance and live-smoke it, or explicitly hand Rust live parity/default-switch to wra-y335 and close this ticket only if the acceptance is considered satisfied by offline Rust guest-service tests plus default required validation.

**2026-05-17T18:37:31Z**

Continuation-2 iteration 14: checked the rebuilt docker/out/artifact-manifest.json; kernel_cmdline is still the default Python appliance and does not include agentvm_payload_service=rust. Built the Rust guest-service binary successfully with cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service, but this environment cannot run sudo non-interactively to rebuild the appliance as rust opt-in. Required validation passed after the user rebuild, but it again validates the default Python appliance plus offline Rust guest-service tests, not Rust guest-service live appliance execution. For wra-662v closure, either rebuild with AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=<built binary> sudo ./docker/build-appliance.sh and live-smoke it, or explicitly scope the remaining Rust live parity/default-switch to wra-y335.

**2026-05-17T18:43:22Z**

Continuation-2 iteration 15: required validation passed again after the self-test entrypoint change, but it still uses the default Python appliance manifest. This does not change the Rust opt-in validation state. Remaining choices are unchanged: build/live-smoke an AGENTVM_PAYLOAD_SERVICE=rust appliance or explicitly leave Rust live parity/default switch to wra-y335.

**2026-05-17T19:01:39Z**

Continuation-2 iteration 20 final loop status: no further implementation changes. The current docker/out artifact manifest remains the default Python guest payload service path; required validation passing in iteration 19 validates the default appliance plus offline Rust guest-service tests, not opt-in Rust appliance execution. The Rust opt-in closure decision remains explicit: either rebuild with AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=<built agentvm-guest-service> sudo ./docker/build-appliance.sh and live-smoke it, or revise/close this implementation ticket with all Rust appliance parity/default-switch live validation owned by wra-y335. Do not switch the default away from Python until wra-y335 passes.

**2026-05-17T19:04:45Z**

After user-rebuilt appliance check: ./vm-frontend/validate.sh required passed with refreshed default artifacts. docker/out/artifact-manifest.json still has vm.kernel_cmdline 'console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet' and does not include agentvm_payload_service=rust, so this validates the default Python appliance path only. wra-662v remains open unless we either build/live-smoke a Rust opt-in appliance or explicitly narrow this implementation ticket and leave all Rust live appliance parity to wra-y335.

**2026-05-17T19:06:31Z**

wra-xcvq-complete-epic iteration 1: user confirmed an appliance rebuild, but docker/out/artifact-manifest.json is still Python-default (vm.kernel_cmdline does not contain agentvm_payload_service=rust). I built the Rust guest-service binary successfully with cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service, then attempted a non-interactive Rust opt-in appliance rebuild with: sudo -n env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh. This failed because sudo requires a password in this environment. Required user-side rebuild for Rust opt-in validation: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh, then run an appropriate live smoke/required validation. Until that is done, this ticket cannot be closed as Rust opt-in live-validated; Python must remain default.

**2026-05-17T20:53:17Z**

User reported appliance rebuild completed, so I inspected docker/out/artifact-manifest.json during iteration 39. The rebuilt manifest is still Python-default: kernel_cmdline is 'console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet' and does not include agentvm_payload_service=rust; source_inputs still include docker/guest-payload-server.py and no Rust guest-service binary artifact. Therefore this does not unblock Rust opt-in live validation/closure for wra-662v. Needed rebuild remains: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh

**2026-05-17T21:30:03Z**

wra-xcvq iteration 49 rechecked Rust opt-in closure state. Built target/debug/agentvm-guest-service successfully with cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service. Inspected docker/out/artifact-manifest.json: kernel_cmdline is still the Python-default `console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet`; it does not contain agentvm_payload_service=rust, and source_inputs still list docker/guest-payload-server.py. Attempted non-interactive Rust opt-in appliance rebuild again with sudo -n env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh; it failed because sudo requires a password. Required user-side command remains: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh. Do not close this ticket as Rust opt-in live-validated until that manifest is rebuilt and live validation is run, or until scope is explicitly narrowed with all Rust live appliance validation left to wra-y335.

**2026-05-17T21:31:49Z**

wra-xcvq iteration 50 focused implementation validation still passes while Rust opt-in appliance live validation remains blocked: cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture passed; bash docker/tests/test_build_appliance.sh passed; sh docker/tests/test_guest_init.sh passed. No code changes. This reinforces that implementation/source tests are healthy, but does not exercise the Rust guest-service inside an opt-in appliance because sudo rebuild is unavailable in this environment.

**2026-05-17T21:34:53Z**

wra-xcvq iteration 52 recheck: target/debug/agentvm-guest-service still builds successfully (cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service). docker/out/artifact-manifest.json remains Python-default: kernel_cmdline is 'console=hvc0 root=/dev/vda rootfstype=ext4 ro init=/usr/local/sbin/agentvm-init quiet' and does not contain agentvm_payload_service=rust. No opt-in Rust appliance live validation can be claimed until the privileged rebuild is run: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh, followed by manifest inspection and live validation.

**2026-05-17T21:35:37Z**

wra-xcvq iteration 53: checked task readiness and current appliance manifest again. docker/out/artifact-manifest.json has a fresh timestamp but is still Python-default: kernel_cmdline lacks agentvm_payload_service=rust, source_inputs includes docker/guest-payload-server.py, and source_inputs do not include a Rust guest-service artifact. This confirms a default appliance rebuild occurred but not the required Rust opt-in rebuild. Needed command remains: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh

**2026-05-17T21:36:42Z**

wra-xcvq iteration 55: rechecked execution environment. target/debug/agentvm-guest-service exists and is executable, but sudo -n true still fails with 'sudo: a password is required', so this agent cannot perform the privileged Rust opt-in appliance rebuild. docker/out/artifact-manifest.json remains Python-default and source_inputs include docker/guest-payload-server.py only for the payload server. User-side command remains: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh

**2026-05-17T21:38:35Z**

wra-xcvq iteration 59 near-loop-end status: no Rust opt-in appliance rebuild or explicit scope-narrowing decision has been provided. Keep this ticket in_progress under current acceptance; do not close it on default Python required validation alone. Closure requires either a manifest containing agentvm_payload_service=rust plus Rust opt-in live validation, or an explicit acceptance/scope change assigning all opt-in appliance live proof to wra-y335.
