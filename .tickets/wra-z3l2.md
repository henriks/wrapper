---
id: wra-z3l2
status: closed
deps: [wra-9ida]
links: []
created: 2026-05-13T21:34:11Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, launcher, lifecycle]
---
# Harden Rust frontend launch shutdown and task lifecycle

The bounded validation launches intentionally kill QEMU with --qemu-timeout-seconds. QEMU exits with SIGKILL and embedded config-fs/composed-fs tasks report HandleRequest(Disconnected), causing launch to exit nonzero even when the network smoke itself succeeded. This relates to vm-frontend/src/launch.rs and vm-frontend/src/vmnet_runtime.rs. The goal is to make long-running and bounded validation runs report expected lifecycle outcomes clearly, propagate shutdown cleanly, and avoid confusing task-disconnect errors after intentional QEMU termination.

## Acceptance Criteria

Intentional timeout or signal shutdown produces a clear expected status and cleanly stops embedded vmnet/composed-fs/config-fs tasks. Unexpected backend task failures still surface as errors. The validation documentation is updated with the new lifecycle behavior and exact command output.


## Notes

**2026-05-13T21:48:29Z**

Implemented and validated focused lifecycle hardening. run_frontend_until_qemu_exit* now returns QemuExit { status, timed_out }; wait_for_qemu marks timeout-triggered SIGKILL separately; state.json records status=timed_out for intentional bounded validation stops; CLI output now says qemu timed out after N seconds and was terminated with status ... instead of treating it as a generic qemu exit. Embedded composed-fs/config-fs/vmnet task threads share a shutdown flag and suppress expected disconnect errors after QEMU termination, while still printing backend failures before shutdown. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 64 lib tests, 8 bin tests, 1 ignored. A 10-second KVM launch with --no-net produced the expected timeout message and state.json status=timed_out without composed-fs/config-fs/vmnet disconnect noise. Updated vm-frontend/vmnet-runtime-validation.md.
