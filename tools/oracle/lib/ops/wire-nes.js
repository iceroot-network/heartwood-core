"use strict";
// Oracle operations of the reference's nes frames (packages/nes/dist/utils: parseNesMessage and
// stringifyNesMessage), without the extended types, as the peer-to-peer port runs them:
//   nes.parse      {hex}      -> {version, type, id, method, statusCode, path, socket,
//                                 heartbeat, payload (hex)}, or the reference's error
//   nes.stringify  {message}  -> {hex}, or the reference's error; message fields as NesMessage has
//                                 them, the payload as hex or as text
// Loaded by lib/ops.js; none is defined yet.

module.exports = function wireNesOps(context) {
    void context;
    return {};
};
