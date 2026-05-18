---
id: wra-do0x
status: closed
deps: []
links: []
created: 2026-05-18T10:35:44Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, guest, compatibility]
---
# Delete guest /workspace compatibility alias

docker/guest-init.sh still treats /workspace as a compatibility alias/fallback for the project path. Project policy says only .sandbox/config.json compatibility matters; non-config path aliases should not be preserved. The guest should mount exactly the configured project path when it is supported and fail clearly when it is not.

## Design

Audit guest-init project path handling around runtime mount setup and /workspace bind creation, plus docker/runtime-contract.md and requirements.md references. Remove silent fallback behavior and alias bind mounts unless there is a current product requirement unrelated to compatibility. Update docs/tests to describe the single supported project mount contract.

## Acceptance Criteria

/workspace alias/fallback behavior is removed or replaced by a clear fail-closed error for unsupported guest project paths; active runtime docs no longer present /workspace as compatibility behavior; validation covers the supported project path contract; required validation is recorded before close.


## Notes

**2026-05-18T12:04:54Z**

Iteration 14 start/audit: batching upcoming guest mount contract work with wra-hizo appliance changes to minimize rebuild churn. Current guest-init still creates /workspace in install_guest_assets, allows PROJECT_PATH=/workspace, falls back unsupported PROJECT_PATH values to /workspace, and bind-mounts PROJECT_PATH to /workspace when different. Active docs still describe /workspace as a compatibility alias. Implementation should fail closed for unsupported project paths and stop creating/binding the alias; this will touch guest-init/build manifest/runtime docs and require sudo appliance rebuild before close.

**2026-05-18T12:09:56Z**

Iteration 15 implementation: removed the guest /workspace alias/fallback. docker/guest-init.sh no longer creates /workspace, no longer accepts /workspace as supported PROJECT_PATH, no longer silently falls back unsupported project paths to /workspace, and no longer bind-mounts PROJECT_PATH to /workspace. Unsupported project paths now fail closed with an explicit guest-init error requiring project_path under /home or /tmp. docker/build-appliance.sh no longer creates /workspace in the rootfs and the appliance manifest guest metadata now records project_mount as configured project_path instead of workspace_mount=/workspace. Updated active runtime docs in docker/README.md, docker/runtime-contract.md, and requirements.md. Added docker/tests/test_guest_services.py coverage to guard against restoring the alias/fallback. Verification so far: bash -n docker/guest-init.sh docker/build-appliance.sh; AGENTVM_GUEST_INIT_SOURCE_ONLY=1 . docker/guest-init.sh; AGENTVM_BUILD_APPLIANCE_SOURCE_ONLY=1 bash docker/build-appliance.sh; python3 -m unittest docker.tests.test_guest_services; bash docker/tests/test_build_appliance.sh; ./vm-frontend/validate.sh guest-services; ./vm-frontend/validate.sh docs. This touches appliance inputs after Henrik's rebuild, so sudo ./docker/build-appliance.sh is required again before live/required validation and close.

**2026-05-18T12:12:35Z**

Validation complete after Henrik rebuilt appliance: ./vm-frontend/validate.sh required passed with timeout 300s, including guest-services test coverage for no /workspace alias/fallback, fuzz target compilation, live-smoke, and live-setup-tools. Active runtime/build inputs no longer create /workspace, fallback unsupported project paths to /workspace, bind PROJECT_PATH to /workspace, or record workspace_mount=/workspace in the appliance manifest. Closing wra-do0x.
