---
id: wra-3gdr
status: closed
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


## Notes

**2026-05-18T09:48:51Z**

Iteration 41: started as the next composed-fs prerequisite for cleanup child wra-8xsb. Initial target is two-part but narrow: prune zero-lookup cold host/shadow nodes from namespace maps after forget, then bound per-call readdir entry creation/work without introducing a separate directory-cache architecture.

**2026-05-18T09:54:35Z**

Iteration 41 progress: implemented cold host/shadow file-node pruning after lookup count reaches zero and after readdir snapshots, so readdir-created file nodes no longer remain in host/shadow inode maps without a lookup. Added readdir_prunes_cold_host_file_nodes regression plus adjusted lookup/forget coverage. Full composed-fs tests passed and ./vm-frontend/validate.sh required passed at /tmp/pi-bash-bc7d08901a951a94.log. Ticket remains open: readdir still scans/stats/materializes the full host directory before emitting entries, so the bounded-work part is not complete.

**2026-05-18T10:02:24Z**

Iteration 42 completion: finished bounded readdir/cold-cache work. Readdir now derives a per-call entry budget from the FUSE buffer size, only returns up to that budget, limits host/shadow directory reads to offset+budget candidates, and creates backend host/shadow nodes only for entries actually emitted. Readdir pagination uses stable backend iteration offsets rather than global lexicographic sorting because global sorting requires scanning the whole directory. Cold zero-lookup host/shadow nodes, including directory nodes with no cached children, are pruned after forget and after readdir snapshots. Added large-directory tiny-buffer pagination and cold-prune regressions. Full composed-fs tests passed and ./vm-frontend/validate.sh required passed at /tmp/pi-bash-674f45bf0d9348fc.log.
