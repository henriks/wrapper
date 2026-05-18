---
id: wra-b9b1
status: closed
deps: []
links: [wra-y5l6, wra-xaf5, wra-fpy2, wra-39g2, wra-0djh]
created: 2026-05-18T10:35:38Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, guest-service, docker, rust]
---
# Move guest Docker socket bridge into Rust or delete the guest TCP bridge path

After the Rust guest payload service became the appliance path, docker/guest-socket-bridge.py remains as the last Python guest service. docker/guest-init.sh starts it, docker/tests/test_guest_services.py tests it separately, and vm-frontend/src/docker_proxy.rs already contains a Rust/Tokio host-side proxy. This creates duplicated bridge/session-limit/timeout behavior and keeps Python in the guest for one narrow service.

## Design

Decide whether the guest Docker TCP-to-Unix bridge belongs as a subcommand/capability in guest-service or whether the guest TCP bridge path should be deleted entirely in favor of another Docker exposure contract. If keeping Docker TCP bridge behavior, implement it in Rust with shared limits/logging semantics and remove guest-socket-bridge.py plus Python-specific tests. Coordinate appliance packaging, artifact freshness, docker docs, and live-docker validation.

## Acceptance Criteria

guest-socket-bridge.py is removed from appliance runtime or explicitly superseded by a documented no-bridge Docker contract; guest-init starts one Rust guest-service path for payload plus any retained Docker bridge behavior; Python bridge tests are deleted or replaced by Rust tests; live Docker validation and required validation are recorded before close.


## Notes

**2026-05-18T10:55:02Z**

Iteration 4 audit: guest Docker bridge is still Python-owned. docker/guest-init.sh starts python3 -u /usr/local/libexec/agentvm-socket-bridge after dockerd readiness; docker/build-appliance.sh installs docker/guest-socket-bridge.py and records it as an appliance source; docker/tests/test_guest_services.py owns bridge behavior tests. Keeping Docker TCP bridge behavior means adding a Rust guest-service subcommand/capability and then removing the Python script/tests/install path. This touches appliance inputs and will require Henrik to run a sudo appliance rebuild before live validation/close.

**2026-05-18T11:39:02Z**

Iteration 10 progress: added the retained Docker TCP-to-Unix bridge capability to the Rust guest-service binary. guest-service/src/lib.rs now exposes DockerBridgeLimits, DockerBridgeStats, serve_docker_bridge_tcp/serve_docker_bridge_listener, and handle_docker_bridge_client using Tokio TcpListener/TcpStream, UnixStream, bounded session semaphore, connect retry timeout, IO timeout, and copy_bidirectional. guest-service/src/main.rs now supports an explicit docker-bridge subcommand while keeping payload as the default mode. Added Rust tests for bridge relay and CLI parsing. Verification: cargo fmt --manifest-path guest-service/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml docker_bridge; cargo check --manifest-path guest-service/Cargo.toml. Next step is appliance/package switchover: update guest-init/build inputs to start guest-service docker-bridge, remove guest-socket-bridge.py from appliance sources, and replace/delete Python bridge tests. This touches the packaged guest-service binary and guest init/appliance inputs, so Henrik will need to run sudo ./docker/build-appliance.sh before live Docker/required validation and before this ticket can close.

**2026-05-18T11:42:09Z**

Iteration 11 progress: switched appliance/runtime ownership from the Python Docker bridge to the Rust guest-service docker-bridge subcommand. docker/guest-init.sh now starts "agentvm-guest-service docker-bridge" for the Docker TCP bridge and uses guest-docker-bridge.log; docker/build-appliance.sh no longer installs or records docker/guest-socket-bridge.py; docker/guest-socket-bridge.py was deleted; docker/tests/test_guest_services.py now covers guest shell syntax only; appliance freshness inputs/tests now require build-appliance.sh and guest-init.sh without the deleted Python bridge. Updated active docs/log references. Verification: cargo fmt --manifest-path guest-service/Cargo.toml; cargo fmt --manifest-path vm-frontend/Cargo.toml; cargo test --manifest-path guest-service/Cargo.toml docker_bridge; cargo test --manifest-path guest-service/Cargo.toml parses_docker_bridge_cli_config; python3 -m unittest docker.tests.test_guest_services; cargo test --manifest-path vm-frontend/Cargo.toml --bin agentvm-frontend appliance_source_hashes. Because these edits happened after Henrik's appliance rebuild, the appliance artifacts must be rebuilt again with sudo ./docker/build-appliance.sh before live/required validation can be meaningful and before this ticket can close.

**2026-05-18T11:43:04Z**

Follow-up in same iteration: adjusted the Rust Docker bridge IO timeout to be an idle timeout rather than a total session duration limit, preserving the Python bridge semantics for long active Docker API calls. Added docker_bridge_closes_idle_session_after_timeout coverage. Re-ran cargo fmt --manifest-path guest-service/Cargo.toml and cargo test --manifest-path guest-service/Cargo.toml docker_bridge.

**2026-05-18T11:45:15Z**

Validation after Henrik's rebuild failed in live-docker. guest-docker-bridge.log showed the packaged guest-service binary rejected the new docker-bridge subcommand and printed the old usage, so the rebuild used a stale target/x86_64-unknown-linux-musl/debug/agentvm-guest-service binary (manifest sha256 c4e97e7b...). I rebuilt that binary locally with cargo build --target x86_64-unknown-linux-musl --bin agentvm-guest-service; new sha256 is 4891fad... and --help includes docker-bridge. Created linked bug wra-39g2 because build-appliance validates/records the supplied binary but does not detect that it is stale relative to guest-service sources. Need Henrik to rerun sudo ./docker/build-appliance.sh with the freshly rebuilt musl binary before live validation can pass.

**2026-05-18T11:51:38Z**

Validation complete after Henrik rebuilt with the fresh musl guest-service binary (artifact manifest records sha256 4891fad...). Ran ./vm-frontend/validate.sh live-docker && ./vm-frontend/validate.sh required with timeout 300s. live-docker passed all three Docker self-tests (container egress allow, no-net denied, published host-to-container port), and required passed including offline tests, fuzz target compilation, live-smoke, and live-setup-tools. Acceptance criteria met: Python guest socket bridge removed from appliance runtime/packaging, guest-init starts Rust guest-service docker-bridge, Rust tests replace Python bridge behavior coverage, and live/required validation is recorded.

**2026-05-18T12:35:00Z**

Follow-up bug wra-39g2 now has an implementation pending rebuild/validation: build-appliance rejects repo AGENTVM_GUEST_SERVICE_BIN files older than guest-service/payload-protocol source inputs and records those source hashes in artifact-manifest source_inputs, so the stale docker-bridge binary scenario should be caught before packaging/live boot.
