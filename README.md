![IceRoot Heartwood Core](banner.png)

# Heartwood Core

The Rust implementation of IceRoot's node software. This repository currently provides cryptography, transaction and block codecs, network configuration, the headless state machine (wallets, delegates, rounds, transaction handlers, block application and reversal), the peer-to-peer wire codec and client, the store (SQLite), the transaction pool, block processing, the public API, a relay process that follows a network and serves that API, and a differential-testing harness. It does not yet provide a forger, a peer-to-peer server (the relay opens no peer-to-peer listener) or the command-line tools.

[Core overview](https://docs.iceroot.com/core/overview/) · [Build from source](https://docs.iceroot.com/core/installation/) · [Architecture](https://docs.iceroot.com/core/architecture/) · [Protocol design](https://docs.iceroot.com/core/protocol-design/)

## Build and test

Install Rust 1.98 or later with rustfmt and Clippy.

```sh
make check
```

This builds the workspace, checks formatting and lints, runs tests against committed vectors (the integration-tagged tests included), builds the documentation with warnings as errors, and checks release features and the crates' line ceilings. `make help` lists individual targets.

```sh
cargo test -p heartwood-crypto --locked
cargo test -p heartwood-crypto --locked --all-features
cargo test -p heartwood-database --locked
cargo test -p heartwood-pool --locked
cargo test -p heartwood-blockchain --locked
cargo test -p heartwood-api --locked
cargo test -p heartwood-core --locked
```

The peer-to-peer client has its own target, which `make check` also runs:

```sh
make test-p2p
```

It lints and tests `heartwood-p2p` without and with its test feature. [Compatibility and testing](docs/compatibility.md#heartwood-p2p) describes its tests against a mock peer and a reference node.

Release builds must select one package at a time, for example `cargo build --release -p heartwood-crypto`. Do not use `--workspace` for releases: the test harness enables features that must not enter production packages.

## Run a relay

```sh
cargo build --release --locked -p heartwood-core
target/release/heartwood relay run --network <name>
```

`heartwood relay run` (or `relay:run`) takes the reference's flags and reads the reference's configuration of the network: `app.json`, `peers.json`, `.env` and the four `crypto/*.json` files in the node's config directory (`~/.config/<token>-core/<network>` on Linux, token `solar` unless `--token` or `config.json` names another; the `CORE_PATH_*` variables move each directory). It keeps its store in the data directory and serves the public API on `CORE_API_PORT` (4003). The relay follows the network as a client of its peers: it opens no peer-to-peer listener yet, so no peer pushes to it: it learns of new blocks by polling its peers and of their pools' transactions by pulling them every 4 seconds. SIGINT or SIGTERM stops it gracefully after the current job. [Compatibility and testing](docs/compatibility.md#heartwood-core) lists the settings it honours and where it differs from a reference relay.

## Components

| Path | Purpose |
| --- | --- |
| `packages/crypto/` | Keys, signatures, addresses, codecs, transaction rules, milestones and the dynamic fee floor |
| `packages/kernel/` | Round and slot arithmetic, block chaining, the clock seam, event names, log levels, node directories, `.env` files and the strict loader of a network's crypto files |
| `packages/logger/` | The reference's log lines, console and file output, daily rotation |
| `packages/state/` | Wallets, delegates, rounds, block application and reversal, genesis, the supply check, the state digest and the records a restart reads back |
| `packages/transactions/` | The six transaction handlers, the fee check and the pool-entry checks |
| `packages/p2p-wire/` | The peer-to-peer wire codec: nes frames, the protobuf messages of the public routes, list framing and the signed peer status |
| `packages/database/` | The store: one SQLite file per node with the chain tables, the state tables and the checks run at every start |
| `packages/pool/` | The transaction pool: admission with the reference's checks, codes and messages, storage, queries, collation and the online record |
| `packages/blockchain/` | The blockchain component: the block processor, jobs committed to the store, the node thread, the job queue, the state machine with fork recovery, and a node's start (first start, restart from the state tables, or replay from the genesis block) |
| `packages/p2p/` | The client side of the peer-to-peer protocol, as the relay uses it: nes sessions over WebSocket, requests and reply checks, peer acceptance and verification, discovery, downloads and broadcasts |
| `packages/api/` | The public API a relay serves: the block, node, peer, transaction and wallet routes with the reference's envelope, query grammar, pagination, headers, rate limit and semaphore |
| `packages/core/` | The `heartwood` binary: `heartwood relay run`, the relay's configuration, process lock, signals and graceful shutdown |
| `tools/difftest/` | Compatibility harness and command-line tools |
| `tools/oracle/` | Reference execution, container builds and vector generation |
| `packages/*/tests/vectors/` | Committed vectors of each crate, with checksums and source metadata |
| `fuzz/` | Decoder fuzz targets, the state machine's apply and revert targets, the state's records, the pool, and the peer-to-peer client's session, reply checks and download path |

The current implementation uses the retained reference cryptography and transaction formats. Proposed protocol features in the whitepaper describe a separate target; they are not all implemented here.

## Compatibility

The reference is [solar-core-ref](https://github.com/iceroot-network/solar-core-ref), based on Solar Core 4.3.1 with a documented patch series. Over the ten crates that hold vector classes, the committed L0 vectors give 8,643 equal comparisons and 177 documented differences, with no record skipped. L1 (random cases against the reference, one call each), L2 (reference devnet chains replayed block by block through the store and through the block processor, with a state digest at every height and restarts by every start path), L3 (one scenario on a reference devnet, observed by a reference relay and by a Heartwood relay) and L4 (reference devnets with Heartwood relays: restarts, a sync from an empty store, a partition and its heal) run as well.

See [compatibility and testing](docs/compatibility.md) for exact boundaries, oracle prerequisites, vector classes, release requirements and regeneration instructions. Oracle runs are isolated from the network. Standard Rust checks use committed vectors and do not require the reference checkout or Docker.

## Contribute

[Contributing](https://github.com/iceroot-network/.github/blob/prod/CONTRIBUTING.md). Work on `dev`. Production changes reach `prod` through a reviewed `dev` → `prod` pull request.

Heartwood Core is licensed under the [Apache License, Version 2.0](LICENSE). [NOTICE](NOTICE) preserves ARK Core and Solar Core attribution. External vector sources and checksums remain alongside the vector files.
