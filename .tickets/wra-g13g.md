---
id: wra-g13g
status: closed
deps: [wra-t2uv]
links: [wra-rh21, wra-vl1o]
created: 2026-05-16T16:13:56Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-bjaa
tags: [async, tokio, vmnet, tls]
---
# Keep rustls synchronous while making TLS MITM async-buffer aware

Problem:
TLS MITM has multiple buffering layers. Async sockets do not solve those by themselves; the TLS state machines can remain synchronous while queue limits and fatal-error semantics are clarified.

Grounding:
- vm-frontend/src/tcp_proxy.rs:347-397 and 403-429 process HTTPS guest plaintext and upstream plaintext.
- vm-frontend/src/tcp_proxy.rs:649-765 drains HTTPS plaintext and TLS writes into pending upstream buffers.
- vm-frontend/src/tls_mitm.rs:158-174 and 213-230 expose synchronous rustls byte-in/byte-out state.
- vm-frontend/src/tls_mitm.rs:370-429 converts plaintext to TLS bytes synchronously.
- vm-frontend/src/tls_mitm.rs:432-441 resolves/generates certificates under cache state.

Proposed implementation shape:
Do not drive rustls directly with Tokio in the first async migration. Keep GuestTlsSession and TlsUpstreamSession as synchronous state machines inside the owner/session actor. Add watermarks around guest TLS bytes, decrypted plaintext, upstream TLS bytes, and pending guest bytes. If certificate generation becomes expensive, move generation to spawn_blocking or a bounded certificate worker only after cache limits are defined.

Relationships:
- Follow or coordinate with wra-vl1o so fatal TLS failures close guest sessions.
- Follow or coordinate with wra-rh21 so certificate generation/cache are bounded before async load increases concurrency.

Risks:
TLS buffering can multiply memory use across layers. Moving certificate generation off-thread without cache/admission limits can hide CPU spikes rather than fixing them.

Validation:
- Large fragmented HTTPS response and blocked upstream write tests with explicit queue limits.
- Malformed/no-SNI async-session regression proving closure.
- Fuzz/stress target around TLS proxy buffering boundaries rather than full network sockets.


## Notes

**2026-05-16T18:24:49Z**

Started TLS MITM async-buffer-awareness slice after wra-kiv5 closure. Scope for first slice: keep rustls synchronous and owner-local, add explicit pending TLS/plaintext watermarks and guest-visible fail-closed events before considering any Tokio/spawn_blocking certificate work. Coordinating with linked wra-vl1o for fatal TLS closure semantics and wra-rh21 for certificate cache bounds.

**2026-05-16T18:26:53Z**

First implementation slice added explicit TLS MITM pending-plaintext watermarking while keeping rustls synchronous and owner-local. TcpProxyBridge now carries internal TcpProxyBufferLimits (default 1 MiB) into sessions; tests can lower it. When HTTPS plaintext waiting for upstream TLS/writable readiness would exceed the pending_upstream_plaintext limit, the owner closes the guest TCP session and emits TcpProxyEvent::BufferLimitExceeded with close/reset guest frames, and vmnet_runtime writes/logs those frames. Regression https_pending_plaintext_limit_fails_closed_when_upstream_not_writable proves bounded/fail-closed behavior. Focused validation passed: cargo fmt; cargo test --manifest-path vm-frontend/Cargo.toml https_pending_plaintext_limit --offline; cargo test --manifest-path vm-frontend/Cargo.toml tcp_proxy --offline; cargo test --manifest-path vm-frontend/Cargo.toml vmnet_runtime --offline.

**2026-05-16T18:32:03Z**

Added fatal TLS MITM closure semantics for active proxy sessions. TcpProxyEvent::TlsMitmFailed now carries owner-generated guest close/reset frames; vmnet_runtime writes/captures/logs those frames like other guest-visible proxy failures. Active guest TLS parse failures, missing/no-SNI once handshake fails, upstream TLS construction/drain failures, upstream TLS read failures, and TLS plaintext write failures now close the guest session through VmnetGateway on the owner. Existing no-SNI and invalid-upstream-TLS regressions now assert nonempty guest close/reset frames. Focused validation passed: cargo fmt; cargo test ... https_guest_without_sni --offline; cargo test ... https_invalid_upstream_tls --offline; cargo test ... tcp_proxy --offline; cargo test ... vmnet_runtime --offline.

**2026-05-16T18:32:46Z**

Broader vmnet-filtered validation also passed after TLS fatal closure changes: cargo test --manifest-path vm-frontend/Cargo.toml vmnet --offline (94 passed, 2 ignored stress tests as before; bin vmnet config tests passed).

**2026-05-16T18:36:57Z**

Extended TLS MITM watermarks to pending upstream TLS/ciphertext bytes. TcpProxyBufferLimits now tracks both pending_upstream_bytes and pending_upstream_plaintext; writes/buffers to upstream TLS/socket queues check bounds before enqueueing, fail closed via BufferLimitExceeded, and emit guest close/reset frames. Added https_pending_upstream_tls_limit_fails_closed_when_upstream_not_writable regression for bounded client-hello/upstream TLS buffering. Focused validation passed: cargo fmt; cargo test ... https_pending_upstream_tls_limit --offline; cargo test ... tcp_proxy --offline (16 tests); cargo test ... vmnet_runtime --offline (20 tests); cargo test ... vmnet --offline (94 passed, 2 ignored stress tests as before, bin vmnet config tests passed).

**2026-05-16T18:42:23Z**

Added pending guest-byte watermarking and TLS buffer limit proptest/stress coverage. TcpProxyBufferLimits now covers pending_guest_bytes as well as upstream TLS/plaintext buffers; send_guest_buffered checks the guest queue before appending and fails closed with BufferLimitExceeded if guest-side backpressure would grow unbounded. Added pending_guest_bytes_limit_fails_closed_before_buffering and proptest_pending_upstream_buffer_limits_stay_bounded. Focused validation passed: cargo fmt; cargo test ... proptest_pending_upstream_buffer_limits_stay_bounded --offline; cargo test ... pending_guest_bytes_limit --offline; cargo test ... tcp_proxy --offline (18 tests); cargo test ... vmnet_runtime --offline (20 tests); cargo test ... vmnet --offline (94 passed, 2 ignored stress tests as before, bin vmnet config tests passed). Required validation was attempted but failed in live-setup-tools Codex bootstrap due missing optional dependency @openai/codex-linux-x64 after npm:@openai/codex@0.130.0 install; documented as bug wra-l7sp.

**2026-05-16T19:03:59Z**

Required validation failure was diagnosed as caused by the pending_guest_bytes watermark applying to incoming chunk size before direct guest send. vmnet-events from the failed Codex setup showed proxy_buffer_limit_exceeded buffer=PendingGuestBytes for large TLS MITM downloads, causing npm to skip the optional @openai/codex-linux-x64 package and Codex to fail. Fixed send_guest_buffered so guest-side watermarking applies only to bytes that remain queued after flushing/attempting smoltcp send; direct sendable chunks larger than the configured queue limit are no longer rejected. Default pending_guest_bytes is now 8 MiB (upstream TLS/plaintext defaults remain 1 MiB). Added pending_guest_bytes_limit_does_not_reject_direct_guest_send. Validation passed: cargo fmt; cargo test ... pending_guest_bytes_limit --offline; cargo test ... tcp_proxy --offline (19 tests); cargo test ... vmnet_runtime --offline (20 tests); cargo test ... vmnet --offline (94 passed, 2 ignored stress tests as before, bin vmnet config tests passed); ./vm-frontend/validate.sh live-setup-tools; ./vm-frontend/validate.sh required.
