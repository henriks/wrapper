---
id: wra-3gdr
status: open
deps: []
links: []
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [composed-fs, performance]
---
# Bound composed-fs readdir work and prune cold inode cache entries

Problem:
ComposedFs readdir ignores the requested FUSE buffer size as a work bound, materializes entire directories into Vec/BTreeMap, stats entries, and creates persistent host nodes. forget_inode decrements lookup counts but does not prune zero-lookup host/shadow nodes or map entries.

Relevant code:
- composed-fs/src/lib.rs:1250 receives _size but does not use it to bound work.
- composed-fs/src/lib.rs:1262-1337 materializes listed entries.
- composed-fs/src/lib.rs:1282-1292 reads/stats host entries and creates nodes.
- composed-fs/src/lib.rs:917-923 forgets without pruning.

Impact:
Large directories and host churn can cause excessive CPU and unbounded memory growth.

Recommended fix:
Make directory listing bounded/streaming. Create backend inodes only for entries actually emitted, or add a bounded cold-node cache with eviction after lookup_count reaches zero.

Validation:
- Add large-directory stress test with tiny FUSE buffer and assert bounded work/memory.
- Add host churn tests that create/delete many names and verify namespace maps shrink or stay capped after forget.

