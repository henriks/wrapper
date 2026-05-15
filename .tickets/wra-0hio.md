---
id: wra-0hio
status: closed
deps: [wra-gx6d, wra-1bks]
links: []
created: 2026-05-15T10:39:55Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, diagnostics, ci]
---
# Add validation artifacts, timeout, and hang diagnostics

Live validation can appear frozen when the guest/control path is waiting. Add systematic timeout, progress, and artifact behavior to validation. Relevant areas include vm-frontend/validate.sh, self-test progress logging, QEMU/frontend process supervision, run-dir artifact layout, and CI/local documentation. Cover phase-level progress messages, bounded waits, last-error reporting, QEMU serial/console capture, guest service logs, network trace snippets where useful, artifact manifest summary, and cleanup behavior after timeout.

## Design

Make validation output explain the current phase before long waits. Write artifacts into the run-dir using stable names. Tests should simulate hung QEMU/payload/bridge phases with fake processes or shims and assert timeout messages plus cleanup.

## Acceptance Criteria

A hung guest or payload control path emits phase progress, bounded timeout, last error, and artifact locations. Offline tests simulate timeout paths without booting QEMU. Live validation failures leave useful logs in the run-dir and do not leave orphaned processes.


## Notes

**2026-05-15T11:21:58Z**

Started. Added phase/artifact diagnostics to launch/self-test: long phases now print starting-frontend, waiting-for-payload-ready with timeout, payload execution, published payload check, and shutdown; launch/self-test failures append run_dir, state.json, qemu.log, console.log, and vmnet-events.log locations. Added frontend_artifact_summary_names_key_run_artifacts unit coverage and validated existing fake-QEMU timeout cleanup path. Offline docs/fmt/guest-services/fast/fuzz-check passed; live remains blocked by stale appliance manifest.

**2026-05-15T11:24:13Z**

Reflection iteration 11 captured in .ralph/wra-neci.md. Added wait_for_payload_ready_with_probe seam and payload_readiness_timeout_reports_last_error unit test so payload readiness hangs can be exercised offline with bounded timeout and last-error diagnostics. Full offline docs/fmt/guest-services/fast/fuzz-check passed; host-live still blocked by stale appliance manifest.

**2026-05-15T11:33:07Z**

Collected live failure artifact evidence after appliance rebuild: ran self-test with invalid Docker image in run-dir .sandbox/docker-vm/self-test-failure. It printed phase progress and artifact paths, failed payload with exit 125, appended artifact summary, left state.json/qemu.log/console.log/vmnet-events.log, and no qemu-system-x86_64 process for that run-dir remained. Then ./vm-frontend/validate.sh required passed end-to-end with live self-test ok.

**2026-05-15T11:35:23Z**

Closure evidence: live induced failure with invalid Docker image left state.json/qemu.log/console.log/vmnet-events.log in .sandbox/docker-vm/self-test-failure, printed phase progress plus artifact summary, exited nonzero as expected, and no matching QEMU process remained. Subsequent ./vm-frontend/validate.sh required passed end-to-end.
