"use strict";
// The reference-client probe: the reference's own peer client (its nes client, PeerConnector,
// PeerCommunicator, the codecs and the reply schemas) run against a relay, case by case from the
// P2P corpus, one JSON line per case into the output directory. Not built yet: it prints its usage
// and exits with status 2.

const USAGE = "usage: node probe/p2p.js --target IP:PORT --cases FILE --out DIR";

process.stderr.write(`${USAGE}\np2p probe: not built\n`);
process.exit(2);
