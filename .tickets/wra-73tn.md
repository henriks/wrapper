---
id: wra-73tn
status: open
deps: [wra-zqci]
links: []
created: 2026-05-17T10:19:36Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, vmnet]
---
# Option 1: split vmnet core from async I/O drivers

Prepare vmnet for Tokio by separating pure smoltcp/gateway state from QEMU, host, upstream, pcap, event-log, DNS/connect, and readiness I/O. Current serve_vmnet_gateway owns all of this in one large manual event loop.

## Design

Create boundaries such as VmnetCore for pure smoltcp/gateway frame handling and timers, VmnetIo for QEMU Unix stream framing and pcap capture, HostIngressDriver for accept/read/write, UpstreamDriver for connect/read/write, and EventSink for structured logs. Keep VmnetGateway and smoltcp single-owner and synchronous. Do not put smoltcp behind Arc<Mutex<_>>.

## Acceptance Criteria

Pure vmnet behavior remains testable without sockets, async driver interfaces are clear, QEMU frame codec has bounded async tests, and no duplicated sync/async runtime path is left without a deletion plan.


## Notes

**2026-05-17T10:28:44Z**

Async-boundary refinement: vmnet should be hybrid. Tokio may own QEMU stream readiness, host listener accepts, upstream socket I/O, DNS/connect workers, timers, and cancellation. VmnetGateway/smoltcp state, guest-visible ordering, pcap ordering, and policy decisions tied to frame handling should remain synchronous and single-owner.
