---
id: wra-s38i
status: closed
deps: [wra-3z80]
links: []
created: 2026-05-15T19:47:19Z
type: task
priority: 0
assignee: Henrik Saksela
parent: wra-ia1a
tags: [vm, filesystem, config, persistence]
---
# Remove .sandbox/home special-case persistence after root overlay

Once persistent root overlay is in place, remove the Bubblewrap-era .sandbox/home special case. Current runtime_mounts creates .sandbox/home and runtime_manifest::guest_runtime_mounts adds RuntimeMount::persistent_home(project/.sandbox/home -> host_home_dir()). guest-init.sh mounts /home tmpfs and then binds composed entries; guest_payload_env sets HOME to host_home_dir(). With a persistent root overlay, guest /home can be a normal writable/persistent guest path unless explicitly overlaid by configured host shares.

Relevant files: vm-frontend/src/runtime_manifest.rs RuntimeMount::persistent_home, ManifestSourceClass::PersistentHome, guest_runtime_mounts tests; vm-frontend/src/main.rs runtime_mounts creates .sandbox/home; docker/guest-init.sh /home tmpfs behavior; docs in requirements.md, docker/OPERATIONS.md, docker/runtime-contract.md, vm-frontend/config-json.md.

## Design

Remove the default persistent-home RuntimeMount and .sandbox/home directory creation. Ensure guest /home exists on the overlay root with suitable permissions. Keep HOME env as the natural guest path (/home/<user>) but do not imply host/project backing unless configured. Setup recipes should add explicit shares for host tool state/auth as needed. Update tests/docs to remove .sandbox/home assumptions.

## Acceptance Criteria

.sandbox/home is no longer created or documented as default guest HOME backing. HOME remains a normal guest path on the persistent root overlay. Existing workspace/tool/auth explicit shares still work. Tests and required validation pass.


## Notes

**2026-05-15T20:12:37Z**

Implementation progress: removed the default persistent-home RuntimeMount and PersistentHome source class from runtime_manifest.rs; guest_runtime_mounts now starts with only the workspace plus explicit tool/auth/user shares. vm-frontend runtime_mounts no longer creates .sandbox/home. docker/guest-payload-server.py now ensures an absolute HOME directory exists on the guest root overlay before launching the payload, creating/chowning it only when it did not already exist. Tests updated away from .sandbox/home assumptions for runtime manifests and reset state. Targeted validation passed: cargo fmt; ./docker/tests/test_guest_init.sh; python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p "*test*.py" -v; cargo test --manifest-path vm-frontend/Cargo.toml --offline guest_runtime_mounts; cargo test --manifest-path vm-frontend/Cargo.toml --offline reset_project_removes_project_local_sandbox_state. Because guest-payload-server.py changed, appliance artifacts need rebuild before live validation.

**2026-05-15T20:13:24Z**

Live validation attempt after removing .sandbox/home was blocked by stale appliance artifacts: docker/guest-payload-server.py changed since docker/out/artifact-manifest.json was written. validate.sh requested rerun sudo ./docker/build-appliance.sh. Do not close until rebuilt appliance live validation passes.

**2026-05-15T20:21:53Z**

After appliance rebuild, live-smoke reached payload execution but failed immediately after home-dir-ok/CA checks. Diagnosis: with persistent-home removed, guest-init creates /home/<user> as root-owned while preparing the workspace bind target (/home/<user>/...); payload server saw HOME already existed and did not chown it, so the non-root payload could not create $HOME/.codex. Fix implemented in docker/guest-payload-server.py: ensure_home now chowns HOME when uid/gid differ, not only when it creates the directory. Added offline regression test test_ensure_home_chowns_existing_home_with_wrong_owner. Because guest-payload-server.py changed again, appliance must be rebuilt before live-smoke/live-persistence can pass.

**2026-05-15T20:25:32Z**

Completion evidence after rebuilt appliance: ./vm-frontend/validate.sh live-smoke passed with persistent-home removed and new self-test step proving non-root payload can write to $HOME (home-write-ok). ./vm-frontend/validate.sh live-persistence passed from a fresh root-overlay state disk across two relaunches. ./vm-frontend/validate.sh live-docker passed from a fresh state disk across allow, no-net cached-state, and published-port scenarios. Added validate.sh cleanup so live-smoke/live-docker/live-persistence start their validation state disks fresh and do not inherit stale/corrupt self-test overlays. Offline docs/shell/Python tests also passed.
