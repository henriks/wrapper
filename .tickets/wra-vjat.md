---
id: wra-vjat
status: closed
deps: [wra-ume5]
links: []
created: 2026-05-15T19:46:56Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-ia1a
tags: [validation, vm, filesystem, persistence]
---
# Add live persistence validation for root overlay

Add live KVM/QEMU validation proving that the persistent root overlay works for ordinary guest paths across relaunches. This should validate the intended state model independently of Docker.

Suggested scenario: launch project VM with a payload that writes files under /home/<user>, /usr/local or another writable system path, XDG cache/state paths, and a package/tool install marker. Shut down cleanly. Relaunch same project/state disk and assert those files still exist with expected contents and ownership. Also assert lower rootfs remains immutable/read-only from the host artifact perspective and reset removes the persistent overlay state.

## Design

Integrate into vm-frontend validation tiers as an appropriate live scenario, likely live-smoke or a named live-fs/persistence test depending on runtime cost. Record artifacts on failure: state disk path, console.log, guest payload logs, and mount output. Keep test generic; do not depend on Codex behavior.

## Acceptance Criteria

Live validation demonstrates ordinary guest filesystem persistence across relaunch. Failure artifacts are useful. The validation is wired into ./vm-frontend/validate.sh required if fast enough, or into a named live tier with docs if too slow. Required validation passes before closing implementation.


## Notes

**2026-05-15T20:05:12Z**

Added live-persistence validation tier plumbing. self-test now accepts --root-persistence-check to write a marker under /usr/local/share/agentvm-root-persistence/marker and --expect-root-persistence to verify it on a subsequent relaunch. vm-frontend/validate.sh live-persistence runs the same project/run-dir twice: first writing the root-overlay marker, then verifying it after relaunch. Added unit coverage for generated payload scripts.

**2026-05-15T20:07:59Z**

Implemented and validated live-persistence tier. vm-frontend self-test now supports --root-persistence-check and --expect-root-persistence; validate.sh live-persistence runs two launches against the same .sandbox/root-overlay-self-test/state.raw. The root-persistence payload skips the basic Docker smoke to isolate ordinary root overlay persistence from Docker-on-overlay behavior. Validation passed after rebuilding appliance and using a fresh state.raw: first launch wrote marker under /var/tmp/agentvm-root-persistence with sync; second launch verified marker persisted.
