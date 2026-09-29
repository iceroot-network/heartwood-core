"use strict";
// Oracle operation of `snapshot:rollback` over a chain session's store, through the reference's
// SnapshotService (snapshots/src/snapshot-service.ts rollbackByHeight and rollbackByNumber) with
// its database service stubbed over the session's store (block-repository.ts's four deletes):
//   chain.rollback  {session, height?, number?} -> {refused: {lines}} or {target, round, lines,
//                   deleted: {blocks, transactions, missed, rounds}, summary}
// `summary` is the store after it: the tip, the highest round row, and the numbers of missed-slot
// rows and of transactions. The session goes on with the rolled-back store, so chain.restart then
// gives the reference's start after a rollback. Not built yet: the operation throws.
// Loaded by lib/ops.js.

module.exports = function chainRollbackOps() {
    return {
        "chain.rollback": async () => {
            throw new Error("chain.rollback is not built");
        },
    };
};
