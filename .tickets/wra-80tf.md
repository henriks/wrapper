---
id: wra-80tf
status: closed
deps: []
links: []
created: 2026-05-18T07:26:35Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-9m5h
---
# FOLLOW-UP: Fix live-setup-tools stale agentvm binary path

During wra-2ku5 required validation after rebuilding the appliance, live-smoke passed but live-setup-tools failed because vm-frontend/validate.sh built agentvm with cargo from the workspace but executed vm-frontend/target/debug/agentvm. The workspace build writes target/debug/agentvm, leaving vm-frontend/target/debug/agentvm stale. The stale binary did not generate guest-config/launch.json, causing guest-init to panic with missing launch config. Fix validate.sh to execute the binary Cargo actually builds (workspace target/debug/agentvm) or otherwise derive it robustly, then rerun required validation.

## Acceptance Criteria

live-setup-tools uses the freshly built agentvm binary; required validation passes after appliance rebuild; ticket notes include evidence and stale-binary diagnosis.


## Notes

**2026-05-18T07:28:39Z**

Fixed validate.sh live-setup-tools to execute /home/hsaksela/ai/wrapper/target/debug/agentvm, the workspace binary produced by cargo build --manifest-path vm-frontend/Cargo.toml --offline --bin agentvm, instead of stale vm-frontend/target/debug/agentvm. Diagnosis confirmed by failed required validation: live-smoke used fresh cargo run and passed launch-config-ok, but live-setup-tools used stale vm-frontend/target/debug/agentvm from May 17, generated no guest-config/launch.json, and guest-init panicked. After fix, ./vm-frontend/validate.sh required passed; full output /tmp/pi-bash-2be9ae6dec1fa6b4.log.
