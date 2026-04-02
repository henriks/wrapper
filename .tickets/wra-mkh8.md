---
id: wra-mkh8
status: in_progress
deps: [wra-fj7n]
links: []
created: 2026-04-01T21:17:14Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, guest, exec]
---
# Implement guest payload execution and host control path

Add the mechanism that actually runs the requested tool command inside the guest and returns its lifecycle to the host wrapper. Today the guest boots `dockerd` and the socket bridge, but the agent payload still runs outside the VM under Bubblewrap. This ticket moves command execution into the guest.

Scope:
- Define how the host passes the selected command and environment into the guest.
- Extend the guest boot path so it can either start services and then `exec` the payload, or delegate payload launching to a dedicated in-guest runner.
- Preserve correct stdio behavior for interactive tool use.
- Propagate exit status and termination signals back to the host wrapper.
- Keep logs and diagnostics inspectable under `.sandbox/docker-vm/run/`.

Relevant code:
- `sandbox-wrap`: `main()`, `DockerVmManager.start()/shutdown()`, current proxy/helper process model.
- `docker/guest-init.sh` and any new guest-side helper used to launch the payload.

This ticket is about the core host/guest execution boundary, not yet about all auth/state mounts or documentation cleanup.

## Design

Prefer the smallest control path that works for interactive agents. Avoid building a general RPC framework if a narrow payload-launch channel is enough. Design for terminal correctness from the start; the recent console/input bugs are a warning against clever but implicit TTY plumbing.

## Acceptance Criteria

A requested command can be launched inside the guest from the host wrapper.

Interactive stdio works well enough for normal Codex/Copilot use.

The host wrapper receives the guest payload exit code and can tear the VM down deterministically afterward.

## Notes

**2026-04-01T21:25:26Z**

Contract decisions from wra-fj7n: the wrapper must launch the requested payload inside the guest and return its exit status to the host; Bubblewrap is no longer part of the execution path; the host wrapper is only an orchestrator. The payload control path should stay narrow and explicit rather than turning into a general RPC layer, but it must support interactive stdio and signal propagation.

**2026-04-02T05:31:58Z**

Implemented a first VM payload-control path in sandbox-wrap plus new guest appliance helpers. Added guest-payload-server.py, a framed TCP protocol for ping/request/input/output/resize/signal/exit, host-side run_guest_payload() bridging stdio and exit status, and a new appliance payload port in appliance.env + artifact-manifest.json. guest-init.sh now starts the payload server and mirrors guest-payload-server.log into .sandbox/docker-vm/run/. For now the new path is wired through the existing --docker flow so commands run inside the guest there while non---docker launches still use the old host Bubblewrap path until the cleanup ticket flips the default. Verification completed here: python compilation plus shell syntax checks. Verification gap: no live KVM/QEMU run was possible in this environment because /dev/kvm is unavailable.

**2026-04-02T07:06:26Z**

Extended the in-progress VM guest path with a first explicit guest-share mechanism so the payload path can actually consume host-backed state. The launcher now prepares a fixed guest-config virtio-fs share plus supplemental virtio-fs shares for tool state, Docker config, optional gh config, and arbitrary --ro/--rw paths. guest-init mounts the config share, reads shares.txt, and mounts each extra share before starting services. Important limitation: this is only syntax-checked here; it still needs a rebuilt appliance and live KVM validation.

**2026-04-02T10:24:00Z**

Adjusted the appliance build so `mise` is treated like the other baked guest dependencies instead of an implicit runtime assumption: added an explicit Alpine package pin in `docker/appliance.env`, taught `docker/refresh-pins.sh` to refresh it, and made `docker/build-appliance.sh` install the pinned package into the image. Also tightened `build_payload_script()` so explicit guest commands no longer run `mise install` at payload startup; runtime `mise install` remains only on the default tool-launch path until the tool bootstrap story is moved fully into the appliance.

**2026-04-02T11:02:00Z**

Switched away from Alpine's stale `mise` package entirely. The appliance build now downloads a pinned upstream `mise` release binary directly into `/usr/local/bin/mise` during image creation, with the version/target pinned in `docker/appliance.env`. `docker/refresh-pins.sh` no longer manages `mise`, since it is no longer sourced from Alpine repositories. This keeps `mise` baked into the image while decoupling it from Alpine package lag.

**2026-04-02T11:18:00Z**

Follow-up to the previous note: `docker/refresh-pins.sh` now manages `mise` again, but from GitHub release metadata instead of Alpine APK indexes. The script now queries the latest `jdx/mise` release, extracts the pinned version for the configured target plus the asset SHA-256 digest, and rewrites `MISE_VERSION` and `MISE_SHA256` in `docker/appliance.env`. `docker/build-appliance.sh` verifies that checksum before installing the binary into the guest image.

**2026-04-02T11:34:00Z**

Live guest launch exposed another bootstrap issue: the VM path was still auto-creating a default project `mise.toml` with `node = "24"`, which forced `mise install` to try building Node inside the guest before Codex could start. The VM path now skips generating that default project config entirely, and the appliance build gained pinned Alpine `nodejs` and `npm` packages so mise's npm backend has a system Node/npm available when installing the tool package. This keeps the VM launcher path narrower and avoids an unnecessary source-build dependency in the guest.

**2026-04-02T11:45:00Z**

Tightened the remaining tool bootstrap path so VM launches no longer run `mise install` unconditionally. The default tool path now checks whether the tool command already exists in the persistent guest HOME state and only runs `mise install --yes` if it is missing. That matches the desired lifecycle better: tool install happens once for a fresh `.sandbox/`, later VM starts just execute the existing tool, and `--reset` is what forces reinstallation.

**2026-04-02T11:52:00Z**

One more bootstrap correction: even the conditional install path was still using bare `mise install`, which means a project-local `mise.toml` could drag in unrelated project runtimes during first-run tool installation. The VM path now installs only the requested agent package (for example `npm:@openai/codex@latest`) when the command is missing, so project-level mise configuration no longer expands the bootstrap surface.

**2026-04-02T12:01:00Z**

Reworked the VM bootstrap again to align with the intended boundary: the guest no longer uses `mise` to install the agent CLI at all. Instead, the default guest payload path installs the missing agent package with system `npm` into the persistent guest HOME prefix (`$HOME/.local`) and only then proceeds to run the tool. The VM HOME-local mise config also stops declaring the AI tool package, so project `mise.toml` remains for project tooling rather than agent bootstrap.

**2026-04-02T12:24:00Z**

Codex itself still probes for system `bubblewrap` inside the guest and warns when only its vendored copy is available. To keep the VM environment unsurprising, the appliance build now includes Alpine `bubblewrap` as a pinned package, and `docker/refresh-pins.sh` refreshes that pin alongside the other guest platform packages.

**2026-04-02T12:31:00Z**

Live testing showed the guest had `dockerd` but not the `docker` CLI on PATH. The appliance build now installs Alpine `docker-cli` separately, and `docker/refresh-pins.sh` manages a distinct `DOCKER_CLI_VERSION` pin so the daemon and client availability are both explicit.

**2026-04-02T12:13:28Z**

Concurrency follow-up tracked separately in wra-1ro6. Important current finding: sandbox-wrap already takes a per-project flock in DockerVmManager.acquire_lock() before removing .sandbox/docker-vm/run, so the earlier interference is unlikely to be a simple same-project run-dir deletion race. Investigation should focus on broader shared-workspace/shared-launcher effects or host-side resource interactions.
