---
id: wra-p06p
status: closed
deps: []
links: [wra-g34z, wra-piqm, wra-0djh]
created: 2026-05-18T11:34:55Z
type: task
priority: 2
assignee: Henrik Saksela
parent: wra-emj5
tags: [cleanup, vmnet, guest, networking]
---
# Disable guest IPv6 when vmnet only supports IPv4

The AgentVM vmnet path is intentionally IPv4-only today: L2Gateway serves ARP and DHCPv4, smoltcp is built with proto-ipv4, guest-init configures an IPv4 address/default route, and VmnetPolicy denies/logs IPv6 frames. Linux can still emit IPv6 link-local/router-solicitation/multicast-discovery traffic when the guest interface comes up, producing unsupported_protocol IPv6 DenyAndLog noise even though no IPv6 route exists. Disable IPv6 inside the guest for this network until IPv6 is deliberately supported end to end.

## Design

Update docker/guest-init.sh to disable IPv6 before or while bringing the guest NIC up, using sysctl/proc settings for all/default and the selected interface once IFACE is known. Keep vmnet gateway IPv6 deny handling as defense in depth. If normal boot still emits IPv6 denied events, consider reducing routine deny log severity or filtering expected local IPv6 discovery separately, but do not pretend IPv6 is routable. Update active docs that mention IPv6 policy if needed.

## Acceptance Criteria

Guest boot disables IPv6 for the VM network interface; normal boot no longer produces routine unsupported IPv6 policy warnings from link-local/discovery traffic; vmnet still denies any IPv6 frame that reaches the gateway; focused guest-init/vmnet tests or live-smoke evidence are recorded; required validation is recorded before close.


## Notes

**2026-05-18T12:30:23Z**

Starting implementation. This touches docker/guest-init.sh guest network setup, so it is an appliance-input change and will require Henrik to rerun sudo ./docker/build-appliance.sh before live/required validation and close.

**2026-05-18T12:31:41Z**

Implemented guest IPv6 disable path in docker/guest-init.sh: disable_guest_ipv6_defaults writes /proc/sys/net/ipv6/conf/{all,default}/disable_ipv6 before lo/guest NIC setup, and disable_ipv6_for "" runs before ip link set "" up. Updated vm-frontend/network-policy.md to document that guest init disables IPv6 while vmnet deny/log remains defense in depth. Added offline test coverage in docker/tests/test_guest_services.py. Local validation passed: bash -n docker/guest-init.sh; AGENTVM_GUEST_INIT_SOURCE_ONLY=1 . docker/guest-init.sh; python3 -m unittest docker.tests.test_guest_services; ./vm-frontend/validate.sh guest-services; ./vm-frontend/validate.sh docs. Because docker/guest-init.sh changed, Henrik must rerun sudo ./docker/build-appliance.sh before live/required validation and close.

**2026-05-18T12:36:08Z**

Additional local validation while awaiting appliance rebuild: ./vm-frontend/validate.sh fast && ./vm-frontend/validate.sh fuzz-check passed with timeout 300s. Closure remains blocked on sudo ./docker/build-appliance.sh and required/live validation because docker/guest-init.sh changed.

**2026-05-18T12:41:02Z**

Henrik rebuilt appliance on 2026-05-18. Verified docker/out/artifact-manifest.json is fresh after rebuild and ./vm-frontend/validate.sh required passed with timeout 300s. The required gate included offline tests, offline guest-service tests (including guest-init IPv6 disable ordering), fuzz target compilation, live-smoke, and live-setup-tools. Closed after validation.
