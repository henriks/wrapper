---
id: wra-bjaa
status: closed
deps: [wra-lcbk]
links: [wra-yl7i, wra-38ai]
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


## Notes

**2026-05-16T16:24:57Z**

Started Ralph loop .ralph/wra-bjaa-async-service-io.md to implement epic in dependency order. Will pause before appliance/guest-service packaging work that may require rebuild.

**2026-05-16T16:37:32Z**

Iteration 5 reflection: wra-35eb is closed after required validation. Payload policy/signal work stayed host-side and did not require appliance rebuild. Next implementation should move to wra-m7gg (now unblocked) and shape a cancellable PayloadSessionRunner using the policy/actions already added.

**2026-05-16T19:18:20Z**

Iteration 50 check: tk ready still shows wra-nui7 in progress as the only active unblocked epic child; wra-lcbk remains blocked by wra-g0uv, wra-sum7, and wra-dky9. No appliance-sensitive files touched.

**2026-05-16T19:34:56Z**

Iteration 56 reflection/check: wra-nui7 remains the only active unblocked child; wra-lcbk remains blocked by wra-g0uv, wra-sum7, and wra-dky9. Added byte-IO proptest coverage; no appliance-sensitive files touched.

**2026-05-16T19:38:15Z**

Required validation passed after iteration 57 byte-IO service-boundary work: ./vm-frontend/validate.sh required completed successfully within 600s, including live setup-tool scenarios. No appliance-sensitive files touched.

**2026-05-16T19:41:26Z**

Closed wra-nui7 after required validation at the tested async-ready boundary: owner-side host-ingress/session bounds, accept queue/fairness caps, upstream read caps, ByteIo command/completion envelopes, executor, spawned worker, owner-step primitive, and proptest/focused coverage are in place. Created follow-up wra-jenv under the hardening epic for production established-session worker adapter after QEMU backpressure/per-session drain/stale-reaping prerequisites are ready. wra-lcbk remains the only open child and is still blocked/appliance-sensitive.

**2026-05-16T19:42:52Z**

Closure check after wra-nui7: all implemented host-side/vmnet/payload async-boundary children are closed with required validation. The only remaining child is wra-lcbk, which is blocked by wra-g0uv, wra-sum7, and wra-dky9 and is appliance-sensitive; no appliance/guest assets were touched. Added notes to wra-lcbk and its blockers documenting that the Rust/Tokio guest-service spike should not start until prerequisite Python service semantics are fixed and appliance-sensitive work is approved.

**2026-05-16T19:43:24Z**

Set epic dependency on remaining open child wra-lcbk so tk ready no longer advertises wra-bjaa while all implementable host-side/vmnet children are closed and the only remaining child is blocked/appliance-sensitive. Epic should resume only when wra-g0uv, wra-sum7, and wra-dky9 unblock the guest-service spike and the user approves appliance-sensitive work.

**2026-05-16T19:44:21Z**

Iteration 61 reflection: all non-appliance async-service-IO slices are closed and validated; wra-bjaa remains blocked solely on appliance-sensitive wra-lcbk, which waits for wra-g0uv, wra-sum7, and wra-dky9. Next progress requires user approval before guest-service/appliance work.

**2026-05-16T19:44:49Z**

Iteration 62 audit: no docker/appliance-sensitive paths are modified; remaining code changes are host-side/vmnet files already covered by iteration 57 required validation. Current blockers unchanged: wra-bjaa depends on wra-lcbk; wra-lcbk waits for wra-g0uv, wra-sum7, and wra-dky9. Ready prerequisite work is outside this epic and includes appliance/guest-service-sensitive areas, so do not proceed here without user approval.

**2026-05-16T19:45:21Z**

Iteration 63 handoff: dependency tree still shows only blocked child wra-lcbk. Modified source list contains no docker/appliance paths; host-side/vmnet changes remain the already-validated implementation. Resume conditions: finish wra-g0uv and wra-dky9, then wra-sum7, then pause for user approval before appliance-sensitive wra-lcbk work.

**2026-05-16T19:45:43Z**

Iteration 64 blocked-state check: wra-bjaa remains blocked by wra-lcbk; no docker/appliance/guest-service paths are modified. Ready prerequisite tickets (wra-g0uv, wra-dky9) are outside this epic and appliance/guest-service-sensitive, so this Ralph loop should stay parked unless the user approves that work.

**2026-05-16T19:46:40Z**

Iteration 66 reflection: implementable non-appliance async-service-IO work is complete/validated; epic remains blocked only on appliance-sensitive wra-lcbk and its prerequisite guest-service tickets. No docker/appliance/guest-service paths are modified; do not continue into prerequisites from this loop without user approval.

**2026-05-16T19:48:31Z**

Iteration 69 audit: status remains blocked on wra-lcbk; ready work remains outside this epic (notably wra-g0uv and wra-dky9) and guest/appliance-sensitive. No docker/appliance/guest-service paths are modified, so no appliance rebuild is needed; do not proceed to guest-service prerequisites without explicit user approval.

**2026-05-16T19:49:22Z**

Iteration 71 reflection: no implementation approach change. All non-appliance async service IO work is complete and validated; the epic remains intentionally blocked on appliance-sensitive wra-lcbk. Continue to avoid guest-service prerequisite work from this loop unless the user explicitly approves appliance-sensitive changes/rebuild workflow.

**2026-05-16T19:51:05Z**

Iteration 76 reflection: all non-appliance children remain closed/validated and no sensitive paths are modified. Epic remains blocked on wra-lcbk; do not proceed into ready prerequisites wra-g0uv/wra-dky9 from this loop without explicit approval because they lead into guest-service/appliance-sensitive work.

**2026-05-16T19:53:38Z**

Iteration 80 final audit: epic remains in_progress/blocked on the only open child wra-lcbk. wra-lcbk is still blocked by wra-g0uv, wra-sum7, and wra-dky9; only wra-g0uv and wra-dky9 are ready and both are outside this epic and lead into guest-service/appliance-sensitive work. git status/diff sensitive-path checks show no docker/, docker-compose, appliance, build-appliance, or guest-* modifications, so no appliance rebuild is needed. Do not proceed without user approval for appliance-sensitive prerequisite work.

**2026-05-16T20:28:30Z**

Blocked-audit iteration 3: rechecked sensitive paths and validation need; no docker/, docker-compose, appliance, build-appliance, or guest-* paths are modified, so no appliance rebuild is needed. No code changed in this audit, so no validation rerun is needed. Epic children remain all closed except appliance-sensitive wra-lcbk. Follow-up wra-jenv is outside this epic and remains blocked by wra-bbgh, wra-57z4, and wra-e9sr.

**2026-05-16T20:30:23Z**

Blocked-audit iteration 6 reflection: epic state is unchanged and intentionally parked. All non-appliance async service IO children remain closed/validated; only wra-lcbk remains open and it is appliance-sensitive and blocked by wra-g0uv, wra-sum7, and wra-dky9. wra-jenv remains outside this epic and blocked on vmnet hardening prerequisites. No docker/appliance/guest-* paths are modified; no validation rerun is needed because no code changed in this audit.

**2026-05-16T20:32:44Z**

Blocked-audit iteration 8: status remains unchanged. wra-bjaa is in_progress and blocked on wra-lcbk; wra-lcbk remains blocked by wra-dky9 and wra-sum7 -> wra-g0uv. Ready relevant work (wra-bbgh, wra-g0uv, wra-dky9) remains outside this epic. Sensitive-path filter still shows no docker/, docker-compose, appliance, build-appliance, or guest-* modifications. No code changed in this audit; no validation rerun needed.

**2026-05-16T20:34:14Z**

Blocked-audit iteration 10 final handoff: 10-iteration audit complete. wra-bjaa remains in_progress and blocked on its only open child wra-lcbk. All non-appliance async-service-IO children remain closed/validated; wra-lcbk remains appliance-sensitive and blocked by wra-dky9 and wra-sum7 -> wra-g0uv. Ready relevant tickets (wra-bbgh, wra-g0uv, wra-dky9) are outside this epic. Sensitive-path checks found no docker/, docker-compose, appliance, build-appliance, or guest-* modifications. No code changed during the audit loop; no validation rerun or appliance rebuild is needed.

**2026-05-16T20:49:39Z**

Started wra-bjaa-guest-service-prereqs loop for remaining wra-lcbk prerequisite chain. Initial audit: wra-g0uv and wra-dky9 are ready; wra-sum7 and wra-lcbk remain blocked. First likely implementation target is wra-g0uv, but it likely touches docker/guest-init.sh/readiness semantics, so pause before code changes that may require appliance rebuild workflow.

**2026-05-16T21:57:40Z**

Ralph iteration 10 summary: prerequisite chain is still blocked on appliance rebuild/required validation for docker/guest-payload-server.py. wra-g0uv and wra-dky9 are closed. wra-sum7 is implemented offline but depends on wra-38ai; wra-38ai fix is implemented and guest-services validation passed, but source freshness is stale and required validation has not been rerun with rebuilt appliance. wra-lcbk remains blocked by wra-sum7.

**2026-05-17T06:03:40Z**

Epic closure: all scoped child tickets are now closed. Host-side async-boundary work landed earlier with required validation while preserving single smoltcp ownership. Guest-service prerequisite bounds are closed after rebuilt-appliance required validation. wra-lcbk completed as a spike: added docker/guest-service-rust-spike.md and deferred the risky Rust/Tokio guest binary/default switch into explicit follow-ups wra-dz2y -> wra-n0xc -> wra-y335. No new guest binary or appliance startup change was introduced by the spike; ./vm-frontend/validate.sh docs passed after the doc-only update, and the full required gate passed immediately beforehand after the appliance rebuild.
