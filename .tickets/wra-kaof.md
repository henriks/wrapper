---
id: wra-kaof
status: closed
deps: [wra-ay63]
links: []
created: 2026-05-14T20:12:35Z
type: task
priority: 1
assignee: Henrik Saksela
parent: wra-f16x
tags: [tests, fuzzing, network, vmnet]
---
# Fuzz vmnet stream frame IO boundaries

Add randomized tests for QEMU/vmnet frame IO in vm-frontend/src/vmnet_stream.rs. This code is the byte stream boundary between QEMU and the frontend: QemuFrameIo reads/writes big-endian length-prefixed Ethernet frames, preserves partial reads in read_buf, reports WouldBlock, rejects zero/oversized frames, and avoids allocating beyond DEFAULT_MAX_FRAME_LEN. Existing tests cover representative examples; expand this with generated chunking and malformed inputs.

## Design

Generate sequences of input chunks containing length prefixes, payload bytes, EOF, and WouldBlock-like read errors using the existing ChunkedReader pattern if possible. Check both read_frame and try_read_frame behavior. Include concatenated valid frames, partial length prefixes, truncated payloads, zero length, oversized length, max-size valid frames, and arbitrary trailing bytes. Keep allocation bounded by using small configurable max_frame_len in tests.

## Acceptance Criteria

- Randomized tests assert no panic and no unbounded allocation for arbitrary byte/chunk sequences.
- Valid generated frames round-trip and preserve frame boundaries.
- Invalid length/truncation cases return VmnetStreamError variants rather than EOF or panic.
- Partial frames remain buffered across WouldBlock and later complete correctly.


## Notes

**2026-05-14T20:19:57Z**

Added vmnet_stream property tests around the QEMU frame boundary: generated valid chunked streams preserve frame boundaries across WouldBlock, arbitrary byte streams remain bounded with max_frame_len=64, invalid length prefixes are rejected before payload allocation, write_frame round-trips valid generated frames, and empty/oversized writes fail. Replaced the module-local scripted reader with the shared ScriptedStream helper. Added ignored stress_many_chunked_frame_splits with a documented cargo test command. Verified with cargo test --manifest-path vm-frontend/Cargo.toml vmnet_stream::tests --offline.
