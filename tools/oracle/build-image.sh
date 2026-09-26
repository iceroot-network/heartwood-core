#!/usr/bin/env bash
# build-image.sh: build the oracle image from scratch.
#
#   tools/oracle/build-image.sh [--commit REV] [--tag TAG] [--repo DIR] [-- extra docker build args]
#
# Assembles the context (make-context.sh), then runs `docker build --no-cache --pull` so no earlier
# layer is reused. The log goes to target/oracle-build-<short commit>.log and the image id to
# target/oracle-image-<short commit>.id.
#
# --repo   a clone of iceroot-network/solar-core-ref with its tags [$SOLAR_REPO; one is required]
# --commit the reference commit or tag [s1-ref-v2]
# --tag    the image tag [heartwood-oracle:<first 9 characters of the commit>]
#
# Environment: DIFFTEST_DOCKER, the docker command [tools/oracle/docker.sh].
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
DOCKER=${DIFFTEST_DOCKER:-$HERE/docker.sh}
REV=s1-ref-v2
REPO=${SOLAR_REPO:-}
TAG=
CONTEXT=$ROOT/target/oracle-context

while [ $# -gt 0 ]; do
    case "$1" in
        --commit) REV=$2; shift 2 ;;
        --tag) TAG=$2; shift 2 ;;
        --repo) REPO=$2; shift 2 ;;
        --) shift; break ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "build-image: unknown argument $1" >&2; exit 2 ;;
    esac
done

if [ -z "$REPO" ]; then
    echo "build-image: no reference repository: pass --repo DIR or set SOLAR_REPO to a clone of iceroot-network/solar-core-ref" >&2
    exit 2
fi

"$HERE/make-context.sh" --repo "$REPO" --commit "$REV" --out "$CONTEXT"
# shellcheck disable=SC1091
. "$CONTEXT/solar-src.env"
SHORT=${SOLAR_COMMIT:0:9}
TAG=${TAG:-heartwood-oracle:$SHORT}
LOG=$ROOT/target/oracle-build-$SHORT.log

echo "build-image: $TAG from Solar $SOLAR_COMMIT; log in $LOG" >&2
start=$(date +%s)
"$DOCKER" build --no-cache --pull \
    --build-arg SOLAR_COMMIT="$SOLAR_COMMIT" --build-arg SOLAR_TREE="$SOLAR_TREE" \
    -t "$TAG" "$@" "$CONTEXT" 2>&1 | tee "$LOG"
id=$("$DOCKER" image inspect --format '{{.Id}}' "$TAG")
printf '%s\n' "$id" > "$ROOT/target/oracle-image-$SHORT.id"
echo "build-image: built $TAG = $id in $(( $(date +%s) - start )) s" >&2
