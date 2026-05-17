---
id: wra-cqjc
status: closed
deps: []
links: []
created: 2026-05-17T17:36:00Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [validation, flake, payload]
---
# Investigate intermittent required-validation hang in payload session cancel test

During wra-662v iteration 20, ./vm-frontend/validate.sh required timed out at the harness timeout after vm-frontend offline tests reported payload_client::tests::payload_session_runner_cancel_token_interrupts_blocked_receive running for over 60 seconds. The same focused test passed immediately when rerun with cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_session_runner_cancel_token_interrupts_blocked_receive -- --nocapture. This looks intermittent/load-sensitive and should be investigated separately so required validation stays reliable.

## Acceptance Criteria

The suspected hang is reproduced or ruled out; the cancel-token payload session test is hardened or the validation timeout/harness behavior is adjusted; required validation can run without intermittent hangs.


## Notes

**2026-05-17T17:36:12Z**

Focused rerun of payload_session_runner_cancel_token_interrupts_blocked_receive passed immediately after the required-validation timeout. Keep this ticket for investigating why the full required run reported it running for over 60 seconds under load.

**2026-05-17T17:48:41Z**

Hardened payload_session_runner_cancel_token_interrupts_blocked_receive: PayloadCancelToken::register_stream now immediately shuts down a newly registered stream if the token was already cancelled, closing a race where cancellation could happen before stream registration. The test server now has a 5s read timeout and asserts client close, and the cancel thread uses recv_timeout instead of waiting indefinitely. Focused validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_session_runner_cancel_token_interrupts_blocked_receive -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline payload_session_runner -- --nocapture. Broad validation passed: cargo test --workspace --offline; cargo fmt --all -- --check. Leave open until required validation completes without the previous hang.

**2026-05-17T17:51:55Z**

Continuation-2 iteration 2 reran ./vm-frontend/validate.sh required after the cancellation/test hardening. The previous payload_session_runner_cancel_token_interrupts_blocked_receive hang did not recur: vm-frontend offline tests completed and validation progressed into live-smoke. The run stopped for an unrelated stale appliance freshness check (docker/build-appliance.sh hash mismatch), so this bug's acceptance criterion is satisfied.
