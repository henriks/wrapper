---
id: wra-nsc9
status: open
deps: [wra-iyae]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, image]
---
# Create build pipeline for the Docker appliance VM image

Add the build assets needed to produce the immutable guest appliance described in docker.md.

## Design

Scope:
- minimal Debian 12 guest build via debootstrap
- pinned Docker Engine packages
- tiny init/PID 1 boot flow
- guest-side bridge from vsock to /var/run/docker.sock
- read-only root image output plus kernel/initrd or kernel image needed for Cloud Hypervisor

The output must match the runtime contract and be consumable by the host-side VM manager.

## Acceptance Criteria

The repo contains a reproducible build path for the appliance artifacts.

The output format matches the runtime contract.

The appliance boots to a state where Docker can become reachable through the guest bridge.

