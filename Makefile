# Heartwood Core: Rust checks and the differential-test workflow. `make help` lists the targets.
#
#   make check          build, clippy (warnings are errors), fmt check, tests (the integration-tagged
#                       ones too), the release check, the crate layers, rustdoc (warnings are
#                       errors), and the review ceilings where tokei is installed
#   make oracle-image   build the oracle image for COMMIT from scratch (needs Docker and the network)
#   make e2e            vectors in the container and locally, diff the two runs, run L0
#   make vectors-install  copy the container run into each crate's tests/vectors
#   make l1             the L1 properties against the oracle
#   make corpus-import corpus-judge l2 l2-synthetic   the L2 chains: import, judge, replay
#   make l2-restart crash-test   the L2 chains restarted at planned heights, and killed mid-commit
#   make pool-bench     the cost of the pool's per-sender views and of the store's commit
#   make l2-processor   the L2 chains through the processor and the job, as a relay imports them
#   make relay-image live live-check api-compare   the relay on a devnet, and its comparisons
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
VECTOR_CRATES ?= crypto=V01,V02,V03,V04,V05,V06,V07,V10,V11,V12,V19,V22 kernel=V09-rounds,V16,V23,V28 \
                 logger=V20 pool=V21 state=V08,V09-shuffle,V15,V18 transactions=V17 p2p-wire=V13,V24 \
                 p2p=V25,V26 blockchain=V27 api=V28
# L1: cases per property, the generator's seed, the properties (P1 to P16 or their names, comma
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
# L2 restarts and the store: TO cuts the corpus at a height (the judge and the replays must use
# the same cut); RESTARTS is `every` (a restart at every height) or `rounds` (the round ends, the
# first blocks of rounds, 20 seeded heights and the node's live restarts), which corpus-judge writes
# into the corpus's restart-plan.json before it judges; KILLS is the crash test's number of kills,
# KILL_SEED the seed of its delays. PROCESSOR (any value) makes corpus-judge also record each
# block's result through the reference's job; RUNTIME (any value) makes crash-test kill a relay
# that imports the corpus from a mock peer.
TO            ?=
RESTARTS      ?=
KILLS         ?= 200
KILL_SEED     ?=
PROCESSOR     ?=
RUNTIME       ?=
# Where the oracle of l1, corpus-judge and l2-synthetic runs: the image by default, or
# ORACLE_ARGS='--solar-dir $(LOCAL_SOLAR)' for a local reference.
ORACLE_ARGS   ?= --image $(IMAGE)
DIFFTEST      := target/debug/difftest
# The comparing targets (l0, l1, the L2 replays, the kill test, live-check, api-compare) are gates:
# `--strict` fails a run in which a case was skipped or had no subject, as a failed case does.
# STRICT= runs them leniently, for a look at a partial tree.
STRICT        ?= --strict
# The packages whose builds are release artefacts, each built on its own (cargo build -p). None of
# them may enable a test seam (SEAMS, crate:feature); `make release-check` checks it.
RELEASE_PACKAGES ?= heartwood-crypto heartwood-kernel heartwood-logger heartwood-state heartwood-transactions \
                    heartwood-database heartwood-pool heartwood-blockchain heartwood-api heartwood-core
SEAMS         := heartwood-crypto:fixed-aux heartwood-kernel:difftest heartwood-blockchain:difftest \
                 heartwood-core:difftest
# The node's crates besides heartwood-crypto, tested and linted each on its own.
NODE_CRATES   := heartwood-kernel heartwood-logger heartwood-state heartwood-transactions \
                 heartwood-database heartwood-pool heartwood-blockchain heartwood-api heartwood-core
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
        l1 corpus-import corpus-judge corpus-verify l2 l2-synthetic l2-restart crash-test pool-bench \
        layers lines lines-if-tokei l2-processor relay-image live live-check api-compare doc \
        test-integration

help: ## this list
	@grep -hE '^[a-z0-9-]+:.*## ' $(MAKEFILE_LIST) | sed -E 's/:.*## /\t/' | awk -F'\t' '{ printf "  make %-15s %s\n", $$1, $$2 }'
	@echo "Variables: SOLAR_REPO=$(SOLAR_REPO) COMMIT=$(COMMIT) IMAGE=$(IMAGE) LOCAL_SOLAR=$(LOCAL_SOLAR)"
	@echo "           VECTORS=$(VECTORS) IMAGE_DIR=$(IMAGE_DIR) TOOLCHAIN_ENV=$(TOOLCHAIN_ENV)"
	@echo "           DEVNET_GENERATOR=$(DEVNET_GENERATOR) CORPUS=$(CORPUS) ORACLE_ARGS=$(ORACLE_ARGS)"

check: build clippy fmt-check test test-integration release-check layers doc lines-if-tokei ## build, clippy, fmt check, tests, release check, layers, rustdoc, ceilings

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

# The tests marked `#[ignore = "integration"]` (the served API over a store, a relay importing from
# a mock peer, the kill test through the runtime, ...): each by its exact name, with its crate's
# features; a marked test that its target does not list fails.
test-integration: build ## the integration-tagged tests of every crate
	$(TOOLCHAIN) && tools/test-integration.sh

# Every crate's documentation, with every feature: a warning (a link to a private item, a broken
# link) fails.
doc: ## rustdoc of the workspace with every feature, warnings are errors
	$(TOOLCHAIN) && RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --locked --workspace --all-features

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

# Each crate gets its classes (VECTOR_CRATES) and a manifest of them. A run that lacks a class
# VECTOR_CRATES routes, or a file a committed manifest lists, is refused, and the committed files
# are kept: the genesis class (V10) and the genesis-state class (V18) are written only with
# DEVNET_GENERATOR.
vectors-install: vectors-diff ## copy the container run into each crate's tests/vectors, with a manifest per crate
	@missing=; \
	for spec in $(VECTOR_CRATES); do \
	    for class in $$(echo $${spec#*=} | tr ',' ' '); do \
	        ls $(VECTORS)/container/$$class*.jsonl >/dev/null 2>&1 || missing="$$missing $$class*.jsonl"; \
	    done; \
	    dir=packages/$${spec%%=*}/tests/vectors; \
	    [ -f $$dir/MANIFEST.sha256 ] || continue; \
	    for file in $$(awk '{ print $$2 }' $$dir/MANIFEST.sha256); do \
	        [ -f "$(VECTORS)/container/$$file" ] || missing="$$missing $$file"; \
	    done; \
	done; \
	if [ -n "$$missing" ]; then \
	    echo "vectors-install: the run in $(VECTORS)/container lacks$$missing, which VECTOR_CRATES" \
	         "routes or a committed manifest lists (V10 and V18 need DEVNET_GENERATOR); nothing was" \
	         "installed" >&2; \
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
	$(DIFFTEST) run l0 $(VECTORS)/container $(STRICT)

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

corpus-judge: build ## the oracle's judge of CORPUS/NAME: digest lines, anchors, restart lines (DUMP, RESTARTS, TO, PROCESSOR)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) corpus judge $(CORPUS)/$(NAME) $(if $(DUMP),--dump $(DUMP)) \
	    $(if $(RESTARTS),--restarts $(RESTARTS)) $(if $(TO),--to $(TO)) $(if $(PROCESSOR),--processor) \
	    $(ORACLE_ARGS)

corpus-verify: build ## the manifest of CORPUS/NAME
	$(need-name)
	$(DIFFTEST) corpus verify $(CORPUS)/$(NAME)

l2: build ## replay CORPUS/NAME in Heartwood through its store against the judge's lines (TO=H)
	$(need-name)
	$(DIFFTEST) run l2 $(CORPUS)/$(NAME) $(if $(TO),--to $(TO)) $(STRICT)

l2-synthetic: build ## L2 of CORPUS/NAME with the synthetic restart, pool and processor cases on top (needs the oracle)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --synthetic $(if $(TO),--to $(TO)) $(STRICT) $(ORACLE_ARGS)
	$(TOOLCHAIN) && $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --processor --synthetic $(if $(TO),--to $(TO)) $(STRICT) \
	    $(ORACLE_ARGS)

l2-restart: build ## L2 of CORPUS/NAME with the restarts of its restart plan, by both of Heartwood's paths (TO=H)
	$(need-name)
	$(DIFFTEST) run l2 $(CORPUS)/$(NAME) --restart $(if $(TO),--to $(TO)) $(STRICT)

crash-test: build ## CORPUS/NAME replayed through the store (or a relay, RUNTIME=1) in a child process killed KILLS times
	$(need-name)
	$(DIFFTEST) store crash $(CORPUS)/$(NAME) --kills $(KILLS) $(if $(KILL_SEED),--seed $(KILL_SEED)) \
	    $(if $(RUNTIME),--runtime) $(STRICT)

pool-bench: ## the cost of the pool's per-sender views and of the store's commit (release build)
	$(TOOLCHAIN) && cargo test --release --locked -p heartwood-pool --test view_cost -- --ignored --nocapture
	$(TOOLCHAIN) && cargo test --release --locked -p heartwood-blockchain --test commit_cost -- --ignored --nocapture

layers: ## crate edges follow the layers; no async, storage or HTTP crate in the consensus crates
	$(TOOLCHAIN) && tools/check-layers.sh

lines: ## code lines per crate against the review ceilings (tokei, inline tests excluded)
	$(TOOLCHAIN) && tools/check-lines.sh

# make check runs the ceilings only where tokei is installed; `make lines` needs it.
lines-if-tokei:
	@$(TOOLCHAIN) && if command -v tokei >/dev/null 2>&1; then tools/check-lines.sh; \
	else echo "lines: tokei is not installed, so the review ceilings are not checked (make lines)"; fi

# ---- heartwood-p2p-wire: the peer-to-peer wire codec ----
# Its golden vectors (class WIRE_CLASS) live beside the crate, with their own manifest; the
# p2p-wire row of VECTOR_CRATES installs them.
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
# The only packages that may enable the legacy-schnorr feature of heartwood-crypto.
WIRE_SCHNORR  := heartwood-difftest heartwood-p2p-wire
RELEASE_PACKAGES += heartwood-p2p-wire

.PHONY: test-wire wire-features fuzz-wire replay-wire

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
# subject fails too. The devnet configuration comes from the genesis class, with heartwood-crypto's
# vectors.
replay-wire: build ## each wire fuzz corpus through the oracle (ORACLE_ARGS) and Heartwood; logs in WIRE_LOGS
	$(need-image)
	@$(TOOLCHAIN) && mkdir -p $(WIRE_LOGS) && fail=0; \
	for pair in $(WIRE_REPLAY); do \
	    target=$${pair%%=*}; op=$${pair#*=}; log=$(WIRE_LOGS)/replay-$$target.log; \
	    if [ -z "$$(ls -A $(FUZZ_CORPUS)/$$target 2>/dev/null)" ]; then \
	        echo "replay-wire: $(FUZZ_CORPUS)/$$target is missing or empty (make fuzz-wire)" >&2; fail=1; continue; \
	    fi; \
	    $(DIFFTEST) oracle replay $$op $(FUZZ_CORPUS)/$$target --vectors packages/crypto/tests/vectors $(ORACLE_ARGS) \
	        > $$log 2>&1; status=$$?; cat $$log; \
	    if [ $$status != 0 ]; then \
	        echo "replay-wire: $$target: the replay failed (exit $$status)" >&2; fail=1; \
	    elif grep -qE '^  [0-9]+ (skipped|records have no Rust implementation)' $$log; then \
	        echo "replay-wire: $$target: some inputs were not compared" >&2; fail=1; \
	    fi; \
	done; \
	exit $$fail
# ---- end of heartwood-p2p-wire ----

# ---- heartwood-p2p: the client side of the peer protocol ----
# Its vector classes, and the wire's new one, are installed once an oracle image writes them.
# Until then the crate tests read a class that is not installed from the directory that
# P2P_VECTORS (heartwood-p2p and difftest's network_vectors test) or WIRE_VECTORS
# (heartwood-p2p-wire) names, relative to the workspace root, such as the run of
# `make vectors-local`; without either the test reports the class as not installed and passes.
P2P_FUZZ      := nes_session reply_check listed_block
# The oracle operation that replays each p2p fuzz target's corpus; the session's has none.
P2P_REPLAY    := reply_check=wire.checkReply listed_block=block.fromListed
# The vector run whose records seed the corpora of reply_check and listed_block.
P2P_SEEDS     ?= $(VECTORS)/local
P2P_LOGS      ?= target/p2p-fuzz
RELEASE_PACKAGES += heartwood-p2p
SEAMS         += heartwood-p2p:difftest

.PHONY: test-p2p fuzz-p2p replay-p2p

check: test-p2p

test-p2p: ## heartwood-p2p alone: clippy (warnings are errors) and cargo test, without and with its features
	$(TOOLCHAIN) && cargo clippy -p heartwood-p2p --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-p2p --all-targets --all-features --locked -- -D warnings
	$(TOOLCHAIN) && cargo test --locked -p heartwood-p2p
	$(TOOLCHAIN) && cargo test --locked -p heartwood-p2p --all-features

# The three targets run at the same time, FUZZ_FORK jobs each, with the wire targets' libFuzzer
# options; reply_check and listed_block are seeded from the records of P2P_SEEDS when it exists.
fuzz-p2p: ## the p2p fuzz targets at once, FUZZ_SECONDS each (nightly and cargo-fuzz); logs in P2P_LOGS
	@$(TOOLCHAIN) && if [ -d '$(P2P_SEEDS)' ]; then \
	    node tools/p2p-fuzz-seeds.js '$(P2P_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-p2p: no seeds: $(P2P_SEEDS) does not exist"; \
	fi
	$(TOOLCHAIN) && for target in $(P2P_FUZZ); do cargo +nightly fuzz build $$target || exit 1; done
	@$(TOOLCHAIN) && mkdir -p $(P2P_LOGS) && pids=; \
	for target in $(P2P_FUZZ); do \
	    mkdir -p $(FUZZ_CORPUS)/$$target; \
	    cargo +nightly fuzz run $$target -- $(WIRE_LIBFUZZER) > $(P2P_LOGS)/$$target.log 2>&1 & pids="$$pids $$!"; \
	done; \
	echo "fuzz-p2p: $(P2P_FUZZ) running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs each; logs in $(P2P_LOGS)"; \
	fail=0; set -- $(P2P_FUZZ); \
	for pid in $$pids; do \
	    target=$$1; shift; \
	    wait $$pid || { echo "fuzz-p2p: $$target stopped with a failure, see $(P2P_LOGS)/$$target.log" >&2; fail=1; }; \
	    last=$$(grep -E '^#[0-9]+: cov: ' $(P2P_LOGS)/$$target.log | tail -n 1); \
	    files=$$(find $(FUZZ_CORPUS)/$$target -type f | wc -l); \
	    size=$$(du -sk $(FUZZ_CORPUS)/$$target | cut -f1); \
	    echo "fuzz-p2p: $$target: $${last:-no status line}; corpus $$files files, $$size KiB"; \
	    case "$$last" in *'oom/timeout/crash: 0/0/0 '*) ;; *) fail=1 ;; esac; \
	done; \
	exit $$fail

# Every input of a corpus must be compared, as in replay-wire.
replay-p2p: build ## the reply_check and listed_block corpora through the oracle (ORACLE_ARGS) and Heartwood; logs in P2P_LOGS
	$(need-image)
	@$(TOOLCHAIN) && mkdir -p $(P2P_LOGS) && fail=0; \
	for pair in $(P2P_REPLAY); do \
	    target=$${pair%%=*}; op=$${pair#*=}; log=$(P2P_LOGS)/replay-$$target.log; \
	    if [ -z "$$(ls -A $(FUZZ_CORPUS)/$$target 2>/dev/null)" ]; then \
	        echo "replay-p2p: $(FUZZ_CORPUS)/$$target is missing or empty (make fuzz-p2p)" >&2; fail=1; continue; \
	    fi; \
	    $(DIFFTEST) oracle replay $$op $(FUZZ_CORPUS)/$$target --vectors packages/crypto/tests/vectors $(ORACLE_ARGS) \
	        > $$log 2>&1; status=$$?; cat $$log; \
	    if [ $$status != 0 ]; then \
	        echo "replay-p2p: $$target: the replay failed (exit $$status)" >&2; fail=1; \
	    elif grep -qE '^  [0-9]+ (skipped|records have no Rust implementation)' $$log; then \
	        echo "replay-p2p: $$target: some inputs were not compared" >&2; fail=1; \
	    fi; \
	done; \
	exit $$fail
# ---- end of heartwood-p2p ----

# ---- the relay: the processor, the runtime, the API and the node process ----
# The live runs are orchestrated by the maintainers' devnet tooling, a separate tool that is not
# published: LIVE_RECIPE (run, steps, archives and the API corpus at the pause, for RUN=r, h, m1 or
# m2), RELAY_IMAGE_RECIPE (the relay's container image from a binary) and LIVE_RESULTS (the run
# directories) are set in local.mk. The comparisons of a run are the harness's (live check, api
# compare). CHUNKS names one job cut of l2-processor (reference or 1; both when empty); LEFT and
# RIGHT are two nodes' API URLs for a direct api-compare.
LIVE_RECIPE        ?=
RELAY_IMAGE_RECIPE ?=
LIVE_RESULTS       ?=
RUN                ?=
CHUNKS             ?=
LEFT               ?=
RIGHT              ?=
API_COMPARE_OUT    ?= target/api-compare
need-live = $(if $(LIVE_RECIPE),,$(error set LIVE_RECIPE, the devnet tooling's live recipe, in local.mk))
need-run = $(if $(RUN),,$(error set RUN, the live run: r, h, m1 or m2))
need-results = $(if $(LIVE_RESULTS),,$(error set LIVE_RESULTS, the live runs' directory, in local.mk))

l2-processor: build ## replay CORPUS/NAME through the processor and the job against the judge's lines (TO, CHUNKS)
	$(need-name)
	$(DIFFTEST) run l2 $(CORPUS)/$(NAME) --processor $(if $(CHUNKS),--chunks $(CHUNKS)) $(if $(TO),--to $(TO)) \
	    $(STRICT)

relay-image: ## the relay's release binary with the harness's flags, and its devnet image (RELAY_IMAGE_RECIPE)
	$(if $(RELAY_IMAGE_RECIPE),,$(error set RELAY_IMAGE_RECIPE, the devnet tooling's image recipe, in local.mk))
	$(TOOLCHAIN) && cargo build --release --locked -p heartwood-core --features difftest
	$(RELAY_IMAGE_RECIPE) $(CURDIR)/target/release/heartwood

live: ## one live run of at most 5 rounds on a devnet (RUN=r|h|m1|m2, LIVE_RECIPE)
	$(need-live)
	$(need-run)
	$(LIVE_RECIPE) $(RUN) $(CURDIR)

live-check: build ## the comparisons of a live run (RUN, LIVE_RESULTS)
	$(need-run)
	$(need-results)
	$(TOOLCHAIN) && $(DIFFTEST) live check $(LIVE_RESULTS)/$(RUN) $(STRICT)

api-compare: build ## judge the API answers a live run recorded (RUN, LIVE_RESULTS), or two nodes' (LEFT, RIGHT)
	$(TOOLCHAIN) && if [ -n '$(LEFT)' ] && [ -n '$(RIGHT)' ]; then \
	    $(DIFFTEST) api compare --left '$(LEFT)' --right '$(RIGHT)' --out $(API_COMPARE_OUT) $(STRICT); \
	else \
	    [ -n '$(RUN)' ] && [ -n '$(LIVE_RESULTS)' ] || { echo "api-compare: set RUN and LIVE_RESULTS, or LEFT and RIGHT" >&2; exit 2; }; \
	    $(DIFFTEST) api compare --run $(LIVE_RESULTS)/$(RUN) $(STRICT); \
	fi
# ---- end of the relay ----
