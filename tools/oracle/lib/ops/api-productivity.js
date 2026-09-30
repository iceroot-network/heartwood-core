"use strict";
// Oracle operation of the delegates' productivity window:
//   api.productivityWindow  {tipTimestamp, epoch, lookback} -> {firstSecond}
//                           the reference's own updateProductivity (blockchain.ts) run with the
//                           tip at tipTimestamp (seconds since the epoch), the network's first
//                           milestone at `epoch` and missedBlocksLookback `lookback` (seconds):
//                           the timestamp it asks the missed-block repository's
//                           getBlockProductivity for, given as the first whole second that the
//                           query's `timestamp >= ?` counts (the timestamp is fractional when the
//                           epoch has milliseconds)
// Loaded by lib/ops.js.

const path = require("path");

module.exports = function apiProductivityOps({ solar, devnet, cm }) {
    async function productivityWindow({ tipTimestamp, epoch, lookback }) {
        if (!Number.isSafeInteger(tipTimestamp) || tipTimestamp < 0) throw new TypeError("tipTimestamp must be a whole number of seconds");
        if (!Number.isSafeInteger(lookback)) throw new TypeError("lookback must be an integer");
        if (typeof epoch !== "string") throw new TypeError("epoch must be text");
        const { Blockchain } = require(path.join(solar.solarDir, "packages", "blockchain", "dist", "blockchain"));
        const config = JSON.parse(JSON.stringify(devnet.config));
        config.milestones = config.milestones.slice(0, 1);
        config.milestones[0].epoch = epoch;
        let asked;
        // What updateProductivity reads of its node; the repository records the window it is asked
        // for and answers no delegate.
        const node = {
            updating: false,
            lastUpdatedBlockId: undefined,
            getLastBlock: () => ({ data: { id: "tip", timestamp: tipTimestamp } }),
            configuration: { get: (key) => (key === "missedBlocksLookback" ? lookback : undefined) },
            missedBlockRepository: {
                getBlockProductivity: async (timestamp) => {
                    asked = timestamp;
                    return {};
                },
            },
            walletRepository: {
                findByUsername: () => {
                    throw new Error("no delegate");
                },
            },
            events: { dispatch: () => undefined },
        };
        try {
            cm.setConfig(config);
            await Blockchain.prototype.updateProductivity.call(node, false);
        } finally {
            devnet.restore();
        }
        if (typeof asked !== "number" || !Number.isFinite(asked)) throw new Error("updateProductivity asked for no window");
        return { firstSecond: Math.ceil(asked) };
    }

    return { "api.productivityWindow": productivityWindow };
};
