#!/usr/bin/env bash
# fuzz-run.sh: run one fuzz target's libFuzzer command with its output in LOG, and judge the run by
# its exit status and its last status line together, so that a gate record of the run carries the
# whole verdict.
#
#   tools/fuzz-run.sh LOG COMMAND [ARG...]
#
# The command (`cargo +nightly fuzz run TARGET -- -fork=N ...`) writes everything into LOG. In
# libFuzzer's fork mode every merged job prints a status line (`#N: cov: ... oom/timeout/crash:
# a/b/c ...`); the run passes when the command exits 0 and its last status line reports no crash,
# timeout or out-of-memory stop. A run that exits 0 without a status line fails: it fuzzed nothing
# a status line counted. The last status line is printed (or "no status line"). An interrupt, a
# terminate or a hang-up is passed on to the command.
set -uo pipefail

if [ $# -lt 2 ]; then
    echo "usage: fuzz-run.sh LOG COMMAND [ARG...]" >&2
    exit 2
fi
log=$1
shift
# The fuzzer keeps the default handling of an interrupt and a quit, which a job in the background
# would ignore.
env --default-signal=INT,QUIT "$@" >"$log" 2>&1 &
child=$!
trap 'kill -TERM "$child" 2>/dev/null' INT TERM HUP
while :; do
    wait "$child"
    status=$?
    kill -0 "$child" 2>/dev/null || break
done
trap - INT TERM HUP
last=$(grep -E '^#[0-9]+: cov: ' "$log" | tail -n 1)
echo "fuzz-run: ${last:-no status line}"
case "$last" in
    *'oom/timeout/crash: 0/0/0 '*) ;;
    *) [ "$status" -ne 0 ] || status=1 ;;
esac
exit "$status"
