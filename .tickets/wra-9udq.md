---
id: wra-9udq
status: open
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
