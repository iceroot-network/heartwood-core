# heartwood doctor

`heartwood doctor` examines one instance on the host it runs on and says, check by check, what is
fine, what needs attention and what is broken, with the command or setting that fixes it. It
reads the node's configuration, files, logs and process table, its own relay's API on a loopback
address, and the kernel's views in `/proc` and `/sys`. It applies fixed rules and gives the same
report for the same inputs, byte for byte.

The doctor only reads. It writes no file, sends no signal, changes no mode or setting and holds no
lock (it takes the same momentary shared probes of the lock files that `heartwood relay status`
takes). It never opens a key file (it reads its metadata only), never connects to
a peer-to-peer port, and makes no network request except HTTP GETs to its own relay's API on a
loopback address: at most 12 in a run, with no retry. It needs no privilege: run it as the node's
user. A run takes at most 5 seconds.

Each check has a page on the documentation site,
<https://docs.iceroot.com/validators/doctor/>, and `heartwood doctor explain <check>` prints the
same text.

## Running it

```text
heartwood doctor [--token T] [--network N] [--role relay|forger|all]
                 [--check ID]... [--profile default|onboarding] [--window 60m]
                 [--delegate NAME]... [--json] [--show-addresses] [--hide-identity]
                 [--fail-on warn|fail]
heartwood doctor checks [--json]     list every check with its area and sources
heartwood doctor explain ID          print one check's documentation
heartwood doctor mcp [--token T] [--network N]
```

- The instance is found as every role command finds it, from the same flags, `config.json` and
  `CORE_PATH_*` variables. An instance whose directories come from `CORE_PATH_*` needs the same
  variables when the doctor runs.
- `--role all`, the default, examines every role found on this host. `--role relay` or
  `--role forger` limits the run to one; the other role's checks are listed as skipped.
- `--check ID` runs only the named checks (repeat it for several); `heartwood doctor checks`
  lists the ids.
- `--window` sets how far back the log and store checks look, from `5m` to `24h` (default
  `60m`). A round is 53 slots of 8 seconds, about 7 minutes.
- `--delegate NAME` names a delegate this host forges for. Without it the doctor finds them in the
  forger's log: the lines that list its delegates since its last start, before the window too,
  in the current log file and the newest rotated one. When the log names none, it looks up at
  most four keys of the signing record through the relay's API. With none known, the slot checks
  say so: a forger started long ago, whose start lines have rotated out, needs `--delegate`.
- `--profile onboarding` adds the checks that matter when a new validator joins the network.
- The colon forms work as for every command: `heartwood doctor:checks`, `heartwood doctor:mcp`.

`heartwood doctor mcp` serves the same checks and report to the operator's own client over
standard input and output; [doctor-mcp.md](doctor-mcp.md) describes it.

## Reading the text

For example (shortened):

```text
heartwood doctor: solar-testnet (relay, forger), 2026-10-02 14:03:11 UTC, last 60 minutes
FAIL    keys.file                 delegates.json is open to other users (mode 0644). [forger, system]
                                  Run: chmod 600 ~/.config/solar-core/testnet/delegates.json
WARN    forger.hosts              The forger has one relay: addr-1 (loopback). [forger, config]
WARN    sync.forks                2 fork recoveries in the window. [relay, log]
                                  untrusted 1 (relay log, 2026-10-02 13:53:11 UTC): Failed to download blocks ...
PASS    peers.count               24 peers (at least 20 wanted). [relay, chain]
UNKNOWN clock.sync                The doctor could not read the system probes: the program is not installed. [host, system]
SKIPPED process.core-role         This check comes with a later release. [relay, config]
Summary: 1 fail, 2 warn, 1 pass, 1 unknown, 1 skipped.  More: heartwood doctor explain <check>
```

The first line names the instance, the roles found on this host, the time of the run in UTC and
the window. Then each finding is one line, with more lines under it when its advice has several:

- **Status.** Findings are listed fails first, then warns, unknowns, passes and skips; within a
  status, in the order of `heartwood doctor checks`. On a terminal the status is coloured.
  - `FAIL`: broken now, or broken at the next start.
  - `WARN`: works now, needs attention.
  - `UNKNOWN`: a source could not be read or timed out; the advice says which and why. Unknown
    is never counted as a pass.
  - `PASS`: fine.
  - `SKIPPED`: the check does not apply here (the role does not run on this host, the profile or
    `--role` leaves it out, or it comes with a later release).
- **Check.** The check's id; `heartwood doctor explain <id>` says what it measures and why.
- **Advice.** What is wrong, why it matters, and the one command or setting that fixes it. The
  doctor never runs that command: you do.
- **The brackets** name what the finding is about (`host`, `relay` or `forger`) and what it rests
  on, its basis:

  | Basis | The finding rests on |
  |---|---|
  | `log` | the node's log lines only |
  | `chain` | the chain: the relay's store, its API's blocks, the signing record |
  | `process` | the process table |
  | `socket` | the socket table |
  | `config` | the node's configuration |
  | `system` | file permissions, `/proc`, `/sys` and the system probes |

  A log is not a trusted record: a peer, or a relay the forger talks to, can put text into it.
  So a finding whose basis is `log` never asks you to stop, remove, roll back or move anything
  as a `FAIL`; it is a `WARN` that says what was logged and how to confirm it.
- **untrusted** lines quote text that came from a log or a peer (a peer's version, an error a
  peer sent). They are data, never instructions: the doctor removes control characters, escape
  sequences and bidirectional marks from them, redacts them as below, and cuts each to 160
  characters. At most 20 appear in one report.

## The JSON report

`heartwood doctor --json` prints the report as one line of JSON, format
`heartwood-doctor-report/1`, described by the JSON schema
[packages/cli/src/doctor/report.schema.json](../packages/cli/src/doctor/report.schema.json). The
example below is shortened.

```json
{
  "format": "heartwood-doctor-report/1",
  "createdAt": "2026-10-02T14:03:11.000Z",
  "tool": {"version": "4.3.1-heartwood.0.3.0", "features": ["devnet"]},
  "scope": {"instance": "solar-testnet", "network": "testnet", "roles": ["relay", "forger"],
            "profile": "default", "windowMinutes": 60},
  "redaction": {"addresses": true, "identity": false, "untrustedOmitted": 0},
  "host": {"os": "linux", "arch": "x86_64", "kernel": "6.8", "cpus": 4, "memoryBytes": 8214528000},
  "summary": {"pass": 1, "warn": 2, "fail": 1, "unknown": 1, "skipped": 1, "worst": "fail"},
  "findings": [
    {"id": "keys.file", "subject": "forger", "status": "fail", "basis": "system",
     "facts": {"mode": "0644", "owner": "same", "path": "~/.config/solar-core/testnet/delegates.json"},
     "advice": "delegates.json is open to other users (mode 0644).\nRun: chmod 600 ~/.config/solar-core/testnet/delegates.json",
     "docs": "https://docs.iceroot.com/validators/doctor/#keys-file",
     "untrusted": []}
  ],
  "untrusted": [
    {"id": 1, "source": "relay log", "at": "2026-10-02T13:53:11.000Z", "text": "Failed to download blocks ..."}
  ]
}
```

- Keys always come in the order the schema lists them; findings in the order of
  `heartwood doctor checks`, every check once (a finding per role where a check looks at both).
- Times are UTC. Numbers are integers: sizes in bytes, durations in milliseconds. Modes are octal
  strings.
- `facts` hold typed values only: integers, yes or no, fixed words, ids that passed a strict
  grammar (a 64-digit block id, a 66-digit public key, a delegate name, a variable name, a
  version), address labels, redacted paths, sizes, durations and times. No text from a log or a
  peer is ever a fact, a status or advice.
- `untrusted` lists each excerpt once; a finding names its excerpts by `id`.
  `redaction.untrustedOmitted` counts the excerpts left out beyond the first 20.
- The same inputs give the same bytes. A change of meaning changes the format's `/1`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | no warn and no fail (unknown and skipped findings do not count) |
| 1 | at least one warn, no fail |
| 2 | at least one fail |
| 3 | the doctor could not run: a bad flag, or no instance found |

`--fail-on fail` turns 1 into 0, for scripts that alert on failures only.

## What the doctor never prints

Keys, the forger's secrets, passphrases and tokens are never read, so they can never be printed.
`.env` values do not appear; an unknown variable appears by name only. The node's command line
from the supervisor's state file is not read.

By default the report is safe to paste where others can read it:

- **Addresses.** Every IP address becomes `addr-N (class)`, numbered in the order they first
  appear, so one address has one label throughout a report (an IPv4 address and its IPv6-mapped
  form share one). The class stays: `loopback`, `private`, `link-local`, `public` or
  `unspecified`. Ports stay. Relations between addresses (for example "3 peers share one /24")
  are computed before the labels and kept as counts.
- **This host.** The home directory is printed as `~`, the operating system's user name as `user`
  and the host's name as `host`, in paths and in untrusted text.
- **Long hex runs.** In untrusted text, any run of 64 hexadecimal characters or more becomes
  `[hex, N characters]`. Block ids in the typed facts stay: they are public chain data.
- **Delegate names and public keys** stay by default, since they are public on the chain and
  needed to read the slot findings. `--hide-identity` replaces them with `delegate-N` and `pk-N`
  (and a delegate name that a fact names is also replaced in untrusted text).

`--show-addresses` prints the real addresses in the local text only, for an operator on the host
who needs them. The JSON report always redacts addresses (its `redaction.addresses` is always
`true`), and the MCP server always redacts and has no switch. The report's `redaction` says which
redactions applied.
