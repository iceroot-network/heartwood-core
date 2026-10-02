"use strict";
// Oracle operations of the codec fuzz targets' replay. Loaded by lib/ops.js.
//
//   tx.fromData   JSON text read as a transaction's data, as the reference's TransactionFactory
//                 reads it: `text` parsed by JSON.parse, the data copied as fromJson copies it
//                 (`amount`, when it is truthy, and `fee` made BigNumbers), then
//                 TransactionFactory.fromData(data, strict) with `strict` true unless it is
//                 false. fromData itself renames a transfer's `asset.payments` to
//                 `asset.transfers` and a truthy `vendorField` to `memo`, and verifies the data
//                 against the schema (the strict one when `strict`) before it serialises and
//                 reads the bytes back. The output is the transaction as every transaction
//                 operation writes it: {id, hex, json, isVerified}. A text that is not JSON, a
//                 value that is not an object, and every refusal of the factory are errors.

module.exports = function codecOps(context) {
    const { solar, txOutput } = context;
    const { Transactions, Utils } = solar.lib;

    return {
        "tx.fromData": ({ text, strict }) => {
            if (typeof text !== "string") throw new TypeError("text must be text");
            if (strict !== undefined && typeof strict !== "boolean") throw new TypeError("strict must be a boolean");
            const json = JSON.parse(text);
            const data = { ...json };
            if (data.amount) data.amount = Utils.BigNumber.make(data.amount);
            data.fee = Utils.BigNumber.make(data.fee);
            return txOutput(Transactions.TransactionFactory.fromData(data, strict !== false));
        },
    };
};
