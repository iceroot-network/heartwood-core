#!/usr/bin/env bash
# check-layers.sh: the dependency edges between the workspace's crates follow the layer map, and
# no async runtime, storage or HTTP crate is in the dependency tree of the consensus crates.
#
#   tools/check-layers.sh
#
# Each workspace crate may depend (normal and build dependencies) only on the crates its rule
# lists; development dependencies are not checked. A crate without a rule fails the check, so a new
# crate needs a rule here first. Needs cargo on PATH; reads the lockfile only (--locked --offline).
set -euo pipefail
cd "$(dirname "$0")/.."

# crate: the workspace crates it may depend on
declare -A ALLOWED=(
    [heartwood-crypto]=""
    [heartwood-kernel]="heartwood-crypto"
    [heartwood-logger]="heartwood-kernel"
    [heartwood-state]="heartwood-crypto heartwood-kernel"
    [heartwood-transactions]="heartwood-crypto heartwood-kernel heartwood-state"
    [heartwood-p2p-wire]="heartwood-crypto heartwood-kernel"
    [heartwood-difftest]="heartwood-crypto heartwood-kernel heartwood-logger heartwood-p2p-wire heartwood-state heartwood-transactions"
)
# The consensus crates, and the crates none of them may pull in.
CONSENSUS="heartwood-kernel heartwood-state heartwood-transactions"
FORBIDDEN="tokio rusqlite axum hyper"

fail=0
metadata=$(mktemp)
trap 'rm -f "$metadata"' EXIT
cargo metadata --locked --offline --format-version 1 --no-deps >"$metadata"
members=$(python3 -c 'import json,sys; print(" ".join(p["name"] for p in json.load(open(sys.argv[1]))["packages"]))' "$metadata")
for crate in $members; do
    if [ -z "${ALLOWED[$crate]+set}" ]; then
        echo "check-layers: $crate has no layer rule" >&2
        fail=1
        continue
    fi
    deps=$(python3 - "$metadata" "$crate" <<'PY'
import json, sys
meta = json.load(open(sys.argv[1]))
pkg = next(p for p in meta["packages"] if p["name"] == sys.argv[2])
names = {p["name"] for p in meta["packages"]}
print(" ".join(sorted({d["name"] for d in pkg["dependencies"] if d["name"] in names and d.get("kind") != "dev"})))
PY
)
    for dep in $deps; do
        case " ${ALLOWED[$crate]} " in
            *" $dep "*) ;;
            *) echo "check-layers: $crate depends on $dep, which its layer does not allow" >&2; fail=1 ;;
        esac
    done
    echo "check-layers: $crate -> ${deps:-(no workspace crate)}"
done

for crate in $CONSENSUS; do
    tree=$(cargo tree --locked --offline -p "$crate" -e normal,build --prefix none --format '{p}' | awk '{print $1}' | sort -u)
    for bad in $FORBIDDEN; do
        if grep -qx "$bad" <<<"$tree"; then
            echo "check-layers: $bad is in the dependency tree of $crate" >&2
            fail=1
        fi
    done
done
[ "$fail" = 0 ] && echo "check-layers: every edge follows the layers; no $FORBIDDEN in $CONSENSUS"
exit "$fail"
