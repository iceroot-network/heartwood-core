"use strict";
// Oracle operations of `snapshot:rollback` over a chain session's store (lib/ops/chain.js), and the
// saved states its start may load. Loaded by lib/ops.js.
//
//   chain.rollback  {session, height?, number?}
//       the command's dispatch (core/src/commands/snapshot-rollback.ts: --height when given and not
//       0, else --number when given and not 0, else its error line), then the reference's
//       SnapshotService.rollbackByHeight or rollbackByNumber (snapshots/src/snapshot-service.ts)
//       with its SnapshotDatabaseService (snapshots/src/database-service.ts) over a stubbed block
//       repository whose findLast, findByHeight and rollback re-implement
//       snapshots/src/repositories/block-repository.ts:25-61 over the session's store: the landing
//       block looked up at the round's first height minus 1, then the transactions with a
//       timestamp above the landing block's (a stored transaction carries its block's), the blocks
//       and the missed-slot rows above its height, and the round rows above the requested
//       height's round minus 1.
//       -> {refused: {lines}} when nothing was deleted, else {target, round, lines, deleted:
//          {blocks, transactions, missed, rounds}, summary}: the landing height, the requested
//          height's round, the lines as [level, text] (an error's stack as its first line), the
//          rows deleted (`rounds` counts round rows), and the store's summary after it
//          ({tip, round, missed, transactions}: the tip, the highest round with rows, and the
//          numbers of missed-slot rows and of transactions).
//       The session's node is then stopped: the store is rolled back under it, and the next
//       operation is chain.restart, which starts a new node over the store, as the relay's start
//       after the command does. The rollback's force-integrity-check.lock goes into the session's
//       own temp directory, which that start reads.
//   chain.saveState {session} -> {height}
//       StateSaver on the session's node, as a node's queue drain runs it, into its saved-states
//       directory beside the states saved before (none pruned); each file saved in the session is
//       one second newer than the last, so the loader tries them newest first. chain.restart with
//       mode "kept" then starts from them.

const fs = require("fs");
const path = require("path");

const { sessions: registry } = require("../pool");
const { newDataDir, referencePackages } = require("../state");

/** The first second of the saved states' times; each state saved in a session is one later. */
const SAVED_EPOCH_MS = Date.UTC(2026, 0, 1);

module.exports = function chainRollbackOps(context) {
    const { solar, devnet } = context;

    const session = (id) => {
        const s = registry.get(id);
        if (s.kind !== "chain") throw new Error(`session ${id} is not a chain session`);
        return s.replay;
    };

    // Run fn under the session's configuration, then restore the devnet configuration.
    const inSession = (s, fn) => s.chain.withConfig(() => devnet.restore(), fn);

    const flag = (value, name) => {
        if (value === undefined || value === null) return undefined;
        if (!Number.isSafeInteger(value)) throw new TypeError(`${name} must be an integer`);
        return value;
    };

    /** A logger that keeps each line as [level, text], an error's stack as its first line. */
    const recordingLogger = (lines) => {
        const keep = (level) => (message) => {
            const text = typeof message === "string" ? message : message && message.stack ? message.stack : String(message);
            lines.push([level, text.split("\n")[0]]);
        };
        const logger = { suppressConsoleOutput() {} };
        for (const level of ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"]) logger[level] = keep(level);
        return logger;
    };

    /** The reference's snapshot service over `store`, logging into `lines`; `landed` is told the rollback's landing. */
    const snapshotService = (store, lines, landed) => {
        const { Application, Container } = referencePackages(solar).kernel;
        const Id = Container.Identifiers;
        const snapshots = (file) => require(path.join(solar.solarDir, "packages", "snapshots", "dist", file));
        const SnapIds = snapshots("ioc").Identifiers;
        const { SnapshotService } = snapshots("snapshot-service");
        const { SnapshotDatabaseService } = snapshots("database-service");
        const repository = {
            async findLast() {
                const entry = store.blocks[store.blocks.length - 1];
                return entry ? store.blockData(entry, false) : undefined;
            },
            async findByHeight(height) {
                const entry = store.blocks[height - 1];
                return entry ? store.blockData(entry, false) : undefined;
            },
            // block-repository.ts:25-61, over the store.
            async rollback(roundInfo) {
                const block = await this.findByHeight(roundInfo.roundHeight - 1);
                if (!block) throw new Error("Cannot find block on height " + roundInfo.roundHeight);
                const deleted = { blocks: 0, transactions: 0, missed: 0, rounds: 0 };
                for (const entry of store.blocks) {
                    if (entry.data.timestamp > block.timestamp) deleted.transactions += entry.transactions.length;
                    if (entry.data.height > block.height) deleted.blocks += 1;
                }
                deleted.missed = store.missed.filter((row) => row.height > block.height).length;
                for (const [round, rows] of store.rounds) if (round > roundInfo.round - 1) deleted.rounds += rows.length;
                store.deleteAbove(block.height, roundInfo.round - 1);
                landed({ target: block.height, round: roundInfo.round, deleted });
            },
        };
        const app = new Application(new Container.Container());
        app.bind(Id.LogService).toConstantValue(recordingLogger(lines));
        app.bind(SnapIds.SnapshotFilesystem).toConstantValue({});
        for (const plugin of ["@solar-network/snapshots", "@solar-network/database"]) {
            app.bind(Id.PluginConfiguration).toConstantValue({}).whenTargetTagged("plugin", plugin);
        }
        app.bind(SnapIds.SnapshotBlockRepository).toConstantValue(repository);
        for (const other of ["SnapshotMissedBlockRepository", "SnapshotRoundRepository", "SnapshotTransactionRepository"]) {
            app.bind(SnapIds[other]).toConstantValue({});
        }
        app.bind(SnapIds.SnapshotDatabaseService).to(SnapshotDatabaseService).inSingletonScope();
        app.bind(Id.SnapshotService).to(SnapshotService).inSingletonScope();
        return app.get(Id.SnapshotService);
    };

    return {
        "chain.rollback": async ({ session: id, height, number }) => {
            height = flag(height, "height");
            number = flag(number, "number");
            const s = session(id);
            return inSession(s, async () => {
                const store = s.chain.store;
                const lines = [];
                let landing;
                const service = snapshotService(store, lines, (value) => (landing = value));
                if (!s.chain.tempPath) s.chain.tempPath = newDataDir("temp");
                const temp = process.env.CORE_PATH_TEMP;
                process.env.CORE_PATH_TEMP = s.chain.tempPath;
                try {
                    if (height) await service.rollbackByHeight(height);
                    else if (number) await service.rollbackByNumber(number);
                    else recordingLogger(lines).error("Please specify either a height or number of blocks to roll back");
                } finally {
                    process.env.CORE_PATH_TEMP = temp;
                }
                if (!landing) return { refused: { lines } };
                return { target: landing.target, round: landing.round, lines, deleted: landing.deleted, summary: store.summary() };
            });
        },

        "chain.saveState": async ({ session: id }) => {
            const s = session(id);
            return inSession(s, async () => {
                const file = await s.chain.saveState({ keep: true });
                s.savedStates = (s.savedStates || 0) + 1;
                const time = new Date(SAVED_EPOCH_MS + s.savedStates * 1000);
                fs.utimesSync(file, time, time);
                return { height: s.chain.lastBlock().data.height };
            });
        },
    };
};
