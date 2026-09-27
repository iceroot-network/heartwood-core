"use strict";
// V23-config: the crypto files of a network as the kernel's configuration driver loads them
// (config.load with `files`). The base is the oracle's devnet: its network, its milestones (the
// merged mainnet rule set with the devnet's leaves), exceptions {} and an empty genesis block.
// Each record changes one thing:
//   - nothing, and forms the strict schema accepts (removed features at their off values, fee
//     tables with every type key, an empty minimum-versions list): both accept;
//   - a missing file: the driver loads none of the four and falls back to its built-in preset;
//   - a file that is not JSON: both refuse;
//   - each kind the strict schema refuses, at each level (an unknown key, a null, a value of
//     another type, a height twice, a removed feature turned on, a non-empty exceptions file): the
//     reference accepts most of them;
//   - values the milestone and network loaders refuse where the reference loads the file (an
//     epoch or block version of another type, a first milestone above height 1, a missing network
//     key), and the fee and revoke keys' types;
//   - files both refuse: a block time that changes, burn.feePercent, donations over 100 %.

const FILES = ["network.json", "milestones.json", "genesisBlock.json", "exceptions.json"];

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function text(value) {
    return JSON.stringify(value, null, 4) + "\n";
}

/** Set `dotted` of `target` to `value`, creating objects on the way; undefined removes the key. */
function setPath(target, dotted, value) {
    const keys = dotted.split(".");
    let node = target;
    for (const key of keys.slice(0, -1)) {
        if (node[key] === null || typeof node[key] !== "object") node[key] = {};
        node = node[key];
    }
    const last = keys[keys.length - 1];
    if (value === undefined) delete node[last];
    else node[last] = value;
}

module.exports = async function configSchema({ devnet, rec, shared }) {
    const out = [];
    const H = shared.height;
    const base = {
        "network.json": text(devnet.config.network),
        "milestones.json": text(devnet.config.milestones),
        "genesisBlock.json": text(devnet.config.genesisBlock),
        "exceptions.json": text(devnet.config.exceptions),
    };
    const push = async (name, files) => out.push({ ...(await rec("config.load", H, { files })), name });
    /** The base files with the milestone list changed by `change`. */
    const milestones = (change) => {
        const list = clone(devnet.config.milestones);
        change(list);
        return { ...base, "milestones.json": text(list) };
    };
    const first = (dotted, value) => milestones((list) => setPath(list[0], dotted, value));
    const later = (dotted, value) => milestones((list) => setPath(list[1], dotted, value));
    const network = (dotted, value) => {
        const file = clone(devnet.config.network);
        setPath(file, dotted, value);
        return { ...base, "network.json": text(file) };
    };
    const typeKeys = ["legacyTransfer", "secondSignature", "delegateRegistration", "legacyVote", "multiSignature", "ipfs", "transfer", "delegateResignation", "htlcLock", "htlcClaim", "htlcRefund", "burn", "vote"];

    // Accepted by both.
    await push("the devnet's files", base);
    await push("removed features at their off values", milestones((list) => {
        for (const entry of list) Object.assign(entry, { legacyTransfer: false, legacyVote: false, bip340: true, acceptLegacySchnorrTransactions: false, htlcEnabled: false });
    }));
    await push("fee tables with every type key", milestones((list) => {
        list[0].fees.staticFees = Object.fromEntries(typeKeys.map((key, i) => [key, i * 1000]));
        list[0].dynamicFees.addonBytes = Object.fromEntries(typeKeys.map((key, i) => [key, i]));
    }));
    await push("no minimum versions", first("p2p.minimumVersions", []));
    await push("integers written as other numbers", first("fees.staticFees.transfer", 5e7));
    await push("exceptions with whitespace", { ...base, "exceptions.json": " {\n}\n" });

    // A missing file.
    for (const file of FILES) {
        const files = { ...base };
        delete files[file];
        await push(`${file} missing`, files);
    }
    await push("network.json and exceptions.json missing", { "milestones.json": base["milestones.json"], "genesisBlock.json": base["genesisBlock.json"] });
    await push("every file missing", {});

    // A file that is not JSON.
    for (const file of FILES) await push(`${file} is not JSON`, { ...base, [file]: "{" });

    // Unknown keys.
    const unknown = [
        ["aip11", true],
        ["vendorFieldLength", 255],
        ["multiPaymentLimit", 64],
        ["block.idFullSha256", true],
        ["dynamicFees.extra", 1],
        ["dynamicFees.addonBytes.payment", 1],
        ["fees.dynamic", {}],
        ["fees.staticFees.entity", 1],
        ["transfer.max", 10],
        ["dynamicReward.tiers", []],
        ["p2p.maxPeers", 10],
        ["burn.feePercent", 90],
    ];
    for (const [dotted, value] of unknown) await push(`unknown key ${dotted} at height 1`, first(dotted, value));
    await push("unknown key htlc at height 2", later("htlc", { enabled: false }));
    for (const [dotted, value] of [["wallet", "x"], ["bip32.seed", 1], ["client.url", "https://example.org"]]) {
        await push(`unknown network key ${dotted}`, network(dotted, value));
    }

    // Nulls.
    const nulls = ["reward", "epoch", "dynamicFees", "dynamicFees.minFee", "fees.staticFees.transfer", "dynamicReward.ranks.60", "donations", "legacyVote", "htlcEnabled", "blocksToRevokeDelegateResignation"];
    for (const dotted of nulls) await push(`${dotted} null at height 1`, first(dotted, null));
    await push("a null inside dynamicReward.enabled", first("dynamicReward.enabled", { on: null }));
    await push("a null in p2p.minimumVersions", first("p2p.minimumVersions", [">=4.3.0", null]));
    await push("donations null at height 2", later("donations", null));
    for (const dotted of ["client.explorer", "nethash", "addressCharacter"]) await push(`network ${dotted} null`, network(dotted, null));

    // Values of another type than the strict schema's.
    const types = [
        ["fees", 5],
        ["fees.staticFees", []],
        ["p2p", []],
        ["p2p.minimumVersions", ">=4.3.0"],
        ["p2p.minimumVersions", [4, 3]],
        ["dynamicReward", true],
        ["fees.staticFees.transfer", -1],
        ["fees.staticFees.transfer", "50000000"],
        ["fees.staticFees.vote", 1.5],
        ["dynamicFees.addonBytes.transfer", -85],
    ];
    for (const [dotted, value] of types) await push(`${dotted} ${JSON.stringify(value)}`, first(dotted, value));

    // A height twice.
    await push("height 2 twice", milestones((list) => list.push(clone(list[1]))));
    await push("height 2 twice with other values", milestones((list) => list.push({ height: 2, reward: 5 })));
    await push("height 1 twice", milestones((list) => list.push({ height: 1 })));

    // Removed features turned on.
    const removed = [
        ["legacyTransfer", true],
        ["legacyVote", true],
        ["bip340", false],
        ["acceptLegacySchnorrTransactions", true],
        ["htlcEnabled", true],
        ["legacyTransfer", 0],
        ["htlcEnabled", "false"],
    ];
    for (const [key, value] of removed) await push(`${key} ${JSON.stringify(value)} at height 2`, later(key, value));
    await push("htlcEnabled true at height 1", first("htlcEnabled", true));

    // exceptions.json other than {}.
    for (const exceptions of ['{"transactions": ["ab"]}', '{"blocks": []}', "[]", "null", "0"]) {
        await push(`exceptions ${exceptions}`, { ...base, "exceptions.json": exceptions });
    }

    // Values the loaders refuse where the reference loads the file.
    const loaders = [
        ["epoch", 0],
        ["epoch", "2026-01-01"],
        ["block.version", "0"],
        ["block", "block"],
        ["block.maxTransactions", 0],
        ["transfer.minimum", "1"],
        ["reward", true],
        ["dynamicReward.ranks.60", "x"],
    ];
    for (const [dotted, value] of loaders) await push(`${dotted} ${JSON.stringify(value)} at height 1`, first(dotted, value));
    await push("the first milestone at height 2", milestones((list) => (list[0].height = 2)));
    await push("height \"2\" as text", later("height", "2"));
    for (const [dotted, value] of [["nethash", undefined], ["client", undefined], ["name", 5], ["wif", 256]]) {
        await push(`network ${dotted} ${value === undefined ? "missing" : JSON.stringify(value)}`, network(dotted, value));
    }

    // The fee and revoke keys.
    for (const [dotted, value] of [["dynamicFees.minFee", "6173"], ["dynamicFees.enabled", "yes"], ["blocksToRevokeDelegateResignation", "106"], ["dynamicFees.minFee", undefined]]) {
        await push(`${dotted} ${value === undefined ? "missing" : JSON.stringify(value)}`, first(dotted, value));
    }

    // Refused by both.
    await push("blockTime 9 at height 2", later("blockTime", 9));
    await push("donations over 100 %", milestones((list) => {
        const donations = list[1].donations;
        const [address] = Object.keys(donations);
        donations[address].basisPoints = 9999;
    }));
    await push("the milestones are an object", { ...base, "milestones.json": "{}" });
    await push("pubKeyHash as text", network("pubKeyHash", "90"));
    return out;
};
