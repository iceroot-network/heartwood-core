-- Schema version 1 of a node's store. The chain tables have the reference's columns, in snake
-- case: ids, keys, hashes and signatures are lowercase hex text, amounts are INTEGER.

-- The chain's identity: schema_version, nethash, genesis_id and state_format.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE blocks (
    height                 INTEGER PRIMARY KEY,
    id                     TEXT    NOT NULL UNIQUE,
    version                INTEGER NOT NULL,
    timestamp              INTEGER NOT NULL UNIQUE,
    previous_block         TEXT    UNIQUE,
    number_of_transactions INTEGER NOT NULL,
    total_amount           INTEGER NOT NULL,
    total_fee              INTEGER NOT NULL,
    burned_fee             INTEGER NOT NULL,
    reward                 INTEGER NOT NULL,
    -- {"address": "amount", ...}, as the reference's jsonb column
    donations              TEXT    NOT NULL,
    payload_length         INTEGER NOT NULL,
    payload_hash           TEXT    NOT NULL,
    generator_public_key   TEXT    NOT NULL,
    username               TEXT,
    block_signature        TEXT    NOT NULL
);
CREATE INDEX blocks_generator_public_key ON blocks (generator_public_key);
CREATE INDEX blocks_username_timestamp ON blocks (username, timestamp);

CREATE TABLE transactions (
    id                TEXT    PRIMARY KEY,
    version           INTEGER NOT NULL,
    block_id          TEXT    NOT NULL REFERENCES blocks (id),
    block_height      INTEGER NOT NULL,
    sequence          INTEGER NOT NULL,
    timestamp         INTEGER NOT NULL,
    nonce             INTEGER NOT NULL,
    sender_id         TEXT    NOT NULL,
    sender_public_key TEXT    NOT NULL,
    recipient_id      TEXT,
    type              INTEGER NOT NULL,
    type_group        INTEGER NOT NULL,
    memo              BLOB,
    amount            INTEGER NOT NULL,
    fee               INTEGER NOT NULL,
    burned_fee        INTEGER NOT NULL,
    serialised        BLOB    NOT NULL,
    asset             TEXT,
    UNIQUE (sender_public_key, nonce)
);
CREATE INDEX transactions_block_id ON transactions (block_id);
CREATE INDEX transactions_block_height_sequence ON transactions (block_height, sequence);
CREATE INDEX transactions_sender_id ON transactions (sender_id);
CREATE INDEX transactions_sender_public_key ON transactions (sender_public_key);
CREATE INDEX transactions_recipient_id ON transactions (recipient_id);
CREATE INDEX transactions_timestamp ON transactions (timestamp);
CREATE INDEX transactions_type_group_type ON transactions (type_group, type);

CREATE TABLE rounds (
    round      INTEGER NOT NULL,
    public_key TEXT    NOT NULL,
    balance    INTEGER NOT NULL,
    PRIMARY KEY (round, public_key)
);

CREATE TABLE missed_blocks (
    timestamp INTEGER PRIMARY KEY,
    height    INTEGER NOT NULL,
    username  TEXT    NOT NULL
);

-- One record per wallet that is not empty, as heartwood-state writes it (JSON text).
CREATE TABLE wallets (
    address TEXT PRIMARY KEY,
    record  TEXT NOT NULL
);

-- The state of the tip besides the wallets (JSON text).
CREATE TABLE state (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    height   INTEGER NOT NULL,
    block_id TEXT    NOT NULL,
    record   TEXT    NOT NULL
);

-- What reverting each block above the genesis block needs (JSON text).
CREATE TABLE undo (
    height INTEGER PRIMARY KEY REFERENCES blocks (height),
    record TEXT    NOT NULL
);

-- The digest of every milestone entry the tip has reached.
CREATE TABLE milestone_entries (
    height INTEGER PRIMARY KEY,
    digest TEXT    NOT NULL
);

-- Node-local: the version each delegate's node last announced, and the round it did.
CREATE TABLE delegate_versions (
    public_key TEXT    PRIMARY KEY,
    round      INTEGER NOT NULL,
    version    TEXT    NOT NULL
);
