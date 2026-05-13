---
id: wra-pkhr
status: closed
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

**2026-05-13T21:11:46Z**

Started HTTPS MITM implementation. Completed first fail-closed/security slice: TCP policy now denies port 443 when HTTPS interception is enabled but CA cert, CA key, or per-host generation is missing; CLI exposes --tls-ca-cert, --tls-ca-key, and --tls-generate-per-host-certs for launch and standalone vmnet-gateway. Added rcgen/rustls/rustls-pemfile dependencies with rustls using the ring provider. Added vm-frontend/src/tls_mitm.rs with CA PEM loading, per-host leaf certificate generation signed by the configured CA, conversion to rustls CertifiedKey, and a rustls handshake test proving a client that trusts the CA can send decrypted HTTP bytes to the MITM server side. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 62 lib tests, 6 bin tests, 1 ignored.

**2026-05-13T21:24:37Z**

Implemented HTTPS MITM data path beyond the initial fail-closed slice. vm-frontend/src/tls_mitm.rs now loads the configured CA, generates per-host leaf certificates, terminates guest TLS, builds upstream TLS client configs from native host roots, and includes in-memory tests for both guest-side termination and upstream TLS encryption/decryption. vm-frontend/src/tcp_proxy.rs now creates guest and upstream TLS sessions for TCP/443, uses guest SNI for upstream validation/server name, buffers decrypted guest HTTP until the upstream TLS handshake is ready, logs HTTP summaries from decrypted plaintext, fails closed on missing SNI/config/TLS errors, and emits TLS handshake/upstream events. vm-frontend/network-policy.md documents CA CLI flags, fail-closed behavior, upstream native-root validation, SNI requirement, and redacted logging scope. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline passes with 64 lib tests, 6 bin tests, 1 ignored. Live QEMU validation remains covered by dependent ticket wra-pah4.
