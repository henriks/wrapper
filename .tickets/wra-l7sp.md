---
id: wra-l7sp
status: closed
deps: []
links: []
created: 2026-05-16T18:42:14Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-bjaa
---
# Fix live setup-tool Codex bootstrap missing optional dependency

During wra-bjaa iteration 45, ./vm-frontend/validate.sh required failed in live-setup-tools while bootstrapping Codex via mise/npm. The guest installed npm:@openai/codex@0.130.0 and Node 24.15.0, then running codex failed with: Error: Missing optional dependency @openai/codex-linux-x64. Reinstall Codex: npm install -g @openai/codex@latest. This occurred after host Rust/vmnet/TLS tests and live-smoke passed, during the Codex setup-tool live scenario. Initial investigation considered whether the setup-tool recipe/mise/npm invocation was omitting optional dependencies, using a musl Node build incompatible with Codex optional package resolution, or hitting a transient npm packaging issue. Reference log: /tmp/pi-bash-2751f62a5c584ed0.log in the dev session. Later notes identify the actual root cause as the pending guest-byte watermark added in `wra-g13g`.


## Notes

**2026-05-16T19:03:59Z**

Root cause was the new pending_guest_bytes watermark in wra-g13g, not a setup-tool recipe bug. The first implementation checked incoming upstream->guest chunks against the pending guest queue limit before attempting any smoltcp send, so large TLS MITM download bursts (~1.1-1.6 MiB in setup-tool logs) were closed even when most/all bytes were immediately sendable. npm treated the optional Codex native package fetch as optional and continued, causing codex to fail later with missing @openai/codex-linux-x64. Fixed by making the watermark apply to queued unsent bytes only: flush existing pending data, attempt direct guest send first when the pending queue is empty, and only limit the remainder that must be queued. Also raised the default pending guest queue to 8 MiB while keeping upstream TLS/plaintext defaults at 1 MiB. Added pending_guest_bytes_limit_does_not_reject_direct_guest_send regression. live-setup-tools passed after the fix with codex-cli 0.130.0 and Pi bootstrap metadata checks.
