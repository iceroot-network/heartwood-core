![IceRoot Heartwood Core](banner.png)

# Heartwood Core

The Rust implementation of IceRoot's node software. This repository currently provides cryptography, transaction and block codecs, network configuration, and a differential-testing harness. It does not yet provide a running node or REST server.

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
```

Release builds must select one package at a time, for example `cargo build --release -p heartwood-crypto`. Do not use `--workspace` for releases: the test harness enables features that must not enter production packages.

## Components

| Path | Purpose |
| --- | --- |
| `packages/crypto/` | Keys, signatures, addresses, codecs, transaction rules and milestones |
| `tools/difftest/` | Compatibility harness and command-line tools |
| `tools/oracle/` | Reference execution, container builds and vector generation |
| `packages/crypto/tests/vectors/` | Committed vectors with checksums and source metadata |
| `fuzz/` | Transaction and block decoder fuzz targets |

The current implementation uses the retained reference cryptography and transaction formats. Proposed protocol features in the whitepaper describe a separate target; they are not all implemented here.

## Compatibility

The reference is [solar-core-ref](https://github.com/iceroot-network/solar-core-ref), based on Solar Core 4.3.1 with a documented patch series. The committed L0 vectors contain 1,468 equal comparisons, 23 documented differences and 27 skipped records. Higher differential-testing levels remain planned.

See [compatibility and testing](docs/compatibility.md) for exact boundaries, oracle prerequisites, vector classes, release requirements and regeneration instructions. Oracle runs are isolated from the network. Standard Rust checks use committed vectors and do not require the reference checkout or Docker.

## Contribute

[Contributing](https://github.com/iceroot-network/.github/blob/prod/CONTRIBUTING.md). Work on `dev`. Production changes reach `prod` through a reviewed `dev` → `prod` pull request.

No project license has been selected yet. [NOTICE](NOTICE) preserves ARK Core and Solar Core attribution. External vector sources and checksums remain alongside the vector files.
