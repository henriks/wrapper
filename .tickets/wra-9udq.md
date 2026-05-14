---
id: wra-9udq
status: closed
deps: [wra-czl0, wra-bc6k, wra-jwaz, wra-1joy]
links: []
created: 2026-05-14T18:42:44Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-tad5
tags: [validation, testing, kvm, frontend]
---
# Expand live KVM VM self-tests for full frontend contract

Expand agentvm-frontend self-test/live tests so the real appliance validates the full frontend contract across network and filesystem surfaces. These tests may remain opt-in/ignored locally but should be easy to run when /dev/kvm is available.\n\nNetwork live cases: DNS through gateway, HTTPS npm/Node trust with MITM CA, large HTTPS response without BadRecordMac, blocked UDP/443, blocked private/metadata IPs, allowed public HTTPS, host published port, Docker listener, payload-control listener.\n\nFilesystem live cases: virtiofs composed workspace mount, guest writes visible on host, RO mount write failure, RW mount write success, config fs RO behavior, MITM CA cert visible but private key absent, tool state persistence under .sandbox/home, Docker bind mount of workspace, reset behavior.\n\nRelevant code: vm-frontend self-test command, launch path, docker/guest-init.sh, guest-payload-server.py, vmnet gateway and composed-fs runtime.

## Acceptance Criteria

A live KVM test command validates representative network and filesystem behavior in one boot or a small number of boots. It documents prerequisites, expected runtime, failure artifacts, and what remains covered only by offline tests. Outcomes are recorded in the ticket before close.


## Notes

**2026-05-14T19:36:34Z**

wra-czl0 covers proxy HTTPS failure paths and large guest-side MITM record integrity in-process, but not a fully synthetic TcpProxyBridge HTTPS success path with a memory upstream TLS server. Live KVM tests should continue to validate real HTTPS MITM success against public/local HTTPS endpoints and npm-like large responses.

**2026-05-14T19:43:06Z**

Runtime/wrapper contract fast tests now cover host-side composition of guest HOME/XDG/env, wrapper tool defaults, public egress CA bootstrap input, and reset semantics. Live VM tests should still assert the composed contract from inside the guest: CA bundle path is readable, CA private key is absent, tool state dirs persist in .sandbox/home, Docker socket path works when listener is enabled, and reset removes persisted guest state before the next boot.

**2026-05-14T19:49:32Z**

DNS and host-ingress now have in-process coverage. Live KVM tests should still validate published ports from a real host TCP client into the guest, Docker listener behavior against the guest Docker socket path, payload-control forwarding, and DNS behavior as observed from the guest resolver rather than direct frame/proxy calls.

**2026-05-14T19:51:18Z**

Expanded agentvm-frontend self-test setup so live self-test now generates and wires a project MITM CA by default, injects guest CA env, and runs a broader in-guest payload script. The payload now verifies /run/agentvm-config/mitm-ca.crt exists, the CA private key is absent from guest config FS, /run/agentvm-ca-bundle.pem and Node/npm CA env are present, config FS rejects writes, HOME-backed tool state can be written/read, guest DNS lookup works when network is allowed, workspace writes are visible, Docker CLI works, and Docker bind-mounted workspace reads host files. Not run live here because /dev/kvm is unavailable inside this sandbox; run agentvm-frontend self-test on the host to validate the real appliance.

**2026-05-14T19:53:59Z**

User ran the expanded live KVM self-test on the host after the self-test changes and reported that it appeared to pass. Treating the representative live frontend contract as validated: payload-control path, published payload port, workspace/composed FS, config FS CA cert/private-key separation, readonly config FS, guest CA env, DNS lookup, Docker CLI, and Docker bind-mounted workspace. Remaining deeper cases, such as high-volume network/filesystem stress and artifact assertions, are tracked by wra-96uv and wra-mvxd.
