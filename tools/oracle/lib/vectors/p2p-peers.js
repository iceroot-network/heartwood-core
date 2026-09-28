"use strict";
// V26, the attestations: peer.attest cases over signed statuses of the status blocks at heights 2
// and 3. The slot window's edges and a slot of 0; an unknown key before and after a valid one; a
// failed signature; a repeated pair; more signatures than keys; a key in upper case; our slot and
// the reply's slot negative; and the rules by which a key held by another peer moves, or does not,
// to the pinged peer.
//
// Crafted replies are written with the reference's generated protobuf code.

const path = require("path");

const { GENERATOR, OTHER, SENDER } = require("./constants");

// The largest number of active delegates of the devnet milestones.
const MAX_DELEGATES = 53;
const NETWORK = { version: 90, name: "devnet", nethash: "00".repeat(32), explorer: "", token: { name: "dROOT", symbol: "dRT" } };
const CONFIG = {
    version: "4.3.1",
    network: NETWORK,
    plugins: [["@solar-network/api", { enabled: true, port: 6003, estimateTotalCount: true }]],
};
// Documentation addresses: the pinged peer and the other peers.
const PINGED = "192.0.2.1";
const [FIRST, SECOND] = ["192.0.2.2", "198.51.100.7"];

module.exports = async function attestationVectors({ solar, rec, must, shared }) {
    const { Identities } = solar.lib;
    const H = shared.height;
    const proto = require(path.join(solar.solarDir, "packages", "p2p", "dist", "socket-server", "codecs", "proto", "protos")).peer;
    const keys = [GENERATOR, SENDER, OTHER, "delegate four", "delegate five"].map((p) => Identities.Keys.fromPassphrase(p));
    const unknown = Identities.Keys.fromPassphrase("not a registered delegate");
    const [d0, d1, d2] = keys;
    const pub = (k) => k.publicKey.secp256k1;
    const delegates = keys.map(pub);
    const [block2, block3] = shared.blocks.statusBlocks.map((b) => b.output);
    const out = [];

    const signed = async (signers, { block = block3, currentSlot = 5 } = {}) => {
        const input = { block: block.hex, forgingAllowed: true, currentSlot, config: CONFIG, keys: signers.map((k) => k.privateKey) };
        return must(await rec("status.sign", H, input)).output.hex;
    };
    const craft = (hex, change) => {
        const reply = proto.GetStatusResponse.toObject(proto.GetStatusResponse.decode(Buffer.from(hex, "hex")));
        change(reply);
        return Buffer.from(proto.GetStatusResponse.encode(reply).finish()).toString("hex");
    };
    const attest = async (name, status, { slot = 5, forked = false, peers = [], lastBlock = { height: block2.json.height, id: block2.id } } = {}) => {
        const input = { status, maxDelegates: MAX_DELEGATES, peer: { ip: PINGED, forked }, peers, lastBlock, slot, delegates };
        out.push({ ...must(await rec("peer.attest", H, input)), name: `attest: ${name}` });
    };
    const holder = (ip, held, { forked = false, header = { height: block2.json.height, id: block2.id } } = {}) => ({
        ip,
        publicKeys: held.map(pub),
        forked,
        header,
    });

    // ---- the slot window -----------------------------------------------------------------------
    const two = await signed([d0, d1]);
    for (const slot of [3, 4, 5, 6, 7]) await attest(`reply slot 5, our slot ${slot}`, two, { slot });
    const zero = await signed([d0], { currentSlot: 0 });
    for (const slot of [0, 1, -1]) await attest(`reply slot 0, our slot ${slot}`, zero, { slot });
    // Our slot negative: the reply's slot is read as a signed 32-bit value (>> 0) and verified so.
    const negative = await signed([d0, d1], { currentSlot: -3 });
    for (const slot of [-4, -3, -2, 5]) await attest(`reply slot -3, our slot ${slot}`, negative, { slot });
    const wraps = await signed([d0], { currentSlot: 2147483648 });
    await attest("reply slot 2^31, our slot -2^31", wraps, { slot: -2147483648 });
    await attest("reply slot 2^31, our slot 2^31", wraps, { slot: 2147483648 });

    // ---- the pairs -------------------------------------------------------------------------------
    await attest("an unknown key first ends the loop", await signed([unknown, d0]));
    await attest("an unknown key after a valid one ends the loop there", await signed([d0, unknown, d1]));
    await attest("a signature of another key ends the loop", craft(two, (r) => (r.signatures[1] = r.signatures[0])));
    await attest("a signature of another key first", craft(two, (r) => (r.signatures[0] = r.signatures[1])));
    await attest("a repeated pair counts once", await signed([d0, d0, d1]));
    await attest("a key twice with another signature ends the loop", craft(two, (r) => (r.publicKeys[1] = r.publicKeys[0])));
    await attest("more signatures than keys end the loop", craft(two, (r) => r.publicKeys.pop()));
    await attest("more keys than signatures", craft(two, (r) => r.signatures.pop()));
    await attest("a key in upper case is not a delegate's", craft(two, (r) => (r.publicKeys[0] = r.publicKeys[0].toUpperCase())));
    await attest("no attestations", await signed([]));
    await attest("three delegates of five", await signed([d2, d0, d1]));

    // ---- keys held by other peers ----------------------------------------------------------------
    // Our last block is block 2 unless a case says otherwise; the pinged peer's header is block 3.
    const at3 = { height: block3.json.height, id: block3.id };
    const at2 = { height: block2.json.height, id: block2.id };
    await attest("a key of no other peer", two, { peers: [holder(FIRST, [d2])] });
    await attest("a key held by a forked peer moves", two, { peers: [holder(FIRST, [d1], { forked: true })] });
    await attest("a forked pinged peer takes no held key", two, { forked: true, peers: [holder(FIRST, [d1], { forked: true })] });
    await attest("the pinged peer's header is our last block", two, {
        lastBlock: at3,
        peers: [holder(FIRST, [d1], { header: { height: 9, id: "ab".repeat(32) } })],
    });
    await attest("the pinged peer is higher and the other's header is not our last block", two, {
        lastBlock: { height: 1, id: "cd".repeat(32) },
        peers: [holder(FIRST, [d1])],
    });
    await attest("the other's header is our last block", two, { lastBlock: at2, peers: [holder(FIRST, [d1], { header: at2 })] });
    await attest("the other is higher", two, {
        lastBlock: { height: 1, id: "cd".repeat(32) },
        peers: [holder(FIRST, [d1], { header: { height: 4, id: "ef".repeat(32) } })],
    });
    await attest("the other has no state", two, { lastBlock: at2, peers: [holder(FIRST, [d1], { header: null })] });
    await attest("the other was never verified and has no state", two, {
        lastBlock: at3,
        peers: [holder(FIRST, [d1], { forked: null, header: null })],
    });
    await attest("two peers hold the key: the first in order gives it up", two, {
        peers: [holder(FIRST, [d1, d2], { forked: true }), holder(SECOND, [d1], { forked: true })],
    });
    await attest("a peer at the pinged peer's address is not another peer", two, {
        peers: [holder(PINGED, [d0, d1], { forked: true }), holder(SECOND, [d1])],
    });
    await attest("each key of the reply moves on its own", await signed([d0, d1, d2]), {
        peers: [holder(FIRST, [d0], { forked: true }), holder(SECOND, [d1, d2], { header: at3 })],
    });
    return out;
};
