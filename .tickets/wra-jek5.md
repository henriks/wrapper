---
id: wra-jek5
status: open
deps: []
links: []
created: 2026-05-14T21:45:29Z
type: bug
priority: 0
assignee: Henrik Saksela
parent: wra-yz27
tags: [vm, filesystem, guest-home]
---
# Expose guest home at the host-natural home path

The guest currently sees HOME as <project>/.sandbox/home. That leaks an implementation directory into the guest and causes tool-state mounts like ~/.codex to appear at <project>/.sandbox/home/.codex instead of the same absolute path Codex and other tools use on the host.

Desired behavior: the guest should see the same home path as the host user, for example /home/hsaksela. The guest does not need to know anything about .sandbox. Writes under the guest-visible home path should be handled by the wrapper/composed filesystem backing policy, such as project-local overlay/state, but the visible path must remain natural. Do not solve this by mounting the broad host home read-write.

Relevant code/docs:
- vm-frontend/src/main.rs: guest_payload_env currently sets HOME, XDG_* and PATH from config.project.join(".sandbox/home").
- vm-frontend/src/runtime_manifest.rs: guest_runtime_mounts currently uses guest_home = project/.sandbox/home and home_mount maps tool state there.
- docker/guest-init.sh: composed bind reconstruction may need to create/bind the natural /home/<user> path.
- requirements.md and docker/runtime-contract.md currently document the project-local guest HOME and need updating.
- plan.md lines around Guest Filesystem Model already show the natural path idea.

## Design

- Separate guest-visible paths from host backing paths in the runtime manifest generation.
- Keep project-local storage as an implementation detail, but bind/project it into the guest at /home/<host-user> or the resolved host HOME path.
- Preserve natural workspace paths and /workspace compatibility.
- Ensure the appliance /home tmpfs setup in guest-init does not hide or conflict with the natural home bind.

## Acceptance Criteria

- Running a tool payload prints HOME equal to the host HOME path, not <project>/.sandbox/home.
- No guest-visible HOME, XDG_* path, Codex path, or PATH entry contains <project>/.sandbox/home.
- The guest can write expected home-local files and those writes persist according to the wrapper backing policy.
- Unit tests cover guest_payload_env and runtime_manifest path generation.
- Docs describe guest-visible home path separately from backing storage.

