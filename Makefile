# Heartwood Core: Rust checks and the differential-test workflow. `make help` lists the targets.
#
#   make check          build, clippy (warnings are errors), fmt check, tests (the integration-tagged
#                       ones too), the release check, the crate layers, rustdoc (warnings are
#                       errors), and the review ceilings where tokei is installed
#   make deny           advisories, licences, sources and bans of every dependency (cargo-deny;
#                       needs the network, so it is not part of make check)
#   make oracle-image   build the oracle image for COMMIT from scratch (needs Docker and the network)
#   make e2e            vectors in the container and locally, diff the two runs, run L0
#   make vectors-install  copy the container run into each crate's tests/vectors
#   make l1             the L1 properties against the oracle
#   make corpus-import corpus-judge l2 l2-synthetic   the L2 chains: import, judge, replay
#   make l2-restart crash-test   the L2 chains restarted at planned heights, and killed mid-commit
#   make pool-bench     the cost of the pool's per-sender views, of the store's commit and of a re-index
#   make l2-processor   the L2 chains through the processor and the job, as a relay imports them
#   make relay-image live live-check api-compare   the relay on a devnet, and its comparisons
#   make p2p-compare await-report   a mixed run's reference-client probe and its AwaitBlock report
#   make rollback-replay   the L2 chains rolled back, started again and re-synced
#   make fuzz-server binary-tests devnet-smoke   the server's fuzz target, the binary's tests and
#                       a forging devnet's smoke test
#   make lockstep       a chain forged by Heartwood while the reference applies every block beside it
#   make fuzz-api replay-api   the API's query fuzz target, and its corpus through the oracle
#   make scenario suites   the scenarios through the runner, and the scenario suites on devnets
#   make live-resources api-compare   the forging runs' resource series, and their route answers
#   make l1-long fuzz-long ledger-merge gate-s1   the long runs, the cumulative ledger and the
#                       baseline record; with GATE_RECORDS set, every gate target writes its record
#   make lockstep-batch l2-batch   lockstep chains made in batches with drawn recipes, and judged
#   make fuzz-codec replay-codec   the codec and head fuzz targets, and their corpora through the oracle
#   make ops-doc-check  the operator guide's command blocks, run as written
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
                 logger=V20 pool=V21 state=V08,V09-shuffle,V15,V18 transactions=V17 p2p-wire=V13,V24,V29 \
                 p2p=V25,V26,V30,V31,V32 blockchain=V27,V33 api=V28 forger=V29
# The API's listings (V34) go to the state, the API and the store, whose tests read each its kinds.
VECTOR_CRATES += state=V34 api=V34 database=V34
# The operator commands' files (V35) go to the CLI.
VECTOR_CRATES += cli=V35
# L1: cases per property, the generator's seed, the properties (P1 to P23 or their names, comma
# separated; all when empty); L1_MIN_REACH, how often P13 must reach each reachable result of the
# processor and each unchained status (when empty: 20 from 1,000 cases per property, else none);
# L1_DECISION_REACH, how often P19 must reach each action and refusal of the reference (when
# empty: 20 from 10,000 cases, else none; its rarest classes come about once in fifty cases);
# L1_SERVED_REACH, how often P20 must reach each public route, and a tenth as often the getBlocks
# cut (when empty: 200 from 10,000 cases, else none).
L1_CASES      ?= 256
L1_SEED       ?= 1
L1_PROPERTY   ?=
L1_MIN_REACH  ?=
L1_DECISION_REACH ?=
L1_SERVED_REACH ?=
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

# The gate records. With GATE_RECORDS naming the records' directory (in local.mk, for the gates;
# empty in development, where nothing is written), `$(gate)` before a gate target's command runs it
# through tools/gate-record.sh, which then writes the target's record with `difftest gate record`:
# the target and its variables, the head, the tested paths (GATE_TESTED; the criteria file
# S1_CRITERIA narrows them where it names the command), the oracle image, the machine, the
# command's exit status and its UTC start and end. It exits with the command's status, or 1 when
# the record cannot be written. A long run (l1-long, fuzz-long) also writes its ledger entry into
# LEDGER_DIR. The ledger and the baseline record read them (ledger-merge, gate-s1). A record names the
# oracle image only for a target that reads the oracle or its vectors (GATE_ORACLE_TARGETS; the
# forge replay only with DIRECTION=oracle) with ORACLE_ARGS naming the image, and for a mixed or
# forging run's live check, which reads IMAGE itself. A record of a live run's check (live-check,
# api-compare, live-resources, p2p-compare, await-report, suites) also names the devnet tooling
# each run was made with, from its run directory's tooling.sha256. The default tested paths hold the code, the workspace's
# manifest and lockfile, this file and the check scripts the gate targets run.
GATE_RECORDS  ?=
LEDGER_DIR    ?=
S1_CRITERIA   ?=
GATE_TESTED   ?= packages tools/difftest tools/oracle fuzz Cargo.toml Cargo.lock Makefile \
                 tools/check-layers.sh tools/check-lines.sh tools/test-integration.sh tools/gate-record.sh \
                 tools/fuzz-run.sh
GATE_LEDGER   ?=
GATE_ORACLE_TARGETS := l0 l1 corpus-judge l2-synthetic rollback-replay lockstep replay-api scenario suites \
                       oracle-image vectors-diff vectors-install lockstep-batch l2-batch replay-codec
gate-reads-oracle = $(or $(filter $(1),$(GATE_ORACLE_TARGETS)),$(and $(filter forge-replay,$(1)),$(filter oracle,$(DIRECTION))))
gate-image-of = $(if $(filter live-check,$(1)),$(if $(or $(is-mixed),$(is-forging),$(filter suites,$(RUN))),$(IMAGE)),$(if $(and $(call gate-reads-oracle,$(1)),$(filter --image%,$(ORACLE_ARGS))),$(IMAGE)))
# The run directories whose tooling.sha256 a live run's record names, each run apart (a run
# directory without one is named with none).
gate-tooling-of = $(if $(filter suites,$(1)),$(suite-dir),$(if $(filter live-check api-compare live-resources p2p-compare await-report,$(1)),$(if $(filter suites,$(RUN)),$(FORGING_RESULTS)/suites/$(SEED)$(if $(SUITE),/$(SUITE)-reference $(FORGING_RESULTS)/suites/$(SEED)/$(SUITE)-heartwood),$(if $(and $(RUN),$(results-of-run)),$(run-dir)))))
gate-for = GATE_RECORDS='$(GATE_RECORDS)' GATE_TARGET=$(1) GATE_VARIABLES='$(filter-out GATE_% GATED=% $(GATED) $(addsuffix \,$(GATED)),$(MAKEOVERRIDES))' \
       GATE_IMAGE='$(call gate-image-of,$(2))' GATE_TESTED='$(GATE_TESTED)' GATE_CRITERIA='$(S1_CRITERIA)' \
       GATE_TOOLING='$(call gate-tooling-of,$(2))' \
       GATE_LEDGER="$${GATE_LEDGER:-$(GATE_LEDGER)}" LEDGER_DIR='$(LEDGER_DIR)' GATE_DIFFTEST='$(DIFFTEST)' tools/gate-record.sh
gate = $(call gate-for,'$@',$@)
need-records = $(if $(GATE_RECORDS),,$(error set GATE_RECORDS, the gate records' directory, in local.mk))
# A recorded fuzz run uses libFuzzer's fork mode: there its exit status fails on every crash,
# timeout or out-of-memory stop, and tools/fuzz-run.sh checks its last status line too.
need-fork = @jobs='$(FUZZ_FORK)'; case "$$jobs" in ''|*[!0-9]*) jobs=0 ;; esac; [ "$$jobs" -ge 1 ] || { \
	    echo "$@: FUZZ_FORK=$(FUZZ_FORK) is not a number of jobs from 1 (a recorded run uses libFuzzer's fork mode)" >&2; \
	    exit 2; }
need-ledger = $(if $(LEDGER_DIR),,$(error set LEDGER_DIR, the cumulative ledger's directory, in local.mk))
need-criteria = $(if $(S1_CRITERIA),,$(error set S1_CRITERIA, the S1 gate's criteria file, in local.mk))

.PHONY: help check build clippy fmt fmt-check test release-check oracle-context oracle-image \
        oracle-save vectors vectors-local vectors-diff vectors-install l0 ping test-oracle e2e \
        l1 corpus-import corpus-judge corpus-verify l2 l2-synthetic l2-restart crash-test pool-bench \
        layers lines lines-if-tokei l2-processor relay-binary relay-image live live-check api-compare doc \
        test-integration rollback-replay p2p-compare await-report scenario suites live-resources \
        l1-long fuzz-long ledger-merge gate-s1 gated

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
# The tests that need the reference run against the oracle image when it is built (or against
# DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR when set); without one they are skipped, and the
# run says so (DIFFTEST_TEST_NO_ORACLE), rather than passing unrun.
test-integration: build ## the integration-tagged tests of every crate (the oracle's against IMAGE when it is built)
	$(TOOLCHAIN) && if [ -z "$$DIFFTEST_TEST_IMAGE$$DIFFTEST_TEST_SOLAR_DIR" ]; then \
	    if $(DIFFTEST_DOCKER) image inspect '$(IMAGE)' >/dev/null 2>&1; then \
	        export DIFFTEST_TEST_IMAGE='$(IMAGE)'; \
	        echo "test-integration: the tests that need the reference run against $(IMAGE)"; \
	    else \
	        export DIFFTEST_TEST_NO_ORACLE=1; \
	        echo "test-integration: no oracle image $(IMAGE): the tests that need the reference are skipped"; \
	    fi; \
	fi; \
	tools/test-integration.sh

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

# Each crate gets its classes (VECTOR_CRATES; a crate named in more than one spec gets the classes
# of all of them) and one manifest of them, which must list every class routed there. A run that
# lacks a class VECTOR_CRATES routes, or a file a committed manifest lists, is refused, and the
# committed files are kept: the genesis class (V10) and the genesis-state class (V18) are written
# only with DEVNET_GENERATOR. So is a run with a kind below its minimum record count
# (tools/oracle/vector-minimums.json).
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
	@$(TOOLCHAIN) && node tools/oracle/check-minimums.js $(VECTORS)/container || { \
	    echo "vectors-install: a class of tools/oracle/vector-minimums.json is missing or has a kind" \
	         "below its minimum; nothing was installed" >&2; exit 1; }
	@crates=; \
	for spec in $(VECTOR_CRATES); do \
	    case " $$crates " in *" $${spec%%=*} "*) ;; *) crates="$$crates $${spec%%=*}" ;; esac; \
	done; \
	for crate in $$crates; do \
	    dir=packages/$$crate/tests/vectors; \
	    files=; \
	    for spec in $(VECTOR_CRATES); do \
	        [ "$${spec%%=*}" = "$$crate" ] || continue; \
	        for class in $$(echo $${spec#*=} | tr ',' ' '); do \
	            files="$$files $$(cd $(VECTORS)/container && ls $$class*.jsonl 2>/dev/null)"; \
	        done; \
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
	@for spec in $(VECTOR_CRATES); do \
	    dir=packages/$${spec%%=*}/tests/vectors; \
	    for class in $$(echo $${spec#*=} | tr ',' ' '); do \
	        for f in $$(cd $(VECTORS)/container && ls $$class*.jsonl 2>/dev/null); do \
	            grep -q "  $$f$$" $$dir/MANIFEST.sha256 2>/dev/null || { \
	                echo "vectors-install: $$dir/MANIFEST.sha256 does not list $$f, which" \
	                     "VECTOR_CRATES routes there" >&2; exit 1; }; \
	        done; \
	    done; \
	done

l0: build ## compare VECTORS/container with Heartwood's subjects
	$(gate) $(DIFFTEST) run l0 $(VECTORS)/container $(STRICT)

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

l1: build ## L1 properties against the oracle (ORACLE_ARGS): L1_CASES, L1_SEED, L1_PROPERTY, L1_MIN_REACH, L1_DECISION_REACH, L1_SERVED_REACH
	$(need-image)
	$(TOOLCHAIN) && reach='$(L1_MIN_REACH)'; decisions='$(L1_DECISION_REACH)'; served='$(L1_SERVED_REACH)'; \
	if [ -z "$$reach" ]; then if [ $(L1_CASES) -ge 1000 ]; then reach=20; else reach=0; fi; fi; \
	if [ -z "$$decisions" ]; then if [ $(L1_CASES) -ge 10000 ]; then decisions=20; else decisions=0; fi; fi; \
	if [ -z "$$served" ]; then if [ $(L1_CASES) -ge 10000 ]; then served=200; else served=0; fi; fi; \
	$(gate) $(DIFFTEST) run l1 $(if $(L1_PROPERTY),--property $(L1_PROPERTY)) --min-reach $$reach \
	    --decision-reach $$decisions --served-reach $$served \
	    --cases $(L1_CASES) --seed $(L1_SEED) --regressions target/l1-regressions $(STRICT) $(ORACLE_ARGS)

corpus-import: build ## devnet archives (ARCHIVE='dir ...', in chain order) to the corpus CORPUS/NAME
	$(need-name)
	$(if $(ARCHIVE),,$(error set ARCHIVE, the devnet archives of the chain in order))
	$(DIFFTEST) corpus import $(ARCHIVE) --out $(CORPUS)/$(NAME) $(if $(CORPUS_SEED),--seed '$(CORPUS_SEED)')

corpus-judge: build ## the oracle's judge of CORPUS/NAME: digest lines, anchors, restart lines (DUMP, RESTARTS, TO, PROCESSOR)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) corpus judge $(CORPUS)/$(NAME) $(if $(DUMP),--dump $(DUMP)) \
	    $(if $(RESTARTS),--restarts $(RESTARTS)) $(if $(TO),--to $(TO)) $(if $(PROCESSOR),--processor) \
	    $(ORACLE_ARGS)

corpus-verify: build ## the manifest of CORPUS/NAME
	$(need-name)
	$(DIFFTEST) corpus verify $(CORPUS)/$(NAME)

l2: build ## replay CORPUS/NAME in Heartwood through its store against the judge's lines (TO=H)
	$(need-name)
	$(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) $(if $(TO),--to $(TO)) $(STRICT)

l2-synthetic: build ## L2 of CORPUS/NAME with the synthetic restart, pool and processor cases on top (needs the oracle)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --synthetic $(if $(TO),--to $(TO)) $(STRICT) $(ORACLE_ARGS)
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --processor --synthetic $(if $(TO),--to $(TO)) $(STRICT) \
	    $(ORACLE_ARGS)

l2-restart: build ## L2 of CORPUS/NAME with the restarts of its restart plan, by both of Heartwood's paths (TO=H)
	$(need-name)
	$(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --restart $(if $(TO),--to $(TO)) $(STRICT)

# The rollback replay compares with the oracle's rollback and restart on the same corpus, so it
# needs the oracle (ORACLE_ARGS), as l2-synthetic does.
rollback-replay: build ## CORPUS/NAME rolled back to each target it reaches, started and re-synced, against the oracle (TO=H)
	$(need-name)
	$(need-image)
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --rollback $(if $(TO),--to $(TO)) $(STRICT) $(ORACLE_ARGS)

crash-test: build ## CORPUS/NAME replayed through the store (or a relay, RUNTIME=1) in a child process killed KILLS times
	$(need-name)
	$(gate) $(DIFFTEST) store crash $(CORPUS)/$(NAME) --kills $(KILLS) $(if $(KILL_SEED),--seed $(KILL_SEED)) \
	    $(if $(RUNTIME),--runtime) $(STRICT)

pool-bench: ## the cost of the pool's per-sender views, of the store's commit and of a re-index (release build)
	$(TOOLCHAIN) && cargo test --release --locked -p heartwood-pool --test view_cost -- --ignored --nocapture
	$(TOOLCHAIN) && cargo test --release --locked -p heartwood-blockchain --test commit_cost -- --ignored --nocapture
	$(TOOLCHAIN) && cargo test --release --locked -p heartwood-state --test reindex_cost -- --ignored --nocapture

layers: ## crate edges follow the layers; no async, storage or HTTP crate in the consensus crates
	$(TOOLCHAIN) && tools/check-layers.sh

lines: ## code lines per crate against the review ceilings (tokei, inline tests excluded)
	$(TOOLCHAIN) && tools/check-lines.sh

# make check runs the ceilings only where tokei is installed; `make lines` needs it.
lines-if-tokei:
	@$(TOOLCHAIN) && if command -v tokei >/dev/null 2>&1; then tools/check-lines.sh; \
	else echo "lines: tokei is not installed, so the review ceilings are not checked (make lines)"; fi

# ---- dependencies: advisories, licences, sources and bans (deny.toml) ----
# Not a prerequisite of make check, which works offline: cargo-deny fetches the RustSec advisory
# database. The fuzz targets are a package of their own, checked against the same deny.toml; the
# wrapper rules for crates outside their graph (the API's, the binary's) do not apply there.
.PHONY: deny

deny: build ## advisories, licences, sources and bans of every dependency (cargo-deny; needs the network)
	@$(TOOLCHAIN) && command -v cargo-deny >/dev/null 2>&1 || \
	    { echo "deny: cargo-deny is not installed (cargo install --locked cargo-deny)" >&2; exit 1; }
	$(TOOLCHAIN) && $(gate) cargo deny --locked --config deny.toml check
	$(TOOLCHAIN) && $(gate) cargo deny --locked --config deny.toml --manifest-path $(FUZZ_MANIFEST) check \
	    --allow unused-wrapper

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
# Its vector classes, and the wire's, are installed in each crate's tests/vectors (VECTOR_CRATES),
# which must hold them: a class test fails when its class is not installed there. P2P_VECTORS
# (heartwood-p2p and difftest's network_vectors test) or WIRE_VECTORS (heartwood-p2p-wire) name
# another vector run instead, relative to the workspace root, such as the run of
# `make vectors-local`; a class that run lacks is reported and its test passes.
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
# m2), DEVNET_RECIPE (the runs of a forging devnet: t, a reference devnet, and m and f, devnets of
# this tree's release binary on loopback addresses), MIXED_RECIPE (the mixed runs: w0, reference
# nodes only, the baseline; w and p, reference nodes beside Heartwood relays; s, Heartwood relays
# and forgers with a reference observer), RELAY_IMAGE_RECIPE (the relay's container image from a
# binary), LIVE_RESULTS (the run directories), MIXED_RESULTS (the mixed runs' directories) and
# OBSERVER_RECIPE (the observer run's recipe, below) are set in local.mk. The comparisons of a run
# are the harness's (live check, api compare, p2p compare, live await). CHUNKS names one job cut of
# l2-processor (reference or 1; both when empty); LEFT and RIGHT are two nodes' API URLs for a
# direct api-compare; COMPARE names the run a run is compared with (m for t and t for m; w0 for w, p
# and s). MIXED_SPREAD, set in local.mk, lists other runs of w0's recipe with reference nodes only
# (the recipe's dry runs): the log counts of runs w, p and s are then compared with the most any of
# w0 and those runs gave, as the reference's own counts spread from run to run.
LIVE_RECIPE        ?=
DEVNET_RECIPE      ?=
MIXED_RECIPE       ?=
RELAY_IMAGE_RECIPE ?=
LIVE_RESULTS       ?=
MIXED_RESULTS      ?=
MIXED_SPREAD       ?=
COMPARE            ?=
RUN                ?=
CHUNKS             ?=
LEFT               ?=
RIGHT              ?=
API_COMPARE_OUT    ?= target/api-compare
# The runs of a forging devnet, which DEVNET_RECIPE runs, and the mixed runs, which MIXED_RECIPE
# runs into MIXED_RESULTS.
DEVNET_RUNS        := t m f
MIXED_RUNS         := w0 w p s
is-mixed = $(filter $(MIXED_RUNS),$(RUN))
# The observer run: OBSERVER_RECIPE's run h (a reference devnet with a Heartwood observer relay;
# LIVE_RECIPE unless local.mk names another recipe of the same runs for it) under OBSERVER_ENV, the
# recipe's environment (its network's name and subnet, its host ports, its stop height and its
# RESULTS directory), whose run directory is OBSERVER_RESULTS/h. They are set in local.mk.
OBSERVER_RECIPE    ?= $(LIVE_RECIPE)
OBSERVER_ENV       ?=
OBSERVER_RESULTS   ?=
OBSERVER_RUNS      := observer
is-observer = $(filter $(OBSERVER_RUNS),$(RUN))
need-observer = $(if $(is-observer),$(if $(OBSERVER_ENV),,$(error set OBSERVER_ENV, the observer run's environment (NAME, SUBNET, HOST_PORT_BASE, STOP_HEIGHT, RESULTS), in local.mk)))
# The forging runs, rootless (mixed-short, mixed, mixed-split: reference and Heartwood nodes forging
# side by side; churn: every churn event on Heartwood nodes) or in mode B (core: Heartwood nodes
# of the combined role), each run into a directory of its own, FORGING_RESULTS/RUN/SEED (SEED the
# run's network seed), by MIXED_FORGING_RECIPE, CHURN_RECIPE and CORE_RECIPE (set in local.mk).
FORGING_RUNS       := mixed-short mixed mixed-split churn core
is-forging = $(filter $(FORGING_RUNS),$(RUN))
forging-recipe-name = $(if $(filter churn,$(RUN)),CHURN_RECIPE,$(if $(filter core,$(RUN)),CORE_RECIPE,MIXED_FORGING_RECIPE))
recipe-name-of-run = $(if $(is-forging),$(forging-recipe-name),$(if $(filter $(DEVNET_RUNS),$(RUN)),DEVNET_RECIPE,$(if $(is-mixed),MIXED_RECIPE,$(if $(is-observer),OBSERVER_RECIPE,LIVE_RECIPE))))
recipe-of-run = $($(recipe-name-of-run))
results-name-of-run = $(if $(is-forging),FORGING_RESULTS,$(if $(is-mixed),MIXED_RESULTS,$(if $(is-observer),OBSERVER_RESULTS,LIVE_RESULTS)))
results-of-run = $($(results-name-of-run))
run-dir = $(results-of-run)/$(if $(is-observer),h,$(RUN)$(if $(is-forging),/$(SEED)))
need-live = $(if $(recipe-of-run),,$(error set $(recipe-name-of-run), the devnet tooling's recipe, in local.mk))
need-run = $(if $(RUN),,$(error set RUN, the live run: r, h, m1, m2, t, m, f, w0, w, p, s, observer, mixed-short, mixed, mixed-split, churn or core))
need-seed = $(if $(is-forging),$(if $(SEED),,$(error set SEED, the forging run's network seed and directory under FORGING_RESULTS/$(RUN))))
need-results = $(if $(results-of-run),,$(error set $(results-name-of-run), the live runs' directory, in local.mk))
need-mixed = $(if $(is-mixed),,$(error set RUN to a mixed run: w0, w, p or s))
# The two relays a mixed run's probe compares: the Heartwood relay against a reference relay (node
# 4 against node 1; in run s node 1, a Heartwood relay, against node 4, the reference observer),
# and the two reference relays of run w0 in the same places.
probe-pair = $(if $(filter s,$(RUN)),--left node1 --right node4,--left node4 --right node1)

l2-processor: build ## replay CORPUS/NAME through the processor and the job against the judge's lines (TO, CHUNKS)
	$(need-name)
	$(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --processor $(if $(CHUNKS),--chunks $(CHUNKS)) $(if $(TO),--to $(TO)) \
	    $(STRICT)

relay-binary: ## the relay's release binary with the harness's flags (target/release/heartwood)
	$(TOOLCHAIN) && cargo build --release --locked -p heartwood-core --features difftest

# The image recipe runs without IMAGE in its environment: make passes a variable of its command line
# to its recipes, and IMAGE there names the oracle's image, not the relay's.
relay-image: relay-binary ## the relay's release binary and its devnet image (RELAY_IMAGE_RECIPE)
	$(if $(RELAY_IMAGE_RECIPE),,$(error set RELAY_IMAGE_RECIPE, the devnet tooling's image recipe, in local.mk))
	env -u IMAGE $(RELAY_IMAGE_RECIPE) $(CURDIR)/target/release/heartwood

# A run with a Heartwood relay (every run but r, t and w0) tests this tree's release binary: live
# builds it (and, for a run in Docker, its image) first, and live-check and api-compare build it
# again (nothing to do when the tree is unchanged), so that a run of another build fails their
# check that it is this tree's relay. The mixed runs start at once and share one relay image, so
# their live target builds the binary only: `make relay-image` runs once before them, and
# MIXED_RECIPE checks that the image holds this tree's binary.
relay-of-run = $(if $(filter-out r t w0 suites,$(RUN)),$(1))
live-prerequisite = $(if $(is-observer),build relay-image,$(if $(filter m f $(FORGING_RUNS),$(RUN)),relay-binary,$(if $(is-mixed),$(call relay-of-run,relay-binary),$(call relay-of-run,relay-image))))

# The planned stop height of each forging run and scenario suite: 2 rounds (106), the lifecycle
# suite 3 (159: a temporary resignation's revoke is valid no earlier than 107 blocks after it),
# the faulted mixed runs and the churn run 5 (265, every fault or churn event in one run). A run
# never passes its planned height or round 5, and never starts within 40 minutes of 00:00 UTC,
# when the nodes' logs rotate; the recipes refuse the same.
planned-mixed-short := 106
planned-mixed := 265
planned-mixed-split := 265
planned-churn := 265
planned-core := 106
planned-transactions := 106
planned-lifecycle := 159
planned-network := 106
planned-network-split := 106
STOP_HEIGHT ?=
define live-guard
@planned='$(1)'; stop='$(STOP_HEIGHT)'; seed='$(SEED)'; \
	if [ -z "$$planned" ]; then echo "$@: no planned stop height for $(2)" >&2; exit 2; fi; \
	if [ -n "$$stop" ] && ! { [ "$$stop" -ge 1 ] && [ "$$stop" -le "$$planned" ] && [ "$$stop" -le 265 ]; } 2>/dev/null; then \
	    echo "$@: STOP_HEIGHT=$$stop is not a height from 1 to $(2)'s planned stop height $$planned (round 5 ends at 265)" >&2; exit 2; \
	fi; \
	case "$$seed" in ''|*..*|*[!A-Za-z0-9._-]*) \
	    echo "$@: SEED=$$seed is not a name of letters, digits, '.', '_' and '-' (it names the run's directory)" >&2; exit 2 ;; \
	esac; \
	now=$$(( 10#$$(date -u +%H) * 60 + 10#$$(date -u +%M) )); \
	if [ $$now -ge 1400 ] || [ $$now -lt 40 ]; then \
	    echo "$@: $$(date -u +%H:%M) UTC is within 40 minutes of 00:00 UTC, when the nodes' logs rotate" >&2; exit 2; \
	fi
endef

live: $(live-prerequisite) ## one live run of at most 5 rounds on a devnet (RUN=r|h|m1|m2 with LIVE_RECIPE, t|m|f with DEVNET_RECIPE, w0|w|p|s with MIXED_RECIPE, observer with OBSERVER_RECIPE and OBSERVER_ENV, mixed-short|mixed|mixed-split|churn|core with SEED)
	$(need-run)
	$(need-live)
	$(need-observer)
	$(need-seed)
	$(if $(is-forging),$(call live-guard,$(planned-$(RUN)),run $(RUN)))
	$(if $(is-forging),STOP_HEIGHT=$(or $(STOP_HEIGHT),$(planned-$(RUN))) $(recipe-of-run) $(RUN) '$(SEED)' $(run-dir),$(if $(is-observer),env $(OBSERVER_ENV) $(recipe-of-run) h,$(recipe-of-run) $(RUN))) $(CURDIR)

# A mixed run's check reads the oracle's image (its rollback's prediction), and its baseline run
# from --compare, not from COMPARE in difftest's environment. The baseline of runs w, p and s is
# w0 unless COMPARE names it.
mixed-compare = $(if $(COMPARE),$(COMPARE),$(if $(filter-out w0,$(is-mixed)),w0))
mixed-spread = $(if $(and $(is-mixed),$(mixed-compare)),$(foreach d,$(MIXED_SPREAD),--spread $(d)))
# The suites' check compares each suite's reference and Heartwood runs of a seed
# (FORGING_RESULTS/suites/SEED/SUITE-IMPL), every suite or SUITE alone, and judges each run's chain
# against the reference through the oracle's IMAGE (its corpora under CORPUS).
live-check: build $(call relay-of-run,relay-binary) ## the comparisons of a live run (RUN, LIVE_RESULTS, MIXED_RESULTS, OBSERVER_RESULTS or FORGING_RESULTS with SEED; COMPARE, for w, p and s w0 by default; RUN=suites with SEED and SUITE)
	$(need-run)
	$(if $(filter suites,$(RUN)),,$(need-results))
	$(need-seed)
	$(if $(or $(is-mixed),$(is-forging),$(filter suites,$(RUN))),$(need-image))
	$(TOOLCHAIN) && if [ '$(RUN)' = suites ]; then \
	    [ -n '$(SEED)' ] && [ -n '$(FORGING_RESULTS)' ] || { echo "live-check: set SEED and FORGING_RESULTS for RUN=suites" >&2; exit 2; }; \
	    $(gate) env DIFFTEST_IMAGE='$(IMAGE)' $(DIFFTEST) scenario compare $(FORGING_RESULTS)/suites/$(SEED) \
	        $(if $(SUITE),--suite $(SUITE)) $(STRICT); \
	else \
	    $(gate) $(if $(or $(is-mixed),$(is-forging)),env -u COMPARE DIFFTEST_IMAGE='$(IMAGE)') $(DIFFTEST) live check \
	        $(run-dir) $(if $(and $(is-mixed),$(mixed-compare)),--compare $(results-of-run)/$(mixed-compare)) $(mixed-spread) $(STRICT); \
	fi

# A forging run records the route corpus of each pair it compares (the first in its run
# directory's api/, the others in directories of their own), each named in its run.json by its two
# nodes; PAIR=5,3 judges the pair of node 5 against node 3 alone.
PAIR               ?=
pair-arg = $(if $(PAIR),--pair node$(word 1,$(subst $(comma), ,$(PAIR)))$(comma)node$(word 2,$(subst $(comma), ,$(PAIR))))
comma := ,
api-compare: build $(if $(LEFT),,$(call relay-of-run,relay-binary)) ## judge the API answers a live run recorded (RUN, LIVE_RESULTS, MIXED_RESULTS, OBSERVER_RESULTS, or FORGING_RESULTS with SEED and PAIR=5,3|4,2), or two nodes' (LEFT, RIGHT)
	$(need-seed)
	$(TOOLCHAIN) && if [ -n '$(LEFT)' ] && [ -n '$(RIGHT)' ]; then \
	    $(gate) $(DIFFTEST) api compare --left '$(LEFT)' --right '$(RIGHT)' --out $(API_COMPARE_OUT) $(STRICT); \
	else \
	    [ -n '$(RUN)' ] && [ -n '$(results-of-run)' ] || { echo "api-compare: set RUN and $(results-name-of-run), or LEFT and RIGHT" >&2; exit 2; }; \
	    $(if $(is-forging),[ -n '$(PAIR)' ] || { echo "api-compare: set PAIR for RUN=$(RUN) (such as PAIR=5$(comma)3)" >&2; exit 2; };) \
	    $(gate) $(DIFFTEST) api compare --run $(run-dir) $(pair-arg) $(STRICT); \
	fi

p2p-compare: build $(call relay-of-run,relay-binary) ## the reference-client probe's records of a mixed run's two relays (RUN=w0|w|s, MIXED_RESULTS)
	$(need-run)
	$(need-mixed)
	$(need-results)
	$(gate) $(DIFFTEST) p2p compare $(results-of-run)/$(RUN) $(probe-pair) $(STRICT)

await-report: build ## the AwaitBlock report of a mixed run's Heartwood relays (RUN=w|p|s, MIXED_RESULTS)
	$(need-run)
	$(need-mixed)
	$(need-results)
	$(gate) $(DIFFTEST) live await $(results-of-run)/$(RUN) $(STRICT)

# RESOURCE_BASELINE names the resource figures a person accepted after a missed budget (a file
# outside the tree, with the decision's reference): a figure within 25 % of it passes.
RESOURCE_BASELINE ?=
live-resources: build ## the resource report of a forging run's series (RUN=mixed-short|mixed|mixed-split|churn|core, SEED, FORGING_RESULTS; RESOURCE_BASELINE)
	$(need-run)
	$(need-seed)
	$(if $(is-forging),,$(error set RUN to a forging run: $(FORGING_RUNS)))
	$(need-results)
	$(gate) $(DIFFTEST) live resources $(run-dir) $(if $(RESOURCE_BASELINE),--baseline $(RESOURCE_BASELINE)) $(STRICT)
# ---- end of the relay ----

# ---- heartwood-forger and heartwood-cli: the forger, the commands and the devnet ----
# The forger's vector class (V29) is installed beside the forger and the wire crate
# (VECTOR_CRATES), where the crate tests read it; FORGER_VECTORS and WIRE_VECTORS name another
# vector run for them. The forger's crash test runs the forger-child binary (feature
# difftest: the forger's loop, relay client and signer) against a mock relay on a loopback
# address. DIRECTION=oracle makes forge-replay rebuild a chain Heartwood
# forged with the reference; otherwise Heartwood rebuilds a chain the reference forged.
FORGER_FUZZ   := internal_body status_to_sign
# The vector run whose V29 records seed the corpus of internal_body.
FORGER_SEEDS  ?= $(VECTORS)/local
FORGER_LOGS   ?= target/forger-fuzz
FORGER_CHILD  := target/debug/forger-child
FORGER_AUX    := heartwood-forger
DIRECTION     ?=
RELEASE_PACKAGES += heartwood-forger heartwood-cli
SEAMS         += heartwood-forger:difftest heartwood-cli:test-child

.PHONY: test-forger test-cli forger-features forge-replay forger-crash fuzz-forger

check: test-forger test-cli forger-features

test-forger: ## heartwood-forger alone, and heartwood-p2p without node: clippy (warnings are errors) and cargo test, without and with features
	$(TOOLCHAIN) && cargo clippy -p heartwood-forger --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-forger --all-targets --all-features --locked -- -D warnings
	$(TOOLCHAIN) && cargo test --locked -p heartwood-forger
	$(TOOLCHAIN) && cargo test --locked -p heartwood-forger --all-features
	$(TOOLCHAIN) && cargo clippy -p heartwood-p2p --no-default-features --features difftest --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo test --locked -p heartwood-p2p --no-default-features --features difftest

test-cli: ## heartwood-cli alone: clippy (warnings are errors) and cargo test, with no feature, the default ones and all
	$(TOOLCHAIN) && cargo clippy -p heartwood-cli --all-targets --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-cli --all-targets --no-default-features --locked -- -D warnings
	$(TOOLCHAIN) && cargo clippy -p heartwood-cli --all-targets --all-features --locked -- -D warnings
	$(TOOLCHAIN) && cargo test --locked -p heartwood-cli
	$(TOOLCHAIN) && cargo test --locked -p heartwood-cli --all-features

forger-features: ## only FORGER_AUX enables the forger-aux feature of heartwood-crypto
	@$(TOOLCHAIN) && set -o pipefail && enablers=$$(cargo tree --locked --workspace -e features,normal,build -i heartwood-crypto \
	        --prefix depth | awk '/^1/ { on = /feature "forger-aux"/ } on && /^2/ { sub(/^2/, ""); print $$1 }' \
	        | sort -u | tr '\n' ' ') || exit 1; \
	if [ "$$enablers" != "$(FORGER_AUX) " ]; then \
	    echo "forger-features: forger-aux is enabled by $$enablers, not by $(FORGER_AUX) only" >&2; exit 1; \
	fi; \
	echo "forger-features: forger-aux is enabled by $(FORGER_AUX) only"

forge-replay: build ## the forge replay of CORPUS/NAME (TO=H); DIRECTION=oracle has the reference rebuild a chain Heartwood forged
	$(need-name)
	$(if $(filter oracle,$(DIRECTION)),$(need-image))
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) run l2 $(CORPUS)/$(NAME) --forge $(if $(TO),--to $(TO)) \
	    $(if $(filter oracle,$(DIRECTION)),--oracle $(ORACLE_ARGS)) $(STRICT)

forger-crash: build ## the forger's crash test: forger-child against a mock relay, killed KILLS times (KILL_SEED)
	$(TOOLCHAIN) && cargo build --locked -p heartwood-forger --features difftest --bin forger-child
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) forger crash --kills $(KILLS) --child $(FORGER_CHILD) $(if $(KILL_SEED),--seed $(KILL_SEED)) \
	    $(STRICT)

# The two targets run at the same time, FUZZ_FORK jobs each, with the wire targets' libFuzzer
# options, on corpora of their own (git-ignored); internal_body is seeded from the V29 records of
# FORGER_SEEDS when it exists.
fuzz-forger: ## the forger's fuzz targets at once, FUZZ_SECONDS each (nightly and cargo-fuzz); logs in FORGER_LOGS
	@$(TOOLCHAIN) && if [ -d '$(FORGER_SEEDS)' ]; then \
	    node tools/forger-fuzz-seeds.js '$(FORGER_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-forger: no seeds: $(FORGER_SEEDS) does not exist"; \
	fi
	$(TOOLCHAIN) && for target in $(FORGER_FUZZ); do cargo +nightly fuzz build $$target || exit 1; done
	@$(TOOLCHAIN) && mkdir -p $(FORGER_LOGS) && pids=; \
	for target in $(FORGER_FUZZ); do \
	    mkdir -p $(FUZZ_CORPUS)/$$target; \
	    cargo +nightly fuzz run $$target -- $(WIRE_LIBFUZZER) > $(FORGER_LOGS)/$$target.log 2>&1 & pids="$$pids $$!"; \
	done; \
	echo "fuzz-forger: $(FORGER_FUZZ) running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs each; logs in $(FORGER_LOGS)"; \
	fail=0; set -- $(FORGER_FUZZ); \
	for pid in $$pids; do \
	    target=$$1; shift; \
	    wait $$pid || { echo "fuzz-forger: $$target stopped with a failure, see $(FORGER_LOGS)/$$target.log" >&2; fail=1; }; \
	    last=$$(grep -E '^#[0-9]+: cov: ' $(FORGER_LOGS)/$$target.log | tail -n 1); \
	    files=$$(find $(FUZZ_CORPUS)/$$target -type f | wc -l); \
	    size=$$(du -sk $(FUZZ_CORPUS)/$$target | cut -f1); \
	    echo "fuzz-forger: $$target: $${last:-no status line}; corpus $$files files, $$size KiB"; \
	    case "$$last" in *'oom/timeout/crash: 0/0/0 '*) ;; *) fail=1 ;; esac; \
	done; \
	exit $$fail
# ---- end of heartwood-forger and heartwood-cli ----

# ---- the peer-to-peer server, the node's wiring and the forging devnet ----
# The server's session target runs on a corpus of its own (git-ignored), with the wire targets'
# libFuzzer options, seeded from the V31 requests of SERVER_SEEDS (the installed class by default)
# when it exists. The binary's tests start relays, forgers and supervisors as processes; the
# devnet smoke test (marked `devnet`) runs a 1-node devnet for 20 blocks, then a 5-node one for a
# round, on the loopback addresses of DEVNET_PREFIX.
SERVER_FUZZ   := nes_server_session
SERVER_SEEDS  ?= packages/p2p/tests/vectors
SERVER_LOGS   ?= target/server-fuzz
DEVNET_PREFIX ?= 127.10.0

.PHONY: fuzz-server binary-tests devnet-smoke

fuzz-server: ## the server's fuzz target, FUZZ_SECONDS (nightly and cargo-fuzz); logs in SERVER_LOGS
	@$(TOOLCHAIN) && if [ -d '$(SERVER_SEEDS)' ]; then \
	    node tools/server-fuzz-seeds.js '$(SERVER_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-server: no seeds: $(SERVER_SEEDS) does not exist"; \
	fi
	$(TOOLCHAIN) && cargo +nightly fuzz build $(SERVER_FUZZ)
	@$(TOOLCHAIN) && mkdir -p $(SERVER_LOGS) $(FUZZ_CORPUS)/$(SERVER_FUZZ) && \
	echo "fuzz-server: $(SERVER_FUZZ) running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs; logs in $(SERVER_LOGS)" && \
	fail=0; cargo +nightly fuzz run $(SERVER_FUZZ) -- $(WIRE_LIBFUZZER) > $(SERVER_LOGS)/$(SERVER_FUZZ).log 2>&1 || fail=1; \
	last=$$(grep -E '^#[0-9]+: cov: ' $(SERVER_LOGS)/$(SERVER_FUZZ).log | tail -n 1); \
	echo "fuzz-server: $(SERVER_FUZZ): $${last:-no status line}"; \
	case "$$last" in *'oom/timeout/crash: 0/0/0 '*) ;; *) fail=1 ;; esac; \
	exit $$fail

binary-tests: build ## the heartwood binary's tests, the slow ones too
	$(TOOLCHAIN) && $(gate) cargo test --locked -p heartwood-core --all-features --tests -- --include-ignored

devnet-smoke: build ## a forging devnet of this tree's binary: 1 node for 20 blocks, then 5 nodes for a round
	$(TOOLCHAIN) && HEARTWOOD_DEVNET_PREFIX=$(DEVNET_PREFIX) cargo test --locked -p heartwood-core --all-features \
	    --test devnet_smoke -- --ignored
# ---- end of the peer-to-peer server, the node's wiring and the forging devnet ----

# ---- the API's listings: the lockstep producer and the query fuzz target ----
# lockstep makes a chain forged by Heartwood's forger from SEED and the genesis profile PROFILE as
# the recipe RECIPE says, with the reference (ORACLE_ARGS) building every transaction and applying
# every block beside it, into the corpus CORPUS/NAME. A chain has ROUNDS rounds: 5 at most, more
# only with LONG=1 (and 50 at most).
SEED          ?=
PROFILE       ?=
RECIPE        ?=
ROUNDS        ?= 5
LONG          ?=
# The API's query fuzz target runs on a corpus of its own (git-ignored), with the wire targets'
# libFuzzer options, seeded from the queries of the vector classes in API_SEEDS (the API's installed
# classes by default) when it exists; its replay sends each input through the oracle's grammar,
# the route's validation and the pagination (api.fuzzQuery) and through Heartwood's.
API_FUZZ      := api_query
API_REPLAY    := api_query=api.fuzzQuery
API_SEEDS     ?= packages/api/tests/vectors
API_LOGS      ?= target/api-fuzz

.PHONY: lockstep fuzz-api replay-api

lockstep: build ## a lockstep chain CORPUS/NAME from SEED, PROFILE and RECIPE, ROUNDS rounds (5; more only with LONG=1)
	$(need-name)
	$(need-image)
	$(if $(SEED),,$(error set SEED, the chain's seed))
	$(if $(PROFILE),,$(error set PROFILE, the genesis profile))
	$(if $(RECIPE),,$(error set RECIPE, the recipe file))
	@if [ '$(ROUNDS)' -gt 5 ] 2>/dev/null && [ -z '$(LONG)' ]; then \
	    echo "lockstep: ROUNDS=$(ROUNDS) is a long chain; set LONG=1 for more than 5 rounds" >&2; exit 2; \
	fi
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) lockstep make --seed '$(SEED)' --profile '$(PROFILE)' --rounds $(ROUNDS) \
	    --recipe '$(RECIPE)' --out $(CORPUS)/$(NAME) $(if $(LONG),--long) $(ORACLE_ARGS)

fuzz-api: build ## the API's query fuzz target, FUZZ_SECONDS (nightly and cargo-fuzz); logs in API_LOGS
	@$(TOOLCHAIN) && if [ -d '$(API_SEEDS)' ]; then \
	    node tools/api-fuzz-seeds.js '$(API_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-api: no seeds: $(API_SEEDS) does not exist"; \
	fi
	$(need-fork)
	$(TOOLCHAIN) && cargo +nightly fuzz build $(API_FUZZ)
	@$(TOOLCHAIN) && mkdir -p $(API_LOGS) $(FUZZ_CORPUS)/$(API_FUZZ) && \
	echo "fuzz-api: $(API_FUZZ) running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs; logs in $(API_LOGS)" && \
	fail=0; $(gate) tools/fuzz-run.sh $(API_LOGS)/$(API_FUZZ).log \
	    cargo +nightly fuzz run $(API_FUZZ) -- $(WIRE_LIBFUZZER) || fail=1; \
	files=$$(find $(FUZZ_CORPUS)/$(API_FUZZ) -type f | wc -l); \
	echo "fuzz-api: $(API_FUZZ): corpus $$files files"; \
	exit $$fail

# Every input of the corpus must be compared, as in replay-wire.
replay-api: build ## the api_query corpus through the oracle (ORACLE_ARGS) and Heartwood; logs in API_LOGS
	$(need-image)
	@$(TOOLCHAIN) && mkdir -p $(API_LOGS) && fail=0; \
	for pair in $(API_REPLAY); do \
	    target=$${pair%%=*}; op=$${pair#*=}; log=$(API_LOGS)/replay-$$target.log; \
	    if [ -z "$$(ls -A $(FUZZ_CORPUS)/$$target 2>/dev/null)" ]; then \
	        echo "replay-api: $(FUZZ_CORPUS)/$$target is missing or empty (make fuzz-api)" >&2; fail=1; continue; \
	    fi; \
	    $(gate) $(DIFFTEST) oracle replay $$op $(FUZZ_CORPUS)/$$target --vectors packages/crypto/tests/vectors $(ORACLE_ARGS) \
	        > $$log 2>&1; status=$$?; cat $$log; \
	    if [ $$status != 0 ]; then \
	        echo "replay-api: $$target: the replay failed (exit $$status)" >&2; fail=1; \
	    elif grep -qE '^  [0-9]+ (skipped|records have no Rust implementation)' $$log; then \
	        echo "replay-api: $$target: some inputs were not compared" >&2; fail=1; \
	    fi; \
	done; \
	exit $$fail
# ---- end of the API's listings ----

# ---- the scenarios, the suites and the baseline record ----
# scenario runs the scenario files (SCENARIO, a file or a directory; every scenario of
# SCENARIOS by default) at LEVEL (l2, l3 or l4) through the runner's BACKEND (lockstep for l2,
# heartwood for a devnet of this tree, external for the tooling's backend, DIFFTEST_SCENARIO_BACKEND
# in local.mk), with the oracle of ORACLE_ARGS compiling their transactions; a level the backend
# cannot run is named in its record as left to a live run. At l3 and l4 the runner drives the
# devnet in DEVNET_ROOT, which must already run (the suites recipe starts its own): the target
# refuses those levels without it, and within 40 minutes of 00:00 UTC.
# suites runs one scenario suite (SUITE: transactions, lifecycle, network or network-split) on one
# implementation (IMPL: reference, on the tooling's Docker devnet; heartwood, on a devnet of this
# tree in mode B) with the network seed SEED, by SUITES_RECIPE, into FORGING_RESULTS/suites/SEED/
# SUITE-IMPL; `live-check RUN=suites SEED=...` compares the two implementations' runs.
# l1-long runs L1_PROPERTY (one property) L1_CASES cases for each seed of L1_SEEDS (such as 1-7, or
# a list) and fuzz-long runs every target of FUZZ_TARGETS for FUZZ_SECONDS with FUZZ_FORK jobs, each
# run a gate record and a ledger entry. ledger-merge adds another machine's records (FROM, a
# directory, and MACHINE, its name) to the ledger; gate-s1 prints the baseline record's table from the
# records, the ledger and the criteria file (PART, one part of it, or all).
SCENARIOS          := tools/difftest/scenarios
SCENARIO           ?=
DEVNET_ROOT        ?=
LEVEL              ?= l2
BACKEND            ?= $(if $(filter l2,$(LEVEL)),lockstep,heartwood)
DIFFTEST_SCENARIO_BACKEND ?=
SUITES_RECIPE      ?=
MIXED_FORGING_RECIPE ?=
CHURN_RECIPE       ?=
CORE_RECIPE        ?=
FORGING_RESULTS    ?=
SUITE              ?=
IMPL               ?=
SUITES             := transactions lifecycle network network-split
IMPLS              := reference heartwood
L1_SEEDS           ?=
FUZZ_TARGETS       ?=
FROM               ?=
MACHINE            ?=
PART               ?=
suite-dir = $(FORGING_RESULTS)/suites/$(SEED)/$(SUITE)-$(IMPL)
# The seeds of L1_SEEDS: a range A-B (A no greater than B) or a list of numbers.
l1-seeds = $(if $(findstring -,$(L1_SEEDS)),$$(seq $(word 1,$(subst -, ,$(L1_SEEDS))) $(word 2,$(subst -, ,$(L1_SEEDS)))),$(subst $(comma), ,$(L1_SEEDS)))

scenario: build ## the scenarios at LEVEL through the runner's BACKEND (SCENARIO, LEVEL, BACKEND, DEVNET_ROOT at l3 and l4; ORACLE_ARGS compiles)
	$(need-image)
	$(if $(filter external,$(BACKEND)),$(if $(DIFFTEST_SCENARIO_BACKEND),,$(error set DIFFTEST_SCENARIO_BACKEND, the tooling's scenario backend, in local.mk)))
	$(if $(filter-out lockstep,$(BACKEND)),$(if $(DEVNET_ROOT),,$(error set DEVNET_ROOT, the running devnet the $(BACKEND) backend drives at LEVEL=$(LEVEL))))
	@if [ '$(BACKEND)' != lockstep ]; then \
	    now=$$(( 10#$$(date -u +%H) * 60 + 10#$$(date -u +%M) )); \
	    if [ $$now -ge 1400 ] || [ $$now -lt 40 ]; then \
	        echo "scenario: $$(date -u +%H:%M) UTC is within 40 minutes of 00:00 UTC, when the nodes' logs rotate" >&2; exit 2; \
	    fi; \
	fi
	$(TOOLCHAIN) && DIFFTEST_SCENARIO_BACKEND='$(DIFFTEST_SCENARIO_BACKEND)' $(gate) $(DIFFTEST) scenario run \
	    --backend $(BACKEND) --level $(LEVEL) $(if $(DEVNET_ROOT),--devnet-root $(DEVNET_ROOT)) \
	    $(or $(SCENARIO),$(SCENARIOS)) $(STRICT) $(ORACLE_ARGS)

suites: build relay-binary ## one scenario suite on one implementation (SUITE, IMPL=reference|heartwood, SEED; SUITES_RECIPE)
	$(if $(filter $(SUITES),$(SUITE)),,$(error set SUITE to one of $(SUITES)))
	$(if $(filter $(IMPLS),$(IMPL)),,$(error set IMPL to one of $(IMPLS)))
	$(if $(SEED),,$(error set SEED, the suite run's network seed))
	$(if $(SUITES_RECIPE),,$(error set SUITES_RECIPE, the devnet tooling's suites recipe, in local.mk))
	$(if $(FORGING_RESULTS),,$(error set FORGING_RESULTS, the forging runs' directory, in local.mk))
	$(call live-guard,$(planned-$(SUITE)),suite $(SUITE))
	STOP_HEIGHT=$(or $(STOP_HEIGHT),$(planned-$(SUITE))) DIFFTEST_SCENARIO_BACKEND='$(DIFFTEST_SCENARIO_BACKEND)' \
	    $(gate) $(SUITES_RECIPE) $(SUITE) $(IMPL) '$(SEED)' $(suite-dir) $(CURDIR)

l1-long: build ## one L1 property over many seeds, each a gate record and a ledger entry (L1_PROPERTY, L1_CASES, L1_SEEDS)
	$(need-image)
	$(if $(filter-out ,$(word 2,$(subst $(comma), ,$(L1_PROPERTY)))),$(error set L1_PROPERTY to one property))
	$(if $(L1_PROPERTY),,$(error set L1_PROPERTY, the property))
	$(if $(L1_SEEDS),,$(error set L1_SEEDS, the seeds: A-B or a list))
	$(need-records)
	$(need-ledger)
	@case '$(L1_SEEDS)' in \
	    *-*-*|-*|*-) echo "l1-long: L1_SEEDS=$(L1_SEEDS) is not a range A-B or a list" >&2; exit 2 ;; \
	    *-*) a='$(word 1,$(subst -, ,$(L1_SEEDS)))'; b='$(word 2,$(subst -, ,$(L1_SEEDS)))'; \
	         if ! [ "$$a" -le "$$b" ] 2>/dev/null; then echo "l1-long: L1_SEEDS=$(L1_SEEDS): $$a is not a number at most $$b" >&2; exit 2; fi ;; \
	    *) for seed in $(subst $(comma), ,$(L1_SEEDS)); do [ "$$seed" -ge 0 ] 2>/dev/null || \
	         { echo "l1-long: L1_SEEDS=$(L1_SEEDS): $$seed is not a number" >&2; exit 2; }; done ;; \
	esac
	@$(TOOLCHAIN) && fail=0; seeds=0; for seed in $(l1-seeds); do seeds=$$((seeds + 1)); \
	    GATE_LEDGER="kind=property id=$(L1_PROPERTY) cases=$(L1_CASES) seed=$$seed" \
	    $(MAKE) --no-print-directory l1 L1_PROPERTY=$(L1_PROPERTY) L1_CASES=$(L1_CASES) L1_SEED=$$seed || fail=1; \
	done; [ "$$seeds" -gt 0 ] || { echo "l1-long: L1_SEEDS=$(L1_SEEDS) names no seed" >&2; exit 2; }; exit $$fail

fuzz-long: build ## every target of FUZZ_TARGETS for FUZZ_SECONDS with FUZZ_FORK jobs, each a gate record and a ledger entry
	$(if $(FUZZ_TARGETS),,$(error set FUZZ_TARGETS, targets of fuzz/fuzz_targets))
	$(need-records)
	$(need-ledger)
	$(need-fork)
	$(TOOLCHAIN) && for target in $(FUZZ_TARGETS); do cargo +nightly fuzz build $$target || exit 1; done
	@$(TOOLCHAIN) && mkdir -p target/fuzz-long && fail=0; for target in $(FUZZ_TARGETS); do \
	    mkdir -p $(FUZZ_CORPUS)/$$target; log=target/fuzz-long/$$target.log; \
	    export GATE_LEDGER="kind=target id=$$target seconds=$(FUZZ_SECONDS) forks=$(FUZZ_FORK)"; \
	    echo "fuzz-long: $$target for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs; log in $$log"; \
	    $(gate) tools/fuzz-run.sh $$log cargo +nightly fuzz run $$target -- $(WIRE_LIBFUZZER) || fail=1; \
	done; exit $$fail

ledger-merge: build ## another machine's gate records (FROM) into the ledger, as MACHINE's
	$(need-ledger)
	$(if $(FROM),,$(error set FROM, the directory of the other machine's records))
	$(if $(MACHINE),,$(error set MACHINE, the other machine's name))
	$(DIFFTEST) gate ledger merge --from '$(FROM)' --machine '$(MACHINE)' --ledger '$(LEDGER_DIR)'

# The targets whose recipe is not one command (a composite target such as check, or several
# commands) run whole through the recorder, one gate record per target named by it, GATED left out
# of its variables (make keeps a value with spaces as escaped words). The inner make is named
# through GATED_MAKE, so that `make -n gated` prints the commands and records nothing.
GATED ?=
GATED_MAKE = $(MAKE)
# The targets whose recipe writes its own record: run through gated, one run would be two records.
GATE_SELF_RECORDING := l0 l1 corpus-judge l2 l2-synthetic l2-restart rollback-replay crash-test \
                       l2-processor live-check api-compare p2p-compare await-report live-resources \
                       forge-replay forger-crash binary-tests lockstep fuzz-api replay-api scenario \
                       suites deny l1-long fuzz-long lockstep-batch l2-batch fuzz-codec replay-codec \
                       ops-doc-check
gated: build ## each target of GATED run through the recorder, a gate record named by it per target (GATED)
	$(if $(GATED),,$(error set GATED, the targets to run through the recorder))
	$(if $(filter $(GATE_SELF_RECORDING),$(GATED)),$(error $(filter $(GATE_SELF_RECORDING),$(GATED)) write their own records; run them without gated))
	$(need-records)
	@fail=0; $(foreach t,$(GATED),$(call gate-for,'$(t)',$(t)) $(GATED_MAKE) --no-print-directory $(t) || fail=1;) exit $$fail

gate-s1: build ## the S1 record's table from the gate records, the ledger and the criteria file (PART; FINAL=1 for the final reading, where missing evidence fails its row)
	$(need-records)
	$(need-ledger)
	$(need-criteria)
	$(DIFFTEST) gate s1 --criteria '$(S1_CRITERIA)' --records '$(GATE_RECORDS)' --ledger '$(LEDGER_DIR)' \
	    $(if $(PART),--part $(PART)) $(if $(FINAL),--final) $(STRICT)
# ---- end of the scenarios, the suites and the baseline record ----

# ---- the lockstep batch, the codec and head fuzz targets and the operator guide ----
# lockstep-batch makes COUNT lockstep chains of ROUNDS rounds (5 at most; more only with LONG=1,
# the long chains, and 50 at most), BATCH_JOBS at once (the machine's parallelism when empty), each
# with a recipe drawn from SEED and the genesis profile PROFILE (devnet when empty), into CORPUS as
# batch-SEED-1 to batch-SEED-COUNT, listed in CORPUS/batch-SEED.json: one gate record per batch.
# l2-batch judges and replays every chain of the batches in CORPUS (BATCH, seeds separated by
# commas; every batch file when empty), with the reference of ORACLE_ARGS: one gate record.
# fuzz-codec runs each codec and head fuzz target (CODEC_FUZZ) for FUZZ_SECONDS with FUZZ_FORK
# jobs, one after the other, each a gate record, on corpora seeded by tools/codec-fuzz-seeds.js
# from the vector run CODEC_SEEDS when both exist; replay-codec sends the corpus of each target
# that has a replay (CODEC_REPLAY) through the oracle and Heartwood, each a gate record.
# ops-doc-check runs the operator guide's Linux command blocks (OPS_GUIDE) as written, with this
# tree's release binary built without the harness's feature into a target directory of its own
# (OPS_DOC_TARGET, so that it never replaces target/release/heartwood, the relay the live runs
# execute), in a scratch data root under OPS_DOC_ROOT, its devnets on the loopback block
# OPS_DOC_PREFIX (the first three octets); it refuses to start within 40 minutes of 00:00 UTC,
# when the nodes' logs rotate. The check prints the binary's SHA-256 and features.
COUNT         ?=
BATCH         ?=
BATCH_JOBS    ?=
CODEC_FUZZ    := address_decode transaction_json api_head
CODEC_REPLAY  ?= address_decode transaction_json
CODEC_SEEDS   ?= $(VECTORS)/local
CODEC_LOGS    ?= target/codec-fuzz
OPS_GUIDE     ?= docs/operating.md
OPS_DOC_PREFIX ?= 127.49.0
OPS_DOC_ROOT  ?= target/ops-doc-check
OPS_DOC_TARGET ?= target/plain

.PHONY: lockstep-batch l2-batch fuzz-codec replay-codec ops-doc-check

lockstep-batch: build ## COUNT lockstep chains of ROUNDS rounds from SEED, BATCH_JOBS at once (5 rounds; more only with LONG=1, 50 at most)
	$(need-image)
	$(if $(COUNT),,$(error set COUNT, the number of chains))
	$(if $(SEED),,$(error set SEED, the batch's seed))
	@if ! [ '$(ROUNDS)' -ge 1 ] 2>/dev/null || [ '$(ROUNDS)' -gt 50 ]; then \
	    echo "lockstep-batch: ROUNDS=$(ROUNDS) is not a number of rounds from 1 to 50" >&2; exit 2; \
	fi; \
	if [ '$(ROUNDS)' -gt 5 ] && [ -z '$(LONG)' ]; then \
	    echo "lockstep-batch: ROUNDS=$(ROUNDS) makes long chains; set LONG=1 for more than 5 rounds" >&2; exit 2; \
	fi
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) lockstep batch --count '$(COUNT)' --rounds $(ROUNDS) --seed '$(SEED)' \
	    $(if $(LONG),--long) $(if $(PROFILE),--profile '$(PROFILE)') --out $(CORPUS) \
	    $(if $(BATCH_JOBS),--jobs '$(BATCH_JOBS)') $(ORACLE_ARGS)

l2-batch: build ## judge and replay every chain of the lockstep batches in CORPUS (BATCH: seeds, every batch when empty)
	$(need-image)
	$(TOOLCHAIN) && $(gate) $(DIFFTEST) run l2-batch $(CORPUS) \
	    $(foreach batch,$(subst $(comma), ,$(BATCH)),--batch '$(batch)') $(STRICT) $(ORACLE_ARGS)

fuzz-codec: build ## the codec and head fuzz targets, FUZZ_SECONDS each (nightly and cargo-fuzz); logs in CODEC_LOGS
	@$(TOOLCHAIN) && if [ -f tools/codec-fuzz-seeds.js ] && [ -d '$(CODEC_SEEDS)' ]; then \
	    node tools/codec-fuzz-seeds.js '$(CODEC_SEEDS)' $(FUZZ_CORPUS) $(WIRE_SEED_MAX); \
	else \
	    echo "fuzz-codec: no seeds: tools/codec-fuzz-seeds.js or $(CODEC_SEEDS) does not exist"; \
	fi
	$(need-fork)
	$(TOOLCHAIN) && for target in $(CODEC_FUZZ); do cargo +nightly fuzz build $$target || exit 1; done
	@$(TOOLCHAIN) && mkdir -p $(CODEC_LOGS) && fail=0; for target in $(CODEC_FUZZ); do \
	    mkdir -p $(FUZZ_CORPUS)/$$target; log=$(CODEC_LOGS)/$$target.log; \
	    echo "fuzz-codec: $$target running for $(FUZZ_SECONDS) s, $(FUZZ_FORK) jobs; log in $$log"; \
	    $(gate) tools/fuzz-run.sh $$log cargo +nightly fuzz run $$target -- $(WIRE_LIBFUZZER) || fail=1; \
	    files=$$(find $(FUZZ_CORPUS)/$$target -type f | wc -l); \
	    echo "fuzz-codec: $$target: corpus $$files files"; \
	done; exit $$fail

# Every input of a corpus must be compared, as in replay-api.
replay-codec: build ## each codec target's corpus (CODEC_REPLAY) through the oracle (ORACLE_ARGS) and Heartwood; logs in CODEC_LOGS
	$(need-image)
	@$(TOOLCHAIN) && mkdir -p $(CODEC_LOGS) && fail=0; \
	for target in $(CODEC_REPLAY); do \
	    log=$(CODEC_LOGS)/replay-$$target.log; \
	    if [ -z "$$(ls -A $(FUZZ_CORPUS)/$$target 2>/dev/null)" ]; then \
	        echo "replay-codec: $(FUZZ_CORPUS)/$$target is missing or empty (make fuzz-codec)" >&2; fail=1; continue; \
	    fi; \
	    $(gate) $(DIFFTEST) oracle replay-codec $$target $(FUZZ_CORPUS)/$$target --vectors packages/crypto/tests/vectors \
	        $(STRICT) $(ORACLE_ARGS) > $$log 2>&1; status=$$?; cat $$log; \
	    if [ $$status != 0 ]; then \
	        echo "replay-codec: $$target: the replay failed (exit $$status)" >&2; fail=1; \
	    elif grep -qE '^  [0-9]+ (skipped|records have no Rust implementation)' $$log; then \
	        echo "replay-codec: $$target: some inputs were not compared" >&2; fail=1; \
	    fi; \
	done; \
	exit $$fail

ops-doc-check: ## the operator guide's Linux command blocks run as written (OPS_GUIDE, OPS_DOC_PREFIX, OPS_DOC_ROOT)
	@now=$$(( 10#$$(date -u +%H) * 60 + 10#$$(date -u +%M) )); \
	if [ $$now -ge 1400 ] || [ $$now -lt 40 ]; then \
	    echo "ops-doc-check: $$(date -u +%H:%M) UTC is within 40 minutes of 00:00 UTC, when the nodes' logs rotate" >&2; exit 2; \
	fi
	$(TOOLCHAIN) && CARGO_TARGET_DIR='$(abspath $(OPS_DOC_TARGET))' cargo build --release --locked -p heartwood-core
	$(TOOLCHAIN) && HEARTWOOD_DEVNET_PREFIX='$(OPS_DOC_PREFIX)' OPS_DOC_BINARY='$(abspath $(OPS_DOC_TARGET))/release/heartwood' \
	    OPS_DOC_ROOT='$(abspath $(OPS_DOC_ROOT))' $(gate) tools/ops-doc-check.sh '$(OPS_GUIDE)'
# ---- end of the lockstep batch, the codec and head fuzz targets and the operator guide ----
