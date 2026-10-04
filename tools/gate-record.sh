#!/usr/bin/env bash
# gate-record.sh: run a gate command, then write its gate record when GATE_RECORDS is set.
#
#   GATE_RECORDS=DIR GATE_TARGET=NAME ... tools/gate-record.sh COMMAND [ARG...]
#
# The Makefile's `$(gate)` sets the variables: GATE_RECORDS (the records' directory; nothing is
# written when it is empty), GATE_TARGET (the make target), GATE_VARIABLES (the variables of the
# make command line), GATE_IMAGE (the oracle image the command used, if any), GATE_TESTED (the
# paths the command tests, which the criteria file GATE_CRITERIA may narrow), GATE_LEDGER and
# LEDGER_DIR (a long run's ledger entry: `kind=property id=P cases=N seed=S` or `kind=target id=T
# seconds=N forks=N`), GATE_TOOLING (the run directories whose `tooling.sha256` names the devnet
# tooling the judged runs were made with), GATE_DIFFTEST (the harness binary). The command runs as
# it is; its exit status is this script's, or 1 when it passed and its record could not be
# written. When a record is written, what the command printed to its standard output is also kept
# in a temporary file and given to `gate record`, which reads the record's counts, hits and
# scenario verdicts from it. The head and whether the tree had changes are read before the command
# runs, so that a record names the tree the command ran on (a tree that changed meanwhile makes
# the record dirty). An interrupt, a terminate or a hang-up sent to this script is passed on to
# the command, and the record says the command was interrupted: a run stopped so counts for
# nothing, while a run killed by anything else (the kernel's out-of-memory killer, a crash) is a
# failed run.
set -uo pipefail

if [ -z "${GATE_RECORDS:-}" ]; then
    exec "$@"
fi
printed=$(mktemp "${TMPDIR:-/tmp}/gate-record.XXXXXX") || exit 1
pipe="$printed.pipe"
trap 'rm -f "$printed" "$pipe"' EXIT
mkfifo "$pipe" || exit 1
head_before=$(git rev-parse --verify -q HEAD) || head_before=
dirty_before=0
[ -z "$(git status --porcelain 2>/dev/null)" ] || dirty_before=1
interrupted=0
command_pid=
pass_on() {
    interrupted=1
    [ -z "$command_pid" ] || kill -TERM "$command_pid" 2>/dev/null
}
trap pass_on INT TERM HUP
started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
tee "$printed" <"$pipe" &
reader=$!
# The command keeps this script's standard input (a job in the background would read nothing) and
# the default handling of an interrupt and a quit (which a job in the background would ignore).
exec 3<&0
env --default-signal=INT,QUIT "$@" <&3 3<&- >"$pipe" &
command_pid=$!
exec 3<&-
# A signal this script handles ends a wait early: wait again until the command has ended.
while :; do
    wait "$command_pid"
    status=$?
    kill -0 "$command_pid" 2>/dev/null || break
done
wait "$reader"
trap - INT TERM HUP
finished=$(date -u +%Y-%m-%dT%H:%M:%SZ)

args=(gate record --target "${GATE_TARGET:-}" --command "$*" --exit "$status"
      --started "$started" --finished "$finished" --out "$GATE_RECORDS" --output "$printed")
[ -n "${GATE_VARIABLES:-}" ] && args+=(--variables "$GATE_VARIABLES")
[ -n "${GATE_IMAGE:-}" ] && args+=(--image "$GATE_IMAGE")
[ -n "$head_before" ] && args+=(--head "$head_before" --dirty-before "$dirty_before")
[ "$interrupted" = 1 ] && args+=(--interrupted)
for dir in ${GATE_TOOLING:-}; do [ -d "$dir" ] && args+=(--tooling "$dir"); done
[ -n "${GATE_CRITERIA:-}" ] && args+=(--criteria "$GATE_CRITERIA")
for path in ${GATE_TESTED:-}; do args+=(--tested "$path"); done
if [ -n "${GATE_LEDGER:-}" ]; then
    args+=(--ledger "$GATE_LEDGER" --ledger-dir "${LEDGER_DIR:-}")
fi
if ! "${GATE_DIFFTEST:-target/debug/difftest}" "${args[@]}"; then
    echo "gate-record: the record of ${GATE_TARGET:-the command} was not written" >&2
    [ "$status" = 0 ] && status=1
fi
exit "$status"
