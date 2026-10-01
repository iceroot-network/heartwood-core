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
# With the reference's own clients on the server and the recovery commands (each ceiling is the
# crate's count before them plus the work planned for them, rounded up to the next 100):
# - heartwood-p2p goes from 8,100 to 8,500: it measured 8,078; the served answers made conformant to
#   the reference's clients (up to 150 lines in the controllers and the served status), the
#   transport's fixes for them (up to 150 in the listener, the session, the bans and the plugins)
#   and the AwaitBlock record (the wait's outcome, the socket's close signal and the record's call
#   site, 91 lines) add about 390.
# - heartwood-blockchain goes from 3,700 to 3,800: it measured 3,665; `snapshot rollback` (the
#   reference's target and refusals, and the store change in one transaction) and the start's write
#   of the next round's rows after it add up to 95, and the job stall's field 4. The rollback is
#   listed on its own. Its `difftest` module (the job stall, behind the `difftest` feature, which no
#   release build enables) is test support: it is left out and listed apart, as the other crates'
#   are. With the bodies built it goes to 3,900: it measured 3,837, 172 above 3,665, since the
#   rollback took 140 lines where 80 were planned (62 lines of signatures written before its
#   bodies, and 78 of bodies: the refusals' lines, the plan, the store change with its row counts
#   and the reference's three lines), the start's write of the next round's rows 20 (16 in
#   commit.rs, 4 in restore.rs), the job stall's field and call 8 (4 in node.rs, 4 in
#   process_blocks_job.rs) and the rollback module's declaration and re-export 4. The start's
#   productivity backfill (21 in restore.rs, 4 in commit.rs) and the rollback's first lines logged
#   before its deletes (4) bring it to 3,866.
# - heartwood-core goes from 3,100 to 3,200: it measured 3,009; the rollback command (up to 60
#   lines), the two clears' dispatch and their marker check (20), the AwaitBlock record's override
#   (10) and the three commands' flags and wiring (68) join the command tree. The clears' bodies
#   live in heartwood-cli, where they need only paths.
# - heartwood-cli goes from 3,500 to 3,600: it measured 3,347; the recovery commands (the markers,
#   the question, the temp directory's trust rule and the clears' bodies, up to 175 lines) and
#   their colon spellings (4) join it. The recovery commands are listed on their own. It goes to
#   3,700 with the supervisor's run-file checks (the run directory's files opened relative to the
#   directory that was checked, and a run file that is not a regular file refused): they took it
#   from 3,347 to 3,405 without the recovery commands, and with the recovery commands' 229 lines
#   (225 in recovery.rs and the colon spellings' 4) it measures 3,634.
# A read-only diagnostics command and its protocol server join the commands crate, which links no
# node crate: heartwood-cli goes from 3,500 to 8,600 and heartwood-core from 3,100 to 3,500. The
# doctor is budgeted at 4,170 lines in heartwood-cli (its runner, report, rendering and redaction,
# its sources and their parsers, 47 checks and the server) and 370 in heartwood-core (its command
# and the node's view for it), each ceiling the count before it plus the budget plus a quarter of
# the budget, rounded up to the next hundred. heartwood-database keeps its 3,000 with the doctor's
# read-only store opener. The doctor is listed on its own, without a ceiling of its own.
# With the doctor built, heartwood-cli goes from 8,600 to 10,300: the crate measures 10,201 lines,
# of which the doctor takes 6,852 against its budget of 4,170. Its runner, seams, parsed view,
# registry, documentation loader and report types take 2,086 (the view alone 856: every source
# read once within the run's budget, its reasons and their conversions); the rendering and the
# redaction 504; the host's files, /proc tables and probes 665 (reads that follow no final link,
# the key files refused by device and inode, fixed probes with a deadline and an output cap); the
# logs and the API 1,212 (every log template as ordered literal pieces with its level and emoji,
# line integrity, rotated files, the loopback address rule and the request budget); the 47 checks
# 2,016, with the confirmations and boundaries of their rules; the protocol server 369. No
# reviewed line was cut to meet the earlier ceiling. The review's fixes took heartwood-cli to 10,295
# (the doctor 6,946), heartwood-core to 3,500 and heartwood-database to 2,627.
# A second round of the review's fixes goes heartwood-cli from 10,300 to 10,400 and heartwood-core
# from 3,500 to 3,600, each the measured count rounded up to the next hundred with no further
# addition planned in this change: heartwood-cli measures 10,328 (the doctor 6,979) and
# heartwood-core 3,508. The store window's own tip and its start below it (the slot checks and the
# version floor read the store with the relay's API off), the plain-unit sizes and the phrases of
# fixed words in the advice take heartwood-cli's 33 lines; the node view's start and the words
# in explain take heartwood-core's 8. heartwood-database, at 2,640, keeps its 3,000. The check of
# that round took heartwood-cli to 10,329 (the doctor 6,980): the clock offset as a millisecond fact.
# With the recovery commands and the doctor both in, heartwood-cli goes from 10,400 to 10,700 and
# heartwood-core from 3,600 to 3,700, each the merged count rounded up to the next hundred with no
# further addition planned in the merge. Each side fit its own ceiling (heartwood-cli 3,634 of
# 3,700 and 10,329 of 10,400, heartwood-core 3,158 of 3,200 and 3,508 of 3,600), but both had
# counted the room left under the base's ceiling, so the merged heartwood-cli measures 10,616 (the
# doctor 6,980), over the base's ceiling plus both raises (10,600), and heartwood-core 3,657.
# heartwood-database, at 2,699, keeps its 3,000.
# With the listing routes over the state and the store (each ceiling is the crate's count with the
# listings' seams, plus the work planned for them and a quarter of it, plus the recovery commands'
# work not yet spent in the same crate, rounded up to the next 100):
# - heartwood-api goes from 5,000 to 8,200: it measured 5,046 with the route table, the
#   productivity seam, the search signatures and the handlers' stubs; the delegate and wallet
#   listings with their criteria (up to 650 lines), the pagination through the reference's tree
#   (400), the wallet search with its collation and sort (450), the productivity over the store
#   (150), the history routes (500) and the chain routes with the fee statistics, the schemas and
#   the OpenAPI document (350) add about 2,500, a quarter of it 625, and the API root 20. The
#   in-memory listings and the store listings are listed on their own.
# - heartwood-database goes from 3,000 to 3,300: it measured 2,622 with the listings' readers'
#   signatures, their filters and the index-only schema step; the readers' bodies (up to 450
#   lines), the saved wallets' order if it must change (20), a quarter of those, and the rollback's
#   store work (37) join it. The listings' readers are listed on their own.
# - heartwood-state goes from 3,000 to 3,100: it measured 2,861 with the index-order iterators'
#   signatures; the indexes kept in the reference's insertion order (up to 120 lines) and a quarter
#   of it join it.
# With the listings written (the same rule, on the measured count):
# - heartwood-api goes from 8,200 to 8,300: the listings measured 8,210, 3,164 lines above the
#   5,046 of their seams where 2,500 were planned: the state listings with every criterion kind,
#   its conversions and its validation 1,153 (650 planned), the history routes with the store's
#   refusals by column type 579 (500), the chain routes, the fees and the document 478 (350), the
#   pagination 376, the wallet search 427 and the productivity 151; the API root (20 lines) joins it
#   when the listings are carried onto it.
# With the listings carried onto the recovery commands and the diagnostics command (the same rule):
# - heartwood-database goes from 3,300 to 3,400: the listings' 3,300 counted the rollback's
#   planned store work but not the diagnostics command's read-only store opener and its reads, so
#   the merged crate measures 3,330 (2,699 before the listings, which added 631); no further store
#   work is planned.
# With the operator commands, the combined role, the devnet library's flags and the scenario
# runner's entry (each ceiling the crate's count with their seams, plus the work planned for them in
# this change and the next, and a quarter of that work, rounded up to the next 100):
# - heartwood-cli goes from 10,700 to 12,400: it measures 10,967 with their seams (the instance
#   index, top, env, config and service with their typed errors and unbuilt bodies, the
#   service-manager record in full with its test, the combined role among the roles, the devnet
#   library's new flags and its run entry: 351 lines); top, the index and env (up to 350 lines),
#   the devnet flags (400), the runner's entry (40), config and service (350) and a quarter of
#   those (285) join it. The operator commands are listed on their own.
# - heartwood-core goes from 3,700 to 4,400: it measures 3,947 with the command tree of the new
#   families (top, env, config, service and the combined role, one file each with the reference's
#   flags, and the verbose version: 279 lines) and the combined role's root; the commands of top
#   and env (30), the combined role (240), config and service (30) and a quarter of those (75)
#   join it. None of these lines takes part in consensus.
# - heartwood-p2p keeps 8,500: it measures 8,251 with the attestation record's types and hook, and
#   the record's three call sites (up to 10 lines) fit.
# heartwood-forger goes from 2,200 to 2,300: it measures 2,172, and the work planned in it next
# (the forger's hooks, about 12 lines, the sort of the block header's fields, about 30, and the
# network hash's comparison, about 10) takes it to about 2,224; the rest leaves room for the
# signing record's rewrite.
CHECKS=(
    "heartwood-crypto:packages/crypto/src:6000"
    "heartwood-kernel:packages/kernel/src:1900"
    "heartwood-kernel utils:packages/kernel/src/utils:400"
    "heartwood-logger:packages/logger/src:500"
    "heartwood-state:packages/state/src:3100"
    "heartwood-state wallets, ranking and rounds:packages/state/src/wallets packages/state/src/dpos packages/state/src/round_state.rs:"
    "heartwood-transactions:packages/transactions/src:2500"
    "heartwood-p2p-wire:packages/p2p-wire/src:3200"
    "heartwood-database:packages/database/src:3400"
    "heartwood-database listing readers:packages/database/src/history/blocks_list.rs packages/database/src/history/missed.rs packages/database/src/history/productivity.rs packages/database/src/history/fees.rs packages/database/src/history/totals.rs packages/database/src/filters/block.rs packages/database/src/filters/missed_block.rs:"
    "heartwood-pool:packages/pool/src:2000"
    "heartwood-blockchain:packages/blockchain/src !packages/blockchain/src/difftest.rs:3900"
    "heartwood-blockchain storage and restart:packages/blockchain/src/commit.rs packages/blockchain/src/rows.rs packages/blockchain/src/restore.rs packages/blockchain/src/milestones.rs packages/blockchain/src/online.rs:"
    "heartwood-blockchain processor:packages/blockchain/src/processor:1200"
    "heartwood-blockchain forger's blocks:packages/blockchain/src/forger_blocks.rs:"
    "heartwood-blockchain rollback:packages/blockchain/src/rollback.rs:"
    "heartwood-blockchain runtime:packages/blockchain/src/node.rs packages/blockchain/src/process_blocks_job.rs packages/blockchain/src/queue.rs packages/blockchain/src/pool_task.rs packages/blockchain/src/events.rs packages/blockchain/src/options.rs packages/blockchain/src/blockchain.rs packages/blockchain/src/state_machine packages/blockchain/src/fork_recovery.rs packages/blockchain/src/chain_state.rs packages/blockchain/src/network.rs packages/blockchain/src/work.rs:"
    "heartwood-blockchain test support (difftest only):packages/blockchain/src/difftest.rs:"
    "heartwood-p2p:packages/p2p/src !packages/p2p/src/testing:8500"
    "heartwood-p2p client side:packages/p2p/src/nes packages/p2p/src/transport.rs packages/p2p/src/connector.rs packages/p2p/src/rate_limiter.rs packages/p2p/src/utils packages/p2p/src/peer.rs packages/p2p/src/repository.rs packages/p2p/src/communicator.rs packages/p2p/src/attestation.rs packages/p2p/src/processor.rs packages/p2p/src/scoring.rs packages/p2p/src/verifier.rs packages/p2p/src/nsect.rs packages/p2p/src/monitor.rs packages/p2p/src/chunk_cache.rs packages/p2p/src/download.rs packages/p2p/src/broadcaster.rs packages/p2p/src/seeds.rs packages/p2p/src/diversity.rs packages/p2p/src/options.rs packages/p2p/src/view.rs packages/p2p/src/seams.rs packages/p2p/src/errors.rs packages/p2p/src/lib.rs:"
    "heartwood-p2p server transport:packages/p2p/src/server !packages/p2p/src/server/controllers packages/p2p/src/node_services.rs packages/p2p/src/devnet.rs:"
    "heartwood-p2p server controllers and forger channel:packages/p2p/src/server/controllers packages/p2p/src/status_server.rs packages/p2p/src/attestation_session.rs packages/p2p/src/network_state.rs:"
    "heartwood-p2p test support (difftest only):packages/p2p/src/testing:"
    "heartwood-api:packages/api/src:8300"
    "heartwood-api in-memory listings:packages/api/src/search:"
    "heartwood-api store listings:packages/api/src/controllers/delegate_history.rs packages/api/src/controllers/rounds.rs packages/api/src/controllers/votes.rs packages/api/src/controllers/blockchain.rs packages/api/src/resources/missed_block.rs packages/api/src/resources/round.rs packages/api/src/resources/fee_statistics.rs:"
    "heartwood-core:packages/core/src !packages/core/src/harness.rs:4400"
    "heartwood-core test support (difftest only):packages/core/src/harness.rs:"
    "heartwood-forger:packages/forger/src !packages/forger/src/bin:2300"
    "heartwood-forger loop and relay client:packages/forger/src/forger_service.rs packages/forger/src/client.rs packages/forger/src/deadlines.rs:"
    "heartwood-forger forging, keys, record and attester:packages/forger/src/delegate.rs packages/forger/src/keys.rs packages/forger/src/signing_record.rs packages/forger/src/signer.rs packages/forger/src/attestation.rs:"
    "heartwood-forger test support (difftest only):packages/forger/src/bin:"
    "heartwood-cli:packages/cli/src !packages/cli/src/bin:12400"
    "heartwood-cli supervisor, commands and network generate:packages/cli/src/argv.rs packages/cli/src/instance.rs packages/cli/src/supervisor packages/cli/src/process.rs packages/cli/src/process_table.rs packages/cli/src/network.rs:"
    "heartwood-cli devnet:packages/cli/src/devnet packages/cli/src/http.rs:"
    "heartwood-cli recovery commands:packages/cli/src/recovery.rs:"
    "heartwood-cli operator commands:packages/cli/src/top.rs packages/cli/src/instance_index.rs packages/cli/src/env.rs packages/cli/src/config.rs packages/cli/src/service.rs:"
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
