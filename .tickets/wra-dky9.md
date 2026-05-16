---
id: wra-dky9
status: open
deps: []
links: []
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

