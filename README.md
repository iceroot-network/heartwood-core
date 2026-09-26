# Heartwood Core

Heartwood Core is the Rust node for the IceRoot network. It is at an early stage: this repository
currently holds the differential-test harness and the reference oracle that the node's crates will
be tested against. The node's own crates are not here yet.

Heartwood Core is a new implementation in Rust. Its protocol design inherits from ARK Core and
Solar Core; Solar Core serves as the reference implementation for compatibility tests.

## How Heartwood is tested

The reference implementation is Solar Core 4.3.1 with a patch series, kept in
[iceroot-network/solar-core-ref](https://github.com/iceroot-network/solar-core-ref). The tests use
its tag `s1-ref-v2` (29 patches; `s1-ref-v1` holds the first 18). Heartwood must behave exactly like it, and differential testing shows that it
does: the same inputs go to the reference (the oracle) and to Heartwood, and the results must be
equal byte for byte, including which inputs each side rejects.

| Level | What is compared | Status |
|---|---|---|
| L0 | golden vectors written by the reference, against Heartwood | runs; nothing to compare until the first crates |
| L1 | random and mutated cases, one oracle call each | planned |
| L2 | block sequences replayed on both sides, a state digest per block | planned |
| L3 | the same scenario on a reference devnet and on a Heartwood devnet | planned |
| L4 | a mixed network of reference and Heartwood nodes | planned |

## Layout

```text
Cargo.toml            workspace: edition 2024, resolver 3, rust-version 1.98, workspace lints
Makefile              `make help` lists the targets
NOTICE                credits for ARK Core and Solar Core
tools/difftest/       heartwood-difftest: the harness library and the `difftest` binary
tools/oracle/         the oracle: Node scripts and the image recipe
  Dockerfile            the oracle image, built from scratch with pinned inputs
  make-context.sh       the build context: the reference at a commit, as a git archive
  build-image.sh        docker build --no-cache --pull of that context
  docker.sh             the docker command: docker, or sudo -n docker when the daemon needs it
  gen-vectors.js        L0: writes the golden vectors
  oracle-server.js      L1: JSON lines over stdio, one request per line
  lib/                  solar.js (load a built reference), network.js (the devnet), ops.js,
                        craft.js (inputs the builders do not make), vectors/ (one file per class)
  check-native.js       checks that the reference's native modules build and load
packages/crypto/tests/vectors/  the golden vectors of heartwood-crypto, with MANIFEST.sha256
  external/             published vector files, with their source URLs and SHA-256
```

## Requirements

- Rust 1.98 or later, with clippy and rustfmt.
- Docker, for the oracle image. Building the image needs network access (Ubuntu packages,
  nodejs.org and the npm registry); running it does not.
- A clone of [iceroot-network/solar-core-ref](https://github.com/iceroot-network/solar-core-ref)
  with its tags.
- For local oracle runs (`make e2e` runs the oracle in the image and locally): Node 18.20.8 and a
  checkout of the reference at the same tag, installed and built with pnpm 6.32.11
  (`npx pnpm@6.32.11 install --frozen-lockfile`, then `npx pnpm@6.32.11 build`; node-gyp needs
  Python 3.10). `tools/oracle/Dockerfile` shows the full recipe.

## Build and test

```sh
make check          # cargo build, clippy (warnings are errors), fmt --check, test
make oracle-image   # build heartwood-oracle:<commit> from scratch (a few minutes)
make e2e            # vectors in the image and locally, diff the two runs, run L0
make vectors-install  # copy the image's vectors into packages/crypto/tests/vectors
```

`make check` needs only Rust. `make e2e` passes when the image run and the local run agree on
every record; its L0 step then reports "0 compared" and lists, per operation, the records that have
no Rust implementation yet. `make help` lists the other targets: `make ping` and
`make test-oracle` (the oracle answers, in the image and locally) and `make oracle-save`
(`docker save` of the image into `IMAGE_DIR`).

### Settings

| Variable | Default | Meaning |
|---|---|---|
| `SOLAR_REPO` | `../solar-core-ref` | a clone of the reference repository, with its tags |
| `COMMIT` | `s1-ref-v2` | the reference commit or tag the image is built from |
| `LOCAL_SOLAR` | `$(SOLAR_REPO)` | a built checkout of the reference at `COMMIT`, for local runs |
| `TOOLCHAIN_ENV` | empty | a shell file that every cargo and node recipe sources first; when it is empty, the tools come from `PATH` |
| `IMAGE` | `heartwood-oracle:<first 9 characters of the commit id COMMIT resolves to>` | the oracle image |
| `VECTORS` | `target/vectors` | where the vector runs are written |
| `IMAGE_DIR` | `target/oracle-images` | where `make oracle-save` writes |
| `DIFFTEST_DOCKER` | `tools/oracle/docker.sh` | the docker command |
| `DEVNET_GENERATOR` | empty | the reference's devnet generator, `gen-network.js`, for the genesis class |
| `CRYPTO_VECTORS` | `packages/crypto/tests/vectors` | where `make vectors-install` puts the container run |

Settings for one machine go in `local.mk` beside the Makefile. It is git-ignored and read before
the defaults, for example:

```make
SOLAR_REPO    = $(HOME)/src/solar-core-ref
LOCAL_SOLAR   = $(HOME)/src/solar-core-ref
TOOLCHAIN_ENV = $(HOME)/toolchains/node18-rust.sh
```

The same variables work on the command line: `make e2e SOLAR_REPO=../solar-core-ref`.

## The oracle image

`make oracle-image`, or `tools/oracle/build-image.sh --repo DIR --commit REV`:

1. `make-context.sh` takes the full tree of `COMMIT` from `SOLAR_REPO` as a `git archive`, records
   the commit and tree hashes, and copies the Dockerfile, the oracle scripts and the external
   vector files into `target/oracle-context`. Solar's packages mark `README.md`, `.gitignore` and
   `.gitattributes` as `export-ignore`, so the archive is made from a temporary shared clone whose
   `info/attributes` turns that off. The repository itself is only read.
2. `docker build --no-cache --pull` with `SOLAR_COMMIT` and `SOLAR_TREE` as build arguments. The
   build stage uses Ubuntu 22.04 pinned by digest (its Python is 3.10), the Node 18.20.8 tarball
   checked against its SHA-256, `npx -y pnpm@6.32.11 install --frozen-lockfile`, `pnpm build` and
   `check-native.js`. It checks the tarball checksum, the commit id embedded in the tarball, the
   git tree hash of the extracted files, and that the working tree is clean after the build. The
   runtime stage holds only Node, the built reference and the oracle scripts, runs
   `check-native.js` again, and runs both scripts once as a self-test.
3. The image is tagged `heartwood-oracle:<first 9 characters of the commit>`; its id goes to
   `target/oracle-image-<short>.id` and the build log to `target/oracle-build-<short>.log`.

The oracle always runs with `--network none`: Node 18 is past its end of life, and the reference's
`ws` 7.5.5 can be crashed remotely. Only the build needs the network.

## Vector files

`gen-vectors.js --out DIR` writes one JSONL file per vector class and `MANIFEST.sha256`
(`sha256sum` format; `difftest vectors verify` checks it). The committed set of `heartwood-crypto`
is in [packages/crypto/tests/vectors](packages/crypto/tests/vectors); `make e2e` followed by
`make vectors-install` regenerates it, and a regeneration must be bit-identical unless the oracle
changed.

| File | Content |
|---|---|
| `V01-keys-addresses.jsonl` | passphrase keys; addresses for network bytes 90, 63 and 30; `Address.validate` and `Address.toBuffer` (the network-byte complaint, a bad checksum, 33 and 35 characters, a valid checksum over a 20- or 22-byte payload); the public-key encodings the reference accepts (`02`/`03`, `04`, `06`/`07` with matching parity) and refuses |
| `V02-transactions.jsonl` | the six kept types (1/1, 1/2, 1/6, 1/7, 2/0, 2/2), v3, BIP340 with fixed aux: memos of 0, 1 and 255 bytes (2-, 3- and 4-byte characters at the end), 256 bytes refused; 1, 2, 255 and 256 recipients, 0 and 257 refused; resignation types 0 to 2; votes of 1 to 53 entries; doubly signed variants. Each built case is recorded as `tx.build`, `tx.fromBytes` and `tx.fromJson` |
| `V03-noncanonical-removed.jsonl` | wire forms the reference decodes to other bytes or refuses, with the strict verdict and the id it computes: version bytes, duplicate, `__proto__` and pattern-failing vote names, a stray resignation byte, memos that are not UTF-8, trailing bytes, signatures of unusual lengths, extended headers; the removed types 1/0, 1/3, 1/4, 1/5, 1/8, 1/9 and 1/10 and their inactive handlers; unknown types |
| `V04-signatures.jsonl` | BIP340 with fixed aux over messages and over the preimages of a transaction's first and second signature and of a block header, with a verify matrix (mutated signature, message and key, the prefix-agnostic key slice, out-of-range values); legacy Schnorr over 0 bytes to 4 KB and over real P2P status preimages, with R at infinity (at least 235 records) |
| `V05-blocks.jsonl` | `BlockFactory.make` with fixed aux for 0, 1, 9 (the kept types) and 150 transactions and for the largest block the devnet rules allow (its bytes and JSON as SHA-256), each decoded and re-encoded; 151 transactions; `maxPayload` at its boundary; `payloadLength` 32n - 1, 32n + 1 and 0; the block version; BIP340 and legacy block signatures under `bip340: false` |
| `V06-arithmetic.jsonl` | the burned fee at 9,000 basis points; the reward by rank and tier; donations at 500 basis points; the dynamic fee floor per type and size; `totalAmount`, which counts top-level amounts only |
| `V07-vote-rules.jsonl` | all 9,999 two-way cent splits (one compact record per name pair: the refused splits and SHA-256 over the verdicts and the vote assets); the vote schema on single cases; `sortVotes` orders; the vote asset at 1,024 and 1,025 bytes |
| `V10-genesis.jsonl` | the devnet chain definition that the reference's devnet generator (`gen-network.js`) writes for three seeds and a fixed epoch: the exact text of the four `crypto/*.json` files, the delegate and wallet keys, the nethash, the genesis id and the issued amount, and `BlockFactory.fromJson` on the result |
| `V11-external.jsonl` | the published vector files in `external/` (BIP340, the bip-schnorr draft at `d194620`, BIP39 English), each with its source URL and SHA-256 in `external/SOURCES.json`, and the reference's answers on their inputs |
| `V12-milestone-config.jsonl` | milestone files as JSON text, accepted (with the merged milestones, rewards and donations) or refused by the configuration manager's start-up checks; transaction rules at the block's height |

The first line of each file is a meta record: format `heartwood-vectors/1`; the Solar version,
commit and tree, and the SHA-256 of the reference's patch register (null when the commit has none);
the oracle mode and image id; the Node and ICU versions; the bcrypto backend; the network
description (with the milestones SHA-256 and the detected key format); the aux; and, where a class
has them, its other inputs (the SHA-256 of the devnet generator's files, of the external files).
Every other line is `{"op", "network", "height", "name"?, "input", "output"}` or the same with
`"error": {"class", "message"}`. Records hold only values the reference computed, so they are
identical across bcrypto backends, image builds and host builds; `difftest vectors diff` compares
two runs record by record.

Every record is produced under the devnet configuration of `tools/oracle/lib/network.js`: network
byte 90; Solar mainnet's merged rules at height 5,000,000, in force from height 1; five reward tiers
by delegate rank; a minimum fee of 6,173; donations of 5 % each to two addresses from height 2,
derived from the devnet key-derivation seed; and a fixed epoch, `2026-01-01T00:00:00.000Z`. The
burn and donation keys follow the reference's own mainnet preset, so the patched format
(`feeBasisPoints`, `basisPoints`) is used automatically when the reference has it. A record whose
input has `milestones` (JSON text, read with `JSON.parse` as the reference reads a file) runs under
those milestones on the devnet network instead.

The genesis class runs the reference's devnet generator, which is not part of the oracle image:
set `DEVNET_GENERATOR` to its `gen-network.js` (`difftest oracle gen --devnet-generator FILE`). A
container run mounts the generator's directory read-only; without it the class is not written.

## difftest

```text
difftest oracle gen --out DIR [--no-backend-check] [--devnet-generator FILE] [--image TAG | --solar-dir DIR]
difftest oracle ping [--image TAG | --solar-dir DIR]
difftest vectors verify DIR
difftest vectors diff DIR DIR
difftest run l0 DIR
difftest run l1|l2|l3|l4        says that the level is not built yet
difftest levels
```

Library modules: `oracle` (run the scripts in the image or locally; `OracleServer` for L1),
`vectors` (files, meta records, manifest), `subject` (a Rust implementation of one oracle
operation, shared by L0 and L1, and the registry), `compare` (a JSON diff that names the path and,
for hex values, the first differing byte with the bytes around it), `report`, and `levels` (L0
runs; L1 to L4 are described with what each one arrives with).

A difference names the JSON path and, for bytes, the first differing offset. This is
`difftest vectors diff` against a copy of the vectors with one byte and one fee changed; an L0
failure (`FAIL <file>:<line> <op> "<name>" (height H)`) prints the same lines:

```text
  DIFF V02-transactions.jsonl:2 tx.build "transfer-1"
      $.output.hex: bytes differ at offset 24 (0x18), both 154 bytes
          expected ...00032b7179bebd24[9c]e23311e958fa95e7...
          actual   ...00032b7179bebd24[0c]e23311e958fa95e7...
      $.output.json.fee: expected "50000000", actual "10000000"
  1 of 1445 records differ, 0 files missing
```

The node's crates plug in by adding subjects (for example `keys.fromPassphrase` backed by
`heartwood-crypto`) to `subject::heartwood()`; `difftest run l0` then compares them, with no other
change.

`make test-oracle` runs the cargo test that pings `oracle-server.js` in the image and locally
(`DIFFTEST_TEST_IMAGE`, `DIFFTEST_TEST_SOLAR_DIR`); plain `cargo test` skips it.

## Licence

Not yet decided. [NOTICE](NOTICE) lists the projects Heartwood Core credits.
