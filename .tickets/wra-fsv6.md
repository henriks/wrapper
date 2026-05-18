---
id: wra-fsv6
status: closed
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


## Notes

**2026-05-18T10:02:34Z**

Iteration 42 start/audit: this is the final blocker for cleanup child wra-8xsb after wra-3gdr closed. Current setlkw still calls fcntl(F_OFD_SETLKW) directly in ComposedFs::setlkw on the virtiofs request worker. The likely minimal fix is to preserve bounded blocking semantics by replacing indefinite F_OFD_SETLKW with bounded nonblocking F_OFD_SETLK retry/sleep or an equivalent cancellable wait, not by increasing DEFAULT_THREAD_POOL_SIZE.

**2026-05-18T10:06:49Z**

Iteration 43 implementation: replaced direct fcntl(F_OFD_SETLKW) on the virtiofs request worker with bounded nonblocking OFD lock retry. SETLKW now calls F_OFD_SETLK in a short retry loop capped by SETLKW_MAX_WAIT (250ms) and returns the lock conflict instead of occupying a request worker indefinitely. This preserves bounded blocking backend semantics without increasing DEFAULT_THREAD_POOL_SIZE or introducing a parallel async filesystem architecture.

**2026-05-18T10:06:49Z**

Iteration 43 validation: added composed_lock_bridge_setlkw_is_bounded_when_conflict_remains to prove a persistent conflict returns within the bounded wait and the same worker can subsequently unlock; existing composed_lock_bridge_setlkw_waits_for_unlock still covers success when the lock is released promptly. Targeted setlkw tests, full composed-fs tests, and ./vm-frontend/validate.sh required passed. Required validation log: /tmp/pi-bash-f220349929bb6059.log.
