# vm-frontend fuzz targets

These targets are cargo-fuzz compatible and are intentionally outside the normal
`cargo test` path.

Install the runner once on a machine that can fetch tools:

```sh
cargo install cargo-fuzz
```

The repository root has make targets for the common flows:

```sh
make fuzz-smoke
make fuzz-target FUZZ_TARGET=vmnet_gateway_frame FUZZ_TIME=21600 FUZZ_JOBS=8
make fuzz-extended FUZZ_TIME=43200
make fuzz-repro FUZZ_TARGET=vmnet_gateway_frame FUZZ_ARTIFACT=vm-frontend/fuzz/artifacts/vmnet_gateway_frame/crash-...
make fuzz-minimize FUZZ_TARGET=vmnet_gateway_frame FUZZ_ARTIFACT=vm-frontend/fuzz/artifacts/vmnet_gateway_frame/crash-...
```

The make targets default to `FUZZ_TOOLCHAIN=nightly`, because cargo-fuzz uses
nightly sanitizer flags on this setup. Override it only if your local
cargo-fuzz runner supports stable.

The wrapper writes libFuzzer's expanding corpus to ignored
`fuzz/run-corpus/<target>/` directories. It initializes that mutable corpus from
named files in checked-in `fuzz/corpus/<target>/` directories, while ignoring
hash-named files that libFuzzer may create during direct runs. Crash artifacts
remain under ignored `fuzz/artifacts/`, and per-target output logs go to ignored
`fuzz/logs/`.

Run a short target through the wrapper:

```sh
vm-frontend/fuzz/run.sh --target vmnet_stream_frame_io --time 30
```

Available targets:

- `vmnet_stream_frame_io`: QEMU frame length-prefix parsing and writing.
- `dns_proxy_payload`: DNS UDP payload parsing and response classification.
- `vmnet_gateway_frame`: full guest Ethernet frame ingestion through
  `VmnetGateway::handle_guest_frame`.
- `composed_manifest_shape`: composed-fs manifest JSON, schema, mount path, and
  protected-path validation without opening host paths from fuzz input.
- `composed_fs_ops`: composed-fs file operation sequences checked against a
  host-side oracle for data, errno, readonly, rename, truncate, and chmod
  behavior.
- `composed_fs_locks`: composed-fs byte-range lock operation sequences checked
  against an owner/range model for `setlk`, `getlk`, and `flush`.

Longer runs should preserve curated regression seeds in `fuzz/corpus/` and keep
generated crashes, logs, and mutable corpus output in ignored fuzz directories.
