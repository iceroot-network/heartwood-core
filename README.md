![IceRoot Heartwood Core](banner.png)

# Heartwood Core

The Rust implementation of IceRoot's node software. This repository currently provides cryptography, transaction and block codecs, network configuration, the headless state machine (wallets, delegates, rounds, transaction handlers, block application and reversal), the peer-to-peer wire codec and client, the store (SQLite) with a node's commit and restart, the transaction pool, the forger, the commands (a process supervisor, `network generate` and a local devnet library), and a differential-testing harness. It does not yet provide a running node binary, block processing, a peer-to-peer server or a REST server.

[Core overview](https://docs.iceroot.com/core/overview/) · [Build from source](https://docs.iceroot.com/core/installation/) · [Architecture](https://docs.iceroot.com/core/architecture/) · [Protocol design](https://docs.iceroot.com/core/protocol-design/)

## Build and test

Install Rust 1.98 or later with rustfmt and Clippy.

```sh
make check
```

This builds the workspace, checks formatting and lints, runs tests against committed vectors, and checks release features. `make help` lists individual targets.

```sh
cargo test -p heartwood-crypto --locked
cargo test -p heartwood-crypto --locked --all-features
cargo test -p heartwood-database --locked
cargo test -p heartwood-pool --locked
cargo test -p heartwood-blockchain --locked
```

The peer-to-peer client has its own target, which `make check` also runs:

```sh
make test-p2p
```

It lints and tests `heartwood-p2p` without and with its test feature. [Compatibility and testing](docs/compatibility.md#heartwood-p2p) describes its tests against a mock peer and a reference node.

The forger and the commands have their own targets too, which `make check` also runs:

```sh
make test-forger
make test-cli
```

They lint and test `heartwood-forger` and `heartwood-cli` with and without their features. `make forger-crash` kills a forger at random moments against a mock relay and checks that no slot gets two different blocks, and `make forge-replay` rebuilds a corpus's blocks through the forger. [Compatibility and testing](docs/compatibility.md#heartwood-forger) describes these tests.

Release builds must select one package at a time, for example `cargo build --release -p heartwood-crypto`. Do not use `--workspace` for releases: the test harness enables features that must not enter production packages.

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
| `packages/blockchain/` | A block's commit to the store, a node's start (first start, restart from the state tables, or replay from the genesis block) and the pool's online record in the store |
| `packages/p2p/` | The client side of the peer-to-peer protocol, not yet wired into a node: nes sessions over WebSocket, requests and reply checks, peer acceptance and verification, discovery, downloads and broadcasts |
| `packages/forger/` | The forger, not yet wired into a process: the slot loop its relay drives, the relay client with failover and call deadlines, block assembly and signing through a persisted signing record, the key file and the status attester |
| `packages/cli/` | The commands without the node: the colon aliases, instances, a built-in process supervisor and the role commands, the files of `network generate`, and a local devnet library with one loopback address per node |
| `tools/difftest/` | Compatibility harness and command-line tools |
| `tools/oracle/` | Reference execution, container builds and vector generation |
| `packages/*/tests/vectors/` | Committed vectors of each crate, with checksums and source metadata |
| `fuzz/` | Decoder fuzz targets, the state machine's apply and revert targets, the state's records, the pool, the peer-to-peer client's session, reply checks and download path, and the forger's internal bodies and status offers |

The current implementation uses the retained reference cryptography and transaction formats. Proposed protocol features in the whitepaper describe a separate target; they are not all implemented here.

## Compatibility

The reference is [solar-core-ref](https://github.com/iceroot-network/solar-core-ref), based on Solar Core 4.3.1 with a documented patch series. Over the seven crates that hold vector classes, the committed L0 vectors give 6,220 equal comparisons, 161 documented differences and 6 skipped records. L1 (random cases against the reference, one call each) and L2 (reference devnet chains replayed block by block through the store, with a state digest at every height and restarts by every start path) run as well; L3 and L4 remain planned.

See [compatibility and testing](docs/compatibility.md) for exact boundaries, oracle prerequisites, vector classes, release requirements and regeneration instructions. Oracle runs are isolated from the network. Standard Rust checks use committed vectors and do not require the reference checkout or Docker.

## Contribute

[Contributing](https://github.com/iceroot-network/.github/blob/prod/CONTRIBUTING.md). Work on `dev`. Production changes reach `prod` through a reviewed `dev` → `prod` pull request.

Heartwood Core is licensed under the [Apache License, Version 2.0](LICENSE). [NOTICE](NOTICE) preserves ARK Core and Solar Core attribution. External vector sources and checksums remain alongside the vector files.
