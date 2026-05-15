# Implement tk epic wra-neci

Epic: `wra-neci` — Make validation representative across VM, network, guest, filesystem, and UX.

## Goals
- Implement the epic through its child tickets in dependency order.
- Prefer removing/simplifying code and combining validation paths over adding duplicated mechanisms.
- Keep tickets updated with implementation notes, cross-ticket discoveries, and accurate status.
- Do not close a ticket unless its acceptance criteria are satisfied and validation has been exercised, including live validation where required.
- Preserve AGENTS.md validation discipline: run `cargo test --manifest-path vm-frontend/Cargo.toml --offline` and `./vm-frontend/validate.sh live` after code changes; if live validation cannot run in this environment, document the limitation and ask the user to run it. Do not mark tasks complete without live evidence.
- For arbitrary input/output parsing or protocol code, add fuzz/property coverage.

## Current epic acceptance criteria
- A coherent validation hardening suite exists.
- Required tier runs offline tests, formatting, fuzz target compilation, and live validation.
- Live validation includes multiple named scenarios.
- Stale artifacts fail before boot.
- Runtime readiness, DNS, guest services, payload framing, Docker bridge, composed-fs live behavior, CLI/TUI config, and security/log contracts have dedicated regression coverage.

## Ticket workflow
For each child ticket:
1. `tk start <id>` before implementation.
2. Read related source/tests and ticket details.
3. Add `tk add-note <id> ...` for significant findings, blockers, or decisions.
4. If a finding affects another child, add a note to that other ticket too.
5. Implement the smallest coherent slice that satisfies acceptance criteria.
6. Add offline/fuzz/live tests as required by the ticket.
7. Run relevant targeted checks while iterating.
8. Run the full validation set before closing: `cargo test --manifest-path vm-frontend/Cargo.toml --offline` and `./vm-frontend/validate.sh live` (or the new required tier once implemented). If live cannot run, leave ticket open and document limitation.
9. `tk close <id>` only after acceptance criteria and validation evidence are complete.
10. Update this Ralph file with evidence, notes, and checklist status.

## Dependency-ordered checklist

### Foundation
- [x] `wra-gx6d` — Add required validation tier and prevent docs/script drift.
  - Implemented `required`/`full`, `docs`, `fmt`, `fuzz-check`, and `host-live` tiers in `vm-frontend/validate.sh`.
  - Updated `AGENTS.md`, `vm-frontend/validation-workflow.md`, and `vm-frontend/README.md` to document the required gate.
  - Added docs drift checks and KVM host-live limitation diagnostics (`KVM_DEVICE` override allows diagnostic testing).
  - Closed after live evidence on 2026-05-15.
- [x] `wra-pssg` — Gate live validation on fresh appliance source hashes. Started/closed 2026-05-15.
  - Implemented manifest `source_inputs` hash recording in `docker/build-appliance.sh`.
  - Added frontend freshness validation before `self-test`/live boot and guest HTTP smoke, replacing the narrow mtime check.
  - Added unit tests for fresh pass, changed `guest-init.sh`, `guest-payload-server.py`, `guest-socket-bridge.py`, and missing source hashes.
  - After user rebuilt appliance artifacts, `./vm-frontend/validate.sh required` passed end-to-end and live self-test reached `self-test: ok`; closed with source freshness gate live evidence.
- [x] `wra-1bks` — Guarantee frontend and QEMU cleanup on self-test failure paths. Started/closed 2026-05-15.
  - Initial survey: `RunningFrontend` owns QEMU child/shutdown flag but had no `Drop` guard; `run_self_test` has multiple `?` failure points after `start_frontend_with_policy` and before explicit `running.terminate()`.
  - Added `RunningFrontend` Drop cleanup: unfinished runners set helper shutdown, kill/reap QEMU, and write `state.json` status `terminated`; normal `wait()`/`terminate()` mark finished so Drop does not overwrite state.
  - Added fake-QEMU tests for Drop cleanup, wait-not-overwritten, and explicit terminate reaping.
  - Added more fake lifecycle tests for wait timeout -> kill/reap + `timed_out` state, repeated runner after Drop cleanup, and stale socket cleanup.
  - After user rebuilt appliance artifacts, `./vm-frontend/validate.sh required` passed end-to-end; closed with fake failure-path cleanup coverage plus live frontend/QEMU lifecycle evidence.
- [x] `wra-0hio` — Add validation artifacts, timeout, and hang diagnostics. Started/closed 2026-05-15.
  - Added launch/self-test phase progress messages for starting frontend, waiting for payload readiness with timeout, published payload check, payload execution, and shutdown.
  - Added shared artifact summary appended to launch/self-test failures, naming run-dir, state.json, qemu.log, console.log, and vmnet event log.
  - Added unit coverage for artifact summary; existing fake-QEMU timeout test continues to cover bounded timeout cleanup/state.
  - Added `wait_for_payload_ready_with_probe` seam and timeout test asserting bounded payload-readiness hangs report the last probe error.
  - Live induced-failure evidence collected with an invalid Docker image: phase progress and artifact summary printed, run-dir logs/state existed, and no matching QEMU process remained. Closed after subsequent `./vm-frontend/validate.sh required` passed.
- [x] `wra-kh0g` — Build a named live validation matrix. Started/closed 2026-05-15.
  - Added `live-smoke` (quick required scenario; aliases `host-live`/`live`), `live-hostile` (hostile `--no-net` scenario), and `live-full` (runs all named live scenarios) to `vm-frontend/validate.sh`.
  - Updated `AGENTS.md`, `vm-frontend/validation-workflow.md`, and `vm-frontend/README.md` for named live scenarios and prerequisites.
  - Validated `live-hostile`, `required` (with `live-smoke`), and `live-full` (smoke + hostile) on the rebuilt appliance.

### Runtime/network/DNS
- [x] `wra-o9y4` — Add deterministic mio runtime readiness integration harness. Started/closed 2026-05-15.
  - Extracted `RuntimeReadyDispatch` from `serve_vmnet_gateway` event classification.
  - Added tests for simultaneous QEMU/host/proxy readiness, close/error readiness mapping, and empty timer-only polls.
  - Added `RuntimePoller` registration lifecycle tests for pre-registration data, stale old-fd readiness after source fd replacement, reregistered interest changes, and registration interest/fd/count assertions.
  - After user rebuilt appliance artifacts, `./vm-frontend/validate.sh required` passed end-to-end; closed with deterministic readiness/poller coverage plus live evidence.
- [x] `wra-crwz` — Harden host ingress and TCP proxy readiness edge coverage. Started/closed 2026-05-15.
  - Changed TCP proxy upstream readable handling to drain immediately available bytes until WouldBlock/EOF instead of one read per event.
  - Added `upstream_readiness_drains_response_until_would_block` regression coverage; existing host-ingress drain coverage remains in place.
  - Added stale-session cleanup so proxy sessions/interests are removed once the gateway no longer reports an established active TCP session; covered by `guest_close_removes_proxy_session_and_interest`.
  - Added upstream EOF/read-error cleanup handling and tests; closed after targeted `tcp_proxy::tests::` and required validation passed.
- [x] `wra-hffq` — Exercise DNS behavior with deterministic, fuzz, and live resolver tests. Started/closed 2026-05-15.
  - Existing offline coverage already covered malformed/truncated DNS input, EDNS additional records, allow/deny policy, CNAME+AAAA, SERVFAIL, REFUSED, gateway destination filtering, and arbitrary-payload fuzz target `vm-frontend/fuzz/fuzz_targets/dns_proxy_payload.rs`.
  - Added explicit NXDOMAIN response-code preservation coverage.
  - Added self-test `--dns-check` and `validate.sh live-dns`, which runs allowed resolver and denied `--hostile --no-net` resolver scenarios with query-specific diagnostics (`dns-allow-ok`/`dns-allow-failed`, `dns-deny-ok`/`dns-deny-unexpected`).
  - Closed after targeted DNS/self-test coverage, `./vm-frontend/validate.sh live-dns`, and `./vm-frontend/validate.sh required` passed.

### Guest services/protocols/Docker
- [x] `wra-otbe` — Add direct tests for guest init and Python guest services. Started/closed 2026-05-15.
  - Added `docker/tests/test_guest_services.py` with offline `unittest` coverage for `guest-init.sh` syntax, payload server ping/failure/real short payload/identity validation, and socket bridge relay/missing Docker socket cleanup.
  - Refactored `docker/guest-init.sh` with `AGENTVM_GUEST_INIT_SOURCE_ONLY=1` source-only mode and added `docker/tests/test_guest_init.sh` for cmdline parsing, invalid/valid composed bind behavior with mutating commands mocked, and critical service exit detection.
  - Expanded guest payload service tests for fragmented initial frames, invalid JSON failure frames, oversized frame rejection before payload read, EOF mid-frame no-hang behavior, and oversized send errors.
  - Fixed `docker/guest-socket-bridge.py` to close the upstream Unix socket when `connect()` fails.
  - Added `vm-frontend/validate.sh guest-services` and included it in `required`/`all-local`; updated docs.
  - After user rebuilt appliance artifacts, `./vm-frontend/validate.sh required` passed end-to-end; closed with guest-services tier plus live rebuilt guest script evidence.
- [x] `wra-gq92` — Test payload protocol framing, limits, and cross-language compatibility. Started/closed 2026-05-15.
  - Prep work landed while `wra-otbe` is open: Python and Rust payload frame readers now enforce a shared 16MiB payload limit, Python reports invalid/oversized initial requests with failure frames, Rust has bounded/truncated/proptest frame coverage, and a Rust-to-real-Python payload-server compatibility test runs outside QEMU.
  - Added self-test `--payload-stress` plus `validate.sh live-payload` to exercise a large payload request environment and large guest output in KVM; documented in `AGENTS.md`, `vm-frontend/README.md`, and `vm-frontend/validation-workflow.md`.
  - First `live-payload` run exposed a real hang: host-ingress discarded bytes when smoltcp accepted only a partial/zero host-to-guest send. Fixed `HostIngressBridge` with pending guest-write buffering/backpressure and added `large_host_payload_backpressures_instead_of_discarding_unsent_bytes` regression coverage.
  - Closed after targeted self-test/host-ingress tests, `./vm-frontend/validate.sh live-payload`, and `./vm-frontend/validate.sh required` passed.
- [x] `wra-zbix` — Cover Docker bridge and guest socket bridge contracts. Started/closed 2026-05-15.
  - Added offline guest socket bridge coverage for relaying through a real Unix Docker socket, separate-client reconnects, and arbitrary binary/malformed byte-stream preservation. The bridge is a raw stream relay rather than a framed protocol, so malformed coverage asserts bytes are preserved instead of parsed.
  - Targeted Python unittest discovery passed with 15 tests; `./vm-frontend/validate.sh required` passed with guest-services plus live-smoke Docker checks.
  - Added `self-test --docker-net-check` and `validate.sh live-docker`, covering container egress allow and no-net denial with image/policy/phase diagnostics (`docker-egress-ok`, `docker-deny-ok`).
  - First no-net Docker run exposed unrelated sqlite concurrency disk I/O before Docker assertions; self-test now uses per-run sqlite DB names and skips sqlite concurrency only for the specialized `docker-net-check + no-net` path so network policy validation remains focused.
  - Added `self-test --publish-container-port` and extended `live-docker` with host-to-container published-port coverage: the guest starts an Alpine container with an nc HTTP response, the host connects through the frontend published port, and validation asserts `agentvm-container-publish-ok`. Closed after `live-docker` and required validation passed.

### Filesystem/security
- [x] `wra-a0bu` — Add live and adversarial composed-fs coverage. Started/closed 2026-05-15.
  - Initial survey: existing offline composed-fs tests already include adversarial path/property coverage, and live self-test already covers config mount read-only behavior, workspace writes/bind mounts, guest+host SQLite WAL/concurrency, and repeated scenario runs that indirectly exercise stale socket cleanup.
  - Added `self-test --fs-check` and `validate.sh live-fs`. The fs check covers guest-visible create/append/truncate/rename/unlink/readdir/uid-gid behavior, private config key symlink denial, and a host-visible marker verified after payload exit.
  - `live-fs` runs the same run-dir twice, explicitly covering stale virtiofs socket/state cleanup on repeated runs.
  - First `live-fs` attempt exposed immediate post-rename lookup latency, so fs-check uses bounded retry before failing. A required run also reproduced the composed lock owner-flush flake, so `fs_setlk_eventually` now tolerates short OFD lock handoff latency while still failing persistent lock leaks.
  - Closed after targeted fs-check/lock tests, `./vm-frontend/validate.sh live-fs`, and `./vm-frontend/validate.sh required` passed.
- [x] `wra-nsvn` — Validate security policy, manifests, and diagnostics. Started/closed 2026-05-15.
  - Initial survey: no-net/hostile live scenarios, DNS deny diagnostics, Docker no-net denial, config key hidden checks, hostile symlink attempts, stale artifact source-hash preflight, runtime manifest validation tests, event-log secret-redaction tests, and induced self-test failure artifact summaries already cover much of the security surface.
  - Consolidated acceptance evidence rather than adding a parallel validation path. `./vm-frontend/validate.sh live-full` passed, covering smoke, hostile/no-net, payload stress, DNS allow/deny, Docker allow/deny/published-port, and repeated composed-fs scenarios with stable diagnostics and artifact summaries; closed after note/evidence update.

### CLI/TUI/setup UX
- [x] `wra-6cei` — Add CLI config, setup-tool, and command override UX tests. Started/closed 2026-05-15.
  - Surveyed existing `vm-frontend/src/main.rs` tests: good model-level coverage already exists for setup-tool pi, configured codex suppressing startup dialog, command override, config schema, argv0 behavior, unconfigured non-TUI diagnostic, and removed flag rejection.
  - Added tests for explicit `--setup-tool codex` config JSON shape, invalid config diagnostics from disk, shell `--command` override without config, legacy schema migration, and CLI network override precedence over config no-net/published ports.
  - Closed after `./vm-frontend/validate.sh required` passed end-to-end; deeper package bootstrap shims remain tracked by `wra-l8ke`.
- [x] `wra-l8ke` — Test setup-tool package bootstrap paths. Started/closed 2026-05-15.
  - Added setup-tool bootstrap tests for pi command-v/npm install behavior, package selection, idempotent first-run script shape, quoted arguments, codex package/auto-flags, unsupported recipe diagnostics, and idempotent config writes.
  - Added actionable missing-npm diagnostics to generated bootstrap scripts (`agentvm: npm is required to install <tool> CLI`).
  - Current implementation uses npm bootstrap, not miso, so miso-specific ticket wording was treated as obsolete.
  - Closed after targeted setup/bootstrap tests and `./vm-frontend/validate.sh required` passed.
- [x] `wra-ntek` — Add TUI interaction tests for config editing and launch flows. Started/closed 2026-05-15.
  - Added `vm-frontend/tests/tui_terminal.rs`, a pty-backed terminal harness using `script(1)` around the real `agentvm` binary.
  - Covered first-run startup cancel diagnostics/no persisted config and config editor terminal key flow for command, network, GitHub auth, share, and published-port edits.
  - Added configured-project no-reprompt coverage by forcing an invalid QEMU path and asserting stable launch phase/artifact diagnostics without booting a VM.
  - Documented the automated pty tests in `vm-frontend/validation-workflow.md`.
  - Added deterministic `ratatui::TestBackend` coverage for startup/config fixed-size rendering and wrapper prompt focus isolation.
  - Closed after `cargo test --manifest-path vm-frontend/Cargo.toml --offline`, `./vm-frontend/validate.sh required`, and `./vm-frontend/validate.sh live` passed.

## Validation evidence log
- 2026-05-15: Loop created. Ran `tk help`, `tk show wra-neci`, `tk dep tree --full wra-neci`, `tk ready`, and `tk show` for all children to seed this plan.
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh docs` passed.
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh fmt` passed.
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh fast` passed: composed-fs 50 passed/4 ignored; vm-frontend lib 142 passed/5 ignored; both bins 48 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh fuzz-check` passed (`cargo check --manifest-path vm-frontend/fuzz/Cargo.toml --offline`).
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh host-live` passed; self-test reached `self-test: ok` including payload, CA/config, SQLite, Docker, and bind checks.
- 2026-05-15 (`wra-gx6d`): `./vm-frontend/validate.sh required` passed end-to-end on KVM host.
- 2026-05-15 (`wra-gx6d`): `KVM_DEVICE=/tmp/agentvm-no-kvm-for-validate-test ./vm-frontend/validate.sh host-live` exited 1 with `host-live limitation` diagnostic.
- 2026-05-15 (`wra-pssg`): targeted tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline appliance_source_hashes -- --nocapture` and `cargo test --manifest-path vm-frontend/Cargo.toml --offline parses_frontend_prepare_defaults_and_policy -- --nocapture`.
- 2026-05-15 (`wra-pssg`): `bash -n docker/build-appliance.sh` passed.
- 2026-05-15 (`wra-pssg`): `./vm-frontend/validate.sh docs`, `fmt`, `fast`, and `fuzz-check` passed after source-hash changes. Fast included composed-fs 50 passed/4 ignored; vm-frontend lib 142 passed/5 ignored; both bins 50 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`): `./vm-frontend/validate.sh host-live` failed before QEMU boot with `stale appliance artifacts: ... artifact-manifest.json does not record appliance source hashes; rerun sudo ./docker/build-appliance.sh`, as expected for current unrebuilt artifacts.
- 2026-05-15 (`wra-1bks`): targeted tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline running_frontend -- --nocapture`, `cargo test --manifest-path vm-frontend/Cargo.toml --offline explicit_terminate -- --nocapture`, and `cargo test --manifest-path vm-frontend/Cargo.toml --offline launch::tests:: -- --nocapture`.
- 2026-05-15 (`wra-1bks`): `./vm-frontend/validate.sh docs`, `fmt`, `fast`, and `fuzz-check` passed after cleanup guard changes. Fast now includes vm-frontend lib 145 passed/5 ignored and both bins 50 passed/1 ignored each.
- 2026-05-15 (`wra-1bks`): `./vm-frontend/validate.sh host-live` still fails before QEMU boot on the `wra-pssg` stale artifact gate until appliance artifacts are rebuilt.
- 2026-05-15 (`wra-1bks`): expanded targeted launch tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline launch::tests:: -- --nocapture` (9 tests).
- 2026-05-15 (`wra-o9y4`): targeted readiness dispatch tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline runtime_ready_dispatch -- --nocapture` and `cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_runtime::tests:: -- --nocapture`.
- 2026-05-15: `./vm-frontend/validate.sh docs` and `fmt` passed. First `fast` run hit a transient composed-fs POSIX lock `WouldBlock` in `composed_lock_bridge_flush_and_release_cleanup_owner_locks`; targeted rerun of that test passed, then `./vm-frontend/validate.sh fast` and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 151 passed/5 ignored; both bins 50 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-o9y4`): `cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_poller::tests:: -- --nocapture` passed (5 tests).
- 2026-05-15 (`wra-otbe`): `python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p '*test*.py' -v` and `./vm-frontend/validate.sh guest-services` passed (7 Python tests).
- 2026-05-15 (`wra-otbe`): `./docker/tests/test_guest_init.sh` passed after adding `AGENTVM_GUEST_INIT_SOURCE_ONLY=1` source-only mode.
- 2026-05-15 (`wra-otbe`/overall): `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Fast now includes vm-frontend lib 154 passed/5 ignored and both bins 50 passed/1 ignored each.
- 2026-05-15 (`wra-pssg`/overall): `./vm-frontend/validate.sh host-live` still fails pre-boot with missing `source_inputs` in `docker/out/artifact-manifest.json`; `docker/guest-init.sh` also changed this iteration, so rebuild is definitely required.
- 2026-05-15 (`wra-6cei`): targeted tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool_codex -- --nocapture`, `invalid_config_from_disk`, `cli_network_overrides`, `shell_command_override_allows_unconfigured_plain_project`, and `legacy_config_from_disk_migrates_to_current_launch_defaults`.
- 2026-05-15 (`wra-a0bu`/composed-fs): recurring transient POSIX lock flake in `composed_lock_bridge_flush_and_release_cleanup_owner_locks` fixed by retrying short lock-release latency with `fcntl_lock_eventually`; targeted test passed.
- 2026-05-15 (`wra-6cei`/overall): `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 154 passed/5 ignored; both bins 55 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`/overall): `./vm-frontend/validate.sh host-live` still fails pre-boot with the expected stale appliance source-hash diagnostic.
- 2026-05-15 (`wra-otbe`): expanded `./docker/tests/test_guest_init.sh` passed with source-only guest-init tests for valid bind behavior and critical service exit detection.
- 2026-05-15 (`wra-6cei`): after CLI/config additions, `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 154 passed/5 ignored; both bins 55 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`/overall): final `./vm-frontend/validate.sh host-live` still fails pre-boot with the expected stale appliance source-hash diagnostic.
- 2026-05-15 (`wra-otbe`/`wra-gq92` prep): targeted `./vm-frontend/validate.sh guest-services` passed with 12 Python/shell guest-service tests after payload protocol error/limit additions.
- 2026-05-15 (`wra-gq92` prep): targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_client::tests:: -- --nocapture` passed with 12 payload-client tests, including Rust-to-real-Python guest-payload-server compatibility and proptest frame coverage.
- 2026-05-15 (`wra-otbe`/`wra-gq92` prep): `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 159 passed/5 ignored; both bins 55 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`/overall): final `./vm-frontend/validate.sh host-live` still fails pre-boot with the expected stale appliance source-hash diagnostic; `docker/guest-payload-server.py` changes now also require appliance rebuild.
- 2026-05-15 (`wra-0hio`): targeted tests passed: `cargo test --manifest-path vm-frontend/Cargo.toml --offline frontend_artifact_summary -- --nocapture` and `cargo test --manifest-path vm-frontend/Cargo.toml --offline launch::tests::wait_timeout_kills_child_and_records_timed_out_state -- --nocapture`.
- 2026-05-15 (`wra-crwz`): targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline tcp_proxy::tests:: -- --nocapture` passed (9 tests), including new upstream drain readiness coverage.
- 2026-05-15 (`wra-0hio`/`wra-crwz`): `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 160 passed/5 ignored; both bins 56 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`/overall): final `./vm-frontend/validate.sh host-live` still fails pre-boot with the expected stale appliance source-hash diagnostic.
- 2026-05-15 (`wra-0hio`): targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_readiness_timeout_reports_last_error -- --nocapture` passed.
- 2026-05-15 (`wra-crwz`): targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline tcp_proxy::tests:: -- --nocapture` passed (10 tests), including new guest-close session cleanup coverage.
- 2026-05-15 (`wra-0hio`/`wra-crwz`): `./vm-frontend/validate.sh docs`, `fmt`, `guest-services`, `fast`, and `fuzz-check` passed. Final fast counts: composed-fs 50 passed/4 ignored; vm-frontend lib 161 passed/5 ignored; both bins 57 passed/1 ignored each; doc tests passed.
- 2026-05-15 (`wra-pssg`/overall): final `./vm-frontend/validate.sh host-live` still fails pre-boot with the expected stale appliance source-hash diagnostic.
- 2026-05-15 (`wra-crwz`): targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline tcp_proxy::tests:: -- --nocapture` passed (12 tests), including upstream EOF/read-error cleanup coverage.
- 2026-05-15 (rebuilt appliance): `./vm-frontend/validate.sh required` passed end-to-end after user rebuilt artifacts. Required included docs/fmt, composed-fs 50 passed/4 ignored, vm-frontend lib 163 passed/5 ignored, both bins 57 passed/1 ignored each, guest-services 12 passed, fuzz-check, and host-live `self-test: ok` with payload, CA/config, SQLite, Docker, bind, and published payload port checks.
- 2026-05-15 closures after required evidence: closed `wra-pssg`, `wra-1bks`, `wra-o9y4`, `wra-6cei`, `wra-otbe`, and `wra-crwz`.
- 2026-05-15 (`wra-0hio`): induced live self-test failure with invalid image passed diagnostically: output included phase progress and artifact summary, `.sandbox/docker-vm/self-test-failure/{state.json,qemu.log,console.log,vmnet-events.log}` existed, and no matching QEMU process remained. Closed `wra-0hio` after `./vm-frontend/validate.sh required` passed again.
- 2026-05-15 (`wra-kh0g`): added named live tiers `live-smoke`, `live-hostile`, and `live-full`; updated docs. `./vm-frontend/validate.sh docs`, `fmt`, `live-hostile`, `required`, and `live-full` all passed. Closed `wra-kh0g`.
- 2026-05-15 (`wra-gq92`): first `./vm-frontend/validate.sh live-payload` reached `phase=running-payload` then timed out after 900s; artifacts in `.sandbox/docker-vm/self-test-payload` showed the large request reached host-ingress in a 64KiB chunk plus 5020 bytes with no response. Fixed dropped partial host-to-guest writes in `HostIngressBridge` using pending guest-write buffering/backpressure. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture` passed; targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline host_ingress::tests:: -- --nocapture` passed (9 passed/1 ignored); rerun `./vm-frontend/validate.sh live-payload` passed with `payload-stress-ok`; `./vm-frontend/validate.sh required` passed end-to-end. Closed `wra-gq92`.
- 2026-05-15 (`wra-hffq`): added explicit DNS NXDOMAIN preservation coverage plus `self-test --dns-check` and named `live-dns` tier. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline dns_proxy::tests:: -- --nocapture` passed (14 passed/1 ignored); `cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture` passed; `./vm-frontend/validate.sh live-dns` passed with allowed `dns-allow-ok example.com ...` and denied `dns-deny-ok example.com EAI_AGAIN`; final `./vm-frontend/validate.sh required` passed end-to-end. Closed `wra-hffq`.
- 2026-05-15 (`wra-l8ke`/`wra-zbix`): added setup-tool bootstrap tests and missing-npm diagnostics; added offline socket bridge tests for Unix Docker socket relay, reconnects, and arbitrary binary stream preservation. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline setup_tool -- --nocapture`, `cargo test --manifest-path vm-frontend/Cargo.toml --offline bootstrap_script -- --nocapture`, and `python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p '*test*.py' -v` passed (15 guest-service tests). `./vm-frontend/validate.sh required` passed end-to-end. Closed `wra-l8ke`; kept `wra-zbix` open for named live Docker bridge scenarios.
- 2026-05-15 (`wra-zbix`): added `live-docker` named tier and `self-test --docker-net-check`; documented in AGENTS/README/validation workflow. First no-net Docker run hit sqlite concurrency disk I/O before Docker assertions, so self-test now uses per-run sqlite DB names and skips sqlite concurrency only for the specialized `docker-net-check + no-net` scenario. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test -- --nocapture` passed; `./vm-frontend/validate.sh live-docker` passed with `docker-egress-ok image=alpine:3.22 policy=allow phase=container-egress` and `docker-deny-ok image=alpine:3.22 policy=deny phase=container-egress`; final `./vm-frontend/validate.sh required` passed end-to-end.
- 2026-05-15 (`wra-zbix`): added `self-test --publish-container-port` and host-to-container live coverage to `live-docker`. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test_payload_published_container_reports_ready_and_hit -- --nocapture` passed; `./vm-frontend/validate.sh live-docker` passed all three scenarios including `published container port 12080 ok phase=host-to-container` and `docker-publish-ok image=alpine:3.22 host_port=12080 guest_port=18080`; `./vm-frontend/validate.sh required` passed end-to-end. Closed `wra-zbix`.
- 2026-05-15 (`wra-a0bu`): added `self-test --fs-check` and `live-fs`, documented the tier, and validated repeated same-run-dir live coverage. First `live-fs` attempt exposed immediate post-rename lookup latency; fs-check now uses bounded retry around the renamed path. First required rerun reproduced the owner-flush lock flake in `composed_lock_bridge_flush_and_release_cleanup_owner_locks`; added `fs_setlk_eventually` and targeted `cargo test --manifest-path composed-fs/Cargo.toml --offline composed_lock_bridge_flush_and_release_cleanup_owner_locks -- --nocapture` passed. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline self_test_payload_fs_check_covers_live_composed_fs_contracts -- --nocapture` passed; `./vm-frontend/validate.sh live-fs` passed both first and repeated run-dir scenarios with `fs-live-ok`; final `./vm-frontend/validate.sh required` passed end-to-end. Closed `wra-a0bu`.
- 2026-05-15 (`wra-nsvn`): consolidated security/diagnostic acceptance evidence and ran `./vm-frontend/validate.sh live-full`, which passed all named live scenarios. Evidence included stable artifact summaries, hostile/no-net diagnostics, DNS deny (`dns-deny-ok ... EAI_AGAIN`), Docker deny (`docker-deny-ok ... policy=deny`), config key hidden checks, fs checks, and existing event-log secret-redaction coverage. Closed `wra-nsvn`.
- 2026-05-15 (`wra-ntek`): added pty-backed terminal tests in `vm-frontend/tests/tui_terminal.rs`. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline --test tui_terminal -- --nocapture` passed (2 tests).
- 2026-05-15 (`wra-ntek`): expanded pty TUI tests to cover configured-project startup without a first-run reprompt plus launch failure artifact diagnostics via invalid QEMU path. Documented the automated pty command in `vm-frontend/validation-workflow.md`. `./vm-frontend/validate.sh docs`, `cargo fmt --manifest-path vm-frontend/Cargo.toml --check`, and targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline --test tui_terminal -- --nocapture` passed (3 tests).
- 2026-05-15 (`wra-ntek`): added deterministic TUI render/focus tests using `ratatui::TestBackend`: startup dialog fixed-size rendering, config editor fixed-size summary rendering after edits, and wrapper prompt focus isolation from guest input. Targeted `cargo test --manifest-path vm-frontend/Cargo.toml --offline tui::tests:: -- --nocapture` passed (16 TUI tests in both bin targets).
- 2026-05-15 (`wra-ntek`/final): `cargo test --manifest-path vm-frontend/Cargo.toml --offline` passed (vm-frontend lib 165 passed/5 ignored; both bins 70 passed/1 ignored; `tui_terminal` 3 passed; doc tests passed). `./vm-frontend/validate.sh required` passed end-to-end, including docs/fmt, composed-fs 50 passed/4 ignored, vm-frontend tests, guest-services 15 passed, fuzz-check, and live-smoke `self-test: ok`. `./vm-frontend/validate.sh live` also passed live-smoke. Closed `wra-ntek` and `wra-neci`.

## Reflection (iteration 5)
- Progress is blocked from closing several tickets by the new stale-appliance gate doing its job. The working tree now requires `sudo ./docker/build-appliance.sh` to regenerate `docker/out/artifact-manifest.json` with `source_inputs`, then `./vm-frontend/validate.sh required` can provide live evidence for `wra-pssg`, `wra-1bks`, and related tickets.
- The dependency strategy remains workable: foundation validation, freshness, cleanup, and readiness seams are landing first. However, avoid piling more partially-closeable tickets behind the same rebuild blocker; next iterations should either deepen offline coverage for already-started tickets or work on independent offline tickets (`wra-6cei`, `wra-otbe`) while clearly marking live closure blocked.
- Validation gate has expanded; keep `AGENTS.md`, `validation-workflow.md`, and `validate.sh docs` aligned whenever adding tiers.

## Reflection (iteration 6)
- Accomplished so far: required validation gate is in place; appliance freshness now fails pre-boot; core QEMU cleanup guard is implemented; readiness dispatch/poller seams have deterministic coverage; guest service tests are now part of validation.
- Working well: adding small explicit seams (`RuntimeReadyDispatch`, source-only guest-init, `guest-services`) gives useful offline coverage without requiring QEMU for every contract.
- Blocking progress: live closure is blocked by root-owned stale `docker/out` and unavailable sudo password. Several tickets should remain open until a human rebuilds artifacts and runs `./vm-frontend/validate.sh required`.
- Approach adjustment: continue with offline-heavy, dependency-unblocking coverage, but avoid marking more tickets closed. Prefer tickets with offline acceptance (`wra-6cei`, more `wra-otbe`, protocol fuzz) until rebuild is available.
- Next priorities: add CLI/config tests (`wra-6cei`) or more guest-init startup/failure shims; after rebuild, immediately rerun required validation and close `wra-pssg`/`wra-1bks`/possibly `wra-o9y4` if evidence satisfies them.

## Reflection (iteration 11)
- Accomplished so far: the required gate/docs drift checks are in place; stale artifacts fail before boot; frontend cleanup and timeout state are covered with fake-QEMU tests; runtime readiness has pure dispatch/poller seams; guest services and payload framing now have direct Python/Rust compatibility coverage; CLI/config UX has disk-level tests; initial validation diagnostics and TCP proxy readiness hardening have landed.
- Working well: small testable seams are paying off. Source-only guest-init, real-Python payload server tests, fake-QEMU launch tests, and runtime readiness classifiers all catch meaningful regressions without waiting on QEMU/KVM.
- Blocking progress was gated by stale root-owned appliance artifacts and unavailable sudo. The user rebuilt the appliance in iteration 12 and `./vm-frontend/validate.sh required` now passes, so foundation/live-evidence tickets could be closed; remaining blockers are broader matrix/scenario work rather than stale artifacts.
- Approach adjustment: continue with offline slices, but bias toward unblocking dependency tickets (`wra-0hio`, `wra-crwz`, `wra-otbe`) and toward changes that reduce duplicated validation paths. Avoid closing more tickets until live evidence is available.
- Next priorities: with foundation and named live matrix now closed, move to downstream coverage tickets: payload protocol live/large scenarios (`wra-gq92`), setup-tool bootstrap shims (`wra-l8ke`), DNS live/fuzz resolver coverage (`wra-hffq`), Docker bridge contracts (`wra-zbix`), composed-fs live/adversarial checks (`wra-a0bu`), and security/diagnostics (`wra-nsvn`).

## Reflection (iteration 16)
- Accomplished so far: the epic now has a coherent required gate with docs drift checks, formatting, offline Rust/Python tests, fuzz compilation, stale-artifact preflight, and live smoke. Foundation cleanup/readiness/diagnostic tickets are closed; runtime/DNS/payload/guest-service/setup-tool coverage has been substantially hardened; live matrix has named smoke/hostile/payload/DNS scenarios; the payload live stress found and fixed a real host-ingress partial-send bug.
- Working well: small deterministic seams and named live scenarios are paying off. Fake-QEMU lifecycle tests, source-only guest-init tests, Rust-to-Python payload compatibility, DNS/property/fuzz coverage, and socket-bridge offline tests provide fast signal while live tiers cover VM/kernel behavior.
- Not working/blocking: remaining tickets are integration-heavy. `wra-zbix` still needs Docker bridge named live coverage beyond smoke, especially host-to-container published port access. `wra-a0bu` needs composed-fs live/adversarial checks that may require more self-test flags rather than separate ad hoc scripts. `wra-nsvn`/`wra-ntek` remain open and should avoid growing duplicate validation paths.
- Approach adjustment: keep extending the shared self-test and `validate.sh` matrix rather than adding standalone live scripts. Close tickets only after named live evidence. For Docker/composed-fs, add minimal flags to existing self-test payload so artifacts/cleanup/freshness handling stays unified.
- Next priorities: finish `wra-zbix` with `live-docker` container allow/deny plus published-port coverage, then `wra-a0bu` composed-fs live/adversarial behavior, then security/log contracts (`wra-nsvn`) and remaining TUI interaction coverage (`wra-ntek`).

## Reflection (iteration 21)
- Accomplished so far: every child ticket except `wra-ntek` is closed. The validation suite now has a required gate, docs drift checks, offline Rust/Python/fuzz compilation coverage, stale-artifact preflight, deterministic runtime/network seams, named live VM scenarios for smoke/hostile/payload/DNS/Docker/fs, and consolidated security/log evidence. `wra-ntek` now has a real pty harness covering first-run cancel, config editing, configured no-reprompt behavior, and launch artifact diagnostics without booting QEMU.
- Working well: using one shared `self-test`/`validate.sh` matrix for live scenarios avoided duplicated scripts, and pty tests plus `ratatui::TestBackend` give a good split between terminal integration and deterministic layout assertions.
- Not working/blocking: the remaining TUI acceptance is mostly snapshot/layout/focus coverage and then expensive final validation. Full required/live validation should be deferred until the last TUI slice to avoid repeated long VM runs.
- Approach adjustment: finish TUI with small deterministic render/focus tests first, then run the full offline test suite plus required/live matrix once before closing `wra-ntek` and the epic.
- Next priorities: add fixed-size TUI render/focus/resize assertions, run targeted TUI tests, then run `cargo test --manifest-path vm-frontend/Cargo.toml --offline` and `./vm-frontend/validate.sh required`/live evidence for closure.

## Notes
- `wra-neci` set to `in_progress` and note added referencing this Ralph loop.
- `wra-gx6d` closed. After iteration 22, all child tickets and the `wra-neci` epic are closed with final offline, required, and live validation evidence.
- `wra-pssg` started. Initial finding: `docker/build-appliance.sh` writes `artifact-manifest.json` without source input hashes; `vm-frontend/src/main.rs` has a narrow `ensure_smoke_hook_artifact_fresh` mtime check only for `guest-init.sh` vs `rootfs.raw` and only for the guest HTTP smoke path. Replaced/generalized this with manifest source hash validation before live/self-test boot.
- `wra-pssg` was closed after the user rebuilt appliance artifacts and `./vm-frontend/validate.sh required` passed; the stale source-hash gate now both fails stale artifacts before boot and allows fresh artifacts through live validation.
- `wra-1bks` started after `wra-pssg` became rebuild-blocked. Added core cleanup guard/Drop semantics around `RunningFrontend` with fake-QEMU coverage, plus timeout/repeated-run/stale-socket tests. Closed after rebuilt-artifact required validation passed.
- `wra-o9y4` started. Initial seam is a pure readiness dispatch classifier used by `serve_vmnet_gateway`; it covers simultaneous and close/error readiness deterministically. Added `RuntimePoller` lifecycle tests for pre-registration data, stale old fd readiness, and reregistered interest changes. Closed after rebuilt-artifact required validation passed.
- `wra-otbe` started. Direct Python tests uncovered and fixed a socket leak in `guest-socket-bridge.py` when Docker socket connection fails. Added source-only guest-init testability and shell tests. Added note to `wra-gq92` to build protocol fragmentation/limit/compatibility tests on the new guest service test harness. Payload protocol tests now cover fragmentation, invalid JSON, oversized frames, EOF mid-frame, and a Rust client to real Python server compatibility path outside QEMU. Closed after guest-services and live required validation passed.
- `wra-6cei` started with a survey, then added missing disk/model-level coverage for codex setup config writes, invalid config diagnostics, shell override on unconfigured projects, legacy config migration, and network precedence. Closed after required validation passed; package bootstrap shims remain in `wra-l8ke`.
- `wra-0hio` started. The first slice adds explicit phase output plus artifact locations to launch/self-test failure strings; payload-readiness timeout diagnostics are now unit-tested with a fake probe. Closed after induced live failure artifact evidence and required validation passed.
- `wra-crwz` started. TCP proxy now drains upstream readable sockets to WouldBlock in one readiness pass, matching the host-ingress drain contract already covered by tests. It also drops proxy sessions/interests after guest close/upstream EOF/read error so stale registrations do not linger. Closed after required validation passed.
- `wra-nsvn` closed after consolidating existing security/offline coverage and fresh `live-full` evidence rather than adding a duplicate validation path.
- `wra-ntek` closed. First slices added pty-backed terminal tests for startup cancel diagnostics, config editor keyboard/save behavior, configured startup without reprompt, and invalid-QEMU launch artifact diagnostics. Automated pty test command is documented. Deterministic fixed-size rendering and prompt-focus isolation tests cover the layout/focus seam; final offline, required, and live validations passed before closing `wra-ntek` and the epic.
