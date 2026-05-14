# VM Frontend Validation Matrix

Ticket: `wra-f76x`

Date: 2026-05-14

This document is the canonical validation plan for the Rust VM frontend. It
covers the two external enforcement surfaces:

- the rootless userspace network gateway under `vm-frontend/src/`
- the composed filesystem/runtime sharing path under `composed-fs/src/`,
  `vm-frontend/src/runtime_manifest.rs`, and `docker/guest-init.sh`

The goal is exhaustive coverage without duplicating implementation work. The
matrix defines what must be tested and which tier should own it. Follow-up
tickets should add reusable harnesses and tests against this matrix, then record
the outcomes and any remaining gaps before they are closed.

## Test Tiers

Use the lowest tier that proves the behavior.

| Tier | Command shape | Purpose |
| --- | --- | --- |
| Fast offline unit | `cargo test --manifest-path vm-frontend/Cargo.toml --offline`; `cargo test --manifest-path composed-fs/Cargo.toml --offline` | Pure Rust behavior with no QEMU, no KVM, no real network dependency. This is the default regression gate. |
| In-process integration | Same cargo commands, using local fake upstreams, fake QEMU streams, temp projects, and generated CA material | Cross-module behavior that needs realistic bytes, sockets, manifests, or async pumps but can stay deterministic. |
| Model/property style | Cargo ignored tests or deterministic seeded tests | Large operation spaces where example tests are insufficient, especially filesystem semantics and network chunking/backpressure. Failures must print the seed or operation sequence. |
| Local stress/adversarial | Ignored cargo tests run explicitly on a developer machine | Higher volume, timing, malformed input, concurrency, and resource pressure. These must be deterministic enough to reproduce from logged artifacts. |
| Live KVM/QEMU | `agentvm-frontend self-test` and targeted `launch` smokes on a host with `/dev/kvm`, QEMU, and rebuilt image artifacts | Final contract validation against the real guest kernel, init scripts, virtiofs, QEMU stream netdev, Docker, and tool payload path. |

## Outcome Requirements

Every implementation ticket under `wra-tad5` must update ticket notes and, when
useful, a checked-in validation result document with:

- commands run
- pass/fail result
- important logs or artifact paths
- known residual gaps
- regressions captured by name, especially for prior failures
- any behavior found that changes another ticket's scope

The existing result documents are `vm-frontend/vmnet-runtime-validation.md`,
`docker/composed-fs-tests.md`, and `docker/microvm-composed-validation.md`.
Future work may append to those or create focused result documents when a test
tier becomes large enough to deserve its own file.

## Network Matrix

| Area | Required coverage | Primary tier | Follow-up ticket |
| --- | --- | --- | --- |
| QEMU stream framing | Big-endian frame lengths, partial reads, multiple frames per read, truncated frames, oversized frames, EOF, `WouldBlock`, malformed length, pcap record emission. | Fast offline unit; in-process integration | `wra-y325` |
| Ethernet parsing | Valid Ethernet II frames, short frames, unknown ethertypes, broadcast/unicast targeting, gateway MAC selection, guest MAC tracking. | Fast offline unit | `wra-y325` |
| ARP | Gateway ARP reply, non-gateway target ignored, malformed ARP denied, duplicate requests, guest MAC learning, emitted frame shape. | Fast offline unit | `wra-y325` |
| DHCPv4 | Discover/offer, request/ack, fixed lease, router, DNS, MTU, lease time, wrong server ID, unknown message types, malformed options. | Fast offline unit | `wra-y325` |
| IPv4 filtering | Checksum handling, fragments, non-IPv4 denial, unsupported protocols, private and metadata ranges, public ranges, no-net mode, explicit allow rules. | Fast offline unit | `wra-y325` |
| UDP policy | DNS-only gateway UDP/53, UDP/443 denial to prevent QUIC, non-DNS UDP denial, malformed UDP, logging for denied paths. | Fast offline unit; live KVM/QEMU | `wra-y325`, `wra-9udq` |
| DNS proxy | Allowed and blocked domains, wildcard policy, CNAME/A/AAAA responses, upstream timeout/failure, malformed query, refused responses, repeated queries, query logging. | In-process integration | `wra-bc6k` |
| TCP state machine | SYN accept/deny, SYN reset on policy denial, handshake, payload reads, FIN, RST, half-close, simultaneous sessions, port reuse, upstream connection failure. | Fast offline unit; in-process integration | `wra-y325`, `wra-czl0` |
| TCP backpressure | smoltcp send-buffer limits, partial guest writes, pending outbound buffers, upstream backpressure, large responses, no silent truncation. | In-process integration; stress/adversarial | `wra-czl0`, `wra-96uv` |
| HTTP/80 proxy | Host header parsing, absolute and origin-form requests, fragmented headers, large headers, multiple requests, upstream errors, policy logging. | In-process integration | `wra-czl0` |
| HTTPS/443 MITM | Missing CA fail-closed, generated CA load, per-host cert generation, SNI handling, guest TLS termination, upstream TLS, large TLS responses, fragmented records, certificate trust env. | In-process integration; live KVM/QEMU | `wra-czl0`, `wra-9udq` |
| Policy matrix | Default deny, wrapper default public egress, `--no-net`, allowed domain/IP, denied private/metadata IPs, blocked UDP/443, IPv6 denied, unsupported protocols denied. | Fast offline unit; live KVM/QEMU | `wra-y325`, `wra-9udq` |
| Host ingress | Payload listener, Docker listener, published TCP ports, no-net interaction, multiple listeners, close/error propagation, backpressure, event logs. | In-process integration; live KVM/QEMU | `wra-bc6k`, `wra-9udq` |
| Capture and observability | pcap file shape, `vmnet-events.log` content, denied preaccept logs, DNS decisions, TCP connect events, TLS failure detail, partial-send diagnostics without leaking secrets. | Fast offline unit; live KVM/QEMU | `wra-mvxd` |

## Filesystem And Runtime Sharing Matrix

| Area | Required coverage | Primary tier | Follow-up ticket |
| --- | --- | --- | --- |
| Manifest parsing | Schema version, absolute paths, required source paths, optional paths, duplicate guest paths, invalid JSON, invalid source classes, safe defaults. | Fast offline unit | `wra-s8nn` |
| Guest path protection | Reject relative paths, `..`, empty components, protected runtime paths, overlapping mounts, symlink escape attempts, host private-key exclusion. | Fast offline unit | `wra-s8nn` |
| Source classes | Workspace, persistent home, requested `--ro`, requested `--rw`, Docker state, tool state, GitHub config, AWS profile, config fs, MITM CA cert mount. | Fast offline unit; live KVM/QEMU | `wra-s8nn`, `wra-jwaz`, `wra-9udq` |
| Read operations | lookup, getattr, open, read, readlink, readdir, statfs, lseek, xattrs where supported, stable inode reuse. | Fast offline unit; model/property style | `wra-0452`, `wra-1joy` |
| Write operations | create, write, append, overwrite, truncate, mkdir, rename, unlink, rmdir, link, symlink, chmod, fsync, flush, mknod fallback. | Fast offline unit; model/property style | `wra-0452`, `wra-1joy` |
| Readonly enforcement | Reject mutating operations on readonly mounts, readonly directory mutation, readonly setattr, cross-mount rename/link, correct errno mapping. | Fast offline unit; live KVM/QEMU | `wra-s8nn`, `wra-0452`, `wra-9udq` |
| Mount boundaries | Nested mounts, file mounts, directory mounts, overlapping entries, cross-mount operations, open-handle behavior after unlink or rename. | Model/property style | `wra-0452`, `wra-1joy` |
| Host reflection | Host-created files visible to guest, guest-created files visible to host, host mutation while mounted, stale inode behavior, reset behavior. | Model/property style; live KVM/QEMU | `wra-0452`, `wra-9udq`, `wra-96uv` |
| Error mapping | `ENOENT`, `EEXIST`, `ENOTDIR`, `EISDIR`, `EROFS`, `EXDEV`, `EPERM`, unsupported operations, malformed requests. | Fast offline unit; model/property style | `wra-0452`, `wra-1joy` |
| Virtiofs protocol | lookup/forget counts, open/release, opendir/releasedir, readdir offsets, request IDs, inode lifetime, unsupported/malformed operations. | In-process integration; stress/adversarial | `wra-1joy` |
| Concurrency | Concurrent reads, concurrent writes, open-then-delete, open-then-rename, directory mutation during listing, lookup/forget churn. | Stress/adversarial | `wra-1joy`, `wra-96uv` |
| Wrapper contract | Absolute project/run dirs, `wrap --tool`, rejected legacy flags, public egress default, `--no-net`, `--reset`, payload env, HOME/XDG, Docker env. | Fast offline unit; live KVM/QEMU | `wra-jwaz`, `wra-9udq` |
| Config fs and secrets | Guest-visible config files, composed bind config, CA cert bundle, no CA private key in guest config fs, no accidental host secret mounts. | Fast offline unit; live KVM/QEMU | `wra-s8nn`, `wra-jwaz`, `wra-9udq` |
| Tool and auth sharing | Codex/Copilot state, npm/node CA env, GitHub auth when requested, AWS profile when requested, Docker client config, persistence across launches. | Fast offline unit; live KVM/QEMU | `wra-jwaz`, `wra-9udq` |
| Observability | Manifest summaries, source class IDs, path context on failures, operation traces, reset logs, live artifact collection. | Fast offline unit; live KVM/QEMU | `wra-mvxd` |

## Known Gaps

These gaps are expected at the start of `wra-tad5` and should be retired by the
follow-up tickets:

- no single reusable validation harness for temp projects, generated CA
  material, fake upstreams, fake QEMU streams, and guest-share fixture trees
- network coverage is broad but still too example-driven for chunking,
  backpressure, malformed input, and simultaneous sessions
- recent HTTPS corruption from dropped partial TCP sends was caught manually,
  not by a pre-existing regression test that exercised large TLS responses end
  to end
- live KVM coverage exists as targeted smoke results, but not as an extensive
  self-test suite covering every frontend contract dimension
- filesystem coverage has strong backend examples, but not enough
  model/property-style operation sequences against host-backed fixture trees
- virtiofs protocol edge cases and concurrency need explicit adversarial tests
- runtime sharing coverage needs more assertions for secret exposure, CA bundle
  behavior, project-local tool state, and reset/persistence rules
- diagnostics are useful, but tests do not yet assert that failure artifacts are
  complete and safe to share

## Implementation Order

1. Build shared deterministic harnesses (`wra-gx4r`) so later tickets reuse the
   same fixtures and do not recreate incompatible test scaffolding.
2. Expand fast packet, policy, manifest, and path validation coverage
   (`wra-y325`, `wra-s8nn`).
3. Add deeper integration and model tests for TCP/HTTP/HTTPS and filesystem
   operations (`wra-czl0`, `wra-0452`).
4. Add DNS, host-ingress, virtiofs protocol, and wrapper contract coverage
   (`wra-bc6k`, `wra-1joy`, `wra-jwaz`).
5. Run full live KVM validation over the real frontend contract (`wra-9udq`).
6. Add opt-in stress/fuzz-style suites and artifact assertions (`wra-96uv`,
   `wra-mvxd`).
7. Document exact commands, runtimes, prerequisites, and CI tiers (`wra-k3di`).
