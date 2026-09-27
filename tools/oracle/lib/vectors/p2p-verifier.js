"use strict";
// V26, the verifier:
//   nsect.find           the kernel's n-ary search over ranges of 1 to 10,000, every common
//                        height of the small ranges, and probes that stop the search;
//   verifier.checkState  the peer verifier on a chain of 70 headers signed by the delegates of
//                        their rounds (rounds of 53, from a rotating list of 56 registered
//                        delegates): the same chain, a peer behind or ahead, forks below our height,
//                        at it and across a round end, claimed headers and fork segments that fail
//                        each check, bogus common-block replies, short and empty replies, the
//                        deadline before each kind of request, and the fast verification.
// Records named "unlinked: ..." are the assertion records of the fork-segment linkage that
// Heartwood adds: the reference accepts or refuses them without it.
//
// Headers are signed with the oracle's fixed aux, so the records are the same on every run.

const crypto = require("crypto");

const ROUND = 53;
const REGISTERED = 56;
const TOP = 70;

function sha256(data) {
    return crypto.createHash("sha256").update(data).digest();
}

/** The chain maker: registered delegates, the rounds' lists, and signed headers. */
function makeChains(solar) {
    const { Blocks, Crypto, Identities, Utils } = solar.lib;
    const keys = (name) => Identities.Keys.fromPassphrase(`verifier ${name}`);
    const delegates = Array.from({ length: REGISTERED }, (_, i) => keys(`delegate ${i}`));
    const publicKey = (k) => k.publicKey.secp256k1;
    const stranger = keys("stranger");
    const standby = delegates[REGISTERED - 1];
    const roundOf = (height) => Math.floor((height - 1) / ROUND) + 1;
    // Round r's delegates: 53 consecutive registered delegates from index r - 1.
    const roundList = (round) => delegates.slice(round - 1, round - 1 + ROUND);

    /** A header signed by `signer`; returns its hex and id. */
    function sign({ height, previous, timestamp, signer, version = 0 }) {
        const data = {
            version,
            timestamp,
            height,
            previousBlock: previous,
            numberOfTransactions: 0,
            totalAmount: Utils.BigNumber.ZERO,
            totalFee: Utils.BigNumber.ZERO,
            reward: Utils.BigNumber.make(180000000),
            payloadLength: 0,
            payloadHash: sha256(Buffer.alloc(0)).toString("hex"),
            generatorPublicKey: publicKey(signer),
        };
        data.blockSignature = Crypto.Hash.signSchnorrBip340(sha256(Blocks.Serialiser.serialise(data, false)), signer);
        return { hex: Blocks.Serialiser.serialise(data, true).toString("hex"), id: Blocks.Block.getId(data), height };
    }

    /** Headers from `from` to `to` on top of `previous`, signed by delegates of their rounds. */
    function extend(previous, from, to, { offset = 0, shift = 0 } = {}) {
        const out = [];
        let last = previous;
        for (let height = from; height <= to; height++) {
            const list = roundList(roundOf(height));
            const header = sign({
                height,
                previous: last,
                timestamp: (height - 1) * 8 + offset,
                signer: list[(height + shift) % list.length],
            });
            out.push(header);
            last = header.id;
        }
        return out;
    }

    const base = extend("00".repeat(32), 1, TOP);
    const id = (height) => base[height - 1].id;

    /** The peer's branch after our block `fork`, up to `top`. */
    const branch = (fork, top) => extend(id(fork), fork + 1, top, { offset: 3, shift: 7 });

    /** A branch header re-signed with changes: another signer, parent or height. */
    function resign(header, changes) {
        const data = Blocks.Deserialiser.deserialise(Buffer.from(header.hex, "hex"), true).data;
        const signer = changes.signer || delegates.find((k) => publicKey(k) === data.generatorPublicKey);
        return sign({
            height: changes.height ?? data.height,
            previous: changes.previous ?? data.previousBlock,
            timestamp: data.timestamp,
            signer,
            version: changes.version ?? data.version,
        });
    }

    /** The input of a verification against our chain at `ours`. */
    function input(ours, peer, claimed, extra = {}) {
        const rounds = [];
        for (let r = 1; r <= roundOf(ours + 1); r++) rounds.push([r, roundList(r).map(publicKey)]);
        return {
            chain: base.slice(0, ours).map((h) => h.hex),
            delegates: delegates.map((k, i) => [publicKey(k), `delegate_${i}`]),
            rounds,
            // The peer's blocks above our height on our chain are its own, as a branch is.
            peer: {
                fork: Math.min(peer.fork, ours),
                branch: [...base.slice(Math.min(peer.fork, ours), peer.fork), ...peer.branch].map((h) => (h.set ? { hex: h.hex, set: h.set } : h.hex)),
                ...peer.extra,
            },
            claimed,
            fast: false,
            ...extra,
        };
    }

    /** The claimed state of a peer whose last block is `header`. */
    const claim = (header, height = header.height) => ({ height, header: header.hex });

    return { base, branch, resign, input, claim, id, delegates, roundList, stranger, standby, publicKey };
}

async function nsect({ rec, must, shared }) {
    const out = [];
    const push = async (input, name) => out.push({ ...must(await rec("nsect.find", shared.height, input)), name });
    for (const common of [1, 2, 9, 1250, 1251, 5000, 7777, 9998, 9999, 10000]) {
        await push({ n: 8, low: 1, high: 10000, common }, `8-ary over [1, 10000], common ${common}`);
    }
    await push({ n: 8, low: 1, high: 10000, common: 0 }, "8-ary over [1, 10000], nothing in common");
    for (let high = 1; high <= 20; high++) {
        for (let common = 0; common <= high; common++) {
            await push({ n: 8, low: 1, high, common }, `8-ary over [1, ${high}], common ${common}`);
        }
    }
    for (const [low, high] of [
        [3, 3],
        [5, 13],
        [100, 181],
        [4294967000, 4294967295],
    ]) {
        await push({ n: 8, low, high, common: Math.floor((low + high) / 2) }, `8-ary over [${low}, ${high}]`);
    }
    for (const n of [1, 2, 3]) {
        for (const common of [1, 17, 50, 99]) await push({ n, low: 1, high: 99, common }, `${n}-ary over [1, 99], common ${common}`);
    }
    for (const stop of [0, 1, 2]) {
        await push({ n: 8, low: 1, high: 1000, common: 777, stop }, `a probe that stops the search at probe ${stop}`);
    }
    return out;
}

async function verifier({ solar, rec, must, shared }) {
    const c = makeChains(solar);
    const out = [];
    const push = async (input, name) => out.push({ ...must(await rec("verifier.checkState", shared.height, input)), name });
    const on = (fork) => ({ fork, branch: [] });
    const forked = (fork, top, extra) => ({ fork, branch: c.branch(fork, top), extra });
    const headerAt = (peer, height) => (height <= peer.fork ? c.base[height - 1] : peer.branch[height - peer.fork - 1]);
    const tip = (peer) => (peer.branch.length ? peer.branch[peer.branch.length - 1] : c.base[peer.fork - 1]);
    const verify = async (ours, peer, name, extra = {}, claimed = c.claim(tip(peer))) => push(c.input(ours, peer, claimed, extra), name);

    // The chains. Round 1 is heights 1 to 53.
    await verify(50, on(50), "the same chain");
    await verify(50, on(40), "a peer behind on our chain");
    await verify(30, on(50), "a peer ahead on our chain, checked to its claimed height");
    await verify(40, on(70), "a peer ahead on our chain, checked to our round's end");
    await verify(50, forked(40, 48), "a fork below our height");
    await verify(70, forked(45, 60), "a fork across a round end, checked to round 1's end");
    await verify(70, forked(53, 60), "a fork at a round's last block, checked by round 2's delegates");
    await verify(50, forked(45, 50), "a fork at our height");
    await verify(50, forked(49, 54), "a fork above our height");
    await verify(30, forked(1, 12), "a fork after the first block");

    // The claimed header.
    const behind = on(40);
    const other = c.delegates[ROUND]; // in round 2 only
    await verify(50, behind, "a claimed height that is not the header's", {}, c.claim(c.base[39], 41));
    await verify(50, on(50), "a claimed header whose signature fails", {}, { height: 50, header: { hex: c.base[49].hex, set: { timestamp: 8000 } } });
    await verify(50, behind, "a claimed header below our height by a delegate of another round", {}, c.claim(c.resign(c.base[39], { signer: other })));
    await verify(50, behind, "a claimed header below our height by a registered delegate in no round", {}, c.claim(c.resign(c.base[39], { signer: c.standby })));
    await verify(50, behind, "a claimed header below our height by a key that is no delegate", {}, c.claim(c.resign(c.base[39], { signer: c.stranger })));
    await verify(30, on(50), "a claimed header above our height by a key that is no delegate", {}, c.claim(c.resign(c.base[49], { signer: c.stranger })));
    await verify(50, on(50), "a claimed header with an uppercase id", {}, { height: 50, header: { hex: c.base[49].hex, set: { id: c.id(50).toUpperCase() } } });
    await verify(50, on(50), "a claimed header without a payload hash", {}, { height: 50, header: { hex: c.base[49].hex, set: { payloadHash: null } } });

    // Common-block replies.
    const fork = forked(40, 48);
    const scripted = (common) => ({ ...fork, extra: { common } });
    await verify(50, scripted([{ height: 25, id: "ab".repeat(32) }]), "a common block that was not asked for");
    await verify(50, scripted([{ height: 26, id: c.id(13) }]), "a common block at another height than ours");
    await verify(50, scripted([{ height: 13, id: c.id(13).toUpperCase() }]), "a common block id in uppercase");
    await verify(50, scripted([null]), "no common block");
    await verify(50, scripted(["honest", { height: 40, id: "cd".repeat(32) }]), "a bogus reply after a match keeps the match");
    await verify(50, scripted(["honest", null]), "no common block after a match keeps the match");
    await verify(50, scripted([{ height: 7, id: c.id(7) }]), "a lower common block than the peer has");

    // Fork segment headers: the fifth, at height 45.
    const segment = (changes, name) => {
        const peer = forked(40, 48);
        peer.branch = peer.branch.slice();
        peer.branch[4] = changes(peer.branch[4]);
        return verify(50, peer, name);
    };
    const emptyHash = sha256(Buffer.alloc(0)).toString("hex");
    await segment((h) => ({ hex: h.hex, set: { timestamp: 1 } }), "a segment header whose signature fails");
    await segment((h) => c.resign(h, { height: 46 }), "a segment header at another height");
    await segment((h) => c.resign(h, { signer: other }), "a segment header by a delegate of another round");
    await segment((h) => c.resign(h, { signer: c.standby }), "a segment header by a registered delegate in no round");
    await segment((h) => c.resign(h, { signer: c.stranger }), "a segment header by a key that is no delegate");
    await segment((h) => c.resign(h, { version: 1 }), "a segment header of version 1");
    await segment((h) => ({ hex: h.hex, set: { payloadHash: emptyHash.slice(0, 62) } }), "a segment header whose payload hash is 31 bytes");
    await segment((h) => ({ hex: h.hex, set: { payloadHash: `${emptyHash}77` } }), "a segment header whose payload hash is 33 bytes");
    await segment((h) => ({ hex: h.hex, set: { payloadHash: `${emptyHash}7` } }), "a segment header whose payload hash has an odd last digit");
    await segment((h) => ({ hex: h.hex, set: { payloadHash: null } }), "a segment header without a payload hash");
    await segment((h) => ({ hex: h.hex, set: { version: null, numberOfTransactions: null, payloadLength: null } }), "a segment header without its zero fields");
    await segment((h) => ({ hex: h.hex, set: { id: "ef".repeat(32), previousBlock: fork.branch[3].id.toUpperCase() } }), "a segment header with another id and an uppercase parent");

    // Linkage (the assertion records).
    await segment((h) => c.resign(h, { previous: "07".repeat(32) }), "unlinked: a segment header with another parent");
    const first = forked(40, 48);
    first.branch = [c.resign(first.branch[0], { previous: c.id(39) }), ...first.branch.slice(1)];
    await verify(50, first, "unlinked: a first segment header that does not chain to the common block");
    const later = forked(40, 48);
    later.branch = later.branch.slice();
    later.branch[2] = c.resign(later.branch[2], { previous: "07".repeat(32) });
    later.branch[6] = { hex: later.branch[6].hex, set: { timestamp: 1 } };
    await verify(50, later, "unlinked: a segment that does not chain before a header that fails");
    const earlier = forked(40, 48);
    earlier.branch = earlier.branch.slice();
    earlier.branch[1] = { hex: earlier.branch[1].hex, set: { timestamp: 1 } };
    earlier.branch[5] = c.resign(earlier.branch[5], { previous: "07".repeat(32) });
    await verify(50, earlier, "a header that fails before a segment that does not chain");

    // Replies of headers.
    await verify(50, forked(40, 48, { perReply: 3 }), "short replies of headers");
    await verify(50, forked(40, 48, { perReply: 0 }), "no headers");
    await verify(30, forked(10, 40, { perReply: 1 }), "one header per reply");

    // The deadline: the fork below our height takes two probes, then one fetch.
    for (const [deadline, name] of [
        [0, "before the first probe"],
        [1, "before the second probe"],
        [2, "before the fetch"],
        [3, "after the fetch"],
    ]) {
        await verify(50, forked(40, 48), `the deadline ${name}`, { deadline });
    }
    await verify(50, forked(40, 48, { perReply: 3 }), "the deadline before the third fetch", { deadline: 4 });

    // Rounds the node cannot read: with the claimed header above our height, the first read of
    // round 1 is the fork segment's.
    const unreadable = (ours) => {
        const i = c.input(ours, forked(40, 48), c.claim(tip(forked(40, 48))));
        i.rounds = i.rounds.filter(([round]) => round !== 1);
        return i;
    };
    await push(unreadable(45), "a segment in a round the node cannot read");
    await push(unreadable(50), "a claimed header in a round the node cannot read");

    // The fast verification.
    await verify(50, on(50), "fast: the same chain", { fast: true });
    await verify(50, on(40), "fast: a peer behind", { fast: true });
    await verify(50, forked(40, 48), "fast: a fork", { fast: true });
    await verify(30, on(50), "fast: a peer ahead", { fast: true });
    await verify(50, behind, "fast: a claimed height that is not the header's", { fast: true }, c.claim(c.base[39], 41));
    return out;
}

module.exports = async function verifierVectors(ctx) {
    return [...(await nsect(ctx)), ...(await verifier(ctx))];
};
