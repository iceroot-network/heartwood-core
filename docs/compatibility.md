# Heartwood compatibility and testing

## How Heartwood is tested

The reference implementation is Solar Core 4.3.1 with a patch series, kept in
[iceroot-network/solar-core-ref](https://github.com/iceroot-network/solar-core-ref). The tests use
its tag `s1-ref-v2` (29 patches; `s1-ref-v1` holds the first 18). Heartwood must behave exactly like it, and differential testing shows that it
does: the same inputs go to the reference (the oracle) and to Heartwood, and the results must be
equal byte for byte, including which inputs each side rejects.

| Level | What is compared | Status |
|---|---|---|
| L0 | golden vectors written by the reference, against Heartwood | runs against the seven crates with vector classes: 6,220 of 6,387 records compared, all equal (see below) |
| L1 | random and mutated cases, one oracle call each | runs: seventeen properties, from the fee floor to the pool, the crypto files, the peer-to-peer client's network monitor and the forger's blocks and decisions (see below) |
| L2 | block sequences replayed on both sides, a state digest per block | runs: chains of reference devnets through Heartwood's store, with seeded reverts, restarts at every height, synthetic blocks and pool sessions, and every block of those chains rebuilt by the forger (see below) |
| L3 | the same scenario on a reference devnet and on a Heartwood devnet | planned |
| L4 | a mixed network of reference and Heartwood nodes | planned |

## Layout

```text
Cargo.toml            workspace: edition 2024, resolver 3, rust-version 1.98, workspace lints
Makefile              `make help` lists the targets
NOTICE                credits for ARK Core, Solar Core and node-emoji, and the licence notice of nes
packages/crypto/      heartwood-crypto: keys, signatures, the transaction and block codecs,
                      milestones, the fee floor and the devnet chain-definition builder
                      (feature `genesis`)
packages/kernel/      heartwood-kernel: round and slot arithmetic, block chaining and the other
                      shared utilities, the clock seam, event names, the component lifecycle,
                      log levels, node directories and `.env` files
packages/logger/      heartwood-logger: the reference's log lines, console and file output,
                      daily rotation
packages/state/       heartwood-state: wallets, delegates, rounds, block application and
                      reversal, genesis, the supply check and the canonical state digest
packages/transactions/  heartwood-transactions: the six transaction handlers, the fee check and
                      the pool-entry checks
packages/p2p-wire/    heartwood-p2p-wire: the peer-to-peer wire codec (nes frames, the protobuf
                      messages of the public routes, list framing, the signed peer status)
packages/database/    heartwood-database: the store, one SQLite file per node (the reference's
                      chain tables, the state tables, the chain's identity, the start checks)
packages/pool/        heartwood-pool: the transaction pool, with its storage, the fee rules, the
                      queries, the collator and the node-local online record
packages/blockchain/  heartwood-blockchain: so far, a block's commit to the store and a node's
                      start (first start, restart from the state tables, replay from genesis)
packages/p2p/         heartwood-p2p: the client side of the peer-to-peer protocol (nes sessions,
                      requests and reply checks, peer acceptance and verification, the network
                      monitor), not yet wired into a node; the mock peer behind `difftest`
packages/forger/      heartwood-forger: the forger (the slot loop its relay drives, the relay
                      client, block assembly, the signing record, the key file and the status
                      attester), not yet wired into a process
packages/cli/         heartwood-cli: the commands without the node (the colon aliases,
                      instances, the supervisor and the role commands, `network generate`'s
                      files, the local devnet library)
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
                        state.js (the reference's state machine without a database, and its two
                        start paths), pool.js (the reference's pool), digest.js (the state
                        digest), corpus.js, vectors/ (one file per class)
  check-native.js       checks that the reference's native modules build and load
tools/wire-fuzz-seeds.js  seeds the wire fuzz corpora from the wire vectors
tools/p2p-fuzz-seeds.js   seeds the p2p fuzz corpora from a vector run's V24 and V26 records
tools/forger-fuzz-seeds.js  seeds the forger's internal_body corpus from a vector run's V29 records
packages/crypto/tests/vectors/external/  published vector files, with their source URLs and SHA-256
fuzz/                 cargo-fuzz targets for the transaction and block decoders, the
                      transaction handlers, whole blocks on the state, the state's records, the
                      pool, the wire decoders, the peer-to-peer client and the forger's inputs
                      (own workspace)
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
make l2-restart NAME=txgen-one-node CORPUS=tools/difftest/corpus   # the same with restarts
make crash-test NAME=txgen-one-node CORPUS=tools/difftest/corpus   # the store after SIGKILLs
make pool-bench     # the cost of the pool's per-sender views and of the store's commit
```

`make check` needs only Rust. It lints and tests every crate on its own, with no features and
with all of them, so that Cargo does not unify the crates' features with those difftest turns on.
Its tests include each crate's own vector classes, the L0 comparison of the committed vectors of
`heartwood-crypto` and of the wire class of `heartwood-p2p-wire`, and the L2 replay of the
committed corpus; it also checks and formats the fuzz targets (their own package, `fuzz/`).
`make e2e` passes when the image run and the local run agree on every record and L0 finds no
difference in the image run. `make help` lists the other targets, among them `make ping` (the
oracle answers, in the image and locally), `make oracle-save` (`docker save` of the image into
`IMAGE_DIR`) and the corpus targets of L2 (below).

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
| `L1_CASES`, `L1_SEED`, `L1_PROPERTY` | `256`, `1`, all | cases per property, the generator's seed, the properties (`P1` to `P12`, `P14` to `P17`, `P19`, or their names, comma separated) |
| `CORPUS`, `NAME` | `target/corpus`, empty | the L2 corpus `CORPUS/NAME` |
| `ARCHIVE`, `CORPUS_SEED`, `DUMP` | empty | the devnet archives `make corpus-import` joins (in chain order); the chain's public key-derivation seed; heights whose records the judge writes |
| `TO` | empty | cut the corpus at this height, in `corpus-judge`, `l2`, `l2-synthetic` and `l2-restart` (the judge and the replays must use the same cut) |
| `RESTARTS` | empty | the restart plan `corpus-judge` writes into the corpus before it judges: `every` (a restart at every height) or `rounds` (the round ends, the first blocks of rounds, 20 seeded heights and the node's live restarts) |
| `KILLS`, `KILL_SEED` | `200`, empty | the number of kills of the kill test and of the forger's crash test, and the seed of their delays |
| `DIRECTION` | empty | `oracle` makes `forge-replay` have the reference rebuild the corpus's blocks; empty, Heartwood rebuilds them |
| `ORACLE_ARGS` | `--image $(IMAGE)` | where the oracle of `l1`, `corpus-judge`, `l2-synthetic` and `replay-wire` runs; `--solar-dir DIR` for a local reference |
| `RELEASE_PACKAGES` | the ten library crates | the packages `make release-check` builds without test seams |
| `FUZZ_SECONDS`, `FUZZ_FORK` | `600`, `8` | how long each wire or p2p fuzz target runs, and its libFuzzer jobs |
| `WIRE_SEEDS`, `WIRE_SEED_MAX` | the installed wire class, `65536` | the vector file that seeds the wire fuzz corpora, and the longest seed in bytes |
| `WIRE_LOGS` | `target/wire-fuzz` | the logs of `make fuzz-wire` and `make replay-wire` |
| `P2P_SEEDS`, `P2P_LOGS` | `$(VECTORS)/local`, `target/p2p-fuzz` | the vector run that seeds the p2p fuzz corpora, and the logs of `make fuzz-p2p` and `make replay-p2p` |
| `FORGER_SEEDS`, `FORGER_LOGS` | `$(VECTORS)/local`, `target/forger-fuzz` | the vector run that seeds the forger's internal_body corpus, and the logs of `make fuzz-forger` |

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
(merged and checked at load, looked up by height), rewards, donations, the fee burn, and the
dynamic fee floor, `(addon bytes + half the size, rounded) × max(minFee, 1)` per type, a pure
function of the transaction's type, its size and the milestone, so that a client can compute it
before it signs. The crate forbids unsafe code, and its decoders return typed errors instead of
panicking on untrusted input.

Every signature names what it is for with a signing domain: a transaction, a block, a peer status
or an off-chain message. In this signature format the domain changes no bytes; an off-chain message
is signed as the reference signs it, BIP340 over SHA-256 of the message's UTF-8 text. From the
post-quantum formats on, each domain carries its own tag of the signing-tag registry.

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

`utils::js` holds the crate's JavaScript number semantics, among them the number formatting of
the reference's messages: `toLocaleString` of an integer (English grouping), and of a double with
at most a given number of fraction digits, rounded as Node's ICU rounds it (the shortest decimal
digits of the double, a tie away from zero). `utils::format_satoshi` writes an amount with the
network's symbol as the reference's `formatSatoshi` does, and `Transaction::display` writes a
transaction's short text, `<sender>#<nonce> <last 8 hex of the id> <type> v<version>`, as the
reference's `toString`.

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
gives back the state from before its block. `state_records` reads arbitrary text as each of the
state's records and restores a state from records changed byte by byte: a refusal is a typed error,
and a state written as records reads back equal. `pool_process` runs sequences of batches, clock
moves, removals, re-adds, blocks and storage restarts on a small fixture: nothing may panic, the
pool stays within its limits, each sender's transactions apply in order on a fresh view, a
collation is in fee order with each sender's nonces in order, and the storage holds exactly the
pool's transactions.

```sh
(cd fuzz && cargo +nightly fuzz run transaction_decode -- -max_total_time=600)
(cd fuzz && cargo +nightly fuzz run state_apply -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
(cd fuzz && cargo +nightly fuzz run state_blocks -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
(cd fuzz && cargo +nightly fuzz run state_records -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
(cd fuzz && cargo +nightly fuzz run pool_process -- -fork=12 -max_total_time=600 -rss_limit_mb=4096)
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
option merge (arrays are replaced); and `pluralise`, the word and count of the reference's
messages. The feature `difftest` adds `ManualClock`, a test seam.

`config::crypto` loads a network's four crypto files (`network.json`, `milestones.json`,
`genesisBlock.json`, `exceptions.json`). All four must be there: a missing file stops the start and
is named, where the reference falls back to a built-in preset. Before the crypto crate's loaders
read them, the network and milestone files pass a strict schema (`config::schema`): an unknown key
at any level, a `null` anywhere, a height given twice or a value of the wrong type is refused with
the file, the height and the key path. The keys of removed features are accepted only at their off
values (`legacyTransfer: false`, `legacyVote: false`, `bip340: true`,
`acceptLegacySchnorrTransactions: false`, `htlcEnabled: false`), and `exceptions.json` must be
exactly `{}`. The reference loads every such file, so these refusals are configuration-only
differences; generated devnets never contain one.

```sh
cargo test -p heartwood-kernel --locked --all-features   # unit tests, the loader's refusals, V09-rounds, V16 and V23
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

`persist` holds the state's records, what the store keeps so that a restarted node reads its
state back instead of replaying the chain: one record per wallet (its delegate record, votes, vote
shares and resignation height included), one for the rest of the tip's state (the supply totals
and the round), and one undo record per block. They are versioned `serde` types that refuse
unknown fields, with amounts as decimal text. `State::restore` builds a state from them and checks
it first: the last block, every key and name held once, each delegate's vote balance and voter
count against its voters' shares, the supply, the forging order against the round's shuffle, and
one undo record per height. `WalletRepository::changed_since` lists the wallets that changed since
an earlier copy, which is what a block's commit writes.

```sh
cargo test -p heartwood-state --locked --all-features   # V08, V09-shuffle, V15, V18, the machine tests and the records on the committed chain
```

## heartwood-transactions

`packages/transactions` holds the six transaction handlers (1/6 transfer, 1/1 second-signature
registration, 1/2 delegate registration, 1/7 delegate resignation, 2/0 burn, 2/2 vote), each with
its checks in the reference's order, its apply and revert, and its genesis path; the fee check
against the dynamic fee floor of `heartwood-crypto` (`minimum_fee` gives the floor of a decoded
transaction); the checks that decide whether a transaction may enter the pool; and error types whose
`class()` is the reference's error class. `TransactionError::message` writes a refusal's message
as the reference does, with amounts in the network's symbol, and the pool-entry refusals carry the
reference's texts. The registry is closed: it dispatches on the transaction type, and no plugin can
replace a rule. A handler runs every check before it changes a wallet.

```sh
cargo test -p heartwood-transactions --locked --all-features   # handlers, pool checks, V17 with every message
```

## heartwood-database

`packages/database` is a node's store: one SQLite file, `<data>/database/blockchain.db`, with one
writer connection (write-ahead log, `synchronous = FULL`, foreign keys, SQLite's defensive mode, no
memory-mapped I/O; extensions are never loaded). Schema version 1 holds the reference's four chain
tables with its columns (`blocks`, `transactions`, `rounds`, `missed_blocks`), the state tables
(`wallets`, `state`, `undo`), `meta` (the schema version, the network's nethash, the genesis
block's id and the state format), the fingerprints of the applied milestone entries and the
node-local delegate versions. Ids, keys and hashes are lowercase hex text. Amounts are integers
written through a checked conversion: a value beyond `2^63 - 1` is a storage error, where the
reference's `bigint` columns stop too. The amount column of a transaction holds 0 where the
reference stores `NULL`. A file of another schema version, or of another program, is refused.

Writes go in chunks: one SQLite transaction per chunk of blocks, with a savepoint per block, so a
chunk is stored whole or not at all. On every start the store checks itself: the block count is
the tip's height; the transaction count, fee sum and amount sum agree with the block columns; no
round rows lie above the next round; there is one undo record per height from 2 to the tip; and the
state row names the tip. The crate stores rows, not state: it does not depend on
`heartwood-state`.

```sh
cargo test -p heartwood-database --locked   # row round trips, chunks, reverts, the start checks, killed commits
```

The kill test (`tests/kill.rs`) kills a child process that commits chunks in a loop with SIGKILL
at random moments, 20 times by default (`HEARTWOOD_DATABASE_KILLS` sets another number), and
checks that the file holds the last reported chunk or the one after it, complete.

## heartwood-pool

`packages/pool` is the transaction pool. It admits transactions as the reference's pool does: the
same checks in the same order, the same error codes and messages, the same `accept`, `broadcast`,
`invalid` and `excess` lists and `errors` map in a processor's result, and the same orders when it
is queried and collated. Every admission, re-add and collation runs under the rules of the next
block (the tip plus one); the collator's byte budget is the tip's.

- A processor decodes a batch on up to `worker_count` threads and then adds its entries in the
  reference's two loops: first the decoding and a 30-second cooldown per transaction id (each
  processor has its own, as the API and the peer-to-peer port each hold one), then the pool's
  checks: a duplicate, the fee rule (the milestone's dynamic fees; when they are off, the pool's
  own floor, or with that off too the exact static fee), a full pool, the per-sender limit, and on
  the sender's view the size, the network, the signature and the handler's pool-entry checks and
  apply.
- Each sender has its own view of the wallets, a copy of the state's, so a transfer waiting in the
  pool does not fund its recipient's own transactions.
- Admitted transactions are stored in `<data>/pool/pool.sqlite`, in the reference's table, and a
  restarted node adds them back. Re-adds, clean-ups (by age and by size), removals with the
  sender's later transactions and the removal of forged transactions follow the reference.
- A removed transaction type is refused at decode, but answered as the reference answers it:
  `ERR_OTHER` "Transaction type Core/N is deactivated".
- Votes, and revokes of a resignation, enter the pool only for delegates that operate a node. That
  online record is node-local: `heartwood-blockchain` saves it in the node's store and every start
  loads it back with each entry's round, and an entry expires five rounds after its round.

Three things differ on purpose. The first two show only after a crash or with an invalid
transaction; the third depends on when the online record changes. A transaction is stored only
once it is admitted: the reference stores it first, so a crash before its checks end
leaves a row that its re-add trusts. In a full pool, a new transaction is checked before the
lowest-priority one is evicted for it: the reference evicts first, so an invalid transaction with a
higher fee evicts a valid one. And the pool reads the online record when it checks, where the
reference's view of a sender keeps the copy of a delegate's wallet it took when it first read it,
until the view is rebuilt after the next block.

The pool is synchronous: each operation takes `&mut self` and returns what it added and removed,
and it reads the time only through the kernel's clock seam.

```sh
cargo test -p heartwood-pool --locked   # every admission step with its code and message, lists, cooldown, orders, collation, storage
make pool-bench                          # the cost of the per-sender views and of the store's commit (release build)
```

Each sender's view is a full copy of the state's wallets, which `make pool-bench` measures: at
100,000 wallets one copy takes about 0.5 ms and 4.3 MiB, but adding back the transactions of 1,000
senders at 10,000 wallets takes about 340 ms and 680 MiB, and memory grows with senders times
wallets. A cheaper view is needed before a network with more than a few thousand wallets. The same
target measures the store's commit: between about 8 and 17 ms per block on the maintainers'
machines, nearly all of it the file system's sync under `synchronous = FULL`, and about 1,800 bytes
of database per block on the committed chain (undo records about 560 bytes on average). The
commit's own wallet work grows with the number of wallets, not with the number a block changes:
it copies the wallets and scans them for the changed ones, about 2.3 ms per block at 100,000
wallets and 75 ms at 1,000,000, which the committed chain's few dozen wallets do not show.

## heartwood-blockchain

`packages/blockchain` holds, so far, the part of the blockchain component that stores the chain and
restores the state from it; the block processor, the queue and the state machine that drive it come
later. A chunk writer commits each applied or reverted block in the chunk's one transaction: the
block and transaction rows with the columns the reference derives (the generator's username, the
burned fee, the donations), the wallets that changed since the last commit, the block's undo
record, the state row, the round rows at a round start and the block's missed slots. A revert
deletes what lies above the new tip, as the reference's does: after a revert to the last block of a
round, that includes the next round's rows, which the round's last block saved and nothing saves
again before the round after it starts. The start never reads round rows, so a restart in such a
round restores the round from the state; the reference's start reads no delegate for it and keeps
an empty forging list, or, when its store holds no missed-slot row, rolls the chain back. This is
the one place where a Heartwood restart and the reference's differ.

`restore` is a node's start:

- An empty store is a first start: the genesis block is checked and stored with round 1's rows,
  the wallets, the state row and the chain's identity, in one transaction.
- Otherwise the start first checks the store (the checks above, and that it holds the configured
  network and genesis block) and stops, naming the check, when one fails; there is no automatic
  rewind.
- The state is then read back from the state tables. When they are missing, of an older format or
  fail `State::restore`'s checks, or when a replay is asked for, every stored block is applied
  again from the genesis block through the handlers, and the state tables are written again. A
  balance that would go below zero during a replay stops the start with the reference's text,
  "State Generation failed: Wallet with negative balance".
- A milestone entry that the tip has reached is fingerprinted when it is first reached. A start
  refuses a milestone file in which such an entry changed or disappeared, or which adds an entry at
  or below the tip, and names the height to roll back to. `p2p` and `fees.staticFees` are left out
  of the fingerprint: they decide which peers a node talks to and what its pool takes, never whether
  a block is valid.
- Every start also returns the pool's online record from the store (`Restored::online`);
  `ChainStore::save_online` replaces the stored record, outside any chunk.

```sh
cargo test -p heartwood-blockchain --locked   # commit and restart at every height of the committed chain, the start's refusals, the milestone check
```

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

- a status plugin named `__proto__` with a value: the reference's decoder makes the plugin the
  prototype of the plugins object, and its reply check then sees the plugin's properties among the
  plugins and refuses the reply. Heartwood drops the entry and checks the plugins the object holds
  itself. Where the reference's check does not refuse the plugins, such a reply is compared as
  usual. Where an entry without a value has made the prototype `null`, a later `__proto__` is an
  ordinary plugin; the reference then checks the signatures against a text that lists that
  plugin's fields among the plugins, and Heartwood builds the same text;
- protobuf groups nested deeper than the reference's stack: the reference fails with a stack
  overflow (between 5,000 and 20,000 levels on Node 18), and Heartwood skips groups at any depth;
- an amount beyond 128 bits: the reference reads integers of any size. It refuses such a getBlocks
  response at a later step, but accepts the total amount and total fee of a getStatus reply whose
  header is at height 1 and verifies the reply's signatures over the whole integer. Heartwood
  refuses the payload when it decodes, since reading such an integer is unbounded work.

```sh
make test-wire                  # clippy on heartwood-p2p-wire alone, then its tests
make fuzz-wire                  # the three wire fuzz targets at once, FUZZ_SECONDS each
make replay-wire                # each wire fuzz corpus through the oracle and Heartwood
```

`cargo test -p heartwood-p2p-wire` runs the unit tests and checks every record of the wire class,
`V13-p2p-wire.jsonl`, which lives beside the crate in
[packages/p2p-wire/tests/vectors](../packages/p2p-wire/tests/vectors) with its own manifest. The
tests fail when it is missing; `WIRE_VECTORS=target/vectors/local` points them at a local oracle
run instead. `gen-vectors.js` writes it with the other classes, and `make vectors-install`
installs it beside the crate (the `p2p-wire` row of `VECTOR_CRATES`). Its records carry their own
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

## heartwood-p2p

`packages/p2p` is the client side of the peer-to-peer protocol: how a relay follows the network.
It connects to peers, verifies their chains, downloads and broadcasts blocks and transactions, and
keeps its peer list, with the reference's requests, timeouts, limits, cadences and decisions. It
opens no listening socket, and it is not yet wired into a node: it reads the node's chain and
tells the node about peers only through the traits of its `view` module, so it depends on no
crate of the chain, the store or the pool. Its edges are `heartwood-crypto`, `heartwood-kernel`
and `heartwood-p2p-wire`, with tokio and tokio-tungstenite; `make layers` lets no other crate
depend on tokio-tungstenite directly, and no consensus crate on tokio.

- **nes sessions**: the reference's nes client on a WebSocket, one session per peer IP: the hello,
  the heartbeat (the server's interval plus its timeout), the one-second ping limit, the
  connection-wide request timeout and receive limit with the points where each is reset, the
  closes for an oversized message (1009), for broken frames (1002) and for bad text (1007), the
  single reconnect after one second, and the connector's pause of ten seconds between sessions to
  one IP. The frames it writes are the reference client's, byte for byte (the client records of the
  wire class).
- **requests and replies**: every request of the reference's client, with each route's receive
  limit and timeout, the outgoing rate limiter (it waits instead of sending too fast), the reply
  check of each route, the error-class rule that decides whether a failed request only counts
  against the peer or disconnects it, the disconnect after three failures in a row, the per-peer
  queue of postTransactions batches, and the ping with its status attestations. The client never
  writes a request that a reference server refuses for its shape: it asks for at most 400 blocks
  at a time and posts batches of at most `maxTransactionsPerRequest` transactions.
- **peers**: the acceptance of candidate peers (the address rules, the whitelist and blacklist,
  the same-subnet cap, the version gate, a ping and a port probe), and the verification of a
  peer's claimed chain: an n-ary search for the highest common block, then the peer's blocks after
  it up to the end of the round.
- **the network monitor**: seeds and peer sources, discovery every 600 seconds, the cleanse of
  slow peers, the network height, fork health, block downloads in chunks of 400 with a chunk cache
  and backup peers, transaction downloads, and the broadcast of blocks (after 500 ms) and of
  transactions (in batches of 40).

Where the reference is lenient with a peer that breaks the protocol, Heartwood is stricter on
purpose. None of these differences shows with an honest peer or a generated devnet's
configuration, except the last, which needs a transaction that no honest wallet writes:

- a fork segment fetched during verification must chain: each header to the one before it, and
  the first to the node's block at the common height. The reference checks each header's
  signature, height and round delegate only. The harness reports these records as documented
  differences, with the unlinked header checked on the input;
- protocol violations are recorded against a peer for 600 seconds: a reply the codec or the reply
  check refuses, a broken nes session, a downloaded transaction the reader refuses, and in a
  verification a common-block reply that names a block not asked for or a fork header that fails
  its checks or does not chain. Nothing an honest peer can cause is a violation: a heartbeat
  timeout, a response after its request timed out, two server pings bunched by a stall, a memo
  that is not valid UTF-8 (the reference accepts it); the session still ends as the reference's.
  Violations count toward the same ignore rule as the cleanse's slow pings, and a peer with a
  violation is not asked for peers, blocks or transactions and is not accepted again until it
  expires. An honest slow peer stays a source;
- the random samples (the discovery sources, the transaction sources, the peers a cleanse pings,
  a chunk's backup peers and the peers whose ports are probed) are drawn across subnets (/24 and
  /48) after the reference's shuffle;
- a peer source over HTTPS is refused when the monitor starts; plain HTTP sources are read up to
  4 MiB, and local files as JSON at each seeding;
- a version range outside the npm semver forms the client reads (hyphen ranges, operators or stars
  inside a word) satisfies no version, so a version gate made only of such ranges refuses every
  peer;
- a downloaded block that holds a removed transaction type or a memo that is not valid UTF-8 fails
  that peer's whole getBlocks reply, and the monitor asks the next peer, since the reader refuses
  both (see L0 below).

```sh
make test-p2p                   # clippy and tests of heartwood-p2p alone, without and with difftest
cargo test -p heartwood-p2p --all-features -- --include-ignored   # also the integration tests
make fuzz-p2p                   # the three p2p fuzz targets at once, FUZZ_SECONDS each
make replay-p2p                 # the reply_check and listed_block corpora through the oracle and Heartwood
```

The crate's tests run on tokio's paused clock, against fakes of the parts each test does not
cover and against the mock peer (the `testing` module, behind the `difftest` feature): a nes
server over in-memory connections or TCP that answers from a script or serves a corpus chain. The
tests marked `integration` join the real parts (the communicator over the connector and the mock
peer, the verifier through the communicator, the monitor's cleanse and download against three
mock peers); they use real time and loopback addresses `127.0.0.x`. The mock peer also runs alone,
`cargo run -p heartwood-p2p --features difftest --bin mock-peer -- --listen <ip:port> --chain <dir>
[--empty-first-getblocks]`, serving a corpus chain (such as `tools/difftest/corpus/txgen-one-node`)
until it is killed. The test `devnet_smoke` of `tests/session.rs` runs against a reference node:
with `HEARTWOOD_SMOKE_PEER=<ip:p2p port>`, `cargo test -p heartwood-p2p --all-features --test
session devnet_smoke -- --ignored --nocapture` sends getStatus, getPeers and a getBlocks of 400
blocks from height 1, checks each reply, answers the node's pings for a minute and closes the
session; without the variable it does nothing.

Three vector classes cover the crate (see Vector files). They are not installed beside the crates
yet: the tests read `V24-p2p-exchange` from the directory in `WIRE_VECTORS` (`heartwood-p2p-wire`),
`V25-peer-rules` from `P2P_VECTORS` (`heartwood-p2p`) and `V26-network` from `P2P_VECTORS`
(difftest's `network_vectors` test), each relative to the workspace root, and report a class that
is neither installed nor named there and pass. With a local run
(`P2P_VECTORS=target/vectors/local`), `network_vectors` sends each V26 record to the subject of
its area and allows no documented difference other than the unlinked fork segment and the two
reader refusals above. The L1 properties P14 to P16 cover the verifier, the exchange and the
monitor (see L1).

The fuzz targets, seeded by `tools/p2p-fuzz-seeds.js` from a vector run's V24 and V26 records
(`P2P_SEEDS`):

| Target | Input | Checks | Replayed with |
|---|---|---|---|
| `nes_session` | a script of server frames, pauses, client requests and terminates | no panic; the pending requests stay bounded, every failure is a typed error, and every request is answered once the clock has run on | none |
| `reply_check` | a selector byte (modulo 6: the routes other than getStatus), then a reply | no panic; an accepted reply encodes to bytes that decode and check to the same value | `wire.checkReply` |
| `listed_block` | a getBlocks reply | no panic in the download path (the codec, the reply check, the transaction reader and the block rebuilt from the listed fields); a rebuilt block decodes to the same block | `block.fromListed` |

## heartwood-forger

`packages/forger` is the forger: the process that holds the delegates' keys and signs their
blocks, driven by its relays as the reference's forger is. It opens no listening socket and has no
store and no API. Its edges are `heartwood-crypto`, `heartwood-kernel`, `heartwood-p2p-wire` and
`heartwood-p2p` built without its `node` feature, so it links only the nes client session and the
transport; `make layers` checks that its tree holds no crate of the state, the transactions, the
pool, the store, the chain or the API. It is not yet wired into a process.

- **the slot loop**: at every slot, the round from the selected relay; when one of the forger's
  delegates is the slot's forger, attempts one second apart with the reference's checks and log
  lines, the 2,000 ms margin before the slot's end, the forging quorum's reading
  (`isForgingAllowed`), the error count that ends a slot's attempts after `blockTime - 1` failed
  ones, and the forger events. A shutdown lets a `postBlock` in flight finish before the sessions
  close.
- **the relay client**: one nes session per configured host with a 20 MiB receive limit, the
  reference's host selection (up to ten sweeps 250 ms apart, the first ready host in list order),
  each call with its route's codec and the forger's headers, and the two attestation routes on the
  selected host.
- **blocks**: a delegate's block from the relay's inputs (the collated transactions, the previous
  block, the timestamp and the reward), byte for byte the reference's for the same inputs and aux.
- **the signing record**: per key, the height, slot, id and bytes of the last block signed,
  written and synced before the block leaves (`<data>/forger/signing-record.json`, mode 0600).
- **the key file**: `delegates.json`, with the reference's messages for a missing file and a file
  without keys.
- **the attester**: signs the status its relay offers over the forger's own session, one
  signature per offered key, naming the offer by the SHA-256 of its text.

Where Heartwood differs on purpose, none of which an honest relay on the same host shows except the
first (the bytes a slot gets stay the reference's):

- the reference signs a fresh block in every attempt of a slot, before its checks; Heartwood
  checks first, then signs a slot once per key through the signing record, and a retry or a
  restart inside the slot sends the recorded bytes again. The slot the record keeps is the slot of
  the block's own timestamp, whatever slot the relay answers. A slot below the recorded one is
  refused, and so is a slot more than one away from the slot of the forger's own clock, so that
  one far answer from a relay cannot stop a key;
- every relay call has a deadline, where the reference sets none: 20 s for the round, the sync
  and the events; the longer of 4 s and the end of the next slot for the network state, the pool's
  transactions and `postBlock`; the longer of 2 s and the end of the next slot for the status and
  the slot numbers; 15 s and 2 s for the attestation routes;
- a wait the loop computes as zero or less is 200 ms, where the reference polls again at once;
- an over-height header by the delegate's own key stops the forger when its signature verifies;
  the reference verifies it as a whole block built from the header alone, which only an empty
  block passes (the documented difference of L0 and P19 below);
- a key file that the group or others can read is refused, and so is a non-empty `secrets` list;
  the file is never rewritten;
- the attester signs an offer only when its text reads back as a status tree and writes back to
  the same bytes, for the forger's network, with a height equal to its header's and a slot within
  one of the forger's clock, and only for the forger's own keys;
- the fields of an internal body that the forger uses are read when the body arrives, so a missing
  or out-of-range field fails the call, where the reference fails later.

```sh
make test-forger                # clippy and tests of heartwood-forger alone, and of heartwood-p2p without node
make forger-features            # only heartwood-forger enables heartwood-crypto's forger-aux
make forge-replay NAME=txgen-one-node CORPUS=tools/difftest/corpus   # Heartwood rebuilds every block of a corpus
make forger-crash KILLS=50      # the signing record under SIGKILL
make fuzz-forger                # the two forger fuzz targets at once, FUZZ_SECONDS each
cargo test -p heartwood-forger --all-features   # the crate's tests, the loop against two far relays among them
```

The crate's tests run on tokio's paused clock, against a scripted relay over the mock peer's
in-memory connections (loopback addresses `127.10.0.x`) and a recording signer, and read the log
lines the loop writes. They cover every branch and line of the slot check, the attempt and the
quorum's reading, each call's codec, headers and deadline, host selection and failover (a relay
that stops answering is left at its deadline, then through the session's heartbeat), the key
file's modes, the signing record's decisions and write order (a crash between the write and the
send, a torn temporary file), the attester against the status preimages of the signature class
and crafted offers, and blocks against the forging records of V29. Two tests run the loop
against a far relay, one whose round, slots and tip are two slots ahead of the forger's clock and
one whose slot answers name the forger's slot while its round's timestamp is 1,000 slots ahead;
neither gets a block signed, recorded or posted.

The crash test (`difftest forger crash`) starts the `forger-child` binary (feature `difftest`),
which runs the slot loop, the relay client and the signer with a fresh random aux for every
signature, against a mock relay on `127.14.0.1` whose answers follow the clock on a test network
with 3-second slots: the child's delegate forges every slot, every network state names a new
random previous block, and a posted block becomes the relay's last block once its answer is sent,
after a random hold. The harness kills the child with SIGKILL at random moments, most of them
while a `postBlock` answer is held, and starts it again. No slot may have two different blocks at
the relay or in the child's reports, a block posted for a recorded slot must be the recorded one,
and a restart inside a recorded slot that the relay has not taken sends the recorded block again
when it sends at all (with less than 2,000 ms of the slot left it sends nothing).

The fuzz targets:

| Target | Input | Checks | Seeded from |
|---|---|---|---|
| `internal_body` | a selector byte (modulo 9: the nine internal bodies) and the body's JSON text | no panic; an accepted body writes bytes that read back to a body that writes the same bytes | the round and network-state bodies of a vector run's V29 records (`FORGER_SEEDS`) |
| `status_to_sign` | an offer: arbitrary text, or a status tree in canonical form perhaps cut or changed by one byte, and a list of keys | no panic; only an offer whose text is the canonical form of a status for the attester's network, height and slot window, with the attester's own keys, is signed, and every such offer is | none (the target builds its status trees) |

## heartwood-cli

`packages/cli` holds the commands that are not a node process, which the one `heartwood` binary
calls. It depends on `heartwood-crypto` and `heartwood-kernel` only, so it links no node
component, and it starts every child process with an explicit argv, never through a shell. clap
and nix are used by this crate only (`make layers`).

- **the colon aliases**: `relay:start` is `relay start`, for every command built.
- **instances**: a token, a network and a role, named `<token>-<network>-<role>`, with the five
  directories of the `CORE_PATH_*` variables; `config.json` only fills a missing `--token` or
  `--network`.
- **the supervisor** (`heartwood supervise`): pm2's restart rules (a child that ran for 1 second or
  more restarts at once; after 5 shorter runs in a row the role is `errored`), a stop by SIGINT
  and SIGKILL 30 seconds later, a lock per role, a state file, and the child's output captured
  into `<name>-out.log` and `<name>-error.log`, rotated at 500 MB into seven numbered files.
- **the role commands**: `start`, `stop`, `restart`, `status` (with a process table read from
  `/proc`, or from `ps` elsewhere) and `log`, with their refusals.
- **network generate**: a network's chain definition, keys and single-node instance for a seed.
  The built-in devnet profile writes crypto files byte-identical to the genesis vectors (V10) for
  their three seeds.
- **the devnet library** (feature `devnet`, on by default): `init`, `start`, `stop`, `status`,
  `logs`, `stop-node`, `start-node`, `restart-node`, `reset`, `wait` and `env` for a local devnet
  with one loopback address per node (node k on `127.0.0.(10 + k)`, P2P on port 6002 and the API
  on 6003; another prefix lets devnets run side by side), its relays and forgers started through
  the supervisor, and the relays' APIs read with a loopback HTTP GET.

Where the commands differ on purpose from the reference's: the process names carry the network;
a second supervisor of an instance's role refuses to start (its lock); a `config.json` that is not
a JSON object of texts is refused, where the reference ignores it; `status` of a role that never
ran gives a `stopped` row, where the reference aborts; rotated captures are numbered and not
compressed, where pm2-logrotate writes dated gzip files.

```sh
make test-cli                   # clippy and tests of heartwood-cli alone, with no feature, the default ones and all
cargo test -p heartwood-cli --all-features -- --include-ignored   # also devnet init over the real network generate
target/debug/difftest network generate packages/crypto/tests/vectors   # network generate against V10
```

The supervisor's tests run the `test-child` binary (feature `test-child`), whose behaviour each
test scripts; one of them waits the 30 seconds before SIGKILL. The devnet's tests run against
recording fakes of the process operations and of the API reads, on loopback addresses
`127.13.0.x`.

## L0: golden vectors

`difftest run l0` compares the records of a vector run with Heartwood, with one subject per
oracle operation. Where both sides refuse, it also compares the error class wherever Heartwood
models the reference's class. On a run of every class (reference tag `s1-ref-v2`) it reports:

| Outcome | Records |
|---|---|
| compared, equal to the reference | 6,220 |
| documented differences, each with its form checked on the input or shown by the oracle's answer | 161 |
| skipped: builders of types or headers that Heartwood does not build | 6 |

A local run that adds the three classes of the peer-to-peer client (V24 to V26, 1,955 records)
compares 1,952 more records, all equal, and documents 6 more: 3 fork segments whose headers do not
chain, and 1 each of a removed type, a memo that is not valid UTF-8 and an amount beyond 128 bits.

`cargo test` checks the classes of each crate in that crate; the difftest test `l0` runs the
comparison over the committed vectors of `heartwood-crypto` (2,212 compared, 58 documented, 6
skipped), the test `l0_wire` over the wire class of `heartwood-p2p-wire` (1,179 compared, 14
documented, none skipped), `pool_vectors` over the pool class (42 sessions compared, 8
documented) and `config_vectors` over the crypto-file class (17 compared, 79 documented).
A refusal's message is compared as well as its class: the handlers' messages in V17 and every
message of the pool.

Outside the wire class, all but the last of the documented differences are inputs that Heartwood
refuses and the reference accepts:

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
  out and compares everything else;
- crypto files that the strict schema refuses (see heartwood-kernel above): the reference loads
  them, or falls back to its built-in preset when a file is missing. Each refusal kind has its own
  predicate on the input, and a file that the reference refuses must be refused by Heartwood.

In the pool's sessions, a step is compared in full (the lists in order, the errors with their codes
and messages in order, the pool's contents after the step), with these documented cases, each
checked on the entries or shown by the oracle's answer:

- the reason after "Invalid transaction data: " of an entry that does not decode: Heartwood names
  its own decoder's reason, where the reference names its decoder's or its schema validator's; the
  code, the entry id, the place in the lists and the verdict must be equal;
- an entry of a removed type, or a JSON vote given as an array or a string: left out on both sides,
  under any id either side gave it, and the rest of the step must be equal, messages included;
- a full pool that the reference emptied by one eviction for a transaction it then refused (the
  oracle notes each such eviction): Heartwood must refuse the same transaction and agree on every
  entry before it; its pool keeps the transaction the reference evicted, so the rest of the session
  is not compared;
- after the session changed the online record, an entry refused as offline on one side only: the
  reference's view of the sender kept the delegate's wallet as it copied it before the change. The
  rest of the session is not compared;
- a count in a message that the oracle's default locale groups otherwise (the oracle notes its
  locale; in the image and on the maintainers' machines it is English, so no case shows it).

The wire class counts 14 documented differences, each shown by the oracle's answer: 1 status
plugin named `__proto__`, 2 payloads with groups nested 50,000 deep and 11 amounts beyond 128 bits
(see heartwood-p2p-wire above). The pool class counts 8: 5 decode reasons, 2 full-pool evictions
and 1 JSON vote with indices. The crypto-file class counts 79: 75 strict-schema refusals and 4
fee-table values of the wrong type.

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
| P7 handlers | wallet sets of 3 to 12 wallets and a mutated transaction from one of them, under the devnet's milestones or variants of them (fewer active delegates, fees off, and the fee floor, burn minimum and revoke delay changing at the rule height or the height after it) | `state.applyTransaction`: the verdict, the error class, and the wallets after apply and after revert; the revert must also give back the wallet set on each side |
| P8 kernel text | JSON values, `.env` text, address lists | `kernel.stringify`, `kernel.dotenv`, `kernel.ip` |
| P9 digest | wallet sets | `state.digest` |
| P10 pool | fixtures of 3 to 12 wallets, some of them online delegates, with random options (small limits in a quarter of the cases, milestone fees on or off, the static mode), and 1 to 8 steps: batches of 1 to 6 entries built from the fixture's keys and mutated (nonces, fees around the floor and the lowest, amounts around the balance, duplicates, removed types, another network, bad signatures, pending conflicts, bytes or JSON with a correct, wrong or missing id), the clock moving on, removals, re-adds, a restart of the storage, changes of the online record, and a collation at the end | `pool.run`: every step's result and the pool's contents after it |
| P11 text | amounts over the whole `i64` and `u64` range, weighted to `2^53` and to rounding ties; fraction digits; symbols; counts; transactions | `text.formatSatoshi`, `text.toLocaleFixed`, `text.toLocaleString`, `text.pluralise`, `text.transaction` |
| P12 crypto files | the devnet's milestone and network files with keys added at random paths, `null`s, heights repeated, value types changed, removed features turned on, and files removed | `config.load`: a file the reference refuses must be refused by Heartwood; one only Heartwood refuses must match one strict-schema predicate; accepted files must merge to the same milestones |
| P14 verifier | a chain of 70 headers signed by the delegates of their rounds, our height, a peer on our chain or forked from a random height with a branch of its own, bogus and short common-block replies, a failing or unlinked header, a deadline | `verifier.checkState`: the result and every request |
| P15 exchange | the client's requests with random versions, ports and fields; replies of the six routes other than getStatus around the schemas' edges, a quarter of them with a byte changed, cut or added | `p2p.validateRequest`, `wire.checkReply` |
| P16 monitor | peer sets with attested keys, ranks, heights and verification results around the fork threshold, with common heights of different digit counts; lists of heights | `monitor.forkDecision`, `monitor.networkHeight` |
| P17 forging | P10's pool sessions, whose collation at the end is the forger's, with a random previous block, slot, reward (mostly the devnet's, sometimes 0 or any 64-bit value), key and aux | `forger.collateAndForge`: the collated and removed ids and the block's bytes |
| P19 decisions | one attempt of a slot: rounds, network states of every status with quorum counts around the threshold and over-height headers (empty blocks of two delegates, the forging delegate's signed header that counts a transaction, and one of its headers whose signature fails), status headers, slot answers, times left around the 2,000 ms margin, and failing calls | `forger.decide`: the attempt's action, the quorum's reason and every log line |

With the date as the seed and 10,000 cases per property (50,000 for P7), every property agrees
with the reference. The only documented difference that appears is the one of a refused
transaction from a sender without a public key (P7): the reference binds the key before its
checks run and keeps it after the refusal, Heartwood leaves the wallet as it was. The report
lists such cases apart, and the comparison leaves that key out and compares everything else. P7
also counts the reference's error classes it reached; each class of the handlers' checks that a
transaction can reach from a consistent wallet set appears.

P10, P11 and P12 run 10,000 cases each with the date as the seed (P10 also 20,000). P11 agrees with
the reference on every case. In P12 about half the files are ones that only Heartwood refuses, each
matching exactly one strict-schema predicate, and every file the reference refuses Heartwood
refuses too. In P10 about a third of the sessions compare in full; the others show one of the pool's
documented differences (see L0), most often an entry that does not decode, which the generator puts
in most batches. P10 reaches every error code the reference's pool can give, each about 200 times
or more per 10,000 cases, apart from two that it cannot give: `ERR_WRONG_NETWORK` (its schema refuses
another network byte first) and `ERR_RETRY` (a failed revert empties the sender's mempool). The
report also counts the reference's refusals with its `ColdWalletError` text; there are none, since
the reference binds a sender's key before its checks run, so the pool cannot reach that error.

P14 to P16 agree with the reference at 10,000 cases each with the date as the seed. The only
documented difference among them is P14's fork segment whose headers do not chain (about one case
in twenty-four), which Heartwood refuses (see heartwood-p2p). P17 and P19 agree with the reference
at 1,000 cases each with the date as the seed. P17's documented cases are the pool's full-pool
eviction (about one in seventy). P19's are the over-height header of the delegate's own key that
is not an empty block and whose signature verifies (about one in seventeen), on which Heartwood
does not forge; the report counts the reference's actions, and every action of an attempt and
every reason of the quorum's refusal appears.

## L2: chains, corpora and the state digest

A corpus is a chain that a reference devnet produced, imported with `difftest corpus import` from
the archives of the maintainers' devnet tooling (a separate tool that is not published): the chain
definition, every block as a relay receives it, and what the node itself recorded (round rows,
missed slots, block columns, API checkpoints). The oracle's
judge, `replay-chain.js` (`difftest corpus judge`), replays the corpus in the reference's own state
classes with storage replaced by memory, checks that replay against the node's records, follows a
seeded plan of reverts and re-applies, and writes one digest line per step. `difftest run l2`
replays the same steps in Heartwood and compares every line; it also checks, before each block,
that the scheduled forger is the block's generator and that the reward in its round is the header's,
and runs the supply check after every step, with a check of the one value the digest leaves out:
each delegate's resignation height must be that of its latest resignation in the chain. `--synthetic` adds blocks that the harness signs with
the devnet's derived keys on top of the chain (refused blocks, a decoding rule and the handlers'
fee floor, burn minimum and revoke delay changing at a height, a vote at a round's end, burns and
fees at their limits, a block by a key that is no delegate, and a competing chain after a revert
across a round end) and sends them to both sides.

Heartwood's replay goes through its store, as a node does: the state after the first start is
read from the store, and every applied or reverted block is committed with the chunk writer, one
block per chunk (and in a second run, chunks of 100 blocks). At the end the store's rows are
compared with the rows the reference node wrote to its own database: the round rows of every round
in the order of the reference's `getRound`, the missed slots, and each block's username, burned fee
and donations.

**Restarts.** `make corpus-judge RESTARTS=every` (or `rounds`) writes a restart plan into the
corpus: the heights at which the replay restarts, and after a restart at a round end a revert of
one block and of 54 blocks and their re-apply. The judge restarts the reference at each planned
height by its two start paths, headless: its rebuild (`StateBuilder` from the stored rows and
`RoundState.restore`) and a start from its saved state (`StateSaver` on the running node, then
`StateLoader`), and writes their lines to `ts-restarts.jsonl`. `make l2-restart` replays the chain
once per start path of Heartwood (the state tables, and the replay from the genesis block), closing
and opening the store at each planned height. A restart line must equal the continuous line of its
height in the last block, the supply, the round and the wallets, and the reference's lines of both
paths; every later line (applies, reverts, re-applies) must equal the continuous line of its step,
and each delegate's resignation height must still be that of its latest resignation.

**Synthetic cases.** `--synthetic` also runs restarts and pool sessions on top of the chain, on both
sides. The restarts: a revoke one block after a restart; a resignation, a restart 50 blocks later
and revokes at the delay minus one, the delay and the delay plus one; a resignation, a revoke and a
registration in one round with a restart in it; restarts at a round end and at the next round's
first block with equal vote balances; delegates named `0x10`, `0b101` and `1e5` (the reference's
saved state reads such names as numbers, which is a documented difference of that path only); a
stored transfer raised above its sender's balance, on which both refuse to start without a saved
state, with "State Generation failed: Wallet with negative balance"; a restart followed by a
revert across a round end and a competing chain; and, twice, a revert to the last block of a round,
the next round's first blocks applied again and a restart in that round: at the chain's tip, where
the store holds missed slots, and at the last round end below the first missed slot, where it holds
none. In those two the reference's starts read no delegate for the round (see heartwood-blockchain)
and show it as the oracle notes it: an empty forging list at the tip, and the chain rolled back
below it; Heartwood's starts give the state before the restart. Both of the reference's start
paths begin from the same store. Each Heartwood restart must also give back each delegate's
resignation height and each wallet's vote shares, which the digest leaves out. The pool
sessions: transfers from three senders collated, forged by the scheduled delegate and applied, then
the synced node's steps (remove the forged transactions, clean up, add the rest back); a competing
transaction for a pending nonce forged in a block; a revert of a block with pool transactions and
their re-add; a vote before and after its delegate is online, and the online record's expiry five
rounds later; a milestone that raises the fee floor at the tip plus two; and a restart of the pool's
storage.

**Forge replay.** `make forge-replay` rebuilds every block of a corpus with Heartwood's forger from
the block's inputs (its transactions, the previous block, the timestamp and the reward) and the
forger aux of the chain: `forgerAux` in `corpus.json` when present, else the one the devnet
tooling derives from the chain's seed. Each rebuilt block must equal the stored bytes; a corpus
whose blocks do not rebuild is reported as "aux unknown", never as passed. With `DIRECTION=oracle`
the reference rebuilds the blocks instead, which checks a chain that Heartwood forged. The
committed corpus and the maintainers' fault and restart chains (to height 265) rebuild in both
directions.

**Kill test.** `make crash-test` replays the chain through the store in a child process
(`difftest store replay`) and kills it with SIGKILL after a random delay, `KILLS` times; after each
kill it restores the store by both of Heartwood's start paths (`difftest store check`), which must
land on the last or the next committed block with a state whose digest equals the continuous line of
that height, and goes on from there.

```sh
make corpus-import ARCHIVE='archive/...' NAME=my-chain   # into target/corpus/my-chain
make corpus-judge NAME=my-chain RESTARTS=every            # the judge's digest lines, checks and restart lines
make l2 NAME=my-chain                                     # Heartwood through its store against those lines
make l2-restart NAME=my-chain                             # the same with the restarts of the plan
make l2-synthetic NAME=my-chain                           # with the synthetic blocks, restarts and pool sessions (needs the oracle)
make crash-test NAME=my-chain KILLS=200                   # the store after SIGKILLs
make forge-replay NAME=my-chain                           # every block rebuilt by the forger
cargo test -p heartwood-difftest --locked --test l2       # the committed corpus with its restart lines, no oracle
```

The committed corpus, `tools/difftest/corpus/txgen-one-node`, is a one-node devnet chain of five
rounds with every kept transaction type and one live restart of the node, with the judge's lines
and a restart line of both reference paths at every height; its README describes it. The
maintainers also replay chains made with the same unpublished tooling, which are imported under
`target/corpus` and are not committed: three nodes with partitions, kills and restarts (replayed up
to the end of its fifth round), and a one-node chain of five rounds restarted once from its saved
state and once without it, with an API checkpoint after each restart, which the judge's headless
restarts must reproduce.

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
| `V19-fee-floor.jsonl` | the dynamic fee floor of the six types over memo lengths, recipients, votes of 1 to 53 entries and resignation types; fee tables switched off, with `minFee` 0 and negative, and without a type's key; tables and revoke delays of the wrong type |
| `V22-text.jsonl` | the number formats of the messages: `formatSatoshi` of 0, 1, 5, `10^8`, `10^8 + 1`, every power of ten, amounts near `2^53`, above it, at `2^63 - 1` and `2^64 - 1`, negatives; `toLocaleString` of doubles at rounding ties with 0 to 8 fraction digits and of integers; a transaction's short text for the six types, nonces up to `2^63 - 1` and the versions |
| `V08-ranking.jsonl` (state) | the delegate ranking over generated sets: equal vote balances broken by public key, resignations, 53 to 80 delegates, rounds larger than the ranking, identical keys refused; the key order over random key pairs, every first byte and keys that first differ at each byte |
| `V09-rounds.jsonl` (kernel) | the round of a height and whether it starts one, for heights 1 to 10,000, ranges hashed, 1 to 53 delegates and heights near 2^32; the forger index, next forger, block timestamp and `canForge` over slot edges, half slots, gaps and times before the epoch; block chaining (previous id, height plus one, a later slot) |
| `V09-shuffle.jsonl` (state) | the forging order: for every count 1 to 53 the SHA-256 of the orders of rounds 1 to 10,000; full orders of rounds 1 to 100 for counts 1, 2, 3, 52 and 53; rounds where a draw is rejected; round 2^53 - 1 |
| `V13-p2p-wire.jsonl` (p2p-wire) | nes frames, protobuf messages, every route codec both ways, and signed and received peer statuses, each record with its own network configuration (see heartwood-p2p-wire above) |
| `V15-vote-amounts.jsonl` (state) | a voter's balance split over 1 to 53 delegates, for balances 0, 1, 101, near 2^63 and random, the remainder in map order |
| `V16-kernel.jsonl` (kernel) | the status canonical JSON; `.env` text (separators, comments, quotes, digits, booleans, empty values); epoch timestamps; address lists with globs, brackets and IPv6; node directories with their environment overrides; `pluralise` over words and counts 0, 1, 2, 999, 1,000 and 2,000,000 |
| `V17-handlers.jsonl` (transactions) | the transaction handlers on wallet sets: at least one case of each error class and of each apply effect, the revoke delay at 105, 106 and 107 blocks, the fee floor and one below it, the burn minimum and one below it, votes identical, empty, too many and unknown, second signatures missing, wrong and unexpected, and a refused first transaction of a wallet without a key; each refusal with its message |
| `V18-genesis-state.jsonl` (state) | the state digest after the genesis block of the three V10 chain definitions and of six variants of the first (fees and burns, a second key, resignations and a revoke, split votes, a transfer before the issuance record, more issuance records, a generator that registers and votes, a vote and a resignation before their registration; two that the reference refuses to start on: a negative balance, too few delegates for round 1), and of five wallet sets, one per record shape |
| `V24-p2p-exchange.jsonl` (not installed) | the client's requests as a reference server reads them (accepted with the reference client's bytes, or refused by the version gate or the payload limit), and the reply checks of the six routes other than getStatus, with their verdicts and changed values |
| `V25-peer-rules.jsonl` (not installed) | the address rules of candidate peers, the version gate over the devnet's, the default and 400 seeded ranges, the same-subnet test and the outgoing rate limiter's waits |
| `V29-forging.jsonl` (not installed) | block assembly (`forger.forge`: an empty block, each kept type alone and together, burns, 256 recipients, 150 and 151 transactions, a block over `maxPayload`, totals and rewards at the 64-bit edges, three aux values, milestones that change at the next height), collation and assembly after pool sessions (`forger.collateAndForge`), one attempt of the forger's slot (`forger.decide`, with its log lines), and the internal routes' round and network-state bodies as a relay writes them and the forger reads them |
| `V26-network.jsonl` (not installed) | the status attestations of a ping, the n-ary search for the common block and the peer verification over forks of a devnet chain, the monitor's network height, fork decisions and download plans, and blocks rebuilt from getBlocks replies |
| `V20-logger.jsonl` (logger) | the log line of every level, coloured and plain, with and without emoji codes (the whole table, in slices), over times at the edges of a JavaScript date and messages with line breaks and colour codes |
| `V21-pool.jsonl` (pool) | 50 scripted pool sessions over wallet fixtures, each step with the reference's answer and the pool's contents after it: every error code with its message (duplicates, the cooldown from 0 to 30 seconds and per channel, the size limit, another network byte, the fee floor and one below it, the static fee exact, lower and higher, a full pool with fee ties and evictions, the per-sender limit with `excess` and allowed senders, every handler refusal, the pending and offline texts, bad signatures and bytes that do not decode, the seven removed types as bytes and as JSON); JSON entries with a correct, wrong or missing id; accepted but not broadcast; a pending incoming transfer; nonce chains and gaps; re-adds after a flush and a restart of the storage; age expiry; removal cascades; forged removal; the online record's expiry; collation (fee and nonce order, ties, `exclude`, the most transactions, the byte budget, a failed sender) |
| `V23-config.jsonl` (kernel) | `config.load` of the four crypto files: the devnet's files, accepted by both, and each refusal kind of the strict schema at each level, the keys of removed features on and off, `exceptions.json` other than `{}`, and missing files (the reference then falls back to its built-in preset) |

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
difftest run l2 DIR [--synthetic] [--restart] [--memory] [--to H] [ORACLE]
difftest run l3|l4              says that the level is not built yet
difftest corpus import ARCHIVE... --out DIR [--seed TEXT]
difftest corpus judge DIR [--restarts every|rounds] [--restart-plan FILE] [--to H] [--dump H,...] [ORACLE]
difftest corpus verify DIR
difftest store replay DIR --db PATH [--to H]
difftest store check DIR --db PATH
difftest store crash DIR [--kills N] [--seed S] [--max-delay MS]
difftest levels
difftest oracle replay OP CORPUS --vectors DIR [--height N] [--milestones FILE] [ORACLE]
difftest genesis --profile FILE --seed TEXT --epoch TIME --out DIR [--force] [--compare DIR]
```

`ORACLE` is `--image TAG` or `--solar-dir DIR`; `difftest --help` lists every option. `run l2
--memory` replays without a store, as before the store existed; `store replay` goes on replaying a
corpus through the store under `PATH`, one block per chunk, and `store check` restores it by both
start paths and prints each one's tip and restart line: the kill test's two halves.

Library modules: `oracle` (run the scripts in the image or locally; `OracleServer` for L1),
`vectors` (files, meta records, manifest), `subject` (a Rust implementation of one oracle
operation, shared by L0 and L1, its verdict, and the registry), `subjects` (the operations answered
with the node's crates, with the reference's error class of a refusal where it is observable, and
each documented difference checked on the input), `compare` (a JSON diff that names the
path and, for hex values, the first differing byte with the bytes around it; numbers compare by
their double value and object keys by their JavaScript order), `report`, `replay` (a corpus of
inputs through the oracle and Heartwood), `genesis` (the chain-definition driver), `fixture`
(wallet sets as both sides read them), `strategies` (the L1 generators), `corpus` and `digests`
(L2 corpora and digest files), `synthetic` and `synthetic_pool` (the L2 synthetic blocks,
restarts and pool sessions), `restart` (restart plans and the reference's restart lines), `store`
(the replay through the store, the store's rows against the node's, and the kill test) and `levels`
(L0, L1 and L2 run, with `levels::pool`, the judge of a pool session; L3 and L4 are described with
what each one arrives with).

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

Heartwood Core is licensed under the [Apache License, Version 2.0](../LICENSE). [NOTICE](../NOTICE) lists the projects Heartwood Core credits; code adapted from ARK Core keeps its MIT notice.
