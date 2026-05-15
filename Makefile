FUZZ_TARGET ?= vmnet_gateway_frame
FUZZ_TIME ?= 3600
FUZZ_SMOKE_TIME ?= 60
FUZZ_JOBS ?= 1
FUZZ_WORKERS ?= $(FUZZ_JOBS)
FUZZ_RSS_MB ?= 4096
FUZZ_TIMEOUT ?= 10
FUZZ_ARGS ?=
export FUZZ_TOOLCHAIN ?= nightly

.PHONY: fuzz-help fuzz-check fuzz-smoke fuzz-target fuzz-extended fuzz-repro fuzz-minimize

fuzz-help:
	@vm-frontend/fuzz/run.sh --help

fuzz-check:
	cargo check --manifest-path vm-frontend/fuzz/Cargo.toml

fuzz-smoke:
	vm-frontend/fuzz/run.sh --all --time $(FUZZ_SMOKE_TIME) --jobs $(FUZZ_JOBS) --workers $(FUZZ_WORKERS) --rss-mb $(FUZZ_RSS_MB) --timeout $(FUZZ_TIMEOUT) -- $(FUZZ_ARGS)

fuzz-target:
	vm-frontend/fuzz/run.sh --target $(FUZZ_TARGET) --time $(FUZZ_TIME) --jobs $(FUZZ_JOBS) --workers $(FUZZ_WORKERS) --rss-mb $(FUZZ_RSS_MB) --timeout $(FUZZ_TIMEOUT) -- $(FUZZ_ARGS)

fuzz-extended:
	vm-frontend/fuzz/run.sh --all --time $(FUZZ_TIME) --jobs $(FUZZ_JOBS) --workers $(FUZZ_WORKERS) --rss-mb $(FUZZ_RSS_MB) --timeout $(FUZZ_TIMEOUT) -- $(FUZZ_ARGS)

fuzz-repro:
	@test -n "$(FUZZ_ARTIFACT)" || (echo "set FUZZ_ARTIFACT=path/to/crash" >&2; exit 2)
	vm-frontend/fuzz/run.sh --target $(FUZZ_TARGET) --repro "$(FUZZ_ARTIFACT)"

fuzz-minimize:
	@test -n "$(FUZZ_ARTIFACT)" || (echo "set FUZZ_ARTIFACT=path/to/crash" >&2; exit 2)
	vm-frontend/fuzz/run.sh --target $(FUZZ_TARGET) --minimize "$(FUZZ_ARTIFACT)"
