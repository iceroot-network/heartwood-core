#!/usr/bin/env bash
# check-lines.sh: the code lines of each crate against its review ceiling.
#
#   tools/check-lines.sh
#
# Code lines as tokei counts them (no blank or comment lines) in each crate's src/, without the
# inline test modules (`#[cfg(test)] mod ... { ... }`) and without tests/, benches and examples.
# The ceilings are the review budgets of the crates; a crate over its ceiling fails the check.
# Some parts are also listed on their own (informational, or with their own ceiling).
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
# side is listed on its own.
CHECKS=(
    "heartwood-crypto:packages/crypto/src:6000"
    "heartwood-kernel:packages/kernel/src:1500"
    "heartwood-kernel utils:packages/kernel/src/utils:400"
    "heartwood-logger:packages/logger/src:500"
    "heartwood-state:packages/state/src:3000"
    "heartwood-state wallets, ranking and rounds:packages/state/src/wallets packages/state/src/dpos packages/state/src/round_state.rs:"
    "heartwood-transactions:packages/transactions/src:2500"
    "heartwood-p2p-wire:packages/p2p-wire/src:2500"
    "heartwood-database:packages/database/src:3000"
    "heartwood-pool:packages/pool/src:2000"
    "heartwood-blockchain:packages/blockchain/src:3500"
    "heartwood-blockchain storage and restart:packages/blockchain/src/commit.rs packages/blockchain/src/rows.rs packages/blockchain/src/restore.rs packages/blockchain/src/milestones.rs:"
    "heartwood-p2p:packages/p2p/src:6000"
    "heartwood-p2p client side:packages/p2p/src/nes packages/p2p/src/transport.rs packages/p2p/src/connector.rs packages/p2p/src/testing packages/p2p/src/rate_limiter.rs packages/p2p/src/utils packages/p2p/src/peer.rs packages/p2p/src/repository.rs packages/p2p/src/communicator.rs packages/p2p/src/attestation.rs packages/p2p/src/processor.rs packages/p2p/src/scoring.rs packages/p2p/src/verifier.rs packages/p2p/src/nsect.rs packages/p2p/src/monitor.rs packages/p2p/src/chunk_cache.rs packages/p2p/src/download.rs packages/p2p/src/broadcaster.rs packages/p2p/src/seeds.rs packages/p2p/src/diversity.rs packages/p2p/src/options.rs packages/p2p/src/view.rs packages/p2p/src/seams.rs packages/p2p/src/errors.rs packages/p2p/src/lib.rs:"
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
paths = sys.argv[2:]
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
    existing=$(for p in $paths; do if [ -e "$p" ]; then echo "$p"; fi; done)
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
