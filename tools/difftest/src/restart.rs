//! Restarts during an L2 replay.
//!
//! A restart plan (`restart-plan.json` in a corpus, seeded like the revert plan) names the heights
//! at which the replay restarts, the ways it restarts (the reference's rebuild and saved-state
//! paths; Heartwood's table and replay paths), and the reverts and re-applies that follow a
//! restart at a round end. The judge writes the reference's restart lines
//! (`ts-restarts.jsonl`); the Rust replay closes and opens its store at each planned height,
//! restores by each of Heartwood's paths, and compares its restart line and every later line with
//! the continuous replay's.
