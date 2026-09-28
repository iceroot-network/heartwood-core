#!/usr/bin/env bash
# test-integration.sh: run every test marked `#[ignore = "integration"]`.
#
#   tools/test-integration.sh
#
# The marker keeps a test out of a plain `cargo test`: such a test needs more than its own crate's
# unit (the served API over a store, a relay importing from a mock peer, the harness's built
# binaries). This script finds each marked test in the workspace's sources, lists the ignored tests
# of its crate's test target (with every feature of the crate, as the second pass of `make test`
# builds it), and runs the marked ones by their exact names. A marked test that its target does not
# list fails the run, so a test cannot drop out unseen. Tests ignored for another reason (the
# measurements of `make pool-bench`, child-process helpers, the devnet tests) are not run; each such
# reason must be in ALLOWED_REASONS, and a test ignored with any other reason, or with none, fails
# the run before anything is built, so a new reason is added deliberately.
#
# The harness's binaries (difftest, the relay with its test seams, the mock peer) are built first,
# as `make build` builds them: the runtime kill test runs them.
set -euo pipefail
cd "$(dirname "$0")/.."

MARKER='#[ignore = "integration"]'

# The reasons a test may be ignored for, besides the marker: the measurements that `make
# pool-bench` runs, the child processes their parent tests run, and the tests that need a devnet.
ALLOWED_REASONS=(
    'integration'
    'a measurement: make pool-bench'
    'a measurement, run in a release build by make pool-bench'
    'run by the crash test in a child process'
    'run by view_cost in a child process'
    'devnet'
)

# Every `#[ignore` of the sources whose reason is not an allowed one, as `file:line: text`.
unknown_ignores() {
    local line reason ok
    { grep -rnE --include='*.rs' '^[[:space:]]*#\[ignore' packages tools || true; } | while IFS= read -r line; do
        reason=$(sed -nE 's/^[^:]*:[0-9]+:[[:space:]]*#\[ignore = "([^"]*)"\][[:space:]]*$/\1/p' <<<"$line")
        for ok in "${ALLOWED_REASONS[@]}"; do
            if [ -n "$reason" ] && [ "$reason" = "$ok" ]; then continue 2; fi
        done
        echo "$line"
    done
}

unknown=$(unknown_ignores)
if [ -n "$unknown" ]; then
    echo "test-integration: tests ignored with a reason outside ALLOWED_REASONS, or none, which no" \
         "gate runs (add the reason to the list deliberately, or mark the test integration):" >&2
    echo "$unknown" >&2
    exit 1
fi

# The package that owns FILE: the name in the nearest Cargo.toml above it.
package_of() {
    local dir
    dir=$(dirname "$1")
    while [ "$dir" != . ] && [ ! -f "$dir/Cargo.toml" ]; do dir=$(dirname "$dir"); done
    [ -f "$dir/Cargo.toml" ] || { echo "test-integration: no Cargo.toml above $1" >&2; return 1; }
    awk '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 } p && /^name *=/ { gsub(/[" ]/, "", $0); sub(/^name=/, ""); print; exit }' \
        "$dir/Cargo.toml"
}

# The cargo target flag of FILE: --lib for a source file, --test NAME for tests/NAME.rs.
target_of() {
    case $1 in
        */tests/*/*) echo "test-integration: $1: a marked test in a test subdirectory is not supported" >&2; return 1 ;;
        */tests/*.rs) local name=${1##*/}; echo "--test ${name%.rs}" ;;
        */src/*) echo "--lib" ;;
        *) echo "test-integration: $1: not a test target this script knows" >&2; return 1 ;;
    esac
}

# The names of the functions marked in FILE: the first `fn NAME` after each marker.
marked_in() {
    awk -v marker="$MARKER" '
        index($0, marker) { want = 1; next }
        want && match($0, /fn [A-Za-z0-9_]+/) { print substr($0, RSTART + 3, RLENGTH - 3); want = 0 }
    ' "$1"
}

mapfile -t files < <(grep -rlF --include='*.rs' "$MARKER" packages tools | sort)
if [ "${#files[@]}" -eq 0 ]; then
    echo "test-integration: no test is marked integration"
    exit 0
fi

# Group the marked tests by package and target.
declare -A wanted
for file in "${files[@]}"; do
    key="$(package_of "$file") $(target_of "$file")"
    wanted[$key]+=" $(marked_in "$file" | tr '\n' ' ')"
done

cargo build --locked

total=0
failed=0
for key in $(printf '%s\n' "${!wanted[@]}" | sort | tr ' ' '|'); do
    read -r package target <<<"${key//|/ }"
    names=${wanted["$package $target"]}
    # shellcheck disable=SC2086  # target is a flag and its value
    listed=$(cargo test --locked -q -p "$package" --all-features $target -- --ignored --list 2>/dev/null |
        sed -n 's/: test$//p')
    exact=()
    for name in $names; do
        found=$(grep -E "(^|::)$name\$" <<<"$listed" || true)
        if [ "$(grep -c . <<<"$found")" -ne 1 ]; then
            echo "test-integration: $package $target: the marked test $name is not listed once among its ignored tests" >&2
            failed=1
            continue
        fi
        exact+=("$found")
    done
    [ "${#exact[@]}" -gt 0 ] || continue
    echo "test-integration: $package $target: ${#exact[@]} tests"
    # shellcheck disable=SC2086
    if ! cargo test --locked -p "$package" --all-features $target -- --ignored --exact "${exact[@]}"; then
        failed=1
    fi
    total=$((total + ${#exact[@]}))
done

echo "test-integration: $total marked tests run in ${#wanted[@]} targets"
exit "$failed"
