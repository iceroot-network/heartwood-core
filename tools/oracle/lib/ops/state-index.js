"use strict";
// Oracle operation of the state's index orders, through the reference's own state classes
// (wallets/wallet-index.ts, wallet-repository.ts, the handlers' deferred re-indexing, state-saver.ts,
// state-loader.ts, state-builder.ts) over a chain session (lib/state.js):
//
//   state.indexOrders {corpus, manifest, line}
//       corpus    the name of a corpus directory (tools/difftest/corpus, the image's oracle/corpus,
//                 or target/corpus of the repository), whose MANIFEST.sha256 file has the SHA-256
//                 `manifest`
//       line      the digest line of the corpus's continuous replay to stop after: line 1 is the
//                 first start, then one line per block applied and per block reverted or applied
//                 again by the corpus's revert plan, as replay-chain.js writes them
//       -> {height, live, saved, rebuild, replayed, notes}: each of live, saved, rebuild and
//          replayed is {addresses, publicKeys, usernames}, the keys of the address, public-key and
//          username indexes in their order (allByAddress, allByPublicKey, allByUsername):
//            live      the session after the steps up to `line`
//            saved     a node started again from the saved state of that session (StateSaver, then
//                      StateLoader, chain.restart {mode: "saved"})
//            rebuild   a node started again with no saved state (StateBuilder, chain.restart
//                      {mode: "rebuild"})
//            replayed  a new session that applies the stored blocks up to the tip once each, in
//                      height order, with no revert: the order the chain's blocks set live
//          A restart the reference refuses gives {refused: reason} in place of its orders.
//          notes.rebuildOrder: the StateBuilder orders differ from the replayed ones;
//          notes.savedStateUsername: a delegate name or a voted name reads as an integer, which the
//          saved-state reviver turns into a number.
//
// The live order is the reference's: an address is set when a lookup first creates its wallet,
// and after each block the handlers re-index every transaction's sender and recipients
// (setImmediate, which the session lets run after each block), which moves each wallet's address,
// public key and name to the end of their indexes, or drops the address of a wallet that holds
// nothing. The session changes neither lib/ops/chain.js nor lib/state.js.
// Loaded by lib/ops.js.

const fs = require("fs");
const path = require("path");

const { integerLikeNames, loadCorpus, ReplaySession } = require("../corpus");

// Where a corpus named in a record is looked for: the image's copy, the repository's committed
// corpora, and the repository's working corpora.
const CORPUS_DIRS = [
    path.join(__dirname, "..", "..", "corpus"),
    path.join(__dirname, "..", "..", "..", "difftest", "corpus"),
    path.join(__dirname, "..", "..", "..", "..", "target", "corpus"),
];

/** The directory of the corpus `name`, or null. */
function findCorpus(name) {
    if (typeof name !== "string" || !/^[a-z0-9][a-z0-9-]*$/.test(name)) throw new TypeError("corpus must be a corpus name");
    for (const dir of CORPUS_DIRS) {
        const candidate = path.join(dir, name);
        if (fs.existsSync(path.join(candidate, "corpus.json"))) return candidate;
    }
    return null;
}

/**
 * The continuous replay's steps after the first start, as replay-chain.js runs them: each block
 * applied, then for each revert-plan step after it, its blocks reverted one at a time and applied
 * again in height order.
 */
function replaySteps(corpus) {
    const plan = new Map();
    for (const step of corpus.revertPlan.steps || []) {
        if (!plan.has(step.after)) plan.set(step.after, []);
        plan.get(step.after).push(step.blocks);
    }
    const steps = [];
    for (const block of corpus.blocks) {
        steps.push({ step: "apply", h: block.h });
        for (const count of plan.get(block.h) || []) {
            for (let i = 0; i < count; i++) steps.push({ step: "revert" });
            for (let h = block.h - count + 1; h <= block.h; h++) steps.push({ step: "reapply", h });
        }
    }
    return steps;
}

/** The keys of the three indexes of a chain's wallet repository, in their order. */
function orders(chain) {
    const keys = (name) => [...chain.walletRepository.getIndex(name).keys()];
    return { addresses: keys("addresses"), publicKeys: keys("publicKeys"), usernames: keys("usernames") };
}

const sameOrders = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/**
 * A chain session of `corpus` run through `steps`: the first start, then each step in turn. The
 * configuration manager must hold the corpus's configuration (the caller's HeadlessChain.withConfig).
 */
async function runSteps(solar, corpus, steps) {
    const session = new ReplaySession(solar, corpus.crypto);
    await session.start();
    const chain = session.chain;
    for (const { step, h } of steps) {
        if (step === "revert") {
            await chain.revert(1);
            continue;
        }
        const result = await chain.apply(Buffer.from(corpus.blocks[h - 2].hex, "hex"), step);
        if (!result.accepted) throw new Error(`block ${h} was refused at its ${step}: ${result.error.message}`);
    }
    return chain;
}

/** The orders of a node started again from `chain` by `mode`, or {refused: reason}. */
async function restarted(chain, mode) {
    const result = await chain.restart(mode);
    return result.refused ? { refused: result.refused.reason } : orders(result.chain);
}

module.exports = function stateIndexOps(context) {
    const { solar, devnet, safeInteger, sha256Hex } = context;
    return {
        "state.indexOrders": async ({ corpus: name, manifest, line }) => {
            safeInteger(line, "line");
            if (line < 1) throw new RangeError("line must be at least 1");
            if (typeof manifest !== "string") throw new TypeError("manifest must be the SHA-256 of MANIFEST.sha256");
            const dir = findCorpus(name);
            if (!dir) throw new Error(`no corpus ${name}`);
            const found = sha256Hex(fs.readFileSync(path.join(dir, "MANIFEST.sha256")));
            if (found !== manifest) throw new Error(`corpus ${name} has the manifest ${found}, not ${manifest}`);
            const corpus = loadCorpus(dir);
            const steps = replaySteps(corpus);
            if (line > steps.length + 1) throw new RangeError(`the replay of ${name} has ${steps.length + 1} lines`);
            // A session that only configures the corpus's chain, so that everything below runs
            // under it and the devnet configuration is restored afterwards.
            const scope = new ReplaySession(solar, corpus.crypto).chain;
            return scope.withConfig(
                () => devnet.restore(),
                async () => {
                    const chain = await runSteps(solar, corpus, steps.slice(0, line - 1));
                    const height = chain.lastBlock().data.height;
                    const live = orders(chain);
                    const notes = { rebuildOrder: false };
                    if (integerLikeNames(chain.wallets()).length) notes.savedStateUsername = true;
                    const saved = await restarted(chain, "saved");
                    const rebuild = await restarted(chain, "rebuild");
                    const forward = [];
                    for (let h = 2; h <= height; h++) forward.push({ step: "apply", h });
                    const replayed = orders(await runSteps(solar, corpus, forward));
                    notes.rebuildOrder = !sameOrders(rebuild, replayed);
                    return { height, live, saved, rebuild, replayed, notes };
                },
            );
        },
    };
};

module.exports.findCorpus = findCorpus;
module.exports.replaySteps = replaySteps;
module.exports.orders = orders;
