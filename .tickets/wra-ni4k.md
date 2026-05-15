---
id: wra-ni4k
status: closed
deps: []
links: []
created: 2026-05-15T08:56:28Z
type: task
priority: 3
assignee: Henrik Saksela
parent: wra-sne0
---
# Use pcap-file for vmnet capture output

Replace hand-written pcap global header and packet record serialization in vm-frontend/src/vmnet_stream.rs PcapWriter with a focused crate such as pcap-file. Current code manually writes magic/version/linktype/timestamps and packet records. This is mostly a correctness and maintenance cleanup, not a major complexity reduction.

## Acceptance Criteria

PcapWriter uses pcap-file or a ticket note justifies keeping the small custom writer. Existing pcap tests still verify a readable Ethernet capture file. cargo test --manifest-path vm-frontend/Cargo.toml --offline passes.


## Notes

**2026-05-15T09:19:22Z**

PcapWriter now delegates global header and packet record serialization to pcap-file with DataLink::ETHERNET and native endianness. The local wrapper keeps the runtime API stable. Validation: writes_standard_pcap_file and full vm-frontend offline suite pass.
