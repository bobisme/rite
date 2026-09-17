//! `rite mentions follow` — a single-process, cross-channel mention stream.
//!
//! This is the scalable replacement for "spawn one watcher per channel and
//! filter": one file watcher over `channels_dir()`, per-channel byte offsets,
//! and incremental reads of only the bytes appended since the last read.
//!
//! Mentions are parsed at write time and stored on [`Message::mentions`], so
//! this never re-parses message bodies.

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use ulid::Ulid;

use crate::cli::OutputFormat;
use crate::core::channel::{dm_agents, is_dm_channel};
use crate::core::identity::resolve_agent;
use crate::core::message::{Message, MessageMeta, filter_deleted, read_messages_continuing};
use crate::core::project::channels_dir;
use crate::storage::jsonl::Continuation;
use crate::storage::watch::{debounce_events_checked, filter_channel_events, watch_directory};

/// How long to batch filesystem events before draining the changed channels.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Why a message was forwarded onto the stream.
///
/// Consumers use this to decide how to present or reply to a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// A direct message in a DM channel this agent participates in.
    Dm,
    /// A message in a regular channel whose `mentions` include this agent.
    Mention,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Dm => "dm",
            Route::Mention => "mention",
        }
    }
}

/// One emitted stream record. Serialized as a single JSON object per line.
#[derive(Debug, Clone, Serialize)]
pub struct MentionRecord {
    /// Why this message was forwarded.
    pub route: Route,
    /// Channel the message was read from (the file name, not `message.channel`).
    pub channel: String,
    /// Where a reply should be addressed: the channel name, or `@agent` for a DM.
    pub reply_target: String,
    /// The full message record as stored.
    pub message: Message,
}

pub struct FollowOptions {
    /// Stream DMs this agent participates in. On by default at the CLI; the
    /// `--no-dms` flag turns it off. A DM is the most direct form of address
    /// there is, so the default must not silently withhold one.
    pub include_dms: bool,
    /// Only stream messages carrying any of these labels (empty = no filter).
    pub labels: Vec<String>,
    /// Stop after this many seconds (None = run until killed).
    pub timeout: Option<u64>,
    /// Stop after this many records (None = uncapped).
    pub count: Option<usize>,
    /// Output format.
    pub format: OutputFormat,
}

/// Decides whether a message belongs on this agent's stream, and why.
///
/// Kept free of I/O so the routing rules — especially the DM privacy rule —
/// are directly unit-testable.
pub struct MentionFilter {
    /// Lowercased agent name; mention comparison is case-insensitive because
    /// `extract_mentions` preserves whatever case was typed.
    agent_lower: String,
    include_dms: bool,
    labels: Vec<String>,
}

impl MentionFilter {
    pub fn new(agent: &str, include_dms: bool, labels: Vec<String>) -> Self {
        Self {
            agent_lower: agent.to_lowercase(),
            include_dms,
            labels,
        }
    }

    /// Classify a message read from `channel`, returning its route if it should
    /// be forwarded.
    ///
    /// `channel` is the channel file's name, which is authoritative for DM
    /// privacy — `message.channel` is attacker-controlled content of the record.
    pub fn classify(&self, msg: &Message, channel: &str) -> Option<Route> {
        // Never echo the agent's own messages back at it.
        if msg.agent.to_lowercase() == self.agent_lower {
            return None;
        }

        // System records (hook firings, registrations) are bookkeeping, not
        // conversation. `rite inbox --mentions` skips them too, and forwarding
        // them would let a hook's own announcement re-trigger the hook.
        if matches!(msg.meta, Some(MessageMeta::System { .. })) {
            return None;
        }

        if !self.labels.is_empty() && !msg.has_any_label(&self.labels) {
            return None;
        }

        if is_dm_channel(channel) {
            // DM privacy is absolute. Participation is the only thing that can
            // route a message out of a DM channel — a mention never overrides
            // it, and a DM channel whose participants cannot be parsed is
            // treated as private (fail closed).
            if !self.is_dm_participant(channel) {
                return None;
            }
            // A participant's DM routes as `dm` whether or not it also mentions
            // the agent — the channel is already as direct as address gets.
            return self.include_dms.then_some(Route::Dm);
        }

        if msg
            .mentions
            .iter()
            .any(|m| m.to_lowercase() == self.agent_lower)
        {
            return Some(Route::Mention);
        }

        None
    }

    fn is_dm_participant(&self, channel: &str) -> bool {
        match dm_agents(channel) {
            Some((a, b)) => {
                a.to_lowercase() == self.agent_lower || b.to_lowercase() == self.agent_lower
            }
            None => false,
        }
    }

    /// Where a reply to this record should be sent.
    /// Route a message and compute where a reply goes, for callers that
    /// deliver outside the stream (push-at-send in `rite send`).
    pub fn route(&self, msg: &Message, channel: &str) -> Option<(Route, String)> {
        let route = self.classify(msg, channel)?;
        Some((route, self.reply_target(channel, msg)))
    }

    fn reply_target(&self, channel: &str, msg: &Message) -> String {
        if !is_dm_channel(channel) {
            return channel.to_string();
        }
        match dm_agents(channel) {
            Some((a, b)) => {
                let other = if a.to_lowercase() == self.agent_lower {
                    b
                } else {
                    a
                };
                format!("@{}", other)
            }
            None => format!("@{}", msg.agent),
        }
    }

    pub fn record(&self, msg: Message, channel: &str) -> Option<MentionRecord> {
        let route = self.classify(&msg, channel)?;
        let reply_target = self.reply_target(channel, &msg);
        Some(MentionRecord {
            route,
            channel: channel.to_string(),
            reply_target,
            message: msg,
        })
    }
}

/// Whether an error chain bottoms out in a missing file.
fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}

/// `(dev, ino)` of the directory at `path`.
fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)
        .with_context(|| format!("cannot stat channels directory {}", path.display()))?;
    if !meta.is_dir() {
        anyhow::bail!("{} is not a directory", path.display());
    }
    Ok((meta.dev(), meta.ino()))
}

/// Fail unless the directory at `path` is still the one the watcher was
/// registered on.
fn ensure_same_directory(path: &Path, watched: (u64, u64)) -> Result<()> {
    let now = directory_identity(path)?;
    if now != watched {
        anyhow::bail!(
            "channels directory {} was replaced under the watcher; the mention stream cannot continue",
            path.display()
        );
    }
    Ok(())
}

/// Per-channel positions in the channel JSONL files, and every message id
/// read through them.
///
/// Existing channels are seeded at their current end of file ("now"), so
/// startup does not replay history. A channel first seen after startup has no
/// entry and therefore starts at offset 0 — deliberate asymmetry, so a channel
/// whose very first message is the mention is not missed.
///
/// A position is a [`Continuation`]: the file's identity and a digest of
/// every byte consumed. It is resumed only when the file, read under the
/// same lock as the parse, is still that file with that exact prefix;
/// otherwise the channel is rescanned from the start. The seen set is what
/// makes a rescan deliver each new message exactly once and nothing already
/// delivered or predating startup, and it outlives the file: a channel that
/// vanishes and comes back replays nothing.
#[derive(Debug, Default)]
struct Cursors {
    positions: HashMap<String, Continuation>,
    seen: HashMap<String, HashSet<Ulid>>,
}

/// One read of a channel: what to hand on, and where the cursor now rests.
struct Consumed {
    messages: Vec<Message>,
    /// Whether the read resumed from the previous position. `false` is a
    /// rescan from the start of the file.
    resumed: bool,
    /// Byte offset the cursor advanced to (the end of the last terminated
    /// line, or the start of an unterminated one).
    end: u64,
    /// Terminated lines this build could not parse, by byte offset.
    skipped: Vec<u64>,
}

impl Cursors {
    /// Seed every channel file currently on disk at its end of file, and
    /// record every id already in it so a later rescan does not replay it.
    /// Fails if any channel cannot be read: a channel with no position and
    /// no history would replay everything on its first change, and a
    /// consumer that holds occupancy on this stream must not start blind.
    fn seeded_at_now(channels_path: &Path) -> Result<Self> {
        let mut cursors = Self::default();
        // Enumeration fails closed too: a channel the listing missed would
        // be read from its start on its first change.
        let entries = std::fs::read_dir(channels_path).with_context(|| {
            format!(
                "cannot list channels in {} to seed the mention stream",
                channels_path.display()
            )
        })?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!(
                    "cannot list channels in {} to seed the mention stream",
                    channels_path.display()
                )
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                cursors
                    .consume(name, &path)
                    .with_context(|| format!("cannot seed the mention stream from #{name}"))?;
            }
        }
        Ok(cursors)
    }

    /// Read `channel` from its position, or from the start if that position
    /// no longer describes the file, advance it, and return the messages
    /// not read before. The position stops at the start of an unterminated
    /// final line, so the next read begins there rather than inside the
    /// record.
    fn consume(&mut self, channel: &str, path: &Path) -> Result<Consumed> {
        let read = read_messages_continuing(path, self.positions.get(channel))?;
        let end = read.position.offset;
        let skipped = read
            .issues
            .skipped
            .iter()
            .map(|s| s.byte_offset)
            .filter(|o| *o < end)
            .collect();
        self.positions.insert(channel.to_string(), read.position);

        // Every record on disk is remembered, tombstones and their targets
        // included, before deletion is applied: what is handed on is the
        // filtered view, but what counts as "already read" is the file.
        let seen = self.seen.entry(channel.to_string()).or_default();
        let unseen: Vec<Message> = read
            .records
            .into_iter()
            .filter(|m| seen.insert(m.id))
            .collect();
        let messages = filter_deleted(unseen);
        Ok(Consumed {
            messages,
            resumed: read.resumed,
            end,
            skipped,
        })
    }

    /// Read every channel on disk from its position and return what is new,
    /// as `(channel, message)`: the catch-up between the startup baseline
    /// and the first watcher event. A channel that appeared since the
    /// baseline is read from its start.
    fn catch_up(&mut self, channels_path: &Path) -> Result<Vec<(String, Message)>> {
        let mut found = Vec::new();
        let entries = std::fs::read_dir(channels_path)
            .with_context(|| format!("cannot list channels in {}", channels_path.display()))?;
        for entry in entries {
            let path = entry
                .with_context(|| format!("cannot list channels in {}", channels_path.display()))?
                .path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                let consumed = self
                    .consume(name, &path)
                    .with_context(|| format!("cannot catch up on #{name}"))?;
                found.extend(consumed.messages.into_iter().map(|m| (name.to_string(), m)));
            }
        }
        Ok(found)
    }

    /// The channel file is gone. Drop the position, but keep every id read
    /// through it: a file that comes back (renamed away and back, a slow
    /// delete then copy, sync tooling that briefly removes the path) is
    /// rescanned from the start, and must not replay work already handed
    /// on. The map therefore stays proportional to every channel seen,
    /// not only to live ones.
    fn forget(&mut self, channel: &str) {
        self.positions.remove(channel);
    }
}

/// Stream every message mentioning `agent` across all channels (plus its DMs).
pub fn follow(options: FollowOptions, explicit_agent: Option<&str>) -> Result<()> {
    let format = options.format;
    follow_with(options, explicit_agent, |record| emit(record, format))
}

/// The stream behind `follow`, handing each record to `on_record` instead of
/// printing it. `rite channel` runs this in-process and turns records into
/// MCP notifications.
pub fn follow_with<F>(
    options: FollowOptions,
    explicit_agent: Option<&str>,
    on_record: F,
) -> Result<()>
where
    F: FnMut(&MentionRecord) -> Result<()>,
{
    follow_with_ready(options, explicit_agent, || {}, on_record)
}

/// [`follow_with`] plus a readiness callback, invoked once the directory
/// watcher is registered and the cursors are seeded: from that point on,
/// every new message is guaranteed to be seen. A consumer that must not
/// claim to be reachable before it can deliver waits for this.
pub fn follow_with_ready<R, F>(
    options: FollowOptions,
    explicit_agent: Option<&str>,
    on_ready: R,
    mut on_record: F,
) -> Result<()>
where
    R: FnOnce(),
    F: FnMut(&MentionRecord) -> Result<()>,
{
    let agent = resolve_agent(explicit_agent).ok_or_else(|| {
        anyhow::anyhow!(
            "mentions follow requires agent identity. Set RITE_AGENT or use --agent <name>."
        )
    })?;

    let filter = MentionFilter::new(&agent, options.include_dms, options.labels.clone());

    let channels_path = channels_dir();
    if !channels_path.exists() {
        std::fs::create_dir_all(&channels_path).with_context(|| {
            format!(
                "Failed to create channels directory: {}",
                channels_path.display()
            )
        })?;
    }

    // Startup, in an order with no hole:
    //
    // 1. Seed a baseline: every existing channel is read to its end and
    //    every id in it is history. Nothing before this point is emitted.
    // 2. Register the watcher. From here on every append queues an event.
    // 3. Catch up: read every channel again and emit what arrived since the
    //    baseline. An append between 1 and 2 produced no event and is
    //    delivered here; an append after 2 is delivered here or by its
    //    queued event, once, because the seen set does not care which.
    // 4. Ready.
    //
    // Seeding after registration instead would consume an append that
    // landed between the two as history, and its queued event would then
    // find nothing new. Seeding before registration alone would miss an
    // append between the two entirely.
    //
    // The watcher is bound to the directory inode that exists at
    // registration; every later read goes through the pathname. If the
    // directory is replaced (renamed away and recreated, a backup copied
    // back), events stop while reads continue, and the stream would be
    // silent rather than wrong. So the directory's identity is taken before
    // the baseline, checked after registration and before readiness, and
    // checked on every poll; a change ends the stream, and a consumer that
    // holds occupancy on it must let go.
    let watched = directory_identity(&channels_path)?;
    let mut cursors = Cursors::seeded_at_now(&channels_path)?;
    let (_watcher, rx) =
        watch_directory(&channels_path).with_context(|| "Failed to watch channels directory")?;
    ensure_same_directory(&channels_path, watched)?;
    let mut caught_up: Vec<MentionRecord> = cursors
        .catch_up(&channels_path)?
        .into_iter()
        .filter_map(|(channel, msg)| filter.record(msg, &channel))
        .collect();
    caught_up.sort_by_key(|r| (r.message.ts, r.message.id));
    ensure_same_directory(&channels_path, watched)?;
    on_ready();

    if options.format == OutputFormat::Pretty {
        eprintln!(
            "{}",
            format!(
                "Following mentions of @{}{} (Ctrl+C to exit)",
                agent,
                if options.include_dms {
                    " + DMs"
                } else {
                    " (--no-dms: DMs suppressed)"
                }
            )
            .cyan()
            .bold()
        );
    }

    let start = Instant::now();
    let mut emitted: usize = 0;

    for record in &caught_up {
        on_record(record)?;
        emitted += 1;
        if let Some(max) = options.count
            && emitted >= max
        {
            return Ok(());
        }
    }

    loop {
        if let Some(timeout) = options.timeout
            && start.elapsed() >= Duration::from_secs(timeout)
        {
            return Ok(());
        }

        // Blocks up to POLL_INTERVAL, so timeout/count are checked regularly
        // even when nothing is happening. A watcher failure ends the stream:
        // after an overflow or an invalidated watch this loop could only
        // report silence, and a consumer that holds occupancy on the
        // strength of this stream must not mistake that for quiet.
        let changed = filter_channel_events(
            debounce_events_checked(&rx, POLL_INTERVAL)
                .with_context(|| "mention stream can no longer deliver")?,
        );
        ensure_same_directory(&channels_path, watched)?;

        let mut batch: Vec<MentionRecord> = Vec::new();

        for channel in changed {
            let path = channels_path.join(format!("{}.jsonl", channel));
            if !path.exists() {
                // Deleted or renamed out from under us.
                cursors.forget(&channel);
                continue;
            }

            let consumed = match cursors.consume(&channel, &path) {
                Ok(consumed) => consumed,
                Err(e) if is_not_found(&e) => {
                    // Deleted between the existence check and the open:
                    // the same case as above, one instant later.
                    cursors.forget(&channel);
                    continue;
                }
                Err(e) => {
                    // Any other failure to open, lock, or read a channel
                    // that just changed ends the stream. Nothing promises
                    // another event to retry on, and a consumer that holds
                    // occupancy on this stream must not keep it over a
                    // change it could not read.
                    return Err(e.context(format!(
                        "cannot read #{channel} after a change; the mention stream cannot continue"
                    )));
                }
            };
            for offset in &consumed.skipped {
                // A complete line this build cannot parse. The cursor is
                // past it, since nothing appended later can change it; an
                // in-place repair changes the consumed prefix and forces a
                // rescan that picks it up.
                eprintln!(
                    "warn: skipped an unreadable record in #{} at byte {}",
                    channel, offset
                );
            }
            if !consumed.resumed && consumed.end > 0 {
                eprintln!(
                    "warn: #{} was rewritten or replaced; rescanned from the start",
                    channel
                );
            }

            batch.extend(
                consumed
                    .messages
                    .into_iter()
                    .filter_map(|msg| filter.record(msg, &channel)),
            );
        }

        // Channels are drained in filesystem-event order, which is arbitrary.
        // Emit each batch in message order so a burst across several channels
        // reaches the consumer chronologically.
        batch.sort_by_key(|r| (r.message.ts, r.message.id));

        for record in &batch {
            on_record(record)?;
            emitted += 1;
            if let Some(max) = options.count
                && emitted >= max
            {
                return Ok(());
            }
        }
    }
}

fn emit(record: &MentionRecord, format: OutputFormat) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    match format {
        // JSONL: exactly one JSON object per line, flushed immediately.
        OutputFormat::Json => writeln!(stdout, "{}", serde_json::to_string(record)?)?,
        OutputFormat::Text => writeln!(
            stdout,
            "{}  {}  {}  {}  {}",
            record.message.id,
            record.route.as_str(),
            record.channel,
            record.message.agent,
            single_line(&record.message.body)
        )?,
        OutputFormat::Pretty => {
            let ts = record
                .message
                .ts
                .with_timezone(&chrono::Local)
                .format("%H:%M");
            writeln!(
                stdout,
                "[{}] {} {}: {}",
                ts.to_string().dimmed(),
                format!("[{}] #{}", record.route.as_str(), record.channel).dimmed(),
                record.message.agent.cyan().bold(),
                record.message.body
            )?
        }
    }
    stdout.flush()?;
    Ok(())
}

/// Collapse newlines so a text record stays on one line.
fn single_line(body: &str) -> String {
    body.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::SystemEvent;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    fn msg(agent: &str, channel: &str, body: &str) -> Message {
        Message::new(agent, channel, body)
    }

    #[test]
    fn mention_in_regular_channel_routes_as_mention() {
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        let m = msg("other", "rite", "can @rite-dev look at this?");
        assert_eq!(filter.classify(&m, "rite"), Some(Route::Mention));
    }

    #[test]
    fn non_mention_is_not_forwarded() {
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        let m = msg("other", "rite", "unrelated chatter");
        assert_eq!(filter.classify(&m, "rite"), None);

        // A mention of a different agent must not match on a prefix.
        let m = msg("other", "rite", "ping @rite-dev-two");
        assert_eq!(filter.classify(&m, "rite"), None);
    }

    #[test]
    fn mention_matching_is_case_insensitive() {
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        let m = msg("other", "rite", "hey @Rite-Dev and @RITE-DEV");
        assert_eq!(filter.classify(&m, "rite"), Some(Route::Mention));

        // ...and so is the agent side of the comparison.
        let filter = MentionFilter::new("Rite-Dev", false, vec![]);
        let m = msg("other", "rite", "hey @rite-dev");
        assert_eq!(filter.classify(&m, "rite"), Some(Route::Mention));
    }

    #[test]
    fn self_authored_messages_are_dropped() {
        let filter = MentionFilter::new("rite-dev", true, vec![]);

        // Even when the agent mentions itself.
        let m = msg("rite-dev", "rite", "note to self @rite-dev");
        assert_eq!(filter.classify(&m, "rite"), None);

        // Case-insensitively, and in its own DMs.
        let m = msg("Rite-Dev", "_dm_alice_rite-dev", "hi");
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), None);
    }

    /// A participant's DM is delivered by default (the CLI passes
    /// `include_dms: true` unless `--no-dms` is given), and suppressed only
    /// when the caller explicitly opts out.
    #[test]
    fn dm_to_participant_routes_as_dm_unless_suppressed() {
        let m = msg("alice", "_dm_alice_rite-dev", "got a minute?");

        let filter = MentionFilter::new("rite-dev", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), Some(Route::Dm));

        // --no-dms
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), None);
    }

    /// A DM that also mentions the agent still routes as `dm`, not `mention` —
    /// the DM channel is the reason it was forwarded.
    #[test]
    fn a_mention_inside_ones_own_dm_routes_as_dm() {
        let m = msg("alice", "_dm_alice_rite-dev", "ping @rite-dev");

        let filter = MentionFilter::new("rite-dev", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), Some(Route::Dm));

        // ...and --no-dms suppresses it: the mention does not smuggle it back in.
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), None);
    }

    /// DM privacy is absolute: a mention never overrides DM participation.
    #[test]
    fn mention_never_leaks_a_dm_the_agent_is_not_party_to() {
        // alice and bob are talking; carol is mentioned but is not a party.
        let m = msg("alice", "_dm_alice_bob", "we should ask @carol about this");
        assert!(m.mentions.iter().any(|x| x == "carol"));

        // With DMs requested...
        let filter = MentionFilter::new("carol", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_bob"), None);

        // ...and without.
        let filter = MentionFilter::new("carol", false, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_bob"), None);

        // Case differences must not create a back door.
        let m = msg("alice", "_dm_alice_bob", "ping @Carol");
        let filter = MentionFilter::new("carol", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_bob"), None);

        // A participant of that same DM still receives it.
        let filter = MentionFilter::new("bob", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_bob"), Some(Route::Dm));
    }

    #[test]
    fn unparseable_dm_channel_fails_closed() {
        let m = msg("alice", "_dm_broken", "hi @carol");
        let filter = MentionFilter::new("carol", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_broken"), None);
    }

    #[test]
    fn dm_privacy_uses_the_file_name_not_the_record_channel() {
        // A record claiming to live in a public channel, stored in someone
        // else's DM file, must not leak.
        let mut m = msg("alice", "_dm_alice_bob", "hey @carol");
        m.channel = "general".to_string();
        let filter = MentionFilter::new("carol", true, vec![]);
        assert_eq!(filter.classify(&m, "_dm_alice_bob"), None);
    }

    #[test]
    fn labels_filter_the_stream() {
        let filter = MentionFilter::new("rite-dev", true, vec!["review".to_string()]);

        let m = msg("other", "rite", "@rite-dev please look").with_labels(vec!["chat".to_string()]);
        assert_eq!(filter.classify(&m, "rite"), None);

        let m =
            msg("other", "rite", "@rite-dev please look").with_labels(vec!["review".to_string()]);
        assert_eq!(filter.classify(&m, "rite"), Some(Route::Mention));

        let m = msg("alice", "_dm_alice_rite-dev", "ping");
        assert_eq!(filter.classify(&m, "_dm_alice_rite-dev"), None);
    }

    #[test]
    fn system_records_are_not_forwarded() {
        let filter = MentionFilter::new("rite-dev", true, vec![]);
        let m = msg("system", "rite", "Hook hk-1 fired: notify @rite-dev").with_meta(
            MessageMeta::System {
                event: SystemEvent::AgentRegistered,
            },
        );
        assert_eq!(filter.classify(&m, "rite"), None);
    }

    #[test]
    fn reply_target_is_the_channel_or_the_other_dm_party() {
        let filter = MentionFilter::new("rite-dev", true, vec![]);

        let m = msg("alice", "rite", "@rite-dev hi");
        let record = filter.record(m, "rite").unwrap();
        assert_eq!(record.reply_target, "rite");
        assert_eq!(record.route, Route::Mention);

        let m = msg("alice", "_dm_alice_rite-dev", "hi");
        let record = filter.record(m, "_dm_alice_rite-dev").unwrap();
        assert_eq!(record.reply_target, "@alice");
        assert_eq!(record.route, Route::Dm);
    }

    #[test]
    fn record_serializes_as_a_single_json_line() {
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        let m = msg("alice", "rite", "@rite-dev multi\nline body");
        let record = filter.record(m, "rite").unwrap();
        let line = serde_json::to_string(&record).unwrap();
        assert!(
            !line.contains('\n'),
            "JSONL record must not contain a newline"
        );
        assert!(line.contains("\"route\":\"mention\""));
    }

    fn consume_bodies(cursors: &mut Cursors, channel: &str, path: &Path) -> Vec<String> {
        cursors
            .consume(channel, path)
            .unwrap()
            .messages
            .into_iter()
            .map(|m| m.body)
            .collect()
    }

    fn position(cursors: &Cursors, channel: &str) -> u64 {
        cursors.positions[channel].offset
    }

    #[test]
    fn existing_channels_are_seeded_at_end_of_file() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "old news")).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();

        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        assert_eq!(position(&cursors, "general"), size);

        // Nothing is replayed from a channel seeded at "now".
        let consumed = cursors.consume("general", &path).unwrap();
        assert!(consumed.resumed);
        assert!(consumed.messages.is_empty());
    }

    #[test]
    fn channels_created_after_startup_are_read_from_offset_zero() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("general.jsonl"), "").unwrap();

        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();

        // A channel that did not exist at seed time starts at 0, so its very
        // first message is delivered.
        let path = temp.path().join("brand-new.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "brand-new", "@rite-dev first"))
            .unwrap();
        let consumed = cursors.consume("brand-new", &path).unwrap();
        assert!(!consumed.resumed);
        let filter = MentionFilter::new("rite-dev", false, vec![]);
        assert_eq!(
            filter.classify(&consumed.messages[0], "brand-new"),
            Some(Route::Mention)
        );
    }

    #[test]
    fn truncated_file_restarts_from_zero() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "one")).unwrap();
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "two")).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let mut cursors = Cursors::default();
        cursors.consume("general", &path).unwrap();
        assert_eq!(position(&cursors, "general"), size);

        // File shrank: it was rewritten or replaced.
        let content = std::fs::read(&path).unwrap();
        let first_line_end = content.iter().position(|b| *b == b'\n').unwrap() + 1;
        std::fs::write(&path, &content[..first_line_end]).unwrap();
        let consumed = cursors.consume("general", &path).unwrap();
        assert!(!consumed.resumed);
        assert!(
            consumed.messages.is_empty(),
            "the surviving record was already seen"
        );
        assert_eq!(position(&cursors, "general"), first_line_end as u64);
    }

    /// The same inode rewritten to the same length: only the bytes tell.
    #[test]
    fn an_equal_length_in_place_rewrite_is_rescanned_and_each_message_delivered_once() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "history")).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();

        let second = msg("alice", "general", "@rite-dev first");
        crate::storage::jsonl::append_record(&path, &second).unwrap();
        let delivered = cursors.consume("general", &path).unwrap();
        assert!(delivered.resumed);
        assert_eq!(delivered.messages.len(), 1);
        assert_eq!(delivered.messages[0].id, second.id);
        let end = delivered.end;
        let ino = std::fs::metadata(&path).unwrap().ino();

        // Rewrite in place: the first line stays, the second is a different
        // message of exactly the same length (a fresh ULID is as long as the
        // old one, and the body length is kept).
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines = content.lines();
        let first_line = lines.next().unwrap().to_string();
        let mut third = second.clone();
        third.id = Ulid::new();
        third.body = "@rite-dev again".to_string();
        assert_eq!(third.body.len(), second.body.len());
        let third_line = serde_json::to_string(&third).unwrap();
        assert_eq!(third_line.len(), lines.next().unwrap().len());
        std::fs::write(&path, format!("{first_line}\n{third_line}\n")).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), end);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), ino);

        let delivered = cursors.consume("general", &path).unwrap();
        assert!(!delivered.resumed, "consumed bytes changed");
        assert_eq!(
            delivered.messages.len(),
            1,
            "history and the already delivered id are not replayed"
        );
        assert_eq!(delivered.messages[0].id, third.id);

        // Now the position is trusted again.
        let again = cursors.consume("general", &path).unwrap();
        assert!(again.resumed);
        assert!(again.messages.is_empty());
    }

    /// A rewrite that changes only an early record, far before the cursor,
    /// and preserves everything after it, including the inode and length.
    #[test]
    fn an_early_in_place_rewrite_far_before_the_cursor_is_rescanned() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        let first = msg("alice", "general", "the very first record here");
        crate::storage::jsonl::append_record(&path, &first).unwrap();
        // Well over 8 KiB of history after it, so the changed bytes are far
        // outside any trailing window.
        for i in 0..200 {
            crate::storage::jsonl::append_record(
                &path,
                &msg(
                    "alice",
                    "general",
                    &format!("filler {i:04} {}", "x".repeat(40)),
                ),
            )
            .unwrap();
        }
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        let live = msg("alice", "general", "@rite-dev live one");
        crate::storage::jsonl::append_record(&path, &live).unwrap();
        let delivered = cursors.consume("general", &path).unwrap();
        assert_eq!(delivered.messages.len(), 1);
        let end = delivered.end;
        assert!(end > 16 * 1024);
        let ino = std::fs::metadata(&path).unwrap().ino();

        // Replace only the first line with a mention of the same length.
        let content = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
        let mut inserted = first.clone();
        inserted.id = Ulid::new();
        inserted.body = "@rite-dev the early insert".to_string();
        assert_eq!(inserted.body.len(), first.body.len());
        let inserted_line = serde_json::to_string(&inserted).unwrap();
        assert_eq!(inserted_line.len(), lines[0].len());
        lines[0] = inserted_line;
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), end);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), ino);

        let delivered = cursors.consume("general", &path).unwrap();
        assert!(!delivered.resumed, "an early byte changed");
        assert_eq!(delivered.messages.len(), 1, "exactly the inserted record");
        assert_eq!(delivered.messages[0].id, inserted.id);
        assert_eq!(position(&cursors, "general"), end);
    }

    /// A replacement with a new inode and greater length, as git sync or a
    /// backup copied back produces.
    #[test]
    fn a_longer_replacement_file_is_rescanned_and_only_new_messages_delivered() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        let old = msg("alice", "general", "@rite-dev history");
        crate::storage::jsonl::append_record(&path, &old).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(position(&cursors, "general"), size);

        let replacement = temp.path().join("general.jsonl.tmp");
        let merged = msg("bob", "general", "@rite-dev merged in from another machine");
        std::fs::write(
            &replacement,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&merged).unwrap(),
                serde_json::to_string(&old).unwrap()
            ),
        )
        .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > size);

        let delivered = cursors.consume("general", &path).unwrap();
        assert!(!delivered.resumed, "new identity");
        assert_eq!(delivered.messages.len(), 1);
        assert_eq!(delivered.messages[0].id, merged.id);
    }

    /// The file vanishes for a while and comes back with its history plus
    /// one new record: only the new record is delivered.
    #[test]
    fn a_channel_that_vanishes_and_returns_replays_nothing() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "@rite-dev history"))
            .unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        let live = msg("alice", "general", "@rite-dev live");
        crate::storage::jsonl::append_record(&path, &live).unwrap();
        assert_eq!(
            consume_bodies(&mut cursors, "general", &path),
            vec!["@rite-dev live"]
        );

        let saved = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        cursors.forget("general");
        assert!(cursors.positions.is_empty());

        let fresh = msg("bob", "general", "@rite-dev after the outage");
        let mut content = saved.clone();
        content.extend_from_slice(serde_json::to_string(&fresh).unwrap().as_bytes());
        content.push(b'\n');
        std::fs::write(&path, content).unwrap();

        assert_eq!(
            consume_bodies(&mut cursors, "general", &path),
            vec!["@rite-dev after the outage"]
        );
    }

    /// A writer observed mid-line: the cursor waits at the start of the
    /// unterminated line, and the completed record is delivered once.
    #[test]
    fn an_unterminated_tail_line_holds_the_cursor_until_it_completes() {
        use std::io::Write as _;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "history")).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        let held_at = position(&cursors, "general");

        let pending = msg("alice", "general", "@rite-dev arrives in two writes");
        let line = serde_json::to_string(&pending).unwrap();
        let (head, tail) = line.split_at(line.len() / 2);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(head.as_bytes()).unwrap();

        let consumed = cursors.consume("general", &path).unwrap();
        assert!(consumed.resumed);
        assert!(consumed.messages.is_empty());
        assert_eq!(
            consumed.end, held_at,
            "the cursor does not enter the torn line"
        );
        assert!(
            consumed.skipped.is_empty(),
            "a torn tail is not a skipped record"
        );

        file.write_all(tail.as_bytes()).unwrap();
        file.write_all(b"\n").unwrap();
        let consumed = cursors.consume("general", &path).unwrap();
        assert!(consumed.resumed);
        let bodies: Vec<&str> = consumed.messages.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, vec!["@rite-dev arrives in two writes"]);
        assert_eq!(consumed.end, std::fs::metadata(&path).unwrap().len());
        assert!(consume_bodies(&mut cursors, "general", &path).is_empty());
    }

    /// A terminated line that cannot be parsed is stepped over and reported;
    /// the records after it are delivered.
    #[test]
    fn a_malformed_complete_line_is_skipped_and_reported_by_offset() {
        use std::io::Write as _;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "history")).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        let bad_at = std::fs::metadata(&path).unwrap().len();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"this is not a record\n").unwrap();
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "@rite-dev after"))
            .unwrap();

        let consumed = cursors.consume("general", &path).unwrap();
        assert_eq!(consumed.skipped, vec![bad_at]);
        let bodies: Vec<&str> = consumed.messages.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, vec!["@rite-dev after"]);
        assert_eq!(consumed.end, std::fs::metadata(&path).unwrap().len());
    }

    /// A channel that cannot be read at startup fails the seed, so a
    /// consumer never starts with a channel it would replay in full.
    #[test]
    fn seeding_fails_closed_on_an_unreadable_channel() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "general", "@rite-dev history"))
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            // Running with privileges that ignore modes; nothing to prove.
            return;
        }
        let err = Cursors::seeded_at_now(temp.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("cannot seed the mention stream from #general"),
            "{err:#}"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// The channels directory itself cannot be listed: the seed fails, so a
    /// channel the listing would have missed is never read from its start
    /// later under held occupancy.
    #[test]
    fn seeding_fails_closed_when_the_channels_directory_cannot_be_listed() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("channels");
        std::fs::create_dir(&dir).unwrap();
        crate::storage::jsonl::append_record(
            &dir.join("general.jsonl"),
            &msg("alice", "general", "@rite-dev history"),
        )
        .unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&dir).is_ok() {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let err = Cursors::seeded_at_now(&dir).unwrap_err();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            format!("{err:#}").contains("cannot list channels"),
            "{err:#}"
        );
    }

    /// A message deleted before startup is remembered as read even though
    /// it is filtered out, so a copy of the file that lacks the tombstone
    /// does not present it as new.
    #[test]
    fn a_message_deleted_before_startup_is_not_replayed_when_its_tombstone_disappears() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("general.jsonl");
        let original = msg("alice", "general", "@rite-dev do the old thing");
        crate::storage::jsonl::append_record(&path, &original).unwrap();
        let mut tombstone = msg("alice", "general", "");
        tombstone.meta = Some(MessageMeta::Deleted {
            target_id: original.id,
            deleted_by: "alice".to_string(),
            deleted_at: chrono::Utc::now(),
        });
        crate::storage::jsonl::append_record(&path, &tombstone).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        assert!(cursors.seen["general"].contains(&original.id));
        assert!(cursors.seen["general"].contains(&tombstone.id));

        // A copy from before the deletion, plus one new mention.
        let fresh = msg("bob", "general", "@rite-dev the new thing");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&original).unwrap(),
                serde_json::to_string(&fresh).unwrap()
            ),
        )
        .unwrap();
        let consumed = cursors.consume("general", &path).unwrap();
        assert!(!consumed.resumed);
        let bodies: Vec<&str> = consumed.messages.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, vec!["@rite-dev the new thing"]);
    }

    /// What lands between the baseline seed and the first watcher event is
    /// delivered by the catch-up, once; a channel born in that window is
    /// read from its start.
    #[test]
    fn catch_up_delivers_what_arrived_since_the_baseline_exactly_once() {
        let temp = TempDir::new().unwrap();
        let general = temp.path().join("general.jsonl");
        crate::storage::jsonl::append_record(
            &general,
            &msg("alice", "general", "@rite-dev history"),
        )
        .unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();

        let gap = msg("alice", "general", "@rite-dev in the gap");
        crate::storage::jsonl::append_record(&general, &gap).unwrap();
        let born = msg("bob", "brand-new", "@rite-dev first ever");
        crate::storage::jsonl::append_record(&temp.path().join("brand-new.jsonl"), &born).unwrap();

        let mut found = cursors.catch_up(temp.path()).unwrap();
        found.sort_by_key(|(c, _)| c.clone());
        let ids: Vec<Ulid> = found.iter().map(|(_, m)| m.id).collect();
        assert_eq!(ids, vec![born.id, gap.id], "{found:?}");

        // The event the appends queued finds nothing new.
        assert!(cursors.catch_up(temp.path()).unwrap().is_empty());
        assert!(consume_bodies(&mut cursors, "general", &general).is_empty());
    }

    #[test]
    fn deleted_channels_drop_their_position_but_keep_their_history() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("gone.jsonl");
        crate::storage::jsonl::append_record(&path, &msg("alice", "gone", "x")).unwrap();
        let mut cursors = Cursors::seeded_at_now(temp.path()).unwrap();
        assert!(!cursors.seen["gone"].is_empty());
        cursors.forget("gone");
        assert!(cursors.positions.is_empty());
        assert!(!cursors.seen["gone"].is_empty());
    }
}
