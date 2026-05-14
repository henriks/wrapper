---
id: wra-1yv3
status: in_progress
deps: [wra-pkhr]
links: []
created: 2026-05-13T21:33:38Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, tls, validation]
---
# Validate HTTPS MITM with guest-installed CA in booted Rust vmnet

Boot a microvm through agentvm-frontend launch with stream vmnet, configure --tls-ca-cert/--tls-ca-key/--tls-generate-per-host-certs, and validate TCP/443 HTTPS interception with the MITM CA trusted inside the guest image. This relates to vm-frontend/src/tls_mitm.rs, vm-frontend/src/tcp_proxy.rs, vm-frontend/src/tcp_gateway.rs, vm-frontend/src/main.rs, and vm-frontend/network-policy.md. Required context: wra-pkhr implemented the TLS data path and unit tests; wra-pah4 validated HTTP egress, metadata denial, and --no-net but did not install a CA into the guest image. The goal is to document the exact guest-image CA installation step, launch command, vmnet-events.log output, guest-side HTTPS result, and whether logging remains summary-only.

## Acceptance Criteria

A booted guest HTTPS request succeeds only when policy allows it and the guest trusts the configured CA. vmnet-events.log records decrypted HTTP summary events without sensitive body/header logging. A missing/untrusted CA failure mode is documented. Any guest-image or lifecycle blockers have follow-up tickets.


## Notes

**2026-05-14T07:08:42Z**

Partial HTTPS MITM validation completed. Added config-fs delivery for the configured --tls-ca-cert as /run/agentvm-config/mitm-ca.crt, guest-init CA installation hook, and ca-certificates to the appliance package set. Generated a test CA under .sandbox/docker-vm/tls and verified agentvm-frontend prepare writes /mitm-ca.crt into config-fs-manifest.json. Booted the current rebuilt image with --allow-public-internet, --host-payload-listener, and TLS MITM flags. A guest Python TLS request to 104.20.23.154:443 with SNI example.com failed with CERTIFICATE_VERIFY_FAILED/UnknownCA, as expected because the current image predates the new guest-init CA install hook; vmnet-events.log showed InterceptHttps, tls_handshake_payload, tls_upstream_payload, and tls_mitm_failed UnknownCA. Re-running the guest script with ssl.create_default_context(cafile=/run/agentvm-config/mitm-ca.crt) proved the CA file is present and trusted by the script: guest TLS connected and vmnet logged decrypted http_request method=GET host=example.com path=/. While testing, found and fixed a tcp_proxy HTTPS buffering issue where guest HTTP plaintext could arrive before the upstream TLS final handshake write completed; the bridge now buffers plaintext until upstream_tls_ready and re-buffers if rustls emits zero encrypted bytes. Tests pass: cargo test --manifest-path vm-frontend/Cargo.toml --offline = 65 lib passed, 8 bin passed, 1 ignored. Ticket remains open until the appliance is rebuilt with docker/guest-init.sh and docker/build-appliance.sh changes, then validated using system trust without explicit cafile.

**2026-05-14T07:13:18Z**

Follow-up after appliance rebuild: boot reached guest-init but panicked before payload services because install_mitm_ca attempted to create /usr/local/share/ca-certificates on the read-only rootfs. Reworked docker/guest-init.sh to avoid update-ca-certificates and all /etc or /usr/local writes: it now builds /run/agentvm-ca-bundle.pem from the existing CA bundle plus /run/agentvm-config/mitm-ca.crt, exports SSL_CERT_FILE and REQUESTS_CA_BUNDLE, then starts guest services so payloads inherit trust. Verified sh -n docker/guest-init.sh and cargo test --manifest-path vm-frontend/Cargo.toml --offline. Ticket remains open until appliance is rebuilt again and HTTPS succeeds with guest default trust, no explicit cafile.

**2026-05-14T07:38:04Z**

After rebuilding the image again, booted HTTPS MITM validation now confirms the guest CA delivery path works: guest-init logs installed MITM CA bundle at /run/agentvm-ca-bundle.pem, payloads inherit SSL_CERT_FILE and REQUESTS_CA_BUNDLE, and a Python ssl.create_default_context() request to 104.20.23.154:443 with SNI example.com completes the guest TLS handshake without an explicit cafile. vmnet-events.log records tcp_connected action=InterceptHttps and decrypted http_request method=GET host=example.com path=/. While validating the response path, found a remaining HTTPS upstream forwarding bug: nonblocking upstream TLS writes can leave handshake/application data buffered, and the guest receives zero response bytes before timing out. Implemented buffered upstream socket writes, added https_plaintext_buffered summary logging, and added rustls drain attempts; cargo test --manifest-path vm-frontend/Cargo.toml --offline still passes. The ticket remains open because the full acceptance criterion requires guest HTTPS response success, not just guest trust and decrypted request logging.
