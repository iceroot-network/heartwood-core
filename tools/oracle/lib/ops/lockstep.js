"use strict";
// Oracle operations of the lockstep producer: a chain session with a reference pool bound in its
// node (lib/lockstep.js), which the pool operations also take by its id:
//   lockstep.open    {crypto, options?} -> {session, line}
//   lockstep.build   {session, build: [spec]} -> {transactions: [{id, hex}]}  (built once, with
//                    the fixed aux; a spec is {kind, passphrase, nonce, fee?, memo?, asset}, its
//                    fee the network's floor at the call's height when absent; kind is transfer,
//                    secondSignature, vote, delegateRegistration, delegateResignation, burn or ipfs)
//   lockstep.submit  {session, transactions: [hex], now} -> the pool's result {accept, broadcast,
//                    invalid, excess, errors?}
//   lockstep.online  {session, round, delegates: [publicKey], expireAt?} -> {online}
//   lockstep.apply   {session, hex, step?, expireAt?} -> {accepted, height, id, reward?, missed, line?, stage?, error?}
//   lockstep.revert  {session, count} -> {lines}
//   lockstep.close   {session} -> {closed: true}
// Loaded by lib/ops.js.

const { lockstepOps } = require("../lockstep");

module.exports = function lockstepFile(context) {
    return lockstepOps(context);
};
