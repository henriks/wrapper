---
id: wra-y335
status: closed
deps: [wra-n0xc, wra-662v]
links: [wra-662v, wra-ylcz, wra-vd8g]
created: 2026-05-17T06:03:06Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [guest, rust, validation]
---
# Validate Rust guest-service parity before default switch

Follow-up from wra-lcbk. Once an opt-in Rust guest-service appliance path exists, run parity validation against the bounded Python baseline. Preserve payload semantics from docker/guest-service-rust-spike.md: PTY/session/process-group behavior, quiet long-running primary payloads, signal/resize/stdin frames, diagnostic limits/timeouts, client/session caps, slow-writer cleanup. Preserve Docker bridge semantics: TCP-to-/var/run/docker.sock binary relay, connect retry, session limits, idle/write-failure close, summary logging.

## Acceptance Criteria

Opt-in Rust guest service passes ./vm-frontend/validate.sh live-payload, live-docker, and required. Any parity gaps are filed before switching defaults. Default switch is a separate explicit ticket.


## Notes

**2026-05-17T11:03:02Z**

Relationship to refactoring option 1: wra-662v owns implementing the real Rust/Tokio guest payload service using the shared payload protocol. This ticket owns parity/live validation of the opt-in Rust guest-service path before any default switch. Do not merge these unless intentionally combining implementation and validation into one large appliance-sensitive ticket; keeping them separate preserves the important gate that default switch/removal of Python is explicit and later.

**2026-05-17T21:31:49Z**

Current handoff from wra-662v/wra-xcvq: Rust guest-service implementation and source-level opt-in appliance wiring have focused tests passing, and default required validation has passed with the Python-default appliance. The remaining parity gate still requires a privileged opt-in appliance build using: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/debug/agentvm-guest-service ./docker/build-appliance.sh. Then inspect docker/out/artifact-manifest.json for agentvm_payload_service=rust and run live-payload/live-docker/required before any default switch. Python remains default.

**2026-05-18T05:39:38Z**

Cleanup epic wra-9m5h adds wra-o75s as the post-parity deletion ticket. During parity validation, explicitly decide Docker socket bridge ownership: either it is in Rust guest-service scope or it is documented as a separate surviving component. The outcome should enable deleting the losing Python/Rust duplicate path rather than keeping both indefinitely.

**2026-05-18T08:16:07Z**

wra-vd8g changed the Rust guest-service opt-in contract: parity rebuilds should no longer use target/debug/agentvm-guest-service. Build or supply an Alpine/musl-compatible binary, e.g. cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service, then run sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/x86_64-unknown-linux-musl/debug/agentvm-guest-service ./docker/build-appliance.sh. The appliance builder now rejects glibc-linked /lib64/ld-linux-x86-64.so.2 binaries before boot.

**2026-05-18T08:34:40Z**

Cleanup loop iteration 31 start: wra-vd8g is closed after required validation, so this is now the only blocker for cleanup child wra-o75s. Parity validation should use the musl guest binary path documented by wra-vd8g, then run an opt-in Rust appliance through live-payload, live-docker, and required before deciding whether wra-o75s can delete the Python path/PAYLOAD_SERVICE selector. First audit step is to verify the musl binary exists/builds and whether this harness can run the privileged opt-in appliance rebuild.

**2026-05-18T08:35:01Z**

Iteration 31 audit: built the documented Rust guest-service musl target successfully with cargo build --bin agentvm-guest-service --target x86_64-unknown-linux-musl --offline. file reports it as an x86-64 static-pie ELF; readelf shows no dynamic interpreter, so it satisfies the wra-vd8g appliance compatibility gate. This harness still cannot perform the required privileged opt-in appliance rebuild: sudo -n true fails with 'sudo: a password is required'. Next host/operator command needed: sudo env AGENTVM_PAYLOAD_SERVICE=rust AGENTVM_GUEST_SERVICE_BIN=/home/hsaksela/ai/wrapper/target/x86_64-unknown-linux-musl/debug/agentvm-guest-service ./docker/build-appliance.sh, then run ./vm-frontend/validate.sh live-payload, ./vm-frontend/validate.sh live-docker, and ./vm-frontend/validate.sh required against the Rust appliance.

**2026-05-18T08:38:59Z**

Iteration 32 Rust parity validation passed after host/operator opt-in appliance rebuild. Manifest includes target/x86_64-unknown-linux-musl/debug/agentvm-guest-service, and live console logs from live-payload/live-docker/required runs show 'agentvm-init: starting rust payload server', confirming the Rust guest service was exercised. Validation passed: ./vm-frontend/validate.sh live-payload (log /tmp/pi-bash-de2083918b23d3cc.log), ./vm-frontend/validate.sh live-docker, and ./vm-frontend/validate.sh required (log /tmp/pi-bash-1bcff5349d4a3aa7.log). Docker bridge behavior remains covered by the separate Python guest-socket-bridge component in the appliance; wra-o75s should delete the Python payload server path and PAYLOAD_SERVICE selector/default fork, not the Docker bridge unless a separate bridge replacement is chosen. Closing wra-y335; default switch/removal remains explicit under wra-o75s.
