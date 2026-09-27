"use strict";
// Oracle operations of the reference's message formats. Loaded by lib/ops.js.
//
//   text.formatSatoshi   the crypto package's Utils.formatSatoshi of `amount` (decimal text of
//                        any size and sign, read as a BigNumber), with the network's token symbol
//                        set to `symbol` for the call (the devnet's when it is not given)
//   text.pluralise       the kernel's Utils.pluralise(word, count, inclusive)
//   text.toLocaleString  count.toLocaleString(), as the handlers' messages write a count
//   text.toLocaleFixed   value.toLocaleString("en", {minimumFractionDigits: 0,
//                        maximumFractionDigits: digits}), the call formatSatoshi makes, on any
//                        number (formatSatoshi's own quotients never need rounding: an integer
//                        divided by 10^8 always prints with at most 8 fraction digits)
//   text.transaction     a transaction's toString, as the pool's messages write it: the bytes of
//                        `hex` rebuilt the way the pool rebuilds a transaction its verifier has
//                        read (fromBytesUnsafe with the verifier's id); no rule of the transaction
//                        is checked
//
// Counts are written in the process's default locale, as the reference writes them; the output's
// notes.locale is true when that locale writes the count otherwise than English does.

const path = require("path");

module.exports = function textOps(context) {
    const { solar, cm, hexBuffer, safeInteger } = context;
    const { Transactions, Utils } = solar.lib;
    let pluralise;
    const kernelUtil = () => {
        if (!pluralise) pluralise = require(path.join(solar.solarDir, "packages", "kernel", "dist", "utils", "pluralise.js")).pluralise;
        return pluralise;
    };

    /** The output, with notes.locale when the default locale writes `count` otherwise than "en". */
    function withLocale(output, count) {
        return count.toLocaleString() === count.toLocaleString("en") ? output : { ...output, notes: { locale: true } };
    }

    /** A signed decimal integer text, of any size. */
    function signedAmount(value, name) {
        if (typeof value !== "string" || !/^-?(0|[1-9][0-9]*)$/.test(value)) throw new TypeError(`${name} must be a decimal integer string`);
        return value === "-0" ? "0" : value;
    }

    return {
        "text.formatSatoshi": ({ amount: value, symbol }) => {
            const saved = cm.get("network.client.symbol");
            if (symbol !== undefined && typeof symbol !== "string") throw new TypeError("symbol must be text");
            try {
                if (symbol !== undefined) cm.set("network.client.symbol", symbol);
                return { text: Utils.formatSatoshi(Utils.BigNumber.make(signedAmount(value, "amount"))) };
            } finally {
                cm.set("network.client.symbol", saved);
            }
        },

        "text.pluralise": ({ word, count, inclusive }) => {
            if (typeof word !== "string") throw new TypeError("word must be text");
            safeInteger(count, "count");
            if (typeof inclusive !== "boolean") throw new TypeError("inclusive must be a boolean");
            const output = { text: kernelUtil()(word, count, inclusive) };
            return inclusive ? withLocale(output, count) : output;
        },

        "text.toLocaleString": ({ count }) => {
            if (typeof count !== "number" || !Number.isInteger(count)) throw new TypeError("count must be an integer");
            return withLocale({ text: count.toLocaleString() }, count);
        },

        "text.toLocaleFixed": ({ value, digits }) => {
            if (typeof value !== "number") throw new TypeError("value must be a number");
            if (!Number.isInteger(digits) || digits < 0 || digits > 20) throw new RangeError("digits must be an integer from 0 to 20");
            return { text: value.toLocaleString("en", { minimumFractionDigits: 0, maximumFractionDigits: digits }) };
        },

        "text.transaction": ({ hex }) => {
            const bytes = hexBuffer(hex, "hex");
            const { id } = Transactions.TransactionFactory.fromBytesUnsafe(bytes);
            return { text: Transactions.TransactionFactory.fromBytesUnsafe(bytes, id).toString() };
        },
    };
};
