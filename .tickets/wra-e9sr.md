---
id: wra-e9sr
status: open
deps: [wra-i3t9]
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, tcp, performance]
---
# Reap closed guest TCP sockets and stale session slots

Problem:
Guest TCP sockets are closed but not removed from smoltcp SocketSet or associated listener/host connection slot vectors.

Relevant code:
- vm-frontend/src/guest_tcp.rs:60-66 initializes SocketSet and slot vectors.
- vm-frontend/src/guest_tcp.rs:116-147 closes sessions but does not reclaim terminal sockets.
- vm-frontend/src/host_ingress.rs:345 removes bridge sessions after guest close while the core socket can remain.

Impact:
Long-running sessions with many sequential outbound or host-ingress connections can accumulate closed sockets, increase memory use, increase per-poll scan cost, and allocate replacement listeners repeatedly.

Recommended fix:
Add a GuestTcpCore reap_closed_sessions path that removes terminal sockets from SocketSet and prunes listener/host connection slots once FIN/RST state is terminal and queued frames are drained.

Validation:
- Add churn tests for hundreds/thousands of sequential outbound and host-ingress sessions.
- Assert bounded socket/slot counts and stable poll cost.
- Run vmnet runtime tests and required validation before closing.

