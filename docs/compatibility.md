# Heartwood compatibility and testing

## How Heartwood is tested

The reference implementation is Solar Core 4.3.1 with a patch series, kept in
[iceroot-network/solar-core-ref](https://github.com/iceroot-network/solar-core-ref). The tests use
its tag `s1-ref-v2` (29 patches; `s1-ref-v1` holds the first 18). Heartwood must behave exactly like it, and differential testing shows that it
does: the same inputs go to the reference (the oracle) and to Heartwood, and the results must be
equal byte for byte, including which inputs each side rejects.

| Level | What is compared | Status |
|---|---|---|
| L0 | golden vectors written by the reference, against Heartwood | runs against all five crates: 4,370 of 4,441 records compared, all equal (see below) |
| L1 | random and mutated cases, one oracle call each | runs: nine properties, from the fee floor to the state digest (see below) |
| L2 | block sequences replayed on both sides, a state digest per block | runs: chains of reference devnets, with seeded reverts and synthetic blocks (see below) |
| L3 | the same scenario on a reference devnet and on a Heartwood devnet | planned |
| L4 | a mixed network of reference and Heartwood nodes | planned |

## Layout

```text
Cargo.toml            workspace: edition 2024, resolver 3, rust-version 1.98, workspace lints
Makefile              `make help` lists the targets
NOTICE                credits for ARK Core, Solar Core and node-emoji
packages/crypto/      heartwood-crypto: keys, signatures, the transaction and block codecs,
                      milestones and the devnet chain-definition builder (feature `genesis`)
packages/kernel/      heartwood-kernel: round and slot arithmetic, block chaining and the other
                      shared utilities, the clock seam, event names, the component lifecycle,
                      log levels, node directories and `.env` files
packages/logger/      heartwood-logger: the reference's log lines, console and file output,
                      daily rotation
packages/state/       heartwood-state: wallets, delegates, rounds, block application and
                      reversal, genesis, the supply check and the canonical state digest
packages/transactions/  heartwood-transactions: the six transaction handlers, the fee floor and
                      the pool-entry checks
packages/*/tests/vectors/  each crate's golden vectors, with its own MANIFEST.sha256
tools/difftest/       heartwood-difftest: the harness library and the `difftest` binary
  corpus/               a committed L2 corpus (a reference devnet chain and its digest lines)
tools/check-layers.sh the dependency edges between the crates, and no async, storage or HTTP crate
                      in the consensus crates
tools/check-lines.sh  code lines per crate against the review ceilings
tools/oracle/         the oracle: Node scripts and the image recipe
  Dockerfile            the oracle image, built from scratch with pinned inputs
  make-context.sh       the build context: the reference at a commit, as a git archive
  build-image.sh        docker build --no-cache --pull of that context
  docker.sh             the docker command: docker, or sudo -n docker when the daemon needs it
  gen-vectors.js        L0: writes the golden vectors
  oracle-server.js      L1: JSON lines over stdio, one request per line
  replay-chain.js       L2: the judge, which replays a corpus in the reference's own state
                        classes and checks the replay against the node that made the chain
  lib/                  solar.js (load a built reference), network.js (the devnet), ops.js and
                        ops/ (the operations), craft.js (inputs the builders do not make),
                        state.js (the reference's state machine without a database), digest.js
                        (the state digest), corpus.js, vectors/ (one file per class)
  check-native.js       checks that the reference's native modules build and load
packages/crypto/tests/vectors/external/  published vector files, with their source URLs and SHA-256
fuzz/                 cargo-fuzz targets for the transaction and block decoders, the
                      transaction handlers and whole blocks on the state (own workspace)
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
make layers lines   # crate edges and code lines per crate
make oracle-image   # build heartwood-oracle:<commit> from scratch (a few minutes)
make e2e            # vectors in the image and locally, diff the two runs, run L0
make vectors-install  # copy the image's vectors into each crate's tests/vectors
make test-oracle    # the oracle answers, and 256 L1 cases per property, in the image and locally
make l1             # the L1 properties against the oracle
make l2 NAME=txgen-one-node CORPUS=tools/difftest/corpus   # replay a corpus against its digest lines
```

`make check` needs only Rust. It lints and tests every crate on its own, with no features and
with all of them, so that Cargo does not unify the crates' features with those difftest turns on.
Its tests include each crate's own vector classes, the L0 comparison of the committed vectors of
`heartwood-crypto` and the L2 replay of the committed corpus; it also checks and formats the fuzz
targets (their own package, `fuzz/`). `make e2e` passes when the image run and the local run agree on
every record and L0 finds no difference in the image run. `make help` lists the other targets,
among them `make ping` (the oracle answers, in the image and locally), `make oracle-save`
(`docker save` of the image into `IMAGE_DIR`) and the corpus targets of L2 (below).

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
| `VECTOR_CRATES` | every crate and its classes | which classes `make vectors-install` puts into which crate's `tests/vectors` |
| `L1_CASES`, `L1_SEED`, `L1_PROPERTY` | `256`, `1`, all | cases per property, the generator's seed, the properties (`P1` to `P9` or their names, comma separated) |
| `CORPUS`, `NAME` | `target/corpus`, empty | the L2 corpus `CORPUS/NAME` |
| `ARCHIVE`, `CORPUS_SEED`, `DUMP` | empty | the devnet archives `make corpus-import` joins (in chain order); the chain's public key-derivation seed; heights whose records the judge writes |
| `ORACLE_ARGS` | `--image $(IMAGE)` | where the oracle of `l1`, `corpus-judge` and `l2-synthetic` runs; `--solar-dir DIR` for a local reference |
| `RELEASE_PACKAGES` | the five library crates | the packages `make release-check` builds without test seams |

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
for, and difftest asks for `fixed-aux`, `genesis` and `legacy-schnorr` of `heartwood-crypto` and
for `difftest` of `heartwood-kernel`. `make release-check`, part of `make check`, fails when a
package of `RELEASE_PACKAGES` builds with one of these test seams (`fixed-aux`, `difftest`).

### Fuzz targets and the chain-definition driver

The fuzz targets run with cargo-fuzz on a nightly toolchain, and a corpus of the decoders can then
be replayed against the oracle. `state_apply` gives the transaction handlers an arbitrary wallet
set and transaction: nothing may panic, a refusal changes nothing, and an accepted transaction's
revert restores every wallet. `state_blocks` gives the state machine a sequence of blocks (with
transactions of every kept type, in near slots, by the scheduled delegate or another key) and
reverts: nothing may panic, a refused block changes nothing, the supply check holds, and a revert
gives back the state from before its block.

```sh
(cd fuzz && cargo +nightly fuzz run transaction_decode -- -max_total_time=600)
(cd fuzz && cargo +nightly fuzz run state_apply -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
(cd fuzz && cargo +nightly fuzz run state_blocks -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
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

## heartwood-kernel

`packages/kernel` holds what the other crates share: the round of a height, the forger index of a
slot and its `canForge` window, the chaining rule between two blocks, and the canonical JSON that
relays sign in their status (the consensus utilities, in `utils`, read the round length and block
time from the milestone at height 1, which may not change later); epoch timestamps as the API and
log lines show them; IP addresses and the address lists of the API, peers and remote access; the
`Clock` seam; the reference's event names; the component lifecycle; the eight log levels as macros
over `tracing`; a node's config, data, cache, log and temp directories; the `.env` parser; and the
option merge (arrays are replaced). The feature `difftest` adds `ManualClock`, a test seam.

```sh
cargo test -p heartwood-kernel --locked --all-features   # unit tests and the V09-rounds and V16 classes
```

## heartwood-logger

`packages/logger` writes log lines as the reference's pretty printer does: the time, the level
name with its colour and the message, with emoji codes such as `:white_check_mark:` replaced
(the table is node-emoji's, with its MIT notice in `src/emoji.tsv`). Console lines are coloured,
file lines are not; the file rotates daily into gzip files and keeps at most 10 of them and
100 MiB. Nothing calls `init` yet: the processes arrive with the node.

```sh
cargo test -p heartwood-logger --locked   # the V20 class, rotation, retention and gzip
```

## heartwood-state

`packages/state` is the consensus state without storage: wallets and their delegate records, the
public-key and username indexes, vote balances (a voter's balance split over its delegates in basis
points, the remainder to the first entries), the delegate ranking, rounds with their forging order
(an unbiased shuffle over a SHA-256 stream seeded by the round), missed slots, the reward of a
block in its round, and `State`, which applies and reverts whole blocks in the reference's order:
missed slots, then the transactions and the forger's payout, then the round. A refused block
leaves the state as it was; every applied block keeps an undo record, so blocks revert in any
number. `State::from_genesis` builds the state after the genesis block with its explicit issuance,
and `check_supply` checks that the balances add up to what was issued plus rewards, minus burned
fees and amounts.

`digest` writes the canonical state digest of a height: five text sections (the last block, the
supply, the round, the missed slots and every wallet that holds something), their SHA-256 hashes
with domain tags, a height digest over the five, and a chain digest over a whole replay. The
oracle computes the same digest over the reference's own state, so a replay is compared at every
height and every revert.

```sh
cargo test -p heartwood-state --locked --all-features   # V08, V09-shuffle, V15, V18 and the machine tests
```

## heartwood-transactions

`packages/transactions` holds the six transaction handlers (1/6 transfer, 1/1 second-signature
registration, 1/2 delegate registration, 1/7 delegate resignation, 2/0 burn, 2/2 vote), each with
its checks in the reference's order, its apply and revert, and its genesis path; the dynamic fee
floor, `(addon bytes + half the size, rounded) × max(minFee, 1)` per type; the checks that decide
whether a transaction may enter the pool; and error types whose `class()` is the reference's error
class. The registry is closed: it dispatches on the transaction type, and no plugin can replace a
rule. A handler runs every check before it changes a wallet.

```sh
cargo test -p heartwood-transactions --locked --all-features   # handlers, pool checks, V17 and V19
```

## L0: golden vectors

`difftest run l0` compares the records of a vector run with Heartwood, with one subject per
oracle operation. Where both sides refuse, it also compares the error class wherever Heartwood
models the reference's class. On a run of every class (reference tag `s1-ref-v2`) it reports:

| Outcome | Records |
|---|---|
| compared, equal to the reference | 4,370 |
| documented differences, each with its form checked on the input | 60 |
| skipped: the peer status message (P2P codec), and builders of types or headers that Heartwood does not build | 11 |

`cargo test` checks the classes of each crate in that crate; the difftest test `l0` runs the
comparison over the committed vectors of `heartwood-crypto` (1,484 compared, 23 documented, 11
skipped).

All but the last of the documented differences are inputs that Heartwood refuses and the
reference accepts:

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
  reference;
- milestone files whose fee table (`dynamicFees`) or revoke delay
  (`blocksToRevokeDelegateResignation`) has a value of a type the reference would coerce, or fail
  on only when it checks a fee, such as a string or a fraction where a whole number belongs.
  Heartwood refuses such a file at load. Generated devnets never contain one;
- a transaction from a sender whose wallet has no public key yet, which both sides refuse with the
  same error class. The reference binds the sender's key to the wallet before its checks run and
  keeps it after the refusal; Heartwood leaves the wallet as it was. The comparison leaves that key
  out and compares everything else.

## L1: properties

`difftest run l1` (`make l1`) generates random and mutated cases with proptest and sends each one
to the oracle once, then compares the answer with Heartwood's. A failed property is shrunk to a
minimal case, which is written to `--regressions`.

| Property | Cases | Operation |
|---|---|---|
| P1 fee floor | transactions of the six types with random memos, recipients, votes and fee tables | `fee.minimum` |
| P2 rounds | heights up to 2^32 - 1, 1 to 53 delegates | `round.calculate`, `round.isNew` |
| P3 slots | timestamps and heights; pairs of headers | `forging.info`, `block.isChained` |
| P4 shuffle | rounds up to 2^53 - 1, 1 to 53 delegates | `state.shuffle` |
| P5 ranking | 1 to 80 delegates with many equal vote balances and resignations | `state.rank` |
| P6 vote amounts | balances up to 2^63 - 1, 1 to 53 votes | `wallet.voteAmounts` |
| P7 handlers | wallet sets of 3 to 12 wallets and a mutated transaction from one of them | `state.applyTransaction`: the verdict, the error class, and the wallets after apply and after revert |
| P8 kernel text | JSON values, `.env` text, address lists | `kernel.stringify`, `kernel.dotenv`, `kernel.ip` |
| P9 digest | wallet sets | `state.digest` |

With the date as the seed and 10,000 cases per property (50,000 for P7), every property agrees
with the reference. The only documented difference that appears is the one of a refused
transaction from a sender without a public key (P7): the reference binds the key before its
checks run and keeps it after the refusal, Heartwood leaves the wallet as it was. The report
lists such cases apart, and the comparison leaves that key out and compares everything else. P7
also counts the reference's error classes it reached; each class of the handlers' checks that a
transaction can reach from a consistent wallet set appears.

## L2: chains, corpora and the state digest

A corpus is a chain that a reference devnet produced, imported with `difftest corpus import` from
the archives of the maintainers' devnet tooling (a separate tool that is not published): the chain definition, every block as a relay receives it, and what
the node itself recorded (round rows, missed slots, block columns, API checkpoints). The oracle's
judge, `replay-chain.js` (`difftest corpus judge`), replays the corpus in the reference's own state
classes with storage replaced by memory, checks that replay against the node's records, follows a
seeded plan of reverts and re-applies, and writes one digest line per step. `difftest run l2`
replays the same steps in Heartwood and compares every line; it also checks, before each block,
that the scheduled forger is the block's generator and that the reward in its round is the header's,
and runs the supply check after every step. `--synthetic` adds blocks that the harness signs with
the devnet's derived keys on top of the chain (refused blocks, a decoding rule and the handlers'
fee floor, burn minimum and revoke delay changing at a height, a vote at a round's end, burns and
fees at their limits, a block by a key that is no delegate, and a competing chain after a revert
across a round end) and sends them to both sides.

```sh
make corpus-import ARCHIVE='archive/...' NAME=my-chain   # into target/corpus/my-chain
make corpus-judge NAME=my-chain                           # the judge's digest lines and checks
make l2 NAME=my-chain                                     # Heartwood against those lines
make l2-synthetic NAME=my-chain                           # with the synthetic blocks (needs the oracle)
cargo test -p heartwood-difftest --locked --test l2       # the committed corpus, no oracle
```

The committed corpus, `tools/difftest/corpus/txgen-one-node`, is a one-node devnet chain of five
rounds with every kept transaction type; its README describes it. The maintainers also replay
longer chains made with the same unpublished tooling (three nodes with partitions, kills and
restarts, and a soak of at least 50 rounds); they are imported under `target/corpus` and are not
committed.

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
(`sha256sum` format; `difftest vectors verify` checks it). Each crate keeps its classes, with a
manifest of its own, in `packages/<crate>/tests/vectors` (`VECTOR_CRATES` lists which);
`make e2e` followed by `make vectors-install` regenerates them from one image run, and a
regeneration must be bit-identical unless the oracle changed. `make vectors-install` refuses a run
that lacks a class a committed manifest lists, such as the genesis and genesis-state classes of a
run without `DEVNET_GENERATOR`, and then installs nothing.

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
| `V08-ranking.jsonl` (state) | the delegate ranking over generated sets: equal vote balances broken by public key, resignations, 53 to 80 delegates, rounds larger than the ranking, identical keys refused; the key order over random key pairs, every first byte and keys that first differ at each byte |
| `V09-rounds.jsonl` (kernel) | the round of a height and whether it starts one, for heights 1 to 10,000, ranges hashed, 1 to 53 delegates and heights near 2^32; the forger index, next forger, block timestamp and `canForge` over slot edges, half slots, gaps and times before the epoch; block chaining (previous id, height plus one, a later slot) |
| `V09-shuffle.jsonl` (state) | the forging order: for every count 1 to 53 the SHA-256 of the orders of rounds 1 to 10,000; full orders of rounds 1 to 100 for counts 1, 2, 3, 52 and 53; rounds where a draw is rejected; round 2^53 - 1 |
| `V15-vote-amounts.jsonl` (state) | a voter's balance split over 1 to 53 delegates, for balances 0, 1, 101, near 2^63 and random, the remainder in map order |
| `V16-kernel.jsonl` (kernel) | the status canonical JSON; `.env` text (separators, comments, quotes, digits, booleans, empty values); epoch timestamps; address lists with globs, brackets and IPv6; node directories with their environment overrides |
| `V17-handlers.jsonl` (transactions) | the transaction handlers on wallet sets: at least one case of each error class and of each apply effect, the revoke delay at 105, 106 and 107 blocks, the fee floor and one below it, the burn minimum and one below it, votes identical, empty, too many and unknown, second signatures missing, wrong and unexpected, and a refused first transaction of a wallet without a key |
| `V18-genesis-state.jsonl` (state) | the state digest after the genesis block of the three V10 chain definitions and of six variants of the first (fees and burns, a second key, resignations and a revoke, split votes, a transfer before the issuance record, more issuance records, a generator that registers and votes, a vote and a resignation before their registration; two that the reference refuses to start on: a negative balance, too few delegates for round 1), and of five wallet sets, one per record shape |
| `V19-fee-floor.jsonl` (transactions) | the dynamic fee floor of the six types over memo lengths, recipients, votes of 1 to 53 entries and resignation types; fee tables switched off, with `minFee` 0 and negative, and without a type's key; tables and revoke delays of the wrong type |
| `V20-logger.jsonl` (logger) | the log line of every level, coloured and plain, with and without emoji codes (the whole table, in slices), over times at the edges of a JavaScript date and messages with line breaks and colour codes |

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
difftest run l1 [--property P]... [--cases N] [--seed S] [--vectors DIR] [--regressions DIR] [ORACLE]
difftest run l2 DIR [--synthetic] [ORACLE]
difftest run l3|l4              says that the level is not built yet
difftest corpus import ARCHIVE... --out DIR [--seed TEXT]
difftest corpus judge DIR [--dump H,...] [ORACLE]
difftest corpus verify DIR
difftest levels
difftest oracle replay OP CORPUS --vectors DIR [--height N] [--milestones FILE] [ORACLE]
difftest genesis --profile FILE --seed TEXT --epoch TIME --out DIR [--force] [--compare DIR]
```

`ORACLE` is `--image TAG` or `--solar-dir DIR`; `difftest --help` lists every option.

Library modules: `oracle` (run the scripts in the image or locally; `OracleServer` for L1),
`vectors` (files, meta records, manifest), `subject` (a Rust implementation of one oracle
operation, shared by L0 and L1, its verdict, and the registry), `subjects` (the operations answered
with the node's crates, with the reference's error class of a refusal where it is observable, and
each documented difference checked on the input), `compare` (a JSON diff that names the
path and, for hex values, the first differing byte with the bytes around it; numbers compare by
their double value and object keys by their JavaScript order), `report`, `replay` (a corpus of
inputs through the oracle and Heartwood), `genesis` (the chain-definition driver), `fixture`
(wallet sets as both sides read them), `strategies` (the L1 generators), `corpus` and `digests`
(L2 corpora and digest files), `synthetic` (the L2 synthetic blocks) and `levels` (L0, L1 and L2
run; L3 and L4 are described with what each one arrives with).

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

`make test-oracle` runs the cargo tests that ping `oracle-server.js` and run 256 L1 cases per
property, in the image and locally (`DIFFTEST_TEST_IMAGE`, `DIFFTEST_TEST_SOLAR_DIR`,
`DIFFTEST_TEST_L1_CASES`); `make test` skips them.

## Licence

Not yet decided. [NOTICE](../NOTICE) lists the projects Heartwood Core credits.
