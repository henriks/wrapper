---
id: wra-2cfd
status: closed
deps: []
links: []
created: 2026-05-15T06:50:42Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [validation, fuzzing, filesystem, network]
---
# Harden filesystem and network fuzz testing strategy

Follow-up epic to turn the current proptest/regression suite into a stronger adversarial fuzzing strategy for the composed filesystem and vmnet network gateway. Current state: composed-fs has useful model/property tests in composed-fs/src/lib.rs, vmnet has proptest coverage in vm-frontend/src/vmnet_stream.rs, vm-frontend/src/vmnet_gateway.rs, vm-frontend/src/tcp_gateway.rs, and DNS stress in vm-frontend/src/dns_proxy.rs. Gaps identified: vm-frontend/validate.sh stress only runs a subset of ignored stress tests; there is no coverage-guided fuzzing layer; filesystem generators use narrow fixed ASCII path/name spaces; randomized FS tests are sequential and miss race/interleaving classes; network fuzzing rarely creates meaningful packet/session states.

## Acceptance Criteria

Epic is complete when the stress tier runs all intended ignored stress tests, coverage-guided fuzz targets or equivalent harnesses exist for the high-risk filesystem and network parsers/state machines, filesystem generators cover path/name/metadata edge cases, deterministic concurrency/interleaving coverage exists, network state fuzzing covers structured packet/session behavior beyond arbitrary bytes, and validation docs explain commands, reproduction, seeds/corpora, and CI/manual tier policy.


## Notes

**2026-05-15T06:52:59Z**

Created from fuzz strategy review on 2026-05-15. Implementation order: first fix stress tier wiring (wra-reoq); then add coverage-guided harness infrastructure (wra-k8br), expand composed-fs generators (wra-rysl), and add structured vmnet packet/session properties (wra-735i); then add deterministic composed-fs interleavings (wra-12nu) and concrete coverage-guided fuzz targets (wra-o7ax); finish by documenting workflow, corpora, reproduction, and CI/manual policy (wra-yxtb).

**2026-05-15T07:10:58Z**

Epic implementation completed. Closed all children: stress tier wiring, cargo-fuzz harness infrastructure, composed-fs generator expansion, structured vmnet packet/session properties, deterministic composed-fs interleavings, concrete fuzz targets, and docs. Final verification: vm-frontend/validate.sh fast, vm-frontend/validate.sh stress, cargo check --manifest-path vm-frontend/fuzz/Cargo.toml, and tk dep cycle all passed.

**2026-05-15T07:19:56Z**

Added repo-root Makefile targets and vm-frontend/fuzz/run.sh for cargo-fuzz workflows. Runner defaults to nightly, writes logs/artifacts/mutable corpora to ignored directories, seeds mutable corpora from curated named seed files, and supports smoke, extended, single-target, repro, and minimize flows.

**2026-05-15T07:37:12Z**

Added coverage-guided composed-fs operation and lock fuzz targets. composed_fs_ops drives bounded mkdir/put/read/rename/unlink/rmdir/host mutation/readdir/truncate/chmod/readonly/cross-mount sequences against a host oracle and final path-content checks; composed_fs_locks drives setlk/getlk/flush owner/range sequences against an in-memory model. Full make fuzz-smoke found and fixed DNS parser panic boundary by rejecting guest DNS packets with resource-record sections before hickory parsing. Verification: cargo check fuzz package, focused DNS regression, composed-fs fuzzing feature compile, make fuzz-smoke FUZZ_SMOKE_TIME=1 FUZZ_JOBS=1 FUZZ_WORKERS=1.

**2026-05-15T07:45:32Z**

Pinned dns_proxy_payload crash-e24b86c7cbb8c268ac47ffd7286bfe88982b5e57 as a normal regression test. The artifact now routes through the DNS query resource-record pre-parse rejection and logs Malformed without response. Verification: cargo test dns_resource_record_payload_is_rejected_before_parser, cargo test dns_parser_panic_payload_is_logged_as_malformed, make fuzz-repro FUZZ_TARGET=dns_proxy_payload FUZZ_ARTIFACT=vm-frontend/fuzz/artifacts/dns_proxy_payload/crash-e24b86c7cbb8c268ac47ffd7286bfe88982b5e57, make fuzz-target FUZZ_TARGET=dns_proxy_payload FUZZ_TIME=1 FUZZ_JOBS=1 FUZZ_WORKERS=1.
