---
id: wra-rh21
status: open
deps: []
links: [wra-vl1o, wra-g13g, wra-g34z]
created: 2026-05-16T15:50:47Z
type: bug
priority: 2
assignee: Henrik Saksela
parent: wra-piqm
tags: [vmnet, tls, performance]
---
# Bound TLS MITM per-host certificate cache

Problem:
TLS MITM certificate generation caches per-host certificates without an explicit size/TTL limit.

Relevant code:
- vm-frontend/src/tls_mitm.rs:21 and related resolver/cache types.
- vm-frontend/src/tls_mitm.rs:92 and certificate generation paths.

Impact:
A guest can connect with many unique SNI values, forcing expensive certificate/key generation and unbounded cache growth.

Recommended fix:
Add a bounded LRU cache and reject or normalize excessive/invalid SNI values before certificate generation. Track/log eviction and generation failures.

Validation:
- Add adversarial resolver tests with many unique SNI names asserting bounded cache size.
- Add small stress/benchmark-style coverage for generation churn.


## Notes

**2026-05-18T10:38:15Z**

Follow-up cleanup epic wra-emj5 links wra-g34z for vmnet event/buffer consolidation. Bound the TLS MITM certificate cache directly inside the existing resolver path, with SNI normalization/rejection before generation. Avoid introducing a second cache layer or separate TLS session cleanup machinery.
