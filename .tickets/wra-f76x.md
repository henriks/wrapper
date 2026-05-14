---
id: wra-f76x
status: closed
deps: []
links: []
created: 2026-05-14T18:41:01Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, planning]
---
# Define exhaustive validation matrix and test taxonomy

Define the canonical validation matrix for the VM frontend so future tests are comprehensive rather than ad hoc. Cover both enforcement surfaces: userspace vmnet and composed filesystem/runtime sharing.\n\nNetwork dimensions to enumerate: QEMU stream framing, Ethernet/ARP/DHCP/IPv4/TCP/UDP/DNS, policy matrix, TCP state transitions, TCP backpressure, HTTP parsing/proxying, HTTPS MITM, DNS failure modes, host ingress/published ports, and live QEMU behavior.\n\nFilesystem dimensions to enumerate: manifest validation, guest path protection, source classes, lookup/stat/read/write/dir/rename/unlink/truncate/fsync semantics, readonly enforcement, mount boundaries, symlink policy, host reflection, error mapping, concurrency, virtiofs protocol operations, config fs, tool/home/auth sharing, and reset/persistence behavior.\n\nDocument test tiers: fast offline unit tests, in-process integration tests, model/property-style tests, ignored local stress tests, and live KVM/QEMU tests.

## Acceptance Criteria

A checked-in validation matrix or design document names all test dimensions, maps each dimension to an intended test tier, and explicitly records known gaps. The document explains that closing implementation tickets requires documenting coverage outcomes and remaining risks.


## Notes

**2026-05-14T18:47:33Z**

Added vm-frontend/validation-matrix.md as the canonical validation taxonomy for the Rust VM frontend. It covers vmnet, composed-fs, runtime sharing, test tiers, implementation order, outcome documentation requirements, and known gaps. Also linked it from vm-frontend/README.md.
