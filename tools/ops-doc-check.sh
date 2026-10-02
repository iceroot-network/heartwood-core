#!/usr/bin/env bash
# ops-doc-check.sh: run the operator guide's Linux command blocks as written, in order, and fail on
# the first block that does not do what the guide says.
#
#   tools/ops-doc-check.sh [GUIDE]
#
# GUIDE is the guide's Markdown file (docs/operating.md by default). Each fenced block of the
# guide marked as a Linux command block runs in a scratch data root made for this run under
# OPS_DOC_ROOT, so that it reads and writes nothing of the machine's own instances; the devnets it
# starts take the loopback block HEARTWOOD_DEVNET_PREFIX names (the first three octets, such as
# 127.49.0), and the commands run the heartwood binary OPS_DOC_BINARY. Every process a block starts
# is stopped by this script before it ends, by its own pid.
#
#   HEARTWOOD_DEVNET_PREFIX   the loopback block of the guide's devnets (required)
#   OPS_DOC_BINARY            the heartwood binary [target/release/heartwood]
#   OPS_DOC_ROOT              where the scratch data roots are made [target/ops-doc-check]
#
# Exit status: 0 every block ran as the guide says, 1 a block failed, 2 a usage error or the
# check is not built yet.
set -euo pipefail

guide=${1:-docs/operating.md}
if [ $# -gt 1 ]; then
    echo "usage: ops-doc-check.sh [GUIDE]" >&2
    exit 2
fi
if [ -z "${HEARTWOOD_DEVNET_PREFIX:-}" ]; then
    echo "ops-doc-check: set HEARTWOOD_DEVNET_PREFIX, the loopback block of the guide's devnets" >&2
    exit 2
fi
if [ ! -f "$guide" ]; then
    echo "ops-doc-check: $guide does not exist" >&2
    exit 2
fi
echo "ops-doc-check: not built yet" >&2
exit 2
