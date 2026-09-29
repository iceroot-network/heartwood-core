//! The line templates: each known line kind as the ordered literal pieces of the log call that
//! writes it, its level and its trailing emoji. A line matches only its whole template, its level
//! and its emoji; the message is matched before the emoji. Several messages are composed in the
//! node's source (the quorum refusals are `"{refusal}. Will not forge"`), so a template is never
//! one literal sentence. The drift guard finds every template's pieces, in order, in one log call
//! of the node's source files.
//!
//! A template's pieces are literal text with a capture between each two: the first piece is a
//! prefix of the message, the last a suffix (an empty last piece leaves a trailing capture), and
//! the pieces between are found leftmost, in order. Each capture is typed by its kind; a capture
//! that does not pass its grammar makes the line "other". Templates with the same role and level
//! are tried in order, so a specific template comes before a general one.

use std::net::{IpAddr, SocketAddr};

use heartwood_kernel::log::Level::{self, Debug, Error, Info, Warning};

use crate::doctor::report::{Id, IdKind};
use crate::doctor::sources::logs::{LineKind, StartError};
use crate::seams::Role::{self, Forger, Relay};

/// One template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    /// The kind's name, as the MCP log summary lists it.
    pub kind: &'static str,
    /// The role whose log holds it.
    pub role: Role,
    /// The level it is logged at.
    pub level: Level,
    /// The literal pieces, in order.
    pub pieces: &'static [&'static str],
    /// The trailing emoji, when the line has one.
    pub emoji: Option<&'static str>,
    /// The node's source file that writes it, relative to the repository's root.
    pub source: &'static str,
}

/// A template.
const fn t(
    kind: &'static str,
    role: Role,
    level: Level,
    pieces: &'static [&'static str],
    emoji: Option<&'static str>,
    source: &'static str,
) -> Template {
    Template {
        kind,
        role,
        level,
        pieces,
        emoji,
        source,
    }
}

/// Every template.
#[rustfmt::skip]
pub const TEMPLATES: &[Template] = &[
    t("RelayStarted", Relay, Info, &["Heartwood Core ", ": a relay of ", " on ", ""], None, "packages/core/src/app/relay.rs"),
    t("InSync", Relay, Info, &["Blockchain 100% in sync"], Some("💯"), "packages/blockchain/src/state_machine/actions/sync.rs"),
    t("ForkRecoveryStarted", Relay, Info, &["Starting fork recovery"], Some("🍴"), "packages/blockchain/src/fork_recovery.rs"),
    t("ForkDetected", Relay, Warning, &["Fork detected - rolling back ", " to fork at height ", " (", ")"], Some("🔁"), "packages/p2p/src/monitor.rs"),
    t("PeersRefreshedAfterFork", Relay, Info, &["Refreshing ", " after fork"], Some("🍴"), "packages/p2p/src/monitor.rs"),
    t("BlocksRemoved", Relay, Info, &["Removing ", ". Reset to height ", ""], Some("⚠️"), "packages/blockchain/src/node.rs"),
    t("DownloadedBlockNotAccepted", Relay, Warning, &["Downloaded block not accepted"], Some("⚠️ ⚠️ ⚠️"), "packages/blockchain/src/state_machine/actions/sync.rs"),
    t("NoBlocksFromAnyPeer", Relay, Info, &["Could not download any blocks from any peer from height ", ""], Some("⚠️"), "packages/blockchain/src/state_machine/actions/sync.rs"),
    t("NoPeersToDownload", Relay, Error, &["Could not download blocks: we have 0 peers"], Some("‼️"), "packages/p2p/src/monitor.rs"),
    t("DownloadFailed", Relay, Info, &["Failed to download blocks ", " from ", ": ", ""], Some("‼️"), "packages/p2p/src/monitor.rs"),
    t("DownloadFailed", Relay, Info, &["Giving up on trying to download blocks ", ": ", ""], Some("‼️"), "packages/p2p/src/monitor.rs"),
    t("FallingBackToSeeds", Relay, Info, &["Couldn't find enough peers. Falling back to seed peers"], Some("🔥"), "packages/p2p/src/monitor.rs"),
    t("NotEnoughPeersForHealth", Relay, Info, &["Not enough peers available to check network health"], Some("🚫"), "packages/p2p/src/monitor.rs"),
    t("NoVerifiedPeersForHealth", Relay, Info, &["No verified peers available to check network health"], Some("🚫"), "packages/p2p/src/monitor.rs"),
    t("MedianNetworkHeight", Relay, Info, &["Median Network Height: ", ""], None, "packages/p2p/src/monitor.rs"),
    t("NetworkMissingBlocks", Relay, Info, &["Network keeps missing blocks"], Some("☂️"), "packages/blockchain/src/state_machine/actions/sync.rs"),
    t("NetworkMissingBlocks", Relay, Info, &["Tried to sync 3 times to different nodes, looks like the network is missing blocks"], Some("☂️"), "packages/blockchain/src/state_machine/actions/sync.rs"),
    t("DiscardedFutureSlot", Relay, Info, &["Discarded block ", " because it takes a future slot"], Some("❗"), "packages/blockchain/src/blockchain.rs"),
    t("DiscardedFutureSlot", Relay, Error, &["Discarded block ", " because it takes a future slot"], Some("🔮"), "packages/blockchain/src/process_blocks_job.rs"),
    t("DiscardedReceivedLate", Relay, Info, &["Discarded block ", " because it was received too late"], Some("❗"), "packages/blockchain/src/forger_blocks.rs"),
    t("DiscardedProcessedLate", Relay, Info, &["Discarded block ", " because it was processed too late"], Some("❗"), "packages/blockchain/src/forger_blocks.rs"),
    t("StateCorrupted", Relay, Error, &["Shutting down app, because state is corrupted"], Some("💥"), "packages/blockchain/src/process_blocks_job.rs"),
    t("StateMightBeCorrupted", Relay, Warning, &["Shutting down app, because state might be corrupted"], Some("💥"), "packages/blockchain/src/blockchain.rs"),
    t("NodeStops", Relay, Error, &["The node stops: ", ""], None, "packages/blockchain/src/node.rs"),
    t("DoubleForgingDetected", Relay, Warning, &["Detected double forging by ", ""], Some("⛓️"), "packages/blockchain/src/processor/unchained.rs"),
    t("AttestationSessionReplaced", Relay, Warning, &["A new attestation session from ", " replaces the session from ", ""], None, "packages/p2p/src/attestation_session.rs"),
    t("AttestationKeysRepeated", Relay, Warning, &["The attestation session from ", " declares ", " keys again; each copy counts as a delegate on this node"], None, "packages/p2p/src/attestation_session.rs"),
    t("BlockReceived", Relay, Info, &["Received new block forged by delegate ", " (#", ") at height ", " with ", " reward"], Some("📦"), "packages/p2p/src/server/controllers/blocks.rs"),
    t("RemoteAccessNoMatch", Relay, Warning, &["The remoteAccess entry ", " matches no address: it admits no forger"], None, "packages/p2p/src/server/controllers/mod.rs"),
    t("RemoteAccessEmpty", Relay, Warning, &["remoteAccess admits no address: no forger can reach this relay's internal routes"], None, "packages/p2p/src/server/controllers/mod.rs"),
    t("RemoteAccessEmpty", Relay, Warning, &["remoteAccess is empty: no forger can reach this relay's internal routes"], None, "packages/core/src/app/relay.rs"),
    t("ApiDisabled", Relay, Info, &["The API is disabled"], None, "packages/core/src/app/relay.rs"),
    t("StartErrorP2pCannotListen", Relay, Error, &["the P2P server cannot listen on ", ": ", ""], None, "packages/p2p/src/server/mod.rs"),
    t("StartErrorApiCannotListen", Relay, Error, &["the API cannot listen on ", ": ", ""], None, "packages/api/src/errors.rs"),
    t("StartErrorApiHttps", Relay, Error, &["CORE_API_SSL is set, but this relay serves no HTTPS: serve its API through a reverse proxy that terminates TLS"], None, "packages/core/src/app/relay.rs"),
    t("StartErrorApiHttps", Relay, Error, &["the API does not serve HTTPS (CORE_API_SSL is set): serve it behind a reverse proxy that terminates TLS"], None, "packages/api/src/errors.rs"),
    t("StartErrorVersionFloor", Relay, Error, &["this node's version ", " is below the minimum versions ", " in force at height ", ""], None, "packages/kernel/src/config/floor.rs"),
    t("StartErrorVersionFloor", Forger, Error, &["this node's version ", " is below the minimum versions ", " in force at height ", ""], None, "packages/kernel/src/config/floor.rs"),
    t("ApiNotStarted", Relay, Error, &["", ": the API is not started"], None, "packages/core/src/app/relay.rs"),
    t("FloorLaterRefuses", Relay, Warning, &["From height ", " the minimum versions are ", ", which this node's version ", " does not satisfy"], None, "packages/core/src/app.rs"),
    t("FloorLaterRefuses", Forger, Warning, &["From height ", " the minimum versions are ", ", which this node's version ", " does not satisfy"], None, "packages/core/src/app.rs"),
    t("ForgerStarted", Forger, Info, &["Heartwood Core ", ": a forger of ", " on ", ""], None, "packages/core/src/app/forger.rs"),
    t("ForgerManagerStarted", Forger, Info, &["Forger Manager started"], Some("🔨"), "packages/forger/src/forger_service.rs"),
    t("DelegatesLoaded", Forger, Info, &["Loaded ", ": ", ""], None, "packages/forger/src/forger_service.rs"),
    t("ActivePositionEntered", Forger, Info, &["Delegate ", " is now in an active forging position"], Some("🎉"), "packages/forger/src/forger_service.rs"),
    t("ActivePositionEntered", Forger, Info, &["Delegates ", " are now in active forging positions"], Some("🎉"), "packages/forger/src/forger_service.rs"),
    t("ActivePositionLeft", Forger, Info, &["Delegate ", " is no longer in an active forging position"], Some("😢"), "packages/forger/src/forger_service.rs"),
    t("ActivePositionLeft", Forger, Info, &["Delegates ", " are no longer in active forging positions"], Some("😢"), "packages/forger/src/forger_service.rs"),
    t("DueToForge", Forger, Info, &["Delegate ", " is due to forge in ", " from now"], Some("✨"), "packages/forger/src/forger_service.rs"),
    t("Forged", Forger, Info, &["Delegate ", " forged a new block at height ", " with ", ""], Some("🔱"), "packages/forger/src/forger_service.rs"),
    t("SendsAgain", Forger, Info, &["Delegate ", " sends block ", " at height ", " of slot ", " again"], None, "packages/forger/src/signer.rs"),
    t("WillNotSign", Forger, Warning, &["Delegate ", " will not sign: ", ""], None, "packages/forger/src/signer.rs"),
    t("ForgedOnAnotherNode", Forger, Warning, &["Not going to forge because delegate ", " has already forged on another node"], Some("🛑"), "packages/forger/src/forger_service.rs"),
    t("AlreadyForgedThisRound", Forger, Warning, &["Delegate ", " already forged a block. Will not forge"], Some("💣"), "packages/forger/src/forger_service.rs"),
    t("NotEnoughQuorum", Forger, Info, &["Not enough quorum to forge next block. Will not forge"], Some("💣"), "packages/forger/src/forger_service.rs"),
    t("NotPeeredEnough", Forger, Info, &["Not peered with enough delegates to get quorum", ". Will not forge"], Some("💣"), "packages/forger/src/forger_service.rs"),
    t("ColdStartSkip", Forger, Info, &["Skipping slot because of cold start", ". Will not forge"], Some("💣"), "packages/forger/src/forger_service.rs"),
    t("NoNetworkState", Forger, Info, &["Failed to get network state from client", ". Will not forge"], Some("💣"), "packages/forger/src/forger_service.rs"),
    t("OverheightHeaders", Forger, Info, &["Detected ", " distinct overheight block header", ""], Some("💤"), "packages/forger/src/forger_service.rs"),
    t("HeightsOutOfSync", Forger, Warning, &["The NetworkState height (", ") and round height (", ") are out of sync. This indicates delayed blocks on the network"], Some("💤"), "packages/forger/src/forger_service.rs"),
    t("TooLittleTime", Forger, Warning, &["Failed to forge new block by delegate ", ", because there were ", "ms left in the current slot (less than ", "ms)"], Some("‼️"), "packages/forger/src/forger_service.rs"),
    t("AlreadyNextSlot", Forger, Warning, &["Failed to forge new block by delegate ", ", because already in next slot"], Some("‼️"), "packages/forger/src/forger_service.rs"),
    t("WaitingForHost", Forger, Warning, &["Waiting for a responsive host"], Some("⏳"), "packages/forger/src/forger_service.rs"),
    t("WaitingForRelayReady", Forger, Info, &["Waiting for relay to become ready"], Some("⏳"), "packages/forger/src/forger_service.rs"),
    t("NoOpenSocket", Forger, Debug, &["No open socket connection to any host: ", ""], Some("‼️"), "packages/forger/src/client.rs"),
    t("RelayRequestFailed", Forger, Warning, &["Request to ", " failed, because of '", "'"], Some("‼️"), "packages/forger/src/errors.rs"),
    t("HostsDidNotRespond", Forger, Warning, &["", " didn't respond. Trying again later"], Some("🎇"), "packages/forger/src/errors.rs"),
    t("SyncCheckFailed", Forger, Error, &["Could not sync check: ", ""], Some("‼️"), "packages/forger/src/client.rs"),
    t("BroadcastFailed", Forger, Error, &["Broadcast block failed: ", ""], Some("‼️"), "packages/forger/src/client.rs"),
    t("EmitFailed", Forger, Error, &["Failed to emit \"", "\" to \"", "\""], Some("‼️"), "packages/forger/src/client.rs"),
    t("NoLocalHosts", Forger, Error, &["emitEvent: unable to find any local hosts"], Some("‼️"), "packages/forger/src/client.rs"),
    t("SessionError", Forger, Error, &["", ""], Some("‼️"), "packages/forger/src/client.rs"),
    t("NotAttesting", Forger, Warning, &["Not attesting the relay's status: ", ""], None, "packages/forger/src/attestation.rs"),
    t("NoStatusToAttest", Forger, Debug, &["The relay gave no status to attest: ", ""], None, "packages/forger/src/attestation.rs"),
    t("AttestationsNotDelivered", Forger, Debug, &["The status attestations were not delivered: ", ""], None, "packages/forger/src/attestation.rs"),
    t("PoolUnavailable", Forger, Warning, &["Could not get unconfirmed transactions from pool"], Some("⚠️"), "packages/forger/src/forger_service.rs"),
    t("SigningRecordSlow", Forger, Warning, &["Writing the signing record took ", " ms"], None, "packages/forger/src/signing_record.rs"),
    t("KeyListedTwice", Forger, Warning, &["", " lists the key of delegate ", " twice"], None, "packages/forger/src/keys.rs"),
    t("RelayHeightBeyondClock", Forger, Warning, &["The relay reports height ", ", beyond height ", " that this forger's clock allows; the version floor is checked at ", ""], None, "packages/core/src/app/forger.rs"),
    t("StartErrorKeyFileMissing", Forger, Error, &["The ", " file does not exist"], None, "packages/forger/src/errors.rs"),
    t("StartErrorNoKeys", Forger, Error, &["Unable to detect any private keys"], None, "packages/forger/src/errors.rs"),
    t("StartErrorKeyFileTooOpen", Forger, Error, &["", " is open to its group or to others (mode ", "); make it private (chmod 600)"], None, "packages/forger/src/errors.rs"),
    t("StartErrorKeyFileSecrets", Forger, Error, &["", " lists passphrases in `secrets`, which Heartwood does not read: give the delegates' private keys in `keys`"], None, "packages/forger/src/errors.rs"),
    t("StartErrorSigningRecordOtherNetwork", Forger, Error, &["the signing record ", " is for the network ", ", not ", ""], None, "packages/forger/src/errors.rs"),
    t("StartErrorSigningRecordInvalid", Forger, Error, &["the signing record ", " is not valid: ", ""], None, "packages/forger/src/errors.rs"),
    t("StartErrorSigningRecordInvalid", Forger, Error, &["the signing record ", " cannot be written or read: ", ""], None, "packages/forger/src/errors.rs"),
    t("StartErrorForgerOption", Forger, Error, &["the forger option ", " is not valid: ", ""], None, "packages/forger/src/errors.rs"),
    t("StartErrorKeyFileInvalid", Forger, Error, &["", " is not valid: ", ""], None, "packages/forger/src/errors.rs"),
    t("StartErrorKeyFileInvalid", Forger, Error, &["", " cannot be read: ", ""], None, "packages/forger/src/errors.rs"),
];

/// The captures of `pieces` in `text`, or `None` when `text` is not of that shape.
fn split<'a>(pieces: &[&str], text: &'a str) -> Option<Vec<&'a str>> {
    let (first, rest) = pieces.split_first()?;
    let mut text = text.strip_prefix(first)?;
    let Some((last, inner)) = rest.split_last() else {
        return text.is_empty().then(Vec::new);
    };
    text = text.strip_suffix(last)?;
    let mut captures = Vec::new();
    for piece in inner {
        let (before, after) = text.split_once(piece)?;
        captures.push(before);
        text = after;
    }
    captures.push(text);
    Some(captures)
}

/// The kind of a `role`'s line at `level` with the message `message` (its emoji included), or
/// `None` for another line.
pub fn classify(role: Role, level: Level, message: &str) -> Option<LineKind> {
    let same = TEMPLATES
        .iter()
        .filter(|t| t.role == role && t.level == level);
    same.into_iter().find_map(|template| {
        let text = match template.emoji {
            Some(emoji) => message.strip_suffix(emoji)?.strip_suffix(' ')?,
            None => message,
        };
        build(template.kind, &split(template.pieces, text)?)
    })
}

/// The emoji that ends the message of a line that is the start of a longer one: it has the shape
/// of a template that ends in an emoji, but not the emoji, so its message goes on in the lines
/// that follow (text of a peer or a relay that an older build wrote raw).
pub fn continues(role: Role, level: Level, message: &str) -> Option<&'static str> {
    let same = TEMPLATES
        .iter()
        .filter(|t| t.role == role && t.level == level);
    same.into_iter().find_map(|template| {
        let emoji = template.emoji.filter(|emoji| !message.ends_with(emoji))?;
        let opened = template
            .pieces
            .first()
            .is_some_and(|first| !first.is_empty());
        (opened && split(template.pieces, message).is_some()).then_some(emoji)
    })
}

/// A count as the node writes it: digits with commas.
fn num(text: &str) -> Option<u32> {
    let digits = text.replace(',', "");
    let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    numeric.then(|| digits.parse().ok())?
}

/// The number that opens a counted noun (`3 peers`).
fn count(text: &str) -> Option<u32> {
    num(text.split(' ').next()?)
}

/// A delegate's name.
fn user(text: &str) -> Option<Id> {
    Id::new(IdKind::Username, text)
}

/// The names of a list (`a, b and c`) that pass the username grammar.
fn users(text: &str) -> Vec<Id> {
    text.replace(" and ", ", ")
        .split(", ")
        .filter_map(user)
        .collect()
}

/// A kind with no typed capture, and whether its captures must be empty (or are free text).
fn plain(kind: &str) -> Option<(LineKind, bool)> {
    use LineKind as K;
    let bare = |line| Some((line, true));
    let free = |line| Some((line, false));
    match kind {
        "InSync" => bare(K::InSync),
        "ForkRecoveryStarted" => bare(K::ForkRecoveryStarted),
        "DownloadedBlockNotAccepted" => bare(K::DownloadedBlockNotAccepted),
        "NoPeersToDownload" => bare(K::NoPeersToDownload),
        "FallingBackToSeeds" => bare(K::FallingBackToSeeds),
        "NotEnoughPeersForHealth" => bare(K::NotEnoughPeersForHealth),
        "NoVerifiedPeersForHealth" => bare(K::NoVerifiedPeersForHealth),
        "NetworkMissingBlocks" => bare(K::NetworkMissingBlocks),
        "StateCorrupted" => bare(K::StateCorrupted),
        "StateMightBeCorrupted" => bare(K::StateMightBeCorrupted),
        "RemoteAccessEmpty" => bare(K::RemoteAccessEmpty),
        "ApiDisabled" => bare(K::ApiDisabled),
        "ForgerManagerStarted" => bare(K::ForgerManagerStarted),
        "NotEnoughQuorum" => bare(K::NotEnoughQuorum),
        "NotPeeredEnough" => bare(K::NotPeeredEnough),
        "ColdStartSkip" => bare(K::ColdStartSkip),
        "NoNetworkState" => bare(K::NoNetworkState),
        "WaitingForHost" => bare(K::WaitingForHost),
        "WaitingForRelayReady" => bare(K::WaitingForRelayReady),
        "NoLocalHosts" => bare(K::NoLocalHosts),
        "PoolUnavailable" => bare(K::PoolUnavailable),
        "DownloadFailed" => free(K::DownloadFailed),
        "NodeStops" => free(K::NodeStops),
        "ApiNotStarted" => free(K::ApiNotStarted),
        "RemoteAccessNoMatch" => free(K::RemoteAccessNoMatch),
        "NoOpenSocket" => free(K::NoOpenSocket),
        "RelayRequestFailed" => free(K::RelayRequestFailed),
        "HostsDidNotRespond" => free(K::HostsDidNotRespond),
        "SyncCheckFailed" => free(K::SyncCheckFailed),
        "BroadcastFailed" => free(K::BroadcastFailed),
        "EmitFailed" => free(K::EmitFailed),
        "SessionError" => free(K::SessionError),
        "NoStatusToAttest" => free(K::NoStatusToAttest),
        "AttestationsNotDelivered" => free(K::AttestationsNotDelivered),
        _ => None,
    }
}

/// The start error a template of a name `StartError<name>` stands for.
fn start_error(name: &str, c: &[&str]) -> Option<StartError> {
    let listen = |in_use: &str| {
        let port = c
            .first()
            .and_then(|a| a.parse::<SocketAddr>().ok())
            .map(|a| a.port());
        (port, in_use.contains("in use"))
    };
    let detail = c.get(1).copied().unwrap_or_default();
    Some(match name {
        "P2pCannotListen" => {
            let (port, in_use) = listen(detail);
            StartError::P2pCannotListen { port, in_use }
        }
        "ApiCannotListen" => {
            let (port, in_use) = listen(detail);
            StartError::ApiCannotListen { port, in_use }
        }
        "ApiHttps" => StartError::ApiHttps,
        "VersionFloor" => StartError::VersionFloor,
        "KeyFileMissing" => StartError::KeyFileMissing,
        "NoKeys" => StartError::NoKeys,
        "KeyFileTooOpen" => StartError::KeyFileTooOpen {
            mode: u32::from_str_radix(detail, 8).ok()?,
        },
        "KeyFileSecrets" => StartError::KeyFileSecrets,
        "KeyFileInvalid" => StartError::KeyFileInvalid,
        "SigningRecordOtherNetwork" => StartError::SigningRecordOtherNetwork,
        "SigningRecordInvalid" => StartError::SigningRecordInvalid,
        "ForgerOption" => StartError::ForgerOption,
        _ => return None,
    })
}

/// The line kind of a template named `kind` whose captures are `c`, when they pass their grammars.
fn build(kind: &str, c: &[&str]) -> Option<LineKind> {
    use LineKind as K;
    if let Some((line, bare)) = plain(kind) {
        return (!bare || c.iter().all(|text| text.is_empty())).then_some(line);
    }
    if let Some(name) = kind.strip_prefix("StartError") {
        return start_error(name, c).map(K::StartError);
    }
    let n = |i: usize| num(c.get(i)?);
    let user_at = |i: usize| user(c.get(i)?);
    let ip = |i: usize| c.get(i)?.parse::<IpAddr>().ok();
    let says = |i: usize, text: &str| c.get(i).is_some_and(|c| c.contains(text));
    let far = "more than one slot away";
    let named = |i: usize| Id::new(IdKind::Name, c.get(i)?);
    Some(match kind {
        "RelayStarted" | "ForgerStarted" => {
            Id::new(IdKind::Version, c.first()?)?;
            (named(1)?, named(2)?);
            if kind == "RelayStarted" {
                K::RelayStarted
            } else {
                K::ForgerStarted
            }
        }
        "ForkDetected" => K::ForkDetected { height: n(1)? },
        "PeersRefreshedAfterFork" => K::PeersRefreshedAfterFork {
            peers: count(c.first()?)?,
        },
        "BlocksRemoved" => K::BlocksRemoved {
            blocks: count(c.first()?)?,
            reset_to: n(1)?,
        },
        "NoBlocksFromAnyPeer" => K::NoBlocksFromAnyPeer { from: n(0)? },
        "MedianNetworkHeight" => K::MedianNetworkHeight { height: n(0)? },
        "DiscardedFutureSlot" => K::DiscardedFutureSlot { height: n(0)? },
        "DiscardedReceivedLate" => K::DiscardedReceivedLate { height: n(0)? },
        "DiscardedProcessedLate" => K::DiscardedProcessedLate { height: n(0)? },
        "DoubleForgingDetected" => K::DoubleForgingDetected {
            username: user_at(0),
        },
        "AttestationSessionReplaced" => K::AttestationSessionReplaced {
            new: ip(0)?,
            old: ip(1)?,
        },
        "AttestationKeysRepeated" => K::AttestationKeysRepeated {
            from: ip(0)?,
            repeated: n(1)?,
        },
        "BlockReceived" => K::BlockReceived {
            username: user_at(0)?,
            rank: n(1)?,
            height: n(2)?,
        },
        "FloorLaterRefuses" => K::FloorLaterRefuses { height: n(0)? },
        "DelegatesLoaded" => K::DelegatesLoaded {
            active: c.first()?.split_once(' ')?.1.starts_with("active "),
            count: count(c.first()?)?,
            names: users(c.get(1)?),
        },
        "ActivePositionEntered" | "ActivePositionLeft" => K::ActivePosition {
            entered: kind == "ActivePositionEntered",
            names: users(c.first()?),
        },
        "DueToForge" => K::DueToForge {
            username: user_at(0)?,
            seconds: count(c.get(1)?)?,
        },
        "Forged" => K::Forged {
            username: user_at(0)?,
            height: n(1)?,
            transactions: count(c.get(2)?)?,
        },
        "SendsAgain" => K::SendsAgain {
            username: user_at(0)?,
            block_id: Id::new(IdKind::Hex64, c.get(1)?)?,
            height: n(2)?,
            slot: c.get(3)?.parse().ok()?,
        },
        "WillNotSign" => K::WillNotSign {
            username: user_at(0)?,
            far_slot: says(1, far),
        },
        "ForgedOnAnotherNode" => K::ForgedOnAnotherNode {
            username: user_at(0)?,
        },
        "AlreadyForgedThisRound" => K::AlreadyForgedThisRound {
            username: user_at(0)?,
        },
        "OverheightHeaders" => {
            let plural = matches!(c.get(1).copied(), Some("" | "s"));
            plural.then_some(K::OverheightHeaders { count: n(0)? })?
        }
        "HeightsOutOfSync" => K::HeightsOutOfSync {
            ours: n(0)?,
            theirs: n(1)?,
        },
        "TooLittleTime" => K::TooLittleTime {
            username: user_at(0)?,
            left_ms: c.get(1)?.parse().ok()?,
            minimum_ms: c.get(2)?.parse().ok()?,
        },
        "AlreadyNextSlot" => K::AlreadyNextSlot {
            username: user_at(0)?,
        },
        "NotAttesting" => K::NotAttesting {
            far_slot: says(0, far),
        },
        "SigningRecordSlow" => K::SigningRecordSlow {
            millis: c.first()?.parse().ok()?,
        },
        "KeyListedTwice" => {
            let listed = c.first() == Some(&"delegates.json");
            Id::new(IdKind::PublicKey, c.get(1)?)
                .filter(|_| listed)
                .map(|_| K::KeyListedTwice)?
        }
        "RelayHeightBeyondClock" => K::RelayHeightBeyondClock { height: n(0)? },
        _ => return None,
    })
}
