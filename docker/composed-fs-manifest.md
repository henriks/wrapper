# Composed Filesystem Manifest Design

Ticket: `wra-a9je`

Date: 2026-05-12

## Outcome

The composed filesystem uses two manifests:

- host manifest: consumed only by the composed backend
- guest bind manifest: consumed by guest init after mounting the composed
  export

The host manifest is the authority for filesystem policy. The guest bind
manifest is only reconstruction metadata telling guest init which paths to
bind into final locations.

## Runtime Paths

V1 paths under `.sandbox/docker-vm/run/`:

```text
.sandbox/docker-vm/run/
  composed-fs-manifest.json
  guest-config/
    composed-binds.json
```

V1 keeps the existing tiny config share for `composed-binds.json`. Removing the
config share is a later optimization and should happen only after a replacement
boot-config mechanism is documented.

The composed backend receives:

```text
--manifest .sandbox/docker-vm/run/composed-fs-manifest.json
--socket-path .sandbox/docker-vm/run/virtiofs.sock
--tag agentvm
```

Guest init mounts the composed export at:

```text
/run/agentvm-host
```

and reads bind instructions from:

```text
/run/agentvm-config/composed-binds.json
```

## Host Manifest Schema

Schema version: `1`

Example:

```json
{
  "schema_version": 1,
  "export_tag": "agentvm",
  "created_by": "sandbox-wrap",
  "mounts": [
    {
      "id": "m0001_workspace",
      "guest_path": "/home/user/project",
      "host_path": "/home/user/project",
      "kind": "dir",
      "access": "rw",
      "source_class": "workspace",
      "required": true,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0002_codex",
      "guest_path": "/home/user/project/.sandbox/home/.codex",
      "host_path": "/home/user/.codex",
      "kind": "dir",
      "access": "rw",
      "source_class": "tool-state",
      "required": false,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0003_docker_config",
      "guest_path": "/home/user/project/.sandbox/home/.docker",
      "host_path": "/home/user/.docker",
      "kind": "dir",
      "access": "rw",
      "source_class": "tool-state",
      "required": false,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0004_gh_config",
      "guest_path": "/home/user/project/.sandbox/home/.config/gh",
      "host_path": "/home/user/.config/gh",
      "kind": "dir",
      "access": "ro",
      "source_class": "auth-config",
      "required": false,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0005_certs",
      "guest_path": "/etc/ssl/certs",
      "host_path": "/etc/ssl/certs",
      "kind": "dir",
      "access": "ro",
      "source_class": "system-ro",
      "required": false,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0006_hosts",
      "guest_path": "/etc/hosts",
      "host_path": "/etc/hosts",
      "kind": "file",
      "access": "ro",
      "source_class": "system-ro",
      "required": false,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0007_user_ro",
      "guest_path": "/opt/reference",
      "host_path": "/opt/reference",
      "kind": "dir",
      "access": "ro",
      "source_class": "user-ro",
      "required": true,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    },
    {
      "id": "m0008_user_rw",
      "guest_path": "/tmp/shared-output",
      "host_path": "/tmp/shared-output",
      "kind": "dir",
      "access": "rw",
      "source_class": "user-rw",
      "required": true,
      "bind": true,
      "metadata": {
        "uid_gid": "host",
        "permissions": "host"
      }
    }
  ],
  "synthetic": {
    "uid": 0,
    "gid": 0,
    "dir_mode": "0555"
  },
  "protected_guest_paths": [
    "/run",
    "/proc",
    "/sys",
    "/dev",
    "/var/lib/docker"
  ]
}
```

Required mount fields:

- `id`: stable unique mount id for this manifest
- `guest_path`: absolute normalized guest path exposed inside the composed
  namespace
- `host_path`: absolute normalized host source path
- `kind`: `dir` or `file`
- `access`: `rw` or `ro`
- `source_class`: `workspace`, `tool-state`, `auth-config`, `system-ro`,
  `user-ro`, or `user-rw`
- `required`: whether missing source fails startup
- `bind`: whether guest init should bind this path into its final location
- `metadata.uid_gid`: `host` for v1
- `metadata.permissions`: `host` for v1

Optional future fields should be ignored only when the schema explicitly marks
them optional. Unknown fields in schema version `1` should fail validation to
avoid silently ignoring security policy.

## Guest Bind Manifest Schema

Schema version: `1`

Example:

```json
{
  "schema_version": 1,
  "composed_mountpoint": "/run/agentvm-host",
  "entries": [
    {
      "mount_id": "m0001_workspace",
      "kind": "dir",
      "source": "/run/agentvm-host/home/user/project",
      "target": "/home/user/project",
      "required": true,
      "create_parent": true
    },
    {
      "mount_id": "m0006_hosts",
      "kind": "file",
      "source": "/run/agentvm-host/etc/hosts",
      "target": "/etc/hosts",
      "required": false,
      "create_parent": true
    }
  ]
}
```

Guest init behavior:

- mount `agentvm` at `composed_mountpoint`
- for each `dir` entry, create `target` and bind-mount `source` to `target`
- for each `file` entry, create `dirname(target)`, create an empty placeholder
  file if needed, then bind-mount `source` to `target`
- fail boot on required bind failure
- log and skip optional bind failure
- never use the bind manifest to grant permissions; permissions come from the
  host manifest and backend

The bind manifest should be derived from the validated host manifest, not
separately authored.

## Source Generation Rules

Workspace:
- required
- `guest_path` is the resolved project absolute path
- `host_path` is the resolved project absolute path
- `access` is `rw`
- `source_class` is `workspace`

Tool state:
- optional unless the current wrapper treats the source as required
- host paths come from `TOOLS[tool]["host_home_mounts"]`
- guest paths go under the project-local guest home:
  `<project>/.sandbox/home/<relative-tool-path>`
- `access` is `rw`
- `source_class` is `tool-state`

Docker client config:
- optional
- host path: `$HOME/.docker`
- guest path: `<project>/.sandbox/home/.docker`
- `access` is `rw`
- `source_class` is `tool-state`

GitHub config:
- optional and only generated for `--gh`
- host path: `$HOME/.config/gh`
- guest path: `<project>/.sandbox/home/.config/gh`
- `access` is `ro`
- `source_class` is `auth-config`

System readonly paths:
- optional unless later code marks one as required
- examples: `/etc/ssl/certs`, `/etc/hosts`
- `access` is `ro`
- `source_class` is `system-ro`

User paths:
- `--ro PATH` becomes `source_class=user-ro`, `access=ro`
- `--rw PATH` becomes `source_class=user-rw`, `access=rw`
- host and guest paths both use the normalized absolute path
- missing paths fail validation for v1 because the user explicitly requested
  them

## Validation Rules

Validate before starting the backend:

- `schema_version` is supported
- every mount has a unique `id`
- every `guest_path` is absolute and normalized
- every `host_path` is absolute and normalized
- `guest_path` must not contain empty components, `.`, or `..`
- `host_path` must exist for required mounts
- optional missing mounts are omitted from the final manifest
- `kind` must match the host source type
- source class and access mode must be valid
- `rw` mounts require host write access by the current user
- `ro` mounts require host read/search access by the current user
- no mount may target a protected guest path unless explicitly allowlisted
- no file mount may have manifest children beneath it
- no synthetic parent may conflict with a file mount

Protected guest paths for v1:

- `/proc`
- `/sys`
- `/dev`
- `/run`
- `/tmp`
- `/var/lib/docker`
- `/var/run/docker.sock`
- `/var/run`

The project workspace may contain `.sandbox/docker-vm/run` because guest logs
are mirrored there, but the manifest must not expose host runtime sockets or
control files as separate user-overridable mounts.

## Conflict And Overlap Rules

Terminology:

- exact duplicate: same normalized `guest_path`
- ancestor overlap: one guest path is a strict ancestor of another
- descendant override: a more specific mount nested under a broader mount

Exact duplicates:

- same `guest_path`, same `host_path`, same `kind`, same `access`: coalesce
  and merge source-class metadata only for diagnostics
- same `guest_path` but different `kind`: fail
- same `guest_path` but different `host_path`: fail unless an explicit user
  mount overrides an optional implicit mount
- same `guest_path` but different `access`: fail unless an explicit user mount
  overrides an optional implicit mount

Override precedence for exact duplicate conflicts:

1. protected internal paths cannot be overridden
2. required workspace cannot be overridden
3. explicit user mounts (`user-ro`, `user-rw`) override optional implicit
   `tool-state`, `auth-config`, or `system-ro`
4. otherwise fail with a clear error

Ancestor overlaps:

- directory ancestor plus directory descendant is allowed
- directory ancestor plus file descendant is allowed
- file ancestor plus any descendant fails
- more specific descendant policy overrides broader ancestor policy
- nested readonly inside writable must remain readonly
- nested writable inside readonly is allowed only for explicit `user-rw` or
  required `workspace`; otherwise fail because it weakens readonly expectations
- cross-boundary rename/link must fail with `EXDEV`

Examples:

```text
/home/user/project                    rw workspace
/home/user/project/.git               ro future-system-policy
```

Allowed. `.git` remains readonly even when reached through the writable
workspace.

```text
/home/user                            rw user-rw
/home/user/.config/gh                 ro auth-config
```

Allowed. GitHub config remains readonly.

```text
/etc                                  ro system-ro
/etc/hosts                            ro system-ro file
```

Allowed. `/etc/hosts` is a file mount listed by synthetic or host-backed
parent readdir as appropriate.

```text
/etc/hosts                            ro file
/etc/hosts/foo                        rw dir
```

Rejected. A file mount cannot have descendants.

## Backend Interpretation

The backend receives only a validated manifest.

Required backend behavior:

- build synthetic parent directories for every mount path
- materialize mount roots from `mounts`
- enforce `access` on every operation, including nested boundaries
- use `source_class` only for diagnostics and policy decisions already encoded
  in validation
- expose file mounts in parent `readdir`
- reject mutation of synthetic-only directories
- treat the host manifest as authoritative

The backend must still defend against malformed manifests because the guest is
untrusted and the host wrapper may have bugs.

## Guest Init Interpretation

Guest init reads only `composed-binds.json`.

Required guest behavior:

- mount the composed export before processing entries
- process bind entries in parent-before-child order
- create missing parent directories for targets when `create_parent=true`
- create file placeholders only for `kind=file`
- bind directory entries with `mount --bind source target`
- bind file entries with `mount --bind source target`
- fail required bind errors
- log optional bind errors and continue

Guest init must not interpret host paths or source classes.

## Security Notes

- Manifest validation is not a substitute for backend enforcement.
- Guest bind manifest is not trusted for permissions.
- User-specified mounts must not override protected runtime/control paths.
- The composed backend should never expose the host manifest inside the guest.
- The guest may see `composed-binds.json` via the config share; it contains
  guest paths only and should not include host source paths.
