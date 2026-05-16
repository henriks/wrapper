---
id: wra-vl1o
status: open
deps: [wra-i3t9]
links: [wra-rh21, wra-g13g]
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, tls, stability]
---
# Close guest TCP sessions on fatal TLS MITM failures

Problem:
TLS MITM failures are often logged as events but can leave the guest TCP session established, causing apparent hangs instead of prompt failure.

Relevant code:
- vm-frontend/src/tcp_proxy.rs:246-252 handles upstream TLS read failures.
- vm-frontend/src/tcp_proxy.rs:355-360 handles guest TLS parse failures.
- vm-frontend/src/tcp_proxy.rs:590-599 handles missing SNI after handshake.

Impact:
No-SNI, malformed guest TLS, or invalid upstream TLS can leave the guest waiting indefinitely.

Recommended fix:
Treat MITM setup and record-processing failures as fatal for the connection. Send a TLS alert when practical, then close/reset the guest TCP session and remove proxy state.

Validation:
- Update no-SNI and invalid-upstream TLS tests to assert session closure and guest frames.
- Add a live HTTPS regression for a bad upstream endpoint proving the guest command exits promptly.

