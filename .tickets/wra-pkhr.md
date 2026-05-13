---
id: wra-pkhr
status: open
deps: [wra-p7m4, wra-40vh]
links: []
created: 2026-05-13T10:22:53Z
type: feature
priority: 2
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, tls, mitm, policy]
---
# Implement HTTPS MITM with configured guest CA

Add HTTPS interception for guest TCP port 443. The gateway must accept guest TLS connections, generate per-host certificates signed by a configured CA key/cert, connect to the real upstream server, and proxy/decrypt/filter/log HTTP traffic. The guest is untrusted, so the boundary must be enforced externally; guest cooperation is limited to having the MITM CA installed in the appliance image.

## Design

Use rcgen plus a Rust TLS stack and evaluate hudsucker/http-mitm-proxy only if they fit the lower-level TCP termination model. Define CA storage/loading, certificate cache behavior, SNI/Host handling, upstream certificate validation, logging redaction, and failure behavior. UDP/443 must remain blocked to prevent QUIC bypass. This ticket depends on the userspace TCP gateway and config policy model.

## Acceptance Criteria

HTTPS requests from the guest succeed only when policy allows them and the configured CA is trusted by the guest. Requests are decrypted enough for HTTP policy/logging. UDP/443 is blocked. Certificate generation and CA loading are tested. Security-sensitive logging behavior is documented.


## Notes

**2026-05-13T10:40:54Z**

wra-40vh defines TlsMitmPolicy with explicit CA cert/key paths and per-host certificate generation toggle. HTTPS interception should fail closed when policy requires MITM but CA material is missing; no implicit host CA generation.
