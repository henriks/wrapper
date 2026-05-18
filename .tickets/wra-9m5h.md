---
id: wra-9m5h
status: closed
deps: []
links: [wra-piqm, wra-xcvq, wra-emj5]
created: 2026-05-18T05:36:19Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [cleanup, technical-debt, agentvm]
---
# Cleanup: remove transitional code and tighten AgentVM architecture

Broad cleanup epic for removing transitional code, unnecessary adapters, duplicated launch/runtime paths, stale process artifacts, and other iteration leftovers discovered during the May 2026 duplicate scans. This is broader than wra-piqm: stability and performance remain end goals, but the primary constraint is minimum necessary code. Favor deleting or merging paths over adding wrappers, compatibility layers, or parallel implementations. Compatibility is only a requirement for .sandbox/config.json.

## Design

Rules for this epic: every ticket should name the code it intends to remove or merge; new abstractions must replace existing code rather than sit beside it; no non-config backwards-compatibility shims; keep Tokio at useful IO/orchestration boundaries without async-all-the-way-down; preserve or improve validation coverage while shrinking production code.

## Acceptance Criteria

All child tickets are implemented or explicitly superseded; applicable tickets from wra-piqm and wra-xcvq are linked and annotated with cleanup direction; no open cleanup work asks for adding a parallel implementation where deletion or consolidation is the goal; dependency cycles are absent; code-changing tickets record required validation or the live-environment limitation before close.


## Notes

**2026-05-18T06:40:08Z**

Iteration 1 inventory: no dependency cycles. wra-o75s is blocked by wra-vd8g and wra-y335; wra-oio0 is blocked by wra-7t63, wra-01tu, and wra-d0q5; wra-ynx7 and wra-8xsb are blocked by correctness tickets. Completed wra-6uak by deleting tracked old/, stale .ralph artifacts, and project-root .codex, adding root-scoped ignores for /.ralph/ and /.codex, and validating with ./vm-frontend/validate.sh required.

**2026-05-18T06:48:30Z**

Iteration 2 completed wra-wv0w. Runtime network policy now carries typed IPv4 ranges (allow_ip_ranges/deny_ip_ranges) parsed at launch/vmnet CLI boundaries; tcp_gateway no longer reparses IP/CIDR strings. Docs/tests/fuzz updated, and ./vm-frontend/validate.sh required passed with live-smoke and live-setup-tools.

**2026-05-18T06:55:37Z**

Iteration 3 completed wra-d0q5. Self-test harness moved behind validation-self-test feature and dedicated agentvm-self-test validation binary; production agentvm/agentvm-frontend reject self-test. validate.sh and validation-workflow.md updated. Required validation passed including live-smoke and live-setup-tools.

**2026-05-18T07:08:29Z**

Iteration 5 completed wra-01tu. Wrapper launch construction is now typed via FrontendLaunchRequest and run_launch_request; wrapper string argv roundtrip and helper scaffolding were removed. Required validation passed including live-smoke/live-setup-tools: /tmp/pi-bash-68c1e69d91499381.log.

**2026-05-18T07:15:07Z**

Iteration 6 reflection and wra-2ku5 first pass: dynamic project/network/HTTP-smoke metadata moved from kernel cmdline to guest-config/launch.json on agentvm-config; guest cmdline parsing removed; payload service selection moved to /etc/agentvm.env. Targeted offline/guest validations passed, but wra-2ku5 remains open pending host rebuild of root-owned docker/out artifacts and ./vm-frontend/validate.sh required.

**2026-05-18T07:17:05Z**

Iteration 7: confirmed dependency status (no cycles; wra-2ku5 still in_progress; wra-oio0 still blocked by wra-7t63). docs/fmt/fuzz-check passed. live-smoke failed before boot due expected stale appliance source hash and requires sudo ./docker/build-appliance.sh, which cannot be run non-interactively in this harness. Added note to wra-7t63 as the remaining blocker for deleting synchronous launch path.

**2026-05-18T07:19:25Z**

Iteration 8: hardened wra-2ku5 with prepare-level regression coverage for launch.json versus kernel cmdline metadata, then ran ./vm-frontend/validate.sh fast successfully. wra-2ku5 remains in_progress only due pending sudo appliance rebuild and required live validation.

**2026-05-18T07:20:44Z**

Iteration 9: extended wra-2ku5 live self-test payload to validate launch.json inside the guest and absence of agentvm_project= in /proc/cmdline. Targeted validation and guest-services passed; required validation still awaits host artifact rebuild.

**2026-05-18T07:21:57Z**

Iteration 10: documentation cleanup for wra-2ku5 in vm-frontend/README.md, annotated linked wra-o75s with payload-service selector cleanup guidance, and rechecked readiness/dependency cycles. docs/fmt/git diff checks passed; no child ticket became newly unblocked.

**2026-05-18T07:23:18Z**

Iteration 11 reflection: recorded progress/blockers/next priorities in .ralph. Annotated linked epics wra-piqm and wra-xcvq with cleanup implications from typed launch config/string-protocol removal. Rechecked ready/blocked tickets and dependency cycles; no cleanup child became newly unblocked and wra-2ku5 remains waiting for host appliance rebuild plus required validation.

**2026-05-18T07:28:39Z**

Iteration 11 continued after user rebuilt appliance: required validation initially revealed bug wra-80tf (live-setup-tools stale agentvm binary path); fixed validate.sh to use target/debug/agentvm, closed wra-80tf, reran ./vm-frontend/validate.sh required successfully, and closed wra-2ku5. Passing required validation log: /tmp/pi-bash-2be9ae6dec1fa6b4.log.

**2026-05-18T07:30:27Z**

Iteration 12: all remaining cleanup children are blocked, so started prerequisite wra-7t63. Audit found supervisor_control supports only status/subscribe/shutdown; LaunchSupervisor has no payload session task; run_launch_request_async falls back to synchronous launch whenever payload/TUI is present. Added notes to wra-7t63 and wra-oio0. No source changes for wra-7t63 yet.

**2026-05-18T07:41:21Z**

Iteration 14: continued prerequisite wra-7t63. Plain async payload launches now use the async supervisor-owned frontend, discover payload endpoint through supervisor control, run payload client, flush, and request supervisor shutdown over control socket. TUI payload viewport still uses the sync branch, so wra-oio0 remains blocked. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins run_launch -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_plan_with_policy_carries_effective_vmnet_policy -- --nocapture; cargo fmt; git diff --check. Also reran tk dep cycle (no cycles).

**2026-05-18T07:45:59Z**

Iteration 15: continued prerequisite wra-7t63. TUI payload launches through production async/wrapper dispatch now use the supervisor-control payload endpoint and control-socket shutdown; production agentvm wrapper dispatch calls run_wrapper_async. Sync run_wrapper/run_launch_request remain only behind the legacy sync run_cli/test boundary and are now the likely wra-oio0 deletion target. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_control -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features supervisor_plan_with_policy_carries_effective_vmnet_policy -- --nocapture; cargo fmt; git diff --check; tk dep cycle.

**2026-05-18T07:52:02Z**

Iteration 16 reflection checkpoint recorded. Required validation for wra-7t63 initially exposed async payload launch failing to report early QEMU/frontend errors before payload readiness timeout; fixed by racing control endpoint/readiness waits against the launch task and validating missing absolute QEMU paths. Targeted tui_terminal regression passed, then ./vm-frontend/validate.sh required passed. Log: /tmp/pi-bash-5f61aeae608bdd10.log.

**2026-05-18T07:54:36Z**

Iteration 17: closed prerequisite wra-7t63 using required validation log /tmp/pi-bash-5f61aeae608bdd10.log, then started child cleanup ticket wra-oio0. Audit found remaining sync launch ownership in launch.rs RunningFrontend/start_frontend_with_policy, launch_cli.rs sync run_launch/run_launch_request, wrapper.rs sync run_wrapper for legacy run_cli tests, and validation-only self_test.rs. Next slice should migrate agentvm-self-test to async supervisor/control shutdown before deleting the sync launch implementation. tk dep cycle still reports no cycles.

**2026-05-18T07:57:47Z**

Iteration 18: migrated validation-only agentvm-self-test to async supervisor/control shutdown for wra-oio0. run_self_test is async, starts run_frontend_until_qemu_exit_with_policy_and_timeout_async, races payload readiness against launch failure, flushes, and requests supervisor shutdown via SupervisorControlClient. Remaining start_frontend_with_policy/RunningFrontend callers are obsolete sync helpers/tests. Added note to wra-rie1 that wra-oio0 should supersede old partial-startup hardening by deletion. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline --features validation-self-test parses_self_test_config_defaults_and_options -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture; git diff --check.

**2026-05-18T08:04:53Z**

Iteration 19: continued wra-oio0 and deleted the obsolete synchronous launch path. Removed RunningFrontend/start_frontend_with_policy/sync run_frontend_until_qemu_exit*, sync launch_cli run_launch/run_launch_request, sync wrapper run_wrapper, and related tests. Legacy sync run_cli now delegates launch/wrapper through Tokio runtime to async dispatch only for tests. rg confirms no remaining references in vm-frontend/src/tests. Validation: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --lib launch::tests:: -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features --bins -- --nocapture (/tmp/iter19-bins.log); git diff --check. Required validation still needed before closing wra-oio0.

**2026-05-18T08:10:32Z**

Iteration 20: ran ./vm-frontend/validate.sh required after wra-oio0 sync launch deletion; passed including live-smoke and live-setup-tools. Log: /tmp/pi-bash-3bda8d0d0854e2f8.log. Closed wra-oio0 and closed linked wra-rie1 as superseded by deletion of RunningFrontend/sync launch path.

**2026-05-18T08:12:10Z**

Iteration 21 reflection: major cleanup wins are closed (legacy artifacts, typed network policy, validation-only self-test, typed wrapper launch, structured guest config, sync launch deletion). Remaining children wra-o75s/wra-ynx7/wra-8xsb are still blocked by prerequisite parity/correctness work, so do not force cleanup there yet. Added blocker-direction notes to those three tickets. tk ready/blocked and tk dep cycle confirm no cycles and no newly unblocked cleanup child.

**2026-05-18T09:07:41Z**

Iteration 35 cleanup progress: closed vmnet prerequisite `wra-57z4`, closed/superseded `wra-jenv` to avoid reintroducing obsolete ByteIo workers, and started direct child `wra-ynx7`. First `wra-ynx7` code slice quarantined/deleted sync vmnet frame-pump scaffolding and passed required validation (`/tmp/pi-bash-42d47e9f4accf1fb.log`).

**2026-05-18T09:17:51Z**

Iteration 36 reflection/closure: completed and closed direct child `wra-ynx7`. The vmnet cleanup now has no stale `allow(dead_code)` islands in `vmnet_runtime`, `vmnet_service_io`, `tcp_proxy`, or `vmnet_gateway`; remaining sync frame-pump helpers are private `#[cfg(test)]`; supervisor launch calls the async vmnet boundary directly; required validation passed at `/tmp/pi-bash-6e47da3ee3d2cc40.log`. Only direct child still open is `wra-8xsb`, blocked by composed-fs correctness prerequisites.

**2026-05-18T09:26:53Z**

Iteration 37: direct cleanup child wra-8xsb remains blocked, so completed prerequisite wra-bcvj. composed-fs lock bridge now keys locks by host dev/ino plus guest owner across handles; added two-handle regression and multi-handle lock proptest coverage. Required validation passed at /tmp/pi-bash-de557005de254a02.log. Remaining wra-8xsb blockers: wra-zlsn, wra-qdte, wra-rleu, wra-3gdr, wra-fsv6.

**2026-05-18T09:33:53Z**

Iteration 38: completed composed-fs prerequisite wra-qdte. Host-relative traversal now requires openat2 and fails closed with EOPNOTSUPP on ENOSYS/EINVAL instead of using the weaker openat fallback; added forced-unavailable and create-mode regression tests, docs update, and required validation evidence /tmp/pi-bash-8bea82967fc804e6.log. wra-rleu is now unblocked; wra-8xsb remains blocked by wra-zlsn, wra-rleu, wra-3gdr, and wra-fsv6.

**2026-05-18T09:38:37Z**

Iteration 39: completed composed-fs prerequisite wra-rleu. readlink/access now use confined parent resolution through with_parent_dir/open_beneath_with_mode, with cached-parent symlink replacement regressions. Required validation passed at /tmp/pi-bash-970b7b9667685e04.log. Remaining wra-8xsb blockers: wra-zlsn, wra-3gdr, wra-fsv6.

**2026-05-18T09:47:29Z**

Iteration 40: completed composed-fs prerequisite wra-zlsn. Host nodes now retain dev/ino identity and validate cached paths before use; replacement files get distinct backend inodes and stale old inodes fail ESTALE. Required validation passed at /tmp/pi-bash-5e4e798ccd25797a.log. Remaining wra-8xsb blockers: wra-3gdr and wra-fsv6.

**2026-05-18T09:48:51Z**

Iteration 41 reflection: all direct cleanup children except wra-8xsb are closed. Composed-fs prerequisites wra-bcvj, wra-qdte, wra-rleu, and wra-zlsn are now closed with required validation. Remaining wra-8xsb blockers are wra-3gdr and wra-fsv6; continue prerequisite-first before structural composed-fs consolidation.

**2026-05-18T10:14:06Z**

Iteration 44 completion: all direct child tickets are now closed, including final composed-fs cleanup child wra-8xsb. The final cleanup slice extracted the composed-fs host_ops boundary while preserving bounded blocking request execution; ./vm-frontend/validate.sh required passed at /tmp/pi-bash-993c468a1e57703b.log. Dependency cycle check reports no cycles.

**2026-05-18T10:37:42Z**

Follow-up duplicate scan created new cleanup epic wra-emj5 rather than reopening this closed epic. wra-emj5 owns residual issues that survived the first cleanup wave, including payload sync/async split, sync CLI/vmnet runtime island, Python guest Docker bridge, /workspace alias, and further module/test harness consolidation.
