---
id: wra-g13g
status: open
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

