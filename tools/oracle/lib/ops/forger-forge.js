"use strict";
// Oracle operations of block assembly, over the reference's forger class:
//   forger.forge            {privateKey, aux, previousBlock: {id, height}, timestamp, reward,
//                           transactions, milestones?} -> {hex, id}: at the rule height of the
//                           previous block (the forger sets the network state's height before it
//                           assembles), each transaction read with TransactionFactory.fromBytesUnsafe,
//                           then the forger's Delegate.forge with the given aux (delegate.ts), which
//                           the block factory makes at the next height
//   forger.collateAndForge  pool.run's input (fixture, lastBlockHeight, milestones?, options?,
//                           online?, steps) with {privateKey, aux, previousBlock: {id}, timestamp,
//                           reward} -> {collated, removed, hex, id}: the pool session's steps, then
//                           a forger's collation (getBlockCandidateTransactions(true, []), as the
//                           relay collates for a caller in remoteAccess), the transactions the
//                           collation removed from the pool (sorted ids), and Delegate.forge on the
//                           collated transactions, each read back from its bytes with
//                           fromBytesUnsafe as the forger reads the relay's answer, on the pool's
//                           tip; with notes.fullPoolEviction when a step's full pool evicted a
//                           transaction and then refused the new one, where Heartwood's pool
//                           evicts nothing (a documented difference of the pool)
// Loaded by lib/ops.js.

const path = require("path");

const { sessions } = require("../pool");
const poolOps = require("./pool");

module.exports = function forgerForgeOps(context) {
    const { solar, withConfig, atHeight, hexBuffer, safeInteger, amount } = context;
    const { Blocks, Transactions } = solar.lib;
    const pool = poolOps(context);
    let Delegate;

    /** The reference's forger Delegate class, sharing the oracle's @solar-network/crypto. */
    function delegateClass() {
        if (!Delegate) {
            const forger = path.join(solar.solarDir, "packages", "forger");
            const crypto = require(require.resolve("@solar-network/crypto", { paths: [forger] }));
            if (crypto !== solar.lib) throw new Error("@solar-network/forger loaded a second copy of @solar-network/crypto");
            Delegate = require(path.join(forger, "dist", "delegate")).Delegate;
        }
        return Delegate;
    }

    /** Delegate.forge on transaction bytes `hexes` after the block `previousBlock`, as {hex, id}. */
    function forge({ privateKey, aux, previousBlock, timestamp, reward }, hexes) {
        if (typeof privateKey !== "string" || !/^[0-9a-f]{64}$/.test(privateKey)) throw new TypeError("privateKey must be 64 hex digits");
        if (!previousBlock || typeof previousBlock.id !== "string") throw new TypeError("previousBlock.id must be text");
        safeInteger(previousBlock.height, "previousBlock.height");
        safeInteger(timestamp, "timestamp");
        amount(reward, "reward");
        const transactions = hexes.map((hex) => Transactions.TransactionFactory.fromBytesUnsafe(hexBuffer(hex, "transaction")).data);
        const delegate = new (delegateClass())(privateKey);
        const block = delegate.forge(transactions, { aux: hexBuffer(aux, "aux"), previousBlock, timestamp, reward });
        if (!block) throw new Error("the block factory made no block");
        const bytes = Blocks.Serialiser.serialiseWithTransactions({ ...block.data, transactions: block.transactions.map((t) => t.data) });
        return { hex: bytes.toString("hex"), id: block.data.id };
    }

    return {
        "forger.forge": ({ milestones, transactions = [], ...input }) => {
            if (!Array.isArray(transactions)) throw new TypeError("transactions must be an array");
            return withConfig(milestones, () => atHeight(input.previousBlock.height, () => forge(input, transactions)));
        },

        "forger.collateAndForge": async ({ fixture, lastBlockHeight, milestones, options, online, steps = [], ...forger }) => {
            if (!Array.isArray(steps)) throw new TypeError("steps must be an array");
            const { session } = await pool["pool.open"]({ fixture, lastBlockHeight, milestones, options, online });
            try {
                let evicted = false;
                for (const { op, ...args } of steps) {
                    const run = pool[`pool.${op}`];
                    if (typeof run !== "function" || op === "run" || op === "open" || op === "close") throw new TypeError(`unknown pool step ${op}`);
                    const result = await run({ session, ...args });
                    evicted = evicted || Boolean(result && result.notes && result.notes.fullPoolEviction);
                }
                const { chain } = sessions.get(session);
                const tip = chain.stateStore.getLastHeight();
                return await withConfig(milestones, () =>
                    atHeight(tip, async () => {
                        const headless = chain.pool;
                        const before = headless.contents().all;
                        const collator = headless.app.get(headless.Id.PoolCollator);
                        const candidates = await collator.getBlockCandidateTransactions(true, []);
                        await headless.settle();
                        const after = new Set(headless.contents().all);
                        const removed = before.filter((id) => !after.has(id)).sort();
                        const hexes = candidates.map((t) => Buffer.from(t.serialised).toString("hex"));
                        const previousBlock = { id: forger.previousBlock && forger.previousBlock.id, height: tip };
                        const block = forge({ ...forger, previousBlock }, hexes);
                        const notes = evicted ? { notes: { fullPoolEviction: true } } : {};
                        return { collated: candidates.map((t) => t.id), removed, ...block, ...notes };
                    }),
                );
            } finally {
                await pool["pool.close"]({ session });
            }
        },
    };
};
