"use strict";
// Oracle operations of the peer protocol's rules, over the reference's own modules:
//   wire.checkReply       {route, hex, maxTransactions?} or {hex, maxTransactions?} (the first byte
//                         selects the route, byte % 6 over getPeers, getCommonBlocks, getBlocks,
//                         postBlock, getUnconfirmedTransactions, postTransactions) -> the reply
//                         codec's decode, then the reply schema (peer-communicator.ts,
//                         validateReply): {valid: true, value} with the value after the schema's
//                         changes, or {valid: false, refused: part}; a codec refusal is its error
//   p2p.validateRequest   {route, payload, minimumVersions?, milestones?} -> the request the
//                         reference's client
//                         writes for `payload` (codec.request.serialise with the payload as it
//                         is, its headers included), then what a reference server with the
//                         `minimumVersions` option (under `milestones`, a milestone file) does with it
//                         before its controller: the route's payload limit (413), the codec
//                         (plugins/codec.ts), the version check and the request schema
//                         (plugins/validate.ts): {accepted: true, hex}, or {accepted: false,
//                         status} with the bytes and the refusal's text in the notes
//   peer.isValid          {ip, allowPrivate, interfaces} -> {valid, processed}: isValidPeer
//                         (utils/is-valid-peer.ts), loaded with CORE_P2P_ALLOW_PRIVATE_IP_RANGES
//                         set when allowPrivate, the host's addresses being `interfaces`;
//                         processed is the peer's ip after the call
//   peer.isValidVersion   {version, minimumVersions?, configured, networkName} -> {valid}:
//                         isValidVersion (utils/is-valid-version.ts) with the milestone's
//                         p2p.minimumVersions (absent when not given), the p2p option
//                         `configured` and the network's name
//   peer.sameSubnet       {ip, peers} -> {same}: the addresses of `peers` that
//                         getSameSubnetPeers (peer-repository.ts) returns for `ip`
//   ratelimit.run         {limiter: {rateLimit, rateLimitPostTransactions}, events: [{t, ip,
//                         route, op}]} -> {answers}: the outgoing limiter (buildRateLimiter with
//                         isOutgoing), each event at `t` milliseconds on a clock the operation
//                         drives, with the storage's expiry timers fired when they are due; op
//                         is consume ("ok" or "exceeded"), wouldThrottle (a boolean) or throttle,
//                         the communicator's step ("wait", or "sent" after consuming)
// Loaded by lib/ops.js.
//
// Payload JSON. Bytes (a block, transactions) are hex; everything else is as the client writes
// it. A refused reply names the part whose rule failed first, from the path of the schema's
// first error: root, peer, common, header, postBlock or unconfirmed.

const path = require("path");

// The reply routes in the order of the selector byte (heartwood_p2p_wire::replies::ROUTES).
const ROUTES = ["getPeers", "getCommonBlocks", "getBlocks", "postBlock", "getUnconfirmedTransactions", "postTransactions"];
// The route ids of the codec names, and the part a refusal inside a reply names.
const IDS = {
    getStatus: ["p2p.peer.getStatus"],
    getPeers: ["p2p.peer.getPeers", "peer"],
    getCommonBlocks: ["p2p.peer.getCommonBlocks", "common"],
    getBlocks: ["p2p.blocks.getBlocks", "header"],
    postBlock: ["p2p.blocks.postBlock", "postBlock"],
    getUnconfirmedTransactions: ["p2p.transactions.getUnconfirmedTransactions", "unconfirmed"],
    postTransactions: ["p2p.transactions.postTransactions", "root"],
};
// The largest 128-bit amount Heartwood reads.
const I128 = 1n << 127n;
// A timestamp in milliseconds, for the limiter's clock.
const CLOCK_BASE = 1_700_000_000_000;

module.exports = function p2pRulesOps({ solar, cm, atHeight, withConfig, hexBuffer, safeInteger }) {
    const { Utils, Validation } = solar.lib;
    const dist = path.join(solar.solarDir, "packages", "p2p", "dist");
    const load = (...parts) => require(path.join(dist, ...parts));
    let reference;
    /** The codecs, schemas and route tables, loaded on first use (the reply schemas read the milestones). */
    function ref() {
        if (!reference) {
            const routes = [["routes", "peer"], ["routes", "blocks"], ["routes", "transactions"]].map((p) => load("socket-server", ...p));
            reference = {
                Codecs: load("socket-server", "codecs").Codecs,
                replySchemas: load("schemas").replySchemas,
                peerSchemas: load("socket-server", "schemas", "peer").peerSchemas,
                blocksSchemas: load("socket-server", "schemas", "blocks").blocksSchemas,
                transactionsSchemas: load("socket-server", "schemas", "transactions").transactionsSchemas,
                maxBytes: Object.fromEntries(
                    routes.flatMap((module) => {
                        const Route = Object.values(module)[0];
                        const route = Object.create(Route.prototype);
                        route.app = app(undefined);
                        route.getController = () => ({});
                        return Object.values(route.getRoutesConfigByPath()).map((config) => [config.id, config.maxBytes]);
                    }),
                ),
                isValidVersion: load("utils", "is-valid-version").isValidVersion,
                PeerRepository: load("peer-repository").PeerRepository,
                buildRateLimiter: load("utils", "build-rate-limiter").buildRateLimiter,
            };
        }
        return reference;
    }

    /** A stub application whose plugin configuration answers the options the rules read. */
    function app(minimumVersions) {
        const options = {
            "@solar-network/p2p": { minimumVersions },
            "@solar-network/pool": { maxTransactionsPerRequest: 40 },
        };
        return {
            getTagged: (_id, _tag, plugin) => ({
                getOptional: (key, fallback) => {
                    const value = (options[plugin] || {})[key];
                    return value === undefined ? fallback : value;
                },
            }),
        };
    }

    /** Run fn with the milestone's p2p section and the network's name replaced. */
    function withMilestone(p2p, networkName, fn) {
        const { getMilestone, get } = cm;
        cm.getMilestone = (...args) => (p2p === undefined ? { ...getMilestone.apply(cm, args), p2p: undefined } : { ...getMilestone.apply(cm, args), p2p });
        cm.get = (key) => (key === "network.name" && networkName !== undefined ? networkName : get.call(cm, key));
        try {
            return fn();
        } finally {
            cm.getMilestone = getMilestone;
            cm.get = get;
        }
    }

    /** A decoded value in JSON: own keys in their order, Buffers as hex, BigNumbers as decimal text. */
    function render(value, notes) {
        if (value === null || typeof value !== "object") return value;
        if (value instanceof Uint8Array) return Buffer.from(value).toString("hex");
        if (value instanceof Utils.BigNumber) {
            const text = value.toString();
            if (BigInt(text) >= I128 || BigInt(text) < -I128) notes.wideAmount = true;
            return text;
        }
        if (Array.isArray(value)) return value.map((item) => render(item, notes));
        return Object.fromEntries(Object.keys(value).map((key) => [key, render(value[key], notes)]));
    }

    /** The reply after the schema, as the client reads it. */
    function replyValue(route, reply, notes) {
        switch (route) {
            case "getPeers":
                return reply.map((peer) => ({ ip: peer.ip, port: peer.port }));
            case "getCommonBlocks":
                return { common: reply.common === null ? null : { height: reply.common.height, id: reply.common.id } };
            case "postBlock":
                return { status: reply.status, height: reply.height };
            default:
                return render(reply, notes);
        }
    }

    /** The payload's Buffers from hex. */
    function payloadInput(route, payload) {
        const input = { ...payload };
        if (route === "postBlock" && typeof input.block === "string") input.block = hexBuffer(input.block, "payload.block");
        if (route === "postTransactions" && Array.isArray(input.transactions)) {
            input.transactions = input.transactions.map((tx, i) => hexBuffer(tx, `payload.transactions[${i}]`));
        }
        return input;
    }

    function requestSchema(route, application) {
        const { peerSchemas, blocksSchemas, transactionsSchemas } = ref();
        if (route === "postTransactions") return transactionsSchemas.createPostTransactionsSchema(application);
        const schema = peerSchemas[route] || blocksSchemas[route] || transactionsSchemas[route];
        if (!schema) throw new TypeError(`no request schema for ${route}`);
        return schema;
    }

    /** isValidPeer loaded with CORE_P2P_ALLOW_PRIVATE_IP_RANGES set or not: its ranges are read when it loads. */
    function isValidPeerModule(allowPrivate) {
        const file = require.resolve(path.join(dist, "utils", "is-valid-peer"));
        const saved = process.env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES;
        delete require.cache[file];
        if (allowPrivate) process.env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES = "true";
        else delete process.env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES;
        try {
            return require(file).isValidPeer;
        } finally {
            if (saved === undefined) delete process.env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES;
            else process.env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES = saved;
            delete require.cache[file];
        }
    }

    /**
     * Run fn on a clock the caller drives: Date, setTimeout and clearTimeout replaced, with
     * advance(t) firing the timers due by t (in due order) and then setting the time to t.
     */
    async function withClock(fn) {
        const saved = { Date: global.Date, setTimeout: global.setTimeout, clearTimeout: global.clearTimeout };
        let now = CLOCK_BASE;
        let sequence = 0;
        const timers = [];
        class ClockDate extends saved.Date {
            constructor(...args) {
                if (args.length === 0) super(now);
                else super(...args);
            }
            static now() {
                return now;
            }
        }
        global.Date = ClockDate;
        global.setTimeout = (callback, ms) => {
            const timer = { at: now + Number(ms), sequence: sequence++, callback, unref: () => timer, ref: () => timer };
            timers.push(timer);
            return timer;
        };
        global.clearTimeout = (timer) => {
            const i = timers.indexOf(timer);
            if (i >= 0) timers.splice(i, 1);
        };
        const advance = (t) => {
            if (t < now) throw new TypeError("events must be in time order");
            for (;;) {
                const due = timers.filter((timer) => timer.at <= t).sort((a, b) => a.at - b.at || a.sequence - b.sequence)[0];
                if (!due) break;
                timers.splice(timers.indexOf(due), 1);
                now = due.at;
                due.callback();
            }
            now = t;
        };
        try {
            return await fn(advance);
        } finally {
            Object.assign(global, saved);
        }
    }

    /** What a reference server does with the request the client writes for `payload`. */
    function validateRequest(route, payload, minimumVersions) {
        const { Codecs, maxBytes, isValidVersion } = ref();
        const [id] = IDS[route] || [];
        if (!id) throw new TypeError(`no request route ${route}`);
        const bytes = Buffer.from(Codecs[route].request.serialise(payloadInput(route, payload)));
        const hex = bytes.toString("hex");
        const refuse = (status, message) => ({ accepted: false, status, notes: { hex, message } });
        if (bytes.length > maxBytes[id]) return refuse(413, "Payload content length greater than maximum allowed");
        let decoded;
        try {
            decoded = Codecs[route].request.deserialise(bytes);
        } catch (e) {
            return refuse(400, `Payload deserialising failed: ${e}`);
        }
        const application = app(minimumVersions);
        const version = decoded && decoded.headers ? decoded.headers.version : undefined;
        if (version && !isValidVersion(application, { version })) return refuse(400, "Validation failed (invalid version)");
        const result = requestSchema(route, application).validate(decoded);
        if (result && result.error) return refuse(400, "Validation failed");
        return { accepted: true, hex };
    }

    return {
        "wire.checkReply": ({ route, hex, maxTransactions }) => {
            const { Codecs, replySchemas } = ref();
            let bytes = hexBuffer(hex, "hex");
            if (route === undefined) {
                route = ROUTES[(bytes.length ? bytes[0] : 0) % ROUTES.length];
                bytes = bytes.subarray(1);
            }
            if (!ROUTES.includes(route)) throw new TypeError(`no reply check for ${route}`);
            const [id, part] = IDS[route];
            let schema = replySchemas[id];
            if (maxTransactions !== undefined) {
                schema = JSON.parse(JSON.stringify(schema));
                schema.properties.transactions.maxItems = safeInteger(maxTransactions, "maxTransactions");
            }
            const reply = Codecs[route].response.deserialise(bytes);
            const notes = {};
            // Rendered before the check as well, for the amounts beyond 128 bits.
            render(reply, notes);
            const { error, errors } = Validation.validator.validate(schema, reply);
            const noted = (fields) => (Object.keys(notes).length ? { ...fields, notes } : fields);
            if (error) {
                const at = errors && errors.length ? errors[0].dataPath : undefined;
                return noted({ valid: false, refused: at === undefined ? "exception" : at === "" ? "root" : part });
            }
            return noted({ valid: true, value: replyValue(route, reply, notes) });
        },

        "p2p.validateRequest": ({ route, payload, minimumVersions, milestones }, height) =>
            withConfig(milestones, () => atHeight(height, () => validateRequest(route, payload, minimumVersions))),

        "peer.isValid": ({ ip, allowPrivate, interfaces }) => {
            const isValidPeer = isValidPeerModule(allowPrivate);
            const os = require("os");
            const { networkInterfaces } = os;
            os.networkInterfaces = () => ({ host: interfaces.map((address) => ({ address })) });
            try {
                const peer = { ip };
                const valid = isValidPeer(peer);
                return { valid, processed: peer.ip };
            } finally {
                os.networkInterfaces = networkInterfaces;
            }
        },

        "peer.isValidVersion": ({ version, minimumVersions, configured, networkName }) => {
            const { isValidVersion } = ref();
            const p2p = minimumVersions === undefined ? undefined : { minimumVersions };
            return { valid: withMilestone(p2p, networkName, () => isValidVersion(app(configured), { version })) };
        },

        "peer.sameSubnet": ({ ip, peers }) => {
            const repository = Object.create(ref().PeerRepository.prototype);
            repository.getPeers = () => peers.map((address) => ({ ip: address }));
            return { same: repository.getSameSubnetPeers(ip).map((peer) => peer.ip) };
        },

        "ratelimit.run": async ({ limiter, events }) =>
            withClock(async (advance) => {
                const outgoing = ref().buildRateLimiter({
                    isOutgoing: true,
                    whitelist: [],
                    remoteAccess: [],
                    rateLimit: limiter.rateLimit,
                    rateLimitPostTransactions: limiter.rateLimitPostTransactions,
                });
                const answers = [];
                for (const { t, ip, route, op } of events) {
                    advance(CLOCK_BASE + safeInteger(t, "t"));
                    const [endpoint] = IDS[route] || [];
                    if (!endpoint) throw new TypeError(`no route ${route}`);
                    if (op === "consume") {
                        answers.push(
                            await outgoing.consume(ip, endpoint).then(
                                () => "ok",
                                () => "exceeded",
                            ),
                        );
                    } else if (op === "wouldThrottle") {
                        answers.push(await outgoing.hasExceededRateLimitNoConsume(ip, endpoint));
                    } else if (op === "throttle") {
                        if (await outgoing.hasExceededRateLimitNoConsume(ip, endpoint)) {
                            answers.push("wait");
                        } else {
                            await outgoing.consume(ip, endpoint).catch(() => undefined);
                            answers.push("sent");
                        }
                    } else {
                        throw new TypeError(`no limiter operation ${op}`);
                    }
                }
                return { answers };
            }),
    };
};
