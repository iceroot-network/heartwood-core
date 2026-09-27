"use strict";
// Oracle operations over the reference's own state machine, run headless in this process
// (lib/state.js): state.digest and state.applyTransaction on wallet fixtures, and the chain
// sessions chain.open, chain.apply, chain.revert, chain.dump and chain.close. Loaded by
// lib/ops.js.
//
// A fixture is the JSON array of tools/difftest's fixture module: wallets in the shape of the
// digest's #W records, without vb, vc and rk, plus resignedAt.
//
//   state.digest {fixture, milestones?} or {crypto: {network, milestones, genesisBlock, exceptions}}
//       -> {sections: {B, S, R, M, W}, hashes: {B, S, R, M, W}, sd}: the digest of the fixture's
//          state (lib/digest.js; #B, #R and #M are empty for a fixture), or of the state after
//          the reference's first start on the chain's genesis block
//   state.applyTransaction {fixture, hex, lastBlockHeight, milestones?}
//       the transaction applied as block lastBlockHeight + 1 applies it, under the rules of that
//       height, then reverted:
//       -> {applied: {W}, reverted: {W}}, the #W section after each; or, when the reference
//          refuses it, {error: {class, message}, after: {W}}, the #W section after the refused
//          attempt (the reference binds the sender's key before its checks)
//
//   chain.open {crypto: {network, milestones, genesisBlock, exceptions}, milestones?}
//              or {corpus: DIR, height?, milestones?}
//       the reference's first start on the chain's genesis block; with a corpus directory, its
//       blocks are applied up to `height` (the tip by default). `milestones` (JSON text) replaces
//       the chain's milestone file.
//       -> {session, lines: [...]}, the digest lines of the start and of each block applied
//   chain.apply {session, hex, step?}   step "apply" (default) or "reapply"
//       -> {accepted, height, id, reward, missed: [{timestamp, height, username}], line} or,
//          refused, {accepted: false, ..., stage, error: {class, message}}; stage is decode (the
//          bytes do not decode), verify (the block's own checks, the wall-clock rule on its
//          timestamp aside) or apply
//   chain.revert {session, count}      -> {lines}, one revert line per block
//   chain.restart {session, mode}      the node stopped at its tip and started again (lib/state.js):
//       mode "rebuild" (no saved state: StateBuilder) or "saved" (StateSaver, then StateLoader)
//       -> {line}, the restart's digest line (step "restart"), or {refused: {reason}}, the text
//          the reference's start ended with; the session goes on with the restarted node.
//          notes.savedStateUsername is true for a saved-state restart with a delegate name or a
//          voted name that reads as an integer
//   chain.replaceStored {session, height, index, hex}
//       replace the stored bytes of transaction `index` of the stored block at `height`, as a
//       database changed on disk would hold them (the state is unchanged; a later restart reads
//       them) -> {replaced: true}
//   chain.dump {session}               -> {sections: {B, S, R, M, W}, line}: the records of the
//                                          last step
//   chain.close {session}              -> {closed: true}
//
// Every chain operation loads its session's configuration and restores the devnet configuration
// afterwards, so sessions and the other operations can share the process.

const path = require("path");

const { loadCorpus } = require("../corpus");
const { ReplaySession } = require("../corpus");
const digest = require("../digest");
const { FixtureState } = require("../state");

module.exports = function chainOps(context) {
    const { solar, devnet, hexBuffer, safeInteger, describe, withConfig } = context;
    const { BigNumber } = solar.lib.Utils;
    let fixtureState;
    const sessions = new Map();
    let nextSession = 1;

    const fixtures = () => {
        if (!fixtureState) fixtureState = new FixtureState(solar);
        return fixtureState;
    };

    const session = (id) => {
        const s = sessions.get(id);
        if (!s) throw new Error(`no chain session ${id}`);
        return s;
    };

    // Run fn under the session's configuration, then restore the devnet configuration.
    const inSession = (s, fn) => s.chain.withConfig(() => devnet.restore(), fn);

    const sectionHashes = (sections) => {
        const { h, sd } = digest.hashes(sections);
        const out = {};
        for (const [k, v] of Object.entries(h)) out[k] = v.toString("hex");
        return { hashes: out, sd: sd.toString("hex") };
    };

    return {
        "state.digest": async ({ fixture, crypto, milestones }, height) => {
            if ((fixture === undefined) === (crypto === undefined)) throw new TypeError("state.digest takes a fixture or a chain's crypto files");
            if (crypto !== undefined) {
                const s = new ReplaySession(solar, crypto);
                const sections = await inSession(s, async () => {
                    await s.start();
                    return s.sections();
                });
                return { sections, ...sectionHashes(sections) };
            }
            return withConfig(milestones, async () => {
                const state = fixtures();
                await state.load(fixture, height);
                const sections = digest.fixtureSections(state.chain.wallets(), BigNumber);
                return { sections, ...sectionHashes(sections) };
            });
        },

        "state.applyTransaction": async ({ fixture, hex, lastBlockHeight, milestones }) => {
            safeInteger(lastBlockHeight, "lastBlockHeight");
            return withConfig(milestones, async () => {
                const state = fixtures();
                await state.load(fixture, lastBlockHeight);
                const result = await state.applyTransaction(hexBuffer(hex, "hex"), lastBlockHeight, digest.walletSection);
                if (result.error) return { error: describe(result.error), after: { W: result.after } };
                return { applied: { W: result.applied }, reverted: { W: result.reverted } };
            });
        },

        "chain.open": async ({ crypto, corpus, height, milestones }) => {
            let config = crypto;
            let blocks = [];
            if (corpus !== undefined) {
                const loaded = loadCorpus(path.resolve(corpus));
                config = loaded.crypto;
                blocks = loaded.blocks;
                if (height !== undefined) {
                    safeInteger(height, "height");
                    blocks = blocks.filter((b) => b.h <= height);
                }
            }
            if (!config || typeof config !== "object") throw new TypeError("chain.open needs crypto or corpus");
            config = JSON.parse(JSON.stringify(config));
            if (milestones !== undefined) {
                if (typeof milestones !== "string") throw new TypeError("milestones must be JSON text");
                config.milestones = JSON.parse(milestones);
            }
            const s = new ReplaySession(solar, config);
            const id = nextSession++;
            const lines = [];
            await inSession(s, async () => {
                lines.push(await s.start());
                for (const b of blocks) {
                    const r = await s.apply(Buffer.from(b.hex, "hex"), "apply");
                    if (!r.accepted) throw new Error(`block ${b.h} of the corpus was refused: ${r.error.class}: ${r.error.message}`);
                    lines.push(r.line);
                }
            });
            sessions.set(id, s);
            return { session: id, lines };
        },

        "chain.apply": async ({ session: id, hex, step = "apply" }) => {
            if (step !== "apply" && step !== "reapply") throw new TypeError("step must be apply or reapply");
            const s = session(id);
            const bytes = hexBuffer(hex, "hex");
            return inSession(s, async () => {
                const r = await s.apply(bytes, step);
                const out = { accepted: r.accepted, height: r.height, id: r.id, missed: r.missed };
                if (r.reward !== undefined) out.reward = r.reward;
                if (r.accepted) {
                    out.line = r.line;
                } else {
                    out.stage = r.stage;
                    out.error = { class: r.error.class, message: r.error.message.split(solar.solarDir).join("<reference>") };
                }
                return out;
            });
        },

        "chain.revert": async ({ session: id, count }) => {
            safeInteger(count, "count");
            const s = session(id);
            return inSession(s, async () => ({ lines: await s.revert(count) }));
        },

        "chain.restart": async ({ session: id, mode }) => {
            if (mode !== "rebuild" && mode !== "saved") throw new TypeError("mode must be rebuild or saved");
            const s = session(id);
            return inSession(s, async () => {
                const r = await s.restart(mode);
                if (r.refused) return { ...(r.notes ? { notes: r.notes } : {}), refused: { reason: r.refused.reason.split(solar.solarDir).join("<reference>") } };
                return r.notes ? { notes: r.notes, line: r.line } : { line: r.line };
            });
        },

        "chain.replaceStored": async ({ session: id, height, index, hex }) => {
            safeInteger(height, "height");
            safeInteger(index, "index");
            const s = session(id);
            const entry = s.chain.store.blocks[height - 1];
            if (!entry || !entry.transactions[index]) throw new RangeError(`no stored transaction ${index} at height ${height}`);
            entry.transactions[index].serialised = hexBuffer(hex, "hex");
            return { replaced: true };
        },

        "chain.dump": async ({ session: id }) => {
            const s = session(id);
            return inSession(s, async () => ({ sections: s.sections(), line: s.lines[s.lines.length - 1] }));
        },

        "chain.close": async ({ session: id }) => {
            session(id);
            sessions.delete(id);
            return { closed: true };
        },
    };
};
