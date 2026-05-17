---
id: wra-dky9
status: closed
deps: []
links: [wra-lcbk, wra-sum7, wra-g0uv, wra-qbpt]
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, docker, performance]
---
# Bound Docker socket bridge sessions and logging

Problem:
The guest Docker socket bridge uses unbounded daemon threads, blocking sendall, and logs every relayed chunk.

Relevant code:
- docker/guest-socket-bridge.py:28-31 selects indefinitely.
- docker/guest-socket-bridge.py:57-61 logs and sends every chunk.
- docker/guest-socket-bridge.py:95-109 accepts connections and spawns unbounded daemon threads.

Impact:
Docker pull/build traffic can create large logs and slow validation. Slow peers can pin bridge threads indefinitely. Early bridge clients can fail if /var/run/docker.sock is not ready yet.

Recommended fix:
Bound bridge sessions, use nonblocking or timed writes, reduce logs to connection summaries/errors, and start/listen only after Docker socket readiness or retry upstream connect briefly.

Validation:
- Bridge tests for delayed Docker socket availability, slow reader timeout, large transfer log-size bound, and connection cap.


## Notes

**2026-05-16T19:42:36Z**

Blocks wra-lcbk. The Rust/Tokio guest-service spike should port the bounded Docker bridge semantics from this ticket (session caps, write/backpressure policy, logging summary behavior, Docker socket readiness) rather than using the current unbounded Python bridge as the contract.

**2026-05-16T21:39:27Z**

Implemented bounded Docker socket bridge semantics in docker/guest-socket-bridge.py: max session limit with rejected clients, retrying Docker Unix-socket connect, per-session IO timeout/write-failure close, and summary logging instead of per-chunk logging. Added offline tests for delayed Docker socket availability, idle timeout close, session limit rejection, and log-size summary behavior. Focused validation passed: python3 -m unittest docker.tests.test_guest_services -v; python3 -m py_compile docker/guest-socket-bridge.py. Appliance rebuild is needed before live/required validation because docker/guest-socket-bridge.py changed.

**2026-05-16T21:40:20Z**

Ralph iteration 5: bridge implementation is done offline, but validation/closure is still waiting for appliance rebuild because docker/guest-socket-bridge.py changed. Do not close until rebuilt appliance passes live/required validation.

**2026-05-16T21:43:09Z**

Validation after appliance rebuild passed. Focused offline validation had already passed (unittest guest services and py_compile). Live validation passed: ./vm-frontend/validate.sh live-docker. Required validation passed: ./vm-frontend/validate.sh required. Closing wra-dky9 with bounded bridge semantics: max sessions, connection retry, IO timeout/write-failure close, and summary logging.
