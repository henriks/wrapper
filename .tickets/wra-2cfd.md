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
