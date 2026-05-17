---
id: wra-dz2y
status: closed
deps: []
links: []
created: 2026-05-17T06:03:06Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, rust, payload, docker]
---
# Add repo-local Rust guest-service protocol crate skeleton

Follow-up from wra-lcbk. Add a Rust guest-service crate/binary skeleton that is NOT installed into the appliance yet. Start with shared payload frame parsing/serialization compatible with docker/guest-payload-server.py and vm-frontend/src/payload_client.rs: !cI header, 16 MiB max payload, P/R/D initial frames, O/X/F responses, I/W/S primary control frames. Add pure offline unit tests mirroring docker/tests/test_guest_services.py protocol/fragment/oversize/EOF cases. Do not introduce appliance packaging or switch guest-init defaults in this ticket.

## Acceptance Criteria

Offline cargo tests pass under ./vm-frontend/validate.sh required; no appliance behavior changes; protocol tests document exact compatibility with the Python payload service.


## Notes

**2026-05-17T06:06:07Z**

Starting with the non-appliance skeleton only. Scope: add repo-local Rust crate/binary for payload frame protocol parsing/serialization and offline tests; do not install into appliance, do not touch docker/build-appliance.sh or guest-init defaults.

**2026-05-17T06:11:35Z**

Implemented repo-local agentvm-guest-service crate skeleton under guest-service/. It is not installed into the appliance and has no appliance startup behavior. Added payload frame protocol parser/encoder compatible with the Python/Rust host protocol: 5-byte !cI header, 16MiB max payload, named frame kinds for P/R/D/K/O/X/F/I/W/S, incremental decoder, incomplete/oversize errors. Added offline tests for Python byte order, fragmented frames, multiple frames, oversize encode/decode rejection before payload allocation, EOF mid-header/payload, every split point round-trip, and seeded arbitrary-input no-panic/bounded-buffer fuzz-style coverage. Updated validate.sh, AGENTS.md, and vm-frontend/validation-workflow.md so required/fmt/fast include guest-service. Validation passed: cargo fmt/test for guest-service, ./vm-frontend/validate.sh docs/fmt/fast, and full ./vm-frontend/validate.sh required in 106s (log /tmp/wra-dz2y-required-20260517-090934.log).
