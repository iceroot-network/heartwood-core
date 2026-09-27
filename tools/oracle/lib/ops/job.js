"use strict";
// Oracle operation of the process-blocks job and the enqueueing: the reference's ProcessBlocksJob
// (packages/blockchain/src/process-blocks-job.ts), its BlockProcessor with the handlers
// (packages/blockchain/src/processor/) and Blockchain.enqueueBlocks
// (packages/blockchain/src/blockchain.ts), run headless on a chain session (lib/corpus.js's
// ReplaySession over lib/state.js). The classes are the reference's own: the service is made with
// Object.create and given its collaborators, the job, the processor and the handlers are resolved
// in the session's application, so every rule they apply is the reference's code. Only storage
// (the session's in-memory chain), time (a fixed clock), the queue, the state machine, the wake-up
// timer and the peer transport are stubbed, and the logger records what it is given.
//
//   chain.job {session, hexes, now, machineState?, started?, downloaded?}
//       the blocks of `hexes` (full block bytes in hex, or {hex, set}: the block with header
//       fields its listing states otherwise, of numberOfTransactions) as one job on the session's
//       chain at the
//       time `now` (milliseconds since the Unix epoch). Each block reaches the job as the peer
//       communicator hands a downloaded block on (its header, its transactions read by
//       fromBytesUnsafe with their block id, the peer's address) with its username set as the
//       enqueueing sets it. `machineState` is the state machine's state ("newBlock" by default),
//       `started` the state store's started flag (true by default), and `downloaded` (true by
//       default) sets the last downloaded block to the job's last block first, as a download does
//       before it enqueues; `lastDownloaded` (block bytes in hex) sets it to that block instead,
//       as a download of several jobs sets it to its own last block. -> the job (below)
//   chain.job {chain: {crypto, blocks}, jobs: [{hexes, now, machineState?, started?,
//              downloaded?}, ...]}
//       a whole session in one call, for the golden vectors: the reference's first start on the
//       chain definition `crypto` ({network, milestones, genesisBlock, exceptions}), the blocks of
//       `blocks` (hex) applied with the last downloaded block then at the tip, as a node that
//       downloaded them has it, then each job in turn -> {jobs: [job, ...]}
//   chain.job {enqueue: {blocks: [{height, numberOfTransactions}], lastDownloadedHeight,
//              maxLastBlocks?}, milestones?}
//       Blockchain.enqueueBlocks of the blocks with that last downloaded height, under the
//       milestones of `milestones` (JSON text; the devnet's by default) -> {jobs: [[height, ...],
//       ...]}: the heights of each job it pushes, in order, the trailing one included when empty
//
// A job is {results, committed, end, block?, broadcast, clearQueue, failed, futureSlot?,
// lastDownloaded, forked, rollback, missed, tip}:
//   results       one entry per block the processor answered, in order: {height, id, result,
//                 unchained?, missed?, line?}; `result` is the BlockProcessorResult's name,
//                 `unchained` the unchained handler's status, `missed` the missed-slot rows that
//                 processing the block recorded and `line` the session's digest line after an
//                 accepted block
//   committed     how many accepted blocks the job saved
//   end           "empty" (no block), "dropped" (the first block at or below the tip), "forkCheck"
//                 (the first block does not chain: the queue cleared, the last downloaded block
//                 reset and the fork check started), "broadcast" (the last result allows it),
//                 "fork" (a block was marked as forked) or "clear" (the queue cleared and the last
//                 downloaded block reset); `block` is the broadcast or forked block {height, id}
//   broadcast     whether the job broadcast its block (only when started and in newBlock)
//   clearQueue    whether a handler cleared the queue
//   failed        whether the job logged "Failed to process chunk of block chunk of blocks ...",
//                 its catch around the loop
//   futureSlot    the height of a block discarded because it takes a future slot
//   lastDownloaded, forked   the state store's last downloaded and forked blocks {height, id}
//                 after the job (null when unset); rollback its number of blocks to roll back
//   missed        every missed-slot row the job recorded, those of refused blocks included
//   tip           the last block {height, id} after the job
//
// The judge's processor mode (processorJudge) replays a corpus through the job, one block per
// job, and writes each step's line with the block's result beside it to ts-processor.jsonl.
//
// Loaded by lib/ops.js.

const fs = require("fs");
const path = require("path");

const { ReplaySession } = require("../corpus");
const { sessions: registry } = require("../pool");
const { settle } = require("../state");

// The unchained handler's statuses (a module-private enum of
// packages/blockchain/src/processor/handlers/unchained-handler.ts), by value.
const UNCHAINED = ["NotReadyToAcceptNewHeight", "AlreadyInBlockchain", "EqualToLastBlock", "GeneratorMismatch", "DoubleForging", "InvalidTimestamp"];

// The log lines a job's outcome reads.
const FAILED_CHUNK = "Failed to process chunk of block chunk of blocks";
const FUTURE_SLOT = /^Discarded block ([\d,]+) because it takes a future slot/;

let classes;
/** The reference's blockchain classes, loaded once. */
function referenceClasses(solar) {
    if (!classes) {
        const load = (sub) => require(path.join(solar.solarDir, "packages", "blockchain", "dist", sub));
        const processor = load("processor/block-processor");
        classes = {
            Blockchain: load("blockchain").Blockchain,
            ProcessBlocksJob: load("process-blocks-job").ProcessBlocksJob,
            BlockProcessor: processor.BlockProcessor,
            BlockProcessorResult: processor.BlockProcessorResult,
            UnchainedHandler: load("processor/handlers/unchained-handler").UnchainedHandler,
            ProcessBlockAction: load("actions/process-block").ProcessBlockAction,
        };
    }
    return classes;
}

/**
 * Run fn with the reference's clock at `nowMs`: Date.now() and a Date made without arguments (as
 * dayjs() makes it, which the slot calculator reads) give that time.
 */
async function atTime(nowMs, fn) {
    const RealDate = global.Date;
    class FixedDate extends RealDate {
        constructor(...args) {
            if (args.length === 0) super(nowMs);
            else super(...args);
        }

        static now() {
            return nowMs;
        }
    }
    global.Date = FixedDate;
    try {
        return await fn();
    } finally {
        global.Date = RealDate;
    }
}

/** A logger that keeps every line it is given, with its level. */
function recordingLogger() {
    const lines = [];
    const logger = { lines };
    for (const level of ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"]) {
        logger[level] = (message) => lines.push({ level, message: String(message) });
    }
    logger.suppressConsoleOutput = () => {};
    return logger;
}

const link = (data) => (data ? { height: data.height, id: data.id } : null);

// The header fields a job's block may state apart from its bytes, as a peer's listing can.
const SETTABLE = ["numberOfTransactions"];

/**
 * A block as the peer communicator hands it on (getPeerBlocks): the header's fields, each
 * transaction read by fromBytesUnsafe with the block's id, and the peer's address. `set` gives
 * header fields the listing states otherwise than the bytes.
 */
function downloaded(solar, bytes, set = {}) {
    const { Blocks } = solar.lib;
    const { data } = Blocks.Deserialiser.deserialise(bytes, false, { deserialiseTransactionsUnchecked: true });
    // A listed block always has its transactions' list, empty or not (the protobuf decode).
    data.transactions = data.transactions || [];
    for (const transaction of data.transactions) transaction.blockId = data.id;
    for (const [key, value] of Object.entries(set)) data[key] = value;
    data.ip = "192.0.2.1";
    return data;
}

/** A job's block: block bytes in hex, or {hex, set} with header fields the listing states otherwise. */
function jobBlock(item, i) {
    const given = typeof item === "string" ? { hex: item, set: {} } : item;
    if (!given || typeof given.hex !== "string" || !/^([0-9a-f]{2})+$/.test(given.hex)) throw new TypeError(`hexes[${i}] must be block bytes in lowercase hex, or {hex, set}`);
    const set = given.set || {};
    for (const [key, value] of Object.entries(set)) {
        if (!SETTABLE.includes(key) || !Number.isSafeInteger(value) || value < 0) throw new TypeError(`hexes[${i}].set.${key} is not a header field a listing may state (${SETTABLE.join(", ")})`);
    }
    return { bytes: Buffer.from(given.hex, "hex"), set };
}

/**
 * The job's world on the chain session `replay`: the reference's Blockchain service (made with
 * Object.create, its collaborators the session's or stubs), a BlockProcessor, and a proxy of the
 * session's application that resolves the handlers and the job with the service, the recording
 * logger and the session's pool.
 */
function jobWorld(solar, replay, { machineState = "newBlock" } = {}) {
    const cls = referenceClasses(solar);
    const chain = replay.chain;
    const app = chain.app;
    const Id = chain.pkgs.kernel.Container.Identifiers;
    const stubs = chain.stubs;
    const logger = recordingLogger();
    const seen = { queueClears: 0, handlerClears: 0, inHandler: false, forkBlocks: [], forkChecks: [], broadcasts: [], dispatched: [], unchained: undefined };

    // The pool: the session's own, or a stub without effect when the session has none.
    const bound = app.get(Id.PoolService);
    const pool = typeof bound.removeForgedTransaction === "function" ? bound : { ...bound, removeForgedTransaction: async () => {}, addTransaction: async () => {} };
    const queue = {
        push() {},
        pause() {},
        resume() {},
        clear() {
            seen.queueClears++;
            if (seen.inHandler) seen.handlerClears++;
        },
        stop: async () => {},
        size: () => 0,
        isRunning: () => false,
    };
    const stateMachine = { getState: () => machineState, transition: (event) => seen.dispatched.push(event) };
    const networkMonitor = {
        broadcastBlock: async (block) => seen.broadcasts.push(link(block.data)),
        // The monitor's fork check finds no fork: the job's background check then logs the
        // reference's "not chained" message.
        checkForFork: async () => 0,
        downloadBlockAtHeight: async () => undefined,
    };
    const blockRepository = { ...stubs.blockRepository, saveBlocks: (blocks) => stubs.databaseService.saveBlocks(blocks) };
    const transactionRepository = {
        ...stubs.transactionRepository,
        // SELECT id FROM transactions WHERE id IN (ids)
        async getForgedTransactionsIds(ids) {
            const wanted = new Set(ids);
            const out = [];
            for (const { t } of chain.store.allTransactions()) if (wanted.has(t.id)) out.push(t.id);
            return out;
        },
    };
    const triggers = app.get(Id.TriggerService);

    const proxy = Object.create(app);
    const blockchain = Object.create(cls.Blockchain.prototype);
    Object.assign(blockchain, {
        app: proxy,
        stateStore: chain.stateStore,
        databaseInteraction: chain.databaseInteraction,
        database: stubs.databaseService,
        blockRepository,
        missedBlockRepository: stubs.missedBlockRepository,
        pool,
        stateMachine,
        networkMonitor,
        peerRepository: { hasPeers: () => false },
        events: chain.events,
        logger,
        roundState: chain.roundState,
        walletRepository: chain.walletRepository,
        queue,
        stopped: false,
        booted: false,
        forkCheck: false,
        forking: false,
        missedBlocks: 0,
        lastCheckNetworkHealthTs: 0,
        lastCheckForkTs: 0,
        updating: false,
    });
    // The wake-up timer is time, which is stubbed: the accept handler's restart of it starts none.
    blockchain.resetWakeUp = () => {};
    const forkBlock = blockchain.forkBlock;
    blockchain.forkBlock = function (block, count) {
        seen.forkBlocks.push({ block: link(block.data), count });
        return forkBlock.call(this, block, count);
    };
    const checkForFork = blockchain.checkForFork;
    blockchain.checkForFork = function (blocks) {
        seen.forkChecks.push(blocks.map((b) => b.height));
        return checkForFork.call(this, blocks);
    };

    // What the service, the job and the handlers inject is replaced by the world's, wherever an
    // instance holds it.
    const patch = (instance) => {
        const own = (key) => Object.prototype.hasOwnProperty.call(instance, key);
        if (own("app")) instance.app = proxy;
        if (own("blockchain")) instance.blockchain = blockchain;
        if (own("logger")) instance.logger = logger;
        if (own("pool")) instance.pool = pool;
        if (own("stateMachine")) instance.stateMachine = stateMachine;
        if (own("networkMonitor")) instance.networkMonitor = networkMonitor;
        if (own("blockRepository")) instance.blockRepository = blockRepository;
        if (own("transactionRepository")) instance.transactionRepository = transactionRepository;
        if (instance instanceof cls.UnchainedHandler) {
            const check = instance.checkUnchainedBlock;
            instance.checkUnchainedBlock = function (block) {
                const status = check.call(this, block);
                seen.unchained = UNCHAINED[status];
                return status;
            };
        }
        return instance;
    };
    const processor = patch(app.resolve(cls.BlockProcessor));
    proxy.resolve = (constructor) => patch(app.resolve(constructor));
    proxy.get = (id) => (id === Id.BlockProcessor ? processor : app.get(id));

    return { cls, chain, replay, app, proxy, blockchain, processor, logger, seen, triggers, Id };
}

/**
 * Run the job of the downloaded blocks `blocks` (block data) in `world` as step `step` (apply or
 * reapply): each processed block is answered through the reference's processBlock action with the
 * reward read before it and the missed rows it records, and the session writes a digest line
 * after each accepted one.
 */
async function runJob(world, blocks, { step, started = true, downloaded: isDownload = true, lastDownloaded }) {
    const { cls, chain, replay, logger, seen, triggers } = world;
    const stateStore = chain.stateStore;
    const results = [];
    const before = chain.store.recorded.length;
    const stored = chain.store.height;
    const tip = stateStore.getLastBlock().data.height;

    for (const block of blocks) world.blockchain.setBlockUsername(block);
    if (lastDownloaded) stateStore.setLastDownloadedBlock(lastDownloaded);
    else if (isDownload && blocks.length) stateStore.setLastDownloadedBlock(blocks[blocks.length - 1]);
    // Every block the session applied is stored.
    stateStore.setLastStoredBlockHeight(chain.store.height);
    const wasStarted = stateStore.isStarted();
    stateStore.setStarted(started);

    const job = world.proxy.resolve(cls.ProcessBlocksJob);
    job.triggers = Object.create(triggers);
    job.triggers.call = async (name, args) => {
        if (name !== "processBlock") return triggers.call(name, args);
        const { block } = args;
        const height = block.data.height;
        let reward;
        if (block.data.username && chain.walletRepository.hasByUsername(block.data.username)) {
            const generator = chain.walletRepository.findByUsername(block.data.username);
            reward = (await chain.roundState.getRewardForBlockInRound(height, generator)).reward.toFixed();
        }
        const rows = chain.store.recorded.length;
        chain.store.recording = `${step}:${height}`;
        seen.unchained = undefined;
        seen.inHandler = true;
        let result;
        try {
            result = await new cls.ProcessBlockAction().execute(args);
        } finally {
            seen.inHandler = false;
            chain.store.recording = null;
        }
        // The node's storage calls let the handlers' deferred work run between blocks.
        await settle();
        const missed = chain.store.recorded.slice(rows).map(({ timestamp, height: h, username }) => ({ timestamp, height: h, username }));
        const entry = { height, id: block.data.id, result: cls.BlockProcessorResult[result] };
        if (seen.unchained !== undefined) entry.unchained = seen.unchained;
        if (missed.length) entry.missed = missed;
        if (result === cls.BlockProcessorResult.Accepted) entry.line = replay.emit(step, reward, missed);
        results.push(entry);
        return result;
    };
    job.setBlocks(blocks);

    // A corrupted state ends the reference's process; here it ends the job with a note.
    const exit = process.exit;
    let exited = false;
    process.exit = () => {
        exited = true;
        throw new Error("the job ended the process");
    };
    const clearsBefore = seen.queueClears;
    try {
        await job.handle();
    } catch (error) {
        if (!exited) throw error;
    } finally {
        process.exit = exit;
    }
    // The fork check of an unchained first block runs on the next turn.
    await settle();
    stateStore.setStarted(wasStarted);

    const jobClears = seen.queueClears - clearsBefore - seen.handlerClears;
    const messages = logger.lines.map((l) => l.message);
    const future = messages.map((m) => FUTURE_SLOT.exec(m)).find(Boolean);
    const last = results[results.length - 1];
    let end;
    let endBlock;
    if (exited) end = "corrupted";
    else if (!blocks.length) end = "empty";
    else if (seen.forkChecks.length) end = "forkCheck";
    else if (seen.forkBlocks.length) {
        end = "fork";
        endBlock = seen.forkBlocks[seen.forkBlocks.length - 1].block;
    } else if (jobClears > 0) end = "clear";
    else if (last && (last.result === "Accepted" || last.result === "DiscardedButCanBeBroadcasted")) {
        end = "broadcast";
        endBlock = { height: last.height, id: last.id };
    } else if (blocks[0].height <= tip) end = "dropped";
    else throw new Error("the job ended in no way the reference has");

    const out = {
        results,
        committed: chain.store.height - stored,
        end,
    };
    if (endBlock) out.block = endBlock;
    out.broadcast = seen.broadcasts.length > 0;
    out.clearQueue = seen.handlerClears > 0;
    out.failed = messages.some((m) => m.startsWith(FAILED_CHUNK));
    if (future) out.futureSlot = Number(future[1].replace(/,/g, ""));
    out.lastDownloaded = link(stateStore.getLastDownloadedBlock());
    const forked = stateStore.getForkedBlock();
    out.forked = forked ? link(forked.data) : null;
    out.rollback = stateStore.getNumberOfBlocksToRollback();
    out.missed = chain.store.recorded.slice(before).map(({ timestamp, height, username }) => ({ timestamp, height, username }));
    out.tip = link(stateStore.getLastBlock().data);
    return out;
}

/** Check the arguments of one job and run it on `replay`. */
async function sessionJob(solar, replay, args, step = "apply") {
    const { hexes, now, machineState, started, downloaded: isDownload, lastDownloaded } = args;
    if (!Array.isArray(hexes)) throw new TypeError("hexes must be a list of blocks");
    const given = hexes.map(jobBlock);
    if (!Number.isSafeInteger(now) || now < 0) throw new TypeError("now must be a time in milliseconds");
    if (machineState !== undefined && typeof machineState !== "string") throw new TypeError("machineState must be a state's name");
    if (started !== undefined && typeof started !== "boolean") throw new TypeError("started must be a boolean");
    if (isDownload !== undefined && typeof isDownload !== "boolean") throw new TypeError("downloaded must be a boolean");
    if (lastDownloaded !== undefined && (typeof lastDownloaded !== "string" || !/^([0-9a-f]{2})+$/.test(lastDownloaded))) throw new TypeError("lastDownloaded must be block bytes in lowercase hex");
    return atTime(now, async () => {
        const blocks = given.map(({ bytes, set }) => downloaded(solar, bytes, set));
        const world = jobWorld(solar, replay, { machineState });
        const last = lastDownloaded === undefined ? undefined : downloaded(solar, Buffer.from(lastDownloaded, "hex"));
        return runJob(world, blocks, { step, started, downloaded: isDownload, lastDownloaded: last });
    });
}

/**
 * Blockchain.enqueueBlocks of `blocks` ({height, numberOfTransactions}) with the last downloaded
 * height `lastDownloadedHeight`: the heights of each job it pushes.
 */
function enqueueCuts(solar, { blocks, lastDownloadedHeight, maxLastBlocks = 100 }) {
    if (!Array.isArray(blocks) || blocks.some((b) => !b || !Number.isSafeInteger(b.height) || !Number.isSafeInteger(b.numberOfTransactions))) {
        throw new TypeError("blocks must be [{height, numberOfTransactions}]");
    }
    if (!Number.isSafeInteger(lastDownloadedHeight)) throw new TypeError("lastDownloadedHeight must be a height");
    if (!Number.isSafeInteger(maxLastBlocks)) throw new TypeError("maxLastBlocks must be a count");
    const cls = referenceClasses(solar);
    const jobs = [];
    const blockchain = Object.create(cls.Blockchain.prototype);
    Object.assign(blockchain, {
        app: { resolve: (constructor) => Object.create(constructor.prototype) },
        queue: { push: (job) => jobs.push(job.getBlocks().map((b) => b.height)), resume() {} },
        stateStore: {
            getLastDownloadedBlock: () => ({ height: lastDownloadedHeight }),
            getMaxLastBlocks: () => maxLastBlocks,
        },
        walletRepository: { hasByPublicKey: () => false },
        forking: false,
    });
    blockchain.enqueueBlocks(blocks.map((b) => ({ height: b.height, numberOfTransactions: b.numberOfTransactions })));
    return { jobs };
}

module.exports = function jobOps(context) {
    const { solar, devnet, withConfig } = context;

    const session = (id) => {
        const s = registry.get(id);
        if (s.kind !== "chain") throw new Error(`session ${id} is not a chain session`);
        return s.replay;
    };
    const inSession = (s, fn) => s.chain.withConfig(() => devnet.restore(), fn);

    return {
        "chain.job": async (args) => {
            if (args.enqueue !== undefined) {
                return withConfig(args.milestones, () => enqueueCuts(solar, args.enqueue));
            }
            if (args.chain !== undefined) {
                const { crypto, blocks } = args.chain;
                if (!crypto || typeof crypto !== "object") throw new TypeError("chain.crypto must be the chain's crypto files");
                if (!Array.isArray(blocks)) throw new TypeError("chain.blocks must be block bytes in hex");
                if (!Array.isArray(args.jobs)) throw new TypeError("jobs must be a list of jobs");
                const s = new ReplaySession(solar, crypto);
                return inSession(s, async () => {
                    await s.start();
                    for (const [i, hex] of blocks.entries()) {
                        const r = await s.apply(Buffer.from(hex, "hex"), "apply");
                        if (!r.accepted) throw new Error(`chain block ${i + 1} was refused: ${r.error.class}: ${r.error.message}`);
                    }
                    // As a node that downloaded these blocks: the last downloaded block is the tip.
                    s.chain.stateStore.setLastDownloadedBlock(s.chain.lastBlock().data);
                    const jobs = [];
                    for (const job of args.jobs) jobs.push(await sessionJob(solar, s, job));
                    return { jobs };
                });
            }
            const s = session(args.session);
            return inSession(s, () => sessionJob(solar, s, args, args.step === "reapply" ? "reapply" : "apply"));
        },
    };
};

// The keys of a digest line that the processor mode compares with the plain replay's line.
const LINE_KEYS = ["h", "step", "id", "rw", "b", "s", "r", "m", "w", "sd", "cd"];

/**
 * replay-chain.js --processor: the corpus replayed through the reference's job, one block per job
 * (each block its own download, at its slot's time, in syncing), following the revert plan as the
 * plain replay does; each step's line goes to `out` with the block's result and the job's end
 * beside it. `plain` are the plain replay's lines, which every line must equal.
 * -> {ok, blocks, lines, jobs, notAccepted, differences}
 */
module.exports.processorJudge = async function processorJudge({ solar, corpus, out, plain }) {
    const LIMIT = 50;
    const session = new ReplaySession(solar, corpus.crypto);
    const write = fs.openSync(out, "w");
    const lines = [];
    const emit = (record) => {
        lines.push(record);
        fs.writeSync(write, JSON.stringify(record) + "\n");
    };
    const steps = new Map();
    for (const step of corpus.revertPlan.steps || []) {
        if (!steps.has(step.after)) steps.set(step.after, []);
        steps.get(step.after).push(step.blocks);
    }
    const notAccepted = [];
    let jobs = 0;
    session.chain.configure();
    const epochMs = new Date(corpus.crypto.milestones.find((m) => m.epoch !== undefined).epoch).getTime();
    const oneJob = async (hex, step) => {
        const bytes = Buffer.from(hex, "hex");
        const header = solar.lib.Blocks.Deserialiser.deserialise(bytes, true).data;
        jobs++;
        const job = await sessionJob(solar, session, { hexes: [hex], now: epochMs + header.timestamp * 1000, machineState: "syncing" }, step);
        const result = job.results[0];
        if (!result || result.result !== "Accepted") {
            const record = { h: header.height, step, id: result ? result.id : null, result: result ? result.result : null, end: job.end };
            notAccepted.push({ ...record, failed: job.failed });
            emit(record);
            return false;
        }
        emit({ ...result.line, result: result.result, end: job.end });
        return true;
    };
    try {
        await session.chain.withConfig(undefined, async () => {
            emit(await session.start());
            for (const block of corpus.blocks) {
                if (!(await oneJob(block.hex, "apply"))) return;
                for (const count of steps.get(block.h) || []) {
                    for (const line of await session.revert(count)) emit(line);
                    for (let h = block.h - count + 1; h <= block.h; h++) {
                        if (!(await oneJob(corpus.blocks[h - 2].hex, "reapply"))) return;
                    }
                }
            }
        });
    } finally {
        fs.closeSync(write);
    }
    const differences = [];
    if (plain) {
        const n = Math.max(plain.length, lines.length);
        for (let i = 0; i < n; i++) {
            const a = plain[i];
            const b = lines[i];
            const keys = a && b ? LINE_KEYS.filter((k) => a[k] !== b[k]) : ["the line"];
            if (keys.length) differences.push({ line: i + 1, h: (a || b).h, step: (a || b).step, keys });
        }
    }
    const ok = notAccepted.length === 0 && differences.length === 0;
    console.log(`replay-chain: processor mode: ${jobs} jobs of one block, ${lines.length} lines, ${notAccepted.length} not accepted, ${differences.length} lines differ from the plain replay`);
    return { ok, blocks: corpus.blocks.length, lines: lines.length, jobs, notAccepted: notAccepted.slice(0, LIMIT), differences: differences.slice(0, LIMIT) };
};

module.exports.enqueueCuts = enqueueCuts;
module.exports.sessionJob = sessionJob;
