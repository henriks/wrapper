---
id: wra-pah4
status: open
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

