"use strict";
// The vector classes of wallets, ranking and rounds:
//   V08-ranking       buildDelegateRanking over generated delegate sets and key tie-breaks;
//   V09-shuffle       the forging order of rounds 1 to 10,000 for every count from 1 to 53, as one
//                     SHA-256 per count, full orders of the first rounds, and the rounds whose draws
//                     reject a value;
//   V15-vote-amounts  a voter's shares for edge and random balances and vote maps.
// Each builder returns its records, or null while the class is not written.

module.exports = {
    ranking: async () => null,
    shuffle: async () => null,
    voteAmounts: async () => null,
};
