# Plan: QEMU `microvm` Migration with a Critical Composed `virtio-fs` Backend

## Summary

Keep QEMU as the VM backend and migrate the current `q35` launch path to
`microvm`, while replacing the current per-share `virtiofsd` model with one
composed `virtio-fs` backend.

The composed filesystem is not optional long-term work. A decent user
experience depends on exposing host paths at their natural guest-visible
absolute paths without consuming one VM device and one host daemon per shared
path. The migration should therefore treat `ComposedFs` as a critical product
requirement, while still using short feasibility spikes to document the exact
QEMU, kernel, and `virtiofsd` crate constraints before committing to detailed
implementation choices.

The target preserves:
- natural guest-visible absolute paths for workspaces, tool state, auth state,
  and user `--ro` / `--rw` paths
- unprivileged host setup
- QEMU user-mode networking
- live host workspace/state sharing
- project-local runtime state under `.sandbox/docker-vm/`

The host remains responsible for:
- preparing `.sandbox/` runtime state
- generating the composed mount manifest
- launching the composed `virtio-fs` backend
- launching QEMU with `microvm`-compatible devices
- supervising QEMU, the backend, the Docker socket proxy, and the payload

## Goals

- Replace device-per-share behavior with one composed filesystem export.
- Keep guest paths natural enough that Docker bind mounts and agent/tool
  expectations continue to work.
- Move to `microvm` only after the composed export works on the current `q35`
  path.
- Document spike outcomes as first-class deliverables so later tickets do not
  repeat investigation work.
- Keep the old per-share path available behind a fallback until the composed
  backend and `microvm` path pass integration tests.

## Non-Goals

- Do not redesign networking unless a spike proves `microvm` requires it.
- Do not implement live migration support in v1.
- Do not chase complete POSIX filesystem parity before integration testing
  proves the wrapper needs it.
- Do not build duplicate staged-tree or symlink-projection implementations in
  parallel with `ComposedFs`.

## Current Constraints

The current wrapper launches:
- QEMU with `-machine q35`
- PCI virtio devices
- one workspace `virtiofsd`
- one config `virtiofsd`
- one `virtiofsd` and one `vhost-user-fs-pci` device per extra share

The current guest init:
- mounts the project share directly at `PROJECT_PATH`
- mounts a separate config share
- reads `shares.txt`
- mounts each extra share independently

This works, but it scales poorly with many host paths and conflicts with
`microvm`'s smaller device model.

## Target Architecture

### Host Process Model

Target steady-state processes:
- one QEMU process
- one repo-local composed `virtio-fs` backend process
- one host Docker Unix-socket proxy process

The composed backend serves one vhost-user socket to QEMU. It exports a virtual
namespace rooted at `/`, backed by a host-generated manifest.

### QEMU Device Model

Target device budget:
- rootfs block device
- Docker data block device
- rng
- net
- composed filesystem
- optional tiny config filesystem only if the composed export cannot safely
  carry boot config

The `microvm` path must use `virtio-mmio`-compatible devices. Exact QEMU
arguments are a spike deliverable, not an assumption.

### Guest Filesystem Model

The guest mounts the composed export at a stable internal location, for example
`/run/agentvm-host`, then bind-mounts selected paths into their final absolute
locations.

Example:
- `/run/agentvm-host/home/user/project` -> `/home/user/project`
- `/run/agentvm-host/home/user/.codex` -> `/home/user/.codex`
- `/run/agentvm-host/etc/ssl/certs` -> `/etc/ssl/certs`

Mounting the composed export directly at `/` is not practical because the
guest already has an appliance root. The guest reconstruction step remains
small, but it is still part of the design and must be specified and tested.

## Spike Deliverables

These spikes are not throwaway work. Each one must update `plan.md` or an
adjacent design note with:
- commands/configurations tested
- observed behavior
- constraints discovered
- recommended implementation decision
- rejected alternatives and why they were rejected
- follow-up tickets that changed because of the result

### Spike 1: `microvm` Boot and Device Feasibility

Purpose:
- prove the exact QEMU `microvm` command shape before changing production code
- verify kernel support for the required virtio-mmio devices
- verify rootfs, Docker data disk, rng, networking, hostfwd, and one ordinary
  upstream `virtiofsd` export

This spike should not implement the final composed filesystem. It should only
remove uncertainty about the machine type and device model.

Required output:
- exact working QEMU command or a documented blocker
- list of kernel config/module requirements
- notes on whether user-mode networking and current `hostfwd` behavior survive
- notes on device count and any `microvm` limitations

Outcome:
- completed in `docker/microvm-spike.md`
- `microvm` is feasible with the current appliance when ACPI is disabled
- the working command uses non-PCI virtio devices and QEMU user-mode
  networking with explicit `-netdev` plus `virtio-net-device`
- hostfwd works for both Docker `_ping` and payload control

### Spike 2: `virtiofsd` Crate Embedding Feasibility

Purpose:
- prove that a repo-local Rust backend can reuse upstream `virtiofsd` public
  interfaces at the protocol boundary
- identify the crate version, feature flags, public APIs, and packaging model
- produce a minimal backend that can serve a trivial synthetic filesystem or
  document why that seam is not viable

Required output:
- minimal buildable Rust proof-of-concept or a documented blocker
- selected dependency/version strategy
- clear decision on whether to use `virtiofsd::vhost_user::VhostUserFsBackend`
  and implement `filesystem::FileSystem`
- fallback plan if the desired public seams are unavailable or unstable

Outcome:
- completed in `docker/virtiofsd-embedding-spike.md`
- `virtiofsd 1.13.3` exposes the needed public protocol-boundary APIs
- a local compile probe built `VhostUserFsBackend<ProbeFs>`, wrapped it in
  `VhostUserDaemon`, and created a vhost-user listener socket
- proceed with a repo-owned `ComposedFs` implementing
  `filesystem::FileSystem` and `SerializableFileSystem`

### Spike 3: Filesystem Semantics and Compatibility Baseline

Purpose:
- define the minimum filesystem behavior needed for the wrapper's real user
  experience
- avoid under-scoping the backend and discovering missing operations late

Required output:
- operation list for v1, including any deliberately unsupported operations
- expected behavior for `lookup`, `forget`, `open`, `release`, `flush`,
  `fsync`, `getattr`, `setattr`, `access`, `readlink`, `symlink`, `mkdir`,
  `create`, `unlink`, `rename`, `readdir`, xattrs, and readonly failures
- smoke commands for real tooling: shell navigation, `git`, package manager
  cache access, Docker bind mounts, Codex/Copilot auth/state access
- documented semantic compromises, especially inode identity and hardlink /
  rename behavior

Outcome:
- completed in `docker/filesystem-semantics-baseline.md`
- v1 must support normal writable development workflows, not only lookup/read
- required behavior includes lookup counts, open handles surviving
  rename/unlink, nested readonly enforcement, file mounts in parent `readdir`,
  `access` checks, `flush`/`fsync`, and xattr delegation where supported
- explicit v1 deferrals include POSIX locks, special-device `mknod`, cross-mount
  hardlinks/renames, live migration state, and rare operations such as
  `ioctl`, `poll`, `copyfilerange`, and `syncfs`

## Manifest Design

The host generates an authoritative manifest before backend startup.

Each manifest should include:
- schema version
- stable mount id
- guest absolute path
- normalized host source path
- mount kind: `dir` or `file`
- access mode: `rw` or `ro`
- source class: `workspace`, `tool-state`, `auth-config`, `system-ro`,
  `user-ro`, or `user-rw`
- bind target hint for guest reconstruction
- uid/gid and permission policy, if not inherited directly from the host

Example entries:
- `/home/user/project` -> `/home/user/project` (`rw`, `dir`, `workspace`)
- `/home/user/.codex` -> `/home/user/.codex` (`rw`, `dir`, `tool-state`)
- `/home/user/.config/gh` -> `/home/user/.config/gh` (`ro`, `dir`,
  `auth-config`)
- `/etc/ssl/certs` -> `/etc/ssl/certs` (`ro`, `dir`, `system-ro`)
- `/etc/hosts` -> `/etc/hosts` (`ro`, `file`, `system-ro`)

The backend enforces the manifest. It must not invent policy outside what the
manifest encodes.

Detailed outcome:
- host manifest and guest bind manifest are defined in
  `docker/composed-fs-manifest.md`
- v1 host manifest path:
  `.sandbox/docker-vm/run/composed-fs-manifest.json`
- v1 guest bind manifest path:
  `.sandbox/docker-vm/run/guest-config/composed-binds.json`
- v1 keeps the existing tiny config share for bind metadata until a replacement
  boot-config mechanism is documented

## Overlap and Conflict Rules

The host resolves conflicts before backend startup.

Required rules:
- guest paths must be absolute, normalized, and free of `..`
- duplicate guest targets must fail unless an explicit deterministic rule is
  documented for that source class pair
- more specific guest paths override broader guest paths
- explicit user `--ro` / `--rw` entries override implicit tool-state shares
  only when doing so does not override protected internal runtime paths
- readonly subtrees nested inside writable parents must remain readonly even
  when reached through the writable parent
- file-over-dir and dir-over-file conflicts must fail with a clear error
- missing required sources fail before QEMU starts
- missing optional tool/auth sources may be skipped only when the current
  wrapper behavior already treats them as optional

The backend receives a conflict-free mount table plus explicit nested boundary
metadata. It still must enforce readonly and mount-boundary behavior.

## `ComposedFs` Design

### Reuse Boundary

The intended reuse boundary is:
- reuse upstream `virtiofsd` at the vhost-user and virtio-fs protocol boundary
- implement a repo-owned `filesystem::FileSystem`
- treat `PassthroughFs` as implementation reference material, not as an
  internal subtree delegation engine
- avoid depending on private `passthrough` internals as stable APIs

Spike 2 may refine this if the public API reality differs.

### Internal Model

Current data structures:
- `MountSpec`: manifest entry plus stable mount id
- `NodeKind`: `SyntheticDir`, `OverlayDir`, `MountRoot`, and host-backed
  delegated nodes
- `Node`: inode number, node kind, synthetic fallback attributes, and lookup
  count
- `MountRuntime`: manifest mount plus root fd opened on backend startup
- future `HandleState`: open file fd, directory iterator, flags, access mode,
  originating inode

Synthetic nodes exist only in the virtual namespace. Mounted roots are explicit
manifest entries. Delegated host nodes are descendants under a mounted subtree
and may be materialized lazily.

Current outcome:
- implemented in `composed-fs/` and documented in
  `docker/composed-fs-core.md`
- `lookup`, `getattr`, and `readdir` now delegate to host-backed subtrees
- nested mount boundaries are represented by overlay directories that merge
  host directory entries with mounted child boundaries

### Inode Identity

Policy:
- stable synthetic inodes for `/`, synthetic parents, and mount roots
- delegated host-backed identity based on mount id plus host `dev` / `ino`
  metadata
- path metadata retained only for traversal, readdir, and invalidation
- lookup-count tracking that satisfies the `FileSystem` trait

Known remaining gap:
- open-handle lifetime and stale inode retirement after unlink/rename are not
  implemented until the operation-surface ticket adds file/dir handle tables

### Safe Host Path Resolution

The backend must never resolve guest paths by string-concatenating host paths.

For each host-backed mount:
- hold a root fd
- resolve descendants with fd-relative `openat2`
- reject `..` before host traversal
- preserve mount boundaries for nested ro/rw overrides through overlay nodes
- use `RESOLVE_IN_ROOT` and `RESOLVE_NO_MAGICLINKS`
- fall back to `openat` only after strict component validation if `openat2` is
  unavailable

The guest is untrusted. Backend path handling is security-sensitive.

### Readonly Enforcement

Readonly is manifest policy enforced by the backend.

Readonly mounts must reject mutating operations with `EROFS`, including:
- create
- unlink
- rename into, out of, or within readonly boundaries
- mkdir
- mknod
- symlink
- write
- truncating open flags
- mutating `setattr`
- mutating xattr operations

Readonly must hold even if the underlying host path is writable.

### V1 Operation Surface

The v1 operation list was finalized by the filesystem semantics baseline. The
implemented operation slice is documented in `docker/composed-fs-operations.md`.

Implemented:
- `lookup`
- `forget`
- `getattr`
- `access`
- `opendir`
- `readdir`
- `releasedir`
- `open`
- `create`
- `release`
- `flush`
- `fsync`
- `read`
- `write`
- `statfs`
- `readlink`
- `symlink`
- `mkdir`
- `mknod` for regular-file fallback and FIFO
- `unlink`
- `rmdir`
- `rename`
- `link`
- `lseek`
- `setattr`
- xattrs
- `fsyncdir`

Documented compromises:
- stale host-backed inodes retain cached attributes after unlink but are not
  retired until backend shutdown
- deferred locks, polling, live-migration state, `copyfilerange`, `syncfs`,
  `tmpfile`, and `fallocate` remain unsupported unless integration testing
  proves a real wrapper workload needs them

## Guest Init Changes

Guest init should replace the current share-per-tag manifest with a bind
manifest derived from the composed export.

The bind manifest should tell guest init:
- where the composed export is mounted
- which guest absolute paths to bind into place
- which parent directories to create first
- which binds are required versus optional
- what to do on failure

The current config share should be removed only after the composed export can
carry boot configuration safely, or after a replacement boot-config mechanism
is documented.

## Migration Strategy

Implementation order:

1. Complete and document the `microvm` boot/device spike.
2. Complete and document the `virtiofsd` crate embedding spike.
3. Complete and document the filesystem semantics spike.
4. Define the manifest schema, overlap rules, and bind manifest format.
5. Implement the composed filesystem namespace and safe traversal core.
6. Implement the composed backend behind a feature flag on the current `q35`
   path.
7. Add backend correctness and adversarial security tests.
8. Switch guest init to mount the composed export and bind selected paths.
9. Validate Docker, payload control, auth/state sharing, and real tool smoke
   commands on `q35 + composed fs`.
10. Add a `microvm` QEMU command branch using the documented spike result.
11. Validate `microvm + composed fs`.
12. Measure startup and readiness times against the current implementation.
13. Make the composed backend and `microvm` path default only after acceptance
    criteria are met.
13. Remove the old per-share export path after a fallback window.

This order avoids duplicate implementations while still using spikes to answer
unknowns. The filesystem remains critical: if a spike finds a blocker, the
required outcome is a documented alternate route to the same user experience,
not dropping the composed namespace goal.

## Test Plan

### Backend Correctness Tests

- synthetic directory lookup and `readdir`
- file mounts and directory mounts
- nested mount boundaries
- readonly enforcement for every mutating operation
- symlink and `..` escape attempts
- open file then rename/unlink behavior
- hardlink behavior, or documented unsupported semantics
- concurrent lookup/open/read/write/readdir behavior
- lookup count and inode lifetime behavior
- xattr behavior required by real tooling

Current unit-level coverage and residual integration gaps are documented in
`docker/composed-fs-tests.md`.

### VM Integration Tests

- current `q35 + composed fs` boots and reaches payload readiness
- `microvm + ordinary virtiofsd` spike boots before production migration
- `microvm + composed fs` boots and reaches payload readiness
- Docker daemon is reachable through the host socket proxy
- payload control path works
- project path inside the guest matches host path expectations
- Docker bind mounts using host-like paths work
- Codex/Copilot state and auth paths work
- `--ro`, `--rw`, `--no-net`, and `--docker-publish` behavior is preserved
- fallback to the old per-share path remains possible until removal

### Performance Tests

Compare:
- current `q35 + many shares`
- `q35 + composed fs`
- `microvm + composed fs`

Measure:
- host launch-to-QEMU start
- host launch-to-Docker ready
- host launch-to-payload ready
- number of host processes
- number of QEMU devices

The migration should define an acceptance threshold before changing defaults.

## Documentation Requirements

Every spike and major implementation milestone must leave durable notes in one
of:
- `plan.md`
- an adjacent design document under `docker/`
- the relevant `tk` ticket notes

Tickets should not be closed until the implementation result and any important
constraints are documented. If a ticket uncovers information relevant to a
later ticket, add a note to that later ticket before closing the current one.
