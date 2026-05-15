# `.sandbox/config.json` Format

`.sandbox/config.json` is the durable, compatibility-sensitive project configuration for `agentvm`. The wrapper reads it from the selected project directory before each launch. CLI flags are one-run overrides unless a setup/config-editing flow explicitly writes this file.

## Compatibility Rules

- `schema_version` is required.
- The current written format is `schema_version: 2`.
- Readers still accept the legacy `schema_version: 1` shape and migrate it in memory. Do not remove config-file compatibility without an explicit migration plan.
- Do not repurpose existing fields. Add new optional fields with safe defaults when extending the format.
- Keep this document, `WrapperSandboxConfig` in `vm-frontend/src/main.rs`, and tests in sync whenever the config format changes.

## Version 2 Shape

All fields except `schema_version` have defaults when omitted.

```json
{
  "schema_version": 2,
  "setup_tool": "codex",
  "default_command": {
    "command": "codex",
    "args": []
  },
  "network": {
    "mode": "public",
    "allowed_domains": [],
    "allowed_hosts": [],
    "allowed_ips": []
  },
  "auth": {
    "github": false,
    "aws_profile": null
  },
  "shares": [
    {
      "host_path": "/home/USER/.codex",
      "guest_path": "/home/USER/.codex",
      "access": "rw",
      "required": false,
      "shadows": ["tmp"]
    }
  ],
  "published_ports": []
}
```

### `schema_version`

Required integer. Must be `2` for the current format.

### `setup_tool`

Optional string or `null`. Supported values:

- `"codex"`: use the Codex setup recipe. If `default_command.command` is `"codex"`, launch through the recipe bootstrap script for `@openai/codex`.
- `"pi"`: use the Pi setup recipe. If `default_command.command` is `"pi"`, launch through the recipe bootstrap script for `@mariozechner/pi-coding-agent`.

`setup_tool` selects recipe bootstrap behavior. Setup writes explicit `shares` for tool state needs; runtime mount composition remains generic.

### `default_command`

Object with:

- `command`: non-empty command name.
- `args`: array of string arguments.

For config compatibility, the reader also accepts a string value such as `"codex"` and treats it as `{ "command": "codex", "args": [] }`. Writers should emit the object form.

Arguments after `agentvm --` override this command for one launch and do not modify the file. Configured/default payloads run with the selected project directory as their current working directory unless a launch-level `--payload-cwd` override is supplied.

### `network`

Object with:

- `mode`: one of `"public"`, `"none"`, or `"allowlist"`.
- `allowed_domains`: domain allowlist entries used when mode is `"allowlist"`.
- `allowed_hosts`: host/domain allowlist entries, currently translated like `allowed_domains`.
- `allowed_ips`: IP or CIDR allowlist entries.

`"public"` allows public internet egress while still denying protected/private ranges by policy. `"none"` denies guest egress. CLI network flags override this section for one launch.

### `auth`

Object with:

- `github`: when true, share `~/.config/gh` read-only and forward a host `gh auth token` result when available.
- `aws_profile`: string profile name or `null`. When set, obtain AWS credentials for that profile on the host and inject only the resolved credential environment into the guest.

### `shares`

Array of explicit host path shares.

```json
{
  "host_path": "/absolute/or/relative/host/path",
  "guest_path": "/absolute/guest/path",
  "access": "rw",
  "required": true,
  "shadows": ["tmp"]
}
```

Fields:

- `host_path`: required non-empty path. Relative paths are resolved from the wrapper process current working directory at launch time; absolute paths are recommended for durable project config.
- `guest_path`: optional non-empty path. If omitted, the guest path is the resolved `host_path`.
- `access`: `"ro"` or `"rw"`.
- `required`: boolean, default `true`. Missing required sources fail launch; missing optional sources are skipped.
- `shadows`: optional array of child shadow mount paths. Allowed on both `"ro"` and `"rw"` shares.

#### Share Shadows

A shadow replaces a guest-visible child path under a configured share with read-write project-local backing storage under `.sandbox/root/<full guest shadow path>`. The parent host share keeps its configured access for all other paths.

Each `shadows[]` string is relative to the share's guest root. It must be a normalized relative path: no empty value, no absolute path, no `..`, and no `=`.

Example: writable host `~/.codex`, but project-local guest `~/.codex/tmp`:

```json
{
  "schema_version": 2,
  "setup_tool": "codex",
  "default_command": { "command": "codex", "args": [] },
  "network": { "mode": "public", "allowed_domains": [], "allowed_hosts": [], "allowed_ips": [] },
  "auth": { "github": false, "aws_profile": null },
  "shares": [
    {
      "host_path": "/home/USER/.codex",
      "guest_path": "/home/USER/.codex",
      "access": "rw",
      "required": true,
      "shadows": ["tmp"]
    }
  ],
  "published_ports": []
}
```

This is generic host-directory mount configuration, not a Codex-specific policy.

### `published_ports`

Array of host-to-guest TCP port mappings:

```json
{ "host": 18080, "guest": 8080 }
```

Published ports are not applied when effective network mode is `"none"`.

## Legacy Version 1

The legacy shape is read for compatibility only:

```json
{
  "schema_version": 1,
  "codex_enabled": true,
  "default_command": "codex"
}
```

At read time, `codex_enabled: true` maps to the Codex setup defaults, then `default_command` is applied. New writes use version 2.
