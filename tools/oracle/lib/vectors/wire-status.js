"use strict";
// V13, the signed status: status.sign over blocks with one key, two keys and a key listed twice,
// with slots before and after the epoch and several plugin sets; status.verify over each, and
// replies built to hit each rule of the reply check once. Every input carries its own network
// configuration and delegate bound, so the records need no other class.
//
// Crafted replies are written with the reference's generated protobuf code, and with raw fields
// where that code cannot write them (a map entry without a value, a missing header, nested groups).

const path = require("path");

const { GENERATOR, OTHER, SENDER } = require("./constants");

// The largest number of active delegates of the devnet milestones.
const MAX_DELEGATES = 53;
// Groups nested this deep overflow the reference's stack (it overflows between 5,000 and 20,000).
const DEEP_GROUPS = 50000;

const NETWORK = { version: 90, name: "devnet", nethash: "00".repeat(32), explorer: "", token: { name: "dROOT", symbol: "dRT" } };
const API = ["@solar-network/api", { enabled: true, port: 6003, estimateTotalCount: true }];

// Raw protobuf fields.
function varint(n) {
    const out = [];
    let v = BigInt(n);
    do {
        let b = Number(v & 0x7fn);
        v >>= 7n;
        if (v) b |= 0x80;
        out.push(b);
    } while (v);
    return Buffer.from(out);
}
const delimited = (field, bytes) => Buffer.concat([varint((field << 3) | 2), varint(bytes.length), bytes]);
const text = (field, value) => delimited(field, Buffer.from(value, "utf8"));

module.exports = async function wireStatusVectors({ solar, rec, must, shared }) {
    const { Identities } = solar.lib;
    const H = shared.height;
    const out = [];
    const proto = require(path.join(solar.solarDir, "packages", "p2p", "dist", "socket-server", "codecs", "proto", "protos")).peer;
    const { State, Config } = proto.GetStatusResponse;
    const secret = (passphrase) => Identities.Keys.fromPassphrase(passphrase).privateKey;
    const [generator, sender, other] = [GENERATOR, SENDER, OTHER].map(secret);
    const [block2, block3] = shared.blocks.statusBlocks.map((b) => b.output.hex);
    const config = (plugins = [API], version = "4.3.1") => ({ version, network: NETWORK, plugins });

    const sign = async (name, { block = block3, forgingAllowed = true, currentSlot = 5, plugins, version, keys = [generator] }) => {
        const r = must(await rec("status.sign", H, { block, forgingAllowed, currentSlot, config: config(plugins, version), keys }));
        out.push({ ...r, name: `sign: ${name}` });
        return r.output.hex;
    };
    const verify = async (name, bytes, maxDelegates = MAX_DELEGATES) => {
        const hex = Buffer.isBuffer(bytes) ? bytes.toString("hex") : bytes;
        out.push({ ...(await rec("status.verify", H, { hex, maxDelegates })), name: `verify: ${name}` });
    };
    const signAndVerify = async (name, input) => verify(name, await sign(name, input));

    // ---- the signer, and the verifier over each signed response ----------------------------------
    // The five states of the V04 status preimages, with one key, two keys, a key listed twice,
    // three keys and none.
    const states = [
        [block2, true, 1, [generator]],
        [block2, false, 2, [generator, sender]],
        [block3, true, 2, [generator, generator]],
        [block3, false, 3, [sender, generator, other]],
        [block3, true, 4000000, []],
    ];
    for (const [i, [block, forgingAllowed, currentSlot, keys]] of states.entries()) {
        await signAndVerify(`status ${i + 1}, ${keys.length} keys`, { block, forgingAllowed, currentSlot, keys });
    }
    // The slot before the epoch is negative in the preimage and wraps to 32 bits on the wire; a
    // verifier whose own slot is negative reads it back with >> 0, which also turns a slot of 2^31
    // or more negative.
    for (const currentSlot of [-3, 0, 1, -2147483648, 2147483648, 4294967295]) {
        await signAndVerify(`slot ${currentSlot}`, { currentSlot });
    }
    // Plugin sets. An empty set is signed as {} but left out by every verifier, so its
    // attestations never verify.
    await signAndVerify("no plugins", { plugins: [] });
    const many = (n) => Array.from({ length: n }, (_, i) => [`plugin-${String(i).padStart(2, "0")}`, { enabled: i % 2 === 0, port: 4000 + i }]);
    await signAndVerify("32 plugins", { plugins: many(32) });
    await signAndVerify("33 plugins", { plugins: many(33) });
    await signAndVerify("array-index names come first", {
        plugins: [["b-plugin", { enabled: true, port: 1 }], ["4294967294", { enabled: false, port: 2 }], ["1234", { enabled: true, port: 3 }], API, ["4294967295", { enabled: true, port: 4 }]],
    });
    await signAndVerify("a repeated name keeps its first place and its last value", {
        plugins: [["first-plugin", { enabled: true, port: 1 }], API, ["first-plugin", { enabled: false, port: 9 }]],
    });
    await signAndVerify("names sorted by UTF-16 code units", {
        plugins: [["｡-plugin", { enabled: true, port: 1 }], ["\u{1f600}-plugin", { enabled: true, port: 2 }], ["é-plugin", { enabled: true, port: 3 }], ["Zeta", { enabled: true, port: 4 }]],
    });
    await signAndVerify("a name of Object.prototype is left out of the preimage", {
        plugins: [API, ["constructor", { enabled: true, port: 5 }], ["toString", { enabled: true, port: 6 }]],
    });
    await signAndVerify("plugins without estimateTotalCount, port 0", {
        plugins: [["@solar-network/core-webhooks", { enabled: false, port: 0 }], ["@solar-network/api", { enabled: true, port: 4003, estimateTotalCount: false }]],
    });
    await signAndVerify("version of 24 code points", { version: "\u{1f600}" + "4.3.1-next.".padEnd(23, "x") });
    await signAndVerify("escaped text", { version: "4.3.1 \"q\" \\ \u0001\n" });

    // ---- crafted replies -------------------------------------------------------------------------
    // The plain reply of a signed status at block 3, slot 5, by the generator.
    const signedHex = await sign("the base reply", {});
    const base = proto.GetStatusResponse.toObject(proto.GetStatusResponse.decode(Buffer.from(signedHex, "hex")));
    const clone = () => JSON.parse(JSON.stringify(base));
    const encode = (reply) => Buffer.from(proto.GetStatusResponse.encode(reply).finish());
    const crafted = async (name, change, maxDelegates) => {
        const reply = clone();
        change(reply);
        await verify(name, encode(reply), maxDelegates);
    };
    const header = (reply) => reply.state.header;

    // The codec: a missing state or header, and the amounts as the codec reads them.
    await crafted("no state", (r) => delete r.state);
    await crafted("no header", (r) => delete r.state.header);
    for (const [label, value] of [
        ["hex amount", "0x10"],
        ["amount with white space", " 12\n"],
        ["empty amount", ""],
        ["amount -0", "-0"],
        ["amount +5", "+5"],
        ["fractional amount", "1.5"],
        ["negative amount", "-5"],
        ["amount 2^63 - 1", "9223372036854775807"],
        ["amount 2^63", "9223372036854775808"],
        ["amount beyond 128 bits", "0x" + "f".repeat(40)],
    ]) {
        await crafted(`totalFee: ${label}`, (r) => (header(r).totalFee = value));
    }
    await crafted("no amounts", (r) => ["totalAmount", "totalFee", "reward"].forEach((k) => delete header(r)[k]));

    // The reply object and the state.
    await crafted("no config", (r) => delete r.config);
    await crafted("no signatures", (r) => (r.signatures = []));
    await crafted("height 0", (r) => (r.state.height = 0));
    await crafted("no forgingAllowed", (r) => delete r.state.forgingAllowed);
    await crafted("no currentSlot", (r) => delete r.state.currentSlot);

    // The header.
    await crafted("header: id not hex", (r) => (header(r).id = "x".repeat(64)));
    await crafted("header: uppercase id", (r) => (header(r).id = header(r).id.toUpperCase()));
    await crafted("header: no timestamp", (r) => delete header(r).timestamp);
    await crafted("header: no version and payload fields", (r) => ["version", "payloadLength", "payloadHash", "numberOfTransactions"].forEach((k) => delete header(r)[k]));
    await crafted("header: empty payloadHash", (r) => (header(r).payloadHash = ""));
    await crafted("header: height 0", (r) => (header(r).height = 0));
    await crafted("header: empty previousBlock", (r) => (header(r).previousBlock = ""));
    for (const previous of ["", "0x0", " 0 ", "0e5", ".0", "1e-400", "1e-300", "00", "-0", "x"]) {
        await crafted(`header at height 1: previousBlock ${JSON.stringify(previous)}`, (r) => {
            header(r).height = 1;
            header(r).previousBlock = previous;
        });
    }
    for (const [key, value] of [
        ["totalAmount", "9223372036854775808"],
        ["totalFee", "170141183460469231731687303715884105727"],
        ["totalFee", "0x" + "f".repeat(40)],
        ["reward", "9223372036854775808"],
        ["totalAmount", "-1"],
    ]) {
        await crafted(`header at height 1: ${key} ${value}`, (r) => {
            header(r).height = 1;
            header(r)[key] = value;
        });
    }
    await crafted("header: uppercase generatorPublicKey is lowercased and still verifies", (r) => (header(r).generatorPublicKey = header(r).generatorPublicKey.toUpperCase()));
    await crafted("header: generatorPublicKey of 64 characters", (r) => (header(r).generatorPublicKey = header(r).generatorPublicKey.slice(2)));
    await crafted("header: blockSignature of 127 characters", (r) => (header(r).blockSignature = header(r).blockSignature.slice(1)));

    // The configuration and its network.
    for (const [label, version] of [
        ["4 characters", "4.3."],
        ["5 characters", "4.3.1"],
        ["25 characters", "4.3.1".padEnd(25, "x")],
        ["24 code points in 25 UTF-16 units", "\u{1f600}" + "x".repeat(23)],
        ["25 code points", "\u{1f600}" + "x".repeat(24)],
    ]) {
        await crafted(`version of ${label}`, (r) => (r.config.version = version));
    }
    await crafted("no version", (r) => delete r.config.version);
    await crafted("no network", (r) => delete r.config.network);
    await crafted("network: empty name", (r) => (r.config.network.name = ""));
    await crafted("network: name of 20 characters", (r) => (r.config.network.name = "n".repeat(20)));
    await crafted("network: name of 21 characters", (r) => (r.config.network.name = "n".repeat(21)));
    await crafted("network: no nethash", (r) => delete r.config.network.nethash);
    await crafted("network: uppercase nethash", (r) => (r.config.network.nethash = "AB".repeat(32)));
    await crafted("network: nethash of 63 characters", (r) => (r.config.network.nethash = "a".repeat(63)));
    await crafted("network: no explorer", (r) => delete r.config.network.explorer);
    await crafted("network: explorer of 129 characters", (r) => (r.config.network.explorer = "e".repeat(129)));
    await crafted("network: no version", (r) => delete r.config.network.version);
    await crafted("network: version 256", (r) => (r.config.network.version = 256));
    await crafted("network: no token", (r) => delete r.config.network.token);
    await crafted("network: token symbol of 5 characters", (r) => (r.config.network.token.symbol = "SSSSS"));
    await crafted("network: empty token name", (r) => (r.config.network.token.name = ""));

    // The plugins: the count first, then the names removed, then each remaining entry.
    const plugin = (port = 1) => ({ enabled: true, port });
    for (const [label, name] of [
        ["a 3-character name is removed", "abc"],
        ["a 4-character name is kept", "abcd"],
        ["a 64-character name is kept", "p".repeat(64)],
        ["a 65-character name is removed", "p".repeat(65)],
        ["a name with a newline is removed", "ab\ncd"],
        ["a name with a carriage return is removed", "ab\rcd"],
        ["a name with U+2028 is removed", "ab cd"],
        ["a name of 32 astral characters is kept", "\u{1f600}".repeat(32)],
        ["a name of 65 UTF-16 units is removed", "a" + "\u{1f600}".repeat(32)],
        ["a name of 2 astral characters is kept", "\u{1f600}\u{1f600}"],
    ]) {
        await crafted(`plugins: ${label}`, (r) => (r.config.plugins[name] = plugin()));
    }
    await crafted("plugins: port 65535", (r) => (r.config.plugins.abcd = plugin(65535)));
    await crafted("plugins: port 65536", (r) => (r.config.plugins.abcd = plugin(65536)));
    await crafted("plugins: port 65536 under a removed name", (r) => (r.config.plugins.abc = plugin(65536)));
    await crafted("plugins: an entry without fields", (r) => (r.config.plugins.abcd = {}));
    await crafted("plugins: 32 entries, one removed", (r) => {
        for (let i = 0; i < 30; i++) r.config.plugins[`plugin-${i}`] = plugin(i);
        r.config.plugins.abc = plugin();
    });
    await crafted("plugins: 33 entries, one invalid", (r) => {
        for (let i = 0; i < 31; i++) r.config.plugins[`plugin-${i}`] = plugin(i);
        r.config.plugins.abc = plugin();
    });

    // Raw replies: a map entry without a value or without a key, __proto__, fields in another
    // order, and nested groups.
    const stateBytes = Buffer.from(State.encode(base.state).finish());
    const configBytes = Buffer.from(Config.encode(base.config).finish());
    const lists = Buffer.concat([...base.publicKeys.map((k) => text(3, k)), ...base.signatures.map((s) => text(4, s))]);
    const raw = (config, { state = stateBytes, before = Buffer.alloc(0) } = {}) =>
        Buffer.concat([before, delimited(1, state), delimited(2, Buffer.concat([configBytes, config])), lists]);
    const entry = (...fields) => delimited(3, Buffer.concat(fields));
    const pluginBytes = (p) => Buffer.from(Config.Plugin.encode(p).finish());
    await verify("plugins: an entry without a value", raw(entry(text(1, "abcd"))));
    await verify("plugins: an entry without a value under a removed name", raw(entry(text(1, "abc"))));
    await verify("plugins: an entry without a key", raw(entry(delimited(2, pluginBytes(plugin())))));
    await verify("plugins: a name set twice", raw(Buffer.concat([entry(text(1, "abcd"), delimited(2, pluginBytes(plugin(1)))), entry(text(1, "abcd"), delimited(2, pluginBytes(plugin(2))))])));
    await verify("plugins: __proto__ with a value", raw(entry(text(1, "__proto__"), delimited(2, pluginBytes(plugin())))));
    await verify("plugins: __proto__ without a value", raw(entry(text(1, "__proto__"))));
    await verify("config before state on the wire", Buffer.concat([delimited(2, configBytes), delimited(1, stateBytes), lists]));
    await verify("a second state replaces the first", Buffer.concat([delimited(1, Buffer.from(State.encode({ height: 9 }).finish())), raw(Buffer.alloc(0))]));
    await verify("an unknown field", raw(Buffer.alloc(0), { before: Buffer.concat([varint((9 << 3) | 0), varint(300)]) }));
    const groups = Buffer.concat([Buffer.alloc(DEEP_GROUPS, (15 << 3) | 3), Buffer.alloc(DEEP_GROUPS, (15 << 3) | 4)]);
    await verify(`groups nested ${DEEP_GROUPS} deep before the state`, raw(Buffer.alloc(0), { before: groups }));

    // The keys and signatures.
    const withLists = (keys, signatures) => (r) => {
        r.publicKeys = keys;
        r.signatures = signatures;
    };
    const [key] = base.publicKeys;
    const [signature] = base.signatures;
    await crafted("an uppercase key verifies", withLists([key.toUpperCase()], [signature]));
    await crafted("an uppercase signature verifies", withLists([key], [signature.toUpperCase()]));
    await crafted("a changed signature does not verify", withLists([key], [signature.slice(0, -2) + (signature.endsWith("00") ? "01" : "00")]));
    await crafted("another key does not verify", withLists([Identities.Keys.fromPrivateKey(sender).publicKey.secp256k1], [signature]));
    await crafted("a key that is not a point", withLists(["05" + key.slice(2)], [signature]));
    await crafted("more signatures than keys", withLists([key], [signature, signature]));
    await crafted("more keys than signatures", withLists([key, key], [signature]));
    await crafted("a key of 64 characters", withLists([key.slice(2)], [signature]));
    await crafted("a signature that is not hex", withLists([key], ["zz".repeat(64)]));
    await crafted(`${MAX_DELEGATES} pairs`, withLists(Array(MAX_DELEGATES).fill(key), Array(MAX_DELEGATES).fill(signature)));
    await crafted(`${MAX_DELEGATES + 1} keys`, withLists(Array(MAX_DELEGATES + 1).fill(key), [signature]));
    await crafted(`${MAX_DELEGATES + 1} signatures`, withLists([key], Array(MAX_DELEGATES + 1).fill(signature)));
    await crafted("two pairs over a bound of one", withLists([key, key], [signature, signature]), 1);

    // Mutations that change the signed bytes: a plugin name the check removes, and the lowercased
    // key, still verify; a changed plugin does not.
    await crafted("a removed plugin name still verifies", (r) => (r.config.plugins.xyz = plugin()));
    await crafted("a changed plugin does not verify", (r) => (r.config.plugins["@solar-network/api"].port = 6004));
    return out;
};
