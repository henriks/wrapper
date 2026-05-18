This project uses a CLI ticket system for task management. Run `tk help` when you need to use it.

* when writing a ticket, please include all the necessary context for somebody to be able to effectively start working on it: things like which parts of the code it relates to, required reference information, whatever you consider relevant
* when creating epics and tickets ensure the dependencies between tickets reflect implementation order correctly
* when working on a ticket, add notes for any significant insights gained during implementation
* when working on a ticket, if encountering details relevant to other tickets, document them in said other tickets
* if encountering bugs during development, make sure to document them as a bug ticket, instead of working around their impact in an inoptimal way -- that way we can return to them
* if a ticket you work on has raises unexpected new requirements, or things that feel like they should be addressed, create tickets with the FOLLOW-UP: prefix
* when done with a ticket, remember to close it


# On compatability

Only care about compatability for the config file. Never keep things like command line argument "for compatability", or do excessively defensive coding.

The `.sandbox/config.json` format is documented in `vm-frontend/config-json.md`. Whenever changing config parsing, serialization, defaults, migration behavior, or any field semantics, update that document and the relevant tests in the same change.

In general, do not care about drastic changes 'breaking production' -- do not introduce intermediary steps or keep old codepaths out of concern for existing users.

# More code is a liability

Always consider carefully whether a problem can be solved by removing and combining code, and introducing abstractions. We can't keep on tacking on more code ad infinitum.

# Testing

When you need to test, run the required validation gate: `./vm-frontend/validate.sh required`. That gate includes formatting, composed-fs, guest-service, payload-protocol, and vm-frontend offline tests, offline guest service tests, fuzz target compilation, documentation drift checks, the quick live host scenario (`live-smoke`), and the Codex/Pi setup-tool bootstrap/persistence live scenario (`live-setup-tools`). The live-only tier remains available as `./vm-frontend/validate.sh host-live` (or `live`), and broader live scenarios are available through `live-setup-tools`, `live-hostile`, `live-payload`, `live-dns`, `live-docker`, `live-fs`, and `live-full`. If live validation cannot be run in the current environment, explicitly document that limitation and ask the user to run it.

Test after finishing larger units of work.

Prefer short command timeouts. Do not use very long timeouts such as 1800s by default; start with at most 300s unless there is concrete evidence a specific command needs more time, and explain why when using a longer timeout.

Unless code has been exercised in the live environment, do not mark a task as completed.

For any code dealing with arbitrary input and output, make sure to add fuzzing tests.

