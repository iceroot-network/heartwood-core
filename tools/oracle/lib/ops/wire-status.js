"use strict";
// Oracle operations of the signed peer status, recomposed from the reference's own functions in
// the order of its getStatus controller (socket-server/controllers/peer.ts, getStatus) and of its
// peer communicator (peer-communicator.ts: emit, then ping):
//   status.sign    {block, forgingAllowed, currentSlot, config, keys} -> {preimage, hex}: the tree
//                  of the block's getHeader(false) and the configuration, Utils.stringify, one
//                  Hash.signSchnorr per key (repeats allowed), and the codec's bytes
//   status.verify  {hex, maxDelegates?} -> {valid, refused?, value, preimage, verified,
//                  preimageNegativeSlot, verifiedNegativeSlot}: the codec's decode, the reply
//                  schema, then both preimages (the verifier's own slot not negative, then
//                  negative) with verifySchnorr for each pair
// An output may carry `notes` (not compared): pluginsPrototype when the decoded plugins object
// has a prototype other than Object.prototype, wideAmount when a decoded amount is beyond 128
// bits. A stack overflow in the decoder is the operation's RangeError.
//
// Inputs. `config` is the node's public configuration: {version, network: {version, name,
// nethash, explorer, token: {name, symbol}}, plugins}, where plugins is a list of [name, {enabled,
// port, estimateTotalCount?}] in the configuration's order, so that a repeated name or __proto__
// reaches the object as the reference's transformPlugins builds it. `keys` are secret keys in hex.
// `maxDelegates` replaces the bound of publicKeys and signatures, which the reference computes
// from the milestones when its schemas load.
//
// A refused reply names the part of the reply whose rule failed first, from the path of the
// schema's first error: root, state, header, config, network, plugins, publicKeys, signatures.

const path = require("path");

// The largest and smallest 128-bit amounts that Heartwood reads.
const I128_MAX = (1n << 127n) - 1n;
const I128_MIN = -(1n << 127n);

// Paths of the reply schema's parts, longest first.
const PARTS = [
    [".state.header", "header"],
    [".state", "state"],
    [".config.network", "network"],
    [".config.plugins", "plugins"],
    [".config", "config"],
    [".publicKeys", "publicKeys"],
    [".signatures", "signatures"],
];

function part(dataPath) {
    for (const [prefix, name] of PARTS) {
        if (dataPath === prefix || dataPath.startsWith(prefix + ".") || dataPath.startsWith(prefix + "[")) return name;
    }
    return dataPath === "" ? "root" : `unknown ${dataPath}`;
}

module.exports = function wireStatusOps({ solar, hexBuffer }) {
    const { Blocks, Crypto, Identities, Utils, Validation } = solar.lib;
    const dist = path.join(solar.solarDir, "packages", "p2p", "dist");
    let reference;
    /**
     * The getStatus codec and reply schema, loaded on first use: the schemas module reads the
     * milestones when it loads, so it must load after the devnet configuration is set.
     */
    function load() {
        if (!reference) {
            reference = {
                codec: require(path.join(dist, "socket-server", "codecs")).Codecs.getStatus,
                schema: require(path.join(dist, "schemas")).replySchemas["p2p.peer.getStatus"],
            };
        }
        return reference;
    }

    function replySchema(maxDelegates) {
        const { schema } = load();
        if (maxDelegates === undefined) return schema;
        if (!Number.isSafeInteger(maxDelegates) || maxDelegates < 0) throw new TypeError("maxDelegates must be a safe integer");
        const copy = JSON.parse(JSON.stringify(schema));
        copy.properties.publicKeys.maxItems = maxDelegates;
        copy.properties.signatures.maxItems = maxDelegates;
        return copy;
    }

    /** The configuration as getPeerConfig builds it, the plugins as transformPlugins sets them. */
    function peerConfig({ version, network, plugins }) {
        const result = {};
        for (const [name, plugin] of plugins) {
            result[name] = { enabled: plugin.enabled, port: plugin.port };
            if (plugin.estimateTotalCount !== undefined) result[name].estimateTotalCount = plugin.estimateTotalCount;
        }
        return {
            version,
            network: {
                version: network.version,
                name: network.name,
                nethash: network.nethash,
                explorer: network.explorer,
                token: { name: network.token.name, symbol: network.token.symbol },
            },
            plugins: result,
        };
    }

    /** A decoded value in JSON: own keys in their order, BigNumbers as decimal text. */
    function render(value) {
        if (value instanceof Utils.BigNumber) return value.toString();
        if (Array.isArray(value)) return value.map(render);
        if (value !== null && typeof value === "object") {
            return Object.fromEntries(Object.keys(value).map((key) => [key, render(value[key])]));
        }
        return value;
    }

    /** verifySchnorr for each pair, as ping checks them: a signature without a key fails. */
    function verifyPairs(reply, preimage) {
        return reply.signatures.map((signature, i) => {
            const publicKey = reply.publicKeys[i];
            return publicKey !== undefined && Crypto.Hash.verifySchnorr(preimage, signature, publicKey);
        });
    }

    function notesOf(reply) {
        const notes = {};
        const plugins = reply.config && reply.config.plugins;
        if (plugins && Object.getPrototypeOf(plugins) !== Object.prototype) notes.pluginsPrototype = true;
        const header = reply.state.header;
        const amounts = [header.totalAmount, header.totalFee, header.reward].map((amount) => amount.toBigInt());
        if (amounts.some((amount) => amount > I128_MAX || amount < I128_MIN)) notes.wideAmount = true;
        return Object.keys(notes).length ? { notes } : {};
    }

    return {
        "status.sign": ({ block, forgingAllowed, currentSlot, config, keys }) => {
            const { codec } = load();
            const lastBlock = Blocks.BlockFactory.fromBytes(hexBuffer(block, "block"));
            const header = {
                state: {
                    height: lastBlock.data.height,
                    forgingAllowed,
                    currentSlot,
                    header: lastBlock.getHeader(false),
                },
                config: peerConfig(config),
            };
            // Utils.stringify of the kernel.
            const stateBuffer = Buffer.from(solar.stringify(header));
            header.publicKeys = [];
            header.signatures = [];
            for (const key of keys) {
                const keyPair = Identities.Keys.fromPrivateKey(key);
                header.publicKeys.push(keyPair.publicKey.secp256k1);
                header.signatures.push(Crypto.Hash.signSchnorr(stateBuffer, keyPair));
            }
            return { preimage: stateBuffer.toString("hex"), hex: codec.response.serialise(header).toString("hex") };
        },

        "status.verify": ({ hex, maxDelegates }) => {
            const { codec } = load();
            const reply = codec.response.deserialise(hexBuffer(hex, "hex"));
            const notes = notesOf(reply);
            const { error, errors } = Validation.validator.validate(replySchema(maxDelegates), reply);
            if (error) return { valid: false, refused: errors.length ? part(errors[0].dataPath) : "exception", ...notes };
            const value = render(reply);
            const preimage = Buffer.from(solar.stringify({ state: reply.state, config: reply.config }));
            const verified = verifyPairs(reply, preimage);
            // The verifier's own slot is negative.
            if (reply.state.currentSlot) reply.state.currentSlot = reply.state.currentSlot >> 0;
            const preimageNegativeSlot = Buffer.from(solar.stringify({ state: reply.state, config: reply.config }));
            return {
                valid: true,
                value,
                preimage: preimage.toString("hex"),
                verified,
                preimageNegativeSlot: preimageNegativeSlot.toString("hex"),
                verifiedNegativeSlot: verifyPairs(reply, preimageNegativeSlot),
                ...notes,
            };
        },
    };
};
