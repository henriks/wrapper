# vm-frontend fuzz targets

These targets are cargo-fuzz compatible and are intentionally outside the normal
`cargo test` path.

Install the runner once on a machine that can fetch tools:

```sh
cargo install cargo-fuzz
```

Run a short smoke target:

```sh
cd vm-frontend
cargo fuzz run vmnet_stream_frame_io -- -max_total_time=30
```

Available targets:

- `vmnet_stream_frame_io`: QEMU frame length-prefix parsing and writing.
- `dns_proxy_payload`: DNS UDP payload parsing and response classification.
- `vmnet_gateway_frame`: full guest Ethernet frame ingestion through
  `VmnetGateway::handle_guest_frame`.
- `composed_manifest_shape`: composed-fs manifest JSON, schema, mount path, and
  protected-path validation without opening host paths from fuzz input.

Longer runs should preserve interesting inputs in `fuzz/corpus/` and keep
generated crashes or temporary artifacts under `fuzz/artifacts/`, which is
ignored.
