---
id: wra-fsv6
status: open
deps: [wra-bcvj]
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, locks, stability]
---
# Prevent blocking SETLKW from starving virtiofs request workers

Problem:
ComposedFs handles blocking SETLKW by calling F_OFD_SETLKW synchronously on the vhost/virtiofs request worker. The default composed-fs thread pool size is 1.

Relevant code:
- composed-fs/src/lib.rs:30 DEFAULT_THREAD_POOL_SIZE is 1.
- composed-fs/src/lib.rs:1087-1099 calls F_OFD_SETLKW synchronously.
- composed-fs/src/lib.rs:1996 passes the thread pool size into the backend builder.
- third_party/virtiofsd/src/server.rs:1250 and related lock dispatch paths process lock requests on worker threads.
- vm-frontend/src/lib.rs:184 uses the default thread pool size in frontend config.

Impact:
A blocking lock wait can occupy the only request worker and prevent the unlock/flush/release request that would unblock it. More workers only mitigate; enough waits can still exhaust the pool. Interrupt handling is not sufficient today.

Recommended fix:
Do not block vhost request workers indefinitely. Use a cancellable lock wait path, a dedicated blocking-lock executor, or bounded nonblocking retry with interrupt handling. Treat increasing pool size as mitigation only.

Validation:
- Add server-level or live virtiofs test with pool size 1 where one request blocks in SETLKW and another releases the conflicting lock.
- Verify no hang and verify interrupt/cancellation behavior.

