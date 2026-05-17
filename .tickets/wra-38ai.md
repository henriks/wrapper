---
id: wra-38ai
status: closed
deps: []
links: [wra-sum7, wra-bjaa]
created: 2026-05-16T21:51:58Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-bjaa
---
# Investigate live-setup-tools pi npm install hang during required validation

During validation for wra-sum7 after the appliance rebuild, ./vm-frontend/validate.sh required was attempted twice. Both attempts were killed by the harness timeout (first 600s, then 900s) while live-setup-tools was installing npm:@mariozechner/pi-coding-agent@0.73.1 after downloading/checksumming http:node@24.15.0. No agentvm/qemu validation processes were left running afterward. live-payload passed before this, but the required gate has not completed, so wra-sum7 cannot be closed yet. Logs: /tmp/pi-bash-b4283d702c3e826a.log and /tmp/pi-bash-49d7f10441dc5170.log.

## Acceptance Criteria

Determine whether this is expected duration, a network/cache issue, or a setup-tool bootstrap bug; make required validation complete reliably or document the necessary operator action/time budget; rerun ./vm-frontend/validate.sh required successfully before unblocking affected implementation tickets.


## Notes

**2026-05-16T21:55:22Z**

Iteration 8 investigation: required validation now fails quickly with status 143 during Pi setup bootstrap, not merely from harness timeout. The Pi bootstrap command reaches npm:@mariozechner/pi-coding-agent@0.73.1 install and then the payload appears to be terminated after ~70s. Inspecting docker/guest-payload-server.py shows wra-sum7 set a 30s socket timeout after the initial frame; run_payload treats socket.timeout while waiting for client input as a disconnect and terminates the process group. That can kill valid long-running quiet payloads such as mise/npm installs. Likely fix: keep initial-frame timeout and slow-writer send timeout, but do not treat absence of client input during an active primary payload as an IO timeout; poll for readable control frames before recv_frame and add regression coverage for a quiet primary payload outlasting io_timeout.

**2026-05-16T21:56:32Z**

Implemented offline fix: run_payload now polls the control socket with select before recv_frame, so absence of stdin/control input no longer triggers the per-socket timeout and kills a quiet primary payload. The socket timeout remains active for sendall/partial frame stalls, preserving slow-writer/write-failure cleanup. Added regression test test_quiet_primary_payload_outlives_io_timeout. Focused validation passed: python3 -m py_compile docker/guest-payload-server.py and python3 -m unittest docker.tests.test_guest_services... Appliance rebuild and required validation are still needed.

**2026-05-16T21:56:52Z**

Additional focused validation passed: ./vm-frontend/validate.sh guest-services. Source freshness for docker/guest-payload-server.py is stale against docker/out/artifact-manifest.json, so appliance rebuild is required before live/required validation can verify the fix.

**2026-05-17T05:59:57Z**

After appliance rebuild, ./vm-frontend/validate.sh required passed in 114s (log /tmp/wra-sum7-required-after-rebuild-20260517-085746.log). The Pi setup-tool bootstrap now completes: npm:@mariozechner/pi-coding-agent@0.73.1 added 202 packages in 53s, no-net restart and metadata checks passed. Closing bug.
