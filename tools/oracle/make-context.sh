#!/usr/bin/env bash
# make-context.sh: assemble the oracle image's docker build context.
#
#   tools/oracle/make-context.sh [--repo DIR] [--commit REV] [--out DIR]
#
# The context holds:
#   solar-src.tar         the full tree of REV from the reference repository, as `git archive`
#                         writes it (the repository is only read, never changed)
#   solar-src.tar.sha256  its checksum, checked again inside the build
#   solar-src.env         SOLAR_COMMIT and SOLAR_TREE (the full commit id and its tree hash), which
#                         build-image.sh passes as build arguments and the Dockerfile verifies
#   Dockerfile, check-native.js, oracle/  the image recipe and the oracle scripts
#   oracle/external       the published vector files that class V11 checks the reference against
#                         (packages/crypto/tests/vectors/external in this repository)
#   oracle/corpus         the committed corpora (tools/difftest/corpus), whose chain class V33
#                         rolls back
#
# --repo   a clone of the reference implementation, iceroot-network/solar-core-ref, with its tags
#          [$SOLAR_REPO; one of the two is required]
# --commit the reference commit or tag [s1-ref-v2]
# --out    the context directory [target/oracle-context in this repository, git-ignored]
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
REPO=${SOLAR_REPO:-}
REV=s1-ref-v2
OUT=$ROOT/target/oracle-context

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO=$2; shift 2 ;;
        --commit) REV=$2; shift 2 ;;
        --out) OUT=$2; shift 2 ;;
        -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
        *) echo "make-context: unknown argument $1" >&2; exit 2 ;;
    esac
done

if [ -z "$REPO" ]; then
    echo "make-context: no reference repository: pass --repo DIR or set SOLAR_REPO to a clone of iceroot-network/solar-core-ref" >&2
    exit 2
fi
if ! git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1; then
    echo "make-context: $REPO is not a git repository: pass --repo DIR or set SOLAR_REPO to a clone of iceroot-network/solar-core-ref" >&2
    exit 2
fi
if ! COMMIT=$(git -C "$REPO" rev-parse --verify -q "$REV^{commit}"); then
    echo "make-context: $REV is not a commit in $REPO (are its tags fetched?)" >&2
    exit 1
fi
TREE=$(git -C "$REPO" rev-parse --verify "$REV^{tree}")

rm -rf "$OUT"
mkdir -p "$OUT/oracle/lib"
# Solar's packages mark README.md, .gitignore and .gitattributes as export-ignore, so a plain
# `git archive` is not the full checkout (and without the package .gitignore files the built tree
# is not clean). The archive is therefore made from a temporary shared clone whose info/attributes
# switches export-ignore and export-subst off. The clone only reads the repository's objects; the
# repository itself is not changed. The Dockerfile checks that the tree hash equals the commit's.
mkdir -p "$ROOT/target"
SRC_GIT=$(mktemp -d "$ROOT/target/oracle-src.XXXXXX")
trap 'rm -rf "$SRC_GIT"' EXIT
git clone -q --bare --shared "$REPO" "$SRC_GIT/src.git"
printf '* -export-ignore -export-subst\n' > "$SRC_GIT/src.git/info/attributes"
git -C "$SRC_GIT/src.git" archive --format=tar -o "$OUT/solar-src.tar" "$COMMIT"
test "$(git get-tar-commit-id < "$OUT/solar-src.tar")" = "$COMMIT"
(cd "$OUT" && sha256sum solar-src.tar > solar-src.tar.sha256)
printf 'SOLAR_COMMIT=%s\nSOLAR_TREE=%s\n' "$COMMIT" "$TREE" > "$OUT/solar-src.env"

cp "$HERE/Dockerfile" "$HERE/check-native.js" "$OUT/"
cp "$HERE/gen-vectors.js" "$HERE/oracle-server.js" "$HERE/replay-chain.js" "$HERE/check-native.js" "$OUT/oracle/"
cp -R "$HERE/lib/." "$OUT/oracle/lib/"
cp -R "$ROOT/packages/crypto/tests/vectors/external" "$OUT/oracle/external"
cp -R "$ROOT/tools/difftest/corpus" "$OUT/oracle/corpus"

echo "make-context: $OUT"
echo "make-context: Solar $REV = $COMMIT (tree $TREE), $(du -h "$OUT/solar-src.tar" | cut -f1) tarball"
