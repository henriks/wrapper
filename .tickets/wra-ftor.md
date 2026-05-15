---
id: wra-ftor
status: closed
deps: []
links: []
created: 2026-05-15T06:06:55Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-zfib
tags: [fs, virtiofs, locks]
---
# Audit FUSE and virtiofs lock request plumbing

Establish exactly what information the guest Linux virtiofs/FUSE client sends for fcntl lock operations and where the Rust virtiofsd 1.13.3 stack currently discards it. Current evidence: composed-fs implements virtiofsd::filesystem::FileSystem; that trait exposes getlk/setlk/setlkw as no-argument methods, so composed-fs cannot see inode/file handle, owner, pid, byte range, lock type, or blocking/nonblocking mode. This ticket should inspect the crate source used by Cargo.lock, the FUSE protocol structs/opcodes, and the vhost-user virtiofs request path.

## Design

Read the local Cargo dependency source if available, otherwise use the vendored Cargo registry. Identify whether a minimal patch can stay inside virtiofsd, whether a repo patch section is needed, or whether replacing the FUSE server layer is lower risk. Capture exact structs/functions/files to modify and any upstream issue/merge-request references worth tracking.

## Acceptance Criteria

A ticket note or design doc identifies the exact API gap, the request fields required by SQLite-compatible locking, the files/functions in virtiofsd that parse lock requests, and the chosen implementation route for exposing those fields to composed-fs.


## Notes

**2026-05-15T06:09:44Z**

Audit finding: virtiofsd 1.13.3 already defines FUSE lock wire structs in src/fuse.rs: LkIn { fh, owner, lk: FileLock, lk_flags, padding }, LkOut { lk }, and FileLock { start, end, type_, pid }. The server dispatch in src/server.rs routes Opcode::Getlk/Setlk/Setlkw to methods at lines around 1206, but those methods ignore the Reader and call FileSystem::getlk/setlk/setlkw with no arguments. The trait in src/filesystem.rs only exposes no-argument TODO hooks. Required API fields for composed-fs: Context/inode from InHeader, file handle fh, FUSE owner, FileLock start/end/type_/pid, lk_flags, and whether the op is blocking. GETLK also needs to return a FileLock/LkOut conflict result.
