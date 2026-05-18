---
id: wra-hizo
status: closed
deps: []
links: [wra-xaf5, wra-0djh]
created: 2026-05-18T10:36:00Z
type: chore
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, appliance, dependencies]
---
# Drop unused bubblewrap appliance dependency

bubblewrap is still pinned, installed into the appliance, and recorded in artifact manifests after the VM-only model. The duplicate scans found no current runtime use. Remove the unused package/pin/manifest field instead of carrying it as historical dependency baggage.

## Design

Audit docker/appliance.env, docker/build-appliance.sh, docker/refresh-pins.sh, artifact manifest versions, and docs for BUBBLEWRAP_VERSION/bubblewrap. If no runtime path uses it, delete the pin, install package, refresh logic, manifest field, and docs. If a current runtime path still uses it, document the exact owner and close this as superseded with evidence.

## Acceptance Criteria

bubblewrap is no longer installed or recorded in appliance metadata unless an active runtime owner is documented; refresh-pins and build-appliance no longer require the version; tests/docs reflect the dependency set; required validation is recorded before close if appliance inputs change.


## Notes

**2026-05-18T12:04:54Z**

Iteration 14 implementation: removed the unused bubblewrap appliance dependency from docker/appliance.env, docker/build-appliance.sh package install/version pin requirements/manifest versions, docker/refresh-pins.sh version resolution/env rewrite/reporting, and docker/README.md package docs. Added docker/tests/test_build_appliance.sh guard that fails if BUBBLEWRAP_VERSION or bubblewrap= remains in active appliance pin/install/refresh/docs inputs. Validation so far: bash -n docker/build-appliance.sh docker/refresh-pins.sh; AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 bash docker/build-appliance.sh; bash docker/tests/test_build_appliance.sh; python3 -m unittest docker.tests.test_guest_services; ./vm-frontend/validate.sh guest-services; ./vm-frontend/validate.sh docs. This touches appliance inputs and requires sudo ./docker/build-appliance.sh before live/required validation and close.

**2026-05-18T12:08:34Z**

Validation complete after Henrik rebuilt appliance: ./vm-frontend/validate.sh required passed with timeout 300s, including docs/fmt/offline tests, guest-services test_build_appliance guard, fuzz target compilation, live-smoke, and live-setup-tools. Bubblewrap is no longer pinned, installed, refreshed, documented in active appliance docs, or recorded in appliance manifest versions. Closing wra-hizo.
