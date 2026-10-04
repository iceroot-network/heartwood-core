-- Schema version 3 of a node's store: version 2 and the indices of the listing routes' queries.
-- Indices only: no table, column or row changes, so a version-2 store gains them in place.
-- `blocks (timestamp)` needs none: its UNIQUE constraint indexes it.
CREATE INDEX IF NOT EXISTS blocks_username_height ON blocks (username, height);
CREATE INDEX IF NOT EXISTS missed_blocks_username_timestamp ON missed_blocks (username, timestamp);
CREATE INDEX IF NOT EXISTS transactions_type_group_type_block_height_sequence
    ON transactions (type_group, type, block_height, sequence);
