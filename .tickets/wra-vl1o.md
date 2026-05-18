---
id: wra-vl1o
status: open
deps: [wra-i3t9]
links: [wra-rh21, wra-g13g, wra-g34z]
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


## Notes

**2026-05-16T18:32:03Z**

wra-g13g has implemented part of this linked behavior in the TLS MITM owner path: active TcpProxyEvent::TlsMitmFailed events now carry guest close/reset frames from VmnetGateway::close_tcp_session, and no-SNI / invalid-upstream-TLS regressions assert guest-visible closure. Remaining wra-vl1o scope should verify any other fatal TLS edge cases and runtime/session cleanup semantics.

**2026-05-16T21:46:56Z**

Review after closing wra-i3t9 and wra-g13g: the main active TLS MITM fatal paths now emit guest close/reset frames (no-SNI guest TLS and invalid upstream TLS have regressions), and upstream connect failure closure is no longer a blocker. Keep this ticket open for the remaining edge sweep and the requested live bad-upstream HTTPS regression before declaring the TLS failure surface fully closed.

**2026-05-18T10:38:19Z**

Follow-up cleanup epic wra-emj5 links wra-g34z. Fatal TLS MITM session closure should reuse the consolidated vmnet/proxy buffer and event path where possible; do not add a separate TLS-only session cleanup adapter if the shared TCP proxy path can own closure.
