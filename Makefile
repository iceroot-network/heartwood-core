# Heartwood Core: Rust checks and the differential-test workflow. `make help` lists the targets.
#
#   make check          build, clippy (warnings are errors), fmt check and tests of the workspace
#   make oracle-image   build the oracle image for COMMIT from scratch (needs Docker and the network)
#   make e2e            vectors in the container and locally, diff the two runs, run L0
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
COMMIT        ?= s1-ref-v1
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
DIFFTEST      := target/debug/difftest

export DIFFTEST_DOCKER ?= $(CURDIR)/tools/oracle/docker.sh

# Stops a recipe that needs IMAGE when COMMIT cannot be resolved in SOLAR_REPO and IMAGE is not set.
need-image = $(if $(SHORT)$(filter-out heartwood-oracle:,$(IMAGE)),,$(error cannot resolve COMMIT=$(COMMIT) in SOLAR_REPO=$(SOLAR_REPO): set SOLAR_REPO to a clone of iceroot-network/solar-core-ref (in local.mk or on the command line), or set IMAGE))

.PHONY: help check build clippy fmt fmt-check test oracle-context oracle-image oracle-save \
        vectors vectors-local vectors-diff l0 ping test-oracle e2e

help: ## this list
	@grep -hE '^[a-z0-9-]+:.*## ' $(MAKEFILE_LIST) | sed -E 's/:.*## /\t/' | awk -F'\t' '{ printf "  make %-15s %s\n", $$1, $$2 }'
	@echo "Variables: SOLAR_REPO=$(SOLAR_REPO) COMMIT=$(COMMIT) IMAGE=$(IMAGE) LOCAL_SOLAR=$(LOCAL_SOLAR)"
	@echo "           VECTORS=$(VECTORS) IMAGE_DIR=$(IMAGE_DIR) TOOLCHAIN_ENV=$(TOOLCHAIN_ENV)"

check: build clippy fmt-check test ## build, clippy, fmt check and tests

build: ## cargo build (the difftest binary)
	$(TOOLCHAIN) && cargo build --locked

clippy: ## cargo clippy, warnings are errors
	$(TOOLCHAIN) && cargo clippy --all-targets --locked -- -D warnings

fmt: ## cargo fmt
	$(TOOLCHAIN) && cargo fmt --all

fmt-check: ## cargo fmt --check
	$(TOOLCHAIN) && cargo fmt --all -- --check

test: ## cargo test (the oracle ping test is skipped unless test-oracle sets it up)
	$(TOOLCHAIN) && cargo test --locked

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
	$(TOOLCHAIN) && $(DIFFTEST) oracle gen --image $(IMAGE) --out $(VECTORS)/container

vectors-local: build ## L0 vectors from LOCAL_SOLAR into VECTORS/local
	$(TOOLCHAIN) && $(DIFFTEST) oracle gen --solar-dir '$(LOCAL_SOLAR)' --out $(VECTORS)/local

vectors-diff: build ## the container and local runs must agree on every record
	$(DIFFTEST) vectors diff $(VECTORS)/container $(VECTORS)/local

l0: build ## compare VECTORS/container with Heartwood (0 compared until the first subjects exist)
	$(DIFFTEST) run l0 $(VECTORS)/container

ping: build ## oracle-server.js answers, in the image and locally
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) oracle ping --image $(IMAGE) && $(DIFFTEST) oracle ping --solar-dir '$(LOCAL_SOLAR)'

test-oracle: ## the oracle ping test of cargo test, against IMAGE and LOCAL_SOLAR
	$(need-image)
	$(TOOLCHAIN) && DIFFTEST_TEST_IMAGE=$(IMAGE) DIFFTEST_TEST_SOLAR_DIR='$(LOCAL_SOLAR)' \
	    cargo test --locked --lib ping_configured_oracles -- --nocapture

e2e: vectors vectors-local vectors-diff l0 ## end to end: generate, diff, compare (needs oracle-image first)
