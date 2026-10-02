# Operating Heartwood Core

This guide is for people who run Heartwood nodes: a relay that follows a network and serves it, a
forger that holds a validator's keys and signs blocks, the optional `core` role that runs both in
one process, and the local devnet that runs five nodes on one machine for practice. It describes
what the code in this repository does today.

- Heartwood nodes run on Linux only. Every command here is for Linux.
- The public testnet has not launched. Its network name, ports, release and files will come with
  its announcement; until then the guide teaches the flows on a throwaway test network that
  `heartwood network generate` makes.
- What is not built yet is marked **Not built yet** and listed in the last section.
- Every block in the language `sh` runs as written on a Linux machine: `make ops-doc-check` builds
  the release binary and runs them in order on two private machines (network namespaces, no
  root), so a command that stops working fails the check. Blocks marked `text`, `json` and
  `ini` are shown, never run. Where a block says `forger-host` or `relay-host`, it is
  addressing another machine, as `scp` does.

The test network's keys come from a seed that anyone can read in this guide. Never use a key, an
address or a file from the examples for anything of value.

## 1. What a node runs

One binary, `heartwood`, runs each role as its own operating system process.

- **The relay** talks to other nodes, checks blocks and transactions, keeps the chain in a local
  database and serves the node's HTTP API. It never reads the forger's keys.
- **The forger** holds the validator's block-signing keys. It signs the blocks that are due, and
  it signs its relay's "online" status messages when the relay asks. It opens no listening port,
  keeps no chain database and serves no API. It connects to a relay and asks it for work.
- **The core role** is a relay and a forger in one process. It exists for tests and small setups
  and no default configuration or command selects it. It puts the validator's keys in the process
  that serves the network's peer-to-peer port and the API, so a bug or an attack on either can
  reach the keys. Run the split roles on a validator.

A forger can connect to more than one relay and uses the first that is ready; that is how you get
failover (section 15). One validator belongs to one operator: its relay, its forger and any
failover or API relays all work for that validator.

**A relay counts a forger as online only when the forger connects from the same machine, over
loopback.** A forger on another machine still forges through its relay, but the relay does not
attest its delegates, other nodes do not see them as online, and their transaction pools refuse
votes for them. An authenticated link for a forger on another machine comes in a later stage.
Section 9 shows the split-machine flow and what it costs.

## 2. Requirements

These are planning figures, not measured minimums.

| Role | CPU | Memory | Disk | Network |
| --- | --- | --- | --- | --- |
| Relay | 4 vCPU, dedicated preferred | 8 GB | 250 GB NVMe SSD to start | 1 Gbit/s port |
| Forger | 2 vCPU | 2 GB | 20 GB | A private link to two or three relays |

If one machine runs both roles, add the figures and still run two processes.

- **Linux, x86-64**, a normal user account (not root), and the `HOME` variable set: the relay and
  the forger stop at start without it.
- **Disk.** A node keeps every block and the database only grows. Use a local SSD or NVMe, never a
  network filesystem.
- **Clock.** A block comes every 8 seconds. The forger does not forge when less than 2 seconds of
  its slot are left and refuses to sign for a slot more than one slot from its own clock. Run
  NTP or chrony on every host. The node does not check that the clock is synchronized;
  `heartwood doctor` does (section 20).
- **Rust**, only to build from source: 1.98 or later and a C compiler (for example the
  `build-essential` package), because the build compiles the bundled SQLite.

## 3. Installing

Build the node from source. Build the `heartwood-core` package alone; do not use `--workspace` for
a release, because the test tools enable features that must not enter the node.

```text
cargo build --release --locked -p heartwood-core
```

The file is `target/release/heartwood`. Put it on your path, for example in `/usr/local/bin`.
Check what you have:

```sh
heartwood version
```

```sh expect="features:"
heartwood version --verbose
```

The version reads like `4.3.1-heartwood.<release>`: the first part is the protocol version the
reference implementation reports, the second the Heartwood release. `--verbose` also prints the
platform and the features the build has; a release build never lists `difftest`.

**Verifying a download.** Downloads start with the public testnet. Each announcement names the
release and the SHA-256 checksum of every download; compare the checksum in the release notes
with the one in the announcement, then compute your file's:

```text
sha256sum heartwood
```

If the three do not match, stop. Releases are not signed with a key: the checksum, published in
two places, is the check.

> **Not built yet.** There is no install script and no package. A binary you build yourself may
> not match a published checksum until reproducible builds exist.

## 4. Where files live

Heartwood follows the standard Linux directories. Under each it uses a directory named
`<token>-core/<network>`. The token is `solar` unless you give another with `--token`; the
network name comes from the release. A token or a network name holds letters, digits, `-` and `_`
only. This guide's test network is `guide`, so its directories carry `solar-core` and `guide`.

| What | Default | Override |
| --- | --- | --- |
| Configuration | `~/.config/<token>-core/<network>` | `CORE_PATH_CONFIG` |
| Data: database, signing record, run files | `~/.local/share/<token>-core/<network>` | `CORE_PATH_DATA` |
| Cache | `~/.cache/<token>-core/<network>` | `CORE_PATH_CACHE` |
| Logs | `~/.local/state/<token>-core/<network>` | `CORE_PATH_LOG` |
| Temporary files | `<tmp>/<user>/<token>-core/<network>` | `CORE_PATH_TEMP` |

`<tmp>` is `$TMPDIR`, else `$TMP`, `$TEMP` or `/tmp`; `<user>` is the last part of your home
directory's path. The `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME`
variables are honoured. An override replaces the whole path, network directory included; a
leading `~` is expanded. `heartwood env paths` prints the five paths of an instance.

The configuration directory holds:

| File | Read by | What it is |
| --- | --- | --- |
| `crypto/network.json`, `crypto/milestones.json`, `crypto/genesisBlock.json` | both | The network's definition, its rule changes by height with the version floor (section 18), and its first block |
| `crypto/exceptions.json` | both | Must hold exactly `{}` |
| `app.json` | both | The components each role starts, in three profiles: `relay`, `forger` and `core` |
| `peers.json` | relay | A `list` of seed peers, each `{"ip": ..., "port": ...}`, and an optional `sources` list |
| `.env` | both | Settings, one `NAME=value` per line. Required, even when empty |
| `delegates.json` | forger | The forger's private keys (section 7) |
| `config.json` | the commands | Optional: the token and the network the commands use when no flag names them |

The four `crypto` files are required and there are no built-in networks: a network's release gives
you its files. Put them in place before the first start.

- Only the first entry of `sources` in `peers.json` is read, and it must be a plain HTTP URL. An
  HTTPS source stops the relay at start.
- A relay with no seed at all stops. A peer at a private address (`10.x.x.x`, `172.16.x.x`,
  `192.168.x.x`, or a link-local or unique-local one) is accepted only when
  `CORE_P2P_ALLOW_PRIVATE_IP_RANGES` is set to a value in `.env`. A loopback address and any
  address of the machine itself are never accepted as a peer (a devnet node's own addressing is
  separate).
- A variable set in the process's own environment wins over the same name in `.env`. A file
  `~/.<token>/.env`, such as `~/.solar/.env`, wins over both. Remove an old one you do not use.
- The relay does not need `delegates.json`. Keep it off every machine that runs a relay only.

**Choosing the instance.** Every role command takes `--token` and `--network`; a flag always wins.
To save typing, put `{"token": "...", "network": "..."}` in a `config.json` one level up,
`~/.config/<token>-core/config.json`, or in the instance's own configuration directory. If
nothing names the network and exactly one network directory under the token's configuration
directory holds a complete `crypto` directory, that network is used. The colon spellings of the
reference (`relay:start`, `config:forger:private-key`) work for every command. This guide always
names the network.

## 5. A test network to practise on

`heartwood network generate` makes a whole new test network from a seed: the chain definition, the
keys of its 53 delegates, and the files of one node that holds all of them. The same seed gives
the same bytes. Do it directly into the configuration directory of the instance:

```sh expect="Wrote"
heartwood network generate --network guide --seed operating-guide --out ~/.config/solar-core/guide
```

```sh expect="600 "
stat -c '%a %n' ~/.config/solar-core/guide/delegates.json
```

The command refuses a directory that is not empty unless you pass `--force`. It writes the key
files, `delegates.json` and `genesis-wallet.json`, at mode 0600. `genesis-wallet.json` holds every
key and passphrase of the test network; a real network has no such file. The instance's `.env`
binds the node to `127.0.0.1` with P2P port 6002 and API port 6003, and its `peers.json` names the
node itself, so that a one-node network has a seed.

```sh expect="solar-core/guide"
heartwood env paths --network guide
heartwood env list --network guide
```

`env get --key NAME` prints one variable, `env list` all of them, `env set --key NAME --value
VALUE` changes one and `env paths` prints the five directories. `env set` writes the file again
from its parsed form: every variable as `NAME=value` in the same order, comment lines and blank
lines dropped. It replaces the file through a temporary file at mode 0600, and refuses a symbolic
link where the file should be.

## 6. A relay and a forger on one machine

This is the shape of a small validator and of every practice network. Start the relay first. A new
network's first node starts with `--networkStart`, which lets it begin the chain without peers;
use it only to start a new network, never to join one that exists.

```sh
heartwood relay start --network guide --networkStart
```

`start` runs the relay under the built-in supervisor, as a daemon, and returns once the supervisor
holds its lock: the relay itself is still starting. Wait until its API answers.

```sh timeout=120
until curl -fsS http://127.0.0.1:6003/api/node/status >/dev/null 2>&1; do sleep 1; done
curl -fsS http://127.0.0.1:6003/api/node/status
```

Now the forger. It reads `delegates.json`, connects to the relay at `127.0.0.1` (its `hosts` in
`app.json`) and forges the blocks that are due.

```sh
heartwood forger start --network guide
```

Wait for the third block, one every 8 seconds or so. The API's `/api/blocks/last` answers with the
newest block:

```sh timeout=240
height() { curl -fsS http://127.0.0.1:6003/api/blocks/last | grep -Eo '"height": ?[0-9]+' | sed -n '1s/[^0-9]//gp'; }
until [ "$(height)" -ge 3 ] 2>/dev/null; do sleep 2; done
echo "the chain is at height $(height)"
```

Look at the two processes:

```sh expect="solar-guide-forger"
heartwood relay status --network guide
heartwood forger status --network guide
```

```text
ID     Name               Version                Status  Uptime  Restarts  CPU   RAM
41233  solar-guide-relay  4.3.1-heartwood.0.3.0  online  2m 4s   0         1.2%  52.3 MB
```

`status` prints a table row with the process id, name (`<token>-<network>-<role>`), version,
status, uptime, restarts, CPU and memory. The statuses are `launching`, `online`,
`online (unsupervised)` (a node started with `run` in a terminal), `stopping`, `stopped` and
`errored`. `heartwood top` lists every running instance of a token across its networks with the
same columns:

```sh expect="solar-guide-relay"
heartwood top
```

`top --token NAME` lists another token's instances. With nothing running it says "No processes are
running" and exits with status 1.

Each role keeps its own logs, so one role's problems never bury the other's (section 17):

```sh follows expect="Tailing last 5 lines"
heartwood forger log --network guide --lines 5
```

`log` prints the last lines of what the process printed and follows it until you press Ctrl+C.

## 7. The forger's keys

The forger reads its private keys from one file, `delegates.json`, in its configuration directory:

```text
{"keys": ["<64 hex digits>", "<64 hex digits>"]}
```

Rules the forger enforces:

- The file must exist and hold at least one key. Without the file it stops with "The ... file does
  not exist"; with no key, with "Unable to detect any private keys".
- **The file must be private.** The forger refuses to start when the group or others have any
  permission on it, even to read, as SSH refuses a private key file. Use mode 600.
- Only `keys` is read. A non-empty `secrets` list is refused, because passphrases are not
  supported, and so is any other top-level name. Each key must be 64 hexadecimal digits of a valid
  private key. A key listed twice gets a warning.
- The node never rewrites the file. Its text is wiped from memory after it is read, the keys are
  wiped when the forger stops, and no key is written to a log.

See the refusal for yourself, and the fix:

```sh fails expect="open to its group or to others"
heartwood forger stop --network guide
chmod 644 ~/.config/solar-core/guide/delegates.json
heartwood forger start --network guide
```

```sh
chmod 600 ~/.config/solar-core/guide/delegates.json
heartwood forger start --network guide
```

The command refuses before it starts anything, so a refused start leaves no process behind.

Also remember:

- **In this stage the forging key is the validator account's own key.** The key in
  `delegates.json` is the private key of the validator's account, the one that holds its funds.
  Anyone who can read the file can sign blocks as the validator and spend from the account. Protect
  the file and the forger host with that in mind. Separate keys for the account, for block signing
  and for the relay's identity come with a later protocol stage.
- **One key on exactly one forger.** Two forgers with the same key can sign two different blocks
  for one slot, which can split the network. Never start a standby forger with the same key while
  the first might still run.
- Use the file only on the forger host. A relay never opens it.
- Writing a key on the command line (`config forger private-key`, section 12) puts it in your
  shell history and in the process list while the command runs. Do that only with test keys, or
  copy a key file instead.

> **Not built yet.** No command makes a production forger key, and the file is not encrypted at
> rest: use file modes and offline storage. `heartwood network generate` makes test keys from a
> seed.

### The signing record

The forger keeps a record of the last block it signed for each key in
`<data>/forger/signing-record.json`, by default
`~/.local/share/<token>-core/<network>/forger/signing-record.json`. It creates the file with mode
0600 and, if the directory does not exist, the directory with mode 0700. It writes the record and
forces it to disk before a block leaves the forger. With the record the forger:

- never signs a different block for a slot it has already signed: a retry sends the same block
  again;
- refuses a slot earlier than the last one it signed for that key;
- refuses a slot more than one slot away from its own clock.

A record that belongs to another network, or cannot be read, stops the forger. **If the record is
missing, the forger creates a new empty one and starts without asking**, with no memory of what it
signed before. The record holds no secret, only public block data, but it is part of the key's
safety:

- Never delete, edit or replace it to get around a refusal. If the forger will not start because of
  the record, stop and ask the team.
- **Never restore a signing record from an old copy or a backup.** An old record can let the forger
  sign a block it should refuse.
- When you move a forger to another machine, stop the old forger first, then move the key and its
  signing record together, delete both from the old machine, and start the new forger.

> **Not built yet.** A check that makes you confirm before forging with a missing or old record.
> Today the forger creates a new record and starts, so keeping the record with its key is up to you.

## 8. A relay without keys

A relay that serves the network and holds no forger keys is the safe shape for every machine that
does not forge: a failover relay (section 15), an API relay, a node that only follows. Start from
the template, add the network's crypto files and its peers, and start it.

On a second machine of its own, with a separate token so that it does not collide with the forger
of section 9:

```sh second expect="delegates.json"
heartwood config publish --token follower --network guide
ls ~/.config/follower-core/guide
```

`config publish` writes the instance's configuration directory from the built-in template: `.env`
with the ports 6002 and 6003, an `app.json` with the `relay`, `forger` and `core` profiles, an
empty `peers.json`, an empty `delegates.json` (`{"keys": []}`, mode 0600) and `config.json`. It
writes no crypto file: those come with the network's release. It refuses a directory that already
holds a configuration unless you pass `--force`.

Before another machine can reach the first machine's relay, the relay has to listen beyond
loopback. Its `.env` binds it to `127.0.0.1`; bind it to every address instead (the default when
the variable is not set) and restart it. Keep the API on loopback.

```sh expect="CORE_P2P_HOST"
heartwood env set --network guide --key CORE_P2P_HOST --value 0.0.0.0
heartwood relay restart --network guide
heartwood env list --network guide
```

```sh timeout=120
until curl -fsS http://127.0.0.1:6003/api/node/status >/dev/null 2>&1; do sleep 1; done
```

Now give the second machine the network's crypto files, which are public, and tell it where to
find a peer. The files travel with `scp` here; any encrypted copy does.

```sh
scp -r ~/.config/solar-core/guide/crypto forger-host:.config/follower-core/guide/
```

```sh second
cat > ~/.config/follower-core/guide/peers.json <<'EOF'
{"list": [{"ip": "10.90.0.1", "port": 6002}]}
EOF
heartwood env set --token follower --network guide --key CORE_P2P_ALLOW_PRIVATE_IP_RANGES --value true
heartwood env set --token follower --network guide --key CORE_P2P_MIN_NETWORK_REACH --value 0
heartwood env set --token follower --network guide --key CORE_API_HOST --value 127.0.0.1
heartwood relay start --token follower --network guide
```

The peer is at a private address, which a relay accepts only with
`CORE_P2P_ALLOW_PRIVATE_IP_RANGES`; keep that setting for private networks only. The relay syncs
from its peer. Wait until it holds the blocks the forger has made:

```sh second timeout=240
height() { curl -fsS http://127.0.0.1:6003/api/blocks/last | grep -Eo '"height": ?[0-9]+' | sed -n '1s/[^0-9]//gp'; }
until [ "$(height)" -ge 3 ] 2>/dev/null; do sleep 2; done
echo "the follower is at height $(height)"
```

This node has no key anywhere, and nothing it can do would sign a block. It is a relay-only node.

```sh second expect="follower-guide-relay"
heartwood relay status --token follower --network guide
ls ~/.config/follower-core/guide
```

## 9. A forger on a second machine

The forger can run on another machine and drive a relay over the network. Use this for a forger
that must not share a host with a public-facing relay. It has a cost, which section 1 states and
this section shows: **the relay attests only a forger that connects over loopback, so the
delegates of a forger on another machine are not attested.** They still forge, but other nodes do
not see them as online and refuse votes for them.

The forger moves to the second machine, with the key and the signing record, and the relay learns
to admit it.

**1. Move the key and its signing record together.** Stop the forger, make the second machine's
directories private before the key arrives, copy the key and the record, and delete both from the
old machine. The first machine's relay then holds no key, which is what a relay's machine should
hold. (The test network's `genesis-wallet.json` holds every key too, so it goes as well.)

```sh
heartwood forger stop --network guide
```

```sh second
mkdir -p -m 700 ~/.config/solar-core ~/.local/share/solar-core/guide
mkdir -m 700 ~/.config/solar-core/guide ~/.local/share/solar-core/guide/forger
```

```sh
scp -p -r ~/.config/solar-core/guide/crypto forger-host:.config/solar-core/guide/
scp -p ~/.config/solar-core/guide/delegates.json forger-host:.config/solar-core/guide/
scp -p ~/.local/share/solar-core/guide/forger/signing-record.json forger-host:.local/share/solar-core/guide/forger/
rm ~/.config/solar-core/guide/delegates.json ~/.config/solar-core/guide/genesis-wallet.json
rm ~/.local/share/solar-core/guide/forger/signing-record.json
```

**2. Let the relay admit the forger's address.** A relay answers the forger's calls only for
addresses in the `remoteAccess` option of its `p2p` component in `app.json`. The default is
loopback only. Put the forger's plain IP address in the list. This writes the relay's profile
again with the address added (it replaces the file, so keep any other setting you made), and
restarts the relay. The firewall of a private relay must let the forger's address, and no other,
reach the peer-to-peer port (section 14).

```sh timeout=120
cat > ~/.config/solar-core/guide/app.json <<'EOF'
{
    "relay": {
        "plugins": [
            {"package": "@solar-network/logger"},
            {"package": "@solar-network/state"},
            {"package": "@solar-network/database"},
            {"package": "@solar-network/transactions"},
            {"package": "@solar-network/pool"},
            {
                "package": "@solar-network/p2p",
                "options": {
                    "remoteAccess": ["127.0.0.1", "::ffff:127.0.0.1", "::1", "10.90.0.2"],
                    "dns": [],
                    "ntp": []
                }
            },
            {"package": "@solar-network/blockchain"},
            {"package": "@solar-network/api"}
        ]
    }
}
EOF
heartwood relay restart --network guide
until curl -fsS http://127.0.0.1:6003/api/node/status >/dev/null 2>&1; do sleep 1; done
```

**3. Point the forger at the relay.** In the forger's `app.json`, the forger component's `hosts`
lists the relays it uses, by IP address and port, and its profile needs only the logger, the
transaction code and the forger. Its `.env` may be empty but must exist.

```sh second
cat > ~/.config/solar-core/guide/app.json <<'EOF'
{
    "forger": {
        "plugins": [
            {"package": "@solar-network/logger"},
            {"package": "@solar-network/transactions"},
            {
                "package": "@solar-network/forger",
                "options": {"hosts": [{"hostname": "10.90.0.1", "port": 6002}]}
            }
        ]
    }
}
EOF
: > ~/.config/solar-core/guide/.env
heartwood forger start --network guide
```

The chain goes on, now with its blocks made on the second machine. Watch the relay's height rise
by two blocks:

```sh timeout=300
height() { curl -fsS http://127.0.0.1:6003/api/blocks/last | grep -Eo '"height": ?[0-9]+' | sed -n '1s/[^0-9]//gp'; }
before=$(height)
until [ "$(height)" -ge $((before + 2)) ] 2>/dev/null; do sleep 2; done
echo "the chain went from height $before to $(height)"
```

The second machine's `forger log` shows the forger working through the relay; the relay's log
shows no attested forger, which is the cost of this shape. Stop everything:

```sh second
heartwood forger stop --network guide
heartwood relay stop --token follower --network guide
```

```sh
heartwood relay stop --network guide
```

Notes for a real split:

- Link the forger to its relays over a private link such as WireGuard and allow only the tunnel
  addresses.
- A new connection from an address closes that address's older connections to the relay, the
  forger's included. Test whether the port is reachable from a machine that has no other link to
  the relay.
- A host name is not accepted in `hosts` or in `remoteAccess`, and a range such as `10.0.0.0/24` in
  `remoteAccess` is accepted but matches nothing (the relay logs a warning): list plain IP
  addresses.

## 10. Five nodes on one machine

`heartwood devnet` builds a practice network on one machine: five validators, each a relay and a
forger under their own supervisors, each node at its own loopback address (node k at
`127.<block>.(10 + k)`, P2P on port 6002 and the API on 6003). Pick a block of the 127.0.0.0/8
range that nothing else uses. This is the only mode: it needs no root on Linux, because the
loopback interface carries the whole range. Do not run it on a validator.

```sh timeout=120 expect="Created a devnet of 5 nodes"
heartwood devnet --dir ~/devnet init --validators 5 --address-prefix 127.49.0
```

`init` generates the chain from a seed, writes each node's configuration and its share of the
delegates' keys (11, 11, 11, 10 and 10 of the 53), and refuses a listener address another process
holds. Start it, wait for a height, and look at it:

```sh timeout=420
heartwood devnet --dir ~/devnet start
heartwood devnet --dir ~/devnet wait --height 3
```

```sh expect="chain consistent"
heartwood devnet --dir ~/devnet status
```

`status` shows every node's processes and height, whether every relay has the same chain
(`consistent`, or `DIVERGED at` a height), and how many delegates are online of the quorum a
forging network needs. Each node has its own token (`node1`, `node2`, ...) and its own logs, one
file per role:

```sh expect="node1-devnet-relay"
heartwood top --token node1
heartwood devnet --dir ~/devnet logs --node 1 --role relay --lines 3
heartwood devnet --dir ~/devnet logs --node 1 --role forger --lines 3
```

Stop one node's forger, see the network go on with the other four, and start it again:

```sh timeout=120
heartwood devnet --dir ~/devnet stop-node 2 --role forger
heartwood devnet --dir ~/devnet start-node 2 --role forger
heartwood devnet --dir ~/devnet stop
```

`stop-node`, `start-node`, `restart-node` (each with `--role relay|forger` and `--kill` for a
SIGKILL), `reset` (a node's chain, logs and caches; its signing record stays), `logs --follow`,
`env N` (a node's environment), `status --json` and `status --resources` act on the nodes, and
`init --core` makes each validator one `core` process, `init --failover 1:1,6,7` a forger that
uses several relays, `--relays N` extra relay-only nodes. A node refuses to start in a network
whose name is not `devnet` or on an address that is not loopback.

## 11. The core role

`heartwood core` runs a relay and a forger in one process. The forger reaches its relay on the
loopback address, and the relay attests it as it attests any forger on its own machine. The
commands are the roles' own: `core run`, `core start`, `core stop`, `core restart`, `core status`
and `core log`. `core start` is a daemon by default.

**It holds the keys in the process that serves the network.** The peer-to-peer port and the API
are in the process that signs. Use it for tests and small setups, never by default on a validator.

Make a second test network for it, so that nothing of the earlier ones is in the way:

```sh expect="Wrote"
heartwood network generate --network solo --seed core-role --out ~/.config/solar-core/solo
```

```sh
heartwood core start --network solo --networkStart
```

```sh timeout=240
height() { curl -fsS http://127.0.0.1:6003/api/blocks/last | grep -Eo '"height": ?[0-9]+' | sed -n '1s/[^0-9]//gp'; }
until [ "$(height)" -ge 3 ] 2>/dev/null; do sleep 2; done
```

One process holds the core's, the relay's and the forger's locks of its instance, so the split
commands refuse while it runs, naming it:

```sh fails expect="solar-solo-core is already running"
heartwood relay start --network solo
```

```sh expect="solar-solo-core"
heartwood core status --network solo
heartwood top
```

A core node refuses to start when the relay's `remoteAccess` does not admit the address its forger
connects from, and when its profile has no forger. If the forger inside it stops with an error, the
whole node stops with the error. Stop it with `core stop`, which asks the node to shut down in order:

```sh
heartwood core stop --network solo
```

## 12. The configuration commands

**`config publish`** writes an instance's configuration from the built-in template, as section 8
showed. It refuses an existing configuration unless `--force` is given:

```sh expect="delegates.json"
heartwood config publish --network scratch
ls ~/.config/solar-core/scratch
```

```sh fails expect="--force"
heartwood config publish --network scratch
```

```sh
heartwood config publish --network scratch --force
```

**`config forger private-key`** puts one private key into an instance's `delegates.json`. The file
must exist (the command refuses an absent one and names its path), the key must be 64 hexadecimal
digits, and the command keeps the file's other top-level keys. It replaces the `keys` list with
that one key, writes the file at mode 0600 through a temporary file and a rename, and refuses a
symbolic link where the file should be. A `secrets` list that is not empty stays where it is, and
the forger's next start refuses it. The key below is not a secret: it is the number 1.

```sh expect="600 "
heartwood config forger private-key --network scratch --privateKey 0000000000000000000000000000000000000000000000000000000000000001
stat -c '%a %n' ~/.config/solar-core/scratch/delegates.json
grep -F 0000000000000000000000000000000000000000000000000000000000000001 ~/.config/solar-core/scratch/delegates.json
```

`config forger` offers one method, the private key (`--method privateKey`, or the subcommand).
The other method, a passphrase (`bip39`), is not offered: passphrases are deferred, and
`--method bip39` says so.

## 13. Running under systemd

`heartwood service install` writes a systemd unit for one role of an instance. It only writes: it
calls no service manager, and it prints the commands that enable the unit. `service uninstall` removes the file.
The unit is `<token>-<network>-<role>.service`: a user unit in `~/.config/systemd/user` by
default, or with `--system` a system unit meant for `/etc/systemd/system`. `--out DIR` writes into
a directory of your choice instead, so that you can read the unit and place it yourself.

```sh expect="solar-solo-relay.service"
mkdir -p ~/units
heartwood service install --role relay --network solo --out ~/units
cat ~/units/solar-solo-relay.service
```

The unit starts `<binary> <role> run --token=<token> --network=<network>` in the foreground, with
`Type=exec`, `Restart=always`, `KillSignal=SIGINT` and `TimeoutStopSec=30` (the node stops on
SIGINT and gets 30 seconds to finish), `UMask=0077` and `NoNewPrivileges=yes`, and whichever
sandboxing directives the host's systemd supports. It sets `CORE_SERVICE_MANAGER=systemd`, which
the node records at its start: under a unit, `heartwood <role> status` reports `online (systemd)`,
and `stop` refuses and names the `systemctl` command to use, so a stop never fights the manager.
The binary's path in `ExecStart` is the file you ran `service install` with: install the unit
from the binary at its permanent place.

Check the unit before you install it, where `systemd-analyze` exists:

```sh optional=systemd-analyze
systemd-analyze --user verify ~/units/solar-solo-relay.service
```

```text
cp ~/units/solar-solo-relay.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now solar-solo-relay.service
loginctl enable-linger "$USER"
```

The last line lets a user unit run when you are not logged in. Run the forger and the relay as two
units; start the relay first.

## 14. Ports and firewall

The relay listens on two ports. The forger listens on none.

| Port | What | Who should reach it |
| --- | --- | --- |
| P2P port | Node-to-node traffic, and the port a forger uses to reach its relay | Public peers, or only your own forger and failover relays if the relay is private |
| API port | The node's HTTP API | Not the public: keep it on loopback or on a private address, or put a separate API relay in front |

The numbers come from the network's configuration: `CORE_P2P_HOST`, `CORE_P2P_PORT`,
`CORE_API_HOST` and `CORE_API_PORT` in `.env`. Without them the node's built-in defaults are 4002
for P2P and 4003 for the API, on every address (`0.0.0.0`); the template and the test networks use
6002 and 6003. Use the ports of the release for the network you join.

- On a validator's relay, set `CORE_API_HOST=127.0.0.1` so the API answers on the machine only,
  or turn the API off (section 15). The API limits each client address to 100 requests per 60
  seconds, which protects it from one client, not from public load.
- The relay refuses to start when `CORE_API_SSL` is set: it serves plain HTTP. Put a reverse proxy
  in front if you need TLS.
- Allow the P2P port in from other nodes, or, on a private relay, only from your forger and your
  failover relays. The forger needs no inbound port; let it connect out to its relays' P2P ports.
  Allow SSH from your own addresses only.
- From a minute after the relay's first sync, a connection that never completes its handshake, such
  as a plain port probe, gets its address banned for `CORE_P2P_BAN_SECONDS` (10 seconds by
  default). Addresses in `remoteAccess` are never banned.

### Which forger addresses a relay accepts

A relay answers the forger's calls only for the addresses in `remoteAccess`, an option of the
`p2p` component in `app.json`. The default is the local machine (`127.0.0.1`, `::ffff:127.0.0.1`
and `::1`). **An empty list admits nobody**, and the relay logs a warning: an empty list does not
open the relay to everyone. A host name is refused and the relay does not start. See section 9
for the split-machine case.

## 15. Validator shapes

These are the shapes a validator uses on a public network. Each uses the files and options above.

**A co-located primary relay with remote failover relays.** The validator's machine runs the
forger and its primary relay. One or two more relays on other machines are the forger's failover
relays. In the forger's `hosts`, list the relay on the forger's machine first, as `127.0.0.1`, then
the failover relays by IP address:

```json
"options": {
    "hosts": [
        {"hostname": "127.0.0.1", "port": 6002},
        {"hostname": "10.0.0.12", "port": 6002}
    ]
}
```

The forger uses the first host that is ready and goes back to it by itself once it is ready again.
To add a failover relay: set up the relay on another machine and sync it fully; add the forger's
address to its `remoteAccess`; add its address to the forger's `hosts` after the first; restart
the forger. Failover changes which relay the forger talks to. It does not add a second forger:
keep one forger per key. While the forger works through a relay on another machine, that relay
does not attest the validator (section 9), so the validator shows as offline for votes. Link the
forger to its relays over a private link.

**A relay whose API is off.** A relay can run without serving the API: set a value in `.env`
(any value that is not empty, even `false`) or leave the `api` component out of the relay's profile:

```text
CORE_API_DISABLED=true
```

The relay then opens one port, the P2P port. Nothing else of the node changes: the forger does
not use the API, and `heartwood doctor` works without it. Use this on the primary relay of a
validator.

**An API-only relay without delegates.** A relay that serves the public API holds no keys and
serves no forger: it has no `delegates.json`, no forger runs against it, and its `remoteAccess`
is empty. An empty list makes the relay log a warning, which is the expected state of such a relay.
Put it behind a reverse proxy with its own rate limits and caching, and run it on a different
machine from the validator.

**Joining the public testnet.** The testnet has not launched; its announcement gives the network
name, the ports, the release and the files. The steps then are: build or download the release and
check its checksum (section 3); `config publish` the instance and place the network's `crypto`
files and its peers; start the relay and wait until it is in sync (`relay status`, and the API's
`/api/node/status`); check the machine with `heartwood doctor --profile onboarding` (section 20);
then start the forger.

## 16. Starting and stopping

Each role has the same commands.

| Command | What it does |
| --- | --- |
| `heartwood relay run` | Runs the relay in the foreground, with no supervisor |
| `heartwood relay start` | Starts the relay under the supervisor, in the background; returns once the supervisor runs |
| `heartwood relay stop` | Stops it and waits until it has exited; also stops a relay started with `run` |
| `heartwood relay restart` | Stops it and starts it again with the same arguments and your current environment |
| `heartwood relay status` | Shows its row of the process table |
| `heartwood relay log` | Shows the last lines of what it printed and follows it |

Replace `relay` with `forger` or `core` for the other roles. Add `--token` and `--network` unless
`config.json` names them. `start` refuses a role that already runs for that token and network
("... is already running"): a lock file prevents a second copy. The relay and the forger refuse to
start while the core role of the same instance runs, and the core role refuses while either does.
To keep a started role in the foreground use `--daemon=false`, or use `run`. An unknown flag is an
error, with a usage line and exit status 2.

The relay takes some more flags: `--networkStart`, `--disableDiscovery`, `--skipDiscovery`,
`--ignoreMinimumNetworkReach` and `--launchMode`. They are for starting or testing a network. Do
not use them on a node that joins an existing network. The forger refuses them.

**What the supervisor does.** It is a small helper that `start` launches; you never run it by hand.

- If the node exits without being asked to stop, the supervisor starts it again at once, with no
  delay and no limit.
- If the node dies within a second of starting five times during the supervisor's first five
  seconds, the supervisor gives up: the status shows `errored` and the supervisor exits. Read the
  log to find out why.
- A node that fails later is started again every time. Watch the `Restarts` column.
- `stop` asks the node to shut down cleanly (SIGINT to the node). If it has not gone after 30
  seconds it is killed.
- A hangup signal is ignored, so closing your terminal does not stop a started node.

The node and its supervisor keep small files in `<data>/run`: `<role>.lock`, `<role>.pid`,
`<role>.supervisor.lock` and `<role>.state.json`. The directory has mode 700; the tools refuse it
if another user owns it or it is a symbolic link.

## 17. Logs

Each role writes two kinds of log, in `<log>`, by default `~/.local/state/<token>-core/<network>`:

| Log | File | Rotation |
| --- | --- | --- |
| The node's own log | `<token>-<network>-<role>-current.log` | At the first write after midnight UTC, once a day; old files are compressed to `<name>-<date>.log.gz`, at most 10 of them and 100 MiB |
| What the process printed, for a role started with `start` | `<token>-<network>-<role>-out.log` and `-error.log` | At 500 MB into 7 numbered files, `.1` to `.7`, not compressed |

The relay, the forger and the core role each have their own files, so the roles' logs are
independent:

```sh expect="solar-guide-relay-current.log" expect="solar-guide-forger-current.log"
ls ~/.local/state/solar-core/guide
```

`heartwood <role> log` reads the captured output, not the node's own log file:

```text
heartwood relay log --network <network>
heartwood relay log --network <network> --error
heartwood relay log --network <network> --lines 100
```

The default is the last 15 lines, followed live; `--error` reads the error capture. Read the
node's own log with ordinary tools, for example `tail -f`. The console level is `CORE_LOG_LEVEL`
(default `info`) and the file's is `CORE_LOG_LEVEL_FILE` (default `debug`). A log rotates by date
only, never by size: watch the disk. `heartwood devnet logs` merges a devnet's logs in time order.

## 18. Upgrades and the version floor

A release that changes the rules for blocks or transactions names the height at which each change
begins. A release that changes no rule, such as a crash fix, names no height. Your node must run a
version that the rule in force at each height accepts. The rule is the `p2p.minimumVersions` list
in `crypto/milestones.json`, and the node checks it for you:

- **At start**, the floor at your stored tip must accept your version. If not, the node will not
  start and names the height. On a first start the tip is height 1.
- **Later heights.** If a later rule change would refuse your version, the node starts and logs a
  warning with the height.
- **The forger** checks at its first round, at the height its relay reports (at most the height
  its own clock allows). If the rule in force refuses it, the forger stops; a later refusal is a
  warning.

To upgrade:

1. Read the announcement and note every activation height it lists.
2. Get the new file: build the release tag, or download it and check its checksum (section 3).
3. Stop the forger, then the relay.
4. Replace the binary and run `heartwood version`.
5. Start the relay and wait until it is in sync.
6. Start the forger.

Upgrade well before the earliest activation height. With failover relays, upgrade them one at a
time and keep the forger pointing at one that runs. If the unit files name the binary's path, keep
the path.

### Recovery commands (danger)

These are for recovery only, and act on the relay. None of them touches the forger's signing
record.

- `heartwood state clear` makes the relay rebuild its state from its stored blocks, from the first
  block, at its next start. `heartwood pool clear` empties the transaction pool at the next start.
  Both ask first; `--force` skips the question.
- `heartwood snapshot rollback --height H` rolls a stopped relay's store back to the last block of
  the round before height `H` (`--number N` counts N blocks down from the tip); the relay syncs
  the rest from its peers.
- The `.env` settings `CORE_RESET_DATABASE` (empties the chain database and the pool at every
  start while it is set) and `CORE_RESET_POOL` (empties the pool at every start) are for a test
  machine. Remove them right after use.

## 19. Backups

Back up:

- The configuration directory, except `delegates.json`: `app.json`, `peers.json`, `.env`, the
  `crypto` files and `config.json`.
- The forger's key material, only in a form you protect.

**Never back up in plain text** `delegates.json`: in this stage it holds the validator account's
key, so a copy can spend the account's funds. Never copy it to cloud storage, email, chat or an
unencrypted disk. Nor any other copy of the account's key, such as a wallet recovery phrase. If you
must keep a copy of the forger's keys, keep it offline, encrypted with a tool you trust, with the
password somewhere else.

The signing record holds no secret, but it must stay with its key. Never restore an old copy of
it: an old record can let the forger sign a block it should refuse, and a missing one is no better,
because the forger starts with an empty one (section 7). Never run one key on two forgers, even
for a short test.

You do not need to back up the relay's database or its logs. The database can be rebuilt by
syncing again from the network, which takes time: a fresh sync is plan B, not a routine.

## 20. When something goes wrong

Start with the diagnostics command. `heartwood doctor` examines your node on this machine,
prints what is wrong and what to do, reads only and never opens a key file. Run it as the node's
user, with the same `--token` and `--network` as your roles. [The doctor](doctor.md) describes
its checks, its report and its exit codes.

```text
heartwood doctor --network <network>
heartwood doctor --network <network> --json
heartwood doctor explain keys.file
```

Its checks for the core role come in a later release. For the rest, by symptom:

**The process will not start.**

- Run `heartwood <role> status`. If it says `errored`, read `heartwood <role> log --error` and the
  node's own log (section 17).
- "The ... file does not exist": `delegates.json` is missing or in the wrong directory. "Unable to
  detect any private keys": the file holds no key.
- "... is open to its group or to others": run `chmod 600` on `delegates.json`.
- "... is already running", or "another relay of this instance holds ...": another copy of that
  role, or the core role, runs for the same token and network. Run `status`, then `stop`.
- "... is below the minimum versions ... in force at height ...": your version is below the floor.
  Install the new release (section 18).
- "cannot read .../.env": create the file in the configuration directory; it may be empty.
- "remoteAccess must admit ...": the core role's forger connects from an address the relay's
  `remoteAccess` does not list.
- The run directory is refused: it must belong to you and must not be a symbolic link.
- "the signing record ... is for the network ...": the record belongs to another network. Check
  `--network` and your data directory. Do not delete the record; ask the team.
- No network named: give `--network`, or name it in `config.json`.

**The relay does not sync, or has no peers.** Check that the P2P port is open and that `peers.json`
has at least one working seed. Check the clock (`timedatectl`, `chronyc tracking`), the free disk
space, and that the database is on a local SSD.

**The forger runs but the relay ignores it.** The relay's `remoteAccess` must include the forger's
address as a plain IP; the forger's `hosts` must be IP addresses with the right port; and a forger
on another machine is not counted as online (section 9).

**The forger will not forge or sign a slot.**

- "has already forged on another node" or "already forged a block. Will not forge": the chain
  already holds a block by this key for the slot. Right after the forger restarts or fails over
  inside a slot it had already signed, this is expected. At any other time another forger may be
  using the same key: stop and find the other copy before you do anything else.
- "will not sign: slot ... is below slot ..., which this delegate has already signed": the
  signing record refuses a slot the key has passed. Never delete the record to make the message go
  away.
- "more than one slot away from this forger's slot", or "ms left in the current slot": check the
  clock first, then the link to the relay and the relay's load.

If you suspect a security problem, do not post it in public: report it privately through GitHub's
private vulnerability reporting for this repository.

## 21. Not built yet

| Item | State |
| --- | --- |
| Install script and packages | Build from source and check the checksum by hand once downloads exist |
| Release signing | None, by design: checksums in two places are the check |
| A command that makes a production forger key | Not built |
| Separate account and block-signing keys | Later protocol stage; today the forging key is the account key |
| Encrypting `delegates.json` at rest | Deferred: use file modes and offline storage |
| Confirmation before forging with a missing or old signing record | Not built; a missing record is created empty |
| A forger on another machine counted as online | Later stage, over an authenticated link |
| `config forger` with a passphrase | Deferred; only the private-key method exists |
| Service units for anything but systemd | None: nodes run on Linux only |
| The doctor's checks for the core role and for the relay's reach | Later release |
| Port numbers | Set per network: take them from the release notes |
| Measured hardware minimums | Not measured; the figures are planning estimates |
