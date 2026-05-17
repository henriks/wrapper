# Complete `wra-xcvq`: Option 1 Tokio-boundary architecture refactor

## Goal
Work dependency-aware through the remaining `tk` epic `wra-xcvq` until the epic is either fully implemented or explicitly superseded, with all child tickets closed or intentionally cancelled, and live-capable validation documented.

## Current known remaining children
- `wra-662v` in progress: opt-in Rust/Tokio guest payload service.
  - Implementation exists, default appliance required validation passes, but current `docker/out/artifact-manifest.json` is Python-default and does not include `agentvm_payload_service=rust`.
  - Remaining decision: build/live-smoke Rust opt-in appliance, or explicitly narrow/close implementation ticket with Rust appliance parity/default-switch validation owned by `wra-y335`.
- `wra-f762` open: replace vmnet poller and service I/O with Tokio actor.
- `wra-yl7i` open: make TUI a control-socket frontend to the agentvm supervisor.
- `wra-0a0r` open: refactor composed-fs as bounded blocking vhost/filesystem backend.

## Ground rules
- Use `tk` for ticket state. Start/close child tickets as work begins/completes and add notes for significant findings.
- Required validation before closing tickets: `./vm-frontend/validate.sh required`.
- If validation says appliance must be rebuilt, tell the user exactly what to rebuild and why, then switch to other useful epic work until rebuild is confirmed.
- Do not close tickets/epic without live-capable validation.
- Preserve `.sandbox/config.json` compatibility. If config parsing/serialization/defaults/field semantics change, update `vm-frontend/config-json.md` and tests in the same change.
- Prefer small mechanical/additive/deletion-focused slices. More code is a liability.
- Keep composed-fs/config-fs/vhost filesystem execution bounded blocking; do not move filesystem request execution onto Tokio core workers.
- Keep vmnet/smoltcp core synchronous/single-owner; do not introduce `Arc<Mutex<_>>` around `VmnetGateway`/`VmnetCore` state.
- Keep Python guest payload service default until `wra-y335` parity/live validation/default-switch gate.
- For arbitrary input/output changes, add or update fuzz coverage.

## Initial priorities
1. Inspect remaining ticket details/dependencies and update this loop task with current state.
2. Resolve `wra-662v` scope/validation first if possible: since the user rebuilt the default appliance but not Rust opt-in, decide whether a Rust opt-in rebuild is available or document/narrow handoff to `wra-y335`.
3. Then choose the next dependency-aware remaining child (`wra-f762`, `wra-yl7i`, or `wra-0a0r`) and implement small validated slices.
4. Keep updating `.ralph/wra-xcvq-option1.md` and this task file with progress.

## Progress
- [x] Iteration 1: inspected remaining epic child status.
  - `wra-662v` is still in progress; it is implemented substantially but current `docker/out/artifact-manifest.json` is Python-default and does not include `agentvm_payload_service=rust`.
  - Remaining open option-1 children are `wra-f762`, `wra-yl7i`, and `wra-0a0r`.
- [x] Iteration 1: attempted to unblock Rust opt-in appliance validation.
  - Built `target/debug/agentvm-guest-service` successfully with `cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service`.
  - Attempted non-interactive opt-in rebuild: `sudo -n env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=$(pwd)/target/debug/agentvm-guest-service ./docker/build-appliance.sh`.
  - Rebuild failed because `sudo` requires a password in this environment.
  - Added a `wra-662v` note with the exact user-side Rust opt-in rebuild command needed: `sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh`.
  - `wra-662v` remains open; Python remains default until `wra-y335` passes.
- [ ] Next: while Rust opt-in rebuild is blocked, start the next useful epic child deliberately, likely `wra-f762` if taking vmnet Tokio actor work, or `wra-0a0r` if preferring bounded-blocking composed-fs cleanup.

## Reflection
- Iteration 1: the user rebuild refreshed the default appliance but did not produce a Rust opt-in appliance. Since this process cannot run privileged rebuilds non-interactively, `wra-662v` cannot honestly be closed as Rust opt-in live-validated now. Continue useful epic work, but do not switch the payload-service default or claim Rust appliance parity.
