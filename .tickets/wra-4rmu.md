---
id: wra-4rmu
status: closed
deps: []
links: []
created: 2026-05-15T08:55:48Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Use shlex for shell quoting in payload scripts

Replace the hand-written shell_quote helper in vm-frontend/src/main.rs with a maintained shell quoting crate such as shlex. Relevant code builds payload scripts, setup tool bootstrap scripts, tool arg quoting, self-test payload commands, Docker command strings, and command override script assembly. Important nuance: legacy --command intentionally accepts shell syntax and must remain raw; only argv-like values should be quoted by the crate.

## Acceptance Criteria

shell_quote is removed or reduced to a thin wrapper over shlex. Existing script construction behavior is preserved semantically; tests that assert script contents are updated only for harmless quote formatting changes. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:18:43Z**

Reduced shell_quote in vm-frontend/src/main.rs to a thin shlex::try_quote wrapper. Existing --command compatibility remains raw; argv-like payload pieces still go through shell_quote. Validation: vm-frontend offline tests and fmt check pass.
