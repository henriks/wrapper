---
id: wra-otbe
status: closed
deps: [wra-gx6d]
links: []
created: 2026-05-15T10:39:05Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-neci
tags: [validation, guest, shell, python]
---
# Add direct tests for guest init and Python guest services

The guest scripts are part of the runtime contract but are mostly exercised only indirectly by live VM runs. Add direct tests for docker/guest-init.sh, docker/guest-payload-server.py, and docker/guest-socket-bridge.py. Cover environment parsing, mounted config discovery, payload control path creation, service startup ordering, failure paths, signal handling/cleanup, and error messages. For shell, use a shell test framework or simple scripted tests that run in a temporary root/mock environment. For Python, use pytest or unittest with socket fixtures and temporary directories.

## Design

Keep tests runnable offline and avoid requiring QEMU. Mock external commands in guest-init.sh via PATH shims where practical. For Python services, test socket-level behavior directly and assert services exit cleanly under cancellation or peer disconnect.

## Acceptance Criteria

Guest init has offline tests for success and key failure paths. Payload server and socket bridge have direct Python tests for normal operation, invalid input, disconnects, and cleanup. These tests are included in the required validation tier or a clearly documented offline guest-services tier invoked by the required script.


## Notes

**2026-05-15T11:06:29Z**

Started and added offline guest service unittest coverage under docker/tests/test_guest_services.py. Tests load guest-payload-server.py and guest-socket-bridge.py directly, cover ping/failure frames, a real short payload request over the framed socket protocol, payload identity validation, bidirectional socket bridge relay, missing Docker socket cleanup, and sh -n syntax validation for guest-init.sh. Found and fixed a resource leak in guest-socket-bridge.py: handle_client now closes the upstream Unix socket if connect() fails. Added validate.sh guest-services tier and included it in required/all-local.

**2026-05-15T11:06:47Z**

Offline guest service tests are now included in ./vm-frontend/validate.sh guest-services and the required/all-local gates. Validation passed: python3 -W error::ResourceWarning -m unittest discover -s docker/tests -p '*test*.py' -v; ./vm-frontend/validate.sh guest-services; docs/fmt/fast/fuzz-check also passed. Remaining possible scope before closure: more guest-init mocked environment tests for startup ordering/failure paths and live validation after appliance rebuild.

**2026-05-15T11:08:47Z**

Expanded guest-init direct coverage. Refactored docker/guest-init.sh so tests can set AGENTVM_GUEST_INIT_SOURCE_ONLY=1 to source functions without executing the boot path; when /etc/agentvm.env is absent in source-only mode, test defaults are provided. Added docker/tests/test_guest_init.sh covering get_cmdline_value parsing and bind_composed_entry invalid required/optional behavior with mutating commands mocked. validate.sh guest-services now runs both the shell test and Python unittests. Validation: ./vm-frontend/validate.sh guest-services, docs, fmt, fast, and fuzz-check passed.

**2026-05-15T11:14:47Z**

Further expanded guest-init direct tests. docker/tests/test_guest_init.sh now also checks valid composed dir bind behavior with mkdir/mount mocked and wait_for_critical_exit returning when a critical service PID disappears. Validation passed through ./vm-frontend/validate.sh guest-services and full offline docs/fmt/fast/fuzz-check.

**2026-05-15T11:18:52Z**

Expanded guest payload service direct coverage: guest-payload-server.py now enforces a 16MiB frame payload limit, reports invalid initial request JSON via F frames, reports oversized initial frames without reading the payload, and handles EOF mid-initial-frame without hanging. docker/tests/test_guest_services.py covers fragmented initial frames, invalid JSON, oversized frames, EOF mid-frame, and send-frame oversized errors. ./vm-frontend/validate.sh guest-services passed with 12 tests.

**2026-05-15T11:30:00Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed end-to-end. guest-services tier is included in required and covers guest-init syntax/cmdline/bind/wait failure behavior plus Python payload/socket services normal, invalid, disconnect, oversized frame, and cleanup paths. Live self-test ok exercised rebuilt guest scripts.
