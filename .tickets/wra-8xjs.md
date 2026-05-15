---
id: wra-8xjs
status: open
deps: []
links: [wra-7xi3, wra-13x7]
created: 2026-05-15T20:58:37Z
type: feature
priority: 1
assignee: Henrik Saksela
tags: [vm, guest, tooling, mise]
---
# Reintroduce mise-based agent tool bootstrap

Restore mise as the primary mechanism for preparing Node/npm for agent CLI bootstrap in the VM guest. Current behavior in vm-frontend/src/main.rs::tool_bootstrap_payload_script bypasses mise and installs requested agent CLIs with system npm into $HOME/.local. That was introduced in wra-mkh8 after bare/project `mise install` caused live-guest problems: auto-generated/project mise.toml could force Node source builds or unrelated project runtime installs. The desired behavior is not to bypass mise, but to use it deliberately: maintain an agent/bootstrap mise config and inject only the tool-specific rows required for the requested setup tool. Relevant current code: vm-frontend/src/main.rs path/env setup includes $HOME/.local/share/mise/shims; tool_bootstrap_payload_script currently emits NPM_CONFIG_PREFIX and npm install --global; tests around setup_tool_*_bootstrap_script assert npm behavior. Appliance build still bakes pinned upstream mise in docker/build-appliance.sh and docker/appliance.env, and also includes nodejs/npm today. Root project mise.toml currently declares node/python for development, but guest agent bootstrap must not blindly run project-level mise install.

## Design

Use a narrow, explicit mise config for agent bootstrap rather than bare `mise install` against the workspace. Depending on requested setup tool, inject the necessary rows into the guest's mise.toml (user wrote 'miso.toml', assumed mise.toml) so mise installs/provides Node/npm and the requested npm-backed CLI package only. Avoid pulling arbitrary project runtimes into first-run agent startup. Keep project tooling and agent bootstrap concerns separate: if project mise.toml is trusted/used for interactive shell commands, it must not expand the agent CLI install surface. Update tests to assert targeted mise usage and tool-specific config injection for Codex/Pi. Update docs if config/default semantics or setup behavior are documented.

## Acceptance Criteria

Fresh guest setup for Codex/Pi uses mise to install/provide Node/npm and the requested agent CLI, not direct system npm as the primary bootstrap path. The emitted/managed mise.toml rows are deterministic and include only the requested tool's requirements. Project-local mise.toml does not cause unrelated runtimes/tools to install during agent bootstrap. Existing PATH/shim behavior is preserved or intentionally simplified. Relevant unit tests in vm-frontend are updated from direct npm assertions to mise-based assertions, with coverage for Codex and Pi. Required validation gate ./vm-frontend/validate.sh required is run successfully, including live-smoke, before closing; if the environment cannot run live validation, document that limitation and leave the ticket open.


## Notes

**2026-05-15T21:12:09Z**

Live evidence while investigating current Codex setup stall: /home/hsaksela/Code/planb ran agentvm --setup-tool codex (pid 4444, qemu pid 4453, listener 127.0.0.1:12076). Logs in /home/hsaksela/Code/planb/.sandbox/docker-vm/run show guest booted and payload session active. vmnet-events.log shows npm fetching https://registry.npmjs.org/@openai%2fcodex via MITM plus repeated IPv6 unsupported_protocol denials, but no observed tarball fetch or successful exec. A concurrent payload-client diagnostic was rejected with 'payload session already active'. Current offline tests only assert generated bootstrap script contains npm install; live-smoke self-test does not install or run Codex.
