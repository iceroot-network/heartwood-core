"use strict";
// V06 arithmetic: the burned share of a fee (burn.feeBasisPoints 9,000), the block reward by
// delegate rank over the five tiers, the donations (500 basis points to each of two recipients from
// height 2), the dynamic fee floor of each kept transaction type, and a block's totalAmount, which
// counts top-level amounts only (a 1/6 transfer has none).

const { FIXED_AUX } = require("../ops");
const { GENERATOR } = require("./constants");

module.exports = async function arithmetic({ rec, must, shared }) {
    const out = [];
    const H = shared.height;

    for (const fee of ["0", "1", "9", "10", "11", "99", "100", "101", "6173", "1000026", "7500411055", "9223372036854775807", "18446744073709551615"]) {
        out.push({ ...must(await rec("fee.burned", H, { fee })), name: `fee ${fee}` });
    }

    for (const height of [1, 2]) {
        const ranks = height === 2 ? Array.from({ length: 55 }, (_, i) => i) : [1, 10, 11, 21, 22, 32, 33, 43, 44, 53, 54];
        for (const rank of ranks) out.push({ ...(await rec("reward.calculate", height, { rank })), name: `rank ${rank}` });
    }

    for (const height of [1, 2, 3]) {
        const rewards = height === 2 ? ["180000000", "190000000", "200000000", "210000000", "220000000", "0", "1", "19", "20", "21", "9223372036854775807"] : ["200000000"];
        for (const reward of rewards) out.push({ ...must(await rec("reward.donations", height, { reward })), name: `reward ${reward}` });
    }

    // The dynamic fee floor: (addonBytes[type] + round(size / 2)) x minFee, where size is the byte
    // length; resignations and burns pay no dynamic fee.
    for (const name of [
        "transfer-1",
        "transfer-1-memo",
        "transfer-memo-255",
        "transfer-2",
        "transfer-256",
        "second-signature",
        "delegate-registration",
        "resignation-0",
        "resignation-2",
        "burn",
        "vote-1",
        "vote-2",
        "vote-53",
        "unvote",
        "transfer-second-signed",
        "vote-second-signed",
    ]) {
        out.push({ ...must(await rec("fee.minimum", H, { hex: shared.tx[name].hex })), name });
    }

    // totalAmount counts the burn's amount and not the transfer's items: the forger's total is
    // accepted, a total that includes the items is refused.
    const aux = FIXED_AUX.toString("hex");
    const transactions = [shared.tx["transfer-1"].hex, shared.tx["burn"].hex, shared.tx["transfer-2"].hex];
    const base = { height: 5, timestamp: 32, previousBlock: "ef".repeat(32), reward: "200000000", passphrase: GENERATOR, aux, transactions };
    out.push({ ...(await rec("block.make", 5, base)), name: "totalAmount: top-level amounts only" });
    const withItems = (BigInt(shared.tx["burn"].json.amount) + BigInt(shared.tx["transfer-1"].json.asset.transfers[0].amount) + 1000n + 1001n).toString();
    out.push({ ...(await rec("block.make", 5, { ...base, overrides: { totalAmount: withItems } })), name: "totalAmount: transfer items included" });
    return out;
};
