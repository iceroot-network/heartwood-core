#!/usr/bin/env bash
# check-layers.sh: the dependency edges between the workspace's crates follow the layer map, no
# async runtime, storage or HTTP crate is in the dependency tree of the consensus crates, and only
# the store and the pool use SQLite directly.
#
#   tools/check-layers.sh
#
# Each workspace crate may depend (normal and build dependencies) only on the crates its rule
# lists; development dependencies are not checked. A crate without a rule fails the check, so a new
# crate needs a rule here first. Some external crates may be a direct dependency of named crates
# only (ONLY_UNDER). Needs cargo on PATH; reads the lockfile only (--locked --offline).
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
    [heartwood-database]="heartwood-crypto heartwood-kernel"
    [heartwood-pool]="heartwood-crypto heartwood-kernel heartwood-state heartwood-transactions"
    [heartwood-blockchain]="heartwood-crypto heartwood-kernel heartwood-state heartwood-transactions heartwood-database heartwood-pool"
    [heartwood-p2p]="heartwood-crypto heartwood-kernel heartwood-p2p-wire"
    [heartwood-difftest]="heartwood-crypto heartwood-kernel heartwood-logger heartwood-p2p heartwood-p2p-wire heartwood-state heartwood-transactions heartwood-database heartwood-pool heartwood-blockchain"
)
# external crate: the only workspace crates that may depend on it directly
declare -A ONLY_UNDER=(
    [tokio-tungstenite]="heartwood-p2p"
)
# The consensus crates, and the crates none of them may pull in.
CONSENSUS="heartwood-kernel heartwood-state heartwood-transactions"
FORBIDDEN="tokio rusqlite axum hyper"
# The only workspace crates that may depend on SQLite directly (normal dependencies).
SQLITE_USERS="heartwood-database heartwood-pool"

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
    external=$(python3 - "$metadata" "$crate" <<'PY'
import json, sys
meta = json.load(open(sys.argv[1]))
pkg = next(p for p in meta["packages"] if p["name"] == sys.argv[2])
print(" ".join(sorted({d["name"] for d in pkg["dependencies"] if d.get("kind") != "dev"})))
PY
)
    for dep in $external; do
        if [ -n "${ONLY_UNDER[$dep]+set}" ]; then
            case " ${ONLY_UNDER[$dep]} " in
                *" $crate "*) ;;
                *) echo "check-layers: $crate depends on $dep, which only ${ONLY_UNDER[$dep]} may" >&2; fail=1 ;;
            esac
        fi
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
# The crates that depend on rusqlite directly: the lines of depth 1 of its inverted tree.
direct=$({ cargo tree --locked --offline --workspace -i rusqlite -e normal --depth 1 \
    --prefix depth --format '{p}' 2>/dev/null || true; } |
    awk '/^1/ { sub(/^1/, ""); print $1 }' | sort -u)
for crate in $direct; do
    case " $SQLITE_USERS " in
        *" $crate "*) ;;
        *) echo "check-layers: $crate depends on rusqlite directly; only $SQLITE_USERS may" >&2; fail=1 ;;
    esac
done
# shellcheck disable=SC2086,SC2116  # the list on one line
users=$(echo $direct)
echo "check-layers: rusqlite is used directly by ${users:-no crate}"

[ "$fail" = 0 ] && echo "check-layers: every edge follows the layers; no $FORBIDDEN in $CONSENSUS"
exit "$fail"
