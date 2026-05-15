---
id: wra-vni1
status: closed
deps: []
links: []
created: 2026-05-15T21:25:18Z
type: bug
priority: 0
assignee: Henrik Saksela
tags: [vm, network, tls, npm, codex]
---
# Fix HTTPS MITM stall on repeated npm registry request

Codex setup in /home/hsaksela/Code/planb stalls during npm install @openai/codex@latest. Live diagnostics from the new payload diagnostic channel show primary payload pid 948 running bootstrap and npm pid 952 stuck. npm log /home/hsaksela/.npm/_logs/2026-05-15T21_23_04_562Z-debug-0.log stops at full packument fetch: lines 12-17 show abbreviated @openai/codex metadata GET 200, then 'fetch manifest @openai/codex@0.130.0' and 'packumentCache full:https://registry.npmjs.org/@openai%2fcodex cache-miss'. vmnet-events.log shows the first HTTPS GET to registry.npmjs.org receives upstream payload, then a second http_request for the same path is logged with no following guest_payload/tls_upstream_payload/upstream_payload. /proc/net/tcp shows npm still has an ESTABLISHED connection to 104.16.4.34:443 but the log has not advanced. This strongly suggests the frontend HTTPS MITM/proxy path mishandles a repeated request on an existing TLS connection, likely buffering pending_upstream_plaintext without rearming/flushing upstream writable interest. Relevant code: vm-frontend/src/tcp_proxy.rs process_guest_payload, session_interest, flush_pending_https_plaintext, drain_upstream_tls_writes, vmnet runtime polling/reregistering.

## Design

Add a regression test for two sequential HTTPS requests on one guest TLS session/one upstream connection, matching npm's abbreviated then full packument fetch. Verify that after the second request is parsed, plaintext is encrypted and flushed upstream even if the upstream fd was not writable in the same readiness cycle. Review vmnet_runtime poll interest/reregister behavior after process_guest_payload adds pending_upstream_plaintext. The fix should be generic for persistent HTTP(S) connections, not npm-specific.

## Acceptance Criteria

A live Codex setup no longer stalls at npm packument fetch. Unit/integration coverage proves repeated HTTPS requests on one connection generate upstream writes and responses. vmnet-events.log for npm install shows the second registry request followed by guest_payload/tls_upstream_payload/upstream_payload or equivalent successful flow. ./vm-frontend/validate.sh required passes after rebuilding appliance if needed.


## Notes

**2026-05-15T21:27:07Z**

User correctly called out that this should have been caught by the intended extensive network/TLS test set. Coverage gap identified: existing tcp_proxy tests cover plain HTTP request/response, upstream readiness/backpressure, fragmented headers, and TLS primitives, but not two sequential HTTPS requests over one persistent guest TLS/upstream connection with event-loop readiness between them. vmnet runtime fixture logs a single injected HTTP request. Add the missing persistent HTTPS keep-alive/reused-connection regression before/with the fix.

**2026-05-15T21:31:01Z**

Implemented focused fix in vm-frontend/src/tcp_proxy.rs: when an HTTPS session has pending_upstream_plaintext and the upstream fd later becomes writable, process_gateway now calls flush_pending_https_plaintext before draining TLS writes/pending bytes. Added regression coverage with a fake TLS upstream server: https_reused_connection_flushes_pending_plaintext_when_upstream_becomes_writable establishes a guest HTTPS session, sends one request successfully, queues a second request while upstream is not writable, verifies session_interest asks for writable readiness, then verifies the second request is actually written when writable fires. Targeted validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline https_reused_connection_flushes_pending_plaintext_when_upstream_becomes_writable -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline tcp_proxy -- --nocapture; cargo build --manifest-path vm-frontend/Cargo.toml --offline.

**2026-05-15T21:35:19Z**

Full validation and live Codex install smoke passed. ./vm-frontend/validate.sh required passed end-to-end including live-smoke. Additional live install smoke with fixed target/debug/agentvm succeeded using a fresh temp project, public egress, MITM CA, and unique payload listener: npm install --global --no-progress @openai/codex@latest completed in 5s, command -v codex returned /home/hsaksela/.local/bin/codex, and codex --version printed codex-cli 0.130.0. This confirms the prior npm packument stall is fixed for the direct install path. The already-running /home/hsaksela/Code/planb VM was started before the frontend fix and must be restarted to use it.
