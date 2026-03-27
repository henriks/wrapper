---
id: wra-ee8x
status: open
deps: [wra-iyae, wra-nsc9]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, cloud-hypervisor]
---
# Implement project-local Cloud Hypervisor lifecycle management

Add host-side code that creates, boots, supervises, and tears down the project-specific VM for one sandbox run.

## Design

Scope:
- create runtime directories under .sandbox/docker-vm/
- create and maintain the sparse Docker data disk in that directory
- start virtiofsd
- create the VM through the Cloud Hypervisor API
- tie VM lifetime to the wrapper process so sandbox exit triggers VM shutdown
- stop and clean up transient runtime resources on sandbox exit or failed startup

The launcher should be idempotent within one sandbox launch and leave the project recoverable after failures.

## Acceptance Criteria

Starting the VM is idempotent within one sandbox launch.

Sandbox termination shuts down the VM and removes transient runtime resources.

Failed startup leaves the project recoverable without manual host cleanup.

