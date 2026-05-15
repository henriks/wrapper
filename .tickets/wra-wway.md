---
id: wra-wway
status: closed
deps: [wra-pido]
links: []
created: 2026-05-15T09:31:29Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-txoc
---
# Integrate QEMU stream readiness and smoltcp deadlines

Finish the runtime loop migration by integrating QEMU stream readiness and smoltcp timer deadlines into the mio-driven runtime. Relevant code: vm-frontend/src/vmnet_runtime.rs serve_vmnet_gateway/run_qemu_stream_tick, vm-frontend/src/vmnet_stream.rs QemuFrameIo/VmnetStreamEndpoint, and vm-frontend/src/guest_tcp.rs GuestTcpCore::poll. Current runtime reads at most one QEMU frame per tick, pumps proxy/host_ingress, and sleeps for DEFAULT_VMNET_IDLE_SLEEP when idle. The new runtime should wait on QEMU readiness, registered host/upstream sockets, and the next smoltcp deadline/housekeeping timeout instead of fixed idle sleeps.

## Design

Keep QEMU frame parsing/writing in QemuFrameIo or a narrow adapter. The runtime owner should call smoltcp poll with explicit Instant values whenever external packets arrive, app data is sent, sessions close, or the next smoltcp deadline expires. If smoltcp deadline APIs are not sufficient, document the minimum conservative timer needed and why. Preserve pcap capture and event logging behavior.

## Acceptance Criteria

serve_vmnet_gateway no longer uses DEFAULT_VMNET_IDLE_SLEEP as the primary idle mechanism for normal operation. QEMU frame readiness, EOF, frame writes, pcap capture, event logging, and smoltcp timer progress are handled by the readiness loop. Existing vmnet_stream/vmnet_gateway/vmnet_runtime tests pass, with added coverage for QEMU readable event, EOF shutdown, smoltcp deadline wake, and no busy idle loop. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:45:17Z**

Integrated the readiness loop into serve_vmnet_gateway. The runtime registers QEMU stream fd and host listener fds, polls with the current smoltcp poll_delay as timeout, drains QEMU frames on readable readiness, accepts host listeners on listener readiness, dispatches host/proxy session readiness, advances smoltcp timer work on poll timeout, and syncs session fd interests after each pump. DEFAULT_VMNET_IDLE_SLEEP remains in config for compatibility but is no longer the normal serve loop idle mechanism.
