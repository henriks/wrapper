---
id: wra-01tu
status: closed
deps: []
links: [wra-oio0]
created: 2026-05-18T05:36:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-9m5h
tags: [cleanup, launch, cli]
---
# Replace wrapper launch argument roundtrip with typed launch request

The wrapper side builds launch arguments as Vec<String> and then routes them back through launch CLI parsing. That fake CLI boundary exists because implementation evolved iteratively, not because the application needs it. Replace the string roundtrip with a typed launch request shared by wrapper and launch code, and keep real CLI parsing only at the external command-line boundary.

## Design

Inspect wrapper launch plumbing, launch_cli parsing, WrapperLaunchFlag-style helpers, and push_launch_value-style argument construction. Introduce the smallest typed request/config structure needed to call launch code directly. Delete the helper functions and tests that only prove the string roundtrip works.

## Acceptance Criteria

Internal wrapper-to-launch calls do not construct argv strings; external CLI behavior is still parsed at the actual CLI boundary; obsolete adapter helpers are deleted; tests cover typed request construction and one CLI-to-request conversion; required validation is recorded before close.


## Notes

**2026-05-18T06:55:50Z**

Started audit. Wrapper still builds Vec<String> launch_args via WrapperLaunchFlag/push_launch_value/push_launch_flag, prepends a fake launch subcommand, and calls run_launch(&launch_args[1..]). Tests in vm-frontend/src/main.rs assert launch_args contents and sometimes parse them back with frontend_config_from_args(&args.launch_args). Implementation should replace WrapperArgs.launch_args with a typed launch request/policy structure and keep frontend_config_from_args only at external CLI boundaries.

**2026-05-18T07:08:09Z**

Implemented typed wrapper launch path: added FrontendLaunchRequest in launch_cli, made CLI parsing return a typed request before config materialization, added run_launch_request/run_launch_request_async, and changed wrapper parsing to mutate WrapperArgs.launch directly instead of constructing fake launch argv. Removed wrapper production helpers for launch argv mutation (WrapperLaunchFlag/push/upsert/has) and updated wrapper tests to assert typed request fields. Validation passed: ./vm-frontend/validate.sh required (includes live-smoke and live-setup-tools), full output /tmp/pi-bash-68c1e69d91499381.log.
