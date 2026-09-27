"use strict";
// The reference's transaction pool, run headless in an application of lib/state.js: its own
// service provider registers the real Service, Mempool, SenderMempool, SenderState (over the
// state's copy-on-write wallet repositories), Processor, DynamicFeeMatcher, Query, Collator,
// ExpirationService, Storage (better-sqlite3 in a temporary file) and the four trigger actions.
// Replaced: the worker pool, by an in-process object that runs the reference's
// WorkerScriptHandler and rebuilds its answer as the pool's Worker does (fromBytesUnsafe with the
// returned id and addresses, isVerified), with the data crossing a JSON boundary as it does over
// IPC; the transaction broadcaster, by a recorder; and Date.now, by each call's time.
//
// The pool's options are the reference's defaults merged with the case's options (objects key by
// key, arrays and values replaced), with one worker and the storage in the session's directory.
//
// A pool lives in a HeadlessChain: over a wallet fixture (a pool session) or over a chain (a chain
// session opened with a pool). Sessions of both kinds share one registry, so the pool operations
// take either.

const fs = require("fs");
const path = require("path");

let pkg;
/** The reference's pool package. */
function poolPackage(solar) {
    if (!pkg) pkg = require(path.join(solar.solarDir, "packages", "pool", "dist"));
    return pkg;
}

let workerHandler;
/** The reference's worker script handler, which a pool worker runs in its own process. */
function workerScriptHandler(solar) {
    if (!workerHandler) {
        const { WorkerScriptHandler } = require(path.join(solar.solarDir, "packages", "pool", "dist", "worker-script-handler"));
        workerHandler = new WorkerScriptHandler();
    }
    return workerHandler;
}

/** `value` through a JSON boundary, as IPC carries it. */
function viaJson(value) {
    return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
}

/**
 * The worker pool, in process: the worker's getTransaction (fromBytes for bytes, fromData for
 * JSON), under the rules the processor runs under, and the transaction rebuilt from its answer.
 */
function inProcessWorkerPool(solar) {
    const { Transactions } = solar.lib;
    return {
        async getTransaction(transactionData) {
            const input = Buffer.isBuffer(transactionData) ? transactionData.toString("hex") : viaJson(transactionData);
            let answer;
            try {
                answer = viaJson(await workerScriptHandler(solar).getTransaction(input));
            } catch (error) {
                throw new Error(error && error.message !== undefined ? error.message : String(error));
            }
            const transaction = Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(answer.serialised, "hex"), answer.id, answer.addresses);
            transaction.isVerified = answer.isVerified;
            return transaction;
        },
    };
}

/** `given` merged into `base`: objects key by key, everything else replaced. */
function merge(base, given) {
    if (given === undefined) return base;
    if (base === null || typeof base !== "object" || Array.isArray(base) || given === null || typeof given !== "object" || Array.isArray(given)) return given;
    const out = { ...base };
    for (const [key, value] of Object.entries(given)) out[key] = merge(base[key], value);
    return out;
}

/** The pool's options: the reference's defaults, the case's options, one worker, the storage file. */
function poolOptions(solar, given, storage) {
    const defaults = require(path.join(solar.solarDir, "packages", "pool", "dist", "defaults")).defaults;
    const options = merge(viaJson(defaults), viaJson(given || {}));
    options.storage = storage;
    options.workerPool = { workerCount: 1 };
    return options;
}

/** The sessions of chains and pools, by id. */
class Sessions {
    constructor() {
        this.map = new Map();
        this.next = 1;
    }

    add(session) {
        const id = this.next++;
        this.map.set(id, session);
        return id;
    }

    get(id) {
        const session = this.map.get(id);
        if (!session) throw new Error(`no session ${id}`);
        return session;
    }

    delete(id) {
        this.get(id);
        this.map.delete(id);
    }
}

/** The registry the chain and pool operations share. */
const sessions = new Sessions();

/**
 * The pool of one application. `bind(solar, options, storage)` returns the binding a
 * HeadlessChain calls while it builds (options.bindPool); the chain then holds the pool as
 * `chain.pool`.
 */
class HeadlessPool {
    static binding(solar, givenOptions, storage) {
        return async (app, chain) => {
            const { Container, Providers } = chain.pkgs.kernel;
            const Id = Container.Identifiers;
            const options = poolOptions(solar, givenOptions, storage);
            const configuration = app.resolve(Providers.PluginConfiguration).from("@solar-network/pool", options);
            app.bind(Id.PluginConfiguration).toConstantValue(configuration).whenTargetTagged("plugin", "@solar-network/pool");
            const broadcasts = [];
            app.bind(Id.PeerTransactionBroadcaster).toConstantValue({
                broadcastTransactions: async (transactions) => {
                    broadcasts.push(transactions.map((t) => t.id));
                },
            });
            const provider = app.resolve(poolPackage(solar).ServiceProvider);
            await provider.register();
            app.rebind(Id.PoolWorkerPool).toConstantValue(inProcessWorkerPool(solar));
            chain.pool = new HeadlessPool(solar, app, chain, options, broadcasts);
            await chain.pool.boot();
        };
    }

    constructor(solar, app, chain, options, broadcasts) {
        this.solar = solar;
        this.app = app;
        this.chain = chain;
        this.options = options;
        this.broadcasts = broadcasts;
        this.Id = chain.pkgs.kernel.Container.Identifiers;
        this.processors = new Map();
        this.pendingRemovals = [];
    }

    /** Open the storage and start the service, as the pool's service provider boots them. */
    async boot() {
        this.storage = this.app.get(this.Id.PoolStorage);
        this.storage.boot();
        this.service = this.app.get(this.Id.PoolService);
        await this.service.boot();
        this.mempool = this.app.get(this.Id.PoolMempool);
        this.watch();
    }

    /**
     * Record, for each transaction the service adds to its mempool, whether a full pool evicted a
     * transaction and then refused it (notes.fullPoolEviction), and track the removals the
     * collator starts without awaiting them.
     */
    watch() {
        const service = this.service;
        const max = this.options.maxTransactionsInPool;
        this.evictions = [];
        // The eviction for a new transaction is the lowest-priority removal of a pool that is
        // exactly full; the removals of a pool above its maximum are a clean-up.
        let adding;
        const removeLowest = service.removeLowestPriorityTransaction.bind(service);
        service.removeLowestPriorityTransaction = async () => {
            if (!adding || service.getPoolSize() > max) return removeLowest();
            const before = this.ids(this.query().getAll());
            await removeLowest();
            const after = new Set(this.ids(this.query().getAll()));
            adding.evicted.push(...before.filter((id) => !after.has(id)));
        };
        const addToMempool = service.addTransactionToMempool.bind(service);
        service.addTransactionToMempool = async (transaction) => {
            adding = { evicted: [] };
            try {
                await addToMempool(transaction);
            } catch (error) {
                if (adding.evicted.length) this.evictions.push({ id: transaction.id, evicted: adding.evicted });
                throw error;
            } finally {
                adding = undefined;
            }
        };
        const removeTransaction = service.removeTransaction.bind(service);
        service.removeTransaction = (transaction) => {
            const done = removeTransaction(transaction);
            this.pendingRemovals.push(done);
            return done;
        };
    }

    /** Close the storage, as a stopping node does. */
    dispose() {
        this.service.dispose();
        this.storage.dispose();
    }

    query() {
        return this.app.get(this.Id.PoolQuery);
    }

    ids(transactions) {
        return Array.from(transactions).map((t) => t.id);
    }

    /** The processor of `channel` ("api" or "p2p"): each has its own, with its own cooldown. */
    processor(channel) {
        if (channel !== "api" && channel !== "p2p") throw new TypeError("channel must be api or p2p");
        if (!this.processors.has(channel)) this.processors.set(channel, this.app.get(this.Id.PoolProcessor));
        return this.processors.get(channel);
    }

    /**
     * Process `entries` ({hex} or {json}) on `channel` at the time `now` (milliseconds): the
     * processor's result, with notes.fullPoolEviction and the evictions when a full pool evicted a
     * transaction and then refused the new one.
     */
    async process(entries, now, channel = "api") {
        if (!Array.isArray(entries)) throw new TypeError("entries must be an array");
        if (!Number.isSafeInteger(now) || now < 0) throw new TypeError("now must be a time in milliseconds");
        const data = entries.map((e, i) => {
            if (e && typeof e.hex === "string") return Buffer.from(e.hex, "hex");
            if (e && e.json !== undefined) return viaJson(e.json);
            throw new TypeError(`entry ${i} is neither {hex} nor {json}`);
        });
        this.evictions = [];
        const realNow = Date.now;
        Date.now = () => now;
        let result;
        try {
            result = await this.processor(channel).process(data);
        } finally {
            Date.now = realNow;
        }
        await this.settle();
        const out = viaJson(result);
        if (this.evictions.length) out.notes = { fullPoolEviction: true, evictions: this.evictions };
        return out;
    }

    /** Wait for the removals the pool started without awaiting them. */
    async settle() {
        while (this.pendingRemovals.length) {
            const pending = this.pendingRemovals.splice(0);
            await Promise.all(pending);
        }
        await new Promise((resolve) => setImmediate(resolve));
    }

    transactions(hexes) {
        const { Transactions } = this.solar.lib;
        return (hexes || []).map((hex) => Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(hex, "hex")));
    }

    /** readdTransactions(previously forged, recheck). */
    async readd(forged, recheck) {
        await this.service.readdTransactions(this.transactions(forged), recheck === true);
        await this.settle();
    }

    async cleanUp() {
        await this.service.cleanUp();
        await this.settle();
    }

    async remove(hex) {
        const [transaction] = this.transactions([hex]);
        await this.service.removeTransaction(transaction);
        await this.settle();
    }

    async removeForged(hex) {
        const [transaction] = this.transactions([hex]);
        await this.service.removeForgedTransaction(transaction);
        await this.settle();
    }

    async flush() {
        await this.service.flush();
    }

    /** The collator's candidates; the failed transactions are removed before this returns. */
    async collate(validate, exclude) {
        const collator = this.app.get(this.Id.PoolCollator);
        const candidates = await collator.getBlockCandidateTransactions(validate === true, exclude || []);
        await this.settle();
        return this.ids(candidates);
    }

    /**
     * A new Storage and Service over the same file (and a new mempool and processors), then the
     * start's readdTransactions(), as a restarted node's pool does.
     */
    async restart() {
        const { Id } = this;
        const pool = poolPackage(this.solar);
        this.dispose();
        this.app.rebind(Id.PoolStorage).to(pool.Storage).inSingletonScope();
        this.app.rebind(Id.PoolMempool).to(pool.Mempool).inSingletonScope();
        this.app.rebind(Id.PoolService).to(pool.Service).inSingletonScope();
        this.processors = new Map();
        await this.boot();
        await this.service.readdTransactions();
        await this.settle();
    }

    /**
     * The online record: `delegate.version` set on each delegate's wallet for its round, then,
     * with `expireAt`, the expiry of a round start at that round (an entry of a round below it
     * minus 5 is forgotten). Returns the public keys that carry a version, sorted.
     */
    setOnline(delegates, expireAt) {
        const repository = this.chain.walletRepository;
        for (const { publicKey, round } of delegates || []) {
            if (!Number.isSafeInteger(round) || round < 1) throw new TypeError("a round must be a positive integer");
            repository.findByPublicKey(publicKey).setAttribute("delegate.version", { round, version: "4.3.1" });
        }
        if (expireAt !== undefined) {
            if (!Number.isSafeInteger(expireAt)) throw new TypeError("expireAt must be a round");
            for (const wallet of repository.allByUsername()) {
                if (!wallet.hasAttribute("delegate.version")) continue;
                const { round } = wallet.getAttribute("delegate.version");
                if (!round || round < expireAt - 5) wallet.forgetAttribute("delegate.version");
            }
        }
        return this.online();
    }

    online() {
        return this.chain.walletRepository
            .allByUsername()
            .filter((w) => w.hasAttribute("delegate.version"))
            .map((w) => w.getPublicKey())
            .sort();
    }

    /**
     * The pool's contents, as ids in the reference's orders: all (sender mempools in creation
     * order, each newest first), bySender ([address, ids oldest first] in creation order),
     * lowest and highest priority, the size, each sender's view of its own wallet ([address,
     * balance, nonce]), and the stored rows ([id, height] in storage order).
     */
    contents() {
        const query = this.query();
        const senders = [...this.mempool.senderMempools.entries()];
        return {
            all: this.ids(query.getAll()),
            bySender: senders.map(([address, mempool]) => [address, this.ids(mempool.getFromEarliest())]),
            lowest: this.ids(query.getFromLowestPriority()),
            highest: this.ids(query.getFromHighestPriority()),
            size: this.service.getPoolSize(),
            wallets: senders.map(([address, mempool]) => {
                const wallet = mempool.getWallet();
                return [address, wallet.getBalance().toFixed(), wallet.getNonce().toFixed()];
            }),
            stored: Array.from(this.storage.getAllTransactions()).map((row) => [row.id, row.height]),
        };
    }
}

module.exports = { HeadlessPool, merge, poolOptions, sessions };
