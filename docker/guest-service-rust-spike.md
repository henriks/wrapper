# Rust/Tokio guest service spike

This note records the `wra-lcbk` spike outcome after the Python guest service
bounds landed in `wra-g0uv`, `wra-dky9`, and `wra-sum7`.

## Current decision

Do **not** switch the appliance from the Python services to a Rust/Tokio guest
service yet.

The Python services are now bounded and validated:

- `guest-init.sh` waits for Docker `_ping` before exposing payload readiness.
- `guest-socket-bridge.py` has client/session limits, Docker-socket connect
  retry, idle/write failure cleanup, and summary logging.
- `guest-payload-server.py` has initial-frame timeouts, bounded active clients,
  bounded diagnostic sessions/output, slow-writer cleanup, process-group cleanup,
  and a regression for quiet long-running primary payloads.

A Rust replacement can be valuable, but it is a packaging and recovery change, not
just a refactor. Keep the Python services as the default until a Rust guest
service is built and validated behind an explicit opt-in appliance path.

## Semantics a Rust guest service must preserve

### Payload control TCP service

The Rust service must preserve the existing frame protocol used by
`vm-frontend/src/payload_client.rs` and `docker/guest-payload-server.py`:

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

The Rust service must preserve the Docker bridge behavior from
`guest-socket-bridge.py`:

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

The appliance builder currently installs Python guest assets directly in
`docker/build-appliance.sh` and records them in `source_inputs` inside
`docker/out/artifact-manifest.json`.

Adding a Rust guest binary would require all of the following before switching
defaults:

1. A repo-owned guest-service crate/binary with locked dependencies that compile
   under `./vm-frontend/validate.sh required`'s offline cargo mode.
2. A build step that produces a Linux guest binary compatible with the Alpine
   appliance environment.
3. `docker/build-appliance.sh` changes to install the binary, while retaining the
   Python fallback until live parity is proven.
4. Artifact manifest/source freshness updates for the new binary and any source
   inputs that affect it.
5. Guest init changes to choose Python default vs Rust opt-in explicitly.
6. Live validation of both Docker bridge and payload protocol before any default
   switch.

Important constraint: Tokio is not currently a dependency of `vm-frontend`, and
required validation uses offline cargo commands. Introducing Tokio for a guest
binary needs a lockfile/vendor/cache plan first; otherwise the required gate will
not be reproducible.

## Recommended implementation order

1. Add a small repo-local Rust guest-service crate that is **not installed into
   the appliance**. Start with shared frame parsing/serialization and pure unit
   tests mirroring the Python payload protocol tests.
2. Add integration tests that run the Rust service locally on loopback or
   socketpairs and compare payload/Docker bridge behavior against the Python
   service contract.
3. Add an opt-in appliance packaging path that installs the Rust binary while
   retaining the Python services and default startup path.
4. Add live opt-in validation for Rust guest service mode:
   - `./vm-frontend/validate.sh live-payload`
   - `./vm-frontend/validate.sh live-docker`
   - `./vm-frontend/validate.sh required`
5. Only after parity, switch the default guest init path and keep a short-lived
   fallback/remove ticket for the Python services.

## Spike conclusion

The bounded Python services are now a good compatibility baseline. The Rust
replacement should proceed as a staged, opt-in migration with tests and packaging
freshness support, not as an immediate default replacement.
