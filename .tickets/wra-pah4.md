---
id: wra-pah4
status: closed
deps: [wra-9ida, wra-pkhr]
links: []
created: 2026-05-13T10:23:29Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, validation, docs]
---
# Validate Rust frontend vmnet sandbox end to end

Run and document end-to-end validation of the Rust frontend with embedded/reused composed fs and userspace vmnet gateway. This is the gate before replacing the Python frontend path.

## Design

Reuse the existing VM smoke expectations and add network-specific checks: DHCP lease, ARP gateway, DNS logging/policy, blocked UDP/443, blocked metadata/private ranges, HTTP interception, HTTPS MITM with configured CA, pcap capture, Docker/payload readiness, --no-net behavior, and published localhost port behavior or replacement. Document exact commands, logs, captures, and failures.

## Acceptance Criteria

Validation results are documented in a design note or ticket notes. Any blockers have follow-up tickets. The result explicitly states whether the Rust frontend is ready to replace the Python frontend for the tested path.


## Notes

**2026-05-13T21:33:29Z**

Validated the Rust stream vmnet path on a booted microvm outside the sandbox because /dev/kvm is hidden in the sandbox. Passing path: launch with --allow-public-internet, --guest-http-smoke-url http://198.51.100.10/, and --local-http-smoke-upstream 198.51.100.10:80. Guest configured eth0 as 10.0.2.15/24 via 10.0.2.2, wget received HTTP/1.1 200 OK, and vmnet-events.log recorded tcp_connected/http_request/guest_payload/upstream_payload. During validation, a denied metadata-IP smoke initially hung and logged nothing; fixed by emitting guest-visible TCP RSTs for pre-smoltcp denied SYNs and adding tcp_denied_preaccept runtime logging. After the fix, http://169.254.169.254/ fails with Connection refused and logs destination denied range. Also added explicit --no-net CLI support; a booted --no-net smoke against the local upstream mapping fails closed, state.json records egress_reason=NoNetFlag, and vmnet-events.log records tcp_denied_preaccept for destination denied by egress policy. Updated vm-frontend/vmnet-runtime-validation.md and README with outcomes and remaining gaps. Validation commands: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 64 lib tests, 8 bin tests, 1 ignored; cargo test --manifest-path composed-fs/Cargo.toml --offline passed with 17 tests; python3 docker/check-qemu-command-shape.py passed.
