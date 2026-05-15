---
id: wra-gx6d
status: closed
deps: []
links: []
created: 2026-05-15T10:37:08Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-neci
---
# Add required validation tier and prevent docs/script drift

Create a single validation entrypoint that matches AGENTS.md expectations instead of relying on separate manual commands. Today vm-frontend/validate.sh has fast/stress/all-local/live, AGENTS.md requires offline plus live, and fuzz/fmt checks are split across scripts/docs. This causes humans and agents to miss required validation.

## Design

Update vm-frontend/validate.sh with a required/full tier that runs composed-fs offline tests, vm-frontend offline tests, cargo fmt --check for relevant manifests, fuzz target compilation/checks, and live validation when host prerequisites exist or an explicit host-live mode is requested. Add a small test or script check that AGENTS.md and validate.sh stay aligned on the required commands. Keep live host-only behavior explicit and diagnosable.

## Acceptance Criteria

A single documented command runs the required validation set. The command includes composed-fs, vm-frontend, fmt checks, and fuzz target compilation/checks. There is coverage preventing AGENTS.md/validate.sh drift. Running without /dev/kvm reports a clear host-live limitation instead of silently passing.


## Notes

**2026-05-15T10:48:15Z**

Implemented required/full validation gate in vm-frontend/validate.sh. Gate now runs docs drift check, rustfmt for composed-fs/vm-frontend/fuzz, composed-fs and vm-frontend offline tests, fuzz target compilation via cargo check on vm-frontend/fuzz, then host-live self-test. Added docs/host-live/fuzz-check tiers, host-live alias, KVM_DEVICE override for diagnostics testing, and updated AGENTS.md plus validation workflow/README. Validation evidence: ./vm-frontend/validate.sh required passed on KVM host; KVM_DEVICE=/tmp/agentvm-no-kvm-for-validate-test ./vm-frontend/validate.sh host-live returned status 1 with host-live limitation diagnostic.
