---
id: wra-nsvn
status: closed
deps: [wra-kh0g, wra-a0bu, wra-pssg]
links: []
created: 2026-05-15T10:39:41Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, security, observability]
---
# Validate security policy, manifests, and diagnostics

Add negative and observability tests for security-sensitive runtime contracts. Relevant areas include network policy enforcement, manifest/config generation, artifact-manifest validation, logs emitted by self-test/live validation, and diagnostics around denied operations. Cover no-net mode, hostile guest attempts, allowlisted-host-only egress, UDP/443 denial, HTTPS CONNECT/MITM expectations if applicable, mount read-only enforcement, config secret leakage, stale or mismatched manifests, and structured artifact/log collection on failure.

## Design

Use offline unit tests for policy decisions and manifest validation. Use live scenarios for guest-originated denied traffic and read-only mount enforcement. Log assertions should be robust: match stable event names/fields rather than entire prose where possible.

## Acceptance Criteria

Security policy tests prove denied network and filesystem operations fail closed. Manifest/config validation rejects stale or inconsistent artifacts before boot. Live validation captures enough logs/artifacts to diagnose the failing phase without leaking secrets or sensitive config values.


## Notes

**2026-05-15T14:45:28Z**

Started after wra-a0bu closure. Survey: much of the security surface already has coverage from earlier tickets: no-net/hostile live scenarios, DNS deny diagnostics, Docker no-net denial, config key hidden in self-test and hostile symlink attempts, stale artifact source-hash preflight, runtime manifest validation tests, event-log secret-redaction test, and induced self-test failure artifact summary. Remaining work should consolidate acceptance evidence and add any missing stable log/diagnostic assertions rather than create another parallel validation path.

**2026-05-15T14:48:32Z**

Consolidated security/diagnostic acceptance evidence without adding a parallel validation path: offline coverage includes policy fail-closed tests for no-net/published-port conflicts, denied TCP/UDP/DNS decisions, runtime manifest validation and secret-redaction assertions, hostile payload script assertions for config key denial/symlink attempts, artifact source-hash preflight tests, and failure artifact-summary tests. Live matrix evidence on 2026-05-15: ./vm-frontend/validate.sh live-full passed, exercising live-smoke, live-hostile/no-net, live-payload, live-dns allow+deny, live-docker allow/deny+published port, and live-fs repeated run-dir composed-fs checks. Stable diagnostics observed include artifact summaries naming run_dir/state/qemu/console/vmnet logs, dns-deny-ok EAI_AGAIN, docker-deny-ok policy=deny, fs-live-ok, ca-key-hidden-ok, and hostile-ok; event-log unit coverage asserts representative denial/failure event names and absence of mitm-ca.key/private-key material.
