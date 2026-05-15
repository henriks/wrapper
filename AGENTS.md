This project uses a CLI ticket system for task management. Run `tk help` when you need to use it.

* when writing a ticket, please include all the necessary context for somebody to be able to effectively start working on it: things like which parts of the code it relates to, required reference information, whatever you consider relevant
* when creating epics and tickets ensure the dependencies between tickets reflect implementation order correctly
* when working on a ticket, add notes for any significant insights gained during implementation
* when working on a ticket, if encountering details relevant to other tickets, document them in said other tickets
* when done with a ticket, remember to close it

# More code is a liability

Always consider carefully whether a problem can be solved by removing and combining code, and introducing abstractions. We can't keep on tacking on more code ad infinitum.

# Testing

After code changes, run the full validation set before considering the work done: the offline test suite (`cargo test --manifest-path vm-frontend/Cargo.toml --offline`) and live validation (`./vm-frontend/validate.sh live`). If live validation cannot be run in the current environment, explicitly document that limitation and ask the user to run it.

Unless code has been exercised in the live environment, do not mark a task as completed.

For any code dealing with arbitrary input and output, make sure to add fuzzing tests.
