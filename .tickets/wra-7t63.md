---
id: wra-7t63
status: open
deps: [wra-yl7i]
links: [wra-yl7i, wra-xcvq]
created: 2026-05-17T21:24:40Z
type: feature
priority: 2
assignee: Henrik Saksela
tags: [tui, control-socket, payload, architecture, agentvm]
---
# Move payload viewport attach/detach onto supervisor control protocol

Follow-up split from wra-yl7i. The current option-1 control-plane work has established a typed bounded supervisor-control socket, status snapshot/subscription, shutdown requests wired into async QEMU/service cancellation, an async-launch sidecar, agentvm control status/shutdown, and TUI status-summary seams. The remaining larger lifecycle/UI task is to move payload viewport I/O itself behind the supervisor/control boundary so the TUI can attach/detach/reconnect without directly owning the VM launch path. Relevant code: vm-frontend/src/tui.rs run_payload_viewport, vm-frontend/src/launch_cli.rs run_launch payload/TUI branch, vm-frontend/src/payload_client.rs PayloadSession/PayloadWriter, vm-frontend/src/supervisor_control.rs control protocol/client, vm-frontend/src/launch.rs async launch supervisor sidecar. Keep the supervisor as the single owner of VM lifecycle; do not introduce a second VM owner in the TUI. Preserve .sandbox/config.json compatibility.

## Acceptance Criteria

Payload start/attach/resize/input/signal/exit semantics are exposed through the supervisor/control boundary or an explicitly documented supervisor-owned payload session boundary; TUI mode uses that client boundary instead of directly starting/stopping the frontend; disconnect/reconnect behavior is defined and tested; nonzero payload failures remain visible after terminal restore; plain CLI and TUI share the client path where practical; arbitrary control/payload input remains bounded/fuzz-covered; ./vm-frontend/validate.sh required passes before closing.


## Notes

**2026-05-17T21:24:52Z**

Creation note correction: the omitted command in the description is agentvm control status|shutdown. This ticket intentionally depends on wra-yl7i: finish the supervisor-control boundary first, then move payload viewport attach/detach/reconnect semantics behind that boundary.

**2026-05-17T21:29:46Z**

Scope clarification: this is a post-option-1 follow-up split from wra-yl7i, not a blocker for closing the wra-xcvq option-1 epic. The option-1 epic owns establishing the supervisor-control boundary and TUI observation seam; this ticket owns the later larger payload attach/detach/reconnect lifecycle rewrite.
