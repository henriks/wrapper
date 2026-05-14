---
id: wra-d1s3
status: closed
deps: []
links: []
created: 2026-05-14T20:48:30Z
type: bug
priority: 1
assignee: Henrik Saksela
---
# Fresh-dir wrap hangs at startup spinner

Running ./vm-frontend/target/debug/agentvm-frontend wrap --tool codex from a fresh directory initializes the ext4 workspace image and starts agentvm-composed-fs, but the frontend remains stuck showing the startup spinner after logs like: mke2fs completes, agentvm-composed-fs serves tag workspace on .sandbox/docker-vm/run/virtiofs.sock, and serves tag agentvm-config on .sandbox/docker-vm/run/guest-config.sock. Investigate vm-frontend wrapper startup/readiness handling, especially fresh-directory sandbox initialization, composed-fs startup, VM readiness, and UI spinner completion conditions. Reproduction command from repo root: ./vm-frontend/target/debug/agentvm-frontend wrap --tool codex

## Acceptance Criteria

wrap --tool codex in a fresh directory progresses past startup spinner into the wrapped tool or reports a concrete startup error; add or update regression coverage where practical; document significant implementation findings as ticket notes.


## Notes

**2026-05-14T20:57:23Z**

Observed from user reproduction on 2026-05-14: VM boot reached guest init and payload server. .sandbox/docker-vm/run/state.json reported status=running with qemu_pid. console.log showed MITM CA bundle installed, virtio_net loaded, network configured, dockerd/socket bridge/payload server started. guest-payload-server.log showed listening on tcp 0.0.0.0:1076. payload-client --ping to host port 12076 succeeded, but a second diagnostic payload command returned 'payload session already active', so the original wrapped session was occupying the single payload control path. npm log under .sandbox/home/.npm/_logs/ stopped at 'silly reify moves {}' after HTTP 200 metadata fetches for @openai/codex@0.130.0 and platform package manifests. vmnet-events.log showed DNS for registry.npmjs.org allowed and repeated HTTPS GET /@openai%2fcodex with about 5MB upstream response data, but no obvious tarball path such as /-/...tgz and no npm completion. This suggests VM readiness is OK and the visible spinner is during the active guest tool install/session, likely after npm metadata resolution and before/during reify/tarball install or output forwarding.

**2026-05-14T21:00:03Z**

Fresh repro run: started from repo root with --project /tmp/agentvm-fresh-wrap-repro2. Terminal reproduced bare spinner after mke2fs and virtiofs startup. Logs showed VM status=running and guest init/payload server healthy. Network advanced beyond npm registry metadata to raw.githubusercontent.com/openai/codex/main/announcement_tip.toml, chatgpt.com/backend-api/plugins/featured, and api.github.com/repos/openai/plugins. Polling the wrapper output later showed 'added 2 packages in 11s', then Codex trust prompt for /tmp/agentvm-fresh-wrap-repro2. So in this clean repro the package install succeeded; the confusing symptom is an unlabeled npm progress spinner during first-run install, followed by Codex's own trust prompt. Also observed that running the binary from an empty cwd without --project from repo root fails earlier because artifact-manifest is resolved relative to cwd.

**2026-05-14T21:01:30Z**

Implemented fix in vm-frontend/src/main.rs tool_payload_script: first-run tool bootstrap now prints an explicit agentvm install message before npm, and runs npm install --global --no-progress to avoid showing only an unlabeled spinner. Updated wrapper tests for Codex and Copilot payload scripts. Verification: rebuilt vm-frontend debug binary, then ran timeout --preserve-status 120s ./vm-frontend/target/debug/agentvm-frontend wrap --tool codex --project /tmp/agentvm-fresh-wrap-repro3 -- --version. Output after virtiofs startup included 'agentvm: installing codex CLI in guest HOME (first run only)...', then 'added 2 packages in 10s', then 'codex-cli 0.130.0'.
