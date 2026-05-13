# Docker VM Startup Measurement and Rollout Decision

Ticket: `wra-9xru`

Date: 2026-05-13

## Method

Each mode was run once with a sleeping payload so `.sandbox/docker-vm/run/state.json`
could be read while the VM was still running:

```sh
sandbox-wrap --project /home/hsaksela/ai/wrapper --tool codex --docker ... \
  -- sh -lc 'sleep 20'
```

The wrapper records timings from `DockerVmManager.start()` entry:

- `qemu_started`: QEMU process launched
- `docker_ready`: Docker `_ping` succeeds through the project-local socket
- `payload_ready`: guest payload control path responds

This is a lightweight local measurement, not a statistically rigorous
benchmark. It is sufficient for the default-rollout decision because the
differences are small and the device/process count reduction is structural.

## Results

| Mode | QEMU start | Docker ready | Payload ready | Host processes | QEMU devices |
| --- | ---: | ---: | ---: | ---: | ---: |
| q35 fallback per-share | 418.1 ms | 2722.9 ms | 2725.2 ms | 6 | 8 |
| q35 composed | 229.0 ms | 2534.1 ms | 2535.8 ms | 4 | 6 |
| microvm composed | 228.8 ms | 2861.8 ms | 2863.7 ms | 4 | 6 |

## Decision

Proceed with microvm composed as the Docker VM default and remove the old q35
per-share fallback instead of preserving redundant legacy code.

Rationale:

- q35 composed improves startup in this sample and removes two host processes
  and two QEMU devices compared with the old per-share model.
- microvm composed keeps the same reduced host process and device count as q35
  composed.
- microvm composed payload readiness was about 329 ms slower than q35 fallback
  and about 328 ms slower than q35 composed in this sample. That is not enough
  to justify keeping the less suitable q35 device model as the migration target.
- q35 composed and microvm composed both passed functional validation before
  this measurement.

## Default Switch Criteria

The default may remain on microvm composed when validation preserves:

- payload readiness within 20% or 1 second of the measured q35 fallback
  baseline on local smoke runs
- Docker readiness through the project-local socket
- project path identity and host writeback
- Docker bind mounts from `$PWD`
- `--ro`, `--rw`, `--no-net`, and `--docker-publish`
- no increase above 4 wrapper-owned host processes for composed mode
- no increase above 6 QEMU devices for composed mode with the tiny config share

The old per-share export path has been removed after composed q35, microvm
composed, and startup/readiness validation passed.
