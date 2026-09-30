"use strict";
// The records of kind api.productivityWindow in V34: the first second of the window the
// reference's updateProductivity asks the store for, at the UTC midnight of the tip's date less
// missedBlocksLookback, over epochs at a whole second, with milliseconds, at midnight and a
// moment before it, tips around the midnights after them and far on, and lookbacks of none, one
// second, a week, 30 days (the default) and more. Inputs are fixed, so every run writes the same
// records.

const EPOCHS = [
    "2022-03-28T18:00:00.000Z",
    "2022-03-28T18:00:00.250Z",
    "2022-03-28T18:00:00.999Z",
    "2026-01-01T00:00:00.000Z",
    "2025-12-31T23:59:59.500Z",
    "2019-06-15T06:30:15.001Z",
];

const LOOKBACKS = [0, 1, 604800, 2592000, 2678400];

module.exports = async function apiProductivityVectors({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const add = async (input, name) => out.push({ ...must(await rec("api.productivityWindow", H, input)), name });
    for (const epoch of EPOCHS) {
        const unix = Date.parse(epoch) / 1000;
        // The seconds to the first UTC midnight after the epoch.
        const toMidnight = Math.ceil((Math.floor(unix / 86400) + 1) * 86400 - unix);
        const tips = [0, 1, toMidnight - 1, toMidnight, toMidnight + 1, toMidnight + 86399, toMidnight + 86400, 94608000 + 12345];
        for (const tip of [...new Set(tips)].filter((t) => t >= 0)) {
            await add({ tipTimestamp: tip, epoch, lookback: 2592000 }, `productivityWindow: epoch ${epoch}, tip ${tip}`);
        }
        for (const lookback of LOOKBACKS.filter((l) => l !== 2592000)) {
            await add({ tipTimestamp: toMidnight + 3600, epoch, lookback }, `productivityWindow: epoch ${epoch}, lookback ${lookback}`);
        }
    }
    return out;
};
