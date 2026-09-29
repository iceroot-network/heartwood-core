"use strict";
// Vector class V33-rollback: `snapshot:rollback` (chain.rollback) on the committed one-node corpus
// at its tip, 265 (rounds of 53): every refusal of the command and of the reference's snapshot
// service (no flag, a 0 for each flag, a negative height, the tip, a height above it, and heights
// of round 1), and rollbacks with --height at the first, a middle and the last block of rounds 2 to
// 5, with --number 1 and 60, with both flags, and with a --height of 0 beside --number. Each record
// that rolls back also holds the start after it: chain.restart without a saved state, its line and
// the store's summary after it, and the rows the start saved for the round after the target (the
// `active` rows of the #R record of chain.dump, sorted by public key: they are compared as a set).
//
//   {op: "chain.rollback", height: 265, name, input: {corpus, height?, number?},
//    output: {rollback, restart?: {line, summary, notes?}, active?}}
//
// `corpus` names the corpus directory: `corpus/` beside the oracle scripts in the image, and the
// difftest's committed corpora in the repository. Each rollback runs on a session of its own; the
// refusals, which change nothing, share one.

const fs = require("fs");
const path = require("path");

const CORPUS = "txgen-one-node";
const TIP = 265;

/** The corpus directory of `name`, in the image or in the repository. */
function corpusDir(name) {
    const places = [path.join(__dirname, "..", "..", "corpus", name), path.join(__dirname, "..", "..", "..", "difftest", "corpus", name)];
    const found = places.find((dir) => fs.existsSync(path.join(dir, "corpus.json")));
    if (!found) throw new Error(`V33-rollback: no corpus ${name} in ${places.join(" or ")}`);
    return found;
}

/** The cases: [name, flags], the refusals first. */
const REFUSALS = [
    ["no flag", {}],
    ["--height 0", { height: 0 }],
    ["--number 0", { number: 0 }],
    ["--height 0 --number 0", { height: 0, number: 0 }],
    ["--height -5", { height: -5 }],
    ["--number 265", { number: 265 }],
    ["--number -5", { number: -5 }],
    ["--height 265 (the tip)", { height: 265 }],
    ["--height 300", { height: 300 }],
    ["--height 1", { height: 1 }],
    ["--height 40", { height: 40 }],
    ["--height 53", { height: 53 }],
];
const ROLLBACKS = [
    ["--height 54", { height: 54 }],
    ["--height 60", { height: 60 }],
    ["--height 80", { height: 80 }],
    ["--height 106", { height: 106 }],
    ["--height 107", { height: 107 }],
    ["--height 133", { height: 133 }],
    ["--height 159", { height: 159 }],
    ["--height 160", { height: 160 }],
    ["--height 180", { height: 180 }],
    ["--height 212", { height: 212 }],
    ["--height 213", { height: 213 }],
    ["--height 239", { height: 239 }],
    ["--height 264", { height: 264 }],
    ["--number 1", { number: 1 }],
    ["--number 60", { number: 60 }],
    ["--height 133 --number 60", { height: 133, number: 60 }],
    ["--height 0 --number 60", { height: 0, number: 60 }],
];

module.exports = async function rollbackVectors({ devnet, rec, must }) {
    const dir = corpusDir(CORPUS);
    const output = async (op, input) => must(await rec(op, 1, input)).output;
    const open = async () => (await output("chain.open", { corpus: dir })).session;
    const record = (name, flags, out) => ({
        op: "chain.rollback",
        network: devnet.info.name,
        height: TIP,
        name,
        input: { corpus: CORPUS, ...flags },
        output: out,
    });
    const records = [];
    const shared = await open();
    for (const [name, flags] of REFUSALS) {
        const rollback = await output("chain.rollback", { session: shared, ...flags });
        if (!rollback.refused) throw new Error(`V33-rollback: ${name} was not refused`);
        records.push(record(name, flags, { rollback }));
    }
    await output("chain.close", { session: shared });
    for (const [name, flags] of ROLLBACKS) {
        const session = await open();
        const rollback = await output("chain.rollback", { session, ...flags });
        if (rollback.refused) throw new Error(`V33-rollback: ${name} was refused`);
        const restart = await output("chain.restart", { session, mode: "rebuild" });
        if (restart.refused) throw new Error(`V33-rollback: the start after ${name} was refused`);
        const dump = await output("chain.dump", { session });
        const round = JSON.parse(dump.sections.R.split("\n")[1]);
        const active = [...round.active].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
        await output("chain.close", { session });
        records.push(record(name, flags, { rollback, restart, active }));
    }
    return records;
};
