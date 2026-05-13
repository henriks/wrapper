---
id: wra-3zt0
status: closed
deps: [wra-z9sh]
links: []
created: 2026-05-13T21:34:02Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-octf
tags: [rust, network, host-ingress, validation]
---
# Validate frontend-owned host ingress listeners in booted Rust vmnet

Validate host-to-guest Docker, payload, and published-port access over the Rust frontend-owned host ingress path, not QEMU hostfwd. This relates to vm-frontend/src/host_ingress.rs, vm-frontend/src/vmnet_runtime.rs, vm-frontend/src/guest_tcp.rs, vm-frontend/src/main.rs, and the launch options --host-docker-listener, --host-payload-listener, and --publish. Required context: wra-z9sh implemented host ingress and wra-b6iu fixed guest close propagation. wra-pah4 validated guest egress but not host ingress against a booted guest service. The goal is to document observed outcomes and blockers.

## Acceptance Criteria

Boot with Rust stream networking and frontend host listeners. Demonstrate host access to the guest Docker/control path and at least one published TCP port or document why the guest service is unavailable. vmnet-events.log should include host_ingress_* events. Confirm no QEMU hostfwd/usernet is present. Document exact commands and results.


## Notes

**2026-05-13T21:45:15Z**

Validated frontend-owned host ingress on booted microvms. Payload control: launched with --no-net --host-payload-listener 12076:1076, connected to 127.0.0.1:12076 outside the sandbox, sent the guest payload server ping frame P/0, and received K/ok; vmnet-events.log recorded host_ingress_opened/host_payload/guest_payload/guest_closed for guest_port=1076 purpose=PayloadControl. Docker control: initial attempt used the wrong guest port 2375 and timed out; the guest socket bridge actually listens on 1075. Relaunching with --no-net --host-docker-listener 12375:1075 and curling --unix-socket .sandbox/docker-vm/run/docker.sock http://docker/_ping returned Docker HTTP/1.1 200 OK with body OK; vmnet-events.log recorded DockerApi host ingress on guest_port=1075. Published TCP: launched with --publish 12077:1076, connected to 127.0.0.1:12077, sent P/0, received K/ok, and vmnet-events.log recorded purpose=PublishedTcp. All validation launches used QEMU stream networking; no QEMU usernet/hostfwd path was used. Updated vm-frontend/vmnet-runtime-validation.md and vm-frontend/README.md with results and the correct Docker guest port.
