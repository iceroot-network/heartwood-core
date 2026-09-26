"use strict";
// Oracle operations of the reference's route codecs (packages/p2p/dist/socket-server/codecs) and
// of its protobuf messages (codecs/proto/protos.js, the static code of protobufjs 7.2.4):
//   wire.encode          {route, direction, value} -> {hex}; route is the codec's name
//                        (getStatus), direction is request or response, value has present keys
//                        only, with bytes and transactions as hex and amounts as decimal text
//   wire.decode          {route, direction, hex}   -> {value}: own keys in their order, Buffers
//                        as hex, BigNumbers as decimal text; or the reference's error
//   wire.decodeSelected  {hex}                     -> {route, direction, value}: the first byte
//                        modulo 14 selects the codec, in route order with the request first
//                        (getPeers, getCommonBlocks, getStatus, getBlocks, postBlock,
//                        getUnconfirmedTransactions, postTransactions), 0 for an empty input, and
//                        the rest is the payload; for the fuzz replay
//   proto.encode         {message, value}          -> {hex}: one message without a route's
//                        post-processing; message is its full name (peer.GetStatusResponse.Config)
//                        and value has present keys only, with bytes as hex
//   proto.decode         {message, hex}            -> {value}, as wire.decode renders it
// An output may carry `notes` (not compared) with what the value cannot show: wideAmount when a
// decoded amount lies outside the signed 128-bit range. The overflow of the Node stack on deeply
// nested groups is the reference's RangeError itself. Loaded by lib/ops.js.

const path = require("path");

// The codecs in the order of the fuzz selector byte.
const SELECTED = ["getPeers", "getCommonBlocks", "getStatus", "getBlocks", "postBlock", "getUnconfirmedTransactions", "postTransactions"]
    .flatMap((route) => [[route, "request"], [route, "response"]]);

// The messages of the four .proto files, and the bytes fields of each (all at the top level).
const MESSAGES = {
    "shared.Headers": [],
    "peer.GetPeersRequest": [],
    "peer.GetPeersResponse": [],
    "peer.GetPeersResponse.Peer": [],
    "peer.GetCommonBlocksRequest": [],
    "peer.GetCommonBlocksResponse": [],
    "peer.GetCommonBlocksResponse.Common": [],
    "peer.GetStatusRequest": [],
    "peer.GetStatusResponse": [],
    "peer.GetStatusResponse.State": [],
    "peer.GetStatusResponse.State.BlockHeader": [],
    "peer.GetStatusResponse.Config": [],
    "peer.GetStatusResponse.Config.Network": [],
    "peer.GetStatusResponse.Config.Network.Token": [],
    "peer.GetStatusResponse.Config.Plugin": [],
    "blocks.GetBlocksRequest": [],
    "blocks.GetBlocksResponse": ["blocks"],
    "blocks.GetBlocksResponse.BlockHeader": ["transactions"],
    "blocks.PostBlockRequest": ["block"],
    "blocks.PostBlockResponse": [],
    "transactions.GetUnconfirmedTransactionsRequest": ["exclude"],
    "transactions.GetUnconfirmedTransactionsResponse": ["transactions"],
    "transactions.PostTransactionsRequest": ["transactions"],
    "transactions.PostTransactionsResponse": [],
};

// The signed 128-bit range that Heartwood's amounts hold.
const I128_MIN = -(2n ** 127n);
const I128_MAX = 2n ** 127n - 1n;

module.exports = function wireCodecOps({ solar, hexBuffer }) {
    let reference;

    /** The codecs, the generated messages and the BigNumber class they use, loaded on first use. */
    function load() {
        if (!reference) {
            const p2p = path.join(solar.solarDir, "packages", "p2p");
            const dir = path.join(p2p, "dist", "socket-server", "codecs");
            reference = {
                Codecs: require(path.join(dir, "index.js")).Codecs,
                protos: require(path.join(dir, "proto", "protos.js")),
                BigNumber: require(require.resolve("@solar-network/crypto", { paths: [p2p] })).Utils.BigNumber,
            };
        }
        return reference;
    }

    function codec(route, direction) {
        const { Codecs } = load();
        if (!SELECTED.some(([r, d]) => r === route && d === direction)) throw new TypeError(`no protobuf codec ${route} ${direction}`);
        return Codecs[route][direction];
    }

    function message(name) {
        if (!Object.prototype.hasOwnProperty.call(MESSAGES, name)) throw new TypeError(`no message ${name}`);
        return name.split(".").reduce((scope, part) => scope[part], load().protos);
    }

    /**
     * A decoded value as plain JSON: own keys in their order (an own key __proto__ included),
     * Buffers as hex, BigNumbers as decimal text, with the notes of what the text cannot show.
     */
    function render(value, notes) {
        if (value === null || typeof value !== "object") return value;
        if (value instanceof Uint8Array) return Buffer.from(value).toString("hex");
        if (value instanceof load().BigNumber) {
            const text = value.toString();
            if (BigInt(text) < I128_MIN || BigInt(text) > I128_MAX) notes.wideAmount = true;
            return text;
        }
        if (Array.isArray(value)) return value.map((item) => render(item, notes));
        const out = {};
        for (const key of Object.keys(value)) {
            Object.defineProperty(out, key, { value: render(value[key], notes), enumerable: true, writable: true, configurable: true });
        }
        return out;
    }

    /** `fields` as the output, with the notes when there are any. */
    function withNotes(fields, notes) {
        return Object.keys(notes).length ? { ...fields, notes } : fields;
    }

    /** A codec's input from the JSON value: the Buffers its serialiser takes, from hex. */
    function codecInput(route, direction, value) {
        const buffers = (list, name) => list.map((item, i) => hexBuffer(item, `${name}[${i}]`));
        if (route === "postBlock" && direction === "request" && value.block !== undefined) {
            return { ...value, block: hexBuffer(value.block, "value.block") };
        }
        const listed = (route === "getUnconfirmedTransactions" && direction === "response") || (route === "postTransactions" && direction === "request");
        if (listed && Array.isArray(value.transactions)) return { ...value, transactions: buffers(value.transactions, "value.transactions") };
        return value;
    }

    function decodeWith(route, direction, bytes) {
        const notes = {};
        const value = render(codec(route, direction).deserialise(bytes), notes);
        return { value, notes };
    }

    return {
        "wire.encode": ({ route, direction, value }) => ({
            hex: Buffer.from(codec(route, direction).serialise(codecInput(route, direction, value))).toString("hex"),
        }),

        "wire.decode": ({ route, direction, hex }) => {
            const { value, notes } = decodeWith(route, direction, hexBuffer(hex, "hex"));
            return withNotes({ value }, notes);
        },

        "wire.decodeSelected": ({ hex }) => {
            const bytes = hexBuffer(hex, "hex");
            const [route, direction] = SELECTED[(bytes.length ? bytes[0] : 0) % SELECTED.length];
            const { value, notes } = decodeWith(route, direction, bytes.subarray(1));
            return withNotes({ route, direction, value }, notes);
        },

        "proto.encode": ({ message: name, value }) => {
            const input = { ...value };
            for (const field of MESSAGES[name] ?? []) {
                if (input[field] !== undefined) input[field] = hexBuffer(input[field], `value.${field}`);
            }
            return { hex: Buffer.from(message(name).encode(input).finish()).toString("hex") };
        },

        "proto.decode": ({ message: name, hex }) => {
            const notes = {};
            const value = render(message(name).decode(hexBuffer(hex, "hex")), notes);
            return withNotes({ value }, notes);
        },
    };
};
