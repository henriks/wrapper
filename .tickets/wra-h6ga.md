---
id: wra-h6ga
status: closed
deps: [wra-ftor]
links: []
created: 2026-05-15T06:07:03Z
type: feature
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [fs, virtiofs, locks]
---
# Expose full lock operations through the virtiofsd FileSystem API

Modify or patch the Rust virtiofsd dependency so the filesystem implementation receives complete FUSE lock operation data. composed-fs cannot implement SQLite-safe locks while virtiofsd::filesystem::FileSystem only exposes no-argument getlk/setlk/setlkw hooks. The API must carry enough information to map guest fcntl locks to host-visible byte-range locks: node/file handle, lock owner, pid if available, start, length/end semantics, read/write/unlock type, nonblocking vs blocking, and enough context to return GETLK conflict details.

## Design

Build on the audit from wra-ftor. Prefer a small local patch/fork of virtiofsd that preserves the rest of the protocol boundary already used by composed-fs. Update composed-fs to compile against the patched trait while initially preserving explicit EOPNOTSUPP behavior until the bridge implementation lands. Keep the change generic; do not special-case SQLite or Codex paths.

## Acceptance Criteria

The repo builds against the patched/forked virtiofsd API; composed-fs receives typed lock request structs for GETLK/SETLK/SETLKW; existing filesystem tests still pass; lock operations may still return EOPNOTSUPP until the composed-fs bridge ticket is complete.


## Notes

**2026-05-15T06:12:05Z**

Implemented a local path-patched virtiofsd crate under third_party/virtiofsd. The FileSystem trait now receives full lock request details for getlk/setlk/setlkw: Context, inode, handle, FUSE owner, fuse::FileLock { start, end, type_, pid }, and lk_flags. server.rs now decodes LkIn for GETLK/SETLK/SETLKW and encodes LkOut for GETLK. composed-fs depends on ../third_party/virtiofsd and was updated to the new signatures while still returning EOPNOTSUPP and not advertising POSIX_LOCKS. Verification: cargo test --manifest-path composed-fs/Cargo.toml --offline passed.
