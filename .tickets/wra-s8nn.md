---
id: wra-s8nn
status: closed
deps: [wra-gx4r]
links: []
created: 2026-05-14T18:41:57Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, filesystem]
---
# Expand composed filesystem manifest and path validation tests

Add exhaustive fast tests for composed filesystem and config filesystem manifest validation. Cover absolute host path requirements, rejection of . and .. components, missing required paths, optional missing paths, duplicate guest paths, protected guest paths, source class metadata, readonly flags, bind flags, required flags, generated config mounts, and CA cert mount visibility.\n\nAlso test path mapping for workspace, guest home under .sandbox/home, tool state dirs, .docker, optional gh config, explicit --ro and --rw, and system config fs mounts.\n\nRelevant code: vm-frontend/src/runtime_manifest.rs, vm-frontend/src/launch.rs policy_config_mounts, vm-frontend/src/main.rs runtime_mounts/guest_payload_env, composed-fs manifest handling.

## Acceptance Criteria

Fast tests cover every manifest/path invariant in the validation matrix. Private MITM CA key is asserted not to appear in guest config fs. Notes document any intentionally unsupported path or mount shape.


## Notes

**2026-05-14T18:52:05Z**

Reuse composed-fs/src/test_support.rs for manifest/path validation tests instead of creating duplicate fixture trees. Relevant helpers: GuestShareFixture for workspace/readonly/config mounts, manifest_with_mounts, dir_mount, file_mount, lookup helpers, and VecReader/VecWriter for operation setup.

**2026-05-14T18:59:53Z**

Expanded fast manifest/path validation. vm-frontend runtime_manifest tests now cover MITM CA cert exposure without private key leakage, Copilot tool-state sharing and gh opt-in behavior, relative host path rejection, guest dotdot rejection, duplicate mount IDs, duplicate guest paths, missing required source rejection, optional missing source skipping, and bind manifest omission for skipped mounts. composed-fs tests now cover schema mismatch, empty manifest, duplicate guest path, protected guest path, relative host path, guest dotdot, and missing host source behavior. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 97 lib tests passed, 1 ignored, and 18 bin tests passed; cargo test --manifest-path composed-fs/Cargo.toml --offline passed with 20 tests.
