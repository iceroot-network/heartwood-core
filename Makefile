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
# The devnet generator, gen-network.js, which the genesis class (V10) runs: a separate tool of the
# maintainers, not published. When it is empty, V10 is not written (vectors-install then keeps the
# committed V10); its directory is mounted read-only in container runs.
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
# the genesis class (V10) is written only with DEVNET_GENERATOR. The wire class (WIRE_CLASS) is left
# out, with its manifest line: vectors-install-wire puts it beside its crate.
vectors-install: vectors-diff ## copy the container run (vector files and manifest) into CRYPTO_VECTORS, but WIRE_CLASS
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
	for file in $(VECTORS)/container/*.jsonl; do \
	    [ "$${file##*/}" = $(WIRE_CLASS).jsonl ] || cp "$$file" $(CRYPTO_VECTORS)/ || exit 1; \
	done
	grep -v '  $(WIRE_CLASS)\.jsonl$$' $(VECTORS)/container/MANIFEST.sha256 > $(CRYPTO_VECTORS)/MANIFEST.sha256
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

# ---- heartwood-p2p-wire: the peer-to-peer wire codec ----
# Its golden vectors (class WIRE_CLASS) live beside the crate, with their own manifest.
WIRE_VECTORS  ?= packages/p2p-wire/tests/vectors
WIRE_CLASS    := V13-p2p-wire
WIRE_FUZZ     := nes_frame wire_message status_verify
# The oracle operation that replays each wire fuzz target's corpus.
WIRE_REPLAY   := nes_frame=nes.parse wire_message=wire.decodeSelected status_verify=status.verify
FUZZ_SECONDS  ?= 600
FUZZ_FORK     ?= 8
# The vector file that seeds the wire corpora, and the longest seed in bytes.
WIRE_SEEDS    ?= $(WIRE_VECTORS)/$(WIRE_CLASS).jsonl
WIRE_SEED_MAX ?= 65536
# The corpora (git-ignored), and the logs of fuzz-wire and replay-wire.
FUZZ_CORPUS   := fuzz/corpus
WIRE_LOGS     ?= target/wire-fuzz
# libFuzzer's options for every wire target: in fork mode a timeout or an out-of-memory stop ends
# the run and fails it, as a crash does.
WIRE_LIBFUZZER = -fork=$(FUZZ_FORK) -max_total_time=$(FUZZ_SECONDS) -rss_limit_mb=4096 -timeout=30 \
                 -ignore_timeouts=0 -ignore_ooms=0
# Where the oracle of replay-wire runs: the image, or ORACLE_ARGS='--solar-dir $(LOCAL_SOLAR)'.
ORACLE_ARGS   ?= --image $(IMAGE)
# The only packages that may enable the legacy-schnorr feature of heartwood-crypto.
WIRE_SCHNORR  := heartwood-difftest heartwood-p2p-wire
RELEASE_PACKAGES += heartwood-p2p-wire

.PHONY: test-wire wire-features vectors-install-wire fuzz-wire replay-wire

check: test-wire wire-features

test-wire: ## heartwood-p2p-wire alone: clippy (warnings are errors) and cargo test; it has no features
	$(TOOLCHAIN) && cargo clippy -p heartwood-p2p-wire --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo test --locked -p heartwood-p2p-wire

wire-features: ## only WIRE_SCHNORR enable the legacy-schnorr feature of heartwood-crypto
	@$(TOOLCHAIN) && set -o pipefail && enablers=$$(cargo tree --locked --workspace -e features,normal,build -i heartwood-crypto \
	        --prefix depth | awk '/^1/ { on = /feature "legacy-schnorr"/ } on && /^2/ { sub(/^2/, ""); print $$1 }' \
	        | sort -u | tr '\n' ' ') || exit 1; \
	if [ "$$enablers" != "$(WIRE_SCHNORR) " ]; then \
	    echo "wire-features: legacy-schnorr is enabled by $$enablers, not by $(WIRE_SCHNORR) only" >&2; exit 1; \
	fi; \
	echo "wire-features: legacy-schnorr is enabled by $(WIRE_SCHNORR) only"

vectors-install-wire: vectors-diff ## copy WIRE_CLASS of the container run, with its manifest line, into WIRE_VECTORS
	@[ -f $(VECTORS)/container/$(WIRE_CLASS).jsonl ] || { \
	    echo "vectors-install-wire: the run in $(VECTORS)/container has no $(WIRE_CLASS).jsonl;" \
	         "nothing was installed" >&2; exit 1; }
	mkdir -p $(WIRE_VECTORS)
	rm -f $(WIRE_VECTORS)/*.jsonl $(WIRE_VECTORS)/MANIFEST.sha256
	cp $(VECTORS)/container/$(WIRE_CLASS).jsonl $(WIRE_VECTORS)/
	grep '  $(WIRE_CLASS)\.jsonl$$' $(VECTORS)/container/MANIFEST.sha256 > $(WIRE_VECTORS)/MANIFEST.sha256
	$(DIFFTEST) vectors verify $(WIRE_VECTORS)

# The three targets run at the same time, FUZZ_FORK jobs each, on corpora seeded from WIRE_SEEDS
# (when the file exists). Each log's last status line gives the runs, the coverage and the corpus.
fuzz-wire: ## the wire fuzz targets at once, FUZZ_SECONDS each (nightly and cargo-fuzz); logs in WIRE_LOGS
	@$(TOOLCHAIN) && if [ -f '$(WIRE_SEEDS)' ]; then \
	    node tools/wire-fuzz-seeds.js '$(WIRE_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-wire: no seeds: $(WIRE_SEEDS) does not exist"; \
	fi
	$(TOOLCHAIN) && for target in $(WIRE_FUZZ); do cargo +nightly fuzz build $$target || exit 1; done
	@$(TOOLCHAIN) && mkdir -p $(WIRE_LOGS) && pids=; \
	for target in $(WIRE_FUZZ); do \
	    mkdir -p $(FUZZ_CORPUS)/$$target; \
	    cargo +nightly fuzz run $$target -- $(WIRE_LIBFUZZER) > $(WIRE_LOGS)/$$target.log 2>&1 & pids="$$pids $$!"; \
	done; \
	echo "fuzz-wire: $(WIRE_FUZZ) running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs each; logs in $(WIRE_LOGS)"; \
	fail=0; set -- $(WIRE_FUZZ); \
	for pid in $$pids; do \
	    target=$$1; shift; \
	    wait $$pid || { echo "fuzz-wire: $$target stopped with a failure, see $(WIRE_LOGS)/$$target.log" >&2; fail=1; }; \
	    last=$$(grep -E '^#[0-9]+: cov: ' $(WIRE_LOGS)/$$target.log | tail -n 1); \
	    files=$$(find $(FUZZ_CORPUS)/$$target -type f | wc -l); \
	    size=$$(du -sk $(FUZZ_CORPUS)/$$target | cut -f1); \
	    echo "fuzz-wire: $$target: $${last:-no status line}; corpus $$files files, $$size KiB"; \
	    case "$$last" in *'oom/timeout/crash: 0/0/0 '*) ;; *) fail=1 ;; esac; \
	done; \
	exit $$fail

# Every input of a corpus must be compared: an empty corpus, a skipped input or one without a
# subject fails too.
replay-wire: build ## each wire fuzz corpus through the oracle (ORACLE_ARGS) and Heartwood; logs in WIRE_LOGS
	$(need-image)
	@$(TOOLCHAIN) && mkdir -p $(WIRE_LOGS) && fail=0; \
	for pair in $(WIRE_REPLAY); do \
	    target=$${pair%%=*}; op=$${pair#*=}; log=$(WIRE_LOGS)/replay-$$target.log; \
	    if [ -z "$$(ls -A $(FUZZ_CORPUS)/$$target 2>/dev/null)" ]; then \
	        echo "replay-wire: $(FUZZ_CORPUS)/$$target is missing or empty (make fuzz-wire)" >&2; fail=1; continue; \
	    fi; \
	    $(DIFFTEST) oracle replay $$op $(FUZZ_CORPUS)/$$target --vectors $(CRYPTO_VECTORS) $(ORACLE_ARGS) \
	        > $$log 2>&1; status=$$?; cat $$log; \
	    if [ $$status != 0 ]; then \
	        echo "replay-wire: $$target: the replay failed (exit $$status)" >&2; fail=1; \
	    elif grep -qE '^  [0-9]+ (skipped|records have no Rust implementation)' $$log; then \
	        echo "replay-wire: $$target: some inputs were not compared" >&2; fail=1; \
	    fi; \
	done; \
	exit $$fail
# ---- end of heartwood-p2p-wire ----
