"use strict";
// V19-fee-floor: the dynamic fee floor of the six transaction types over memo lengths, transfer
// items, vote entries and resignation types, under milestone variants (dynamic fees off, low and
// negative minFee, a missing addon key, sums past 2^53, a change of the table at a later height),
// and under milestone files whose fee or revoke-delay values the reference coerces or trips over
// when a fee is checked, while it loads the file.
// Transactions come from V02 (shared.tx). Returns the records.

const { milestonesText } = require("./milestones");

// The devnet's table, as the template milestone states it for the variants.
const DEVNET_TABLE = {
    enabled: "true",
    minFee: "6173",
    addonBytes: '{"burn":0,"delegateRegistration":1214968,"delegateResignation":0,"secondSignature":99,"transfer":85,"vote":98}',
};

/** A dynamicFees object as JSON text from member texts; undefined members are left out. */
function table({ enabled, minFee, addonBytes } = DEVNET_TABLE) {
    const members = [];
    if (enabled !== undefined) members.push(`"enabled":${enabled}`);
    if (minFee !== undefined) members.push(`"minFee":${minFee}`);
    if (addonBytes !== undefined) members.push(`"addonBytes":${addonBytes}`);
    return `{${members.join(",")}}`;
}

module.exports = async function fees({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const tx = shared.tx;
    const floor = async (name, height, hex, milestones) => {
        const input = milestones === undefined ? { hex } : { hex, milestones };
        const record = { ...(await rec("fee.minimum", height, input)), name };
        out.push(record);
        return record;
    };

    // ---- the devnet milestones: every built transaction of the six types -----------------------
    const devnetCases = [
        "transfer-1",
        "transfer-memo-empty",
        "transfer-memo-1",
        "transfer-1-memo",
        "transfer-memo-255",
        "transfer-memo-255-3byte",
        "transfer-memo-255-4byte",
        "transfer-memo-replacement",
        "transfer-2",
        "transfer-255",
        "transfer-256",
        "transfer-second-signed",
        "transfer-2-second-signed",
        "second-signature",
        "delegate-registration",
        "delegate-registration-second-signed",
        "resignation-0",
        "resignation-1",
        "resignation-2",
        "resignation-0-second-signed",
        "resignation-2-second-signed",
        "burn",
        "burn-memo",
        "burn-second-signed",
        "unvote",
        "vote-2-uneven",
        "vote-second-signed",
    ];
    for (let n = 1; n <= 53; n++) devnetCases.push(`vote-${n}`);
    for (const name of devnetCases) {
        if (!tx[name]) throw new Error(`V19 needs the V02 transaction ${name}`);
        must(await floor(name, H, tx[name].hex));
    }

    // ---- milestone variants, each on a transaction of every type -------------------------------
    const sample = ["transfer-1", "transfer-memo-1", "transfer-256", "second-signature", "delegate-registration", "resignation-2", "burn", "vote-53"];
    const variants = [
        ["no dynamicFees", milestonesText()],
        ["dynamicFees null", milestonesText({ dynamicFees: "null" })],
        ["the devnet table", milestonesText({ dynamicFees: table() })],
        ["enabled false", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, enabled: "false" }) })],
        ["no enabled key", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, enabled: undefined }) })],
        ["minFee 0", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "0" }) })],
        ["minFee -5", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "-5" }) })],
        ["minFee 1", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "1" }) })],
        ["minFee 6173.0", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "6173.0" }) })],
        ["no addon keys", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: "{}" }) })],
        ["no transfer addon", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: '{"vote":98,"delegateRegistration":1214968}' }) })],
        ["negative addons", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: '{"transfer":-1000,"vote":-98,"secondSignature":-99999999}' }) })],
        [
            "addons of 2^53 - 1, minFee 1",
            milestonesText({ dynamicFees: table({ enabled: "true", minFee: "1", addonBytes: '{"transfer":9007199254740991,"vote":9007199254740991,"secondSignature":9007199254740990,"delegateRegistration":-9007199254740991}' }) }),
        ],
        [
            "addons and minFee of 2^53 - 1",
            milestonesText({ dynamicFees: table({ enabled: "true", minFee: "9007199254740991", addonBytes: '{"transfer":9007199254740991,"vote":9007199254740989}' }) }),
        ],
    ];
    for (const [variant, milestones] of variants) {
        for (const name of sample) must(await floor(`${name}, ${variant}`, H, tx[name].hex, milestones));
    }

    // The table at the rule height: minFee changes at height 10, addonBytes merges key by key, and
    // the table is switched off at 20 and on again at 30.
    const changing = milestonesText({ dynamicFees: table() }, [
        '{"height":10,"dynamicFees":{"minFee":10000,"addonBytes":{"transfer":1}}}',
        '{"height":20,"dynamicFees":{"enabled":false}}',
        '{"height":30,"dynamicFees":{"enabled":true}}',
    ]);
    for (const height of [9, 10, 19, 20, 29, 30]) {
        for (const name of ["transfer-1", "vote-1"]) must(await floor(`${name}, table changing over heights`, height, tx[name].hex, changing));
    }

    // ---- files the reference loads although it coerces a value or fails when a fee is checked,
    // and that Heartwood refuses when it loads them --------------------------------------------
    const loose = [
        ["dynamicFees true", milestonesText({ dynamicFees: "true" })],
        ['dynamicFees "on"', milestonesText({ dynamicFees: '"on"' })],
        ["dynamicFees []", milestonesText({ dynamicFees: "[]" })],
        ["enabled 1", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, enabled: "1" }) })],
        ['enabled "false"', milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, enabled: '"false"' }) })],
        ["enabled 0", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, enabled: "0" }) })],
        ['minFee "6173"', milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: '"6173"' }) })],
        ["minFee 1.5", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "1.5" }) })],
        ["minFee null", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "null" }) })],
        ["minFee 9007199254740993", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: "9007199254740993" }) })],
        ["no minFee", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, minFee: undefined }) })],
        ["no addonBytes", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: undefined }) })],
        ["addonBytes null", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: "null" }) })],
        ['addon "85"', milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: '{"transfer":"85","burn":"0"}' }) })],
        ["addon 85.5", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: '{"transfer":85.5}' }) })],
        ["addon null", milestonesText({ dynamicFees: table({ ...DEVNET_TABLE, addonBytes: '{"transfer":null}' }) })],
        ["a disabled table with minFee \"6173\"", milestonesText({ dynamicFees: table({ enabled: "false", minFee: '"6173"', addonBytes: "{}" }) })],
        ['blocksToRevokeDelegateResignation "106"', milestonesText({ dynamicFees: table(), blocksToRevokeDelegateResignation: '"106"' })],
        ["blocksToRevokeDelegateResignation 106.5", milestonesText({ dynamicFees: table(), blocksToRevokeDelegateResignation: "106.5" })],
        ["blocksToRevokeDelegateResignation null", milestonesText({ dynamicFees: table(), blocksToRevokeDelegateResignation: "null" })],
        ["enabled at a later height without minFee", milestonesText({ dynamicFees: table({ enabled: "false", addonBytes: "{}" }) }, ['{"height":10,"dynamicFees":{"enabled":true}}'])],
    ];
    for (const [variant, milestones] of loose) {
        for (const name of ["transfer-1", "burn"]) await floor(`${name}, ${variant}`, H, tx[name].hex, milestones);
    }
    // At the height where the table without minFee is switched on, the reference throws.
    await floor("transfer-1, enabled at a later height without minFee", 10, tx["transfer-1"].hex, loose[loose.length - 1][1]);
    return out;
};
