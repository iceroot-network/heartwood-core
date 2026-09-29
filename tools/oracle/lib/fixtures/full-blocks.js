"use strict";
// Inputs of the served answers (V32) that are too large to write by hand: the store of full blocks
// that makes getBlocks reach its 20 MiB cut. A block of the store holds 150 transfers of 256
// recipients with a 255-byte memo, the most a block of the one-node corpus's milestones allows,
// which is about 1.2 MB of bytes and 2.4 MB as the hex text the reference's store returns. The
// transactions are made by the reference's builders under the corpus's milestones and are never
// forged, verified or applied: the store's blocks are a row each, with a header made from the
// block's height alone, and all of a block's transactions are the one transaction.
//
// `syntheticStore` is what a record's `store.synthetic` holds; lib/ops/p2p-controller.js and
// Heartwood's subject build the rows from it in the same way (`syntheticRow`).

const RECIPIENTS = 256;
const MEMO_BYTES = 255;
const PER_BLOCK = 150;

/** The hex of the one transaction of the synthetic store, built under the milestones now set. */
function fullTransaction(solar, { recipients = RECIPIENTS, memoBytes = MEMO_BYTES } = {}) {
    const { Identities, Transactions } = solar.lib;
    let builder = Transactions.BuilderFactory.transfer();
    for (let i = 0; i < recipients; i++) {
        builder = builder.addTransfer(Identities.Address.fromPassphrase(`full block recipient ${i}`), String(1_000 + i));
    }
    const transaction = builder
        .memo("m".repeat(memoBytes))
        .nonce("1")
        .fee("100000000")
        .sign("full block sender")
        .build();
    return transaction.serialised.toString("hex");
}

/**
 * A store of `count` blocks from height `from`, each with `perBlock` copies of the transaction
 * `transaction` (hex).
 */
function syntheticStore({ from, count, transaction, perBlock = PER_BLOCK }) {
    return { from, count, perBlock, transaction };
}

/** The row of the block at `height` of a synthetic store, as the store's header fields. */
function syntheticRow(store, height) {
    const bytes = store.transaction.length / 2;
    const padded = (n, width) => n.toString(16).padStart(width, "0");
    return {
        id: padded(height, 64),
        version: 0,
        timestamp: height * 8,
        previousBlock: padded(height - 1, 64),
        height,
        numberOfTransactions: store.perBlock,
        totalAmount: String(store.perBlock * 256 * 1_000),
        totalFee: String(store.perBlock * 100_000_000),
        reward: "200000000",
        payloadLength: store.perBlock * (bytes + 4),
        payloadHash: padded(height + 1_000, 64),
        generatorPublicKey: `02${padded(height, 64)}`,
        blockSignature: padded(height, 128),
    };
}

module.exports = { MEMO_BYTES, PER_BLOCK, RECIPIENTS, fullTransaction, syntheticRow, syntheticStore };
