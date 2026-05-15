---
id: wra-i4qr
status: closed
deps: []
links: []
created: 2026-05-15T08:55:54Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-sne0
---
# Move composed-fs CLI parsing to clap

Replace the custom CLI parser in composed-fs/src/lib.rs run_cli and related option parsing with clap, matching the vm-frontend parser direction. Current code manually handles required args, help, numeric parsing, and error strings around the composed-fs server CLI. Candidate crate: clap derive or builder API.

## Acceptance Criteria

composed-fs has clap as a direct dependency and no longer hand-parses its top-level CLI args. Help/errors are generated or normalized through clap. cargo test --manifest-path composed-fs/Cargo.toml --offline passes. Any docs or validation commands that mention composed-fs CLI options still match behavior.


## Notes

**2026-05-15T09:18:56Z**

Moved composed-fs top-level CLI parsing to clap derive. Args now declare required --manifest/--socket-path and defaults for --tag/--thread-pool-size via clap, removing next_arg/print_usage/manual loop. Validation: composed-fs offline tests and fmt check pass.
