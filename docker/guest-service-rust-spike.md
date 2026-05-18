# Rust/Tokio guest service spike

This note records the `wra-lcbk` spike outcome after the Python guest service
bounds landed in `wra-g0uv`, `wra-dky9`, and `wra-sum7`, and the later parity
validation that made the Rust service the only payload service.

## Current decision

Use the Rust/Tokio guest service in the appliance for both payload execution and
the Docker TCP-to-Unix bridge.

## Semantics the Rust guest service preserves

### Payload control TCP service

The Rust service preserves the existing frame protocol used by
`vm-frontend/src/payload_client.rs`:

- Frame header is `!cI`/network byte order: one frame type byte plus a 32-bit
  payload length.
- Maximum frame payload is 16 MiB.
- Initial frames:
  - `P` returns `K` with `ok`.
  - `R` starts the single primary PTY payload.
  - `D` starts a bounded diagnostic payload that may run concurrently with the
    primary payload.
- Output frames are `O`; exit frames are `X`; failure frames are `F`.
- Input/control frames for primary payloads are:
  - `I` for stdin bytes.
  - `W` for terminal resize JSON.
  - `S` for signal forwarding JSON.

Primary payload behavior to preserve:

- Allocate a PTY and set the requested rows/cols before spawning the child.
- Start a new process group/session and attach the controlling terminal.
- Apply `AGENTVM_UID`/`AGENTVM_GID` together when present, including home
  directory ownership setup.
- Preserve quiet long-running payloads: lack of stdin/control input is not an
  idle failure while the client remains connected.
- Bound slow/non-reading clients through write deadlines/backpressure and clean
  up the child process group if output cannot be delivered.
- On client disconnect, terminate the process group with TERM, then KILL after a
  grace period.

Diagnostic behavior to preserve:

- Diagnostics are concurrency-limited independently from the single primary
  payload lock.
- Diagnostic timeout and output limits are request-bounded and capped by service
  maxima.
- Timed-out diagnostics return exit code `124`; output-limited diagnostics return
  exit code `125`.
- Diagnostic semaphore slots must be released on client disconnect, write
  failure, timeout, and child failure.

Server bounds to preserve:

- Initial-frame timeout for idle connected clients.
- Maximum active client/session count with immediate reject/close when full.
- Bounded task queues; no unbounded per-client task or byte queues.
- One concise session summary log per Docker bridge session and bounded logging
  for payload failures.

### Docker TCP-to-Unix bridge

The Rust service must preserve the Docker bridge behavior:

- Listen on the configured guest TCP port and connect each accepted client to
  `/var/run/docker.sock`.
- Retry Unix-socket connect for a bounded period so the bridge tolerates Docker
  startup races.
- Limit active bridge sessions and reject/close excess clients.
- Relay arbitrary binary streams bidirectionally without HTTP assumptions.
- Close both sides on idle timeout, EOF, or write failure.
- Log one session summary with byte counts and timeout status, not per-chunk
  relay logs.

## Packaging findings

The appliance builder installs a repo-built Rust guest payload-service binary
and records it in `source_inputs` inside `docker/out/artifact-manifest.json`.

A guest-service binary build requires:

1. A repo-owned guest-service crate/binary with locked dependencies that compile
   under `./vm-frontend/validate.sh required`'s offline cargo mode.
2. A build step that produces a Linux guest binary compatible with the Alpine
   appliance environment.
3. `docker/build-appliance.sh` support to install the binary. Build the Rust
   service for the Alpine/musl guest, for example
   `cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service`.
   The install hook is
   `sudo env AGENTVM_GUEST_SERVICE_BIN=target/x86_64-unknown-linux-musl/debug/agentvm-guest-service ./docker/build-appliance.sh`;
   the builder rejects GNU/glibc-linked host binaries such as
   `target/debug/agentvm-guest-service`.
4. Artifact manifest/source freshness updates for the new binary and any source
   inputs that affect it. The opt-in install hook records the installed binary in
   `source_inputs` so stale binaries are rejected before live boot.
5. Live validation of both Docker bridge and payload protocol before deleting
   any remaining duplicate guest-service code.

Important constraint: Tokio is not currently a dependency of `vm-frontend`, and
required validation uses offline cargo commands. Introducing Tokio for a guest
binary needs a lockfile/vendor/cache plan first; otherwise the required gate will
not be reproducible.

## Completed validation

Rust guest-service parity was validated with:

- `./vm-frontend/validate.sh live-payload`
- `./vm-frontend/validate.sh live-docker`
- `./vm-frontend/validate.sh required`

## Spike conclusion

The Rust payload service replaced the Python payload server after parity
validation. Keep the Docker bridge as its own component until a dedicated bridge
replacement is implemented and validated.
