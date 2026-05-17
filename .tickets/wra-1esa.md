---
id: wra-1esa
status: open
deps: [wra-09v9]
links: []
created: 2026-05-17T10:18:48Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, cli, deletion]
---
# Option 1: remove non-config CLI compatibility surface

Remove compatibility-only command line behavior. The review called out legacy argv routing in vm-frontend/src/main.rs, the agentvm-frontend binary alias and wrap compatibility path, removed-flag handling, and stringly launch-argv rewriting. Compatibility only matters for .sandbox/config.json.

## Design

After the CLI/config split, delete old command aliases and removed flag shims unless they represent current product behavior. Prefer typed clap derive or typed command structs over building Vec<String> launch arguments. Do not preserve command line behavior solely for compatibility.

## Acceptance Criteria

The supported CLI surface is explicit in clap help and tests, obsolete aliases and removed flags are gone, wrapper-to-launch conversion is typed rather than stringly, and .sandbox/config.json compatibility remains intact.

