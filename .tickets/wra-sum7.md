---
id: wra-sum7
status: open
deps: [wra-g0uv]
links: []
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

