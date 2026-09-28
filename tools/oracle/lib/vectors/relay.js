"use strict";
// The relay's side of its forger:
//   relayForger (V30)  forger.round on a corpus chain at round starts, round ends, before the
//                      epoch and with no delegate at an index; quorum.analyze (cold start, below
//                      the minimum, the local peer and the key removal, an attested key ranked in
//                      the top 53 but outside the round's set, a duplicated key, forked and
//                      over-height peers, every total from 0 to 60 for canForge)
//   statusServe (V31)  status.serve with no forger, one key, an unregistered key and a duplicate
// Each returns its records, or null while it has none.

module.exports = {
    relayForger: async function relayForgerVectors() {
        return null;
    },
    statusServe: async function statusServeVectors() {
        return null;
    },
};
