# Microvm Plus Composed Filesystem Validation

Ticket: `wra-11dm`

Date: 2026-05-13

## Result

`microvm + composed fs` is the Docker VM default:

```sh
sandbox-wrap --docker ...
```

The old q35 per-share path has been removed rather than preserved as redundant
legacy code.

## Checks Run

Preflight:

- `python3 docker/check-qemu-command-shape.py`

Integration:

- `--docker --no-net` boots,
  reaches payload readiness, exposes Docker through the socket proxy, preserves
  the project working directory, and writes project files back to the host.
- The guest mounts the config share and one composed export, then reconstructs
  the project path, tool state, Docker state, and `/workspace` as `virtiofs`
  bind mounts from `/run/agentvm-host`.
- `--ro` entries reject writes with `Read-only file system`.
- `--rw` entries accept guest writes and persist them on the host.
- Docker bind mounts from `$PWD` work inside the guest.
- `--docker-publish 28082:18082` forwards host localhost traffic to a server
  inside the guest and returned `microvm-publish-ok`.
- Clean shutdown removes `.sandbox/docker-vm/run`, including transient state
  and logs.

The same user-facing filesystem behavior validated on q35 composed mode also
works on microvm composed mode for these smoke cases.

## Remaining Rollout Work

- Measure launch, Docker readiness, and payload readiness times against q35.
- Keep broader Codex/Copilot auth-state behavioral coverage with the VM-only
  state-sharing work.
