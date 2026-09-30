"use strict";
// Oracle operation of the server's public controllers (socket-server/controllers/{peer,blocks,
// transactions}.ts), the reference's own classes over stubs of their collaborators:
//   p2p.controller  {route, request, ip, remoteAccess, milestones?, store?, peers?, pool?,
//                   delegates?, pinged?, lastDownloaded?, alreadyForged?, status?, digest?}
//                   -> {hex, lines, ...} or {thrown: true, lines, notes}
// `route` is a route id (`p2p.peer.getPeers`, ...), `request` the payload's hex, which the route's
// request codec reads as the reference's codec plugin does, and `ip` the caller's address as the
// socket reports it. The record's height is the height of the node's last block. `milestones`
// (JSON text) replaces the devnet's for the call. The output:
//   hex      the answer written by the route's response codec
//   digest   with `digest` set, {sha256, length, blocks} of the answer in place of `hex`
//            (`blocks`: how many blocks getBlocks answered)
//   lines    the log lines the controller wrote, [severity, text] in order
//   thrown   true when the controller threw, which the server answers 500 and ends the socket;
//            notes.error then holds the error's class and message
//   handled  postBlock: what handleIncomingBlock got, {id, fromForger, ip}, null when it was not
//            called
//   collated getUnconfirmedTransactions: what the collator was asked, {validate, exclude}, null
//            when it was not called
// The stubs:
//   store       {tip?, blocks?, synthetic?, memory?}: the node's stored blocks, which its database
//               and its in-memory last blocks answer from. `tip` is the last block (hex) and
//               `blocks` the others (hex; a block of which only the header is given has none of its
//               transactions); rows of a synthetic store (lib/fixtures/full-blocks.js) come with
//               their height alone. The last `memory` (default 100) heights are also in memory.
//   peers       [{ip, port, latency}] of the peer repository, in order
//   pool        {size, candidates, refuse?, known?}: the pool's size; the transactions (hex) the
//               collator gives; the ids the processor refuses and the ids it already holds. A
//               transaction's id here is the SHA-256 of its bytes; the processor accepts each one
//               that is not refused, not held and not seen before in the request
//   delegates   [{publicKey, username, rank}] the registered delegates (rank null: none)
//   pinged      whether the blockchain's pingBlock answers true
//   lastDownloaded  the last downloaded block (hex); the tip by default
//   alreadyForged   what the round state says of the block's generator
//   status      getStatus: {tip, now, peerConfig, registered, forgerKeys}, as status.serve reads it
// The collaborators are the node's, not the pool's or the database's real ones: what a record
// checks is the controller's own steps (the filters, the order, the cut, the lines, the answers
// it writes). Loaded by lib/ops.js.

const crypto = require("crypto");
const path = require("path");

const { syntheticRow } = require("../fixtures/full-blocks");

const HEADER_BYTES = 205;

const ROUTES = {
    "p2p.peer.getPeers": ["peer", "PeerController", "getPeers"],
    "p2p.peer.getCommonBlocks": ["peer", "PeerController", "getCommonBlocks"],
    "p2p.peer.getStatus": ["peer", "PeerController", "getStatus"],
    "p2p.blocks.getBlocks": ["blocks", "BlocksController", "getBlocks"],
    "p2p.blocks.postBlock": ["blocks", "BlocksController", "postBlock"],
    "p2p.transactions.getUnconfirmedTransactions": ["transactions", "TransactionsController", "getUnconfirmedTransactions"],
    "p2p.transactions.postTransactions": ["transactions", "TransactionsController", "postTransactions"],
};

const CODEC_OF = {
    "p2p.peer.getPeers": ["peer", "getPeers"],
    "p2p.peer.getCommonBlocks": ["peer", "getCommonBlocks"],
    "p2p.peer.getStatus": ["peer", "getStatus"],
    "p2p.blocks.getBlocks": ["blocks", "getBlocks"],
    "p2p.blocks.postBlock": ["blocks", "postBlock"],
    "p2p.transactions.getUnconfirmedTransactions": ["transactions", "getUnconfirmedTransactions"],
    "p2p.transactions.postTransactions": ["transactions", "postTransactions"],
};

const sha256 = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

function list(value, name) {
    if (value === undefined) return [];
    if (!Array.isArray(value)) throw new TypeError(`${name} must be an array`);
    return value;
}

module.exports = function p2pControllerOps(context) {
    const { solar, hexBuffer, safeInteger, withConfig } = context;
    const { Blocks, Utils } = solar.lib;
    let reference;

    function load() {
        if (reference) return reference;
        const packages = path.join(solar.solarDir, "packages");
        const dist = (pkg, ...file) => require(path.join(packages, pkg, "dist", ...file));
        const shared = require(require.resolve("@solar-network/crypto", { paths: [path.join(packages, "p2p")] }));
        if (shared !== solar.lib) throw new Error("@solar-network/p2p loaded a second copy of @solar-network/crypto");
        const kernel = require(require.resolve("@solar-network/kernel", { paths: [path.join(packages, "p2p")] }));
        reference = {
            Identifiers: kernel.Container.Identifiers,
            Peer: dist("p2p", "peer").Peer,
            controllers: {
                peer: dist("p2p", "socket-server", "controllers", "peer"),
                blocks: dist("p2p", "socket-server", "controllers", "blocks"),
                transactions: dist("p2p", "socket-server", "controllers", "transactions"),
            },
            codecs: {
                peer: dist("p2p", "socket-server", "codecs", "peer"),
                blocks: dist("p2p", "socket-server", "codecs", "blocks"),
                transactions: dist("p2p", "socket-server", "codecs", "transactions"),
            },
            DatabaseInterceptor: dist("state", "database-interceptor").DatabaseInterceptor,
        };
        return reference;
    }

    /** The row of the block `hex` as the node's store keeps it: its header and its transactions. */
    function rowOf(hex, name) {
        const bytes = hexBuffer(hex, name);
        const { data } = Blocks.Deserialiser.deserialise(bytes, true);
        const transactions = [];
        if (bytes.length > HEADER_BYTES) {
            const count = data.numberOfTransactions;
            let offset = HEADER_BYTES + 4 * count;
            for (let i = 0; i < count; i++) {
                const length = bytes.readUInt32LE(HEADER_BYTES + 4 * i);
                transactions.push(bytes.subarray(offset, offset + length).toString("hex"));
                offset += length;
            }
        }
        return { ...data, transactions };
    }

    /** The store's rows in height order, and the block data of its last block. */
    function storeOf(store, lastHeight) {
        const given = store || {};
        const rows = list(given.blocks, "store.blocks").map((hex, i) => rowOf(hex, `store.blocks[${i}]`));
        let tip;
        if (given.tip !== undefined) {
            tip = Blocks.BlockFactory.fromBytes(hexBuffer(given.tip, "store.tip"));
            if (tip.data.height !== lastHeight) throw new Error(`store.tip is at height ${tip.data.height}, not ${lastHeight}`);
            rows.push(rowOf(given.tip, "store.tip"));
        }
        if (given.synthetic) {
            const { from, count, transaction, perBlock } = given.synthetic;
            for (let height = from; height < from + count; height++) {
                const row = syntheticRow(given.synthetic, height);
                rows.push({
                    ...row,
                    totalAmount: Utils.BigNumber.make(row.totalAmount),
                    totalFee: Utils.BigNumber.make(row.totalFee),
                    reward: Utils.BigNumber.make(row.reward),
                    // The rows share the one transaction text, as a database's rows would not.
                    transactions: Array.from({ length: perBlock }, () => transaction),
                });
            }
        }
        rows.sort((a, b) => a.height - b.height);
        return { rows, tip };
    }

    async function serve(input, height) {
        const { Identifiers, Peer, controllers, codecs, DatabaseInterceptor } = load();
        const [controllerFile, controllerClass, method] = ROUTES[input.route];
        const [codecFile, codecName] = CODEC_OF[input.route];
        const lines = [];
        const logger = Object.fromEntries(
            ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"].map((level) => [level, (text) => lines.push([level, String(text)])]),
        );
        const ip = input.ip;
        if (typeof ip !== "string") throw new TypeError("ip must be text");
        const remoteAccess = list(input.remoteAccess, "remoteAccess");
        const { rows, tip } = storeOf(input.store, height);
        const memory = input.store && input.store.memory !== undefined ? safeInteger(input.store.memory, "store.memory") : 100;
        const inMemory = rows.filter((row) => row.height > height - memory && row.height <= height);
        const seen = { handled: null, collated: null };

        const databaseService = {
            findBlockByID: async (ids) => rows.filter((row) => ids.includes(row.id)).map(({ transactions, ...header }) => header),
            findBlockByHeights: async () => [],
            getBlocksForDownload: async (offset, limit, headersOnly) => {
                const wanted = rows.filter((row) => row.height >= offset && row.height <= offset + limit - 1);
                return headersOnly ? wanted.map(({ transactions, ...header }) => header) : wanted;
            },
        };
        const databaseInterceptor = Object.create(DatabaseInterceptor.prototype);
        Object.assign(databaseInterceptor, {
            databaseService,
            stateStore: {
                getCommonBlocks: (ids) => inMemory.filter((row) => ids.includes(row.id)).map(({ transactions, ...header }) => header),
            },
        });

        const byKey = new Map(list(input.delegates, "delegates").map((d) => [d.publicKey, d]));
        const byName = new Map([...byKey.values()].map((d) => [d.username, d]));
        const wallet = (d) => ({
            hasAttribute: (name) => (name === "delegate.username" ? true : name === "delegate.rank" ? d.rank !== null && d.rank !== undefined : false),
            getAttribute: (name) => (name === "delegate.username" ? d.username : d.rank),
            getPublicKey: () => d.publicKey,
        });
        const walletRepository = {
            hasByPublicKey: (key) => byKey.has(key),
            findByPublicKey: (key) => wallet(byKey.get(key)),
            hasByUsername: (name) => byName.has(name),
            findByUsername: (name) => wallet(byName.get(name)),
            allByUsername: () => list(input.status && input.status.registered, "status.registered").map((key) => ({ getPublicKey: () => key })),
        };

        let downloaded = tip && tip.data;
        if (input.lastDownloaded !== undefined) downloaded = Blocks.BlockFactory.fromBytes(hexBuffer(input.lastDownloaded, "lastDownloaded")).data;
        const blockchain = {
            getLastHeight: () => height,
            getLastBlock: () => tip,
            pingBlock: () => input.pinged === true,
            getLastDownloadedBlock: () => downloaded,
            // Blockchain.setBlockUsername.
            setBlockUsername: (block) => {
                if (block.version === 0 && !block.username && byKey.has(block.generatorPublicKey) && wallet(byKey.get(block.generatorPublicKey)).hasAttribute("delegate.username")) {
                    block.username = byKey.get(block.generatorPublicKey).username;
                }
            },
            handleIncomingBlock: (block, fromForger, address) => {
                seen.handled = { id: block.id, fromForger, ip: address };
            },
        };

        const candidates = list(input.pool && input.pool.candidates, "pool.candidates");
        const known = new Set(list(input.pool && input.pool.known, "pool.known"));
        const refused = new Set(list(input.pool && input.pool.refuse, "pool.refuse"));
        const collator = {
            getBlockCandidateTransactions: async (validate, exclude) => {
                seen.collated = { validate, exclude: [...exclude] };
                return candidates.map((hex) => ({ serialised: Buffer.from(hex, "hex") }));
            },
        };
        const processor = {
            process: async (buffers) => {
                const held = new Set(known);
                const accept = [];
                for (const buffer of buffers) {
                    const id = sha256(buffer);
                    if (refused.has(id) || held.has(id)) continue;
                    held.add(id);
                    accept.push(id);
                }
                return { accept };
            },
        };

        const app = { get: () => databaseService, configPath: () => "/config" };
        const configuration = { getOptional: (key, fallback) => (key === "remoteAccess" ? remoteAccess : fallback) };
        const controller = Object.create(controllers[controllerFile][controllerClass].prototype);
        Object.assign(controller, {
            app,
            logger,
            configuration,
            blockchain,
            database: databaseService,
            databaseInterceptor,
            walletRepository,
            roundState: { getRewardForBlockInRound: async () => ({ alreadyForged: input.alreadyForged === true }) },
            collator,
            processor,
            pool: { getPoolSize: () => (input.pool ? safeInteger(input.pool.size, "pool.size") : 0) },
            peerRepository: {
                getPeers: () =>
                    list(input.peers, "peers").map((stub) => {
                        const peer = new Peer(stub.ip, stub.port);
                        peer.latency = stub.latency;
                        return peer;
                    }),
            },
            cachedHeader: undefined,
        });

        const payload = codecs[codecFile][codecName].request.deserialise(hexBuffer(input.request ?? "", "request"));
        const request = { payload, info: { remoteAddress: ip } };

        try {
            if (input.route === "p2p.peer.getStatus") {
                const { hex } = await require("./relay")(context)["status.serve"](input.status);
                return { hex, lines };
            }
            const answer = await controller[method](request, {});
            const bytes = codecs[codecFile][codecName].response.serialise(answer);
            const output = { lines };
            if (input.digest) output.digest = { sha256: sha256(bytes), length: bytes.length, blocks: Array.isArray(answer) ? answer.length : null };
            else output.hex = Buffer.from(bytes).toString("hex");
            if (input.route === "p2p.blocks.postBlock") output.handled = seen.handled;
            if (input.route === "p2p.transactions.getUnconfirmedTransactions") output.collated = seen.collated;
            return output;
        } catch (error) {
            return { thrown: true, lines, notes: { error: { class: error.name, message: error.message } } };
        }
    }

    return {
        "p2p.controller": async (input, height) => {
            const route = ROUTES[input.route];
            if (!route) throw new Error(`no public route ${input.route}`);
            return withConfig(input.milestones, () => serve(input, height));
        },
    };
};
