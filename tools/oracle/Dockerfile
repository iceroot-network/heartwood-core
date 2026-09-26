# Heartwood oracle image: the Solar Core TypeScript reference built from a pinned commit, plus the
# oracle scripts. DEVNET AND TEST USE ONLY. The scripts serve the first two levels of Heartwood's
# differential tests: gen-vectors.js writes the golden vectors of level L0 (fixed cases, compared
# byte for byte), and oracle-server.js answers the requests of level L1 (random and mutated cases,
# one oracle call each).
#
# Built from scratch: nothing is taken from another image or from a host build. The inputs are
# pinned: the base image by digest, the Node tarball by SHA-256, pnpm by version, the Solar sources
# by commit and tree hash, the npm packages by Solar's lockfile. The recipe is the one Solar Core
# 4.3.1 builds with: Node 18.20.8, Python 3.10, pnpm 6.32.11, install --frozen-lockfile, build,
# check-native.js, and a clean working tree afterwards.
#
# Context: tools/oracle/make-context.sh. Build: tools/oracle/build-image.sh, which runs
#   docker build --no-cache --pull --build-arg SOLAR_COMMIT=<commit> --build-arg SOLAR_TREE=<tree> <context>
# The build needs registry access (Ubuntu packages, nodejs.org, the npm registry), as every Solar
# install does. The running image needs none and is always started with --network none: Node 18 is
# past its end of life, and the reference's ws 7.5.5 can be crashed remotely.

# Multi-arch index digest of ubuntu:22.04 (amd64 and arm64). Its system Python is 3.10, which
# node-gyp 7.1.2 needs.
ARG BASE_IMAGE=ubuntu:22.04@sha256:b8b6ee6aa931ecd9d0d952abc34dc0e5f7c6a30c6bb71b079fe399fde0329c02

# --------------------------------------------------------------------------------------------------
FROM ${BASE_IMAGE} AS build

ARG SOLAR_COMMIT
ARG SOLAR_TREE
ARG NODE_VERSION=18.20.8
ARG NODE_SHA256_X64=5467ee62d6af1411d46b6a10e3fb5cacc92734dbcef465fea14e7b90993001c9
ARG NODE_SHA256_ARM64=224e569dbe7b0ea4628ce383d9d482494b57ee040566583f1c54072c86d1116b
ARG PNPM_VERSION=6.32.11
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ENV DEBIAN_FRONTEND=noninteractive

RUN set -eux; \
    [[ "${SOLAR_COMMIT}" =~ ^[0-9a-f]{40}$ ]] || { echo "SOLAR_COMMIT must be a full commit id" >&2; exit 1; }; \
    [[ "${SOLAR_TREE}" =~ ^[0-9a-f]{40}$ ]] || { echo "SOLAR_TREE must be a full tree id" >&2; exit 1; }

# The build packages from Solar's install.sh list that the build needs; git for the tree check.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl xz-utils build-essential python3 git \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) node_arch=x64;   sha="$NODE_SHA256_X64" ;; \
      arm64) node_arch=arm64; sha="$NODE_SHA256_ARM64" ;; \
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz"; \
    echo "${sha}  /tmp/node.tar.xz" | sha256sum -c -; \
    mkdir -p /opt/node; \
    tar -xJf /tmp/node.tar.xz -C /opt/node --strip-components=1; \
    rm /tmp/node.tar.xz
ENV PATH=/opt/node/bin:$PATH
RUN set -eux; \
    test "$(node --version)" = "v${NODE_VERSION}"; \
    python3 -c 'import sys; assert sys.version_info[:2] == (3, 10), sys.version'

# Solar sources: the tarball's checksum, its embedded commit id (git archive's pax header) and the
# git tree hash of the extracted files must all match. The temporary repository (an index equal to
# the commit's tree, no commit) is kept until after the build for the recipe's last check, a clean
# working tree: no change against the index and no untracked file that Solar's .gitignore files do
# not cover. Then it is removed.
RUN useradd -l -m -u 1000 build && mkdir -p /opt/solar
COPY solar-src.tar solar-src.tar.sha256 /tmp/solar-src/
RUN set -eux; \
    cd /tmp/solar-src; \
    sha256sum -c solar-src.tar.sha256; \
    test "$(git get-tar-commit-id < solar-src.tar)" = "${SOLAR_COMMIT}"; \
    tar -xf solar-src.tar -C /opt/solar; \
    rm -rf /tmp/solar-src; \
    cd /opt/solar; \
    git -c init.defaultBranch=oracle init -q; \
    git add -A -f; \
    test "$(git write-tree)" = "${SOLAR_TREE}"; \
    chown -R build:build /opt/solar

USER build
WORKDIR /opt/solar
# Native addons build against the pinned Node's own headers with Python 3.10 (here the system
# python3 is 3.10). pnpm keeps its store in the build user's home directory.
ENV npm_config_nodedir=/opt/node \
    npm_config_python=/usr/bin/python3
RUN set -eux; \
    test "$(npx -y "pnpm@${PNPM_VERSION}" --version)" = "${PNPM_VERSION}"; \
    npx -y "pnpm@${PNPM_VERSION}" install --frozen-lockfile --prefer-offline --store-dir "$HOME/.pnpm-store"
RUN npx -y "pnpm@${PNPM_VERSION}" build
COPY --chown=build:build check-native.js /tmp/check-native.js
RUN set -eux; \
    node /tmp/check-native.js /opt/solar; \
    test "$(node -p 'require("/opt/solar/packages/crypto/package.json").version')" = "4.3.1"; \
    status="$(git diff --name-status; git ls-files --others --exclude-standard)"; \
    if [ -n "$status" ]; then echo "working tree not clean after the build:" >&2; echo "$status" >&2; exit 1; fi; \
    rm -rf .git; \
    printf '%s\n' "${SOLAR_COMMIT}" > .solar-commit; \
    printf '%s\n' "${SOLAR_TREE}" > .solar-tree

# --------------------------------------------------------------------------------------------------
FROM ${BASE_IMAGE} AS runtime

ARG SOLAR_COMMIT
ARG SOLAR_TREE

RUN useradd -l -m -u 1000 oracle
COPY --from=build --chown=0:0 /opt/node /opt/node
COPY --from=build --chown=0:0 /opt/solar /opt/solar
COPY --chown=0:0 oracle/ /opt/oracle/

ENV PATH=/opt/node/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    SOLAR_DIR=/opt/solar \
    LANG=C.UTF-8 \
    TZ=UTC
USER oracle
WORKDIR /opt/oracle

# Self-tests in the final image: the native modules load without the build packages, the L1
# server answers a ping, and the L0 generator runs, including its native-versus-js backend check.
# The genesis class needs the reference's devnet generator, which is not in the image (a vector run
# mounts it), so the self-test leaves that class out.
RUN set -eux; \
    node /opt/oracle/check-native.js /opt/solar; \
    echo '{"id":1,"op":"ping"}' | node /opt/oracle/oracle-server.js | grep -q '"ok":true'; \
    node /opt/oracle/gen-vectors.js --out /tmp/self-test; \
    rm -rf /tmp/self-test

LABEL org.opencontainers.image.title="heartwood-oracle" \
      org.opencontainers.image.description="Solar TypeScript reference and the Heartwood differential-test oracle scripts. Test use only." \
      org.opencontainers.image.revision="${SOLAR_COMMIT}" \
      io.heartwood.solar.tree="${SOLAR_TREE}"

CMD ["node", "/opt/oracle/gen-vectors.js", "--help"]
