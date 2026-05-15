This project uses a CLI ticket system for task management. Run `tk help` when you need to use it.

* when writing a ticket, please include all the necessary context for somebody to be able to effectively start working on it: things like which parts of the code it relates to, required reference information, whatever you consider relevant
* when creating epics and tickets ensure the dependencies between tickets reflect implementation order correctly
* when working on a ticket, add notes for any significant insights gained during implementation
* when working on a ticket, if encountering details relevant to other tickets, document them in said other tickets
* when done with a ticket, remember to close it


# On compatability

Only care about compatability for the config file. Never keep things like command line argument "for compatability", or do excessively defensive coding.

The `.sandbox/config.json` format is documented in `vm-frontend/config-json.md`. Whenever changing config parsing, serialization, defaults, migration behavior, or any field semantics, update that document and the relevant tests in the same change.

# More code is a liability

Always consider carefully whether a problem can be solved by removing and combining code, and introducing abstractions. We can't keep on tacking on more code ad infinitum.

# Testing

After code changes, run the required validation gate before considering the work done: `./vm-frontend/validate.sh required`. That gate includes formatting, composed-fs and vm-frontend offline tests, offline guest service tests, fuzz target compilation, documentation drift checks, and the quick live host scenario (`live-smoke`). The live-only tier remains available as `./vm-frontend/validate.sh host-live` (or `live`), and broader live scenarios are available through `live-hostile`, `live-payload`, `live-dns`, `live-docker`, `live-fs`, and `live-full`. If live validation cannot be run in the current environment, explicitly document that limitation and ask the user to run it.

Unless code has been exercised in the live environment, do not mark a task as completed.

For any code dealing with arbitrary input and output, make sure to add fuzzing tests.
