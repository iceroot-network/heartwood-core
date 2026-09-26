"use strict";
// The kernel's and the logger's vector classes:
//   V09-rounds  calculateRound and isNewRound over heights 1 to 10,000 and around 2^32,
//               calculateForgingInfo over slot edges, half slots and gaps, isBlockChained cases;
//   V16-kernel  stringify, .env text, epoch timestamps, IP lists and node paths;
//   V20-logger  log lines for every level, coloured and plain.
// Each builder returns its records, or null while the class is not written.

module.exports = {
    rounds: async () => null,
    kernel: async () => null,
    logger: async () => null,
};
