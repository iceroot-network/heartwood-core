# Heartwood Core: Rust checks and the differential-test workflow. `make help` lists the targets.
#
#   make check          build, clippy (warnings are errors), fmt check, tests, the release check,
#                       the crate layers, and the review ceilings where tokei is installed
#   make oracle-image   build the oracle image for COMMIT from scratch (needs Docker and the network)
#   make e2e            vectors in the container and locally, diff the two runs, run L0
#   make vectors-install  copy the container run into each crate's tests/vectors
#   make l1             the L1 properties against the oracle
#   make corpus-import corpus-judge l2 l2-synthetic   the L2 chains: import, judge, replay
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
# The devnet generator, gen-network.js, which the genesis class (V10) runs: a separate tool of the
# maintainers, not published. When it is empty, V10 is not written (vectors-install then keeps the
# committed V10); its directory is mounted read-only in container runs.
DEVNET_GENERATOR ?=
GENERATOR_ARG := $(if $(DEVNET_GENERATOR),--devnet-generator '$(DEVNET_GENERATOR)',)
# The vector classes of each crate: crate=class prefixes. vectors-install writes each crate's
# classes, with their own MANIFEST.sha256, into packages/<crate>/tests/vectors.
VECTOR_CRATES ?= crypto=V01,V02,V03,V04,V05,V06,V07,V10,V11,V12 kernel=V09-rounds,V16 logger=V20 \
                 state=V08,V09-shuffle,V15,V18 transactions=V17,V19
# L1: cases per property, the generator's seed, the properties (P1 to P9 or their names, comma
# separated; all when empty).
L1_CASES      ?= 256
L1_SEED       ?= 1
L1_PROPERTY   ?=
# L2: the corpus directory CORPUS/NAME; ARCHIVE the devnet archives to import (in chain order);
# CORPUS_SEED the chain's public key-derivation seed (difftest's default when empty); DUMP heights
# whose records the judge writes.
CORPUS        ?= target/corpus
NAME          ?=
ARCHIVE       ?=
CORPUS_SEED   ?=
DUMP          ?=
# Where the oracle of l1, corpus-judge and l2-synthetic runs: the image by default, or
# ORACLE_ARGS='--solar-dir $(LOCAL_SOLAR)' for a local reference.
ORACLE_ARGS   ?= --image $(IMAGE)
DIFFTEST      := target/debug/difftest
# The packages whose builds are release artefacts, each built on its own (cargo build -p). None of
# them may enable a test seam (SEAMS, crate:feature); `make release-check` checks it.
RELEASE_PACKAGES ?= heartwood-crypto heartwood-kernel heartwood-logger heartwood-state heartwood-transactions
SEAMS         := heartwood-crypto:fixed-aux heartwood-kernel:difftest
# The node's crates besides heartwood-crypto, tested and linted each on its own.
NODE_CRATES   := heartwood-kernel heartwood-logger heartwood-state heartwood-transactions
empty         :=
space         := $(empty) $(empty)
# The cargo-fuzz targets, a package of their own outside the workspace.
FUZZ_MANIFEST := fuzz/Cargo.toml

export DIFFTEST_DOCKER ?= $(CURDIR)/tools/oracle/docker.sh

# Stops a recipe that needs IMAGE when COMMIT cannot be resolved in SOLAR_REPO and IMAGE is not set.
need-image = $(if $(SHORT)$(filter-out heartwood-oracle:,$(IMAGE)),,$(error cannot resolve COMMIT=$(COMMIT) in SOLAR_REPO=$(SOLAR_REPO): set SOLAR_REPO to a clone of iceroot-network/solar-core-ref (in local.mk or on the command line), or set IMAGE))
# Stops a corpus recipe without NAME.
need-name = $(if $(NAME),,$(error set NAME, the corpus under CORPUS=$(CORPUS)))

.PHONY: help check build clippy fmt fmt-check test release-check oracle-context oracle-image \
        oracle-save vectors vectors-local vectors-diff vectors-install l0 ping test-oracle e2e \
        l1 corpus-import corpus-judge corpus-verify l2 l2-synthetic layers lines lines-if-tokei

help: ## this list
	@grep -hE '^[a-z0-9-]+:.*## ' $(MAKEFILE_LIST) | sed -E 's/:.*## /\t/' | awk -F'\t' '{ printf "  make %-15s %s\n", $$1, $$2 }'
	@echo "Variables: SOLAR_REPO=$(SOLAR_REPO) COMMIT=$(COMMIT) IMAGE=$(IMAGE) LOCAL_SOLAR=$(LOCAL_SOLAR)"
	@echo "           VECTORS=$(VECTORS) IMAGE_DIR=$(IMAGE_DIR) TOOLCHAIN_ENV=$(TOOLCHAIN_ENV)"
	@echo "           DEVNET_GENERATOR=$(DEVNET_GENERATOR) CORPUS=$(CORPUS) ORACLE_ARGS=$(ORACLE_ARGS)"

check: build clippy fmt-check test release-check layers lines-if-tokei ## build, clippy, fmt check, tests, release check, layers, ceilings

build: ## cargo build (the difftest binary)
	$(TOOLCHAIN) && cargo build --locked

clippy: ## cargo clippy, warnings are errors: the workspace, each library crate alone, the fuzz targets
	$(TOOLCHAIN) && cargo clippy --workspace --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-crypto --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && for package in $(NODE_CRATES); do \
	    cargo clippy -p "$$package" --all-targets --locked -- -D warnings && \
	    cargo clippy -p "$$package" --all-targets --all-features --locked -- -D warnings || exit 1; \
	done
	$(TOOLCHAIN) && cargo clippy --manifest-path $(FUZZ_MANIFEST) --locked -- -D warnings

fmt: ## cargo fmt, the workspace and the fuzz targets
	$(TOOLCHAIN) && cargo fmt --all && cargo fmt --manifest-path $(FUZZ_MANIFEST)

fmt-check: ## cargo fmt --check, the workspace and the fuzz targets
	$(TOOLCHAIN) && cargo fmt --all -- --check && cargo fmt --manifest-path $(FUZZ_MANIFEST) -- --check

# Each library crate is tested on its own, so that Cargo does not unify its features with those
# that difftest turns on: heartwood-crypto with no features (unit tests, and the doctest that the
# test seams are absent) and with every feature (every vector class); the kernel with and without
# its test seam. The oracle ping test of difftest is skipped unless test-oracle sets it up.
test: ## cargo test: each library crate alone, with no features and with all, then difftest
	$(TOOLCHAIN) && cargo test --locked -p heartwood-crypto
	$(TOOLCHAIN) && cargo test --locked -p heartwood-crypto --all-features
	$(TOOLCHAIN) && for package in $(NODE_CRATES); do \
	    cargo test --locked -p "$$package" && cargo test --locked -p "$$package" --all-features || exit 1; \
	done
	$(TOOLCHAIN) && cargo test --locked -p heartwood-difftest

release-check: ## each release package builds without the test seams (SEAMS)
	@$(TOOLCHAIN) && for package in $(RELEASE_PACKAGES); do \
	    for seam in $(SEAMS); do \
	        crate=$${seam%%:*}; feature=$${seam#*:}; \
	        tree=$$(cargo tree --locked -p "$$package" -e normal,build,features -i "$$crate" --prefix none 2>/dev/null) || continue; \
	        if grep -q "feature \"$$feature\"" <<<"$$tree"; then \
	            echo "release-check: $$package enables the $$feature seam of $$crate" >&2; exit 1; \
	        fi; \
	    done; \
	    echo "release-check: $$package builds without $(SEAMS)"; \
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

# Each crate gets its classes (VECTOR_CRATES) and a manifest of them. A run that lacks a class a
# committed manifest lists is refused, and the committed files are kept: the genesis class (V10)
# and the genesis-state class (V18) are written only with DEVNET_GENERATOR.
vectors-install: vectors-diff ## copy the container run into each crate's tests/vectors, with a manifest per crate
	@missing=; \
	for spec in $(VECTOR_CRATES); do \
	    dir=packages/$${spec%%=*}/tests/vectors; \
	    [ -f $$dir/MANIFEST.sha256 ] || continue; \
	    for file in $$(awk '{ print $$2 }' $$dir/MANIFEST.sha256); do \
	        [ -f "$(VECTORS)/container/$$file" ] || missing="$$missing $$file"; \
	    done; \
	done; \
	if [ -n "$$missing" ]; then \
	    echo "vectors-install: the run in $(VECTORS)/container lacks$$missing, which a committed" \
	         "manifest lists (V10 and V18 need DEVNET_GENERATOR); nothing was installed" >&2; \
	    exit 1; \
	fi
	@for spec in $(VECTOR_CRATES); do \
	    dir=packages/$${spec%%=*}/tests/vectors; \
	    files=; \
	    for class in $$(echo $${spec#*=} | tr ',' ' '); do \
	        files="$$files $$(cd $(VECTORS)/container && ls $$class*.jsonl 2>/dev/null)"; \
	    done; \
	    files=$$(echo $$files); \
	    [ -n "$$files" ] || continue; \
	    mkdir -p $$dir; \
	    rm -f $$dir/*.jsonl $$dir/MANIFEST.sha256; \
	    for f in $$files; do cp $(VECTORS)/container/$$f $$dir/; done; \
	    (cd $$dir && sha256sum $$files > MANIFEST.sha256); \
	    echo "vectors-install: $$dir: $$files"; \
	    $(DIFFTEST) vectors verify $$dir >/dev/null || exit 1; \
	done

l0: build ## compare VECTORS/container with Heartwood's subjects
	$(DIFFTEST) run l0 $(VECTORS)/container

ping: build ## oracle-server.js answers, in the image and locally
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) oracle ping --image $(IMAGE) && $(DIFFTEST) oracle ping --solar-dir '$(LOCAL_SOLAR)'

test-oracle: ## the oracle tests of cargo test (ping, 256 L1 cases per property), against IMAGE and LOCAL_SOLAR
	$(need-image)
	$(TOOLCHAIN) && DIFFTEST_TEST_IMAGE=$(IMAGE) DIFFTEST_TEST_SOLAR_DIR='$(LOCAL_SOLAR)' \
	    cargo test --locked --lib ping_configured_oracles -- --nocapture
	$(TOOLCHAIN) && DIFFTEST_TEST_IMAGE=$(IMAGE) DIFFTEST_TEST_SOLAR_DIR='$(LOCAL_SOLAR)' DIFFTEST_TEST_L1_CASES=256 \
	    cargo test --locked -p heartwood-difftest --test l1 properties_agree_with_the_configured_oracles -- --nocapture

e2e: vectors vectors-local vectors-diff l0 ## end to end: generate, diff, compare (needs oracle-image first)

l1: build ## L1 properties against the oracle (ORACLE_ARGS): L1_CASES, L1_SEED, L1_PROPERTY
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) run l1 $(if $(L1_PROPERTY),--property $(L1_PROPERTY)) \
	    --cases $(L1_CASES) --seed $(L1_SEED) --regressions target/l1-regressions $(ORACLE_ARGS)

corpus-import: build ## devnet archives (ARCHIVE='dir ...', in chain order) to the corpus CORPUS/NAME
	$(need-name)
	$(if $(ARCHIVE),,$(error set ARCHIVE, the devnet archives of the chain in order))
	$(DIFFTEST) corpus import $(ARCHIVE) --out $(CORPUS)/$(NAME) $(if $(CORPUS_SEED),--seed '$(CORPUS_SEED)')

corpus-judge: build ## the oracle's judge of CORPUS/NAME: digest lines and anchors (DUMP=H,...)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) corpus judge $(CORPUS)/$(NAME) $(if $(DUMP),--dump $(DUMP)) $(ORACLE_ARGS)

corpus-verify: build ## the manifest of CORPUS/NAME
	$(need-name)
	$(DIFFTEST) corpus verify $(CORPUS)/$(NAME)

l2: build ## replay CORPUS/NAME in Heartwood against the judge's digest lines
	$(need-name)
	$(DIFFTEST) run l2 $(CORPUS)/$(NAME)

l2-synthetic: build ## L2 of CORPUS/NAME with the synthetic blocks on top (needs the oracle)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --synthetic $(ORACLE_ARGS)

layers: ## crate edges follow the layers; no async, storage or HTTP crate in the consensus crates
	$(TOOLCHAIN) && tools/check-layers.sh

lines: ## code lines per crate against the review ceilings (tokei, inline tests excluded)
	$(TOOLCHAIN) && tools/check-lines.sh

# make check runs the ceilings only where tokei is installed; `make lines` needs it.
lines-if-tokei:
	@$(TOOLCHAIN) && if command -v tokei >/dev/null 2>&1; then tools/check-lines.sh; \
	else echo "lines: tokei is not installed, so the review ceilings are not checked (make lines)"; fi
