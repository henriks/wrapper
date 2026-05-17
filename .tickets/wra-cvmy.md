---
id: wra-cvmy
status: open
deps: [wra-9glk]
links: []
created: 2026-05-17T10:19:23Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-xcvq
tags: [refactoring-option-1, tokio, payload, protocol]
---
# Option 1: extract shared payload protocol crate and codecs

Extract the duplicated payload frame protocol from vm-frontend/src/payload_client.rs and guest-service/src/lib.rs into a shared crate or shared module. The protocol uses 5 byte frame headers, bounded payload sizes, request frames, output/exit/failure events, stdin, resize, signal, ping, and diagnostic requests.

## Design

Prefer a workspace crate such as agentvm-payload-protocol. Move frame kind constants, request/event structs, max-size checks, sync decoder tests, and fuzz targets. Add async codec APIs using BytesMut or tokio_util::codec while preserving simple sync tests for arbitrary fragmentation. This is the foundation for async payload client and real Rust guest service.

## Acceptance Criteria

Frontend and guest-service use the same protocol definitions, duplicated framing code is removed, codecs reject oversized payloads before allocation, fragmentation tests and fuzz targets cover arbitrary input, and public APIs are ready for Tokio read/write integration.


## Notes

**2026-05-17T10:20:56Z**

Coordinate with wra-yl7i. The shared payload protocol should remain separate from the supervisor control protocol unless a deliberate design decision says otherwise. Payload framing can be reused by control clients only if it fits status/events/attach semantics cleanly.
