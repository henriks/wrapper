---
id: wra-5rrw
status: closed
deps: [wra-8xjs]
links: []
created: 2026-05-15T21:53:19Z
type: task
priority: 1
assignee: Henrik Saksela
tags: [tests, live, pi, mise, setup-tool]
---
# Add Pi/mise setup-tool live smoke after mise bootstrap

Follow-up split from wra-13x7. Codex setup-tool bootstrap/persistence is now covered by required live-setup-tools via wra-j7lp. After wra-8xjs restores mise-based agent tool bootstrap, add/extend live setup-tool validation for Pi and assert the intended mise path rather than the temporary npm-only path.

## Acceptance Criteria

live-setup-tools (or a named sibling tier) covers Pi setup-tool bootstrap non-interactively with a bounded version/help command. The test verifies the intended mise bootstrap path after wra-8xjs, survives payload shutdown/relaunch as applicable, and documentation names when it runs. Required gate policy is updated if Pi should become mandatory.


## Notes

**2026-05-15T22:19:48Z**

Pi live bootstrap failed after a long npm install with 'payload client IO failed: failed to fill whole buffer'. Logs show the VM was later SIGKILLed by frontend cleanup and no payload-server crash text; vmnet showed many npm tarball fetches. Likely the Pi command path closed without sending an exit frame or was still unstable under the previous node=system/bootstrap script. Retesting after node@24 musl mise change is needed; keep an eye on payload framing if it reproduces.

**2026-05-15T22:27:37Z**

Retested Pi after sequential http:node + npm package mise bootstrap. The previous 'failed to fill whole buffer' did not reproduce. live-setup-tools installed Pi 0.73.1, no-net restart printed 0.73.1, and metadata verification found package.json plus pi-version=0.73.1.

**2026-05-15T22:46:15Z**

Pi failure root cause: first install command used mise install -C  <http-node-spec>, but mise still loaded the config and attempted the npm package in parallel/alongside Node, so the supposedly sequential bootstrap was not actually sequential. Pi's larger dependency tree exposed this as a payload connection failure while npm was still fetching. Fixed by installing http:node from a separate temp bootstrap cwd with no mise.toml, then installing npm:<package> from the targeted config after Node is present. Added quiet/non-progress env for setup-tool installs. live-setup-tools now passes for Codex and Pi.

**2026-05-15T22:51:47Z**

After a successful standalone live-setup-tools run, required validation still reproduced Pi payload EOF during npm fetches. Added npm install stabilizers for setup-tool bootstrap: NPM_CONFIG_MAXSOCKETS=1 to avoid many concurrent registry/TLS MITM streams for Pi's larger dependency tree, plus retry timeouts. A subsequent standalone live-setup-tools pass succeeded for Codex and Pi.
