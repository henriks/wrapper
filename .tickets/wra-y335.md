---
id: wra-y335
status: open
deps: [wra-n0xc, wra-662v]
links: [wra-662v]
created: 2026-05-17T06:03:06Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, rust, validation]
---
# Validate Rust guest-service parity before default switch

Follow-up from wra-lcbk. Once an opt-in Rust guest-service appliance path exists, run parity validation against the bounded Python baseline. Preserve payload semantics from docker/guest-service-rust-spike.md: PTY/session/process-group behavior, quiet long-running primary payloads, signal/resize/stdin frames, diagnostic limits/timeouts, client/session caps, slow-writer cleanup. Preserve Docker bridge semantics: TCP-to-/var/run/docker.sock binary relay, connect retry, session limits, idle/write-failure close, summary logging.

## Acceptance Criteria

Opt-in Rust guest service passes ./vm-frontend/validate.sh live-payload, live-docker, and required. Any parity gaps are filed before switching defaults. Default switch is a separate explicit ticket.


## Notes

**2026-05-17T11:03:02Z**

Relationship to refactoring option 1: wra-662v owns implementing the real Rust/Tokio guest payload service using the shared payload protocol. This ticket owns parity/live validation of the opt-in Rust guest-service path before any default switch. Do not merge these unless intentionally combining implementation and validation into one large appliance-sensitive ticket; keeping them separate preserves the important gate that default switch/removal of Python is explicit and later.
