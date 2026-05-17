---
id: wra-9glk
status: closed
deps: []
links: []
created: 2026-05-17T10:18:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, workspace]
---
# Option 1: establish root workspace and shared crate boundaries

Create a real root Cargo workspace for vm-frontend, guest-service, composed-fs, and third_party/virtiofsd so shared crates can be extracted cleanly. Centralize dependency versions where sensible. This unlocks agentvm-payload-protocol and other library splits without path/version drift.

## Design

Keep this mechanical and narrow. Do not refactor behavior here. Ensure existing package commands still work, including vm-frontend/fuzz. Be careful with third_party/virtiofsd membership and any tooling that currently assumes per-crate Cargo.toml files.

## Acceptance Criteria

cargo metadata works from the repo root, existing per-crate builds/tests still run, no behavior changes are introduced, and validation docs or scripts are updated if workspace invocation changes.


## Notes

**2026-05-17T11:13:16Z**

Starting with a mechanical workspace pass. Initial inspection found crate manifests at vm-frontend, guest-service, composed-fs, third_party/virtiofsd, and vm-frontend/fuzz. Root currently has no Cargo.toml; Makefile and validation still use per-manifest cargo commands, so the first implementation should add a root workspace without changing those commands unless necessary. Need keep vm-frontend/fuzz working and be cautious with vendored third_party/virtiofsd generated manifest.

**2026-05-17T11:18:02Z**

Implemented the mechanical root workspace slice: added root Cargo.toml with members composed-fs, guest-service, vm-frontend, and third_party/virtiofsd; moved shared package metadata and common dependency declarations into workspace.package/workspace.dependencies for local crates; added an empty workspace table to vm-frontend/fuzz so cargo-fuzz remains independently invocable; added root target/ to .gitignore; generated a root Cargo.lock and removed stale per-member lockfiles (fuzz keeps its own lockfile). Existing per-manifest commands still work. Validation so far: cargo metadata --offline from repo root lists the four workspace packages; cargo check --manifest-path vm-frontend/fuzz/Cargo.toml --offline passed; cargo test --manifest-path guest-service/Cargo.toml --offline passed; cargo test --manifest-path composed-fs/Cargo.toml --offline passed; cargo test --manifest-path vm-frontend/Cargo.toml --offline passed; cargo fmt --manifest-path for composed-fs, guest-service, vm-frontend, and fuzz passed. Note: cargo emits an existing workspace warning that third_party/virtiofsd has a member-local [profile.release] that is ignored under the root workspace.

**2026-05-17T11:21:46Z**

Full required validation passed after workspace changes: ./vm-frontend/validate.sh required completed successfully, including docs/fmt/offline tests, guest-services, fuzz-check, live-smoke, and live-setup-tools. No appliance rebuild was requested by validation.
