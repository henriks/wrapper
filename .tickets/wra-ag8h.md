---
id: wra-ag8h
status: open
deps: [wra-0bmi]
links: []
created: 2026-05-14T20:38:57Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-zewf
tags: [ui, tui, payload, vm-frontend]
---
# Refactor payload client into reusable session APIs for TUI

Refactor the payload client so plain streaming and the TUI can share protocol handling without duplicating frame logic.

Context:
- Depends on the design ticket `wra-0bmi`.
- Current `run_payload_tcp_with_control` in `vm-frontend/src/payload_client.rs` owns stdin forwarding, signal/resize forwarding, output writes, and exit handling in one synchronous helper.
- A TUI event loop needs lower-level control: connect, send request, receive output/exit/failure frames, send input bytes, send resize frames based on viewport size, and send signals.
- The guest protocol is implemented by `docker/guest-payload-server.py` and should remain compatible.

Relevant code:
- `vm-frontend/src/payload_client.rs` for frame send/receive, signal/resize helpers, and tests.
- `vm-frontend/src/main.rs` for call sites in `payload-client`, `launch`, and wrapper mode.

Goal:
Expose a reusable payload session/client abstraction for interactive consumers while preserving the existing simple plain-streaming API.

## Design

Prefer a small abstraction such as a `PayloadSession` owning the TCP stream and providing methods/events for request, input, resize, signal, output, exit, and failure. Keep frame encoding/decoding in one module. Avoid making Ratatui a dependency of this low-level client.

## Acceptance Criteria

- Existing plain streaming behavior still works through `run_payload_tcp_with_control` or an equivalent compatibility wrapper.
- A lower-level API exists for the TUI to receive payload events and send input/control frames.
- Frame encoding/decoding remains covered by tests.
- Resize and signal semantics remain compatible with `docker/guest-payload-server.py`.
- Call sites are updated without changing user-visible behavior.

