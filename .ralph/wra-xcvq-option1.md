# wra-xcvq: Option 1 Tokio-boundary architecture refactor

Work the `tk` epic `wra-xcvq` toward completion over at most 40 iterations.

## Epic Goal
Implement or explicitly supersede refactoring option 1: make agentvm a Tokio application at orchestration and byte-stream I/O boundaries, while keeping single-owner protocol/state cores synchronous and composed-fs/vhost filesystem request execution bounded blocking.

## Ground Rules
- Use `tk` for task state. Start/close child tickets as work begins/completes and add notes for significant implementation findings.
- Respect implementation dependencies from tickets; begin with unblocked foundational work.
- Prefer deletion, consolidation, and clearer module boundaries over layering more code.
- Config-file compatibility is the only compatibility requirement. If config parsing/serialization/defaults/field semantics change, update `vm-frontend/config-json.md` and tests in the same change.
- For arbitrary input/output changes, add or update fuzz coverage.
- If validation says the appliance must be rebuilt, tell the user exactly what needs rebuilding and why, then switch to other useful epic work until the user confirms rebuild completion.
- Do not mark tickets or the epic complete unless changes have passed live-capable validation.

## Initial Dependency-Aware Order
1. `wra-9glk` establish root workspace/shared crate boundaries.
2. `wra-09v9` split `vm-frontend/src/main.rs` into responsibility modules.
3. `wra-r070` typed domain errors, then `wra-rjt4` tracing.
4. `wra-n0fe` Tokio application edge/supervisor skeleton.
5. Protocol/shared work: `wra-cvmy`, then `wra-jkeg`, `wra-662v`, `wra-yl7i` backend boundaries as applicable.
6. Launch/runtime async edge work: `wra-zqci`, `wra-73tn`, `wra-f762`, `wra-gq8e`.
7. Cleanup/refinement: `wra-1esa`, `wra-6n71`, `wra-0a0r`, and any bugs discovered.

## Key Files From Epic
- `vm-frontend/src/main.rs`
- `vm-frontend/src/launch.rs`
- `vm-frontend/src/vmnet_runtime.rs`
- `vm-frontend/src/vmnet_poller.rs`
- `vm-frontend/src/vmnet_service_io.rs`
- `vm-frontend/src/payload_client.rs`
- `vm-frontend/src/docker_proxy.rs`
- `guest-service/src/lib.rs`
- `composed-fs/src/lib.rs`

## Checklist
- [ ] Inspect current ticket graph and select the next unblocked child ticket.
- [ ] Start selected ticket with `tk start <id>` and add planning note.
- [ ] Implement the selected ticket with minimal, idiomatic changes.
- [ ] Update tests, fuzz targets, and docs relevant to the change.
- [ ] Run focused tests while iterating.
- [ ] Run `./vm-frontend/validate.sh required` before considering code complete.
- [ ] If appliance rebuild is required, notify user and continue non-blocked work until confirmation.
- [ ] Close completed child tickets only after required validation passes in live-capable environment.
- [ ] Continue through dependent tickets until `wra-xcvq` is implemented or explicitly superseded.

## Verification Log
- 2026-05-17: Read `tk show wra-xcvq`; initial child/dependency ordering captured from ticket output.

## Notes
- `wra-yl7i` owns TUI control-plane semantics; option 1 supervisor/payload tickets should provide clean backend boundaries rather than reimplement TUI coupling.
- Intended async boundary: Tokio at orchestration and byte-stream edges; synchronous deterministic smoltcp/protocol cores; synchronous composed-fs/vhost filesystem execution on bounded blocking workers.
