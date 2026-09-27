# L2 corpora

A corpus is a chain that a reference devnet produced, in the format of `difftest corpus import`
(see `tools/difftest/src/corpus.rs`), with the digest lines that the oracle's judge
(`tools/oracle/replay-chain.js`) recorded for it. `cargo test -p heartwood-difftest --test l2`
replays the committed corpus in Heartwood through its store, with its restarts, and compares
every step with those lines; no oracle runs.

## txgen-one-node

A one-node devnet of the reference implementation with its default public seed, run natively:
the scenario generator of the maintainers' devnet tooling (a separate tool that is not
published) sends every kept transaction type (transfers with one to 256 items and memos of up to
255 bytes, votes and re-votes, delegate registrations, temporary and permanent resignations and a
revoke 106 blocks after its resignation, burns, a second-signature registration and doubly
signed transactions, fees at the dynamic floor) and its expected refusals; the node is restarted,
and its forger is stopped long enough to miss slots, so that some delegates forge twice in a
round and receive the secondary reward. The chain was archived at the end of its fifth round
(height 265).

- `blocks.jsonl`: the 264 blocks after the genesis block, as a relay receives them.
- `reference/`: the node's round rows, missed slots, block columns and the API state at the tip,
  which the judge's anchors compare with its headless replay. `reference/restarts.jsonl` and
  `reference/restarts/h000000177.json`: the node's live restart at height 177, once from its
  saved state and once without it, and the state it showed right after each (the two were
  identical), which the judge's restarts by the same paths must reproduce (anchor A6).
- `revert-plan.json`: one block reverted and applied again at every round end, and 20 reverts of
  1 to 102 blocks at seeded heights.
- `restart-plan.json`: a restart at every height, by the reference's two start paths and by
  Heartwood's two, with a revert of one block and of one round and one block after each restart
  at a round end.
- `ts-digests.jsonl`: 2,073 lines (the genesis step, 264 applies, 904 reverts and 904 re-applies).
- `ts-restarts.jsonl`: the reference's restart line at every height by each of its paths (530
  lines); each equals the continuous line of its height, and every line after it the continuous
  line of its step.
- `anchors.json`: the judge's anchors; all hold.

To judge it again (after a change of the oracle or its image):

```sh
make corpus-judge NAME=txgen-one-node CORPUS=tools/difftest/corpus RESTARTS=every
make l2 NAME=txgen-one-node CORPUS=tools/difftest/corpus
make l2-restart NAME=txgen-one-node CORPUS=tools/difftest/corpus
make l2-synthetic NAME=txgen-one-node CORPUS=tools/difftest/corpus
```

The judge records its lines and anchor results either way; an L2 run fails when the anchors do
not hold, since a replay that does not match the node is no judge. The longer chains that the
maintainers make with the same tooling (three nodes with faults, and a soak of at least 50 rounds)
are imported under `target/corpus` and are not committed.
