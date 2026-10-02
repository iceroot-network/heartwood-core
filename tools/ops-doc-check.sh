#!/usr/bin/env bash
# ops-doc-check.sh: run the operator guide's Linux command blocks as written, in order, and fail on
# the first block that does not do what the guide says.
#
#   tools/ops-doc-check.sh [GUIDE]
#
# GUIDE is the guide's Markdown file (docs/operating.md by default).
#
#   HEARTWOOD_DEVNET_PREFIX   the loopback block of the guide's devnet: the first three octets,
#                             such as 127.49.0 (required)
#   OPS_DOC_BINARY            the heartwood binary [target/release/heartwood]
#   OPS_DOC_ROOT              where the scratch data roots are made [target/ops-doc-check]
#
# Exit status: 0 every block ran as the guide says, 1 a block or a rule of the guide failed, 2 a
# usage error or a machine that cannot run the check.
#
# What counts as a command block. A fenced block whose language is `sh` is a command block and
# runs; `text`, `json` and `ini` blocks are shown, never run; any other fence language, a fence
# without one and an indented fence are errors, so that no command can hide from the check. After
# `sh` a fence may carry words, which the check reads and a Markdown viewer ignores:
#
#   second          run on the second machine (the forger's host) instead of the first
#   follows         the command follows a log until it is interrupted: stop it after 8 s, success
#   fails           the block must end with a failure (a refusal the guide shows)
#   timeout=N       give the block N seconds instead of 240
#   optional=TOOL   skip the block with a note when TOOL is not installed on this machine
#   expect="TEXT"   the block's output must contain TEXT (repeat the word for more than one)
#
# Where the blocks run. The check makes a private network of two machines without root: one user
# and network namespace for the first machine (its own loopback, so the default addresses of a
# node, 127.0.0.1, are free), and a second network namespace inside it for the second machine,
# joined by a veth pair: 10.90.0.1 (first) and 10.90.0.2 (second). Nothing is published on the
# host and no host address, route or sysctl is touched. Each machine has its own scratch home
# directory under OPS_DOC_ROOT, so a block reads and writes nothing of this machine's own
# instances, its own temporary directory, and its own runtime directory (XDG_RUNTIME_DIR, mode
# 700), as a login session has one. Blocks run with a clean environment, the
# heartwood binary first on PATH. A block's processes live in these namespaces, and the check
# stops every one of them, found by its namespace, before it ends.
#
# Three stand-ins let a block be written as an operator writes it. The guide's devnet addresses
# use the prefix 127.49.0; the check replaces it with HEARTWOOD_DEVNET_PREFIX. `scp` copies to
# and from a machine's home directory, `forger-host:` being the second machine and
# `relay-host:` the first (there is no ssh server to reach in a sandbox); it takes the options
# -p, -r and -q.
#
# Rules of the guide, checked before any block runs: no macOS or launchd step (nodes run on
# Linux only), no unmarked or unknown fence, and at least one command block.
set -uo pipefail

usage() {
    echo "usage: ops-doc-check.sh [GUIDE]" >&2
    exit 2
}
fail() {
    echo "ops-doc-check: $*" >&2
    exit 1
}
refuse() {
    echo "ops-doc-check: $*" >&2
    exit 2
}

[ $# -le 1 ] || usage
guide=${1:-docs/operating.md}
[ -f "$guide" ] || refuse "$guide does not exist"
prefix=${HEARTWOOD_DEVNET_PREFIX:-}
[ -n "$prefix" ] || refuse "set HEARTWOOD_DEVNET_PREFIX, the loopback block of the guide's devnet"
[[ $prefix =~ ^127\.[0-9]{1,3}\.[0-9]{1,3}$ ]] || refuse "HEARTWOOD_DEVNET_PREFIX=$prefix is not three octets of 127.0.0.0/8, such as 127.49.0"
binary=${OPS_DOC_BINARY:-target/release/heartwood}
[ -x "$binary" ] || refuse "$binary is not an executable (build it: cargo build --release --locked -p heartwood-core)"
binary=$(readlink -f -- "$binary")
root=${OPS_DOC_ROOT:-target/ops-doc-check}
mkdir -p -- "$root" || refuse "cannot make $root"
root=$(cd -- "$root" && pwd)
guide=$(readlink -f -- "$guide")
for tool in unshare nsenter ip curl timeout setsid awk sed grep; do
    command -v "$tool" >/dev/null || refuse "this check needs $tool"
done

# ---- the guide's rules and its command blocks ----

if hits=$(grep -n -i -E 'macos|mac os|\bosx\b|darwin|launchd|homebrew|\bbrew\b' "$guide"); then
    echo "$hits" >&2
    fail "the guide has a macOS step or mention above: nodes run on Linux only"
fi

run=$root/run
if [ -e "$run" ]; then
    [ -f "$run/.ops-doc-check" ] || refuse "$run exists and was not made by this check; remove it"
    rm -rf -- "$run"
fi
mkdir -p -- "$run/blocks" "$run/logs" "$run/bin" "$run/m1/home" "$run/m1/tmp" "$run/m2/home" "$run/m2/tmp" || refuse "cannot make $run"
mkdir -m 700 -- "$run/m1/rt" "$run/m2/rt" || refuse "cannot make the machines' runtime directories"
touch "$run/.ops-doc-check"

if ! awk -v dir="$run/blocks" '
    function bad(message) { print FILENAME ":" NR ": " message > "/dev/stderr"; errors++ }
    /^[ \t]+```/ { bad("an indented fence: write every fence at the start of its line"); next }
    /^```/ {
        if (!inside) {
            info = substr($0, 4); sub(/^[ \t]+/, "", info)
            lang = info; sub(/[ \t].*$/, "", lang)
            rest = info; sub(/^[^ \t]*[ \t]*/, "", rest)
            inside = 1; keep = 0
            if (lang == "sh") {
                keep = 1; count++
                file = sprintf("%s/%03d.sh", dir, count); meta = sprintf("%s/%03d.meta", dir, count)
                printf "" > file
                print NR "\t" rest > meta; close(meta)
            } else if (lang != "text" && lang != "json" && lang != "ini") {
                bad("the fence language \"" lang "\" is not sh, text, json or ini")
            }
        } else {
            if ($0 !~ /^```[ \t]*$/) bad("a closing fence with text after it")
            inside = 0
            if (keep) close(file)
        }
        next
    }
    inside && keep { print $0 >> file }
    END {
        if (inside) bad("a fence that is never closed")
        if (count == 0) bad("no command block (a fence in the language sh)")
        if (errors) exit 1
        print count
    }
' "$guide" >"$run/count"; then
    fail "the guide's fences are not in order (above)"
fi
count=$(cat "$run/count")

# ---- two private machines ----

holder_a=""
holder_b=""
nets=()

spawn_holder() { # spawn_holder PIDFILE COMMAND...: a detached sleeper in the namespaces COMMAND makes
    local pidfile=$1 i
    shift
    rm -f -- "$pidfile"
    setsid bash -c 'echo $$ >"$1"; shift; exec "$@" sleep infinity' _ "$pidfile" "$@" >/dev/null 2>&1 </dev/null &
    for i in $(seq 1 50); do [ -s "$pidfile" ] && break; sleep 0.1; done
    [ -s "$pidfile" ] || return 1
    sleep 0.2
    cat "$pidfile"
}
enter() { # enter PID COMMAND...: as the namespaces' root, in the network namespace of process PID
    local pid=$1
    shift
    nsenter -t "$pid" -U -n --preserve-credentials -- "$@"
}
members() { # the pids in the two machines' network namespaces, the holders aside
    local p ns n
    for p in /proc/[0-9]*; do
        p=${p#/proc/}
        [ "$p" = "$$" ] || [ "$p" = "$holder_a" ] || [ "$p" = "$holder_b" ] && continue
        ns=$(readlink "/proc/$p/ns/net" 2>/dev/null) || continue
        for n in "${nets[@]+"${nets[@]}"}"; do
            [ -n "$n" ] && [ "$ns" = "$n" ] && echo "$p"
        done
    done
}
stop_machines() { # every process of the two machines, then the holders: TERM, a wait, then KILL
    local left i
    left=$(members)
    if [ -n "$left" ]; then
        # shellcheck disable=SC2086
        kill -TERM $left 2>/dev/null
        for i in $(seq 1 80); do
            left=$(members)
            [ -n "$left" ] || break
            sleep 0.5
        done
        if [ -n "$left" ]; then
            # shellcheck disable=SC2086
            kill -KILL $left 2>/dev/null
            sleep 0.5
        fi
    fi
    [ -z "$holder_b" ] || kill -KILL "$holder_b" 2>/dev/null
    [ -z "$holder_a" ] || kill -KILL "$holder_a" 2>/dev/null
    return 0
}
finish() {
    local status=$?
    trap - EXIT INT TERM
    stop_machines
    exit "$status"
}
trap finish EXIT
trap 'exit 130' INT TERM

holder_a=$(spawn_holder "$run/holder-1" unshare -r -n) || refuse "cannot make a user and network namespace (is kernel.apparmor_restrict_unprivileged_userns 0?)"
nets+=("$(readlink "/proc/$holder_a/ns/net")")
holder_b=$(spawn_holder "$run/holder-2" nsenter -t "$holder_a" -U -n --preserve-credentials -- unshare -n) || refuse "cannot make the second machine's network namespace"
nets+=("$(readlink "/proc/$holder_b/ns/net")")
[ "${nets[0]}" != "${nets[1]}" ] || refuse "the two machines share a network namespace"
{
    enter "$holder_a" ip link set lo up &&
        enter "$holder_b" ip link set lo up &&
        enter "$holder_a" ip link add relay-link type veth peer name forger-link &&
        enter "$holder_a" ip link set forger-link netns "$holder_b" &&
        enter "$holder_a" ip addr add 10.90.0.1/24 dev relay-link &&
        enter "$holder_a" ip link set relay-link up &&
        enter "$holder_b" ip addr add 10.90.0.2/24 dev forger-link &&
        enter "$holder_b" ip link set forger-link up
} || refuse "cannot set up the private network of the two machines"

# The scp stand-in: the machines share this host's disk, so a copy is a copy between homes.
cat >"$run/bin/scp" <<'SCP'
#!/usr/bin/env bash
set -euo pipefail
flags=()
args=()
for arg in "$@"; do
    case "$arg" in
        -p | -r) flags+=("$arg") ;;
        -q) ;;
        -*) echo "scp: this stand-in takes -p, -r and -q only (got $arg)" >&2; exit 2 ;;
        *) args+=("$arg") ;;
    esac
done
[ "${#args[@]}" -ge 2 ] || { echo "usage: scp [-p] [-r] [-q] source... [host:]destination" >&2; exit 2; }
place() {
    case "$1" in
        forger-host:*) echo "$OPS_HOME_2/${1#*:}" ;;
        relay-host:*) echo "$OPS_HOME_1/${1#*:}" ;;
        *:*) echo "scp: no such host: ${1%%:*}" >&2; exit 1 ;;
        *) echo "$1" ;;
    esac
}
last=$((${#args[@]} - 1))
destination=$(place "${args[$last]}")
unset "args[$last]"
sources=()
for arg in "${args[@]}"; do sources+=("$(place "$arg")"); done
cp "${flags[@]}" -- "${sources[@]}" "$destination"
SCP
chmod +x "$run/bin/scp"
ln -s "$binary" "$run/bin/heartwood"

# ---- the blocks ----

block_env() { # block_env MACHINE: the clean environment of a machine's commands
    echo "HOME=$run/m$1/home" "USER=operator" "LANG=C" "TERM=dumb" "TMPDIR=$run/m$1/tmp" "XDG_RUNTIME_DIR=$run/m$1/rt" \
        "PATH=$run/bin:/usr/local/bin:/usr/bin:/bin" "OPS_HOME_1=$run/m1/home" "OPS_HOME_2=$run/m2/home"
}

# A word of a fence, with an optional value: expect="some text", timeout=60, optional=tool.
word_pattern='^[[:space:]]*([a-z]+)(=("([^"]*)"|([^[:space:]"]*)))?(.*)$'
ran=0
skipped=0
for number in $(seq -f '%03g' 1 "$count"); do
    script=$run/blocks/$number.sh
    IFS=$'\t' read -r line words <"$run/blocks/$number.meta"
    machine=1
    follows=0
    fails=0
    limit=240
    optional=""
    expects=()
    rest=$words
    while [[ $rest =~ $word_pattern ]]; do
        word=${BASH_REMATCH[1]}
        value=${BASH_REMATCH[4]:-${BASH_REMATCH[5]:-}}
        rest=${BASH_REMATCH[6]}
        case "$word" in
            second) machine=2 ;;
            follows) follows=1 ;;
            fails) fails=1 ;;
            timeout) limit=$value ;;
            optional) optional=$value ;;
            expect) expects+=("$value") ;;
            *) fail "line $line: the fence word \"$word\" is not one this check knows" ;;
        esac
    done
    [[ $rest =~ ^[[:space:]]*$ ]] || fail "line $line: cannot read the fence words: $words"
    [[ $limit =~ ^[0-9]+$ ]] || fail "line $line: timeout=$limit is not a number of seconds"
    title=$(grep -m1 -v -E '^[[:space:]]*(#|$)' "$script" | cut -c1-70)
    if [ -n "$optional" ] && ! command -v "$optional" >/dev/null; then
        echo "skip  block $number (line $line): $title ($optional is not installed here)"
        skipped=$((skipped + 1))
        continue
    fi
    # The guide's devnet addresses are written with 127.49.0; run them on the check's own block.
    { echo "set -euo pipefail"; sed "s/127\\.49\\.0/$prefix/g" "$script"; } >"$run/blocks/$number.run"
    log=$run/logs/$number.log
    holder=$holder_a
    [ "$machine" = 1 ] || holder=$holder_b
    cmd=(bash "$run/blocks/$number.run")
    if [ "$follows" = 1 ]; then
        limit=8
    fi
    # shellcheck disable=SC2046
    timeout --signal=TERM --kill-after=15 "$limit" nsenter -t "$holder" -U -n --preserve-credentials -- \
        env -i $(block_env "$machine") "${cmd[@]}" >"$log" 2>&1 </dev/null
    status=$?
    ok=1
    if [ "$follows" = 1 ]; then
        [ "$status" = 124 ] || ok=0
    elif [ "$fails" = 1 ]; then
        { [ "$status" != 0 ] && [ "$status" != 124 ]; } || ok=0
    else
        [ "$status" = 0 ] || ok=0
    fi
    reason=""
    if [ "$ok" = 0 ]; then
        reason="exit status $status where $(if [ "$follows" = 1 ]; then echo "a followed command stops at the timeout"; elif [ "$fails" = 1 ]; then echo "a refusal was expected"; else echo "0 was expected"; fi)"
    else
        for text in "${expects[@]+"${expects[@]}"}"; do
            if ! grep -F -q -- "$text" "$log"; then
                ok=0
                reason="the output does not contain \"$text\""
                break
            fi
        done
    fi
    if [ "$ok" = 0 ]; then
        echo "FAIL  block $number (line $line, machine $machine): $reason" >&2
        echo "---- the block ----" >&2
        cat "$script" >&2
        echo "---- its output (the last 60 lines of $log) ----" >&2
        tail -n 60 "$log" >&2
        exit 1
    fi
    echo "ok    block $number (line $line, machine $machine): $title"
    ran=$((ran + 1))
done

echo "ops-doc-check: $ran blocks of $guide ran as written, $skipped skipped"
exit 0
