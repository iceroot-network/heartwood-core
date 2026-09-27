"use strict";
// Oracle operations of the reference's nes frames (packages/nes/dist/utils: parseNesMessage and
// stringifyNesMessage), without the extended types, as the peer-to-peer port runs them:
//   nes.parse      {hex}      -> {version, type, id, method, statusCode, path, socket,
//                                 heartbeat: {interval, timeout}, payload (hex)}, or the
//                                 reference's error
//   nes.stringify  {message}  -> {hex}, or the reference's error; the message has the fields of a
//                                 NesMessage (type, method, version, id, statusCode, path, socket,
//                                 heartbeat), each optional, and the payload as hex
// Loaded by lib/ops.js.

const path = require("path");

module.exports = function wireNesOps({ solar, hexBuffer }) {
    let utils;
    const nes = () => (utils ??= require(path.join(solar.solarDir, "packages", "nes", "dist", "utils.js")));

    return {
        // parseNesMessage(buf): the P2P server (socket.ts) and client (client.ts) call it with
        // one argument, so the extended types are off.
        "nes.parse": ({ hex }) => {
            const m = nes().parseNesMessage(hexBuffer(hex, "hex"));
            return {
                version: m.version,
                type: m.type,
                id: m.id,
                method: m.method,
                statusCode: m.statusCode,
                path: m.path,
                socket: m.socket,
                heartbeat: { interval: m.heartbeat.interval, timeout: m.heartbeat.timeout },
                payload: m.payload.toString("hex"),
            };
        },

        // stringifyNesMessage(message) on a message with the given fields only.
        "nes.stringify": ({ message }) => {
            if (message === null || typeof message !== "object" || Array.isArray(message)) {
                throw new TypeError("message must be an object");
            }
            const m = { ...message };
            if (m.payload !== undefined) m.payload = hexBuffer(m.payload, "message.payload");
            return { hex: nes().stringifyNesMessage(m).toString("hex") };
        },
    };
};
