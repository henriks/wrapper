---
id: wra-o8pc
status: closed
deps: [wra-huvu, wra-4bi6]
links: []
created: 2026-05-15T08:12:26Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-293p
---
# Make post--- command override the canonical one-run command UX

Rework command override semantics so the canonical UX is likely  or equivalent, instead of requiring . The configured default command runs when no post--- command is supplied. Post--- should mean one-run payload command override, while config default arguments should live in config.json. Preserve or deprecate --command deliberately. Relevant code: parse_wrapper_args_with_terminal currently treats post--- values as tool args and --command as a payload-script override.

## Acceptance Criteria

The command override grammar is documented and unambiguous. The parser maps post--- command forms to the intended guest payload. Existing tool-arg passthrough behavior is either migrated into config/default command args or kept through a clearly named compatibility path. Tests cover default command, command override with args, shell session example, explicit tool args if retained, and error messages for ambiguous forms.


## Notes

**2026-05-15T08:12:54Z**

Correction: shell backticks were expanded during ticket creation. Intended context: make the canonical one-run command override likely  or equivalent, rather than requiring . When no post- command is supplied, run the configured default command from config.json. Treat post- as a one-run payload command override. Preserve or deprecate  deliberately. Current code treats post- as tool args and  as the payload-script override.

**2026-05-15T08:13:04Z**

Clean correction: the intended canonical one-run command override is: agentvm -- <command> [args...]. If no command follows the separator, run the configured default command from config.json. The separator should mean payload command override, not tool-argument passthrough. Preserve or deprecate the existing --command flag deliberately. Current code treats separator args as tool args and --command as the payload-script override.

**2026-05-15T08:34:01Z**

Changed wrapper separator semantics: arguments after -- are now the full one-run payload command override. An empty separator leaves the configured default command in place. Legacy --command remains supported and can append args from -- for compatibility. Tool args for compatibility --tool launches are now expressed with --tool-arg. Tests cover post--- bash override while preserving configured Codex tool-state mounts.
