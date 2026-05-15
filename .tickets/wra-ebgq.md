---
id: wra-ebgq
status: closed
deps: [wra-x2tl]
links: []
created: 2026-05-15T08:56:01Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-sne0
---
# Convert vm-frontend clap builders to typed parsers

Follow up the clap migration in vm-frontend/src/main.rs by reducing stringly ArgMatches extraction. Current code defines clap builder commands then repeatedly calls get_one/get_many and parses strings for wrapper, launch/prepare, vmnet-gateway, payload-client, and self-test. Candidate approach: clap derive structs/enums or custom FromStr value_parser types for IP:PORT, HOST:GUEST, KEY=VALUE, durations, setup tools, and guest tools.

## Acceptance Criteria

At least the wrapper and one low-level subcommand use typed clap parsing or value_parser types instead of stringly get_one/get_many extraction. Error messages remain acceptable. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes. Ticket notes identify any remaining parser sections intentionally left builder-based.


## Notes

**2026-05-15T09:19:47Z**

Added typed clap parsing for wrapper --project as PathBuf and --docker-publish as PortPair, plus vmnet-gateway --socket as PathBuf, --prefix-len as u8, and listener/publish options as PortPair. Remaining sections stay builder/get_one based where they mostly forward raw strings or compatibility flags. Validation: vm-frontend offline tests and fmt check pass.
