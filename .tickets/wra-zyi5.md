---
id: wra-zyi5
status: closed
deps: []
links: []
created: 2026-05-18T06:31:15Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-xcvq
tags: [validation, live-setup-tools, pi]
---
# live-setup-tools pi metadata check ignores pi --version stderr

./vm-frontend/validate.sh required reached live-setup-tools with Rust opt-in appliance and failed in the pi package metadata step: pi --version printed 0.73.1 in the combined log, but the inline shell used version=, so if that package writes the version to stderr the captured variable is empty and validation reports 'unexpected pi version:'. Relevant file: vm-frontend/validate.sh live_setup_tools pi_metadata_log command.

## Acceptance Criteria

The live-setup-tools pi metadata check captures pi --version from stderr/stdout consistently, still validates semver, and ./vm-frontend/validate.sh required can proceed past the pi metadata step.


## Notes

**2026-05-18T06:31:26Z**

Fixed vm-frontend/validate.sh live_setup_tools pi metadata command to capture `pi --version` with `2>&1` before semver validation, matching the earlier bootstrap/restart checks that inspect combined output. This addresses the observed required-validation failure where pi printed `0.73.1` in the log but command substitution captured an empty stdout-only value.

**2026-05-18T06:33:46Z**

Validated fix with ./vm-frontend/validate.sh required. The pi package metadata step now captures `pi --version` from combined stdout/stderr and printed `pi-version=0.73.1`; required validation completed successfully afterward.
