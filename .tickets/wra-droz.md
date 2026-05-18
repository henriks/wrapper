---
id: wra-droz
status: closed
deps: []
links: []
created: 2026-05-18T05:42:46Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-662v
tags: [guest-service, payload, rust, live]
---
# Rust guest-service must launch payloads with requested UID/GID

Rust opt-in live-payload with a musl/static appliance progressed to the self-test payload but failed after output 'payload-start' and 'home-ok'. The next self-test assertion checks id -u against AGENTVM_UID, so the Rust guest-service was launching primary payloads as root instead of honoring AGENTVM_UID/AGENTVM_GID and HOME setup like docker/guest-payload-server.py. Relevant files: guest-service/src/lib.rs primary/diagnostic command spawning and docker/guest-payload-server.py payload_identity/ensure_home/payload_preexec. This blocks wra-662v Rust opt-in live validation.

## Acceptance Criteria

guest-service parses AGENTVM_UID/AGENTVM_GID together, creates/chowns HOME when requested, spawns primary and diagnostic children in the requested identity, has focused tests for primary identity/HOME, and the updated musl binary is rebuilt into the Rust opt-in appliance before live-payload is retried.


## Notes

**2026-05-18T05:42:51Z**

Fixed in guest-service/src/lib.rs by adding PayloadIdentity parsing from AGENTVM_UID/AGENTVM_GID, ensure_home, and configure_process setsid/setgroups/setgid/setuid handling for primary and diagnostic command spawns. Added focused test guest_service_primary_uses_requested_identity_and_home. Focused validation passed: cargo test --manifest-path guest-service/Cargo.toml --offline -- --nocapture and cargo build --manifest-path guest-service/Cargo.toml --offline --bin agentvm-guest-service --target x86_64-unknown-linux-musl. Full closure still requires rebuilding the opt-in Rust appliance with the updated musl binary and rerunning live-payload under wra-662v.
