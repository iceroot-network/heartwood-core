#!/usr/bin/env bash
# check-lines.sh: the code lines of each crate against its review ceiling.
#
#   tools/check-lines.sh
#
# Code lines as tokei counts them (no blank or comment lines) in each crate's src/, without the
# inline test modules (`#[cfg(test)] mod ... { ... }`) and without tests/, benches and examples.
# The ceilings are the review budgets of the crates; a crate over its ceiling fails the check.
# Some parts are also listed on their own (informational, or with their own ceiling). A path
# written `!path` is left out of its line.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v tokei >/dev/null || { echo "check-lines: tokei is not installed (cargo install tokei)" >&2; exit 2; }

# name: path (a crate's src/ or a part of it), ceiling (empty: informational)
# heartwood-crypto was re-baselined from 5,500 to 5,750 with the state machine crates, which added
# the milestones' fee table and revoke delay, the domain-tag registry and the transaction type key;
# about 680 of its lines are the devnet chain-definition builder behind the `genesis` feature. It
# was re-baselined again to 6,000 with the store and the pool: the dynamic fee floor moved in from
# heartwood-transactions (a move, not new code), and the JavaScript number formatting of the
# reference's messages (grouped integers and fractions, amounts with the network's symbol, a
# transaction's short text) joined the one module that holds JavaScript number semantics.
# heartwood-p2p's ceiling covers the client and the server side of the peer protocol; the client
# side is listed on its own. Its `testing` module (the mock peer, behind the `difftest` feature,
# which no release build enables) is test support: it is left out, as tests/ is, and listed apart.
# heartwood-kernel was re-baselined from 1,500 to 1,900 with the relay's process: the node's
# configuration (the environment, app.json and the components' options, peers.json, the version
# floor) and the event bus live in the kernel, which loads configuration in the reference too, and
# the kernel was at its ceiling before them. heartwood-blockchain's processor has a ceiling of its
# own; its runtime (the node thread, the job, the queue, the pool on the node thread, the service,
# the state machine and the fork recovery) is listed on its own. heartwood-blockchain was
# re-baselined from 3,500 to 3,600 with the peer faults the job and the processor name (a chained
# block refused by a rule of the shared state, and a block discarded for its future slot, judged
# without the clock): the crate was at its ceiling before them. The parts listed without a ceiling
# (the blockchain's runtime and storage, heartwood-p2p's client side, ...) are informational: the
# crate's ceiling bounds them. CI installs tokei, so make check holds every ceiling there too.
# heartwood-p2p was re-baselined from 6,000 to 8,000 and heartwood-p2p-wire from 2,500 to 3,200
# before the server side: the integrated client measures 4,815 lines (about 1,100 above its
# estimate), the server side is estimated at 2,500, and the wire crate, at 2,437 with the client's
# requests and reply checks, gains the internal routes' bodies, the status reader and the server's
# request checks (about 550). heartwood-forger and heartwood-cli start at their planned ceilings;
# the parts of each are listed on their own. heartwood-forger was re-baselined from 1,500 to 2,100
# once its parts were built: it measured 2,028 lines then, and 2,098 after fixes to those parts. The
# slot loop and the relay client take 1,013 (the reference's forger service and client are 600
# TypeScript lines; the Rust reads every relay answer as typed, untrusted input and gives every
# call a deadline), block assembly, the key file, the signing record and the attester 565 (the
# record, the key file's permission check and the attester have no counterpart in the reference's
# forger), and the options, seams, errors and crate root 520. The binaries under src/bin of
# heartwood-forger and heartwood-cli are test seams (the crash test's child behind `difftest`, the
# supervisor tests' child behind `test-child`), which no release build has: they are left out, as
# tests/ is, and listed apart.
# With the server side and the node's wiring:
# - heartwood-forger goes from 2,100 to 2,200: its 2 lines of room cannot hold the signing record's
#   write off the runtime's thread and the first round's height for the version floor check.
# - heartwood-blockchain goes from 3,500 to 3,700, which also holds the peer faults above (their
#   3,600 on their own): the node's side of its own forger (the lateness checks of a forger's
#   block, the wake-up on request, the block ping, the queue's running state, the pool's collation
#   and third processor, the node's own delegates) belongs to the service, as the reference keeps
#   it in its blockchain package; the processor's ceiling is unchanged, and the forger's blocks are
#   listed apart.
# - heartwood-core goes from 2,000 to 3,000: the command tree of both roles, one file per command
#   family, the forger's composition root and the server's wiring join the relay's configuration
#   and composition root, and no line of it takes part in consensus. Its `harness` module (the
#   digest, block and network-state files of a relay run, and the seed rule of a mock peer, behind
#   the `difftest` feature, which no release build enables) is test support: it is left out, as
#   tests/ is, and listed apart, as heartwood-p2p's `testing` module and the forger's src/bin are.
# - heartwood-p2p-wire keeps 3,200: the server's request checks are the server's plugin logic and
#   live in heartwood-p2p's server, and the wire crate takes only the server-side inputs of the two
#   response encoders that held values twice.
# heartwood-p2p's server side is listed in two parts: the transport (the listener, the session, the
# bans, the plugins, the limits, the request checks, the node's traits and a devnet node's
# addressing) and the controllers with the forger's channel (the status, the attestation session,
# the network state).
# With the relay's security fixes, heartwood-p2p goes from 8,000 to 8,100 and heartwood-core from
# 3,000 to 3,100: the peer caps, the probe and fetch deadlines, the fork segment and reply checks
# and the chunk cache's byte bound add 122 lines to heartwood-p2p, and the marker check of the temp
# directory 24 to heartwood-core, which were at 7,956 and 2,985 of their ceilings before them.
# A read-only diagnostics command and its protocol server join the commands crate, which links no
# node crate: heartwood-cli goes from 3,500 to 8,600 and heartwood-core from 3,100 to 3,500. The
# doctor is budgeted at 4,170 lines in heartwood-cli (its runner, report, rendering and redaction,
# its sources and their parsers, 47 checks and the server) and 370 in heartwood-core (its command
# and the node's view for it), each ceiling the count before it plus the budget plus a quarter of
# the budget, rounded up to the next hundred. heartwood-database keeps its 3,000 with the doctor's
# read-only store opener. The doctor is listed on its own, without a ceiling of its own.
CHECKS=(
    "heartwood-crypto:packages/crypto/src:6000"
    "heartwood-kernel:packages/kernel/src:1900"
    "heartwood-kernel utils:packages/kernel/src/utils:400"
    "heartwood-logger:packages/logger/src:500"
    "heartwood-state:packages/state/src:3000"
    "heartwood-state wallets, ranking and rounds:packages/state/src/wallets packages/state/src/dpos packages/state/src/round_state.rs:"
    "heartwood-transactions:packages/transactions/src:2500"
    "heartwood-p2p-wire:packages/p2p-wire/src:3200"
    "heartwood-database:packages/database/src:3000"
    "heartwood-pool:packages/pool/src:2000"
    "heartwood-blockchain:packages/blockchain/src:3700"
    "heartwood-blockchain storage and restart:packages/blockchain/src/commit.rs packages/blockchain/src/rows.rs packages/blockchain/src/restore.rs packages/blockchain/src/milestones.rs packages/blockchain/src/online.rs:"
    "heartwood-blockchain processor:packages/blockchain/src/processor:1200"
    "heartwood-blockchain forger's blocks:packages/blockchain/src/forger_blocks.rs:"
    "heartwood-blockchain runtime:packages/blockchain/src/node.rs packages/blockchain/src/process_blocks_job.rs packages/blockchain/src/queue.rs packages/blockchain/src/pool_task.rs packages/blockchain/src/events.rs packages/blockchain/src/options.rs packages/blockchain/src/blockchain.rs packages/blockchain/src/state_machine packages/blockchain/src/fork_recovery.rs packages/blockchain/src/chain_state.rs packages/blockchain/src/network.rs packages/blockchain/src/work.rs:"
    "heartwood-p2p:packages/p2p/src !packages/p2p/src/testing:8100"
    "heartwood-p2p client side:packages/p2p/src/nes packages/p2p/src/transport.rs packages/p2p/src/connector.rs packages/p2p/src/rate_limiter.rs packages/p2p/src/utils packages/p2p/src/peer.rs packages/p2p/src/repository.rs packages/p2p/src/communicator.rs packages/p2p/src/attestation.rs packages/p2p/src/processor.rs packages/p2p/src/scoring.rs packages/p2p/src/verifier.rs packages/p2p/src/nsect.rs packages/p2p/src/monitor.rs packages/p2p/src/chunk_cache.rs packages/p2p/src/download.rs packages/p2p/src/broadcaster.rs packages/p2p/src/seeds.rs packages/p2p/src/diversity.rs packages/p2p/src/options.rs packages/p2p/src/view.rs packages/p2p/src/seams.rs packages/p2p/src/errors.rs packages/p2p/src/lib.rs:"
    "heartwood-p2p server transport:packages/p2p/src/server !packages/p2p/src/server/controllers packages/p2p/src/node_services.rs packages/p2p/src/devnet.rs:"
    "heartwood-p2p server controllers and forger channel:packages/p2p/src/server/controllers packages/p2p/src/status_server.rs packages/p2p/src/attestation_session.rs packages/p2p/src/network_state.rs:"
    "heartwood-p2p test support (difftest only):packages/p2p/src/testing:"
    "heartwood-api:packages/api/src:5000"
    "heartwood-core:packages/core/src !packages/core/src/harness.rs:3500"
    "heartwood-core test support (difftest only):packages/core/src/harness.rs:"
    "heartwood-forger:packages/forger/src !packages/forger/src/bin:2200"
    "heartwood-forger loop and relay client:packages/forger/src/forger_service.rs packages/forger/src/client.rs packages/forger/src/deadlines.rs:"
    "heartwood-forger forging, keys, record and attester:packages/forger/src/delegate.rs packages/forger/src/keys.rs packages/forger/src/signing_record.rs packages/forger/src/signer.rs packages/forger/src/attestation.rs:"
    "heartwood-forger test support (difftest only):packages/forger/src/bin:"
    "heartwood-cli:packages/cli/src !packages/cli/src/bin:8600"
    "heartwood-cli supervisor, commands and network generate:packages/cli/src/argv.rs packages/cli/src/instance.rs packages/cli/src/supervisor packages/cli/src/process.rs packages/cli/src/process_table.rs packages/cli/src/network.rs:"
    "heartwood-cli devnet:packages/cli/src/devnet packages/cli/src/http.rs:"
    "heartwood-cli doctor:packages/cli/src/doctor:"
    "heartwood-cli test support (test-child only):packages/cli/src/bin:"
)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Copy the Rust files of PATHS into DIR without their inline test modules.
strip_tests() {
    local dir=$1
    shift
    python3 - "$dir" "$@" <<'PY'
import os, re, sys
out = sys.argv[1]
excluded = [os.path.normpath(p[1:]) for p in sys.argv[2:] if p.startswith("!")]
paths = [p for p in sys.argv[2:] if not p.startswith("!")]
files = []
for p in paths:
    if os.path.isfile(p):
        files.append(p)
    else:
        for root, _, names in os.walk(p):
            files.extend(os.path.join(root, n) for n in names if n.endswith(".rs"))
# Files that are test-only modules: `#[cfg(test)] mod name;` names them.
test_files = set()
for f in files:
    text = open(f, encoding="utf-8").read()
    for m in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", text):
        base = os.path.dirname(f)
        stem = os.path.splitext(os.path.basename(f))[0]
        if stem not in ("mod", "lib", "main"):
            base = os.path.join(base, stem)
        test_files.add(os.path.normpath(os.path.join(base, m.group(1) + ".rs")))
        test_files.add(os.path.normpath(os.path.join(base, m.group(1), "mod.rs")))
files = [f for f in files if os.path.normpath(f) not in test_files]
files = [f for f in files
         if not any(os.path.normpath(f) == e or os.path.normpath(f).startswith(e + os.sep) for e in excluded)]
for i, f in enumerate(sorted(files)):
    lines = open(f, encoding="utf-8").read().split("\n")
    kept, skip, depth, pending = [], False, 0, False
    for line in lines:
        if skip:
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                skip = False
            continue
        if re.match(r"\s*#\[cfg\(test\)\]\s*$", line):
            pending = True
            continue
        if pending and re.match(r"\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{", line):
            depth = line.count("{") - line.count("}")
            skip = depth > 0
            pending = False
            continue
        if pending and re.match(r"\s*#\[", line):
            continue
        if pending:
            kept.append("#[cfg(test)]")
            pending = False
        kept.append(line)
    open(os.path.join(out, f"{i:04d}.rs"), "w", encoding="utf-8").write("\n".join(kept))
PY
}

fail=0
for check in "${CHECKS[@]}"; do
    IFS=: read -r name paths ceiling <<<"$check"
    # shellcheck disable=SC2086  # paths is a list
    existing=$(for p in $paths; do case $p in '!'*) echo "$p" ;; *) if [ -e "$p" ]; then echo "$p"; fi ;; esac; done)
    [ -n "$existing" ] || { echo "check-lines: $name: not in this tree"; continue; }
    dir="$work/$(echo "$name" | tr -c 'a-z0-9' '_')"
    mkdir -p "$dir"
    # shellcheck disable=SC2086
    strip_tests "$dir" $existing
    code=$(tokei --output json "$dir" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("Rust", {}).get("code", 0))')
    if [ -z "$ceiling" ]; then
        printf 'check-lines: %-45s %6s code lines\n' "$name" "$code"
    elif [ "$code" -le "$ceiling" ]; then
        printf 'check-lines: %-45s %6s of %6s\n' "$name" "$code" "$ceiling"
    else
        printf 'check-lines: %-45s %6s of %6s  OVER THE CEILING\n' "$name" "$code" "$ceiling" >&2
        fail=1
    fi
done
exit "$fail"
