# Validation Workflow

This project has one required gate plus narrower tiers for iteration and triage.
Before considering code complete, run the required gate:

```sh
vm-frontend/validate.sh required
```

The required gate performs the validation documentation drift check, `cargo fmt
--check` for the Rust crates and fuzz package, composed-fs and vm-frontend
offline tests, offline guest service tests, fuzz target compilation with `cargo
check --manifest-path vm-frontend/fuzz/Cargo.toml --offline`, the quick
`live-smoke` host live validation scenario, and the `live-setup-tools` Codex
bootstrap/persistence scenario. `full` is an alias for the same gate.
If `/dev/kvm` is unavailable, the gate fails with a host-live limitation instead
of silently passing; rerun it on a KVM-capable host before closing live
validation work.

## Fast Offline

Use this during local iteration when the full gate would be too expensive:

```sh
vm-frontend/validate.sh fast
```

Equivalent commands:

```sh
cargo test --manifest-path composed-fs/Cargo.toml --offline
cargo test --manifest-path vm-frontend/Cargo.toml --offline
```

Expected runtime is a few seconds on a warm build. This tier must not require
network access, root, `/dev/kvm`, QEMU, Docker, or rebuilt appliance artifacts.
For vmnet runtime changes this tier covers the domain event/poller boundary,
readiness-buffered host ingress writes, readiness-buffered upstream proxy
writes, smoltcp/QEMU stream framing, policy behavior, pcap output, and event log
formatting without depending on host loopback networking.

## Formatting And Drift Checks

Run these directly when changing validation scripts or Rust formatting-sensitive
code:

```sh
vm-frontend/validate.sh docs
vm-frontend/validate.sh fmt
vm-frontend/validate.sh fuzz-check
vm-frontend/validate.sh guest-services
```

`docs` asserts that `AGENTS.md`, this workflow, and `vm-frontend/validate.sh`
stay aligned on the required gate. `fuzz-check` compiles/checks the fuzz targets
without running libFuzzer. `guest-services` runs offline `unittest` coverage for
`docker/guest-init.sh`, `docker/guest-payload-server.py`, and
`docker/guest-socket-bridge.py`.

## Stress And Property

Run before closing validation tickets or after touching protocol, path, or policy
logic:

```sh
vm-frontend/validate.sh stress
```

Equivalent commands:

```sh
cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_flat_file_operation_sequences -- --ignored --nocapture
cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_nested_operation_sequences_stress -- --ignored --nocapture
cargo test --manifest-path composed-fs/Cargo.toml --offline proptest_lock_operation_sequences_stress -- --ignored --nocapture
cargo test --manifest-path composed-fs/Cargo.toml --offline stress_seeded_flat_file_operation_sequences -- --ignored --nocapture
cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_stream::tests::stress_many_chunked_frame_splits -- --ignored --nocapture
cargo test --manifest-path vm-frontend/Cargo.toml --offline vmnet_gateway::tests::stress_seeded_generated_guest_frames -- --ignored --nocapture
cargo test --manifest-path vm-frontend/Cargo.toml --offline dns_proxy_stress -- --ignored --nocapture
```

The composed-fs operation and lock property tests use `proptest` with shrinking.
The fixed-seed filesystem regression uses seed `0x5eedf17e20260514`, the vmnet
gateway generated-frame stress uses seed `0x676174657761795f`, and the DNS
stress regression uses seed `0xd15c20260514`. Failures print either the
minimized generated input, seed, or operation trace; paste that reproducer into
the fixing ticket before closing it.

## Coverage-Guided Fuzzing

The cargo-fuzz targets live under `vm-frontend/fuzz`. They are not part of the
fast or stress tiers because they require the `cargo-fuzz` runner and are meant
for manual or scheduled bug-finding runs.

Install the runner on a machine that can fetch tools:

```sh
cargo install cargo-fuzz
```

Short smoke run:

```sh
cd vm-frontend
cargo fuzz run vmnet_stream_frame_io -- -max_total_time=30
```

Current targets:

```sh
cargo fuzz run vmnet_stream_frame_io
cargo fuzz run dns_proxy_payload
cargo fuzz run vmnet_gateway_frame
cargo fuzz run composed_manifest_shape
```

The fuzz package can be checked without running libFuzzer:

```sh
cargo check --manifest-path vm-frontend/fuzz/Cargo.toml
```

Checked-in seed inputs live in `vm-frontend/fuzz/corpus/<target>/`. Generated
crashes and temporary outputs belong under `vm-frontend/fuzz/artifacts/`, which
is ignored. When filing a ticket from a fuzz failure, include the target name,
the minimized crashing input path, the exact `cargo fuzz run ...` command, and
whether the failure is a panic, timeout, memory growth issue, or invariant
assertion.

## Live KVM

Run after rebuilding appliance artifacts and before closing live frontend
contract work. The live matrix has named scenarios:

- `live-smoke` (also `host-live`/`live`): quick required self-test with the published payload listener.
- `live-setup-tools`: required Codex setup-tool bootstrap over npm/TLS MITM, then no-net relaunch from persisted guest state and optional-package metadata verification.
- `live-hostile`: slower hostile/no-net self-test that probes denied metadata/loopback/DNS behavior.
- `live-payload`: payload protocol stress self-test with a large request environment and large guest output.
- `live-dns`: allowed resolver-path and denied/no-net resolver-path self-tests with explicit query diagnostics.
- `live-docker`: Docker bridge container egress, no-net denial, and host-to-container published-port self-tests with image/policy/phase diagnostics.
- `live-fs`: composed-fs live/adversarial self-test, run twice with the same run-dir to cover stale socket/state cleanup.
- `live-full`: runs all named live scenarios.

Prefer `host-live` in docs when emphasizing host prerequisites:

```sh
vm-frontend/validate.sh host-live
vm-frontend/validate.sh live-setup-tools
vm-frontend/validate.sh live-hostile
vm-frontend/validate.sh live-payload
vm-frontend/validate.sh live-dns
vm-frontend/validate.sh live-docker
vm-frontend/validate.sh live-fs
vm-frontend/validate.sh live-full
```

Equivalent command:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline -- \
  self-test \
  --project "$PWD" \
  --run-dir "$PWD/.sandbox/docker-vm/self-test" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --image alpine:3.22 \
  --publish-payload-port 12079
```

Prerequisites:

- `/dev/kvm` visible to the process, or set `KVM_DEVICE=/path/to/kvm` for an equivalent exposed device.
- `qemu-system-x86_64` installed.
- `docker/out/artifact-manifest.json`, `docker/out/vmlinuz`,
  `docker/out/initrd.img`, and `docker/out/rootfs.raw` built from the current
  guest scripts.
- Host network access if the selected Docker image is not already present in
  the project-local Docker data disk.

The live tier validates the real QEMU stream network, payload-control listener,
published payload port, composed virtiofs workspace, guest config filesystem,
MITM CA bundle exposure without private key exposure, guest DNS lookup,
SQLite/WAL activity under guest `$HOME`, concurrent host+guest SQLite WAL
writes against the same workspace database, Docker CLI, Docker bind-mounted
workspace, setup-tool npm bootstrap, and persisted agent CLI state across
payload-triggered VM shutdown and relaunch.

For the SQLite concurrency check, the self-test creates
`.agentvm-self-test-sqlite/state.sqlite`, starts a host `python3 sqlite3`
worker, and runs a guest `python3 sqlite3` worker against the same database.
The run is valid only if both sides write 200 rows and a final
`PRAGMA integrity_check` returns `ok`.

## Interactive TUI Smoke

Automated pty-backed terminal tests cover the non-live wrapper setup path, config
editor keyboard flow, configured-project no-reprompt behavior, and launch failure
artifact diagnostics. Run them directly while iterating on TUI behavior:

```sh
cargo test --manifest-path vm-frontend/Cargo.toml --offline --test tui_terminal -- --nocapture
```

Run the manual smoke below after touching wrapper TUI behavior, terminal
sizing/input, prompt focus, startup setup, or wrapper entrypoint semantics. It
requires a real terminal and user interaction in addition to the live KVM
prerequisites above.

Start the default interactive wrapper path:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm -- \
  --project "$PWD" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64
```

Expected behavior:

- On an unconfigured project, a startup dialog appears because no setup recipe
  or `.sandbox/config.json` exists.
- Accepting the default initializes Codex and the generated
  `.sandbox/docker-vm/run/composed-fs-manifest.json` contains a writable
  `$HOME/.codex` tool-state mount backed by the same host path.
- The accepted setup is persisted to `.sandbox/config.json`; rerunning the same
  `agentvm` command starts the configured default command without showing the
  startup dialog.
- The guest payload renders inside the terminal viewport, with a
  one-line wrapper status/prompt area below it.
- Typed input in guest focus reaches the guest payload.
- Pressing `Ctrl-\` then `p` opens the wrapper prompt; typed prompt input does
  not appear in the guest. `Enter` accepts the prompt and `Esc` cancels it.
- Resizing the host terminal redraws the viewport and sends the guest PTY the
  viewport size.
- `Ctrl-C` in guest focus is delivered to the guest payload as SIGINT.
- Normal exit and interrupted shutdown restore raw mode and the alternate
  screen.

Verify the plain fallback path:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm -- \
  --no-tui \
  --project "$PWD" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  --setup-tool codex \
  -- --help
```

Expected behavior:

- No startup dialog or alternate-screen TUI appears.
- Payload output streams directly to stdout.
- The process exits with the guest payload exit status.

Verify the command override path:

```sh
cargo run --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm -- \
  --project "$PWD" \
  --artifact-manifest "$PWD/docker/out/artifact-manifest.json" \
  --qemu /usr/bin/qemu-system-x86_64 \
  -- bash -l
```

Expected behavior:

- The configured project is not reprompted.
- The guest starts an interactive `bash -l` payload while preserving configured
  Codex state mounts.

On failure, collect `.sandbox/docker-vm/run/state.json`,
`.sandbox/docker-vm/run/qemu.log`, `.sandbox/docker-vm/run/vmnet-events.log`,
and the generated composed/config manifests. Also note the host terminal size,
terminal emulator, `$TERM`, and whether the failure happened in guest focus,
wrapper prompt focus, resize handling, or cleanup.

## Artifacts And Triage

On live failures, collect `.sandbox/docker-vm/self-test/state.json` first. It
points to:

- `qemu.log`
- `console.log`
- `vmnet-events.log`
- `composed-fs-manifest.json`
- `config-fs-manifest.json`
- `guest-config/composed-binds.json`

`vmnet-events.log` is the primary network artifact. It records DNS decisions,
UDP denials, unsupported protocol classifications, TCP policy/setup failures,
HTTP/TLS proxy events, TLS MITM failures, upstream failures, and host-ingress
events. It must not contain CA private key material.

If the source guest scripts changed but `docker/out/rootfs.raw` has not been
rebuilt, live payload behavior may not match the working tree. For lock
validation during development, a temporary rootfs copy can be patched with
`debugfs` and referenced by a temporary artifact manifest; do not treat that as
a replacement for rebuilding `docker/out` before release.

For filesystem failures, preserve the generated manifests and any operation
trace from the failing test. The trace is part of the repro and should be added
to the ticket note.

## CI Policy

Recommended split:

- Per-commit CI: `vm-frontend/validate.sh fast`.
- Pre-merge/manual local CI without KVM: `vm-frontend/validate.sh all-local`.
- Required pre-close gate on a KVM-capable host: `vm-frontend/validate.sh required`.
- Scheduled fuzz smoke on a machine with `cargo-fuzz`: short runs of each
  `vm-frontend/fuzz` target.
- Live-only reruns during triage: `vm-frontend/validate.sh host-live` on a
  runner with KVM and rebuilt appliance artifacts.

When adding validation, update `validation-matrix.md`, add the exact command to
this file if it creates a new tier or named filter, and record the outcome in
the relevant `tk` ticket before closing it. The required gate's `docs` check
will fail if AGENTS.md stops naming `./vm-frontend/validate.sh required` or this
workflow stops naming the required/host-live commands.
