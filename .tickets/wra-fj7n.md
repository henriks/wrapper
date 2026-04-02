---
id: wra-fj7n
status: closed
deps: []
links: []
created: 2026-04-01T21:17:14Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, sandbox, design]
---
# Define the VM-only runtime contract and CLI semantics

Write down the new execution contract before code changes land. The current contract in `docker/runtime-contract.md` and `requirements.md` still assumes Bubblewrap launches the agent on the host and the VM exists mainly to provide Docker. This ticket should replace that mental model with a VM-only one.

Required decisions:
- What exactly runs on the host versus inside the guest.
- Which current CLI flags survive, change semantics, or are deleted when Bubblewrap goes away.
- How the selected tool command is transmitted into the guest and how its exit code propagates back to the host wrapper.
- Which host directories are shared into the guest for project workspace, tool state, Docker config, GitHub auth, and optional AWS credentials.
- What stays persistent across runs versus what is transient under `.sandbox/docker-vm/run/`.
- What minimal guarantees the guest init path must provide before the payload can start.

Relevant code/docs:
- `sandbox-wrap`: `build_bwrap_args()`, `main()`, `DockerVmManager`.
- `docker/guest-init.sh`.
- `docker/runtime-contract.md` and `requirements.md`.

This ticket should intentionally delete old assumptions rather than document compatibility behavior.

## Design

Bias the contract toward one isolation boundary only: the VM. Avoid designs that require keeping host-side namespace/mount logic alive just to preserve old flags. The resulting contract should be specific enough that the guest payload/control implementation can proceed without reopening basic lifecycle questions.

## Acceptance Criteria

Updated contract notes define the VM-only execution model, the supported CLI semantics, the host/guest responsibility split, and the persistence model.

The contract explicitly states that Bubblewrap is no longer part of the agent execution path.

Downstream tickets can implement against the written contract without re-deciding core behavior.

## Notes

**2026-04-01T21:25:22Z**

Defined the VM-only runtime contract in docker/runtime-contract.md and updated the top-level architecture sections of requirements.md to match. Key decisions: the VM is now the only sandbox boundary; the agent payload runs inside the guest; Docker is always available inside that same guest; persistent guest HOME moves to .sandbox/home/; the host wrapper becomes a thin orchestrator only; supported flags are --project, --tool, --no-net, --docker-publish, --gh, --aws, --reset, and extra command args; --docker, --ro, --rw, and --pass-env are removed rather than preserved as compatibility baggage.

**2026-04-01T21:27:09Z**

Requirement change after contract closure: arbitrary host path mounts are required after all. The VM-only contract now keeps --ro PATH and --rw PATH, with the explicit constraint that they must be reimplemented as guest shares at the same absolute path, not by preserving the old Bubblewrap bind matrix. --pass-env remains removed.
