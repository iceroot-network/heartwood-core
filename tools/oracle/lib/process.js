"use strict";
// The reference's block processor, run headless on a chain session (lib/corpus.js's
// ReplaySession), as the patch tests run the reference's classes: Object.create of the real
// BlockProcessor (packages/blockchain/src/processor/block-processor.ts), whose handlers the
// session's own application resolves with their real injections. Only storage, time and the
// blockchain service's queue and timers are stubbed; every rule the processor and its handlers
// apply is the reference's code.
//
//   makeProcessor(solar, session) -> {
//       processor        the reference's BlockProcessor on the session's chain
//       rebuild(bytes, now, named = true)
//                        a block as a job hands it to the processor: decoded, its delegate name
//                        set as enqueueBlocks sets it (setBlockUsername, with the wallets of now;
//                        with `named` false, none, as for a block enqueued before its delegate
//                        registered), and rebuilt by BlockFactory.fromData with the clock at `now`
//                        (ms since the Unix epoch), so that its verification reads that time
//       process(block, now, named = true) -> {result, handler, unchained?, missed,
//                               lastDownloaded, forked, clearQueue, wakeUp, removed, reward?, line?}
//                        BlockProcessor.process of a rebuilt block (or of block bytes, rebuilt
//                        first): the result's name, the class of the handler the processor chose,
//                        the unchained handler's status, the missed rows written while it ran, the
//                        last downloaded block and the forked block after it ({height, id} and a
//                        height, or null), whether it cleared the queue and restarted the wake-up
//                        timer, and the ids it asked the pool to remove; after an accepted block,
//                        the reward and the digest line the session would write next (the session
//                        itself does not move on: the caller saves and emits)
//       setForged(ids)   ids the store holds besides its blocks' (a store ahead of the state)
//   }
//
// The stubs:
//   - the blockchain service (bound in the session's application, so the handlers get it too):
//     resetLastDownloadedBlock is the reference's own (blockchain.ts); clearQueue and resetWakeUp
//     only record that they ran (the queue and the wake-up timer are the job's and the service's);
//   - the pool: removeForgedTransaction and addTransaction record the ids, then call the session's
//     pool when it has one;
//   - the transaction repository: getForgedTransactionsIds answers from the session's stored
//     blocks at or below the state store's last stored height, and the ids of setForged;
//   - the clock: BlockFactory.fromData runs with the global Date fixed at `now`.

const path = require("path");

const { DigestChain, chainSections } = require("./digest");
const { settle } = require("./state");

// The reference's UnchainedBlockStatus, in its declaration order (unchained-handler.ts).
const UNCHAINED = ["NotReadyToAcceptNewHeight", "AlreadyInBlockchain", "EqualToLastBlock", "GeneratorMismatch", "DoubleForging", "InvalidTimestamp"];

// What the handlers did during the process call that is running, or null.
let capture = null;

let modules;
/**
 * The reference's processor, its handlers and its blockchain service, loaded once. The unchained
 * handler's private status check is wrapped so that the status it returns is recorded; the status
 * and the handler's outcome are unchanged.
 */
function processorModules(solar) {
    if (modules) return modules;
    const dist = path.join(solar.solarDir, "packages", "blockchain", "dist");
    const { BlockProcessor, BlockProcessorResult } = require(path.join(dist, "processor"));
    const handlers = require(path.join(dist, "processor", "handlers"));
    const { Blockchain } = require(path.join(dist, "blockchain"));
    const proto = handlers.UnchainedHandler.prototype;
    const check = proto.checkUnchainedBlock;
    if (typeof check !== "function") throw new Error("the reference's UnchainedHandler has no checkUnchainedBlock");
    proto.checkUnchainedBlock = function (block) {
        const status = check.call(this, block);
        if (capture) capture.unchained = UNCHAINED[status];
        return status;
    };
    modules = { BlockProcessor, BlockProcessorResult, Blockchain };
    return modules;
}

/** Run fn with the global Date showing `ms` (milliseconds since the Unix epoch) when read without arguments. */
function atTime(ms, fn) {
    const RealDate = global.Date;
    class FixedDate extends RealDate {
        constructor(...args) {
            if (args.length === 0) super(ms);
            else super(...args);
        }

        static now() {
            return ms;
        }
    }
    global.Date = FixedDate;
    try {
        return fn();
    } finally {
        global.Date = RealDate;
    }
}

/** `object[name]`, wrapped once so that each call records its transaction's id in `capture[list]`. */
function recordCalls(object, name, list) {
    const original = object[name];
    if (original && original.recording) return;
    const wrapped = async function (transaction) {
        if (capture) capture[list].push(transaction.id);
        if (original) return original.call(this, transaction);
    };
    wrapped.recording = true;
    object[name] = wrapped;
}

const processors = new WeakMap();

function makeProcessor(solar, session) {
    const chain = session.chain;
    if (processors.has(chain)) return processors.get(chain);
    const { BlockProcessor, BlockProcessorResult, Blockchain } = processorModules(solar);
    const { Blocks } = solar.lib;
    const app = chain.app;
    const Id = chain.pkgs.kernel.Container.Identifiers;
    const stateStore = chain.stateStore;
    let extraForged = [];

    // The blockchain service the session binds, as the processor and its handlers call it.
    const blockchain = chain.blockchain;
    blockchain.resetLastDownloadedBlock = function () {
        return Blockchain.prototype.resetLastDownloadedBlock.call({ stateStore, getLastBlock: () => stateStore.getLastBlock() });
    };
    blockchain.clearQueue = () => {
        if (capture) capture.clearQueue = true;
    };
    blockchain.resetWakeUp = () => {
        if (capture) capture.wakeUp = true;
    };
    const pool = app.get(Id.PoolService);
    recordCalls(pool, "removeForgedTransaction", "removed");
    recordCalls(pool, "addTransaction", "added");

    // The application as the processor sees it: resolve records the first handler class it makes.
    const recordingApp = new Proxy(app, {
        get(target, prop) {
            if (prop === "resolve") {
                return (cls) => {
                    if (capture && capture.handler === undefined) capture.handler = cls.name;
                    return target.resolve(cls);
                };
            }
            const value = Reflect.get(target, prop, target);
            return typeof value === "function" ? value.bind(target) : value;
        },
    });

    const transactionRepository = {
        async getForgedTransactionsIds(ids) {
            const wanted = new Set(ids);
            const stored = stateStore.getLastStoredBlockHeight();
            const found = [];
            for (const entry of chain.store.blocks) {
                if (entry.data.height > stored) continue;
                for (const t of entry.transactions) if (wanted.has(t.id)) found.push(t.id);
            }
            for (const id of extraForged) if (wanted.has(id)) found.push(id);
            return found;
        },
    };

    const registry = (() => {
        try {
            return app.getTagged(Id.TransactionHandlerRegistry, "state", "null");
        } catch {
            return app.getTagged(Id.TransactionHandlerRegistry, "state", "blockchain");
        }
    })();

    const processor = Object.create(BlockProcessor.prototype);
    Object.assign(processor, {
        app: recordingApp,
        blockchain,
        logger: chain.logger,
        roundState: chain.roundState,
        stateStore,
        transactionRepository,
        triggers: app.get(Id.TriggerService),
        transactionHandlerRegistry: registry,
        walletRepository: chain.walletRepository,
    });

    function rebuild(bytes, now, named = true) {
        const deserialised = Blocks.Deserialiser.deserialise(bytes);
        const data = { ...deserialised.data, transactions: deserialised.transactions.map((t) => t.data) };
        if (named) chain.pkgs.setBlockUsername.call({ walletRepository: chain.walletRepository }, data);
        const block = atTime(now, () => Blocks.BlockFactory.fromData(data));
        if (!block) throw new Error("BlockFactory returned no block");
        return block;
    }

    async function process(input, now, named = true) {
        const block = Buffer.isBuffer(input) ? rebuild(input, now, named) : input;
        const store = chain.store;
        const before = store.recorded.length;
        store.recording = `process:${block.data.height}`;
        capture = { handler: undefined, unchained: undefined, clearQueue: false, wakeUp: false, removed: [], added: [] };
        let result;
        let done;
        try {
            result = await processor.process(block);
            await settle();
        } finally {
            done = capture;
            capture = null;
            store.recording = null;
        }
        const missed = store.recorded.slice(before).map(({ timestamp, height, username }) => ({ timestamp, height, username }));
        const downloaded = stateStore.getLastDownloadedBlock();
        const forked = stateStore.getForkedBlock();
        const out = { result: BlockProcessorResult[result], handler: done.handler };
        if (done.unchained !== undefined) out.unchained = done.unchained;
        Object.assign(out, {
            missed,
            lastDownloaded: downloaded ? { height: downloaded.height, id: downloaded.id } : null,
            forked: forked ? forked.data.height : null,
            clearQueue: done.clearQueue,
            wakeUp: done.wakeUp,
            removed: done.removed,
        });
        if (out.result === "Accepted") {
            out.reward = block.data.reward.toFixed();
            const probe = new DigestChain();
            probe.cd = session.digests.cd;
            const last = chain.lastBlock();
            out.line = probe.line({ height: last.data.height, step: "apply", id: last.data.id, reward: out.reward }, chainSections(chain, missed));
        }
        return { out, block };
    }

    const made = {
        processor,
        rebuild,
        process,
        setForged(ids) {
            extraForged = [...ids];
        },
    };
    processors.set(chain, made);
    return made;
}

module.exports = { UNCHAINED, atTime, makeProcessor };
