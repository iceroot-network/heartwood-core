"use strict";
// Oracle operation of the server's transport, over the reference's socket server as it wires
// itself (socket-server/server.ts initialise: the nes plugin, the four route classes and every
// plugin in its order) with the collaborators stubbed, and one nes socket (nes/socket.ts) over a
// stand-in WebSocket:
//   p2p.serveRequest  {route, hex, ip, remoteAccess, booted, known?, minimumVersions?,
//                     maxTransactionsPerRequest?, milestones?} -> {status, message, sent,
//                     terminated, banReason, connectionClose, acceptPort, versionRecorded}
// The socket says hello, then sends one request whose nes path is `route` (a route id such as
// `p2p.peer.getStatus`, or any other text) with the payload `hex`, from `ip`. The handler is
// stubbed: it answers, and its route's response codec writes nothing. The output:
//   status           200 when the handler answered, else the refusal's status code
//   message          the refusal's message, null when the handler answered
//   sent             the payload text of the graceful error (499) the socket was sent, or null
//   terminated       the reason the nes socket ended with (its `ban`), or null when it stays open
//   banReason        the reason of the address's ban (the ban hammer's first, at the refusal or
//                    when the ended socket closes), or null
//   connectionClose  whether the close-connection plugin answered with `connection: close`
//   acceptPort       the port the peer processor was asked to accept the caller at, or null
//   versionRecorded  the version recorded for the caller, a known peer when `known`, or null
// A refusal of the route's codec ("Payload deserialising failed: <error>") is cut after the
// error's class in `message`, `terminated` and `banReason`; the library's text is in
// notes.message (Heartwood's decoder words it without the protobuf reader's offsets).
// The inbound rate limiter is `ratelimit.run` with `limiter.inbound` (lib/ops/p2p-rules.js).
// Loaded by lib/ops.js.

const path = require("path");

const DEFAULT_RANGES = ["^3.2", "^3.2.0-next.0"];
const PORT = 4002;
const BAN_SECONDS = 10;
const CODEC_FAILED = /^(Payload deserialising failed: [A-Za-z]*Error)(: .*)?$/s;

module.exports = function serverOps({ solar, atHeight, withConfig, hexBuffer }) {
    const dist = path.join(solar.solarDir, "packages", "p2p", "dist");
    const load = (...parts) => require(path.join(dist, ...parts));
    const resolve = (name) => require(require.resolve(name, { paths: [path.join(solar.solarDir, "packages", "p2p")] }));

    /** A nes frame of `type` with `id`, the hello's version, a path and a payload. */
    function frame(type, id, version, route, payload) {
        const path = Buffer.from(route);
        const header = Buffer.alloc(14);
        header.writeUInt8(version, 0);
        header.writeUInt8(type, 1);
        header.writeUInt32BE(id, 2);
        header.writeUInt16BE(200, 6);
        header.writeUInt8(path.length, 8);
        return Buffer.concat([header, path, payload]);
    }

    /** The server's wiring over stubs; what the stubs saw. */
    async function build({ remoteAccess, booted, known, minimumVersions, maxTransactionsPerRequest }) {
        const { Server } = load("socket-server", "server");
        const { Utils } = resolve("@solar-network/kernel");
        const seen = { bans: [], acceptPort: null, versionRecorded: null };
        const p2p = {
            remoteAccess,
            minimumVersions,
            rateLimit: 100,
            rateLimitPostTransactions: 25,
            "server.port": PORT,
        };
        const pool = { maxTransactionsPerRequest };
        const configuration = (options) => ({
            get: (key) => options[key],
            getOptional: (key, fallback) => (options[key] === undefined ? fallback : options[key]),
        });
        const peer = {};
        const deps = {
            logger: {
                debug: (line) => {
                    const ban = /^Banning .* Reason: (.*) :oncoming_police_car:$/s.exec(line);
                    if (ban) seen.bans.push(ban[1]);
                },
                info: () => {},
                warning: () => {},
                error: () => {},
            },
            events: { listen: () => {} },
            peerProcessor: {
                isWhitelisted: ({ ip }) => Utils.isWhitelisted(remoteAccess, ip),
                validateAndAcceptPeer: ({ port }) => {
                    seen.acceptPort = port;
                },
            },
            configuration: configuration(p2p),
            blockchain: { isBooted: () => booted, getQueue: () => ({ isRunning: () => false }) },
            stateStore: { getBlockchain: () => ({ value: "idle" }) },
            peerRepository: { hasPeer: () => known, getPeer: () => peer },
        };
        const controller = new Proxy({}, { get: () => async () => ({}) });
        let banHammer;
        // Around the close-connection plugin: the response before it (a refusal's status and
        // message) and after it (its `connection: close`).
        const observe = (server, method) => server.ext({ type: "onPreResponse", method: (request, h) => (method(request.response), h.continue) });
        const observed = (instance) => {
            const register = instance.register.bind(instance);
            instance.register = (server) => {
                observe(server, (response) => {
                    seen.status = response.isBoom ? response.output.statusCode : response.statusCode;
                    seen.message = response.isBoom ? response.message : null;
                });
                register(server);
                observe(server, (response) => {
                    seen.connectionClose = !response.isBoom && response.headers.connection === "close";
                });
            };
            return instance;
        };
        const app = {
            resolve: (Class) => {
                if (/Controller$/.test(Class.name)) return controller;
                const instance = Object.assign(new Class(), deps, { app });
                if (Class.name === "BanHammerPlugin") banHammer = instance;
                return Class.name === "CloseConnectionPlugin" ? observed(instance) : instance;
            },
            getTagged: (_id, _tag, plugin) => configuration(plugin === "@solar-network/pool" ? pool : p2p),
        };
        const server = Object.assign(new Server(), { app, logger: deps.logger });
        await server.initialise("P2P", { banSeconds: BAN_SECONDS, hostname: "127.0.0.1", port: PORT });
        Object.defineProperty(seen, "versionRecorded", { get: () => peer.version ?? null });
        return { hapi: server.server, banHammer, seen };
    }

    /** One socket from `ip` through its hello and one request. */
    async function serve(input) {
        const { Socket } = resolve("@solar-network/nes");
        const { Codecs } = load("socket-server", "codecs");
        const payload = hexBuffer(input.hex, "hex");
        // The handler's answer is not under test: every response codec writes nothing.
        const serialisers = Object.values(Codecs).map((codec) => [codec.response, codec.response.serialise]);
        for (const [response] of serialisers) response.serialise = () => Buffer.alloc(0);
        try {
            const { hapi, banHammer, seen } = await build(input);
            const sent = [];
            const ws = {
                readyState: 1,
                _socket: {},
                on: () => {},
                send: (message, done) => {
                    sent.push(message);
                    done();
                },
                terminate: () => {
                    ws.readyState = 3;
                },
                close: () => {},
            };
            const request = { headers: {}, socket: { remoteAddress: input.ip, remotePort: 50000 } };
            const socket = new Socket(ws, request, hapi.plugins.nes._listener, input.ip);
            await socket._onMessage(frame(0, 1, 2, "", Buffer.alloc(0)));
            await socket._onMessage(frame(3, 2, 0, input.route, payload));
            const terminated = ws._socket.ban ?? null;
            // The ban hammer's close handler: the socket's reason bans an address not yet banned.
            if (terminated !== null && !banHammer.banList.has(input.ip)) banHammer.ban(input.ip, terminated);
            // A 499 is the one answer after the hello that is not the handler's.
            const reply = sent[1];
            const graceful = reply && reply.readUInt16BE(6) === 499;
            // Refused before the server's routes (the nes socket's own checks): no plugin ran.
            const nes = seen.status === undefined;
            const notes = {};
            const cut = (text) => {
                const failed = text === null ? null : CODEC_FAILED.exec(text);
                if (!failed) return text;
                notes.message = text;
                return failed[1];
            };
            const output = {
                status: nes ? (terminated === "Not Found" ? 404 : 400) : seen.status,
                message: cut(nes ? terminated : seen.message),
                sent: graceful ? reply.subarray(14 + reply[8] + reply[9]).toString() : null,
                terminated: cut(terminated),
                banReason: cut(seen.bans[0] ?? null),
                connectionClose: seen.connectionClose === true,
                acceptPort: seen.acceptPort,
                versionRecorded: seen.versionRecorded,
            };
            return Object.keys(notes).length ? { ...output, notes } : output;
        } finally {
            for (const [response, serialise] of serialisers) response.serialise = serialise;
        }
    }

    return {
        "p2p.serveRequest": (input, height) =>
            withConfig(input.milestones, () =>
                atHeight(height, () =>
                    serve({
                        ...input,
                        booted: input.booted !== false,
                        known: input.known === true,
                        minimumVersions: input.minimumVersions ?? DEFAULT_RANGES,
                        maxTransactionsPerRequest: input.maxTransactionsPerRequest ?? 40,
                    }),
                ),
            ),
    };
};
