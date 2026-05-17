---
id: wra-cvmy
status: closed
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

**2026-05-17T12:32:18Z**

Starting implementation. First slice will inventory the duplicated payload frame protocol in vm-frontend/src/payload_client.rs and guest-service/src/lib.rs, then extract a small shared workspace crate with behavior-preserving sync frame definitions/tests before adding async codec surface. Keep max-size rejection before allocation and preserve existing fuzz target coverage by moving/adding fuzz entry points as needed.

**2026-05-17T12:34:31Z**

Iteration 32 first extraction slice: added workspace crate agentvm-payload-protocol, moved the guest-service frame protocol implementation/tests there, made guest-service re-export the shared crate, and changed vm-frontend payload_client send_frame/recv_frame to use the shared encode/decode-header/FrameKind/MAX_FRAME_PAYLOAD semantics. Added a payload_protocol_frame fuzz target that exercises arbitrary frame bytes and round-trips bounded generated payloads. Focused validation passed: payload-protocol tests, guest-service tests, vm-frontend frame tests, and cargo check for the new fuzz target.

**2026-05-17T12:38:54Z**

Iteration 33 protocol surface slice: moved PayloadRequest, DiagnosticRequest, PayloadEvent, ExitFrame/SignalFrame/ResizeFrame payload schemas, event decoding, and signal/resize JSON payload builders into agentvm-payload-protocol. vm-frontend payload_client now re-exports the moved public request/event types to preserve existing module API while using shared event/control-frame decoding. Added Tokio-ready read_frame_async/write_frame_async APIs in the protocol crate; async read rejects oversized declared lengths immediately after the 5-byte header, before payload allocation, and async write rejects oversized public Frame payloads before encoding. Focused validation passed: payload-protocol tests (including async helpers/schema/event tests), guest-service tests, vm-frontend payload_client tests, and new payload_protocol_frame fuzz target compilation.

**2026-05-17T12:42:18Z**

Required validation passed for shared payload protocol crate/codecs: ./vm-frontend/validate.sh required completed end-to-end with updated payload-protocol fmt/offline test coverage in the gate, docs drift check, composed-fs/guest-service/vm-frontend offline tests, offline guest service tests, fuzz target compilation including payload_protocol_frame, live-smoke, and live-setup-tools. No appliance rebuild was requested. Scope covers shared frame constants/kinds/decoder/encoder, request/event/control payload schemas, sync fragmentation/oversize tests, fuzz coverage for arbitrary frame bytes, and Tokio-ready async frame read/write APIs; frontend keeps existing payload_client API via re-exports.
