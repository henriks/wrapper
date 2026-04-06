---
id: wra-6dn7
status: closed
deps: []
links: []
created: 2026-04-03T18:54:14Z
type: feature
priority: 2
assignee: Henrik Saksela
tags: [wrapper, pi]
---
# Add pi agent support to sandbox wrapper

Extend /home/hsaksela/ai/wrapper/sandbox-wrap to support the pi coding agent alongside codex and copilot. Update tool detection, --tool choices, tool metadata (CLI/package/home mounts), and any user-facing help/error text. Add a pi-wrap symlink similar to codex-wrap/copilot-wrap. Review pi docs to confirm the npm package name/CLI/config directory, then update requirements.md to document pi support.


## Notes

**2026-04-03T18:56:01Z**

Reviewed pi docs before implementation. Pi CLI is installed from npm package @mariozechner/pi-coding-agent, executable name pi, and persistent config/session/auth data lives under ~/.pi/agent (so mounting ~/.pi is sufficient for normal auth/config reuse). I also mounted ~/.agents so global agent skills remain available inside the sandbox. During implementation I found the wrapper was treating EXTRA_ARGS as a full replacement command instead of passing them through to the selected CLI; I corrected that while adding pi so tool flags/messages now behave as documented.
