---
id: wra-bjaa
status: open
deps: []
links: [wra-yl7i]
created: 2026-05-16T16:09:38Z
type: epic
priority: 1
assignee: Henrik Saksela
tags: [async, tokio, stability, performance, agentvm]
---
# Epic: Evaluate and introduce async runtime for AgentVM service IO

Several stability/performance findings from the AgentVM hardening pass point at the same architectural pressure: the frontend currently mixes a synchronous smoltcp owner loop with blocking or nonblocking-but-not-queued service IO. This epic evaluates and, where justified, introduces an async runtime boundary for service IO while preserving the current correctness model.

Motivation:
- QEMU stream writes need backpressure and partial-write handling.
- DNS upstream exchange and TCP connect should not block the vmnet event loop.
- TCP/host-ingress sessions need fairness, bounded queues, and cancellation.
- Payload control would benefit from robust cancellation and signal/shutdown semantics.
- Guest Python TCP services may either need bounded async implementations or a Rust replacement.

Important constraints:
- Do not rewrite everything just to use Tokio. Keep smoltcp owned by a single runtime owner unless a ticket proves another model is better.
- Keep existing network policy and guest-visible failure semantics explicit.
- Compose changes so live validation remains meaningful after each step.
- For arbitrary input/output paths, include focused fuzz/stress coverage where behavior changes.

Acceptance:
- Child tickets document the proposed async boundary, scoped implementation slices, risks, migration order, and validation requirements.
- Any Tokio adoption has clear wins over the current mio/synchronous code, not just stylistic preference.
- Required validation and relevant live tiers pass before implementation tickets are closed.

