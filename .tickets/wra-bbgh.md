---
id: wra-bbgh
status: open
deps: []
links: [wra-t2uv, wra-57z4, wra-olu4, wra-i3t9, wra-kiv5, wra-jenv, wra-e9sr, wra-nui7]
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, stability, performance]
---
# Handle QEMU stream write backpressure in vmnet runtime

Problem:
QEMU stream connections are accepted as nonblocking, but guest-bound writes still use all-or-nothing write_frame/write_all. If the QEMU socket returns WouldBlock or a partial write during a burst, the vmnet runtime can fail instead of buffering and retrying.

Relevant code:
- vm-frontend/src/vmnet_stream.rs:46 accepts the QEMU stream as nonblocking.
- vm-frontend/src/vmnet_stream.rs:156-169 writes length and frame payload with write_all.
- vm-frontend/src/vmnet_runtime.rs:199-203 registers QEMU only for readable readiness.
- vm-frontend/src/vmnet_runtime.rs:491-494 and 565-567 synchronously write guest-bound frames.

Impact:
Any burst of guest-bound frames from upstream TCP, TLS handshake data, host ingress, or gateway responses can terminate the runtime or lose retry semantics under socket backpressure.

Recommended fix:
Add a framed QEMU output queue with partial-write state. Register QEMU writable interest while the queue is nonempty and drain only on writable readiness. Keep pcap capture aligned with successful queued frame emission rather than attempted immediate writes.

Validation:
- Add a scripted nonblocking QEMU writer test that accepts a partial length/frame, returns WouldBlock, then succeeds after a writable event.
- Add a large upstream/host-ingress response regression proving vmnet remains alive and delivers all frames.
- Run vm-frontend offline tests and the required validation gate before closing.


## Notes

**2026-05-16T17:27:51Z**

wra-kiv5 added ServiceIo wakeup dispatch and a pollable nonblocking wake pipe. Once DNS/connect completions produce guest frames asynchronously, they will still write through the existing QEMU frame path; QEMU write backpressure remains relevant and should be handled before broad completion-driven output growth.

**2026-05-16T17:46:26Z**

wra-kiv5 now has VmnetServiceOwner::submit_to for external bounded sinks. This may be useful for any future QEMU-write/backpressure path that wants owner-side pending context only after a bounded sink accepts work; rejected sink submissions return command+pending without inserting pending state.

**2026-05-16T19:29:40Z**

Byte-IO service commands now model bounded chunks and completions without moving QEMU frame ownership. This helps future session workers, but QEMU stream write backpressure remains separate: owner-produced guest frames can still need bounded write/backpressure policy before all async completions are safe under sustained output.

**2026-05-16T19:31:47Z**

Spawned byte-IO worker uses nonblocking completion try_send and exits on full completion queues, mirroring DNS/TCP connect worker shutdown hardening. This avoids worker shutdown hangs but intentionally does not solve owner-side QEMU frame write backpressure; that remains on wra-bbgh.

**2026-05-16T19:33:03Z**

Byte-IO owner-step full-completion behavior is now explicit: a full completion queue is reported and pending owner context remains intact. This is useful for future QEMU/backpressure handling, but does not itself resolve owner-side frame write blocking.
