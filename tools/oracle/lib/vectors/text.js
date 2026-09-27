"use strict";
// V22-text: the reference's message formats.
//   text.formatSatoshi   amounts 0, 1 and 5, 10^8 and its neighbours, every power of ten and its
//                        neighbours, amounts around 2^53, 2^60, 2^63 and 2^64, negatives, and
//                        other token symbols (an integer divided by 10^8 always prints with at
//                        most 8 fraction digits, so formatSatoshi never rounds; above 2^53 the
//                        amount is first the nearest double);
//   text.toLocaleFixed   the rounding formatSatoshi's toLocaleString call makes, on numbers that
//                        need it: ties of every fraction digit count from 0 to 8 (half away from
//                        zero, on the shortest digits of the double), numbers just beside a tie,
//                        negatives, large and tiny magnitudes;
//   text.toLocaleString  integer counts, from 0 to beyond 2^53, and negatives;
//   text.transaction     toString of the six kept types, of second-signed ones, and of copies with
//                        nonces from 0 to 2^63 - 1, wire version bytes 0 and 2 and another network
//                        byte (the sender's address is the node's network's either way).
// The pluralise records are in V16-kernel, since pluralise is the kernel's.

const SYMBOL = "dRT";
const TWO_53 = 2n ** 53n;

module.exports = async function text({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (op, input, name) => out.push({ ...must(await rec(op, H, input)), name });

    const amounts = [0n, 1n, 5n, 9n, 10n, 99n, 100n, 12345678n, 99999999n, 100000000n, 100000001n, 150000000n, 123456789n];
    for (let k = 0n; k <= 20n; k++) amounts.push(10n ** k, 10n ** k - 1n, 10n ** k + 1n);
    for (const base of [TWO_53, 2n ** 54n, 2n ** 60n, 2n ** 63n, 2n ** 64n]) {
        for (const offset of [-3n, -2n, -1n, 0n, 1n, 2n, 3n, 5n]) amounts.push(base + offset);
    }
    amounts.push(2n ** 64n - 1n, 9223372036854775807n, 10n ** 16n + 5n, 10n ** 17n + 5n, 4503599627370497n);
    const unique = [...new Set(amounts.map(String))];
    for (const amount of unique) await push("text.formatSatoshi", { amount, symbol: SYMBOL }, `amount ${amount}`);
    const negatives = ["1", "50", "100000000", "100000001", "9007199254740993", "9223372036854775807", "9223372036854775808", "18446744073709551615"];
    for (const amount of negatives) await push("text.formatSatoshi", { amount: `-${amount}`, symbol: SYMBOL }, `amount -${amount}`);
    for (const symbol of ["SXP", "", "ROOT", "₿", "a b"]) {
        for (const amount of ["0", "150000000", "-2"]) await push("text.formatSatoshi", { amount, symbol }, `amount ${amount}, symbol ${JSON.stringify(symbol)}`);
    }

    const fixed = [];
    for (let digits = 0; digits <= 8; digits++) {
        for (const text of ["0.5", "1.5", "2.5", "0.05", "0.15", "0.25", "0.125", "1.000000005", "1.000000015", "1.0000000249999998", "0.0000000049999999", "99999999.999999995", "1234.5678", "0.1", "0.3", "7"]) {
            fixed.push([Number(text), digits]);
            fixed.push([-Number(text), digits]);
        }
    }
    fixed.push([0, 8], [-0, 8], [1e21, 8], [1.5e21, 0], [5e-324, 8], [1.7976931348623157e308, 2], [2 ** 64, 8], [123456789.123456789, 3], [1e-7, 8], [9.5e-8, 8]);
    for (const [value, digits] of fixed) await push("text.toLocaleFixed", { value, digits }, `${value} to ${digits} digits`);

    const counts = [0, 1, 2, 12, 999, 1000, 1001, 9999, 10000, 123456, 1000000, 2000000, 2 ** 31 - 1, 2 ** 32, 2 ** 53 - 1, 2 ** 53, 2 ** 53 + 2, 1e18, 123456789012345680, 9223372036854775807];
    for (const count of [...counts, ...counts.filter((c) => c > 0).map((c) => -c)]) await push("text.toLocaleString", { count }, `count ${count}`);

    const kept = ["transfer-1", "transfer-1-memo", "transfer-2", "transfer-256", "second-signature", "delegate-registration", "resignation-0", "resignation-1", "resignation-2", "burn", "unvote", "vote-1", "vote-53"];
    const signed = ["transfer-second-signed", "delegate-registration-second-signed", "resignation-2-second-signed", "burn-second-signed", "vote-second-signed"];
    for (const name of [...kept, ...signed]) await push("text.transaction", { hex: shared.tx[name].hex }, name);
    // Copies with another nonce (bytes 9 to 16), version byte (1) or network byte (2); only the
    // id changes with them, and no rule is checked. A nonce of 2^63 or more has no id: the
    // reference's serialiser writes the nonce as a signed 64-bit integer and throws.
    const copy = (name, change) => {
        const bytes = Buffer.from(shared.tx[name].hex, "hex");
        change(bytes);
        return bytes.toString("hex");
    };
    for (const nonce of [0n, 1n, 9n, 10n, 2n ** 32n, 2n ** 53n + 1n, 2n ** 63n - 1n]) {
        for (const name of ["transfer-1", "vote-1"]) {
            await push("text.transaction", { hex: copy(name, (b) => b.writeBigUInt64LE(nonce, 9)) }, `${name}, nonce ${nonce}`);
        }
    }
    for (const version of [0, 2]) {
        for (const name of ["transfer-1", "burn", "resignation-0"]) {
            await push("text.transaction", { hex: copy(name, (b) => (b[1] = version)) }, `${name}, version byte ${version}`);
        }
    }
    for (const network of [30, 63, 0]) {
        await push("text.transaction", { hex: copy("delegate-registration", (b) => (b[2] = network)) }, `delegate-registration, network byte ${network}`);
    }
    return out;
};
