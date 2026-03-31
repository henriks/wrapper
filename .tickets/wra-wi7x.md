---
id: wra-wi7x
status: closed
deps: [wra-ee8x, wra-bmhk]
links: []
created: 2026-03-27T21:22:04Z
type: feature
priority: 0
assignee: Henrik Saksela
parent: wra-hggq
tags: [docker, vm, sandbox-wrap]
---
# Replace direct Docker socket mounting with VM-backed --docker behavior

Update sandbox-wrap so --docker starts the project VM before entering bwrap, mounts only the project-local Docker socket into the sandbox, and ensures teardown when the sandbox exits.

## Design

Scope:
- remove direct host socket discovery/mount behavior from the --docker path
- start and verify the VM before entering bwrap
- mount the project-local Unix socket into the sandbox at /run/docker.sock and /var/run/docker.sock
- preserve ~/.docker config mounting if still needed for client auth
- ensure wrapper-driven teardown on normal exit and signal exit paths
- enforce Linux/KVM-only support with clear preflight errors

## Acceptance Criteria

--docker works without exposing the host daemon directly.

The sandbox sees only the project-local socket.

Exiting the sandbox also terminates the VM.

Unsupported hosts fail early with a clear message.


## Notes

**2026-03-27T21:26:46Z**

Runtime contract decision from wra-iyae: --docker cannot use os.execvp into bwrap. The wrapper must stay as the parent process, launch bwrap as a child after VM readiness, and guarantee VM teardown on sandbox exit and signal paths. It must mount only .sandbox/docker-vm/run/docker.sock into the sandbox.

**2026-03-27T21:59:07Z**

Launcher primitives from wra-ee8x are now available in sandbox-wrap. The integration ticket can build on DockerVmManager.start()/shutdown() plus the lock-aware --reset behavior. Remaining integration work is to add the host proxy, mount .sandbox/docker-vm/run/docker.sock into bwrap, and keep the wrapper alive as the supervisor instead of execvp'ing directly into bwrap when --docker is enabled.

**2026-03-27T22:01:35Z**

Proxy primitives from wra-bmhk are now available in sandbox-wrap. The integration ticket can mount .sandbox/docker-vm/run/docker.sock into bwrap and rely on DockerVmManager.start() to bring up virtiofsd, Cloud Hypervisor, the host proxy, and the Docker readiness probe before launching the sandbox child.

**2026-03-27T22:03:23Z**

Integrated VM-backed Docker into sandbox-wrap. Removed the direct host Docker socket discovery path from --docker, changed bwrap construction to require and mount only the project-local socket at /run/docker.sock and /var/run/docker.sock, set DOCKER_HOST=unix:///run/docker.sock inside the sandbox, and changed the main execution path so --docker starts DockerVmManager before launching bwrap as a child and always shuts the VM down afterward. Also fixed repo-root resolution for symlink invocations by using realpath(__file__). Verification completed: python compilation, CLI help, build_bwrap_args socket/env assertions, and a preflight run showing clear early failure when cloud-hypervisor/virtiofsd are missing rather than falling back to the host daemon.

**2026-03-31T21:35:00Z**

Live use exposed a bind-mount ergonomics gap: the guest originally mounted the workspace only at `/workspace`, while the sandboxed Docker client still saw the project at its host-style absolute path (for example `/home/hsaksela/...`). That made `docker run -v "$PWD":...` and Compose bind mounts awkward or incorrect. Updated the launcher to append `agentvm_project=<project_path>` to the guest kernel cmdline so the appliance can mount the shared repo at the original absolute project path inside the guest and keep `/workspace` as an alias.
