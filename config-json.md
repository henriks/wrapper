# Sandbox configuration

`.sandbox/config.json` is optional, local to the project, and preserved by
`codex-wrap --reset`. It is excluded from every guest directory export along with `.sandbox/`,
using mounts enforced in a private host namespace. Host aliases of the control
state are rejected as directory-share sources.
Run `codex-wrap --help-config` to print this reference without starting a VM.
Unknown fields, invalid modes and malformed domain rules are errors.

```json
{
  "network": {
    "mode": "proxy",
    "deny": ["example.com", "*.example.com"]
  }
}
```

The default, including when no file exists, is CONNECT proxy access restricted
by the built-in domain allowlist. `network.mode` accepts `proxy` (default),
`off` (no external access), or `direct` (raw networking without the proxy).
Broader access is always an explicit choice through CLI options or configuration.

The default allowlist contains these exact hosts (no wildcard domains):

| Purpose | Hosts |
| --- | --- |
| Codex/OpenAI | `api.openai.com`, `auth.openai.com`, `chatgpt.com` |
| mise, Node, npm | `mise.jdx.dev`, `mise-versions.jdx.dev`, `nodejs.org`, `registry.npmjs.org` |
| GitHub source and downloads | `github.com`, `api.github.com`, `raw.githubusercontent.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com` |
| GitHub artifact verification | `tuf-repo-cdn.sigstore.dev` |
| Python packages | `pypi.org`, `files.pythonhosted.org` |
| AWS CLI bootstrap | `awscli.amazonaws.com` |
| Docker Hub | `auth.docker.io`, `registry-1.docker.io`, `production.cloudfront.docker.com`, `production.cloudflare.docker.com`, `docker-images-prod.6aa30f8b08e16409b46e0173d6de2f56.r2.cloudflarestorage.com` |

The defaults cover tool bootstrap and common development downloads, not every
project's package registries or external APIs. Credential options such as `--aws`
do not broaden the allowlist; add required API hosts explicitly. These are shared services;
allowing a hostname permits all content served there, including user content.
See the documented [Codex endpoints](https://openai.com/index/unrolling-the-codex-agent-loop/)
and [Docker Hub endpoints](https://docs.docker.com/desktop/enterprise/allow-list/).

In proxy mode, optional `allow` and `deny` arrays contain exact domain names or
`*.domain` rules. A wildcard matches subdomains, including deeper levels, but
not the base domain. Rules are case insensitive and normalized with IDNA. Deny
takes precedence. Omitted `allow` uses the built-in list; `allow: []` denies
everything; explicit `allow: null` permits any public destination. `deny` defaults
to an empty list. Private/non-global addresses are always denied in proxy mode,
regardless of domain rules. Both lists require proxy mode.

CLI overrides (mutually exclusive):

- `--unfiltered-net`: CONNECT to any public destination, ignoring both domain lists.
  IP checks and the port-443 restriction remain enforced.
- `--raw-net`: direct VM networking without proxy enforcement or proxy variables.
- `--no-net`: no external access.

Repeatable `--allow-domain DOMAIN` and `--deny-domain DOMAIN` select filtered proxy
mode and replace only their corresponding configured list. They cannot be combined
with the mode switches above. An empty string clears a list: `--deny-domain ''`.
Clearing `allow` produces an empty allowlist (deny all).

CLI options never edit `config.json`. To persist public unfiltered CONNECT access,
set `{"network":{"mode":"proxy","allow":null}}`; to persist raw access, set
`{"network":{"mode":"direct"}}`. To return to filtered defaults, omit `allow`
and use `mode: "proxy"`, or remove the network configuration.

A running shared VM uses a policy snapshot until its last session ends. Joining
sessions inherit that snapshot, including CLI overrides; explicit CLI network
options must agree with it. Config edits take effect at the next VM startup,
not while sessions are running. Generated `.sandbox/launch.json` is separate
runtime state; it includes the effective proxy policy and socket mapping.

Proxy mode accepts only `CONNECT host:443` and forwards the resulting TCP
stream unchanged, with bounded buffers and backpressure. Ordinary HTTP proxy
requests are rejected; use HTTPS endpoints. The proxy checks the requested
hostname, not tunneled TLS content. Every DNS answer must be public; connections
use the checked sockaddr without resolving it again. There is no TLS interception
or new certificate authority.
The HTTP header limit is 64 KiB, request-header and DNS/connect deadlines are
10 seconds, and the host accepts at most 128 active proxy connections.

The wrapper disables VM outbound networking and mounts a private host Unix
socket into the guest. A guest TCP relay exposes the proxy on port 4750;
applications and dockerd receive HTTP(S)_PROXY variables. Direct TCP/UDP, DNS,
SSH and QUIC egress are unavailable; tools must support HTTP CONNECT. Local
VM/container communication and explicitly published inbound ports remain local
facilities, not arbitrary outbound Internet access. Published ports can still
communicate with their inbound clients.

Container applications must also receive proxy settings: with Docker's default
bridge, use `http://172.17.0.1:4750` (or that bridge's actual gateway); with
`--network host`, use `http://127.0.0.1:4750`. Docker pulls use dockerd's own proxy
settings. Guest root may disable the relay, but cannot enable VM egress or
change the host's policy snapshot. Host configuration editing and explicitly writable host files
remain trusted; this is an egress layer, not protection against host compromise.

Existing VMs without the proxy socket mapping require rebuilding the image and
explicitly resetting the VM before proxy mode can be used. The wrapper refuses
to enable it silently; `--raw-net` can explicitly run an existing direct-network
VM without resetting. A VM created with proxy support can switch between modes
after all sessions exit; a retained socket is served with deny-all policy when
proxy mode is disabled. Rebuild alone never replaces a persistent VM's filesystem.

Host-directory protection uses stable generated `.sandbox/exports` paths and a
`host_mounts` marker in launch.json; these do not change this user config format.
Stopped VMs with old raw shares receive one update while retaining guest data.
The current preparation script is streamed from the host, so a reset is not
required just to replace guest-side masks. Active VMs using old mount layouts
must finish before protection can be enabled. See README.md for the Ubuntu
namespace helper/profile and platform requirements.
