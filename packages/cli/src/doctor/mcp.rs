//! `heartwood doctor mcp`: a Model Context Protocol server over standard input and output for the
//! operator's own client, which starts it over SSH. Read-only: five tools (`list_checks`,
//! `run_checks`, `get_report`, `read_log_summary`, `explain_check`) that run the same checks and
//! read the same report, redacted log summaries and check documentation. No listener, no tool that
//! writes or runs anything, no key access; untrusted text only in fields named `untrusted`.
//!
//! The protocol revisions it serves are [`REVISIONS`]. The current revision is stateless: every
//! request names its revision and the client's capabilities in `_meta` (without them: -32602), the
//! server answers `server/discover`, a request of another revision gets -32022 with the supported
//! list, and every result carries `resultType`. The one before it opens with `initialize`, which
//! selects that revision's rules for the rest of the process (`ping` is answered even before it).
//! One JSON-RPC message per line, one request at a time, in order; nothing but protocol on
//! standard output; a batch refused; a message over [`MAX_MESSAGE`] bytes refused and the session
//! ended; a notification never answered. The server installs no signal handler, so SIGTERM and
//! SIGHUP end it, and it returns at the end of its input.
//!
//! The tools' names, descriptions and schemas are data (`mcp.json`); every input schema is closed
//! and is checked before anything runs. A fresh run of the checks happens at most every [`CACHE`],
//! and so does a fresh read of each role's log. At most [`MAX_SERVERS`] servers of one user run.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::sync::LazyLock;
use std::time::Duration;

use heartwood_kernel::timestamp::UtcDateTime;
use serde_json::{Map, Value, json};

use crate::doctor::docs::{self, DocsError};
use crate::doctor::redact::{MAX_EXCERPTS, Redactor};
use crate::doctor::registry::{CheckId, ENTRIES};
use crate::doctor::render::render_json;
use crate::doctor::report::HostIdentity;
use crate::doctor::seams::{ProcFile, ProcView, World};
use crate::doctor::sources::logs::LineKind;
use crate::doctor::sources::procfs::parse_status;
use crate::doctor::view::{Gathered, View};
use crate::doctor::{DoctorOptions, DoctorProfile, DoctorScope, RoleSelection};
use crate::seams::Role;

/// The protocol revisions served, newest first: the stateless revision, then the last revision
/// with the `initialize` handshake.
pub const REVISIONS: [&str; 2] = ["2026-07-28", "2025-11-25"];
/// The longest message read; a longer one ends the session.
pub const MAX_MESSAGE: usize = 1024 * 1024;
/// How long a report is served from the cache before `run_checks` runs again.
pub const CACHE: Duration = Duration::from_secs(30);
/// The most `heartwood doctor mcp` servers one user runs at once.
pub const MAX_SERVERS: usize = 2;
/// The server's name in its answers.
pub const SERVER_NAME: &str = "heartwood-doctor";

const VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const INVALID_REQUEST: i64 = -32600;
const INVALID_PARAMS: i64 = -32602;
const TOO_LARGE: &str = "Message longer than 1 MiB; the session ends";
const MISSING_META: &str = "Every request carries _meta with the protocol version and the \
                            client's capabilities, unless the session opened with initialize";

/// Why the server stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// The tool definitions compiled in do not read (a defect of this build).
    #[error("the MCP server's tool definitions are not valid")]
    Definitions,
    /// The check documentation compiled in does not read (a defect of this build).
    #[error(transparent)]
    Docs(DocsError),
    /// Standard input or output failed.
    #[error("the session's input or output failed: {0:?}")]
    Io(std::io::ErrorKind),
    /// A message was longer than [`MAX_MESSAGE`].
    #[error("a message was longer than {MAX_MESSAGE} bytes")]
    TooLarge,
    /// [`MAX_SERVERS`] servers of this user already run.
    #[error("{MAX_SERVERS} doctor servers of this user already run")]
    TooManyServers,
}

/// A refused request: its JSON-RPC error code, message and data.
type Refusal = (i64, &'static str, Option<Value>);
/// A value kept for [`CACHE`], its age in whole seconds, and whether it was made just now.
type Aged = (Value, u64, bool);

/// The instructions and the tools of `mcp.json`, with the check ids and the annotations filled in.
static TOOLS: LazyLock<Option<(String, Vec<Value>)>> = LazyLock::new(|| {
    let ids: Value = ENTRIES.iter().map(|entry| entry.id).collect();
    let text = include_str!("mcp.json").replace("\"CHECK_IDS\"", &ids.to_string());
    let value: Value = serde_json::from_str(&text).ok()?;
    let annotations = json!({"readOnlyHint": true, "destructiveHint": false,
        "idempotentHint": true, "openWorldHint": false});
    let mut tools = value.get("tools")?.as_array()?.clone();
    for tool in &mut tools {
        let tool = tool.as_object_mut()?;
        tool.insert("annotations".into(), annotations.clone());
    }
    Some((value.get("instructions")?.as_str()?.to_owned(), tools))
});

/// Serve the session on `input` and `output` until end of input, over `world` for the instance
/// `scope` names.
pub fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    world: &dyn World,
    scope: &DoctorScope,
) -> Result<(), McpError> {
    let (instructions, tools) = TOOLS.as_ref().ok_or(McpError::Definitions)?;
    docs::docs().map_err(McpError::Docs)?;
    if servers(world.procfs()) > MAX_SERVERS {
        return Err(McpError::TooManyServers);
    }
    let mut session = Session {
        world,
        scope,
        instructions,
        tools,
        legacy: false,
        cache: BTreeMap::new(),
    };
    let limit = u64::try_from(MAX_MESSAGE).map_or(u64::MAX, |max| max.saturating_add(2));
    loop {
        let mut line = Vec::new();
        let read = input.by_ref().take(limit).read_until(b'\n', &mut line);
        if read.map_err(|error| McpError::Io(error.kind()))? == 0 {
            return Ok(());
        }
        let body = line.trim_ascii_end();
        if body.len() > MAX_MESSAGE {
            let refusal = (INVALID_REQUEST, TOO_LARGE, None);
            send(&mut output, &failure(Value::Null, refusal))?;
            return Err(McpError::TooLarge);
        }
        if let Some(answer) = session.message(body) {
            send(&mut output, &answer)?;
        }
    }
}

/// `message` on one line of `output`, flushed.
fn send(output: &mut impl Write, message: &Value) -> Result<(), McpError> {
    let sent = writeln!(output, "{message}").and_then(|()| output.flush());
    sent.map_err(|error| McpError::Io(error.kind()))
}

fn failure(id: Value, (code, message, data): Refusal) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let (Some(data), Some(error)) = (data, error.as_object_mut()) {
        error.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// The running `heartwood doctor mcp` processes of this process's user, this one included; 0 when
/// `/proc` does not tell.
fn servers(procfs: &dyn ProcView) -> usize {
    let uid = |file| Some(parse_status(&procfs.read(file).ok()?).ok()?.uid);
    let own = uid(ProcFile::SelfStatus);
    let pids = procfs.pids().unwrap_or_default().into_iter();
    let ours = pids.filter(|pid| own.is_some() && uid(ProcFile::PidStatus(*pid)) == own);
    let argv = ours.filter_map(|pid| procfs.read(ProcFile::PidCmdline(pid)).ok());
    argv.filter(|argv| is_server(argv)).count()
}

/// Whether the command line `argv` (NUL-separated) is a `heartwood doctor mcp`.
fn is_server(argv: &[u8]) -> bool {
    let words: Vec<&[u8]> = argv.split(|byte| *byte == 0).collect();
    let program = words.first().copied().unwrap_or_default();
    let name = program.rsplit(|byte| *byte == b'/').next();
    let pair = [b"doctor".as_slice(), b"mcp"];
    name.is_some_and(|name| name.starts_with(b"heartwood"))
        && (words.windows(2).any(|w| w == pair) || words.contains(&b"doctor:mcp".as_slice()))
}

/// One session: its era and what it keeps for [`CACHE`].
struct Session<'a> {
    world: &'a dyn World,
    scope: &'a DoctorScope,
    instructions: &'static str,
    tools: &'static [Value],
    /// Whether the client opened with `initialize`.
    legacy: bool,
    /// The last report (`report`) and log summary of each role (by name), with when each was made.
    cache: BTreeMap<&'static str, (i64, Value)>,
}

impl Session<'_> {
    /// The answer to one message; `None` for a notification or a response.
    fn message(&mut self, body: &[u8]) -> Option<Value> {
        let refuse = |code, text| Some(failure(Value::Null, (code, text, None)));
        let message = match serde_json::from_slice::<Value>(body) {
            _ if body.trim_ascii().is_empty() => return None,
            Ok(Value::Object(message)) => message,
            Ok(Value::Array(_)) => return refuse(INVALID_REQUEST, "Batches are not supported"),
            Ok(_) => return refuse(INVALID_REQUEST, "Invalid request"),
            Err(_) => return refuse(-32700, "Parse error"),
        };
        let (id, method) = (message.get("id")?, message.get("method")?.as_str());
        let id = Some(id).filter(|id| id.is_string() || id.is_i64() || id.is_u64());
        let version = message.get("jsonrpc").and_then(Value::as_str);
        let (Some(id), Some(method), Some("2.0")) = (id.cloned(), method, version) else {
            let refusal = (INVALID_REQUEST, "Invalid request", None);
            return Some(failure(id.cloned().unwrap_or_default(), refusal));
        };
        let empty = Map::new();
        let result = match message.get("params") {
            None => self.request(method, &empty),
            Some(Value::Object(params)) => self.request(method, params),
            Some(_) => Err((INVALID_PARAMS, "Invalid params", None)),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(refusal) => failure(id, refusal),
        })
    }

    fn request(&mut self, method: &str, params: &Map<String, Value>) -> Result<Value, Refusal> {
        let capabilities = json!({"tools": {"listChanged": false}});
        let server = json!({"name": SERVER_NAME, "version": self.world.node().build().version});
        match method {
            "initialize" => {
                self.legacy = true;
                let result = json!({"protocolVersion": REVISIONS[1], "capabilities": capabilities,
                    "serverInfo": server, "instructions": self.instructions});
                return Ok(result);
            }
            "ping" => return Ok(json!({})),
            "server/discover" | "tools/list" | "tools/call" => {}
            _ => return Err((-32601, "Method not found", None)),
        }
        let meta = params.get("_meta");
        let declared = meta.and_then(|meta| meta.get(CAPABILITIES));
        let modern = match meta.and_then(|meta| meta.get(VERSION)).map(Value::as_str) {
            None if self.legacy => false,
            Some(Some(asked)) if !REVISIONS.contains(&asked) => {
                let data = json!({"supported": REVISIONS, "requested": asked});
                return Err((-32022, "Unsupported protocol version", Some(data)));
            }
            Some(Some(_)) if declared.is_some_and(Value::is_object) => true,
            _ => return Err((INVALID_PARAMS, MISSING_META, None)),
        };
        let mut result = match method {
            "server/discover" => json!({"supportedVersions": REVISIONS,
                "capabilities": capabilities, "instructions": self.instructions}),
            "tools/list" => json!({"tools": self.tools}),
            _ => self.call(params)?,
        };
        if let (true, Some(fields)) = (modern, result.as_object_mut()) {
            fields.insert("resultType".into(), "complete".into());
            fields.insert("_meta".into(), json!({ SERVER_INFO: server }));
            if method != "tools/call" {
                fields.insert("ttlMs".into(), 3_600_000.into());
                fields.insert("cacheScope".into(), "private".into());
            }
        }
        Ok(result)
    }

    /// `tools/call`: an unknown tool or a malformed call is refused; arguments outside the tool's
    /// input schema give a tool error before anything runs. The result is `structuredContent` and
    /// the same JSON as one text item.
    fn call(&mut self, params: &Map<String, Value>) -> Result<Value, Refusal> {
        let name = params.get("name").and_then(Value::as_str);
        let name = name.ok_or((INVALID_PARAMS, "Invalid params", None))?;
        let tool = self.tools.iter().find(|tool| tool["name"] == name);
        let tool = tool.ok_or((INVALID_PARAMS, "Unknown tool", None))?;
        let empty = Map::new();
        let args = match params.get("arguments") {
            None => &empty,
            Some(Value::Object(args)) => args,
            Some(_) => return Err((INVALID_PARAMS, "Invalid params", None)),
        };
        if !fits(&tool["inputSchema"], &Value::Object(args.clone())) {
            let text = "Refused before anything ran: the arguments do not fit the input schema.";
            return Ok(json!({"content": [{"type": "text", "text": text}], "isError": true}));
        }
        let options = options(args);
        let role = match args.get("role").and_then(Value::as_str) {
            Some("forger") => Role::Forger,
            _ => Role::Relay,
        };
        let mut docs = docs::docs().into_iter().flat_map(|docs| &docs.checks);
        let value = match name {
            "list_checks" => {
                let checks = docs.map(|doc| {
                    json!({"id": doc.id, "title": doc.title, "area": doc.area,
                        "subjects": doc.subjects, "sources": doc.sources, "docs": doc.docs})
                });
                json!({"checks": checks.collect::<Vec<_>>()})
            }
            "run_checks" => {
                let (report, age, fresh) = self.cached("report", |session| {
                    let report = crate::doctor::run(session.world, session.scope, &options);
                    serde_json::from_str(&render_json(&report)).unwrap_or_default()
                });
                let next = CACHE.as_secs().saturating_sub(age);
                json!({"report": report, "fresh": fresh, "cachedForSeconds": age,
                    "nextRunInSeconds": next})
            }
            "get_report" => {
                let (made, report) = self.cache.get("report").cloned().unzip();
                let age = made.map(|made| self.world.clock().now_ms().saturating_sub(made) / 1000);
                json!({"report": report, "cachedForSeconds": age.map(|age| age.max(0))})
            }
            "read_log_summary" => {
                let (mut summary, age, _) = self.cached(role.name(), |session| {
                    log_summary(&Gathered::new(session.world, session.scope, &options), role)
                });
                if let Some(fields) = summary.as_object_mut() {
                    fields.insert("cachedForSeconds".into(), age.into());
                }
                summary
            }
            _ => {
                let id = args.get("id").and_then(Value::as_str);
                let doc = docs.find(|doc| Some(doc.id.as_str()) == id);
                let doc = doc.ok_or((INVALID_PARAMS, "Unknown check", None))?;
                json!({"id": doc.id, "title": doc.title, "area": doc.area,
                    "subjects": doc.subjects, "sources": doc.sources, "rules": doc.rules,
                    "advice": doc.advice, "docs": doc.docs})
            }
        };
        let content = json!([{"type": "text", "text": value.to_string()}]);
        Ok(json!({"content": content, "structuredContent": value, "isError": false}))
    }

    /// What `key` held when it was made within [`CACHE`], else what `make` gives now, which is
    /// kept; with its age in whole seconds and whether it is fresh.
    fn cached(&mut self, key: &'static str, make: impl FnOnce(&Self) -> Value) -> Aged {
        let now = self.world.clock().now_ms();
        if let Some((made, value)) = self.cache.get(key)
            && let Some(elapsed) = now.checked_sub(*made).and_then(|ms| u64::try_from(ms).ok())
            && elapsed < u64::try_from(CACHE.as_millis()).unwrap_or(u64::MAX)
        {
            return (value.clone(), elapsed / 1000, false);
        }
        let value = make(self);
        self.cache.insert(key, (now, value.clone()));
        (value, 0, true)
    }
}

/// The run's options from `run_checks`'s (or `read_log_summary`'s) checked arguments.
fn options(args: &Map<String, Value>) -> DoctorOptions {
    let text = |key| args.get(key).and_then(Value::as_str);
    let ids = args.get("checks").and_then(Value::as_array);
    let ids = ids.into_iter().flatten().filter_map(Value::as_str);
    let minutes = args.get("window_minutes").and_then(Value::as_u64);
    DoctorOptions {
        roles: match text("roles") {
            Some("relay") => RoleSelection::Relay,
            Some("forger") => RoleSelection::Forger,
            _ => RoleSelection::All,
        },
        checks: ids.filter_map(CheckId::parse).collect(),
        profile: match text("profile") {
            Some("onboarding") => DoctorProfile::Onboarding,
            _ => DoctorProfile::Default,
        },
        window: Duration::from_secs(minutes.unwrap_or(60).saturating_mul(60)),
        ..DoctorOptions::default()
    }
}

/// Whether `value` fits `schema`, the closed subset the tools use: an object of known properties
/// with every required one present, a string of an enum, a bounded integer, or a bounded list of
/// distinct such strings.
fn fits(schema: &Value, value: &Value) -> bool {
    let bound = |key: &str, or: u64| schema[key].as_u64().unwrap_or(or);
    let (least, most) = (bound("minimum", 0), bound("maximum", u64::MAX));
    let (properties, enums) = (&schema["properties"], schema["enum"].as_array());
    let mut required = schema["required"].as_array().into_iter().flatten();
    match (schema["type"].as_str(), value) {
        (Some("object"), Value::Object(args)) => {
            let known = |(key, value)| properties.get(key).is_some_and(|p| fits(p, value));
            args.iter().all(known)
                && required.all(|key| key.as_str().is_some_and(|k| args.contains_key(k)))
        }
        (Some("integer"), _) => value.as_u64().is_some_and(|n| (least..=most).contains(&n)),
        (Some("string"), Value::String(_)) => enums.is_none_or(|values| values.contains(value)),
        (Some("array"), Value::Array(items)) => {
            let unique = |(at, item): (usize, &Value)| !items.iter().take(at).any(|x| x == item);
            u64::try_from(items.len()).is_ok_and(|len| len <= bound("maxItems", u64::MAX))
                && items.iter().all(|item| fits(&schema["items"], item))
                && (schema["uniqueItems"] != true || items.iter().enumerate().all(unique))
        }
        _ => false,
    }
}

/// The summary `read_log_summary` gives of `role`'s log in `view`'s window: for each known line
/// kind and level, the count and the first and last time; the counts of other lines and of lines
/// not trusted for their time; and at most [`MAX_EXCERPTS`] excerpts, cleaned, redacted and capped
/// as the report's untrusted text is, in `untrusted`. A log that cannot be read gives `unknown`.
pub fn log_summary(view: &dyn View, role: Role) -> Value {
    let minutes = view.window().to_ms.saturating_sub(view.window().from_ms) / 60_000;
    let log = match view.log(role) {
        Ok(log) => log,
        Err(unknown) => {
            let (source, reason) = (unknown.source.name(), unknown.reason.name());
            return json!({"role": role.name(), "windowMinutes": minutes, "kinds": [],
                "unknown": {"source": source, "reason": reason}, "otherLines": 0, "untrusted": []});
        }
    };
    let mut kinds: BTreeMap<(String, &str), (u64, i64, i64)> = BTreeMap::new();
    for event in &log.events {
        let key = (kind_name(&event.kind), event.level.name());
        let seen = kinds.entry(key).or_insert((0, event.at_ms, event.at_ms));
        (seen.0, seen.1) = (seen.0.saturating_add(1), seen.1.min(event.at_ms));
        seen.2 = seen.2.max(event.at_ms);
    }
    let mut list = Vec::new();
    for ((kind, level), (count, first, last)) in kinds {
        let (first, last) = (time(first), time(last));
        let kind =
            json!({"kind": kind, "level": level, "count": count, "first": first, "last": last});
        list.push(kind);
    }
    let identity = HostIdentity {
        home: view.scope().home.clone(),
        user: view.scope().user.clone(),
        host_name: view.host_name().ok().map(str::to_owned),
    };
    let mut redactor = Redactor::new(&identity, true);
    let excerpts = log.excerpts.iter().take(MAX_EXCERPTS).zip(1_u32..);
    let untrusted = excerpts.map(|(excerpt, id)| {
        json!({"id": id, "source": format!("{} log", role.name()),
            "at": excerpt.at_ms.map(time), "text": redactor.untrusted(&excerpt.text)})
    });
    json!({"role": role.name(), "windowMinutes": minutes, "unknown": null, "kinds": list,
        "otherLines": log.other_lines, "untrustedLines": log.untrusted_lines,
        "whole": log.covered.whole, "capped": log.capped,
        "untrusted": untrusted.collect::<Vec<_>>()})
}

/// A line kind's name: its variant's name in kebab case, such as `fork-detected`.
fn kind_name(kind: &LineKind) -> String {
    let (debug, mut name) = (format!("{kind:?}"), String::new());
    for c in debug.chars().take_while(char::is_ascii_alphanumeric) {
        if c.is_ascii_uppercase() && !name.is_empty() {
            name.push('-');
        }
        name.push(c.to_ascii_lowercase());
    }
    name
}

/// A time as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn time(unix_ms: i64) -> String {
    UtcDateTime::from_unix_ms(unix_ms).map_or_else(String::new, |time| time.to_iso_string())
}
