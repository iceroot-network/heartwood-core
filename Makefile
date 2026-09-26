# Heartwood Core: Rust checks and the differential-test workflow. `make help` lists the targets.
#
#   make check          build, clippy (warnings are errors), fmt check, tests and the release check
#   make oracle-image   build the oracle image for COMMIT from scratch (needs Docker and the network)
#   make e2e            vectors in the container and locally, diff the two runs, run L0
#   make vectors-install  copy the container run into packages/crypto/tests/vectors
#
# Settings for this machine (SOLAR_REPO, LOCAL_SOLAR, TOOLCHAIN_ENV, ...) go in local.mk, which is
# git-ignored, or on the command line: make e2e SOLAR_REPO=/path/to/solar-core-ref

SHELL := /bin/bash
.DEFAULT_GOAL := help

-include local.mk

# A clone of the reference implementation, iceroot-network/solar-core-ref, with its tags.
SOLAR_REPO    ?= ../solar-core-ref
# The reference commit (or tag) the oracle image is built from: Solar Core 4.3.1 plus the
# reference's patch series.
COMMIT        ?= s1-ref-v2
# A checkout of the reference at COMMIT, installed and built, for local oracle runs.
LOCAL_SOLAR   ?= $(SOLAR_REPO)
# Optional shell file that puts the toolchain (Node 18.20.8, Rust) on PATH. When it is set, every
# recipe that runs cargo or node sources it first; when it is empty, the tools come from PATH.
TOOLCHAIN_ENV ?=
TOOLCHAIN     := $(if $(TOOLCHAIN_ENV),source '$(TOOLCHAIN_ENV)',true)
SHORT         := $(shell git -C '$(SOLAR_REPO)' rev-parse --verify -q '$(COMMIT)^{commit}' 2>/dev/null | cut -c1-9)
IMAGE         ?= heartwood-oracle:$(SHORT)
VECTORS       ?= target/vectors
IMAGE_DIR     ?= target/oracle-images
# The reference's devnet generator, gen-network.js, which the genesis class (V10) runs. When it is
# empty, V10 is not written; its directory is mounted read-only in container runs.
DEVNET_GENERATOR ?=
GENERATOR_ARG := $(if $(DEVNET_GENERATOR),--devnet-generator '$(DEVNET_GENERATOR)',)
# Where the committed golden vectors of heartwood-crypto live.
CRYPTO_VECTORS ?= packages/crypto/tests/vectors
DIFFTEST      := target/debug/difftest
# The packages whose builds are release artefacts, each built on its own (cargo build -p). None of
# them may enable a test seam of heartwood-crypto (TEST_SEAMS); `make release-check` checks it.
RELEASE_PACKAGES ?= heartwood-crypto
TEST_SEAMS    := fixed-aux
empty         :=
space         := $(empty) $(empty)
# The cargo-fuzz targets, a package of their own outside the workspace.
FUZZ_MANIFEST := fuzz/Cargo.toml

export DIFFTEST_DOCKER ?= $(CURDIR)/tools/oracle/docker.sh

# Stops a recipe that needs IMAGE when COMMIT cannot be resolved in SOLAR_REPO and IMAGE is not set.
need-image = $(if $(SHORT)$(filter-out heartwood-oracle:,$(IMAGE)),,$(error cannot resolve COMMIT=$(COMMIT) in SOLAR_REPO=$(SOLAR_REPO): set SOLAR_REPO to a clone of iceroot-network/solar-core-ref (in local.mk or on the command line), or set IMAGE))

.PHONY: help check build clippy fmt fmt-check test release-check oracle-context oracle-image \
        oracle-save vectors vectors-local vectors-diff vectors-install l0 ping test-oracle e2e

help: ## this list
	@grep -hE '^[a-z0-9-]+:.*## ' $(MAKEFILE_LIST) | sed -E 's/:.*## /\t/' | awk -F'\t' '{ printf "  make %-15s %s\n", $$1, $$2 }'
	@echo "Variables: SOLAR_REPO=$(SOLAR_REPO) COMMIT=$(COMMIT) IMAGE=$(IMAGE) LOCAL_SOLAR=$(LOCAL_SOLAR)"
	@echo "           VECTORS=$(VECTORS) IMAGE_DIR=$(IMAGE_DIR) TOOLCHAIN_ENV=$(TOOLCHAIN_ENV)"
	@echo "           DEVNET_GENERATOR=$(DEVNET_GENERATOR) CRYPTO_VECTORS=$(CRYPTO_VECTORS)"

check: build clippy fmt-check test release-check ## build, clippy, fmt check, tests, release check

build: ## cargo build (the difftest binary)
	$(TOOLCHAIN) && cargo build --locked

clippy: ## cargo clippy, warnings are errors: the workspace, heartwood-crypto alone, the fuzz targets
	$(TOOLCHAIN) && cargo clippy --workspace --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-crypto --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy --manifest-path $(FUZZ_MANIFEST) --locked -- -D warnings

fmt: ## cargo fmt, the workspace and the fuzz targets
	$(TOOLCHAIN) && cargo fmt --all && cargo fmt --manifest-path $(FUZZ_MANIFEST)

fmt-check: ## cargo fmt --check, the workspace and the fuzz targets
	$(TOOLCHAIN) && cargo fmt --all -- --check && cargo fmt --manifest-path $(FUZZ_MANIFEST) -- --check

# heartwood-crypto is tested on its own, so that Cargo does not unify its features with those that
# difftest turns on: with no features (unit tests, and the doctest that the test seams are absent)
# and with every feature (every vector class). The oracle ping test of difftest is skipped unless
# test-oracle sets it up.
test: ## cargo test: heartwood-crypto with no features and with all, then difftest
	$(TOOLCHAIN) && cargo test --locked -p heartwood-crypto
	$(TOOLCHAIN) && cargo test --locked -p heartwood-crypto --all-features
	$(TOOLCHAIN) && cargo test --locked -p heartwood-difftest

release-check: ## each release package builds heartwood-crypto without its test seams
	@$(TOOLCHAIN) && for package in $(RELEASE_PACKAGES); do \
	    seams=$$(cargo tree --locked -p "$$package" -e normal,build,features -i heartwood-crypto --prefix none \
	        | grep -oE 'feature "($(subst $(space),|,$(TEST_SEAMS)))"' | sort -u | tr '\n' ' '); \
	    if [ -n "$$seams" ]; then echo "release-check: $$package enables $$seams" >&2; exit 1; fi; \
	    echo "release-check: $$package builds heartwood-crypto without $(TEST_SEAMS)"; \
	done

oracle-context: ## the image's build context in target/oracle-context (git archive of COMMIT)
	tools/oracle/make-context.sh --repo '$(SOLAR_REPO)' --commit '$(COMMIT)'

oracle-image: ## docker build --no-cache of the oracle image for COMMIT
	$(need-image)
	tools/oracle/build-image.sh --repo '$(SOLAR_REPO)' --commit '$(COMMIT)' --tag '$(IMAGE)'

oracle-save: ## docker save IMAGE to IMAGE_DIR (git-ignored under target/), gzip-compressed
	$(need-image)
	mkdir -p $(IMAGE_DIR)
	$(DIFFTEST_DOCKER) save $(IMAGE) | gzip > $(IMAGE_DIR)/heartwood-oracle-$(SHORT).tar.gz
	sha256sum $(IMAGE_DIR)/heartwood-oracle-$(SHORT).tar.gz

vectors: build ## L0 vectors from the oracle image into VECTORS/container
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) oracle gen --image $(IMAGE) --out $(VECTORS)/container $(GENERATOR_ARG)

vectors-local: build ## L0 vectors from LOCAL_SOLAR into VECTORS/local
	$(TOOLCHAIN) && $(DIFFTEST) oracle gen --solar-dir '$(LOCAL_SOLAR)' --out $(VECTORS)/local $(GENERATOR_ARG)

vectors-diff: build ## the container and local runs must agree on every record
	$(DIFFTEST) vectors diff $(VECTORS)/container $(VECTORS)/local

# A run that lacks a class the committed manifest lists is refused, and the committed files are kept:
# the genesis class (V10) is written only with DEVNET_GENERATOR.
vectors-install: vectors-diff ## copy the container run (vector files and manifest) into CRYPTO_VECTORS
	@missing=; \
	if [ -f $(CRYPTO_VECTORS)/MANIFEST.sha256 ]; then \
	    for file in $$(awk '{ print $$2 }' $(CRYPTO_VECTORS)/MANIFEST.sha256); do \
	        [ -f "$(VECTORS)/container/$$file" ] || missing="$$missing $$file"; \
	    done; \
	fi; \
	if [ -n "$$missing" ]; then \
	    echo "vectors-install: the run in $(VECTORS)/container lacks$$missing, which the committed" \
	         "manifest lists (V10-genesis needs DEVNET_GENERATOR); nothing was installed" >&2; \
	    exit 1; \
	fi
	mkdir -p $(CRYPTO_VECTORS)
	rm -f $(CRYPTO_VECTORS)/*.jsonl $(CRYPTO_VECTORS)/MANIFEST.sha256
	cp $(VECTORS)/container/*.jsonl $(VECTORS)/container/MANIFEST.sha256 $(CRYPTO_VECTORS)/
	$(DIFFTEST) vectors verify $(CRYPTO_VECTORS)

l0: build ## compare VECTORS/container with Heartwood's subjects
	$(DIFFTEST) run l0 $(VECTORS)/container

ping: build ## oracle-server.js answers, in the image and locally
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) oracle ping --image $(IMAGE) && $(DIFFTEST) oracle ping --solar-dir '$(LOCAL_SOLAR)'

test-oracle: ## the oracle ping test of cargo test, against IMAGE and LOCAL_SOLAR
	$(need-image)
	$(TOOLCHAIN) && DIFFTEST_TEST_IMAGE=$(IMAGE) DIFFTEST_TEST_SOLAR_DIR='$(LOCAL_SOLAR)' \
	    cargo test --locked --lib ping_configured_oracles -- --nocapture

e2e: vectors vectors-local vectors-diff l0 ## end to end: generate, diff, compare (needs oracle-image first)
