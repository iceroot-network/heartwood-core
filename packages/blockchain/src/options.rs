//! The blockchain options, with the reference's defaults: the database rollback bounds, the
//! missed-blocks lookback, and `networkStart`, which `--networkStart` sets. The relay reads them
//! from its configuration as the reference's option schema reads them (`heartwood-core`'s
//! options).

/// The database rollback's bounds (`databaseRollback`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseRollback {
    /// The most blocks a rollback rewinds (`maxBlockRewind`).
    pub max_block_rewind: u32,
    /// The blocks removed per step (`steps`).
    pub steps: u32,
}

/// The blockchain component's options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockchainOptions {
    /// The database rollback's bounds.
    pub database_rollback: DatabaseRollback,
    /// How far back the missed blocks are counted, in seconds (`missedBlocksLookback`).
    pub missed_blocks_lookback: u64,
    /// Whether the node starts a new network (`networkStart`).
    pub network_start: bool,
}

impl Default for BlockchainOptions {
    /// The reference's defaults.
    fn default() -> BlockchainOptions {
        BlockchainOptions {
            database_rollback: DatabaseRollback {
                max_block_rewind: 10_000,
                steps: 1_000,
            },
            missed_blocks_lookback: 2_592_000,
            network_start: false,
        }
    }
}

impl BlockchainOptions {
    /// The package whose options these are.
    pub const PACKAGE: &'static str = "@solar-network/blockchain";
}
