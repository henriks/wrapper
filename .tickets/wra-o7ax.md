---
id: wra-o7ax
status: closed
deps: [wra-k8br, wra-rysl, wra-735i]
links: []
created: 2026-05-15T06:51:31Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-2cfd
tags: [fuzzing, filesystem, network]
---
# Add coverage-guided fuzz targets for composed-fs and vmnet surfaces

Build on the fuzz harness infrastructure with concrete high-value targets. Files and surfaces to consider: vm-frontend/src/vmnet_stream.rs QemuFrameIo frame parsing; vm-frontend/src/dns_proxy.rs DNS query parsing and response handling; vm-frontend/src/vmnet_gateway.rs handle_guest_frame; vm-frontend/src/tcp_gateway.rs HTTP request parser and destination policy; composed-fs manifest/runtime path validation; composed-fs operation sequence interpreter if practical. Seed corpora should include valid minimal Ethernet/IP/TCP/UDP/DNS frames, malformed length/checksum examples, QEMU frame stream boundaries, representative manifests, and path components that exercise mount/symlink rejection logic.

## Acceptance Criteria

At least several coverage-guided fuzz targets exist for both network and filesystem surfaces. Each target has seed corpus entries, a short README or validation-workflow section with run/minimize/reproduce commands, and crash artifacts are excluded or stored according to repo policy. Targets avoid external network, root, KVM, QEMU, and Docker requirements.


## Notes

**2026-05-15T07:08:40Z**

Added concrete cargo-fuzz targets for vmnet stream framing, DNS payload parsing/classification, full vmnet gateway frame ingestion, and composed-fs manifest shape validation. Added seed corpus files and a safe public validate_manifest_json_shape API that validates manifest schema/path shape without opening fuzz-supplied host paths. Verified cargo check --manifest-path vm-frontend/fuzz/Cargo.toml, composed-fs manifest_shape_validation_does_not_open_host_sources, and vm-frontend DNS malformed payload test.
