---
id: wra-e9sr
status: closed
deps: [wra-i3t9]
links: [wra-57z4, wra-nui7, wra-jenv, wra-bbgh, wra-73tn, wra-f762, wra-ynx7]
created: 2026-05-16T15:50:47Z
type: bug
priority: 1
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, tcp, performance]
---
# Reap closed guest TCP sockets and stale session slots

Problem:
Guest TCP sockets are closed but not removed from smoltcp SocketSet or associated listener/host connection slot vectors.

Relevant code:
- vm-frontend/src/guest_tcp.rs:60-66 initializes SocketSet and slot vectors.
- vm-frontend/src/guest_tcp.rs:116-147 closes sessions but does not reclaim terminal sockets.
- vm-frontend/src/host_ingress.rs:345 removes bridge sessions after guest close while the core socket can remain.

Impact:
Long-running sessions with many sequential outbound or host-ingress connections can accumulate closed sockets, increase memory use, increase per-poll scan cost, and allocate replacement listeners repeatedly.

Recommended fix:
Add a GuestTcpCore reap_closed_sessions path that removes terminal sockets from SocketSet and prunes listener/host connection slots once FIN/RST state is terminal and queued frames are drained.

Validation:
- Add churn tests for hundreds/thousands of sequential outbound and host-ingress sessions.
- Assert bounded socket/slot counts and stable poll cost.
- Run vmnet runtime tests and required validation before closing.


## Notes

**2026-05-16T19:08:35Z**

wra-nui7 BufferLimitExceeded host-ingress failures remove the bridge session and vmnet_runtime deregisters HostSession handles on that event, similar to HostClosed/GuestClosed. This reduces one stale-registration path but does not address broader stale guest TCP socket/session reaping covered by wra-e9sr.

**2026-05-16T21:46:53Z**

Review after closing wra-i3t9: this ticket is now unblocked on the connect-failure side. Async connect failures now close/reset guest sessions through TcpProxyBridge::complete_connect, but GuestTcpCore still needs an explicit reap_closed_sessions path for terminal smoltcp sockets and listener/host slot pruning under churn.

**2026-05-18T08:24:44Z**

Cleanup loop iteration 28 audit: GuestTcpCore still has no reap_closed_sessions/removal API; listen_tcp/connect_to_guest push handles into listeners/host_connections and close_session only calls socket.close(). active_sessions()/host_ingress_sessions() filter by session() but never remove terminal sockets or slot entries. TcpProxyBridge and HostIngressBridge already remove their own HashMap sessions on guest/upstream/host close and some tests assert bridge-session removal, so the remaining leak/reap target is the smoltcp SocketSet plus GuestTcpCore listener/host slot vectors. To unblock wra-ynx7, implement reaping inside GuestTcpCore/VmnetGateway owner path rather than adding cleanup to every proxy/host-ingress adapter.

**2026-05-18T08:28:05Z**

Iteration 29 implementation: added central GuestTcpCore::reap_closed_sessions with GuestTcpReap counts. It removes Closed smoltcp sockets from SocketSet and prunes listener/host connection slots in one owner/core path. Added VmnetGateway::reap_closed_tcp_sessions and call it at the end of the production async vmnet owner loop after proxy/host-ingress processing, so adapters get a chance to observe guest/session closure before core slots are reclaimed. Added unit coverage for closed listener and host-connection socket/slot reaping. Validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features guest_tcp::tests::reaps_closed -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features guest_tcp::tests:: -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features vmnet_runtime::tests:: -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml; git diff --check; ./vm-frontend/validate.sh fast passed with full log /tmp/pi-bash-7542e5df312bf90b.log. Required validation remains blocked until privileged appliance rebuild after wra-vd8g docker/build-appliance.sh changes.

**2026-05-18T08:29:34Z**

Iteration 30 churn hardening: added churn tests for GuestTcpCore reaping. reaping_bounds_listener_slots_under_churn loops 512 listen/close/reap cycles and asserts listener slots and SocketSet count return to zero each cycle. reaping_bounds_host_connection_slots_under_churn loops 512 host connect/close/reap cycles and asserts host slot and SocketSet count return to zero each cycle. Additional validation passed: cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features guest_tcp::tests::reaping_bounds -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features guest_tcp::tests:: -- --nocapture; cargo test --manifest-path vm-frontend/Cargo.toml --offline --no-default-features vmnet_gateway::tests:: -- --nocapture; cargo fmt --manifest-path vm-frontend/Cargo.toml; git diff --check. Required validation remains blocked pending privileged appliance rebuild.

**2026-05-18T08:33:40Z**

Required validation passed after appliance rebuild: ./vm-frontend/validate.sh required exited successfully, including guest_tcp/vmnet tests, fuzz target compilation, live-smoke, and live-setup-tools. Full output: /tmp/pi-bash-4593beabebdd1968.log. Churn coverage now asserts repeated listener and host-connection close/reap cycles keep socket/slot counts bounded. Closing wra-e9sr.
