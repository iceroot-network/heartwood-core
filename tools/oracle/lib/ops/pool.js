"use strict";
// Oracle operations of the reference's transaction pool: pool.open, pool.process, pool.setOnline,
// pool.readd, pool.cleanUp, pool.remove, pool.removeForged, pool.flush, pool.blockApplied,
// pool.contents, pool.collate, pool.restart, pool.tip, pool.close and pool.run. Loaded by
// lib/ops.js.
//
// Each session binds the reference's own pool classes in an application over a wallet fixture or
// a chain session, with the real storage in a temporary file; only the worker transport, the
// broadcaster and the clock are replaced (lib/pool.js). A fixture session has no blocks: its last
// block is a placeholder at lastBlockHeight, which pool.tip moves. Every pool operation takes a
// pool session or a chain session opened with a pool (chain.open {pool: {options}}).
//
// Contents (after every operation that changes the pool): {all, bySender, lowest, highest, size,
// wallets, stored}, see lib/pool.js. Transactions are hex; ids are lowercase hex.
//
//   pool.open {fixture, lastBlockHeight, milestones?, options?, online?: [{publicKey, round}]}
//       -> {session, online}: a fixture's state (the fixture module of tools/difftest, as
//          state.applyTransaction reads it) with a pool; `milestones` (JSON text) rules every
//          operation of the session
//   pool.process {session, entries: [{hex} | {json}], now, channel?}
//       the processor of `channel` ("api" by default, or "p2p"; each has its own cooldown) at the
//       time `now` in milliseconds -> {accept, broadcast, invalid, excess, errors?, notes?}; with
//       notes.fullPoolEviction and notes.evictions when a full pool evicted a transaction and then
//       refused the new one
//   pool.setOnline {session, delegates: [{publicKey, round}], expireAt?} -> {online}
//   pool.readd {session, forged?: [hex], recheck?}      readdTransactions -> {notes?, contents}
//   pool.cleanUp {session}                              cleanUp -> {contents}
//   pool.remove {session, hex}, pool.removeForged {session, hex} -> {contents}
//   pool.flush {session}                                flush -> {contents}
//   pool.collate {session, validate, exclude?: [id]}    -> {ids, contents}
//   pool.restart {session}      a new Storage and Service over the same file, then the start's
//                               re-add -> {notes?, contents}
//       A re-add (pool.readd, pool.restart) has notes.fullPoolEviction and notes.evictions, as
//       pool.process has, when its full pool evicted a transaction and then refused the re-added
//       one, and then notes.attempts: each transaction the re-add tried to add back, in order, as
//       [id, entered], and [id, false, true] for a refusal that came after an eviction
//   pool.tip {session, height}  a fixture session's last block moved to `height` -> {contents}
//   pool.blockApplied {session, hex}   a chain session: the block applied (as chain.apply), then
//       as a synced node does: removeForgedTransaction for each of its transactions, cleanUp,
//       then the queue drain's readdTransactions(none, true)
//       -> {accepted, height, id, reward?, missed, line?, stage?, error?, contents}
//   pool.revert {session, count}   a chain session: `count` blocks reverted as the blockchain's
//       removeBlocks does, then readdTransactions of their transactions, in chain order
//       -> {lines, contents}
//   pool.contents {session} -> {contents};   pool.close {session} -> {closed: true}
//   pool.run {fixture, lastBlockHeight, milestones?, options?, online?, steps: [{op, ...}]}
//       a whole pool session in one call, for the golden vectors and L1: `op` is process,
//       setOnline, readd, cleanUp, remove, removeForged, flush, collate, restart or tip, with
//       that operation's arguments -> {results: [{result, contents}]}, one per step

const path = require("path");

const { HeadlessPool, sessions } = require("../pool");
const { FixtureState, newDataDir } = require("../state");

module.exports = function poolOps(context) {
    const { solar, devnet, withConfig, atHeight, safeInteger } = context;

    /** Run fn(pool, session) under the session's rules: a fixture's at its tip, a chain's own. */
    const inPool = async (id, fn) => {
        const session = sessions.get(id);
        if (session.kind === "pool") {
            return withConfig(session.milestones, () => atHeight(session.chain.stateStore.getLastHeight(), () => fn(session.chain.pool, session)));
        }
        if (session.kind === "chain") {
            const chain = session.replay.chain;
            if (!chain.pool) throw new Error(`chain session ${id} has no pool: open it with {pool: {options}}`);
            return chain.withConfig(
                () => devnet.restore(),
                () => fn(chain.pool, session),
            );
        }
        throw new Error(`session ${id} is not a pool or a chain`);
    };

    const withContents = async (id, fn) => inPool(id, async (pool, session) => ({ ...(await fn(pool, session)), contents: pool.contents() }));

    const open = async ({ fixture, lastBlockHeight, milestones, options, online }) => {
        safeInteger(lastBlockHeight, "lastBlockHeight");
        if (milestones !== undefined && typeof milestones !== "string") throw new TypeError("milestones must be JSON text");
        return withConfig(milestones, () =>
            atHeight(lastBlockHeight, async () => {
                const storage = path.join(newDataDir("pool"), "pool.sqlite");
                const state = new FixtureState(solar, { bindPool: HeadlessPool.binding(solar, options, storage) });
                await state.load(fixture, lastBlockHeight);
                const pool = state.chain.pool;
                const onlineNow = pool.setOnline(online || []);
                const id = sessions.add({ kind: "pool", chain: state.chain, milestones });
                return { session: id, online: onlineNow };
            }),
        );
    };

    /** Move a fixture session's placeholder last block to `height`. */
    const moveTip = (session, height) => {
        if (session.kind !== "pool") throw new Error("only a fixture session's tip moves: apply blocks to a chain session");
        safeInteger(height, "height");
        const store = session.chain.stateStore;
        store.lastBlocks = store.lastBlocks.clear();
        store.setLastBlock({ data: { height, id: "00".repeat(32), timestamp: 0 }, transactions: [] });
    };

    /**
     * Record in `attempts` each transaction that `service`, the service of `pool`, tries to add back
     * to its mempool: [id, entered], or [id, false, true] for a refusal after the full pool evicted a
     * transaction (the pool's watch notes the eviction). Returns the function that stops recording.
     */
    const recordAttempts = (pool, service, attempts) => {
        const add = service.addTransactionToMempool;
        service.addTransactionToMempool = async (transaction) => {
            const noted = pool.evictions.length;
            try {
                await add(transaction);
            } catch (error) {
                attempts.push(pool.evictions.length > noted ? [transaction.id, false, true] : [transaction.id, false]);
                throw error;
            }
            attempts.push([transaction.id, true]);
        };
        return () => {
            service.addTransactionToMempool = add;
        };
    };

    /** The notes of a re-add whose full pool evicted a transaction and then refused the re-added one. */
    const readdNotes = (pool, attempts) =>
        pool.evictions.length ? { notes: { fullPoolEviction: true, evictions: pool.evictions, attempts } } : {};

    /** One step of pool.run, or the body of the operation of the same name. */
    const step = async (pool, session, { op, ...args }) => {
        switch (op) {
            case "process":
                return pool.process(args.entries, args.now, args.channel);
            case "setOnline":
                return { online: pool.setOnline(args.delegates, args.expireAt) };
            case "readd": {
                const attempts = [];
                pool.evictions = [];
                const stop = recordAttempts(pool, pool.service, attempts);
                try {
                    await pool.readd(args.forged, args.recheck);
                } finally {
                    stop();
                }
                return readdNotes(pool, attempts);
            }
            case "cleanUp":
                await pool.cleanUp();
                return {};
            case "remove":
                await pool.remove(args.hex);
                return {};
            case "removeForged":
                await pool.removeForged(args.hex);
                return {};
            case "flush":
                await pool.flush();
                return {};
            case "collate":
                return { ids: await pool.collate(args.validate, args.exclude) };
            case "restart": {
                // The restart boots a new service, whose watch starts the evictions afresh; its re-add's
                // attempts are recorded from then on.
                const attempts = [];
                let stop = () => {};
                pool.watch = function watch() {
                    HeadlessPool.prototype.watch.call(this);
                    stop = recordAttempts(this, this.service, attempts);
                };
                try {
                    await pool.restart();
                } finally {
                    delete pool.watch;
                    stop();
                }
                return readdNotes(pool, attempts);
            }
            case "tip":
                moveTip(session, args.height);
                return {};
            default:
                throw new TypeError(`unknown pool step ${op}`);
        }
    };

    const byStep = (op) => async ({ session, ...args }) => withContents(session, (pool, s) => step(pool, s, { op, ...args }));

    return {
        "pool.open": open,

        "pool.process": async ({ session, entries, now, channel }) => inPool(session, (pool) => pool.process(entries, now, channel)),

        "pool.setOnline": async ({ session, delegates, expireAt }) => inPool(session, (pool) => ({ online: pool.setOnline(delegates, expireAt) })),

        "pool.readd": byStep("readd"),
        "pool.cleanUp": byStep("cleanUp"),
        "pool.remove": byStep("remove"),
        "pool.removeForged": byStep("removeForged"),
        "pool.flush": byStep("flush"),
        "pool.collate": byStep("collate"),
        "pool.restart": byStep("restart"),
        "pool.tip": byStep("tip"),

        "pool.contents": async ({ session }) => inPool(session, (pool) => ({ contents: pool.contents() })),

        "pool.blockApplied": async ({ session, hex }) =>
            withContents(session, async (pool, s) => {
                if (s.kind !== "chain") throw new Error("pool.blockApplied needs a chain session");
                const r = await s.replay.apply(Buffer.from(hex, "hex"), "apply");
                const out = { accepted: r.accepted, height: r.height, id: r.id, missed: r.missed };
                if (r.reward !== undefined) out.reward = r.reward;
                if (!r.accepted) return { ...out, stage: r.stage, error: r.error };
                for (const transaction of r.block.transactions) await pool.service.removeForgedTransaction(transaction);
                await pool.cleanUp();
                await pool.readd([], true);
                return { ...out, line: r.line };
            }),

        "pool.revert": async ({ session, count }) =>
            withContents(session, async (pool, s) => {
                if (s.kind !== "chain") throw new Error("pool.revert needs a chain session");
                safeInteger(count, "count");
                const store = s.replay.chain.store;
                const tip = store.height;
                const forged = [];
                for (let h = tip - count + 1; h <= tip; h++) {
                    const entry = store.blocks[h - 1];
                    if (!entry) throw new RangeError(`no stored block ${h}`);
                    for (const t of entry.transactions) forged.push(t.serialised.toString("hex"));
                }
                const lines = await s.replay.revert(count);
                await pool.readd(forged, false);
                return { lines };
            }),

        "pool.close": async ({ session }) => {
            const s = sessions.get(session);
            if (s.kind !== "pool") throw new Error("pool.close closes a pool session; chain.close closes a chain");
            s.chain.pool.dispose();
            sessions.delete(session);
            return { closed: true };
        },

        "pool.run": async ({ fixture, lastBlockHeight, milestones, options, online, steps }) => {
            if (!Array.isArray(steps)) throw new TypeError("steps must be an array");
            const { session } = await open({ fixture, lastBlockHeight, milestones, options, online });
            try {
                const results = [];
                for (const s of steps) {
                    results.push(await withContents(session, async (pool, sess) => ({ result: await step(pool, sess, s) })));
                }
                return { results };
            } finally {
                const s = sessions.get(session);
                s.chain.pool.dispose();
                sessions.delete(session);
            }
        },
    };
};
