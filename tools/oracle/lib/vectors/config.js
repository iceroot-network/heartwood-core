"use strict";
// V12 milestone configuration: the configuration manager's merge and start-up checks on milestone
// files given as JSON text (each value kept as it was written, so a loader reads it as it reads a
// file), and the transaction rules of a block at the block's own height.
//
// Accepted files record the merged milestones and the reward and donations at chosen heights;
// refused files record the error. Addresses come from V01 and V02.

const { FIXED_AUX } = require("../ops");
const { GENERATOR, SENDER } = require("./constants");
const { milestonesText } = require("./milestones");

const TIERS = [180000000, 190000000, 200000000, 210000000, 220000000];

/** A rank table as JSON text, ranks 1 to n, in tiers of 10, 11, 11, 11 and the rest. */
function ranksText(n, quote = false) {
    const tier = (rank) => (rank <= 10 ? 0 : rank <= 21 ? 1 : rank <= 32 ? 2 : rank <= 43 ? 3 : 4);
    const entries = [];
    for (let rank = 1; rank <= n; rank++) {
        const value = TIERS[tier(rank)];
        entries.push(`"${rank}":${quote ? `"${value}"` : value}`);
    }
    return `{${entries.join(",")}}`;
}

module.exports = async function config({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const [A, B] = [shared.recipients[10], shared.recipients[11]];
    const { badChecksum, addr63, addr30 } = shared.addresses;
    const M = milestonesText;
    const dynamic = (ranks) => `{"enabled":true,"secondaryReward":180000000,"ranks":${ranks}}`;
    const donations = (entries) => `{${entries.map(([address, value]) => `${JSON.stringify(address)}:${value}`).join(",")}}`;

    const load = async (name, milestones, queries = {}) => {
        out.push({ ...(await rec("config.load", H, { milestones, ...queries })), name });
    };
    const reward200 = "200000000";

    // ---- accepted ----------------------------------------------------------------------------------
    await load("template", M(), { rewards: [[1, 1]], donations: [[1, reward200]] });
    await load("ranks shrink from 53 to 51 keys, activeDelegates 51 from height 1", M({ activeDelegates: "51", dynamicReward: dynamic(ranksText(53)) }, [`{"height":102,"dynamicReward":{"ranks":${ranksText(51)}}}`]), {
        rewards: [[1, 51], [1, 52], [1, 53], [102, 1], [102, 51], [102, 52]],
    });
    await load(
        "donations replaced, then cleared by {}",
        M({ donations: donations([[A, '{"basisPoints":500,"purpose":"a"}'], [B, '{"basisPoints":500,"purpose":"b"}']]) }, [`{"height":5,"donations":${donations([[A, '{"basisPoints":700}']])}}`, '{"height":9,"donations":{}}']),
        { donations: [[1, reward200], [4, reward200], [5, reward200], [9, reward200], [5, "19"], [5, "0"]] },
    );
    await load("reward 0 at a later height", M({ reward: "200000000" }, ['{"height":10,"reward":0}']), { rewards: [[9, 1], [10, 1]] });
    await load("dynamicReward null at a later height", M({ reward: "150000000", dynamicReward: dynamic(ranksText(53)) }, ['{"height":10,"dynamicReward":null}']), {
        rewards: [[9, 1], [9, 53], [10, 1], [10, 53]],
    });
    await load("no reward key", M({ reward: undefined }), { rewards: [[1, 1]] });
    await load("rank table with decimal-string rewards", M({ dynamicReward: dynamic(ranksText(53, true)) }), { rewards: [[1, 1], [1, 53]] });
    await load("donations summing to exactly 10000", M({ donations: donations([[A, '{"basisPoints":5000}'], [B, '{"basisPoints":5000}']]) }), { donations: [[1, reward200]] });
    await load("blockTime and activeDelegates restated at later heights", M({}, ['{"height":107,"blockTime":8}', '{"height":213,"activeDelegates":53}']));
    await load("blockTime 8.0 and activeDelegates 53.0 restated", M({}, ['{"height":107,"blockTime":8.0,"activeDelegates":53.0}']));
    await load("blockTime 8.0000000000000001 restated", M({}, ['{"height":107,"blockTime":8.0000000000000001}']));
    await load("blockTime 8e0 and activeDelegates 5.3e1 at height 1", M({ blockTime: "8e0", activeDelegates: "5.3e1" }));
    await load("blockTime and activeDelegates 2^53 - 1 at height 1", M({ blockTime: "9007199254740991", activeDelegates: "9007199254740991" }));
    for (const value of ["2000000.0", "2e6", "-0", "0", "9007199254740991"]) {
        await load(`burn.txAmount ${value}`, M({ burn: `{"feeBasisPoints":9000,"txAmount":${value}}` }));
    }
    await load("a later milestone that sets only feeBasisPoints", M({}, ['{"height":100,"burn":{"feeBasisPoints":5000}}']));

    // ---- refused: donations ------------------------------------------------------------------------
    const refusedDonations = [
        ["a sum of 10001", donations([[A, '{"basisPoints":5001}'], [B, '{"basisPoints":5000}']])],
        ["an entry null", donations([[A, "null"]])],
        ['basisPoints "500"', donations([[A, '{"basisPoints":"500"}']])],
        ["basisPoints NaN (not JSON)", donations([[A, '{"basisPoints":NaN}']])],
        ["basisPoints 0", donations([[A, '{"basisPoints":0}']])],
        ["basisPoints 1.5", donations([[A, '{"basisPoints":1.5}']])],
        ["basisPoints 10001", donations([[A, '{"basisPoints":10001}']])],
        ["an unknown key", donations([[A, '{"basisPoints":500,"share":5}']])],
        ["a leftover percent", donations([[A, '{"percent":5,"purpose":"development"}']])],
        ["an address with a bad checksum", donations([[badChecksum, '{"basisPoints":500}']])],
        ["a byte-63 address", donations([[addr63, '{"basisPoints":500}']])],
        ["a byte-30 address", donations([[addr30, '{"basisPoints":500}']])],
        ["donations null", "null"],
        ["donations as an array", "[]"],
        ["a purpose that is not text", donations([[A, '{"basisPoints":500,"purpose":5}']])],
    ];
    for (const [name, value] of refusedDonations) await load(`donations: ${name}`, M({ donations: value }));
    await load("donations: a bad value at a later height", M({}, [`{"height":100,"donations":${donations([[A, '{"basisPoints":0}']])}}`]));

    // ---- refused: burn -----------------------------------------------------------------------------
    for (const [name, value] of [
        ['feeBasisPoints "9000"', '{"feeBasisPoints":"9000","txAmount":2000000}'],
        ["feeBasisPoints 90.5", '{"feeBasisPoints":90.5,"txAmount":2000000}'],
        ["feeBasisPoints -1", '{"feeBasisPoints":-1,"txAmount":2000000}'],
        ["feeBasisPoints 10001", '{"feeBasisPoints":10001,"txAmount":2000000}'],
        ["no burn", undefined],
        ["burn null", "null"],
        ["feePercent 90", '{"feePercent":90,"txAmount":2000000}'],
        ["feePercent next to feeBasisPoints", '{"feeBasisPoints":9000,"feePercent":90,"txAmount":2000000}'],
        ['txAmount "2000000"', '{"feeBasisPoints":9000,"txAmount":"2000000"}'],
        ["txAmount 1.5", '{"feeBasisPoints":9000,"txAmount":1.5}'],
        ["txAmount -1", '{"feeBasisPoints":9000,"txAmount":-1}'],
        ["txAmount missing", '{"feeBasisPoints":9000}'],
        ["txAmount null", '{"feeBasisPoints":9000,"txAmount":null}'],
        ["txAmount 2^53", '{"feeBasisPoints":9000,"txAmount":9007199254740992}'],
        ["txAmount 9007199254740993", '{"feeBasisPoints":9000,"txAmount":9007199254740993}'],
    ]) {
        await load(`burn: ${name}`, M({ burn: value }));
    }
    await load("burn: feeBasisPoints 10001 at a later height", M({}, ['{"height":100,"burn":{"feeBasisPoints":10001}}']));
    await load('burn: txAmount "3000000" at a later height', M({}, ['{"height":100,"burn":{"txAmount":"3000000"}}']));

    // ---- refused: block time and active delegates -------------------------------------------------
    await load("blockTime 8, then 6 at height 107", M({}, ['{"height":107,"blockTime":6}']));
    await load("activeDelegates 53, then 51 at height 107", M({}, ['{"height":107,"activeDelegates":51}']));
    await load('a later blockTime "8"', M({}, ['{"height":50,"blockTime":"8"}']));
    await load("a later blockTime null", M({}, ['{"height":50,"blockTime":null}']));
    for (const key of ["blockTime", "activeDelegates"]) {
        for (const [label, value] of [
            ["missing", undefined],
            ['"8"', '"8"'],
            ["0", "0"],
            ["-8", "-8"],
            ["1.5", "1.5"],
            ["null", "null"],
            ["2^53", "9007199254740992"],
            ["9007199254740993", "9007199254740993"],
            ["-0", "-0"],
        ]) {
            await load(`${key} ${label} at height 1`, M({ [key]: value }));
        }
    }

    // ---- refused: rank table ----------------------------------------------------------------------
    await load("rank table without rank 53", M({ dynamicReward: dynamic(ranksText(52)) }));
    await load("rank table with a negative reward", M({ dynamicReward: dynamic(ranksText(53).replace('"53":220000000', '"53":-1')) }));

    // ---- transaction rules at the block's height ------------------------------------------------
    // transfer.maximum drops to 2 at height 10: a 3-recipient transfer is valid in a block at
    // height 9 and refused at height 10.
    const rules = M({}, ['{"height":10,"transfer":{"maximum":2}}']);
    await load("transfer.maximum 2 from height 10", rules);
    const three = { ...must(await rec("tx.build", H, { builder: "transfer", calls: shared.recipients.slice(20, 23).map((r, i) => ["addTransfer", r, String(10 + i)]).concat([["nonce", "500"]]), sign: SENDER })), name: "3-recipient transfer" };
    out.push(three);
    const aux = FIXED_AUX.toString("hex");
    for (const height of [9, 10]) {
        out.push({ ...(await rec("tx.fromBytes", height, { hex: three.output.hex, milestones: rules })), name: `3-recipient transfer at height ${height}` });
        const input = { height, timestamp: 8 * (height - 1), previousBlock: "12".repeat(32), reward: "0", passphrase: GENERATOR, aux, transactions: [three.output.hex], milestones: rules };
        out.push({ ...(await rec("block.make", height, input)), name: `3-recipient transfer in a block at height ${height}` });
    }
    return out;
};
