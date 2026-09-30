"use strict";
// The records of kind state.indexOrders in V34: the live, saved-state, StateBuilder and replayed
// orders (lib/ops/state-index.js) over the one-node corpus, at digest lines of its continuous
// replay (its revert plan included) chosen by rule: the first start; each block, applied or
// applied again, where a public key or a delegate name lands; each revert that forgets one; the
// first block of each round end; the tip; then as many lines spread evenly over the replay as
// make about LINES records per corpus. A line where a reference restart is refused is left out.
//
// Only the one-node corpus is in the repository. STATE_INDEX_CORPORA (corpus names separated by
// commas, looked up as the operation looks them up) adds corpora for a local run, such as the
// fault and restart corpora of target/corpus; the committed class never has them.
// The builder returns its records.

const fs = require("fs");
const path = require("path");

const { loadCorpus, ReplaySession } = require("../corpus");
const { sha256Hex } = require("../solar");
const { findCorpus, orders, replaySteps } = require("../ops/state-index");

const CORPORA = ["txgen-one-node"];
const EXTRA = "STATE_INDEX_CORPORA";
// The records per corpus that the spread lines make up to.
const LINES = 26;
// The lines of each kind that are chosen at most.
const PER_KIND = 3;

/**
 * The digest lines of `corpus`'s continuous replay to record, each with its tip height and why it
 * was chosen, in line order.
 */
async function chooseLines(solar, devnet, corpus) {
    const steps = replaySteps(corpus);
    const roundLength = corpus.revertPlan.roundLength || corpus.crypto.milestones[0].activeDelegates;
    const chosen = new Map();
    const counts = new Map();
    const choose = (line, height, why) => {
        if (chosen.has(line) || (counts.get(why) || 0) >= PER_KIND) return;
        counts.set(why, (counts.get(why) || 0) + 1);
        chosen.set(line, { line, height, why });
    };
    const session = new ReplaySession(solar, corpus.crypto);
    await session.chain.withConfig(
        () => devnet.restore(),
        async () => {
            await session.start();
            const chain = session.chain;
            const keySets = () => {
                const o = orders(chain);
                return { keys: new Set(o.publicKeys), names: new Set(o.usernames) };
            };
            chosen.set(1, { line: 1, height: 1, why: "first start" });
            let before = keySets();
            const roundEnds = new Set();
            for (let i = 0; i < steps.length; i++) {
                const { step, h } = steps[i];
                if (step === "revert") await chain.revert(1);
                else await chain.apply(Buffer.from(corpus.blocks[h - 2].hex, "hex"), step);
                const line = i + 2;
                const height = chain.lastBlock().data.height;
                const after = keySets();
                const added = (a, b) => [...b].some((key) => !a.has(key));
                if (step !== "revert" && added(before.names, after.names)) choose(line, height, `a delegate name lands (${step})`);
                if (step !== "revert" && added(before.keys, after.keys)) choose(line, height, `a public key lands (${step})`);
                if (step === "revert" && added(after.names, before.names)) choose(line, height, "a revert forgets a delegate name");
                if (step === "revert" && added(after.keys, before.keys)) choose(line, height, "a revert forgets a public key");
                if (step === "apply" && height % roundLength === 0 && !roundEnds.has(height)) {
                    roundEnds.add(height);
                    choose(line, height, "a round end");
                }
                before = after;
            }
            chosen.set(steps.length + 1, { line: steps.length + 1, height: chain.lastBlock().data.height, why: "the tip" });
        },
    );
    // Spread lines: the heights of the replay's lines are needed, so they come from the steps.
    const heights = [1];
    let tip = 1;
    for (const { step, h } of steps) {
        tip = step === "revert" ? tip - 1 : h;
        heights.push(tip);
    }
    const total = steps.length + 1;
    const needed = Math.max(0, LINES - chosen.size);
    for (let k = 1; k <= needed; k++) {
        const line = Math.max(1, Math.round((k * total) / (needed + 1)));
        if (!chosen.has(line)) chosen.set(line, { line, height: heights[line - 1], why: "spread" });
    }
    return [...chosen.values()].sort((a, b) => a.line - b.line);
}

module.exports = async function stateIndexVectors({ solar, devnet, rec, must }) {
    const extra = (process.env[EXTRA] || "")
        .split(",")
        .map((name) => name.trim())
        .filter(Boolean);
    const out = [];
    for (const name of [...CORPORA, ...extra]) {
        const dir = findCorpus(name);
        if (!dir) {
            if (CORPORA.includes(name)) continue;
            throw new Error(`${EXTRA} names ${name}, which is not a corpus here`);
        }
        const manifest = sha256Hex(fs.readFileSync(path.join(dir, "MANIFEST.sha256")));
        const corpus = loadCorpus(dir);
        for (const { line, height, why } of await chooseLines(solar, devnet, corpus)) {
            const record = must(await rec("state.indexOrders", height, { corpus: name, manifest, line }));
            if (record.output.saved.refused || record.output.rebuild.refused) continue;
            out.push({ ...record, name: `${name}, line ${line}: ${why}` });
        }
    }
    return out;
};
