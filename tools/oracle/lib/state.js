"use strict";
// The reference implementation's own state machine, run headless in this process: the real
// Wallet, WalletRepository (with its indexers), the transaction handlers and their registry,
// StateStore, BlockState, DposState, RoundState, DposPreviousRoundState (over clone
// repositories), StateBuilder and DatabaseInteraction, wired by the reference's own service
// providers in its own IoC container. Only storage is replaced: the database service, the block,
// transaction and missed-block repositories and the two history services answer from an
// in-memory chain (ChainStore below) with the query semantics the state code relies on.
//
//   start      the reference's first start: DatabaseInteraction.initialise() (genesis decoded
//              and validated, the issued amount, the genesis block stored as the last block),
//              then as the blockchain's Initialise action does it at height 1: delete the round
//              rows of rounds 1 and 2, StateBuilder.run(), RoundState.restore().
//   apply      a block as a relay receives it: Blocks.Deserialiser, BlockFactory.fromData, the
//              blockchain's setBlockUsername, the reward of the block in its round (read before
//              applying, as the block processor does), then DatabaseInteraction.applyBlock
//              (missed slots, block state, round state). An accepted block is stored; a refused
//              one leaves the last block unchanged, as the accept handler does.
//   revert     n blocks as the blockchain's removeBlocks does: DatabaseInteraction.revertBlock
//              of the last block, the new last block rebuilt from the stored data, one block at a
//              time; the reverted blocks leave the store after the last revert.
//
// Everything a node keeps outside the state (the pool, P2P, events, the API) is absent. The
// forger directory (CORE_PATH_TEMP) is an empty directory, so no delegate carries a version.

const fs = require("fs");
const os = require("os");
const path = require("path");

/** An error the reference raised while starting: the start is refused. */
class StartRefused extends Error {
    constructor(reason, cause) {
        super(cause && cause.message ? `${reason}: ${cause.message}` : reason);
        this.name = "StartRefused";
        this.cause = cause;
    }
}

let packages;
/** The reference's kernel, state, transactions and blockchain packages, loaded once. */
function referencePackages(solar) {
    if (packages) return packages;
    const load = (pkg, sub = "") => require(path.join(solar.solarDir, "packages", pkg, "dist", sub));
    const kernel = load("kernel");
    const cryptoOfKernel = require(require.resolve("@solar-network/crypto", { paths: [path.join(solar.solarDir, "packages", "kernel")] }));
    if (cryptoOfKernel !== solar.lib) throw new Error("@solar-network/kernel loaded a second copy of @solar-network/crypto");
    packages = {
        kernel,
        stateProvider: load("state", "service-provider"),
        state: load("state"),
        transactions: solar.transactions(),
        setBlockUsername: load("blockchain", "blockchain").Blockchain.prototype.setBlockUsername,
    };
    return packages;
}

const silentLogger = {
    emergency() {},
    alert() {},
    critical() {},
    error() {},
    warning() {},
    notice() {},
    info() {},
    debug() {},
    suppressConsoleOutput() {},
};

/** An event dispatcher that drops wallet events and counts the others by name. */
function countingDispatcher() {
    const counts = new Map();
    const note = (event) => {
        const name = String(event);
        if (name.startsWith("wallet.")) return;
        counts.set(name, (counts.get(name) || 0) + 1);
    };
    return {
        counts,
        dispatch: async (event) => note(event),
        dispatchSync: (event) => note(event),
        dispatchSeq: async (event) => note(event),
        listen: () => () => {},
        listenMany: () => new Map(),
        listenOnce: () => {},
        forget: () => true,
        forgetMany: () => {},
        flush: () => {},
        getListeners: () => [],
        hasListeners: () => false,
        countListeners: () => 0,
    };
}

/**
 * The in-memory chain: the blocks and transactions a node would have in PostgreSQL, the round
 * rows, and every missed-slot row recorded. Blocks are stored as the database stores them (the
 * block data with username, burned fee and donations, and each transaction's serialised bytes).
 */
class ChainStore {
    constructor(lib) {
        this.lib = lib;
        this.blocks = []; // [{ data, transactions: [{ id, serialised, sequence }] }], by height from 1
        this.rounds = new Map(); // round -> [{ publicKey, balance (string), round }] as saved
        this.missed = []; // every row recorded, with the step that recorded it
        this.recording = null; // the tag of the rows recorded now
        this.history = []; // transaction data outside the stored blocks (a fixture's history)
    }

    get height() {
        return this.blocks.length;
    }

    save(block) {
        const { data } = block;
        if (data.height !== this.blocks.length + 1) throw new Error(`store: block ${data.height} does not follow ${this.blocks.length}`);
        const stored = { ...data };
        delete stored.transactions;
        this.blocks.push({
            data: stored,
            transactions: block.transactions.map((t, sequence) => ({ id: t.id, serialised: Buffer.from(t.serialised), sequence })),
        });
    }

    deleteFrom(height) {
        this.blocks.length = Math.min(this.blocks.length, height - 1);
    }

    /** The block data a block query returns (username absent when null, as the model converter). */
    blockData(entry, withTransactions) {
        const data = { ...entry.data };
        if (data.username === null || data.username === undefined) delete data.username;
        if (withTransactions) data.transactions = entry.transactions.map((t) => this.lib.Transactions.TransactionFactory.fromBytesUnsafe(t.serialised, t.id).data);
        return data;
    }

    /** A stored transaction's data, as the model converter builds it from its row. */
    transactionData(entry, t) {
        const data = this.lib.Transactions.TransactionFactory.fromBytesUnsafe(t.serialised, t.id).data;
        data.blockId = entry.data.id;
        data.blockHeight = entry.data.height;
        data.sequence = t.sequence;
        return data;
    }

    *allTransactions() {
        for (const entry of this.blocks) {
            for (const t of entry.transactions) yield { entry, t };
        }
    }
}

/** Whether transaction data matches one criteria object of the history services. */
function matchesCriteria(data, criteria) {
    for (const [key, value] of Object.entries(criteria)) {
        if (value === undefined) continue;
        if (key === "blockHeight") {
            if (value.from !== undefined && data.blockHeight < value.from) return false;
            if (value.to !== undefined && data.blockHeight > value.to) return false;
            continue;
        }
        if (!["typeGroup", "type", "senderId", "senderPublicKey", "id"].includes(key)) {
            throw new Error(`history stub: criteria key ${key} is not supported`);
        }
        if (data[key] !== value) return false;
    }
    return true;
}

function sortRows(rows, sorting) {
    const order = sorting && sorting.length ? sorting : [{ property: "blockHeight", direction: "asc" }, { property: "sequence", direction: "asc" }];
    return rows.sort((a, b) => {
        for (const { property, direction } of order) {
            const x = a[property];
            const y = b[property];
            if (x === y) continue;
            const c = x < y ? -1 : 1;
            return direction === "desc" ? -c : c;
        }
        return 0;
    });
}

/** The storage services of the database package, over a ChainStore. */
function storageStubs(store, lib) {
    const { BigNumber } = lib.Utils;
    const txRows = (criteria) => {
        const list = Array.isArray(criteria) ? criteria : [criteria];
        const rows = [];
        for (const { entry, t } of store.allTransactions()) {
            const data = store.transactionData(entry, t);
            if (list.some((c) => matchesCriteria(data, c))) rows.push(data);
        }
        for (const data of store.history) {
            if (list.some((c) => matchesCriteria(data, c))) rows.push(JSON.parse(JSON.stringify(data)));
        }
        return rows;
    };
    const page = (rows, pagination) => {
        const offset = pagination ? pagination.offset : 0;
        const limit = pagination ? pagination.limit : rows.length;
        return { results: rows.slice(offset, offset + limit), totalCount: rows.length, meta: { totalCountIsEstimate: false } };
    };

    const databaseService = {
        async getBlocks(start, end) {
            return store.blocks.filter((e) => e.data.height >= start && e.data.height <= end).map((e) => store.blockData(e, true));
        },
        async findBlockByHeights(heights) {
            return store.blocks.filter((e) => heights.includes(e.data.height)).map((e) => store.blockData(e, false));
        },
        async findLatestBlock() {
            const last = store.blocks[store.blocks.length - 1];
            return last ? store.blockData(last, false) : undefined;
        },
        async getLastBlock() {
            const last = store.blocks[store.blocks.length - 1];
            if (!last) return undefined;
            return lib.Blocks.BlockFactory.fromData(store.blockData(last, true), { deserialiseTransactionsUnchecked: true });
        },
        async saveBlocks(blocks) {
            for (const block of blocks) store.save(block);
        },
        async deleteBlocks(blocks) {
            const lowest = Math.min(...blocks.map((b) => b.height));
            store.deleteFrom(lowest);
        },
        async saveRound(delegates) {
            const rows = delegates.map((d) => ({
                publicKey: d.getPublicKey(),
                balance: d.getAttribute("delegate.voteBalance").toString(),
                round: d.getAttribute("delegate.round"),
            }));
            for (const row of rows) {
                if (!store.rounds.has(row.round)) store.rounds.set(row.round, []);
                store.rounds.get(row.round).push(row);
            }
        },
        async getRound(round) {
            const rows = (store.rounds.get(round) || []).slice();
            rows.sort((a, b) => {
                const c = BigNumber.make(b.balance).comparedTo(BigNumber.make(a.balance));
                if (c !== 0) return c;
                return a.publicKey < b.publicKey ? -1 : a.publicKey > b.publicKey ? 1 : 0;
            });
            return rows.map((r) => ({ ...r }));
        },
        async deleteRound(round) {
            for (const r of [...store.rounds.keys()]) if (r >= round) store.rounds.delete(r);
        },
        async verifyBlockchain() {
            return true;
        },
        async reset() {},
    };

    const blockRepository = {
        async getBlockRewards() {
            const sums = new Map();
            for (const { data } of store.blocks) {
                const key = data.username ?? null;
                const add = BigNumber.make(data.reward).plus(data.totalFee).minus(data.burnedFee ?? 0);
                sums.set(key, (sums.get(key) || BigNumber.ZERO).plus(add));
            }
            return [...sums].map(([username, rewards]) => ({ username, rewards: rewards.toFixed() }));
        },
        async getDonations() {
            const sums = new Map();
            for (const { data } of store.blocks) {
                for (const [address, amount] of Object.entries(data.donations || {})) {
                    const key = `${data.username ?? ""}\u0000${address}`;
                    const row = sums.get(key) || { username: data.username ?? null, address, amount: BigNumber.ZERO };
                    row.amount = row.amount.plus(amount);
                    sums.set(key, row);
                }
            }
            return [...sums.values()].map((r) => ({ ...r, amount: r.amount.toFixed() }));
        },
        async getDelegatesForgedBlocks() {
            const byName = new Map();
            for (const { data } of store.blocks) {
                const key = data.username ?? null;
                const row = byName.get(key) || { username: key, totalFees: BigNumber.ZERO, burnedFees: BigNumber.ZERO, totalRewards: BigNumber.ZERO, donations: BigNumber.ZERO, totalProduced: 0 };
                row.totalFees = row.totalFees.plus(data.totalFee);
                row.burnedFees = row.burnedFees.plus(data.burnedFee ?? 0);
                row.totalRewards = row.totalRewards.plus(data.reward);
                for (const amount of Object.values(data.donations || {})) row.donations = row.donations.plus(amount);
                row.totalProduced++;
                byName.set(key, row);
            }
            return [...byName.values()].map((r) => ({
                ...r,
                totalFees: r.totalFees.toFixed(),
                burnedFees: r.burnedFees.toFixed(),
                totalRewards: r.totalRewards.toFixed(),
                donations: r.donations.toFixed(),
            }));
        },
        async getLastForgedBlocks() {
            const last = new Map();
            for (const { data } of store.blocks) last.set(data.username ?? null, data);
            return [...last.values()]
                .sort((a, b) => b.timestamp - a.timestamp)
                .map((d) => ({ id: d.id, height: d.height, username: d.username ?? null, timestamp: d.timestamp }));
        },
        async deleteBlocks(blocks) {
            return databaseService.deleteBlocks(blocks);
        },
    };

    const transactionRepository = {
        async getSentTransactions() {
            const bySender = new Map();
            for (const { entry, t } of store.allTransactions()) {
                const data = store.transactionData(entry, t);
                const row = bySender.get(data.senderId) || { senderId: data.senderId, amount: BigNumber.ZERO, fee: BigNumber.ZERO, nonce: 0 };
                row.amount = row.amount.plus(data.amount || 0);
                row.fee = row.fee.plus(data.fee);
                row.nonce++;
                bySender.set(data.senderId, row);
            }
            return [...bySender.values()].map((r) => ({ senderId: r.senderId, amount: r.amount.toFixed(), fee: r.fee.toFixed(), nonce: String(r.nonce) }));
        },
        async findByIds(ids) {
            const rows = [];
            for (const { entry, t } of store.allTransactions()) if (ids.includes(t.id)) rows.push(store.transactionData(entry, t));
            return rows;
        },
        // Legacy transfers (1/0): SUM(amount) per recipient.
        async findReceivedTransactions() {
            const byRecipient = new Map();
            for (const data of txRows({ typeGroup: 1, type: 0 })) {
                byRecipient.set(data.recipientId, (byRecipient.get(data.recipientId) || BigNumber.ZERO).plus(data.amount));
            }
            return [...byRecipient].map(([recipientId, amount]) => ({ recipientId, amount: amount.toFixed() }));
        },
        // The HTLC types (1/8, 1/9, 1/10) are inactive at every height, so no stored block holds one.
        async getOpenHtlcLocks() {
            return noHtlc();
        },
        async getClaimedHtlcLockBalances() {
            return noHtlc();
        },
        async getRefundedHtlcLockBalances() {
            return noHtlc();
        },
    };
    function noHtlc() {
        if (txRows([8, 9, 10].map((type) => ({ typeGroup: 1, type }))).length) throw new Error("the store holds an HTLC transaction");
        return [];
    }

    const missedBlockRepository = {
        async addMissedBlocks(rows) {
            for (const row of rows) store.missed.push({ ...row, step: store.recording });
        },
    };

    const blockHistoryService = {
        async listByCriteria(criteria, sorting, pagination) {
            const list = Array.isArray(criteria) ? criteria : [criteria];
            const rows = store.blocks
                .map((e) => store.blockData(e, false))
                .filter((d) =>
                    list.some((c) =>
                        Object.entries(c).every(([key, value]) => {
                            if (key === "height") return (value.from === undefined || d.height >= value.from) && (value.to === undefined || d.height <= value.to);
                            if (key === "username") return d.username === value;
                            throw new Error(`block history stub: criteria key ${key} is not supported`);
                        }),
                    ),
                );
            return page(sortRows(rows, sorting), pagination);
        },
    };

    const transactionHistoryService = {
        async *streamByCriteria(criteria) {
            for (const row of sortRows(txRows(criteria), undefined)) yield row;
        },
        async listByCriteria(criteria, sorting, pagination) {
            return page(sortRows(txRows(criteria), sorting), pagination);
        },
        async findManyByCriteria(criteria) {
            return sortRows(txRows(criteria), undefined);
        },
    };

    const poolQuery = {
        getAllBySender() {
            return { whereKind: () => ({ has: () => false }), has: () => false };
        },
        getAll() {
            return { whereKind: () => ({ has: () => false }), has: () => false };
        },
    };

    return { databaseService, blockRepository, transactionRepository, missedBlockRepository, blockHistoryService, transactionHistoryService, poolQuery };
}

/** Run fn with process.exit and app.terminate turned into a thrown StartRefused. */
async function failClosed(app, fn) {
    const exit = process.exit;
    const terminate = app.terminate;
    let reason;
    app.terminate = async (why, error) => {
        reason = new StartRefused(why, error);
        throw reason;
    };
    process.exit = () => {
        throw reason || new StartRefused("the reference exited");
    };
    try {
        return await fn();
    } finally {
        process.exit = exit;
        app.terminate = terminate;
    }
}

/**
 * Make the reference forget the genesis transaction ids it caches per network byte, so that the
 * next lookup reads the genesis block of the configuration loaded now. (The cache is refreshed
 * only when the network byte changes, and the oracle loads several configurations of one byte.)
 */
function forgetGenesisTransactions(lib) {
    const cm = lib.Managers.configManager;
    const network = cm.get("network.pubKeyHash");
    cm.set("network.pubKeyHash", (network + 1) % 256);
    try {
        lib.Utils.isGenesisTransaction("");
    } finally {
        cm.set("network.pubKeyHash", network);
    }
}

/** Let the handlers' deferred re-indexing (setImmediate) run. */
function settle() {
    return new Promise((resolve) => setImmediate(() => setImmediate(resolve)));
}

let tempDir;
function emptyTempDir() {
    if (!tempDir) tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "heartwood-oracle-state-"));
    return tempDir;
}

/**
 * The reference's state machine for one chain definition (`crypto`: network, milestones,
 * genesisBlock, exceptions, as the crypto files hold them). The configuration manager must hold
 * this chain's configuration while any method runs (see HeadlessChain.withConfig).
 */
class HeadlessChain {
    constructor(solar, crypto) {
        this.solar = solar;
        this.lib = solar.lib;
        this.crypto = crypto === undefined ? undefined : JSON.parse(JSON.stringify(crypto));
        this.pkgs = referencePackages(solar);
        this.store = new ChainStore(this.lib);
        this.started = false;
    }

    /** Load this chain's configuration into the configuration manager, at the last block's height. */
    configure() {
        const cm = this.lib.Managers.configManager;
        cm.setConfig(JSON.parse(JSON.stringify(this.crypto)));
        forgetGenesisTransactions(this.lib);
        cm.setHeight(this.started ? this.stateStore.getLastBlock().data.height : 1);
    }

    /** Run fn under this chain's configuration, then call restore (the caller's configuration). */
    async withConfig(restore, fn) {
        this.configure();
        try {
            return await fn();
        } finally {
            if (restore) {
                restore();
                forgetGenesisTransactions(this.lib);
            }
        }
    }

    async build() {
        const { kernel, stateProvider, transactions } = this.pkgs;
        const { Application, Container, Services, Providers } = kernel;
        const Id = Container.Identifiers;
        process.env.CORE_PATH_TEMP = emptyTempDir();

        const app = new Application(new Container.Container());
        this.app = app;
        const stubs = storageStubs(this.store, this.lib);
        this.stubs = stubs;
        this.events = countingDispatcher();

        app.bind(Id.ApplicationVersion).toConstantValue("4.3.1");
        app.bind(Id.LogService).toConstantValue(silentLogger);
        app.bind(Id.EventDispatcherService).toConstantValue(this.events);
        app.bind(Id.TriggerService).to(Services.Triggers.Triggers).inSingletonScope();
        app.bind(Id.DatabaseService).toConstantValue(stubs.databaseService);
        app.bind(Id.DatabaseBlockRepository).toConstantValue(stubs.blockRepository);
        app.bind(Id.DatabaseTransactionRepository).toConstantValue(stubs.transactionRepository);
        app.bind(Id.DatabaseMissedBlockRepository).toConstantValue(stubs.missedBlockRepository);
        app.bind(Id.BlockHistoryService).toConstantValue(stubs.blockHistoryService);
        app.bind(Id.TransactionHistoryService).toConstantValue(stubs.transactionHistoryService);
        app.bind(Id.PoolQuery).toConstantValue(stubs.poolQuery);
        const stateDefaults = require(path.join(this.solar.solarDir, "packages", "state", "dist", "defaults")).defaults;
        const pluginConfiguration = app.resolve(Providers.PluginConfiguration).from("@solar-network/state", JSON.parse(JSON.stringify(stateDefaults)));
        app.bind(Id.PluginConfiguration).toConstantValue(pluginConfiguration).whenTargetTagged("plugin", "@solar-network/state");
        if (this.crypto) app.get(Id.ConfigRepository).set("crypto", JSON.parse(JSON.stringify(this.crypto)));

        const txProvider = app.resolve(transactions.ServiceProvider);
        await txProvider.register();
        const stProvider = app.resolve(stateProvider.ServiceProvider);
        await stProvider.register();

        this.stateStore = app.get(Id.StateStore);
        this.walletRepository = app.getTagged(Id.WalletRepository, "state", "blockchain");
        this.databaseInteraction = app.get(Id.DatabaseInteraction);
        this.roundState = app.get(Id.RoundState);
        this.handlerRegistry = app.getTagged(Id.TransactionHandlerRegistry, "state", "blockchain");
    }

    /** The reference's first start on the genesis block. Throws StartRefused when it refuses. */
    async start() {
        await this.build();
        const Id = this.pkgs.kernel.Container.Identifiers;
        const app = this.app;
        this.store.recording = "start";
        await failClosed(app, async () => {
            await this.databaseInteraction.initialise();
            const block = this.stateStore.getLastBlock();
            if (block.data.height !== 1) throw new StartRefused("the start did not end at the genesis block");
            const { databaseService } = this.stubs;
            await databaseService.deleteRound(1);
            await databaseService.deleteRound(this.pkgs.kernel.Utils.roundCalculator.calculateRound(1).round + 1);
            await app.get(Id.StateBuilder).run();
            await this.databaseInteraction.restoreCurrentRound();
        });
        await settle();
        this.started = true;
        this.store.recording = null;
    }

    lastBlock() {
        return this.stateStore.getLastBlock();
    }

    /** A block as a relay receives it, rebuilt by BlockFactory.fromData. */
    receive(bytes) {
        const { Blocks } = this.lib;
        const deserialised = Blocks.Deserialiser.deserialise(bytes);
        const data = { ...deserialised.data, transactions: deserialised.transactions.map((t) => t.data) };
        const block = Blocks.BlockFactory.fromData(data);
        if (!block) throw new Error("BlockFactory returned no block");
        return block;
    }

    /**
     * Apply a block's bytes. Returns {accepted, block, reward, missed, error}: `reward` is the
     * reward the state computes for the block before it is applied (absent when the generator is
     * no delegate), `missed` the missed-slot rows recorded while applying it.
     */
    async apply(bytes, step = "apply") {
        const block = this.receive(bytes);
        this.pkgs.setBlockUsername.call({ walletRepository: this.walletRepository }, block.data);
        let reward;
        if (block.data.username && this.walletRepository.hasByUsername(block.data.username)) {
            const generator = this.walletRepository.findByUsername(block.data.username);
            reward = (await this.roundState.getRewardForBlockInRound(block.data.height, generator)).reward.toFixed();
        }
        const before = this.store.missed.length;
        this.store.recording = `${step}:${block.data.height}`;
        const previous = this.stateStore.getLastBlock();
        let error;
        try {
            await this.databaseInteraction.applyBlock(block, { index: undefined });
            await this.stubs.databaseService.saveBlocks([block]);
        } catch (e) {
            error = e;
            // The accept handler: revert the block if it became the last block, then the previous
            // block is the last block again.
            if (this.stateStore.getLastBlock().data.height === block.data.height) {
                await this.databaseInteraction.revertBlock(block);
            }
            this.stateStore.setLastBlock(previous);
        }
        this.store.recording = null;
        await settle();
        const missed = this.store.missed.slice(before).map(({ timestamp, height, username }) => ({ timestamp, height, username }));
        return { accepted: !error, block, reward, missed, error };
    }

    /**
     * Revert the last `count` blocks as the blockchain's removeBlocks does. `onEach` runs after
     * each single revert, with the new last block.
     */
    async revert(count, onEach) {
        const lastHeight = this.stateStore.getLastBlock().data.height;
        if (count < 1 || count >= lastHeight) throw new RangeError(`cannot revert ${count} blocks at height ${lastHeight}`);
        const remaining = await this.stubs.databaseService.getBlocks(lastHeight - count, lastHeight);
        for (let i = 0; i < count; i++) {
            const last = this.stateStore.getLastBlock();
            this.store.recording = `revert:${last.data.height}`;
            await this.databaseInteraction.revertBlock(last);
            remaining.pop();
            const target = remaining[remaining.length - 1];
            const next =
                target.height === 1
                    ? this.stateStore.getGenesisBlock()
                    : this.lib.Blocks.BlockFactory.fromData(target, { deserialiseTransactionsUnchecked: true });
            this.stateStore.setLastBlock(next);
            this.store.recording = null;
            await settle();
            if (onEach) await onEach(next);
        }
        await this.stubs.databaseService.deleteBlocks([{ height: lastHeight - count + 1 }]);
    }

    /** The wallets of the state, in the order of the address index. */
    wallets() {
        return this.walletRepository.allByAddress();
    }

    /** The round state's current forging list and the blocks of the current round. */
    round() {
        return { forging: this.roundState.forgingDelegates, blocks: this.roundState.blocksInCurrentRound };
    }
}

/**
 * The reference's state for one wallet fixture (see tools/difftest's fixture module): the wallets,
 * their vote balances as the reference builds them at a start (DposState.buildVoteBalances), a
 * last block at `lastBlockHeight`, and the history the handlers look up: one vote (2/2) at
 * lastBlockHeight - 1 with each voter's current votes, and one resignation (1/7) at each
 * delegate's resignedAt with its type (2, a revoke, when the delegate is not resigned). It runs
 * under the configuration the caller set; one container serves every fixture.
 */
class FixtureState {
    constructor(solar) {
        this.chain = new HeadlessChain(solar, undefined);
        this.built = false;
    }

    async load(fixture, lastBlockHeight) {
        if (!Array.isArray(fixture)) throw new TypeError("a fixture is an array of wallets");
        if (!Number.isSafeInteger(lastBlockHeight) || lastBlockHeight < 1) throw new TypeError("lastBlockHeight must be a positive integer");
        const chain = this.chain;
        if (!this.built) {
            await chain.build();
            this.built = true;
        }
        const { Utils } = chain.lib;
        const { BigNumber } = Utils;
        const repository = chain.walletRepository;
        repository.reset();
        chain.store.blocks = [];
        chain.store.rounds = new Map();
        chain.store.missed = [];
        chain.store.history = [];
        chain.stateStore.lastBlocks = chain.stateStore.lastBlocks.clear();
        chain.stateStore.setLastBlock({ data: { height: lastBlockHeight, id: "00".repeat(32), timestamp: 0 }, transactions: [] });
        const amount = (value, what) => {
            if (value === undefined) return BigNumber.ZERO;
            if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value)) throw new TypeError(`${what} must be a decimal string`);
            return BigNumber.make(value);
        };
        const percents = (votes) => Object.fromEntries(votes.map(([name, bps]) => [name, bps / 100]));
        fixture.forEach((w, i) => {
            const wallet = repository.findByAddress(w.a);
            if (w.pk !== undefined) wallet.setPublicKey(w.pk);
            wallet.setBalance(amount(w.bal, `wallet ${i} bal`));
            wallet.setNonce(amount(w.n, `wallet ${i} n`));
            if (w.spk !== undefined) wallet.setAttribute("secondPublicKey", w.spk);
            if (w.u !== undefined) {
                const delegate = {
                    username: w.u,
                    voteBalance: BigNumber.ZERO,
                    forgedFees: amount(w.ff, `wallet ${i} ff`),
                    burnedFees: amount(w.bf, `wallet ${i} bf`),
                    forgedRewards: amount(w.fr, `wallet ${i} fr`),
                    donations: amount(w.dn, `wallet ${i} dn`),
                    producedBlocks: w.pb || 0,
                    voters: 0,
                };
                if (w.lb !== undefined) delegate.lastBlock = w.lb;
                wallet.setAttribute("delegate", delegate);
                if (w.rs !== undefined) wallet.setAttribute("delegate.resigned", w.rs);
                if (w.resignedAt !== undefined) {
                    chain.store.history.push({
                        id: `fixture-resignation-${i}`,
                        senderId: w.a,
                        senderPublicKey: w.pk,
                        typeGroup: 1,
                        type: 7,
                        blockHeight: w.resignedAt,
                        sequence: 0,
                        asset: { resignationType: w.rs !== undefined ? w.rs : 2 },
                    });
                }
            }
            if (w.v && w.v.length) {
                wallet.setAttribute("votes", new Map(Object.entries(Utils.sortVotes(percents(w.v)))));
                chain.store.history.push({
                    id: `fixture-vote-${i}`,
                    senderId: w.a,
                    senderPublicKey: w.pk,
                    typeGroup: 2,
                    type: 2,
                    blockHeight: lastBlockHeight - 1,
                    sequence: 0,
                    asset: { votes: percents(w.v) },
                });
            }
            repository.index(wallet);
        });
        const Id = chain.pkgs.kernel.Container.Identifiers;
        chain.app.getTagged(Id.DposState, "state", "blockchain").buildVoteBalances();
        await settle();
    }

    /**
     * Apply the transaction `bytes` to the loaded fixture as block lastBlockHeight + 1 applies it
     * (BlockState.applyTransaction under the rules of that height), then revert it. Returns the
     * #W sections after each, or the refusal and the #W section after the refused attempt.
     */
    async applyTransaction(bytes, lastBlockHeight, walletSection) {
        const chain = this.chain;
        const { Managers, Transactions } = chain.lib;
        const cm = Managers.configManager;
        const height = lastBlockHeight + 1;
        const Id = chain.pkgs.kernel.Container.Identifiers;
        const blockState = chain.app.getTagged(Id.BlockState, "state", "blockchain");
        const transaction = await cm.runAtHeight(height, async () => Transactions.TransactionFactory.fromBytes(bytes, true));
        transaction.data.blockHeight = height;
        transaction.data.blockId = "00".repeat(32);
        transaction.data.sequence = 0;
        try {
            await cm.runAtHeight(height, () => blockState.applyTransaction(height, transaction));
        } catch (error) {
            await settle();
            return { error, after: walletSection(chain.wallets()) };
        }
        await settle();
        const applied = walletSection(chain.wallets());
        await cm.runAtHeight(height, () => blockState.revertTransaction(height, transaction));
        await settle();
        return { applied, reverted: walletSection(chain.wallets()) };
    }
}

module.exports = { FixtureState, HeadlessChain, StartRefused, forgetGenesisTransactions, referencePackages };
