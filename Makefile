FUZZ_TARGET ?= vmnet_gateway_frame
FUZZ_TIME ?= 3600
FUZZ_SMOKE_TIME ?= 60
FUZZ_JOBS ?= 1
FUZZ_WORKERS ?= $(FUZZ_JOBS)
FUZZ_RSS_MB ?= 4096
FUZZ_TIMEOUT ?= 10
FUZZ_ARGS ?=
export FUZZ_TOOLCHAIN ?= nightly
BUILD_PROFILE ?= dev
BUILD_BIN ?= agentvm
RELEASE_HOST_BINS ?= agentvm agentvm-frontend
RELEASE_CARGO_FLAGS ?= --offline
GUEST_SERVICE_TARGET ?= x86_64-unknown-linux-musl
GUEST_SERVICE_RELEASE_BIN ?= target/$(GUEST_SERVICE_TARGET)/release/agentvm-guest-service

.PHONY: build-binary release release-host release-self-test release-guest-service release-appliance fuzz-help fuzz-check fuzz-smoke fuzz-target fuzz-extended fuzz-repro fuzz-minimize

build-binary:
	cargo build --manifest-path vm-frontend/Cargo.toml --bin $(BUILD_BIN) $(if $(filter release,$(BUILD_PROFILE)),--release,)

release: release-host release-self-test release-guest-service

release-host:
	@for bin in $(RELEASE_HOST_BINS); do \
		echo "building release host binary: $$bin"; \
		cargo build --manifest-path vm-frontend/Cargo.toml --release $(RELEASE_CARGO_FLAGS) --bin "$$bin"; \
	done

release-self-test:
	cargo build --manifest-path vm-frontend/Cargo.toml --release $(RELEASE_CARGO_FLAGS) --features validation-self-test --bin agentvm-self-test

release-guest-service:
	cargo build --manifest-path guest-service/Cargo.toml --release $(RELEASE_CARGO_FLAGS) --target $(GUEST_SERVICE_TARGET) --bin agentvm-guest-service

release-appliance: release-guest-service
	sudo env AGENTVM_GUEST_SERVICE_BIN="$(GUEST_SERVICE_RELEASE_BIN)" ./docker/build-appliance.sh

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
