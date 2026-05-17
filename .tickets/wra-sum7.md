---
id: wra-sum7
status: closed
deps: [wra-g0uv, wra-38ai]
links: [wra-lcbk, wra-dky9, wra-g0uv, wra-38ai]
created: 2026-05-16T15:50:48Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, payload, stability]
---
# Bound guest payload server idle clients and slow writers

Problem:
The guest payload server is thread-per-connection with blocking sockets. A client can connect and never send a frame, or stop reading output, consuming threads or diagnostic semaphore slots indefinitely.

Relevant code:
- docker/guest-payload-server.py:35-50 blocking recv_exact/recv_frame.
- docker/guest-payload-server.py:149 and 291 send output/diagnostic frames with blocking sendall.
- docker/guest-payload-server.py:395-412 accepts connections and spawns unbounded daemon threads.

Impact:
Idle or slow clients can cause payload hangs, resource exhaustion, or blocked diagnostic capacity.

Recommended fix:
Add initial-frame/control read timeouts, bounded active client/session limits, write timeouts/backpressure policy, and guaranteed child process cleanup on client write failure.

Validation:
- Offline tests for idle client timeout, slow/non-reading diagnostic client releasing semaphore, max-client rejection, and child reaping after blocked output.


## Notes

**2026-05-16T19:42:36Z**

Blocks wra-lcbk. The Rust/Tokio guest-service spike should port the bounded payload-server semantics from this ticket (client/session limits, idle/slow-writer policy, cleanup behavior) rather than inventing them during the spike.

**2026-05-16T21:36:50Z**

wra-g0uv is closed after guest-init Docker readiness gating and required validation. wra-sum7 is now unblocked and can inspect/implement guest payload server idle/slow-writer bounds next; this will touch docker/guest-payload-server.py and remains appliance-sensitive.

**2026-05-16T21:40:20Z**

Ralph iteration 5 inspection: guest payload server remains thread-per-connection with blocking initial recv_frame and blocking send_frame/sendall in primary and diagnostic output paths. Existing docker/tests/test_guest_services.py covers protocol, diagnostics, and disconnect cases but lacks idle-client timeout, active-client cap, slow/non-reading diagnostic client semaphore-release, and blocked-output child cleanup coverage. Implementing wra-sum7 will touch docker/guest-payload-server.py and require appliance rebuild before live/required validation.

**2026-05-16T21:46:23Z**

Implemented offline payload-server bounds in docker/guest-payload-server.py: initial-frame timeout, per-session IO timeout, bounded max clients with rejected connections, bounded diagnostic count via configurable semaphore, slow writer/write-failure cleanup, and process-group termination helper. Added offline tests for idle initial client timeout, max-client rejection, slow/non-reading diagnostic client releasing semaphore, and blocked primary output cleanup. Focused validation passed: python3 -m py_compile docker/guest-payload-server.py && python3 -m unittest docker.tests.test_guest_services -v. Appliance rebuild is needed before live/required validation because docker/guest-payload-server.py changed.

**2026-05-16T21:47:14Z**

Ralph iteration 7 audit: wra-sum7 implementation is still waiting for appliance rebuild before live/required validation. Source freshness check for docker/guest-payload-server.py is stale/not reflected in docker/out/artifact-manifest.json; sensitive changed paths include docker/guest-payload-server.py and related guest-service files. Do not close until rebuilt appliance passes validation.

**2026-05-16T21:51:58Z**

After user rebuilt the appliance, live-payload validation passed. Full required validation was attempted twice but was killed by harness timeout during live-setup-tools pi npm install (600s then 900s); created linked bug wra-38ai for investigation. wra-sum7 remains open because the required gate has not completed.

**2026-05-16T21:55:22Z**

Iteration 8: found validation blocker root cause. The wra-sum7 primary payload IO timeout is too aggressive: after initial request, run_payload waits in recv_frame with conn timeout and terminates the process group on socket.timeout, so quiet long-running commands (Pi mise/npm setup) can be killed even though the client is still connected. Need appliance-sensitive edit to docker/guest-payload-server.py plus regression test; wra-sum7 now depends on linked bug wra-38ai until fixed and required validation passes.

**2026-05-16T21:56:32Z**

Implemented wra-38ai fix in docker/guest-payload-server.py and added regression coverage in docker/tests/test_guest_services.py. Focused guest-service tests passed. Because docker/guest-payload-server.py changed again, appliance rebuild is required before live/required validation and before closing wra-38ai/wra-sum7.

**2026-05-16T21:56:52Z**

Focused validation for the wra-38ai guest-payload-server fix passed with ./vm-frontend/validate.sh guest-services. Current blocker is appliance rebuild/source freshness for docker/guest-payload-server.py, then live/required validation.

**2026-05-17T05:59:57Z**

After appliance rebuild including latest docker/guest-payload-server.py, ./vm-frontend/validate.sh required passed in 114s (log /tmp/wra-sum7-required-after-rebuild-20260517-085746.log). This covers offline guest-service tests, live-smoke, and live-setup-tools; prior live-payload also passed after the payload-server bounds. wra-38ai is closed; closing wra-sum7.
