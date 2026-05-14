---
id: wra-1joy
status: closed
deps: [wra-0452]
links: []
created: 2026-05-14T18:42:20Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, filesystem, virtiofs]
---
# Add virtiofs protocol adversarial and concurrency tests

Add deeper tests at the virtiofs/FUSE request handling boundary. Cover lookup/forget counts, getattr/setattr, open/read/write/release ordering, opendir/readdir/releasedir, flush/fsync behavior, unsupported operations, malformed requests, request ID behavior, inode lifetime after unlink/rename, open-then-delete, open-then-rename, concurrent reads/writes, and directory mutation during listing.\n\nThis should reuse the operation model fixtures but exercise the protocol layer directly enough to catch lifecycle and concurrency bugs that ordinary path-level tests miss.\n\nRelevant code: composed-fs virtiofs backend, namespace/inode/request handling, and documented filesystem semantics baseline.

## Acceptance Criteria

Protocol-level tests cover lifecycle, malformed/unsupported requests, and concurrency scenarios. The ticket notes any protocol operations intentionally unsupported and verifies they fail safely. Outcomes include whether additional instrumentation or counters were added for diagnosability.


## Notes

**2026-05-14T19:38:53Z**

wra-0452 added model-style backend operation sequences but did not cover protocol-level lookup/forget/open/release ordering, request IDs, concurrent operations, or directory mutation during readdir. Those remain in scope for this virtiofs protocol adversarial ticket.

**2026-05-14T19:46:26Z**

Added protocol-boundary composed-fs tests for lookup/forget count saturation, malformed lookup components, mismatched release handling, open-then-rename writes, concurrent distinct-offset writes through a shared handle, readdir iterator snapshots during host directory mutation, and unsupported character-device mknod failure. Fixed a lifecycle bug where release(handle, wrong_inode) removed the handle before returning EBADF; release now verifies inode ownership before consuming the handle. Offline protocol tests pass; raw malformed FUSE byte-level request decoding remains out of scope for these direct FileSystem trait tests and should be covered by a future harness if needed.
