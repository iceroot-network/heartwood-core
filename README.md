# Heartwood Core

Heartwood Core is the Rust node for the IceRoot network. It is at an early stage: this repository
holds the differential-test harness, the reference oracle that the node's crates are tested
against, and the node's first crate, `heartwood-crypto`.

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
| L0 | golden vectors written by the reference, against Heartwood | runs against `heartwood-crypto`: 1,468 of 1,518 records compared, all equal (see below) |
| L1 | random and mutated cases, one oracle call each | planned |
| L2 | block sequences replayed on both sides, a state digest per block | planned |
| L3 | the same scenario on a reference devnet and on a Heartwood devnet | planned |
| L4 | a mixed network of reference and Heartwood nodes | planned |

## Layout

```text
Cargo.toml            workspace: edition 2024, resolver 3, rust-version 1.98, workspace lints
Makefile              `make help` lists the targets
NOTICE                credits for ARK Core and Solar Core, and the licence notice of nes
packages/crypto/      heartwood-crypto: keys, signatures, the transaction and block codecs,
                      milestones and the devnet chain-definition builder (feature `genesis`)
packages/p2p-wire/    heartwood-p2p-wire: the peer-to-peer wire codec (nes frames, the protobuf
                      messages of the public routes, list framing, the signed peer status)
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
packages/p2p-wire/tests/vectors/  the golden vectors of heartwood-p2p-wire, with MANIFEST.sha256
tools/wire-fuzz-seeds.js  seeds the wire fuzz corpora from the wire vectors
fuzz/                 cargo-fuzz targets for the transaction and block decoders (own workspace)
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
make check          # cargo build, clippy (warnings are errors), fmt --check, test, release check
make oracle-image   # build heartwood-oracle:<commit> from scratch (a few minutes)
make e2e            # vectors in the image and locally, diff the two runs, run L0
make vectors-install  # copy the image's vectors into packages/crypto/tests/vectors
```

`make check` needs only Rust; it includes the L0 comparison of the committed vectors, and it
checks and formats the fuzz targets too (their own package, `fuzz/`). `make e2e`
passes when the image run and the local run agree on every record and L0 finds no difference in
the image run. `make help` lists the other targets: `make ping` and
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
| `DEVNET_GENERATOR` | empty | the devnet generator, `gen-network.js`, for the genesis class; a separate tool that is not published |
| `CRYPTO_VECTORS` | `packages/crypto/tests/vectors` | where `make vectors-install` puts the container run |
| `WIRE_VECTORS` | `packages/p2p-wire/tests/vectors` | where `make vectors-install-wire` puts the wire class of the container run |
| `FUZZ_SECONDS`, `FUZZ_FORK` | `600`, `8` | how long each wire fuzz target runs, and its libFuzzer jobs |
| `WIRE_SEEDS`, `WIRE_SEED_MAX` | the installed wire class, `65536` | the vector file that seeds the wire fuzz corpora, and the longest seed in bytes |
| `WIRE_LOGS` | `target/wire-fuzz` | the logs of `make fuzz-wire` and `make replay-wire` |
| `ORACLE_ARGS` | `--image $(IMAGE)` | where the oracle of `make replay-wire` runs; `--solar-dir DIR` for a local reference |

Settings for one machine go in `local.mk` beside the Makefile. It is git-ignored and read before
the defaults, for example:

```make
SOLAR_REPO    = $(HOME)/src/solar-core-ref
LOCAL_SOLAR   = $(HOME)/src/solar-core-ref
TOOLCHAIN_ENV = $(HOME)/toolchains/node18-rust.sh
```

The same variables work on the command line: `make e2e SOLAR_REPO=../solar-core-ref`.

## heartwood-crypto

`packages/crypto` is the byte-exact layer of the node: hashes, secret and public keys, addresses,
BIP340 signatures and the legacy Schnorr scheme, the version 3 transaction codec for the six kept
transaction types (1/1, 1/2, 1/6, 1/7, 2/0, 2/2) with the transaction rules and builders, the
block codec with its verification rules and factory, the network description and the milestones
(merged and checked at load, looked up by height), rewards, donations and the fee burn. The crate
forbids unsafe code, and its decoders return typed errors instead of panicking on untrusted input.

Features, all off by default:

| Feature | What it enables |
|---|---|
| `legacy-schnorr` | the pre-BIP340 Schnorr scheme, which only peer status messages use |
| `genesis` | the devnet chain-definition builder |
| `fixed-aux` | a test seam: `Aux::fixed`, reproducible BIP340 signatures for test vectors; only test code enables it |

```sh
cargo test -p heartwood-crypto --locked                  # unit tests and doctests of the default build
cargo test -p heartwood-crypto --locked --all-features   # and every vector class
```

The first command also checks, with a doctest that must fail to compile, that the default build has
no way to choose the signature aux. The integration tests need `fixed-aux` and run with
`--all-features`: they check every record of the vector classes (bytes, ids, JSON text with key
order, verdicts and error classes), the published BIP340, bip-schnorr and BIP39 vector files, and a
cross-check of both signature schemes with `k256` on every signature record and on random keys and
messages. `make test` runs both commands and the difftest tests.

A node accepts a block from another node with `Block::receive`, not with `Block::decode`. As the
reference does, it decodes the block and then builds it again from the decoded fields: every
transaction is serialised again after the transaction rules have changed it (a vote name that is
not a delegate name is removed, which changes the transaction's id), and the payload hash, the
transaction ids and the size that the block rules check are those of this canonical form.
`Block::decode` checks the bytes as they are; it is for the node's own genesis block and for
tests, and it applies the fee rules of genesis transactions to every transaction at height 1.

### Release builds

Release artefacts are built one package at a time (`cargo build --release -p PACKAGE`), never with
`--workspace`: Cargo turns on, for every package of one build, the features that any of them asks
for, and difftest asks for `fixed-aux`, `genesis` and `legacy-schnorr`. `make release-check`, part
of `make check`, fails when a package of `RELEASE_PACKAGES` builds `heartwood-crypto` with a test
seam.

### L0 results

`difftest run l0` compares the same records through the harness, with one subject per oracle
operation. Where both sides refuse, it also compares the error class wherever Heartwood models the
reference's class. On the committed vectors (reference tag `s1-ref-v2`) it reports:

| Outcome | Records |
|---|---|
| compared, equal to the reference | 1,468 |
| documented differences, each with its form checked on the input | 23 |
| skipped: the dynamic fee floor (transaction handlers) and the peer status message (P2P codec) | 27 |

The documented differences are inputs that Heartwood refuses and the reference accepts:

- three wire forms that the reference decodes and only refuses when the transaction is applied, so
  a block that holds one is refused either way: the removed transaction types, the extended header,
  and multi-signature entries after the signatures;
- a memo that is not valid UTF-8. The reference reads it as replacement characters (U+FFFD) and
  accepts it, with the id and the signature of the replaced text, in a block too. This is a known
  consensus difference: anyone can put a replacement character in a memo, and a relay can then
  swap its three bytes for an invalid byte without changing the transaction id or the block id.
  The reference accepts that copy, and Heartwood refuses it. The copy with the replacement
  characters is accepted by both, so a node must not mark a block id as invalid because one copy
  of the block has such a memo;
- in the JSON form only, `asset.votes` given as a non-empty array or string, whose indices the
  reference writes as vote names and removes after it has computed the id. A vote asset that is a
  truthy value without keys of its own (a number, `true`, an empty array) is no votes, as in the
  reference.

### Fuzz targets and the chain-definition driver

The fuzz targets run with cargo-fuzz on a nightly toolchain, and a corpus can then be replayed
against the oracle:

```sh
(cd fuzz && cargo +nightly fuzz run transaction_decode -- -max_total_time=600)
difftest oracle replay tx.fromBytes fuzz/corpus/transaction_decode \
    --vectors packages/crypto/tests/vectors --milestones fuzz/milestones/transaction_decode.json \
    --image heartwood-oracle:<commit>
```

With the `genesis` feature, `difftest genesis` writes a devnet chain definition (the four
`crypto/*.json` files, the delegate and wallet keys and a summary) from a genesis profile file, a
seed and an epoch. Every genesis vector record carries its profile as input, and the tests check
that the driver writes, byte for byte, the files those records hold. `--compare DIR` checks a
chain definition against a directory that the devnet generator wrote for the same inputs; the
generator is a separate tool of the maintainers and is not published (see the genesis class
below).

## heartwood-p2p-wire

`packages/p2p-wire` is the codec of the peer-to-peer port. It writes and reads the bytes that
nodes exchange exactly as the reference does, including how the reference reads malformed bytes,
so that both implementations accept the same payloads and decode them to the same values. It has
no I/O and no clock: the nes sessions (hello, pings, heartbeats, request ids) and the WebSocket
transport arrive with the crate that uses the codec.

- **nes frames**: the frames of the port (hello, ping, request, response and error), parsed and
  written with the reference's defaults and limits; the other nes types are refused, as the
  reference's parser refuses them on this port. A frame's path is normalised as the reference's
  parser does it with the WHATWG URL parser, and the path and the socket id are read as lossy
  UTF-8.
- **protobuf messages** of the seven public routes, with field presence: an absent field and a
  field present with its default value are different values. A hand-written reader and writer
  reproduce the code that protobufjs 7.2.4 generates, including how it reads bytes that are not
  canonical: a known field is read by its declared type whatever its wire type, a string is cut
  at the end of the buffer, a varint may take ten bytes, a later field replaces an earlier one,
  and a decoded object has the reference's key order.
- **route codecs**: the reference's encoder and decoder of each public route, request and
  response (14 directions), with their post-processing: amounts read as BigInt text, the
  length-prefixed block and transaction lists with the reference's framing and caps, and the
  exclude list of 32-byte ids. The route table holds the twelve routes with their ids, paths,
  payload limits and codec kinds; the JSON bodies of the five internal routes are not in the
  crate yet.
- **signed peer status**: the tree a relay signs over its last block and its configuration, its
  canonical JSON, the slot before the network epoch (negative in the signed text, wrapped on the
  wire), the attestations (legacy Schnorr over the text, one per key; a key listed twice is signed
  twice), and what a node does with a status it receives: the reply check with the changes it
  makes to the value (plugin names outside 4 to 64 characters removed, the generator key
  lowercased), the text the sender signed, and the signature checks.

The crate depends on `heartwood-crypto`, with `legacy-schnorr`, and on `thiserror` only. It is the
one production crate that enables `legacy-schnorr`, which only peer status messages use, and
`make wire-features` (part of `make check`) fails when another package enables it. Every decoder
takes untrusted bytes: none panics, and none recurses on the nesting of its input.

The reference's leniency with malformed protobuf bytes and lists is kept: every value it produces
is one that a well-formed message could carry, so keeping it keeps both implementations' verdicts
equal. Three inputs, which only a crafted message contains, are read differently on purpose. For
each, the oracle reports what the reference did, and the harness counts a case as a documented
difference only when that report shows it:

- a status plugin named `__proto__`: the reference's decoder replaces the prototype of the plugins
  object with the entry, and its reply check then sees the prototype's properties, which can
  refuse the reply. Heartwood drops the entry and checks the plugins the object holds itself;
- protobuf groups nested deeper than the reference's stack: the reference fails with a stack
  overflow (between 5,000 and 20,000 levels on Node 18), and Heartwood skips groups at any depth;
- an amount beyond 128 bits: the reference reads integers of any size and refuses such a value at
  a later step. Heartwood refuses it when it decodes, since reading it is unbounded work.

```sh
make test-wire                  # clippy on heartwood-p2p-wire alone, then its tests
make vectors-install-wire       # copy the wire class of the image run beside the crate
make fuzz-wire                  # the three wire fuzz targets at once, FUZZ_SECONDS each
make replay-wire                # each wire fuzz corpus through the oracle and Heartwood
```

`cargo test -p heartwood-p2p-wire` runs the unit tests and checks every record of the wire class,
`V13-p2p-wire.jsonl`, which lives beside the crate in
[packages/p2p-wire/tests/vectors](packages/p2p-wire/tests/vectors) with its own manifest.
`gen-vectors.js` writes it with the other classes; `make vectors-install` leaves it out of the set
of `heartwood-crypto`, and `make vectors-install-wire` installs it. Its records carry their own
configuration, so the crate tests them with no other class. They cover the frames the reference
sends on the port for every route and the parser's edges (every type byte, short and overrunning
lengths, path normalisation, invalid UTF-8, the writer's defaults and limits); every message with
fields present, absent and present with their defaults, with long varints, repeated and unknown
fields, groups, overruns and map entries; every codec direction both ways, the requests the
reference's clients send, the list caps and framing edges and the amount texts; and statuses
signed over blocks, verified with the verifier's own slot negative and not, and replies built to
hit each rule of the reply check. `tools/difftest/tests/l0_wire.rs` runs L0 over the class and
pins its counts: every record is compared, none is skipped, and the three documented differences
appear only in the records that assert them.

The fuzz targets take arbitrary bytes, and `make replay-wire` sends each file of their corpora to
the oracle and to Heartwood (`difftest oracle replay`), which must agree on every one:

| Target | Input | Checks | Replayed with |
|---|---|---|---|
| `nes_frame` | a frame | no panic; an accepted frame encodes to bytes that parse to the same frame | `nes.parse` |
| `wire_message` | a selector byte (modulo 14: the codecs in route order, the request before the response), then a payload | no panic; an accepted value encodes, and the encoding decodes to the same value | `wire.decodeSelected` |
| `status_verify` | a getStatus response | no panic; the decoder, the reply check, the signed text and the signature checks | `status.verify` |

`make fuzz-wire` builds the three targets, seeds their corpora (`fuzz/corpus/`, git-ignored) from
the installed wire class with `tools/wire-fuzz-seeds.js`, and runs them at the same time for
`FUZZ_SECONDS` each with `FUZZ_FORK` libFuzzer jobs. A crash, an input that runs longer than 30
seconds and an out-of-memory stop each end the run and fail it; the logs, with each target's runs,
coverage and corpus size, go to `target/wire-fuzz`. `make replay-wire` fails on a difference, on
an input that was not compared, and on a missing or empty corpus.

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
changed. `make vectors-install` refuses a run that lacks a class the committed manifest lists, such
as the genesis class of a run without `DEVNET_GENERATOR`, and then installs nothing.

| File | Content |
|---|---|
| `V01-keys-addresses.jsonl` | passphrase keys; addresses for network bytes 90, 63 and 30; `Address.validate` and `Address.toBuffer` (the network-byte complaint, a bad checksum, 33 and 35 characters, a valid checksum over a 20- or 22-byte payload); the public-key encodings the reference accepts (`02`/`03`, `04`, `06`/`07` with matching parity) and refuses |
| `V02-transactions.jsonl` | the six kept types (1/1, 1/2, 1/6, 1/7, 2/0, 2/2), v3, BIP340 with fixed aux: memos of 0, 1 and 255 bytes (2-, 3- and 4-byte characters at the end), 256 bytes refused, and a memo of replacement characters; 1, 2, 255 and 256 recipients, 0 and 257 refused; resignation types 0 to 2; votes of 1 to 53 entries; doubly signed variants. Each built case is recorded as `tx.build`, `tx.fromBytes` and `tx.fromJson`. JSON forms the builders do not write: `burnedFee` values, `asset.payments` beside a falsy `asset.transfers`, `asset.votes` values that are not objects, a vote share a few units in the last place from a cent |
| `V03-noncanonical-removed.jsonl` | wire forms the reference decodes to other bytes or refuses, with the strict verdict and the id it computes: version bytes, network byte 0, duplicate, `__proto__` and pattern-failing vote names, a stray resignation byte, memos that are not UTF-8, trailing bytes, signatures of unusual lengths, extended headers; the removed types 1/0, 1/3, 1/4, 1/5, 1/8, 1/9 and 1/10 and their inactive handlers; unknown types |
| `V04-signatures.jsonl` | BIP340 with fixed aux over messages and over the preimages of a transaction's first and second signature (also for a copy with network byte 0, which the reference writes as the configured byte) and of a block header, with a verify matrix (mutated signature, message and key, the prefix-agnostic key slice, out-of-range values); legacy Schnorr over 0 bytes to 4 KB and over real P2P status preimages, with R at infinity (at least 235 records) |
| `V05-blocks.jsonl` | `BlockFactory.make` with fixed aux for 0, 1, 9 (the kept types) and 150 transactions and for the largest block the devnet rules allow (its bytes and JSON as SHA-256), each decoded and re-encoded; 151 transactions; `maxPayload` at its boundary; `payloadLength` 32n - 1, 32n + 1 and 0; the block version; BIP340 and legacy block signatures under `bip340: false`; a transaction with network byte 0. Blocks as a node receives them from another node (`block.receive`: decoded, then built again from the fields), next to `block.fromBytes`, for transactions in wire forms that are not canonical: vote names the rules remove (the payload hash over either id), a repeated vote name, a stray resignation byte, version 0, a memo of 0xff bytes in place of replacement characters, a block at `maxPayload` under milestones of its own, and a header that counts transactions that are not sent |
| `V06-arithmetic.jsonl` | the burned fee at 9,000 basis points; the reward by rank and tier; donations at 500 basis points; the dynamic fee floor per type and size; `totalAmount`, which counts top-level amounts only |
| `V07-vote-rules.jsonl` | all 9,999 two-way cent splits (one compact record per name pair: the refused splits and SHA-256 over the verdicts and the vote assets); the vote schema on every double within 40 units in the last place of every cent (compact: the accepted count, the accepted shares that are not exact cents, SHA-256 over the verdicts), since the `multipleOf` rule works in double arithmetic; the vote schema on single cases; `sortVotes` orders; the vote asset at 1,024 and 1,025 bytes |
| `V10-genesis.jsonl` | the devnet chain definition that the devnet generator (`gen-network.js`, see below) writes for three seeds and a fixed epoch: the exact text of the four `crypto/*.json` files, the delegate and wallet keys, the nethash, the genesis id and the issued amount, and the reference's `BlockFactory.fromJson` on the result |
| `V11-external.jsonl` | the published vector files in `external/` (BIP340, the bip-schnorr draft at `d194620`, BIP39 English), each with its source URL and SHA-256 in `external/SOURCES.json`, and the reference's answers on their inputs |
| `V12-milestone-config.jsonl` | milestone files as JSON text, accepted (with the merged milestones, rewards and donations) or refused by the configuration manager's start-up checks; transaction rules at the block's height |

The first line of each file is a meta record: format `heartwood-vectors/1`; the reference's version,
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

The genesis class runs the devnet generator, `gen-network.js`: a separate tool of the maintainers,
which is not part of the reference implementation, of the oracle image or of this repository, and
is not published. `V10-genesis.jsonl` is therefore a frozen, committed class that only the
maintainers regenerate, with `DEVNET_GENERATOR` set to the generator (`difftest oracle gen
--devnet-generator FILE`); a container run mounts the generator's directory read-only. Without it
the class is not written, and the committed one stays: the tests read the devnet configuration
from it. Its meta record names the SHA-256 of the generator's files, so a regeneration shows which
generator wrote it.

## difftest

```text
difftest oracle gen --out DIR [--no-backend-check] [--devnet-generator FILE] [--image TAG | --solar-dir DIR]
difftest oracle ping [--image TAG | --solar-dir DIR]
difftest vectors verify DIR
difftest vectors diff DIR DIR
difftest run l0 DIR
difftest run l1|l2|l3|l4        says that the level is not built yet
difftest levels
difftest oracle replay OP CORPUS --vectors DIR [--height N] [--milestones FILE] [--image TAG | --solar-dir DIR]
difftest genesis --profile FILE --seed TEXT --epoch TIME --out DIR [--force] [--compare DIR]
```

Library modules: `oracle` (run the scripts in the image or locally; `OracleServer` for L1),
`vectors` (files, meta records, manifest), `subject` (a Rust implementation of one oracle
operation, shared by L0 and L1, its verdict, and the registry), `subjects` (the operations answered
with `heartwood-crypto`, with the reference's error class of a refusal where it is observable, and
each documented difference checked on the input's bytes), `compare` (a JSON diff that names the
path and, for hex values, the first differing byte with the bytes around it; numbers compare by
their double value and object keys by their JavaScript order), `report`, `replay` (a corpus through
the oracle and Heartwood), `genesis` (the chain-definition driver) and `levels` (L0 runs; L1 to L4
are described with what each one arrives with).

A difference names the JSON path and, for bytes, the first differing offset. This is
`difftest vectors diff` against a copy of the vectors with one byte and one fee changed; an L0
failure (`FAIL <file>:<line> <op> "<name>" (height H)`) prints the same lines:

```text
  DIFF V02-transactions.jsonl:2 tx.build "transfer-1"
      $.output.hex: bytes differ at offset 24 (0x18), both 154 bytes
          expected ...00032b7179bebd24[9c]e23311e958fa95e7...
          actual   ...00032b7179bebd24[0c]e23311e958fa95e7...
      $.output.json.fee: expected "50000000", actual "10000000"
  1 of 1518 records differ, 0 files missing
```

The subjects take the devnet configuration from the vector set itself: the network and milestone
files of the genesis class's chain definition that the other classes' meta records name, and the
aux of the meta records. Later crates plug in by adding subjects to `subjects`; `difftest run l0`
then compares them, with no other change.

`make test-oracle` runs the cargo test that pings `oracle-server.js` in the image and locally
(`DIFFTEST_TEST_IMAGE`, `DIFFTEST_TEST_SOLAR_DIR`); `make test` skips it.

## Licence

Not yet decided. [NOTICE](NOTICE) lists the projects Heartwood Core credits.
