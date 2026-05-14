---
id: wra-l6ed
status: closed
deps: [wra-snm9]
links: []
created: 2026-04-01T21:17:14Z
type: feature
priority: 1
assignee: Henrik Saksela
parent: wra-oszw
tags: [vm, test, verification]
---
# Add a real VM self-test and verification harness

Create an explicit verification path for the VM-only sandbox so regressions are caught without relying on ad hoc log spelunking. The user explicitly wants a way for the agent to verify changes on its own where possible, and the current implementation has required repeated manual host testing.

Scope:
- Add a host-runnable end-to-end smoke test for the real KVM path.
- Cover at least: booting the VM, running a trivial payload in the guest, Docker daemon readiness, a simple `docker run` with observable output, a bind-mount check, and optional localhost port publish if still supported.
- Make most host-side configuration/build logic unit-testable without KVM where practical.
- Produce logs/artifacts that make failures diagnosable without interactive reproduction.

Relevant code/docs:
- `sandbox-wrap` and any new helper scripts.
- `docker/build-appliance.sh` and guest boot path.
- `requirements.md` and docs for operator guidance.

This ticket should define a verification story that future changes can reuse.

## Design

Separate KVM-required end-to-end checks from logic that can run in ordinary CI or restricted environments. The smoke path should be one command, deterministic, and targeted at the real supported workflow rather than a synthetic toy mode.

## Acceptance Criteria

There is a documented one-command or one-script verification path for hosts with KVM.

The verification covers the core VM-only workflow end to end.

Non-KVM environments still have some automated coverage for host-side logic that does not require a live VM.

## Notes

**2026-04-01T21:25:38Z**

Contract decisions from wra-fj7n: the verification harness should target the VM-only path, not the old --docker opt-in flow. It should prove that the tool payload really runs inside the guest, that Docker is available there by default, and that guest HOME/project sharing behavior matches the new .sandbox/home/ plus virtio-fs contract.

**2026-05-14T18:11:16Z**

Added a Rust-owned VM self-test harness as agentvm-frontend self-test. The command boots the real microvm path, waits for the guest payload control service, optionally validates a host-published guest payload port with --publish-payload-port, runs a guest payload that checks HOME and workspace sharing, verifies dockerd via docker version/info, runs docker run --rm -v "/home/hsaksela/ai/wrapper:/work:ro" against the configured image (default alpine:3.22), and reads a workspace bind-mounted file from inside the container. Documented the one-command KVM self-test in vm-frontend/README.md and described the non-KVM coverage as the cargo test suite. Verification: cargo test --manifest-path vm-frontend/Cargo.toml --offline passed with 75 lib tests, 1 existing ignored connector test, and 14 CLI tests. Live validation: cargo run --manifest-path vm-frontend/Cargo.toml --offline -- self-test --project /home/hsaksela/ai/wrapper --run-dir /home/hsaksela/ai/wrapper/.sandbox/docker-vm/self-test-final --artifact-manifest /home/hsaksela/ai/wrapper/docker/out/artifact-manifest.json --qemu /usr/bin/qemu-system-x86_64 --publish-payload-port 12079 passed; output included published payload port ok, docker-run-ok, bind-ok, payload-ok, and self-test ok; no QEMU process remained for the self-test run dir.
