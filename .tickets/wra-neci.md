---
id: wra-neci
status: closed
deps: []
links: []
created: 2026-05-15T10:36:57Z
type: epic
priority: 0
assignee: Henrik Saksela
---
# Make validation representative across VM, network, guest, filesystem, and UX

The current test suite is broad but still misses important real-world contracts, as shown by live failures around mio readiness, DNS EDNS records, guest loopback setup, stale appliance artifacts, and Docker pull behavior. This epic turns the five independent undertesting reviews into an implementation program. Scope covers offline tests, live validation matrix, appliance freshness, guest Python services, vmnet readiness, DNS, payload protocol, composed-fs through the real guest/kernel path, CLI/TUI config UX, security/log assertions, and validation workflow enforcement.

## Design

Favor executable validation over documentation-only policy. Add deterministic offline harnesses where possible; add live VM scenarios where the kernel, QEMU, guest services, or Docker are part of the contract. Keep live tiers named and diagnosable so failures distinguish environment problems from product regressions. Ticket dependencies should build foundations first: validation command structure, artifact freshness, cleanup/lifecycle, then live matrix and subsystem-specific coverage.

## Acceptance Criteria

There is a coherent validation hardening suite whose required tier runs offline tests, formatting, fuzz target compilation, and live validation; live validation includes multiple named scenarios; stale artifacts fail before boot; runtime readiness, DNS, guest services, payload framing, Docker bridge, composed-fs live behavior, CLI/TUI config, and security/log contracts have dedicated regression coverage.


## Notes

**2026-05-15T10:43:32Z**

Started Ralph loop wra-neci to implement epic in dependency order. Loop task file: .ralph/wra-neci.md

**2026-05-15T11:31:31Z**

After user rebuilt appliance artifacts, ./vm-frontend/validate.sh required passed end-to-end on 2026-05-15. Closed child tickets with live/required evidence: wra-pssg, wra-1bks, wra-o9y4, wra-6cei, wra-otbe, wra-crwz. Remaining active child: wra-0hio; next ready children include wra-gq92 and wra-l8ke, with wra-kh0g still depending on wra-0hio.

**2026-05-15T14:57:31Z**

All child tickets are closed with validation evidence. Final gate after TUI coverage passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline; ./vm-frontend/validate.sh required; and ./vm-frontend/validate.sh live. The required tier now covers docs drift, formatting, composed-fs/vm-frontend offline tests including TUI terminal tests, guest services, fuzz target compilation, and live-smoke. Named live scenarios include smoke/hostile/payload/DNS/Docker/fs/full; stale artifact preflight and subsystem-specific runtime/DNS/guest/payload/Docker/fs/CLI/TUI/security coverage are implemented.
