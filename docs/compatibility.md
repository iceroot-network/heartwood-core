# Heartwood compatibility and testing

## How Heartwood is tested

The reference implementation is Solar Core 4.3.1 with a patch series, kept in
[iceroot-network/solar-core-ref](https://github.com/iceroot-network/solar-core-ref). The tests use
its tag `s1-ref-v2` (29 patches; `s1-ref-v1` holds the first 18). Heartwood must behave exactly like it, and differential testing shows that it
does: the same inputs go to the reference (the oracle) and to Heartwood, and the results must be
equal byte for byte, including which inputs each side rejects.

| Level | What is compared | Status |
|---|---|---|
| L0 | golden vectors written by the reference, against Heartwood | runs against the ten crates with vector classes: 8,643 of 8,820 records compared, all equal, the other 177 documented differences (see below) |
| L1 | random and mutated cases, one oracle call each | runs: nineteen properties, from the fee floor to the pool, the crypto files, the peer-to-peer client, the block processor, the forger's blocks and decisions and the relay's forging quorum (see below) |
| L2 | block sequences replayed on both sides, a state digest per block | runs: chains of reference devnets through Heartwood's store and through its block processor and jobs, with seeded reverts, restarts at every height, synthetic blocks, pool sessions and kills of a running relay, and every block of those chains rebuilt by the forger (see below) |
| L3 | one scenario, observed through the node under test | runs: a scenario on a reference devnet whose observer is a reference relay, then the same with a Heartwood relay in its place; a devnet of Heartwood nodes only joins with the forger (see below) |
| L4 | a mixed network of reference and Heartwood nodes | runs: reference devnets with Heartwood relays that restart by every path, sync from an empty store, and follow a partition and its heal (see below) |

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
packages/blockchain/  heartwood-blockchain: the block processor, jobs and their commit to the
                      store, the node thread, the job queue, the state machine with fork
                      recovery, and a node's start (first start, restart from the state tables,
                      replay from genesis)
packages/p2p/         heartwood-p2p: the client side of the peer-to-peer protocol (nes sessions,
                      requests and reply checks, peer acceptance and verification, the network
                      monitor) and, with its feature `node`, the server (the listener, the ban
                      hammer, the inbound limits, the controllers and the forger's status
                      attestation); the mock peer behind `difftest`
packages/forger/      heartwood-forger: the forger (the slot loop its relay drives, the relay
                      client, block assembly, the signing record, the key file and the status
                      attester)
packages/cli/         heartwood-cli: the commands' library (the colon aliases, instances, the
                      supervisor and the role commands, `network generate`'s files, the local
                      devnet)
packages/api/         heartwood-api: the public API a relay serves (routes, query grammar,
                      pagination, headers, rate limit, semaphore)
packages/core/        heartwood-core: the `heartwood` binary, the relay and forger roles with
                      their configuration, process lock and signals, `network generate` and
                      `devnet`
packages/*/tests/vectors/  each crate's golden vectors, with its own MANIFEST.sha256
tools/difftest/       heartwood-difftest: the harness library and the `difftest` binary
  corpus/               a committed L2 corpus (a reference devnet chain and its digest lines)
  api-corpus/           the route corpus: the queries of every served route, for L3 and L4
tools/check-layers.sh the dependency edges between the crates, and no async, storage or HTTP crate
                      in the consensus crates
tools/check-lines.sh  code lines per crate against the review ceilings
tools/test-integration.sh  runs every test marked `integration`, by name
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
tools/server-fuzz-seeds.js  seeds the server's nes_server_session corpus from V31's requests
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
make check          # cargo build, clippy (warnings are errors), fmt --check, test, the integration
                    # tests, release check, layers, rustdoc (warnings are errors), line ceilings
make layers lines   # crate edges and code lines per crate
make doc            # rustdoc of the workspace with every feature, warnings are errors
make test-integration  # the tests marked `integration` in every crate
make oracle-image   # build heartwood-oracle:<commit> from scratch (a few minutes)
make e2e            # vectors in the image and locally, diff the two runs, run L0
make vectors-install  # copy the image's vectors into each crate's tests/vectors
make test-oracle    # the oracle answers, and 256 L1 cases per property, in the image and locally
make l1             # the L1 properties against the oracle
make l2 NAME=txgen-one-node CORPUS=tools/difftest/corpus   # replay a corpus against its digest lines
make l2-restart NAME=txgen-one-node CORPUS=tools/difftest/corpus   # the same with restarts
make crash-test NAME=txgen-one-node CORPUS=tools/difftest/corpus   # the store after SIGKILLs
make l2-processor NAME=txgen-one-node CORPUS=tools/difftest/corpus # the same chain through the processor
make pool-bench     # the cost of the pool's per-sender views and of the store's commit
make relay-binary   # the relay's release binary with the harness's flags
make live RUN=h     # a live run on a devnet, then make live-check RUN=h and make api-compare RUN=h
```

`make check` needs only Rust. It lints and tests every crate on its own, with no features and
with all of them, so that Cargo does not unify the crates' features with those difftest turns on.
Its tests include each crate's own vector classes, the L0 comparison of the committed vectors of
`heartwood-crypto` and of the wire and exchange classes of `heartwood-p2p-wire`, the L2 replay of
the committed corpus through the store and through the processor; it also checks and formats the
fuzz targets (their own package, `fuzz/`). A test marked `#[ignore = "integration"]` needs more
than its crate's unit (the served API over a store, a relay importing from a mock peer, the
harness's built binaries); `make test-integration` (`tools/test-integration.sh`) finds every such
marker and runs those tests by name, and fails when a marked test is not found, so `make check`
runs them all. It also counts code lines with `tokei` when it is installed (CI installs it), so the
crates' ceilings hold there too.
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
| `L1_CASES`, `L1_SEED`, `L1_PROPERTY` | `256`, `1`, all | cases per property, the generator's seed, the properties (`P1` to `P19`, or their names, comma separated) |
| `L1_MIN_REACH` | 20 from 1,000 cases, else none | how often P13 must reach each reachable result of the processor and each unchained status |
| `CORPUS`, `NAME` | `target/corpus`, empty | the L2 corpus `CORPUS/NAME` |
| `ARCHIVE`, `CORPUS_SEED`, `DUMP` | empty | the devnet archives `make corpus-import` joins (in chain order); the chain's public key-derivation seed; heights whose records the judge writes |
| `TO` | empty | cut the corpus at this height, in `corpus-judge`, `l2`, `l2-synthetic` and `l2-restart` (the judge and the replays must use the same cut) |
| `RESTARTS` | empty | the restart plan `corpus-judge` writes into the corpus before it judges: `every` (a restart at every height) or `rounds` (the round ends, the first blocks of rounds, 20 seeded heights and the node's live restarts) |
| `KILLS`, `KILL_SEED` | `200`, empty | the number of kills of the kill test and of the forger's crash test, and the seed of their delays |
| `DIRECTION` | empty | `oracle` makes `forge-replay` have the reference rebuild the corpus's blocks; empty, Heartwood rebuilds them |
| `PROCESSOR` | empty | any value makes `corpus-judge` also record each block's result through the reference's job, one block per job (`ts-processor.jsonl`) |
| `RUNTIME` | empty | any value makes `crash-test` kill a relay that imports the corpus from a mock peer, instead of the store's replay |
| `CHUNKS` | empty | the one job cut `l2-processor` runs (`reference` or `1`); both when empty |
| `STRICT` | `--strict` | the comparing targets fail a run in which a case was skipped or had no Rust implementation; `STRICT=` runs them leniently, for a look at a partial tree |
| `ORACLE_ARGS` | `--image $(IMAGE)` | where the oracle of `l1`, `corpus-judge`, `l2-synthetic` and `replay-wire` runs; `--solar-dir DIR` for a local reference |
| `RELEASE_PACKAGES` | the ten library crates | the packages `make release-check` builds without test seams |
| `FUZZ_SECONDS`, `FUZZ_FORK` | `600`, `8` | how long each wire or p2p fuzz target runs, and its libFuzzer jobs |
| `WIRE_SEEDS`, `WIRE_SEED_MAX` | the installed wire class, `65536` | the vector file that seeds the wire fuzz corpora, and the longest seed in bytes |
| `WIRE_LOGS` | `target/wire-fuzz` | the logs of `make fuzz-wire` and `make replay-wire` |
| `P2P_SEEDS`, `P2P_LOGS` | `$(VECTORS)/local`, `target/p2p-fuzz` | the vector run that seeds the p2p fuzz corpora, and the logs of `make fuzz-p2p` and `make replay-p2p` |
| `FORGER_SEEDS`, `FORGER_LOGS` | `$(VECTORS)/local`, `target/forger-fuzz` | the vector run that seeds the forger's internal_body corpus, and the logs of `make fuzz-forger` |
| `LIVE_RECIPE`, `RELAY_IMAGE_RECIPE`, `LIVE_RESULTS` | empty | the devnet tooling's live recipe, its recipe of the relay's container image, and the directory of the live runs (a separate tool of the maintainers, which is not published) |
| `DEVNET_RECIPE` | empty | the devnet tooling's recipe of the forging devnets' runs (not published either) |
| `RUN` | empty | the live run: `r`, `h`, `m1` or `m2` (see L3 and L4) with `LIVE_RECIPE`, or `t`, `m` or `f`, the runs of a forging devnet, with `DEVNET_RECIPE` |
| `LEFT`, `RIGHT`, `API_COMPARE_OUT` | empty, empty, `target/api-compare` | two nodes' API URLs for a direct `api-compare`, and where it writes their answers |

A crate's vector tests, and the harness's tests of its classes (`tools/difftest/tests`), read
the classes from the crate's own `tests/vectors`; there the harness's tests fail when a class is
missing. `WIRE_VECTORS` (`heartwood-p2p-wire`), `P2P_VECTORS` (`heartwood-p2p` and difftest's
`network_vectors`) and `NODE_VECTORS` (difftest's tests of V27 and V28) name another directory,
such as a local oracle run (`target/vectors/local`), relative to the workspace root; only such a
directory may lack a class. `DIFFTEST_IMAGE`, or `SOLAR_DIR` for a local reference, is the oracle a
`difftest` command uses when it is given neither `--image` nor `--solar-dir`.

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

The rest of a node's configuration has the reference's files and semantics, so one configuration
serves both implementations:

- `config::env`: the node's environment, the process's own variables, then the keys of
  `<config>/.env` the process does not have, then every key of `~/.<token>/.env` over both. The
  reference refuses to start without the home file; a Heartwood relay does not need it. A switch is
  on for any non-empty value, as the reference reads `!!process.env.X`.
- `config::app`: `app.json`, with its three profiles checked as the reference's schema checks
  them, and each component's options merged over its defaults (arrays replaced) and read as the
  reference's option schemas read them (numbers and booleans from text). A profile's `flags` and an
  `app.js` beside the file are not read; the relay names them in a warning at its start.
- `config::peers`: `peers.json`, the seed list and the peer sources, which must be URIs, as the
  reference's schema requires, so a local file cannot be a source on either side.
- `config::floor`: the node's own version against the milestones' `p2p.minimumVersions`. The floor
  in force at the stored tip (height 1 at a first start) must admit the node's version, or the
  start stops and names the height; a later milestone whose floor would refuse the node is a
  warning with its height, since the network cuts the node off there. The reference checks no
  floor against its own version.
- `bus`: the event bus, with the reference's event names, to every subscriber in the order the
  node emitted them. Nothing in a relay consumes it beyond the node's own ordered listeners; the
  external payloads (webhooks, the API's socket) come with their first consumer.

```sh
cargo test -p heartwood-kernel --locked --all-features   # unit tests, the loader's refusals, V09-rounds, V16 and V23
```

The class V28, installed beside the kernel and the API, holds the reference's reading of
`app.json` (`config.loadApp`) and of the components' options (`config.pluginOptions`); difftest's
`config_app_vectors` test compares them.

## heartwood-logger

`packages/logger` writes log lines as the reference's pretty printer does: the time, the level
name with its colour and the message, with emoji codes such as `:white_check_mark:` replaced
(the table is node-emoji's, with its MIT notice in `src/emoji.tsv`). Console lines are coloured,
file lines are not; the file rotates daily into gzip files and keeps at most 10 of them and
100 MiB. The relay calls `init` with its log options (`CORE_LOG_LEVEL`, `CORE_LOG_LEVEL_FILE`); a
level name that is not one of the reference's eight turns that output off, as the reference drops
it.

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
reference stores `NULL`; the API's criteria and orders read the amount of a burn only, as the
reference's table holds it (see heartwood-api). Schema version 2 adds `transaction_recipients`,
every recipient of a stored transfer, which the API's address criteria read as the reference's
containment query on the transfers finds them; a file of version 1 is upgraded when it opens. A
file of another schema version, or of another program, is refused.

Writes go in chunks: one SQLite transaction per chunk of blocks, with a savepoint per block, so a
chunk is stored whole or not at all. On every start the store checks itself: the block count is
the tip's height; the transaction count, fee sum and amount sum agree with the block columns; no
round rows lie above the next round; there is one undo record per height from 2 to the tip; and the
state row names the tip. The crate stores rows, not state: it does not depend on
`heartwood-state`. When the writer closes, it checkpoints the write-ahead log into the database
file (a passive checkpoint, which never waits for a reader), so after a stop the file alone holds
every committed block.

The API reads through `Readers`: read-only connections of its own, never the writer, so a read
never waits for a chunk and never sees half of one. Each interrupts a statement that runs longer
than the database's `apiConnectionTimeout` (3 s by default), as the reference's API connection has
that statement timeout. `history` holds the queries of the served routes: blocks by id or height,
the transactions of the reference's criteria (`address`, `senderId`, `recipientId`, `id` as a
prefix, `memo` as a `LIKE` pattern on its bytes, `asset` as a JSON containment after the
reference's expansion of the query's strings, and the rest), with the reference's orders and
pages, and a round's rows. A listing counts every match and reads its page in one read
transaction. Ties of the requested order are broken by the block height and the sequence, so a page
is the same on every call; text sorts by its bytes, which is the reference's order on a database
in the C locale. The pattern, the containment and the `jsonb` order were checked against
PostgreSQL 14 in the C locale.

```sh
cargo test -p heartwood-database --locked   # row round trips, chunks, reverts, the start checks, killed commits, the history queries
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
and it reads the time only through the kernel's clock seam. A processor also returns the admitted
transactions that pass the broadcast fee rule, cloned when admitted, so a transaction that a later
entry of the same batch evicts is still broadcast, as the reference's list is. The view a relay
publishes after each change shares the transactions it already held with the view before it.

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

`packages/blockchain` is the blockchain component: it stores the chain and restores the state from
it, processes blocks as the reference's processor does, and runs a relay's follower: the node
thread, the job queue, the state machine and the blockchain service around them.

### Storage and the start

A chunk writer commits each applied or reverted block in the chunk's one transaction: the
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

### The processor and the jobs

The processor judges a block in the reference's order: its verification at the current time (its
transactions strictly decoded when the block is rebuilt), the senders' nonces, the generator (the
delegate of the block's key, registered, ranked, and the slot's delegate in the round's list),
then the chaining to the tip (the unchained statuses: not ready for the next height, already in
the chain, equal to the last block, a generator mismatch, double forging, an invalid timestamp),
and only then the refusal of an invalid generator, the reward of the round, two transactions of
one unique type from one sender, a transaction id already stored or in an earlier block of the
job, and the apply. The outcomes are the reference's
(`Accepted`, `DiscardedButCanBeBroadcasted`, `Rejected`, `Rollback`), each with the reference's
side effects on the chain state: every refusal resets the last downloaded block, and an unchained
block also clears the queue.

A job is the reference's `ProcessBlocksJob`, run on the node thread. A job that starts at or below
the tip does nothing; one whose first block does not chain to the tip ends with a fork check. Each
block is rebuilt as the reference's block factory rebuilds a downloaded block and processed with
the delegate name it was given when it was enqueued; the accepted blocks are committed in one
SQLite transaction, each block's rows written right after its apply, and the job ends as the
reference's does (broadcast, fork, or a cleared queue). After the commit the node publishes its
snapshots (the state, the pool, the chain status) and its events, never between two blocks.

The blockchain service enqueues as the reference does: every block of a download, and every
incoming block, is named first (`setBlockUsername`) from the state the node published at its last
commit, then the download is cut into jobs after 150 transactions, 100 blocks, or at a milestone
height. So a block whose delegate registered in an earlier block of the same download has no name:
its generator is refused (`Rejected`), the queue is cleared, and the block is accepted when it is
downloaded again, as in the reference.

### The node thread and the state machine

The node thread alone holds the state, the store, the pool and the online record, and runs their
work in the order it arrives: jobs, pool batches, the queue's drain, block removals, changes of the
online record and the stop. After each commit it runs the pool's steps (the online record's expiry
at a round start, the re-add when a milestone starts, the clean-up), in the reference's order. The
network and the API read the published snapshots and never wait for the thread, except for a
submission's answer. A job locks the chain state it shares with the service for one block at a
time, so the peer client sees each applied block as the tip. The chain state (the last blocks,
the last downloaded block, the ping, and the rounds a running job has started) moves block by
block while a job runs; the store and the published snapshots (the state, the pool and the chain
status) move only at the job's commit. A round a block starts is recorded in the chain state with
the tip that the same block moved, so the peer client's round lookup, which reads it there first,
never pairs a new tip with an old round. After the commit the store answers. The published state
answers only for its own round; any other round that is neither started nor stored is a lookup
error, as the reference asserts before its fallback.

The state machine has the reference's states, events and thirteen entry actions: the start
(`initialise`: the pool re-added from its storage, the online record loaded, the network monitor
booted), the sync (`checkLastDownloadedBlockSynced`, `downloadBlocks` and the rest), the idle loops
(a download from the tip every 500 ms, the peers' pool transactions every 4 s, the seeds again
below the minimum network reach every second, discovery every 30 s), the wake-up after 60 s without
an accepted block, and fork recovery (the queue stopped, the forked blocks removed, a random depth
of 4 to 102 blocks when the fork sets none, the queue resumed and the peers refreshed). A start
has no last downloaded block, so the first sync after every start downloads once from the tip, as
the reference's does. The sync's downloads take the configured `getBlocksTimeout`; the idle
download keeps its 2 seconds.

### Where the runtime differs

Each of these shows only with a storage failure, a crash or a crafted block, or only in the timing
of a step; none changes a verdict or a digest on an honest chain:

- a job is one SQLite transaction: a failed write or commit stops the node, and its next start
  restores the committed tip. The reference reverts the job's accepted blocks, adds their
  transactions back to the pool and goes on. A pool storage failure while the forged transactions
  are removed stops the node after the job's commit, where the reference reverts that block;
- a block refused in its apply leaves no missed-slot rows: they are written only with an applied
  block, in its commit. The reference writes them before it validates the block;
- apply and revert read only the state and its undo records, never the store, so no result
  depends on how far the store lags a multi-block job;
- the reference names blocks from its live wallets, which a running job changes block by block;
  Heartwood names them from the state of the last commit. When a download ends while a job that
  applies a delegate's registration runs, the reference may name that delegate's blocks, and
  Heartwood names them once the job has committed;
- an unchained block's clear of the queue takes effect when the job ends, not inside the handler,
  so a block enqueued during that job is cleared with the rest; the reference keeps it when the job
  ends discarded;
- the pool is cleaned up once per job at the committed tip, where the reference cleans up after
  each applied block. The same transactions leave, since every addition already keeps the pool
  within its limit; only the removal log lines come at the job's end;
- a SIGINT or SIGTERM finishes or rolls back the current job, flushes the pool, saves the online
  record and closes the store before the process exits with 0; the reference exits at once;
- a fork check whose first unchained block is at height 2 asks no peer for the block at height 1
  (a request a reference server refuses with a ban); the chain's outcome is the same;
- the events the reference also puts on its bus for plugins (`block.received`,
  `block.disregarded`, `state.started`) have no listener in a relay, and two log lines of the idle
  loop are not written: the launch countdown is logged once instead of every slot, and the line
  that says a delegate's reward was reduced.

```sh
cargo test -p heartwood-blockchain --locked   # commit and restart at every height of the committed chain, the start's refusals,
                                              # the milestone check, the processor, jobs, the node thread, the machine, sync and fork recovery
```

The class V27, installed beside the crate, holds the reference's processor (`chain.process`, one
block on a chain state) and its job (`chain.job`, a download of one or more jobs); difftest's
`processor_vectors` and `job_vectors` tests compare them, including two records that put a
delegate's registration and its first block in one download.

## heartwood-p2p-wire

`packages/p2p-wire` is the codec of the peer-to-peer port. It writes and reads the bytes that
nodes exchange exactly as the reference does, including how the reference reads malformed bytes,
so that both implementations accept the same payloads and decode them to the same values. It has
no I/O and no clock: the nes sessions (hello, pings, heartbeats, request ids) and the WebSocket
transport are in `heartwood-p2p`, which uses the codec.

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
- **requests and replies**: the requests a node's client writes (`ClientRequest`), whose typed
  inputs cannot take a shape a reference server refuses or bans for (headers without a version, a
  field the encoder would ignore for a sibling), and the reply schemas of the six routes other than
  getStatus, with the block-header rules they share with the getStatus reply check.
- **signed peer status**: the tree a relay signs over its last block and its configuration, its
  canonical JSON, the slot before the network epoch (negative in the signed text, wrapped on the
  wire), the attestations (legacy Schnorr over the text, one per key; a key listed twice is signed
  twice), and what a node does with a status it receives: the reply check with the changes it
  makes to the value (plugin names outside 4 to 64 characters removed, the generator key
  lowercased), the text the sender signed, and the signature checks. Only the reply check makes a
  `CheckedStatus`, the one input the signed text is rebuilt from, so an unchecked reply cannot
  reach the signature checks.

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
keeps its peer list, with the reference's requests, timeouts, limits, cadences and decisions. The
client opens no listening socket; the server, behind the crate's `node` feature, is the relay's
listener. It reads the node's chain and tells the node about peers only through
the traits of its `view` module, so it depends on no crate of the chain, the store or the pool; the
relay (`heartwood-core`) implements them over the node's published snapshots and the store's
read-only connections, one per worker of its runtime, so the verifier's store reads never wait for
another worker's connection. Its edges are `heartwood-crypto`, `heartwood-kernel`
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
  limit and timeout (a getBlocks without its own timeout, as the sync's downloads send it, takes
  the configured `getBlocksTimeout`), the outgoing rate limiter (it waits instead of sending too fast), the reply
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
- a peer source over HTTPS is refused when the monitor starts, with advice to use plain HTTP;
  plain HTTP sources are read up to 4 MiB. `peers.json` takes only URIs as sources, as the
  reference's schema does, so no configuration names a local file on either side;
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

Three vector classes cover the crate (see Vector files), installed beside the crates:
`V24-p2p-exchange` in `packages/p2p-wire/tests/vectors`, `V25-peer-rules` and `V26-network` in
`packages/p2p/tests/vectors` (`WIRE_VECTORS` and `P2P_VECTORS` point the tests at another run).
`heartwood-p2p-wire`'s tests check the client's requests and the reply checks against V24, and
difftest's `l0_wire` test compares V24 under the devnet configuration with nothing skipped;
`heartwood-p2p` checks the peer rules against V25; difftest's `network_vectors` sends each V26
record to the subject of its area and allows no documented difference other than the unlinked
fork segment and the two reader refusals above. The L1 properties P14 to P16 cover the verifier,
the exchange and the monitor (see L1).

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
pool, the store, the chain or the API. `heartwood forger run` runs it (see heartwood-core).

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
- the distinct over-height headers, which only the quorum's lines show, are sorted as the
  reference sorts them for ids that are text or missing (with V8's own order below 64 headers);
  an id of another JSON type compares as a missing one, a `null` header reads as a header without
  an id where the reference throws, and 64 headers or more that mix text and missing ids are
  sorted with the missing ones first;
- a key file on which the group or others have any permission (to read it, to write it) is
  refused, as SSH refuses a private key file, and so is a non-empty `secrets` list;
  the file is never rewritten;
- a relay hostname with a CIDR suffix (`127.0.0.1/8`), which the reference's schema accepts, and a
  `delay` that is not a whole number of milliseconds, which the reference reads as no delay, are
  refused at start; `localAddress` is Heartwood's own option;
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
when it sends at all (with less than 2,000 ms of the slot left it sends nothing). Such a restart
whose first status request (the forger takes the time left just before it) comes with at least
2,000 ms of the slot left, while the relay has not taken the slot's block, is not killed before
the relay has received the recorded block from it again, and fails the test when that does not
happen within 10 s. At least one restart must post its recorded block again, and a test of 50
kills or more must have such a restart.

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
- **the supervisor** (`heartwood supervise`): pm2's restart rules (a child that exits restarts at
  once; a run under 1 second counts as unstable only in the supervisor's first 5 seconds, the
  count is never reset, and the 5th unstable run makes the role `errored`), the child leading a
  process group of its own, `relay start` and `forger start` daemons unless `--daemon=false` or
  `--no-daemon` says otherwise, a stop by SIGINT
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
  the supervisor, and the relays' APIs read with a loopback HTTP GET. A start refuses a node
  whose P2P or API host, as `app.json`, the environment and the `.env` files give it, is not
  loopback, and a relay with devnet addressing refuses to start when its API would listen outside
  127.0.0.0/8.

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
## heartwood-api

`packages/api` is the public API a relay serves, on axum and hyper, under `/api`. It serves the
reference's routes that a follower answers, 18 in all: `/blocks/last`, `/blocks/{id}` and
`/blocks/{id}/transactions`; `/node/status`, `/node/syncing`, `/node/configuration` and
`/node/configuration/crypto`; `/peers` and `/peers/{ip}`; `/transactions` (the list, and `POST` of
a batch into the pool), `/transactions/{id}`, `/transactions/unconfirmed` and
`/transactions/unconfirmed/{id}`, `/transactions/types` and `/transactions/fees`; `/wallets/{id}`
and `/wallets/{id}/transactions`. Every answer has the reference's envelope, keys in the reference's
order, amounts as decimal strings, and hapi's error bodies. The API reads only the node's published
snapshots and the store's read-only connections, so a request never waits for the node thread,
except a submission for its pool's answer.

A request goes through the reference's lifecycle in its order: the address whitelist (403); the
query grammar (a query the reference's plugins throw on is 500, on any path and uncharged); the
route; a `POST`'s payload (its declared and decoded size up to 1 MiB, its content type read as hapi
reads it, gzip and deflate bodies decoded, JSON; any failure is 422 with hapi's message); the rate
limit (100 points per 60 s per address, charged before validation, 429 without `Retry-After`); the
path, query and payload against the route's schema (422 in Joi's style; the text of a 422 message is
not part of the contract); the semaphore; the handler. Every answer, errors included, carries
`X-Block-Height` (the published tip), `connection: close` and `cache-control: no-cache`; every
answer but a preflight's carries `vary: origin` and `access-control-expose-headers`, and
`access-control-allow-origin` when the request has an `Origin`; a preflight is answered 204 with
the reference's fixed set of headers, and its refusals carry no CORS header. A `GET`'s 200 answer
with a body offers byte ranges and serves one range as hapi does (206, or 416 for a range it
cannot serve). A `GET` or `HEAD` that no route matches is answered as the reference's catch-all
route (its documentation's) answers it: a path that does not decode, as hapi normalises it, is 400
"Bad Request" without a charge (500 to its preflight); any other is charged a point and answered
404, or 403 for a path of fewer than 20 characters that leads above the documentation's directory
(`/..%2Fx`), as inert refuses it. The headers are read as Node gives them to hapi: as Latin-1, and a
repeated header's values joined with `, ` (the first one kept for `Content-Type`,
`Content-Length` and the others Node does not join), so two `Range` headers do not read and two
`Content-Encoding` values name no decoder. A preflight's empty `Origin` or
`Access-Control-Request-Method` is a missing one, and an echoed `Origin` is written back in the
bytes Node writes (UTF-8 when the answer has a body that hapi would not compress, Latin-1
otherwise, the encoding chosen from `Accept-Encoding` as `@hapi/accept` chooses it). A gzip body
is read member by member, and a zero byte where a member would start ends it, as Node's gunzip
reads it.

The query grammar is the reference's: Node's `querystring` (at most 1,000 pairs, a repeated key as
a list), then its comma and dot plugins, with their throws, sparse lists, JavaScript's key order
and a later key changing a value an earlier one set. It builds a query with a loop, at any depth,
and writes it out at most 128 levels deep and with at most 16,384 holes of sparse lists, so no
query costs unbounded work. The bounds show only in the `asset` criteria of the three transaction
lists, which answer 500 for a deeper value; the reference answers 200 up to about 3,000 levels and
500 from about 4,000, when its link rendering overflows its stack.

A request's head is read as Node 18's parser reads it (llhttp 6.1.1, in the loose mode Node builds
it in), before hyper serves the connection. What that parser refuses (a head whose target and header
names and values reach 16,384 bytes; an unknown or lowercase method; a byte it does not take in the
target, a name or a value; a version other than 0.9, 1.0, 1.1 and 2.0; a folded line; a bad or
repeated `Content-Length`; `Transfer-Encoding` after `chunked` or beside a length) is answered
`HTTP/1.1 400 Bad Request` with no header, as the reference's server answers it. Line ends before
the request line are skipped without being counted. A `CONNECT` and a stream that ends inside its
head get no answer, as from Node's server; a target that starts with `*` is hapi's 400 "Invalid
URL"; a client that shuts its side of the connection after its request is answered, as Node's server
allows half-open sockets. Where the parser fails on what follows the head while hapi serves the
request, hapi answers that request with its JSON 400, before any of its steps and without a charge,
and so does Heartwood for what was read with the head: a `Transfer-Encoding` whose last coding is
not `chunked`, and, on a connection llhttp keeps alive and does not upgrade, bytes after the request
(and after its body, unless a route serves the request as a `GET` or `HEAD`, which hapi answers
without reading its body) that start no request the parser takes. A request after it that Node's
server closes the connection on (a `CONNECT`, or a method it does not know) leaves the first
unanswered. A head the parser accepts is written again for hyper with the meaning Node's server
gives it: single spaces and `HTTP/1.1` in the request line (Node answers every version it accepts as
`HTTP/1.1`), the target's bytes that hyper refuses rewritten (a byte beyond ASCII as the UTF-8 of
its Latin-1 character, another one percent-encoded) and its fragment dropped, the first 1,000 header
fields (Node's server passes on no more) and those that frame the body, less those whose name hyper
refuses (an empty name, or one with a space: none that the API reads) and an empty
`Transfer-Encoding`, which names no coding for Node's parser. The API reads the target itself as
Node gives it to hapi: each byte a Latin-1 character, the path before the first `?` and the query
after it, so the router, the catch-all's 20-character test and the query grammar see what the
reference's do.

Lists have the reference's pagination: `offset=0` is ignored and another offset replaces the page,
and the links are relative to the configured base path, with the path written as hapi's request
URL writes it.

The history routes read the store as the reference reads PostgreSQL (see heartwood-database). A
transfer's resource shows the sum of its transfers, but the `amount` criteria and orders read only
a burn's amount, as the reference's table holds it. `/node/configuration/crypto` serves every
genesis transaction with `headerType` 0, as the reference's serialiser leaves it at boot.

Where the API differs on purpose, the reference has a defect or does work the relay leaves out;
each case shows only with the request named:

- a store error on a list, or a statement that runs past the database's `apiConnectionTimeout`, is
  a logged 500; the reference answers an empty 200. A `memo` pattern that ends with its escape
  character is the one such error a client can cause;
- a block height above the tip, and a `/blocks/{id}/transactions` value that is neither a full id
  nor a height at or below the tip, is 404 "Block not found"; the reference matches an id prefix;
- the semaphore bounds the store's routes with the reference's settings, which the reference reads
  from a path that does not exist and so never applies: a request beyond a level's places and
  queue is 429;
- the pool routes leave `burnedFee` out of their answers without changing the pool's transactions;
- `/peers?ip=` lists the peers at exactly that address, where the reference ignores the filter;
  `/peers?version=` reads the same subset of npm ranges as the client's version gate, and a range
  outside it is 404 "Invalid version range provided", as an invalid range is in the reference;
- ties of a requested order are broken by the block height and the sequence;
- pagination links drop the configured base path, where the reference drops the first four
  characters of the path whatever the base path is;
- no response is compressed: the reference compresses a JSON answer of 1,024 bytes or more with
  the gzip or deflate its client's `Accept-Encoding` chooses, and such an answer carries no
  `accept-ranges` and is never cut to a range; Heartwood answers identity, with the reference's
  `vary` and the echoed `Origin` in the bytes the reference writes it in;
- the 5 s socket timeout bounds the whole connection, headers included, where the reference's is
  5 s without activity; with a socket timeout of 0 a connection is bounded by 300 s, where the
  reference bounds only the request's reception by that time;
- a delegate's attributes come in the order the reference's wallets have after a rebuild from the
  store (rank before voters, then the last block, the version and the resignation); a reference
  node that has kept a wallet since a key was dropped and set again lists that key last;
- a form body (`application/x-www-form-urlencoded`) is read as no fields, which the schema then
  refuses (422 on both sides, with another message);
- the router takes the path as sent: the reference first decodes percent-escaped unreserved
  characters and resolves dot segments, so `/%61pi/...` or `/api/./peers` is 404 here;
- the reference's documentation pages (`/`, `/index.html`, `/api.json`, `/swagger-ui.css`,
  `/swagger-ui.js`, and the paths that reach them through an escaped slash, such as `/..%2Fwww`)
  and the routes a relay does not serve are unmatched paths here, including
  `/blocks/first`, `/blocks/missed`, `/transactions/schemas` and `/wallets/top`, which a served
  parameter route would otherwise take and which the reference answers 200;
- a request cookie that the reference refuses (400) is not read;
- a JSON payload with a lone surrogate escape, or nested deeper than 128 levels, is 422 "Invalid
  request payload JSON format"; the reference parses it and its schema or pool answers;
- a byte range whose suffix is longer than the answer is served as the whole answer (206), where
  the reference declares more bytes than it sends, and a 416 carries `X-Block-Height` and
  `connection: close`, which the reference's lacks;
- a `POST /transactions` batch is at most 40 transactions, the pool's default
  `maxTransactionsPerRequest`; the reference takes the configured value;
- a request head that has not ended after 417,792 bytes (possible only with bytes Node does not
  count, such as white space before a value) is 400, where Node reads on;
- a method that Node's parser reads and its server does not know (`PRI` without a version, or a
  method of RTSP with an `RTSP/` version or none) makes the reference's server throw, which stops
  its process; Heartwood closes that connection without an answer.

The server is hyper, which reads some requests otherwise than Node's server even after the head
reader; none is sent by a common HTTP client:

- what the client sends after its request's head beyond what was read with it, and a chunked body:
  Node's parser reads them as they come, and hapi answers its 400 when the parser fails on them
  before the answer is sent (a malformed chunk, or bytes that start no request); Heartwood judges
  only what was read with the head, and hyper reads a chunked body itself (a malformed chunk is a
  422 or goes unread). A submission whose JSON does not parse, followed by such bytes, is the
  reference's 422 without its plugins' headers, and Heartwood's 400;
- an `Expect` other than `100-continue` on HTTP/1.1: Node's server answers 417 itself and keeps the
  connection; Heartwood serves the request;
- a body that does not arrive in full: the reference reads the body of a `POST`, of a preflight
  and of a request that no route takes before it answers, and answers nothing if the connection
  ends first; Heartwood answers a preflight or an unmatched request without reading its body, and
  a `POST /transactions` whose body ends early with a 422;
- an absolute-form target (`GET http://host/path`, as a proxy is sent a request): the reference
  reads its path and query with the WHATWG URL parser, and answers 400 "Invalid URL" for a host that
  parser refuses (`http://a@b:c/`); Heartwood reads them as hyper gives them, and hyper refuses with
  a 400 of its own headers some hosts that parser takes (`http://a%41/`).

```sh
cargo test -p heartwood-api --locked                                # routes, grammar, envelope, plugins
cargo test -p heartwood-api --locked -- --include-ignored          # also the served chain over a store
cargo test -p heartwood-difftest --locked --test api_vectors       # V28's grammar and pagination records
```

The tests serve the committed chain from a store and pin the answers to the reference's for the
same chain (its node controller's answers, and its wallets, blocks and transactions at the tip);
those marked `integration` need the whole crate. The class V28 holds the reference's query grammar (`api.parseQuery`) and pagination
(`api.paginate`); the route corpus (`tools/difftest/api-corpus`) compares whole answers of a live
node with a reference node's (see L3 and L4).

## heartwood-core

`packages/core` is the `heartwood` binary. `heartwood relay run` (or `heartwood relay:run`) runs a
relay in the foreground, with the reference's flags in camelCase and their kebab-case aliases:
`--token`, `--network`, `--env`, `--networkStart`, `--disableDiscovery`, `--skipDiscovery`,
`--ignoreMinimumNetworkReach` and `--launchMode`. `config.json` in the config directory fills a
missing `--token` or `--network` and never overrides a given flag. The relay reports its version
as `4.3.1-heartwood.<crate version>`, a prerelease of the reference's version that a devnet's
version floor admits and that tells the two implementations apart.

The start reads the configuration before anything else (the directories, the environment, the
relay's profile of `app.json`, the components' options, `peers.json` and the crypto files), takes
the process lock (`<data>/run/relay.lock`, and `relay.pid`, a new file of mode 0600 in a run
directory of mode 0700), starts the logger, and then the components in the reference's order: the
store and the state (restored, with the version floor checked at the stored tip), the pool, the
node thread, the peer-to-peer client, the blockchain service, and, once the blockchain has started,
the API. SIGINT or SIGTERM stops them in reverse: the API stops accepting connections, the queue
stops after its job, the pool is flushed, the online record written and the store closed, and the
process exits with 0; a second signal exits at once. A signal during the start's restore (a long
replay) ends the start once the restore is done, with 0. The log lines that tools read (a refused
or undone block, a download, a fork) keep the reference's text; other lines are Heartwood's.

The reference's marker files and reset variables work as in the reference: `clear-saved-states.lock`
in the temp directory restores the state by a replay from the genesis block (and is removed when it
is done), `clear-pool.lock` clears the pool's storage, `CORE_RESET_DATABASE` empties the store (the
start is then a first start) and the pool, and `CORE_RESET_POOL` empties the pool, at every start
while they are set. The pool's online record is loaded at every start, whatever the restore path, so
a vote for a delegate that announced itself before the stop is admitted at once; a reference relay
started without its saved state refuses it until a peer announces the delegate again.

A relay differs from a reference relay only in what the configuration or the command line asks of
it:

- files and variables a relay does not need or use: a relay needs no `delegates.json` and no home
  `.env`; the reference's test and skip variables (`CORE_ENV=test`, `NODE_ENV=test`,
  `CORE_SKIP_PEER_STATE_VERIFICATION`, `CORE_SKIP_BLOCKCHAIN_STARTED_CHECK`), `CORE_API_CACHE`,
  `CORE_API_LOG`, `CORE_WALLET_SYNC_ENABLED`, a profile's `flags`, an `app.js` and the state's options
  in `app.json` (its saved states, caches and wallet table) are not honoured, and the start names
  each one that is set in a warning; the DNS and NTP checks the reference logs at boot are not run;
- an API option that the reference's schema refuses is logged and the relay runs without an API, as
  the reference's API is not a required component; `CORE_API_SSL` and an API host that does not
  resolve refuse the start (serve HTTPS through a reverse proxy); an API address that cannot be
  listened on stops the relay with exit status 1 once its blockchain has started, where the
  reference's application terminates its components and leaves the process running;
- a log level name that is not one of the reference's eight turns that output off, as in the
  reference; a level that is not text refuses the start, where the reference runs with its memory
  logger on standard output. With `HOME` unset the start is refused, where the reference falls back
  to the user's passwd entry; `USER` is not read;
- a version floor in force at the stored tip that refuses the relay's own version stops the start,
  naming the height;
- the command line is read by clap: an unknown flag, a boolean flag given a value
  (`--networkStart=false`), the `--no-` forms and the reference's global `-v` and `--quiet` are
  refused with a usage error (status 2); `--network` takes any name that has a complete set of
  crypto files, and with neither `--network` nor `config.json` the start is refused, where the
  reference discovers the one network directory.

Release builds build the package alone, `cargo build --release --locked -p heartwood-core`, since a
build of the whole workspace turns on the test seams through `heartwood-difftest`. With its feature
`difftest` (`make relay-binary`) `relay run` also takes the harness's flags, which no release build
has: `--difftest-digest FILE` writes one state digest line per applied, reverted and restarted state,
`--difftest-blocks FILE` the bytes of every applied block, and `--difftest-seed IP:PORT` names one
seed that the address rules let through, for a mock peer on the same host.

```sh
cargo test -p heartwood-core --locked                               # the command line, the configuration, the lock and the views
cargo test -p heartwood-core --locked --all-features -- --include-ignored   # also a relay importing the committed chain
```

The integration tests start the binary: a relay imports the committed chain from a mock peer, serves
its tip through the API with a digest equal to the judge's, and stops on SIGINT with 0; a relay
whose API port is taken stops with an error.

## L0: golden vectors

`difftest run l0` compares the records of a vector run with Heartwood, with one subject per
oracle operation. Where both sides refuse, it also compares the error class wherever Heartwood
models the reference's class. On a run of every class (reference tag `s1-ref-v2`) it reports:

| Outcome | Records |
|---|---|
| compared, equal to the reference | 8,643 |
| documented differences, each with its form checked on the input or shown by the oracle's answer | 177 |
| skipped | 0 |

That is 8,820 records in 28 files, and a run of the image and a local run agree on every one. The
gate runs with `--strict` (`make l0`), which fails a record that is skipped or has no Rust subject
as it fails a difference. The builders of the removed types and of the extended header, which
Heartwood does not have, are documented refusals (checked on the builder's input), so nothing is
skipped.

`cargo test` checks the classes of each crate in that crate; the difftest test `l0` runs the
comparison over the committed vectors of `heartwood-crypto` (2,212 compared, 64 documented, none
skipped), the test `l0_wire` over the wire class of `heartwood-p2p-wire` (1,179 compared, 14
documented, none skipped) and its exchange class, `pool_vectors` over the pool class (42 sessions
compared, 8 documented), `config_vectors` over the crypto-file class (17 compared, 79 documented),
`network_vectors` over V26 (404 compared, 5 documented), `processor_vectors` and `job_vectors` over
V27, and `api_vectors` and `config_app_vectors` over V28.
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
  predicate on the input, and a file that the reference refuses must be refused by Heartwood;
- a block refused in its apply after missed slots: the reference keeps the missed-slot rows it
  wrote before it validated the block. The case is documented only when the reference also refused
  the block in its apply and its answer, less those rows, equals Heartwood's whole answer (two V27
  records assert it);
- a block with a transaction of a removed type in a download: Heartwood's reader refuses the whole
  getBlocks reply, where the reference's job applies the blocks before it and rejects that block in
  its apply. The case is documented only when the reference's answer is that refusal (every earlier
  block accepted, that block rejected, the queue cleared).

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
| P13 processor | on five prefixes of each corpus chain (the committed one, and `c2-faults` cut at 265 and `c4-restarts` when they are under `target/corpus`), the next block built as the slot's delegate builds it and then mutated (timestamp, height, previous id, generator, reward, transactions, version, payload length, signature, or a block of the chain itself), with a chain state around it (the last downloaded and forked blocks, whether the blockchain has started, blocks of the job not stored yet, a stored id, no delegate name at enqueue) | `chain.process`: the result, the handler, the unchained status, the chain state after it and the digest line after an accepted block |
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

P13 agrees with the reference at 10,000 and at 20,000 cases with the date as the seed. Its only
documented case is a block refused in its apply after missed slots (34 and 59 cases). `make l1`
prints the reference's results, unchained statuses and handlers for P13 and, from 1,000 cases,
fails the run when a reachable result or an unchained status appears fewer than 20 times
(`--min-reach`, `L1_MIN_REACH`); at 20,000 cases the reference answered 11,251 `Rejected`, 4,590
`Accepted`, 2,619 `DiscardedButCanBeBroadcasted` and 1,540 `Rollback`, and every unchained status
at least 671 times. `Reverted` and `Corrupted` cannot come from one processed block. The report
names the corpora the cases drew from; a missing one is a skipped part, which fails a strict
run.

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
that height, and goes on from there. With `RUNTIME=1` (`--runtime`) the child is a relay built with
the harness's flags that imports the chain from a mock peer on the same host, through the peer
client, the jobs and the node thread: its digest lines must equal the continuous lines, the store
must restore by both paths to one committed tip, and after the kills a last relay imports the rest
of the chain. In half the kills the mock peer answers its first getBlocks with nothing; that
request is the relay's verification of the seed (headers only), so the relay drops the seed, falls
back to it after the monitor's wait, and imports in downloads of 400 blocks cut into jobs of 100.
The monitor downloads a tenth of a chunk only after an empty download of blocks, which this mock
peer never gives, so the report counts the stored tips off the grid of those jobs, which stays 0.
On the committed chain, 200 kills of a relay land on stored tips 1, 101, 201 and 265 and all pass.

**Through the processor.** `make corpus-judge PROCESSOR=1` also runs the chain through the
reference's own job, one block per job, and writes each block's result and job end to
`ts-processor.jsonl`; every block must be accepted and every line must equal the plain replay's.
`make l2-processor` (`difftest run l2 DIR --processor`) replays the corpus as a follower imports it:
each run of blocks between two reverts is one download, listed as a getBlocks reply, read through
the reply's codec, its check and the download reader, named and cut into jobs as the reference's
enqueueing cuts it (and again one block per job), and run through the job and the processor,
committed per job to a new store. Both cuts must give the judge's lines, every result and job end
must be the judge's, the store's rows must equal the node's, and the two cuts must give equal lines
and results; a strict run fails on a corpus judged without `PROCESSOR`. `make l2-synthetic` also
runs the synthetic processor and fork cases on top of the chain, each through the reference's job
and Heartwood's: every unchained status, a double-forged block and the recovery at fixed depths, a
refused block after a slot gap, an unchained first block, a job cut at a milestone, conflicting and
already forged transactions, a removal across a round end with the pool's re-adds, a block in a
future slot, a removed transaction type in a block (first in a job, and after an accepted block), a
vote at a round's end with a competing branch, and a milestone at exactly the last downloaded
height. On the committed chain the processor replay compares 6,490 records and the synthetic
processor cases 36, with 4 documented (the two differences above).

```sh
make corpus-import ARCHIVE='archive/...' NAME=my-chain   # into target/corpus/my-chain
make corpus-judge NAME=my-chain RESTARTS=every            # the judge's digest lines, checks and restart lines
make l2 NAME=my-chain                                     # Heartwood through its store against those lines
make l2-restart NAME=my-chain                             # the same with the restarts of the plan
make l2-synthetic NAME=my-chain                           # with the synthetic blocks, restarts and pool sessions (needs the oracle)
make crash-test NAME=my-chain KILLS=200                   # the store after SIGKILLs
make crash-test NAME=my-chain KILLS=200 RUNTIME=1         # a relay importing the chain, after SIGKILLs
make corpus-judge NAME=my-chain PROCESSOR=1               # also each block's result through the reference's job
make l2-processor NAME=my-chain                           # Heartwood's download path, jobs and processor against those
make forge-replay NAME=my-chain                           # every block rebuilt by the forger
cargo test -p heartwood-difftest --locked --test l2       # the committed corpus with its restart lines, through the
                                                          # store and through the processor, no oracle
```

The committed corpus, `tools/difftest/corpus/txgen-one-node`, is a one-node devnet chain of five
rounds with every kept transaction type and one live restart of the node, with the judge's lines
and a restart line of both reference paths at every height; its README describes it. The
maintainers also replay chains made with the same unpublished tooling, which are imported under
`target/corpus` and are not committed: three nodes with partitions, kills and restarts (replayed up
to the end of its fifth round), and a one-node chain of five rounds restarted once from its saved
state and once without it, with an API checkpoint after each restart, which the judge's headless
restarts must reproduce. The chains of the live runs (see L3 and L4) are imported and judged the
same way, with `PROCESSOR=1`, and replayed through the processor.

## L3 and L4: live runs

L3 and L4 run on devnets of the maintainers' devnet tooling (a separate tool that is not published;
`LIVE_RECIPE`), with reference nodes of the reference's image and Heartwood relays of the image the
tooling builds from `target/release/heartwood` (`make relay-image`). A Heartwood relay starts with
production addressing on the devnet's own network, and each run stops at the end of its fifth round
at the latest. `make live RUN=<run>` runs one; `make live-check RUN=<run>` (`difftest live check`)
judges its directory, and `make api-compare RUN=<run>` its route corpus.

| Run | Network | What happens |
|---|---|---|
| `r` | three reference forgers and a reference relay as the observer (node 4) | the scenario: every kept transaction type with its expected refusals, sent through node 4; a batch through node 4 in every pool before the next block; node 4 finding its peers |
| `h` | the same, with a Heartwood relay as node 4 | the same scenario; each submission must get the answer it got in run `r`, and the scenario's wallets must equal run `r`'s |
| `m1` | three reference forgers, a reference relay, and two Heartwood relays (nodes 4 and 5) | node 4 stopped by SIGINT, killed three times, and restarted with the replay marker; node 5 started with an empty store while the chain advances and killed during its first sync; a forger stopped; votes before and after the replay restart |
| `m2` | four nodes with a Heartwood relay as node 4 | a partition into two pairs across a round end, both sides forging, two conflicting transfers, then the heal |

For each run the check requires: every scripted step met its expectation; node 1's chain (and the
minority's, before a heal) imported as a corpus and judged, with its anchors; each Heartwood relay's
digest lines equal to the judge's at every height and restart, its store's rows equal to node 1's,
and its starts equal to the scripted ones (a kill during the sync must end the first start below
node 1's height); no reply refusal and no protocol violation recorded against a reference peer; no
reference node banning a Heartwood relay for a refused request or a broken session (a closed socket
inside a fault window is recorded, not failed); one tip at the end, every generator the scheduled
one and the supply check; and, for node 4 of `r`, `h` and `m1`, every block within one block time
of node 1. A run binds itself to the relay it tested: the check fails when the SHA-256 of the
judged tree's `target/release/heartwood` is not the one the run recorded. For `m2` both sides must
have forged, nodes 3 and 4 must converge on the majority's chain by themselves, node 4's lines must
equal the judge's on the minority's chain and then on the majority's, and of the two conflicting
transfers only the majority's may be in the final chain, while the other has left every pool.

The route corpus (`tools/difftest/api-corpus`, 1,038 queries: each served route, each criterion
alone and in pairs, orders, pages, `offset=0`, unknown keys, invalid values, the 404 texts, wallets
by address, key and name, the peers' filters, the pool after two submissions, and 102 requests for
the rate limit) is sent at the run's pause to node 1 and to the node under test, alternately, and
their answers are judged: equal status codes, `connection: close` and `X-Block-Height` present and
equal at the paused tip, bodies equal after the volatile fields are left out (confirmations, times,
versions, latencies), the peer routes compared by address for the peers both list, the rate
limit's 429s equal, and each documented difference of heartwood-api behaving as documented. A 422
message's text is not compared. A documented difference excuses the status and the body only: the
headers are judged as for any case, except `accept-ranges` when the statuses differ, and a tie of
an order excuses only the order of the same items (an item on both pages must be equal on both,
and one on a single page must belong to a tie that the page's start or end cuts). The rate-limit
requests go from a container on the devnet, since the nodes whitelist the host. A rate-limit
request that failed, or an answer without a status, is a harness error, and each node must answer
429 at least once in the section; a body is compared where the recipe records one, which is a
429's. The live check's rate-limit step applies the same rules.

On the last runs every run passed. Run `r`, the reference against itself, compared all 1,038
queries with no difference. In `h` and `m1` the corpus compared 1,006 and 1,008 answers with no
undocumented difference; the documented ones were ties of an order (27 and 25), a height above the
tip (3 each), a store failure (1 each) and a decoder's reason text in the pool's refusals (2 each).
The Heartwood relays' lines equalled the judge's on every chain, and the processor replays of the
five live chains passed. A Heartwood relay applies a block 0.5 s after node 1 at the median (at most
2.5 s away from a fault), where a reference relay applies it at once: it learns of blocks by its
500 ms poll, since no peer pushes to a relay without a listener; a relay started with an empty
store while the chain was at height 107 was synced 12 s after its start, a kill during the sync
included; a transaction posted to a Heartwood relay reached the reference nodes' pools in about
220 ms.

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
| `V24-p2p-exchange.jsonl` (p2p-wire) | the client's requests as a reference server reads them (accepted with the reference client's bytes, or refused by the version gate or the payload limit), and the reply checks of the six routes other than getStatus, with their verdicts and changed values |
| `V25-peer-rules.jsonl` (p2p) | the address rules of candidate peers, the version gate over the devnet's, the default and 400 seeded ranges, the same-subnet test and the outgoing rate limiter's waits |
| `V26-network.jsonl` (p2p) | the status attestations of a ping, the n-ary search for the common block and the peer verification over forks of a devnet chain, the monitor's network height, fork decisions and download plans, and blocks rebuilt from getBlocks replies |
| `V27-processor.jsonl` (blockchain) | the reference's processor on one block and a chain state (`chain.process`: every result and unchained status, the invalid generator, the reward, nonces, conflicting and stored transactions, a block refused in its apply after missed slots, a removed type) and its job on a download of one or more jobs (`chain.job`: the job's results and end, its commit, a delegate's registration and its first block in one download, as one job and as two) |
| `V28-node.jsonl` (kernel and api) | the reference's reading of `app.json` (`config.loadApp`) and of the components' options (`config.pluginOptions`), and the API's query grammar (`api.parseQuery`, with its throws) and pagination (`api.paginate`) |
| `V29-forging.jsonl` (not installed) | block assembly (`forger.forge`: an empty block, each kept type alone and together, burns, 256 recipients, 150 and 151 transactions, a block over `maxPayload`, totals and rewards at the 64-bit edges, three aux values, milestones that change at the next height), collation and assembly after pool sessions (`forger.collateAndForge`), one attempt of the forger's slot (`forger.decide`, with its log lines), and the internal routes' round and network-state bodies as a relay writes them and the forger reads them |
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
difftest run l1 [--property P]... [--cases N] [--seed S] [--vectors DIR] [--regressions DIR] [--min-reach N] [ORACLE]
difftest run l2 DIR [--synthetic] [--restart] [--memory] [--to H] [ORACLE]
difftest run l2 DIR --processor [--chunks reference|1] [--synthetic] [--to H] [ORACLE]
difftest run l3|l4              points to the live runs (see `difftest levels`)
difftest corpus import ARCHIVE... --out DIR [--seed TEXT]
difftest corpus judge DIR [--restarts every|rounds] [--restart-plan FILE] [--to H] [--dump H,...] [--processor] [ORACLE]
difftest corpus verify DIR
difftest store replay DIR --db PATH [--to H]
difftest store check DIR --db PATH
difftest store crash DIR [--kills N] [--seed S] [--max-delay MS] [--runtime]
difftest live check DIR
difftest api compare --run DIR
difftest api compare --left URL --right URL [--corpus DIR] --out DIR
difftest levels
difftest oracle replay OP CORPUS --vectors DIR [--height N] [--milestones FILE] [ORACLE]
difftest genesis --profile FILE --seed TEXT --epoch TIME --out DIR [--force] [--compare DIR]
```

`ORACLE` is `--image TAG` or `--solar-dir DIR`; `difftest --help` lists every option. `run l2
--memory` replays without a store, as before the store existed; `store replay` goes on replaying a
corpus through the store under `PATH`, one block per chunk, and `store check` restores it by both
start paths and prints each one's tip and restart line: the kill test's two halves. `live check`
judges a live run's directory and `api compare --run` its recorded API answers (see L3 and L4);
`api compare --left --right` sends the route corpus to two running nodes and judges their answers.
`--strict`, with any command that compares, fails a run in which a case was skipped or had no Rust
subject, as a failed case does; the Makefile's gate targets pass it.

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
restarts and pool sessions), `synthetic_processor` (the synthetic processor and fork cases),
`restart` (restart plans and the reference's restart lines), `store` (the replay through the store,
the store's rows against the node's, and the kill test), `runtime_kill` (the kill test through a
relay), `levels` (L0, L1 and L2 run, with `levels::pool`, the judge of a pool session, and
`levels::l2_processor`, the replay through the processor and the judge of a job), `live` (the
checks of a live run) and `api` (the route corpus, its client, recording and judge, and the V28
subjects).

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
