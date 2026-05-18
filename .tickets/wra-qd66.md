---
id: wra-qd66
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [frontend, persistence, stability]
---
# Create and validate VM state disks atomically

Problem:
ensure_state_disk creates state.raw, sets length to 20 GiB, then runs mkfs.ext4. If formatting fails or the process is interrupted, the partial file remains. Future launches accept any existing path because validate_launch_inputs only checks existence.

Relevant code:
- vm-frontend/src/launch.rs:235 calls ensure_state_disk before launch validation.
- vm-frontend/src/launch.rs:360-367 validates only existence.
- vm-frontend/src/launch.rs:370-390 creates and formats the disk in place.

Impact:
A corrupt or unformatted state disk can be accepted forever, causing boot hangs, root overlay failures, or confusing persistence bugs.

Recommended fix:
Format a temp file in the same directory, fsync/rename only after success, and clean temp files on failure. Validate existing state.raw size and an ext4 signature or explicit frontend metadata marker before accepting it.

Validation:
- Offline tests for zero-length, wrong-size, and temp/partial state disks.
- Live persistence test that corrupts/deletes state.raw and verifies deterministic rebuild or clear refusal.


## Notes

**2026-05-18T10:38:23Z**

Follow-up cleanup scan under wra-emj5: keep this fix deletion-oriented. Replace the current exists-means-valid state disk behavior with temp-file formatting plus direct size/ext4 validation or deterministic rebuild/refusal. Avoid adding long-lived sidecar compatibility metadata unless direct validation is insufficient and the need is documented.
