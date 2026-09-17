//! `rite sessions` — record live harness sessions and their occupancy claims.
//!
//! See `crate::core::session` for the model. This is the phase-one registry
//! from `notes/agent-sessions.md`: attach, detach, renew, list. No routing,
//! no delivery ledger.
//!
//! ## Two logs, one protocol
//!
//! An attachment lives in `local/sessions.jsonl`; its occupancy is an
//! `agent://<name>` claim in `claims.jsonl`. Two append-only files cannot be
//! written atomically together, so every operation follows the same rules:
//!
//! - **Reserve, then occupy, then commit.** `attach` first appends an
//!   `attaching` record under the sessions lock, where the predicate enforces
//!   one reservation per agent and per session id. Then it stakes the claim.
//!   Only then does it append `attached`. An `attached` record therefore never
//!   exists without its claim having existed first.
//! - **Every claim write is authorised by the claim's `owner`.** The claim
//!   carries the attachment id that owns it. Release and extension are
//!   compare-and-appends whose predicate checks that owner under the claims
//!   lock, so a stale detach or renew from an earlier attachment cannot touch
//!   a successor's claim. `--replace` re-tags the owner to the successor
//!   instead of releasing and restaking, so occupancy has no gap.
//! - **Every session write is conditional too.** Detach and commit are
//!   compare-and-appends on the fold, so a record that was already retired is
//!   not retired twice and a reservation that was detached mid-attach is not
//!   committed.
//! - **Direct attach cannot protect a harness that already exists.** A
//!   responder hook that stakes `agent://<name>` before `attach` takes the
//!   claims lock has legitimately started, and the harness the launcher
//!   already began now overlaps it; `attach` refuses and says so, but it
//!   cannot undo the launch. A launcher that must never overlap a responder
//!   runs `reserve` first, starts the harness, and binds the session id
//!   with `attach --attachment`. The reservation already holds the claim,
//!   so there is no gap.
//! - **Occupancy is advisory, like every rite claim.** The locks above are
//!   file locks on the current `claims.jsonl`. `rite sync pull` merges with
//!   git, which can replace that file under a held lock, so a commit fenced
//!   here can validate a snapshot that a concurrent pull has just superseded.
//!   This is the same property every claim and hook lease in rite has, and
//!   the owner accepted it for phase one rather than serialise sync with
//!   storage writes. Do not run `sync pull` while attaching if that window
//!   matters to you.
//! - **Leftovers are reconciled, not trusted.** A crash can still leave an
//!   `attaching` record without a commit, or a claim whose owner is no longer
//!   reserved. Every session command for an agent first retires reservations
//!   older than `PENDING_TTL_SECS` and releases claims owned by attachments
//!   that are no longer reserved. Both repairs are themselves conditional.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use ulid::Ulid;

use super::OutputFormat;
use super::format::format_output;
use super::mentions::MentionFilter;
use crate::core::claim::{ClaimEvent, FileClaim};
use crate::core::identity::{require_agent, resolve_agent};
use crate::core::message::Message;
use crate::core::project::{adapters_path, claims_path, data_dir, local_dir, sessions_path};
use crate::core::session::{AdapterTable, builtin_adapters};
use crate::core::session::{
    SessionKind, SessionRecord, active_for_agent, fold, occupancy_pattern, reserved_for_agent,
    reserved_for_session,
};
use crate::storage::jsonl::{
    ScanIssues, append_if_reporting, read_records, read_records_reporting, with_exclusive_read,
};
use crate::sync::auto_commit::{auto_commit_after_claim, auto_commit_after_release};

/// The folded session log plus whatever this build could not read in it.
fn load_state_reporting() -> Result<(Vec<SessionRecord>, ScanIssues)> {
    let path = sessions_path();
    if !path.exists() {
        return Ok((Vec::new(), ScanIssues::default()));
    }
    let (records, issues) = read_records_reporting::<SessionRecord>(&path)
        .with_context(|| "Failed to read sessions")?;
    Ok((fold(&records), issues))
}

/// The folded session log, refusing to proceed if any record is unreadable.
///
/// Admission fails closed: a torn or damaged record could hide a live
/// attachment or expose a retired one, and every decision below reuses an
/// identity or releases occupancy on the strength of what it reads. The
/// same check runs again immediately before each conditional append.
fn load_state() -> Result<Vec<SessionRecord>> {
    let (state, issues) = load_state_reporting()?;
    ensure_readable(&issues)?;
    Ok(state)
}

fn ensure_readable(issues: &ScanIssues) -> Result<()> {
    if issues.is_empty() {
        return Ok(());
    }
    bail!(
        "{} has {} unreadable record(s) and {} damaged field(s); refusing to change session state. Inspect with rite doctor, repair the file, then retry.",
        sessions_path().display(),
        issues.skipped.len(),
        issues.damaged.len()
    )
}

/// The occupancy claim syncs with `claims.jsonl`; the harness session id
/// must not travel with it. It stays in `local/sessions.jsonl` only.
const CLAIM_MESSAGE: &str = "live harness session";

fn ensure_local_dir() -> Result<()> {
    std::fs::create_dir_all(local_dir()).with_context(|| "Failed to create local state dir")
}

/// Append a session record only if `predicate` holds on the folded state
/// under the sessions-file lock.
fn append_session_if<F>(record: &SessionRecord, predicate: F) -> Result<bool>
where
    F: FnOnce(&[SessionRecord]) -> bool,
{
    ensure_local_dir()?;
    // The readability decision uses the same locked snapshot as the append:
    // a torn record left by a concurrent writer must not make the fold look
    // free, and a separate pre-check could be invalidated between the check
    // and the lock.
    let damaged: std::cell::RefCell<Option<ScanIssues>> = std::cell::RefCell::new(None);
    let appended = append_if_reporting(&sessions_path(), record, |existing, issues| {
        if !issues.is_empty() {
            *damaged.borrow_mut() = Some(issues.clone());
            return false;
        }
        predicate(&fold(existing))
    })
    .with_context(|| "Failed to write session record")?;
    if let Some(issues) = damaged.into_inner() {
        ensure_readable(&issues)?;
    }
    Ok(appended)
}

/// Compare-and-append on the claims log with the same fail-closed rule: the
/// predicate sees the latest state per claim id from the locked snapshot,
/// and nothing is written if any record in it was unreadable.
fn append_claim_if<F>(record: &FileClaim, predicate: F) -> Result<bool>
where
    F: FnOnce(&HashMap<Ulid, FileClaim>) -> bool,
{
    let damaged: std::cell::RefCell<Option<ScanIssues>> = std::cell::RefCell::new(None);
    let appended = append_if_reporting(&claims_path(), record, |existing, issues| {
        if !issues.is_empty() {
            *damaged.borrow_mut() = Some(issues.clone());
            return false;
        }
        predicate(&fold_claims(existing))
    })
    .with_context(|| "Failed to write claim record")?;
    if let Some(issues) = damaged.into_inner() {
        bail!(
            "{} has {} unreadable record(s); refusing to change occupancy. Repair the file, then retry.",
            claims_path().display(),
            issues.skipped.len()
        );
    }
    Ok(appended)
}

/// Latest state per claim id.
fn latest_claims() -> Result<HashMap<Ulid, FileClaim>> {
    let path = claims_path();
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let all: Vec<FileClaim> = read_records(&path)?;
    Ok(fold_claims(&all))
}

/// Latest state per claim id. `owner` is sticky: a later record for the same
/// id that lacks it (written by a rite that predates the field) does not
/// erase it, so an old binary's extension cannot turn an owned claim into an
/// ownerless one.
fn fold_claims(existing: &[FileClaim]) -> HashMap<Ulid, FileClaim> {
    let mut latest: HashMap<Ulid, FileClaim> = HashMap::new();
    for c in existing {
        let mut next = c.clone();
        if next.owner.is_none()
            && let Some(prev) = latest.get(&c.id)
        {
            next.owner = prev.owner.clone();
        }
        latest.insert(c.id, next);
    }
    latest
}

/// The principal an owned occupancy claim is written under. Not the agent's
/// own name: the generic claim commands of any rite version select claims by
/// `agent`, so an older binary's `claims release --all` or `refresh` run as
/// the agent never matches these. Hook admission checks the pattern only, so
/// the claim still blocks a responder.
fn occupancy_principal(attachment: Ulid) -> String {
    format!("session:{attachment}")
}

/// Who holds `pattern` right now, if anyone. `claims` is the latest state
/// per claim id; expiry counts as not held even though `active` stays true.
fn holder_of(
    pattern: &str,
    claims: &HashMap<Ulid, FileClaim>,
    now: DateTime<Utc>,
) -> Option<FileClaim> {
    claims
        .values()
        .find(|c| c.active && c.expires_at > now && c.patterns.iter().any(|p| p == pattern))
        .cloned()
}

fn owned_by(claim: &FileClaim, attachment: Ulid) -> bool {
    claim.owner.as_deref() == Some(attachment.to_string().as_str())
}

/// Release `claim_id` only if it is still active and still owned by
/// `attachment`, under the claims lock. Returns whether a release was written.
fn release_owned(claim_id: Ulid, attachment: Ulid) -> Result<bool> {
    let claims = latest_claims()?;
    let Some(claim) = claims.get(&claim_id) else {
        return Ok(false);
    };
    if !claim.active || !owned_by(claim, attachment) {
        return Ok(false);
    }
    let release = claim.release();
    let released = append_claim_if(&release, |latest| {
        latest
            .get(&claim_id)
            .is_some_and(|c| c.active && owned_by(c, attachment))
    })?;
    if released {
        auto_commit_after_release(&data_dir(), &claim_id.to_string());
    }
    Ok(released)
}

/// Move ownership of `claim_id` from attachment `from` to attachment `to`,
/// keeping its remaining TTL, only if it is still the sole unexpired holder
/// of `pattern` and still owned by `from`. Used when a replacement fails or
/// is abandoned after the claim already changed hands, so the live
/// predecessor never loses occupancy. Returns whether it was written.
fn hand_back(claim_id: Ulid, from: Ulid, to: Ulid, pattern: &str) -> Result<bool> {
    let claims = latest_claims()?;
    let Some(claim) = claims.get(&claim_id) else {
        return Ok(false);
    };
    if !claim.active || claim.expires_at <= Utc::now() || !owned_by(claim, from) {
        return Ok(false);
    }
    let mut handback = claim
        .extend(ttl_remaining(claim))
        .with_owner(to.to_string());
    handback.agent = occupancy_principal(to);
    let written = append_claim_if(&handback, |latest| {
        holder_of(pattern, latest, Utc::now())
            .is_some_and(|h| h.id == claim_id && owned_by(&h, from))
    })?;
    if written {
        auto_commit_after_claim(&data_dir(), &[pattern.to_string()]);
    }
    Ok(written)
}

/// What `occupy` did to the `agent://` claim. Every outcome is a claim owned
/// by the attachment: ownerless occupancy is refused, never adopted, because
/// it cannot give the release and renewal guarantees the attachment needs,
/// and because it is exactly what a responder hook holds while it runs.
enum Occupancy {
    /// Staked a fresh claim under `claim_id`, owned by the attachment.
    Staked(FileClaim),
    /// Extended (and re-tagged to the attachment) the claim it is entitled to.
    Extended(FileClaim),
}

/// Make `pattern` held by `agent` under claim id `claim_id`, owned by
/// `attachment`, or say why not.
///
/// Every write is a compare-and-append under the claims lock, so the
/// decision and the append see the same state:
///
/// 1. extend, only if the current unexpired holder is `claim_id` and its
///    owner is `attachment` (or `takeover_from`, during a replace);
/// 2. otherwise stake, only if nobody holds the pattern;
/// 3. otherwise report the holder as an error: another agent, another
///    attachment, or an ownerless claim of the same agent.
///
/// A lapsed claim is never revived over a newer holder, two live claims for
/// one pattern cannot be created, and an attachment that was replaced can
/// neither extend nor release what it lost.
///
/// Refusing ownerless occupancy is also what makes hook admission safe
/// across the two logs. A responder hook reads the reservation, then stakes
/// an ownerless claim under the claims lock; attach reserves under the
/// sessions lock, then occupies under the claims lock. Whichever of the two
/// claim writes comes second sees the first: the hook's stake fails because
/// the pattern is held, or attach fails here because the holder is
/// ownerless. Neither can spawn beside the other.
fn occupy(
    claim_id: Ulid,
    attachment: Ulid,
    takeover_from: Option<Ulid>,
    agent: &str,
    pattern: &str,
    ttl_secs: u64,
    message: &str,
) -> Result<Occupancy> {
    let entitled = move |c: &FileClaim| {
        c.id == claim_id
            && (owned_by(c, attachment) || takeover_from.is_some_and(|old| owned_by(c, old)))
    };

    let mut extended = FileClaim::with_message(
        occupancy_principal(attachment),
        vec![pattern.to_string()],
        ttl_secs,
        Some(message.to_string()),
    )
    .with_owner(attachment.to_string());
    extended.id = claim_id;
    extended.event = ClaimEvent::Extended;
    if append_claim_if(&extended, |latest| {
        holder_of(pattern, latest, Utc::now()).is_some_and(|h| entitled(&h))
    })? {
        auto_commit_after_claim(&data_dir(), &[pattern.to_string()]);
        return Ok(Occupancy::Extended(extended));
    }

    let mut created = FileClaim::with_message(
        occupancy_principal(attachment),
        vec![pattern.to_string()],
        ttl_secs,
        Some(message.to_string()),
    )
    .with_owner(attachment.to_string());
    created.id = claim_id;
    if append_claim_if(&created, |latest| {
        holder_of(pattern, latest, Utc::now()).is_none()
    })? {
        auto_commit_after_claim(&data_dir(), &[pattern.to_string()]);
        return Ok(Occupancy::Staked(created));
    }

    // Neither predicate held: someone holds it. Report who. A holder that
    // vanishes between the failed appends and this read is a benign retry.
    match holder_of(pattern, &latest_claims()?, Utc::now()) {
        Some(held) if held.owner.is_some() || held.agent.eq_ignore_ascii_case(agent) => {
            match &held.owner {
                None => bail!(
                    "{} is held by {} without a session owner until {}: a responder or a manual claim occupies this identity. If your harness is already running, it now overlaps that holder; stop one of them. To avoid this, reserve the identity before starting the harness: rite sessions reserve, then attach --attachment. Otherwise wait for the holder to finish, or release it with rite claims release {}",
                    pattern,
                    held.agent,
                    held.expires_at.format("%Y-%m-%d %H:%M:%S UTC"),
                    pattern
                ),
                Some(owner) => bail!(
                    "{} is owned by attachment {}; this attachment ({}) was replaced or never held it",
                    pattern,
                    owner,
                    attachment
                ),
            }
        }
        Some(held) => bail!(
            "{} is held by {} until {}; another agent occupies this identity",
            pattern,
            held.agent,
            held.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        None => bail!("{} changed hands while attaching; retry", pattern),
    }
}

/// Repair what a crash between two appends can leave behind for `agent`:
/// an `attaching` record that never committed, and an `agent://` claim whose
/// owner is no longer reserved. Both repairs are conditional, so a concurrent
/// attach that is still inside its window is left alone.
fn reconcile(agent: &str) -> Result<()> {
    let state = load_state()?;
    for r in state.iter().filter(|r| r.agent.eq_ignore_ascii_case(agent)) {
        if !r.is_abandoned() {
            continue;
        }
        let id = r.attachment_id;
        // An abandoned successor may already own the claim it was taking
        // over. Its predecessor is still live, so hand the claim back
        // before retiring the reservation; otherwise the release loop below
        // would strip occupancy from a live session.
        // An abandoned successor may already own the claim it was taking
        // over while its predecessor is still live: hand it back before
        // retiring the reservation. Only a sole unexpired holder moves.
        if let (Some(old_id), Some(claim_id)) = (r.replaces, r.claim_id)
            && state
                .iter()
                .any(|x| x.attachment_id == old_id && x.is_attached())
        {
            hand_back(claim_id, id, old_id, &occupancy_pattern(agent))?;
        }
        append_session_if(&r.detached(Some("abandoned attach".to_string())), |s| {
            s.iter().any(|x| x.attachment_id == id && x.is_abandoned())
        })?;
    }

    // A committed successor logically retires the attachment it replaced.
    // Attach detaches it in a separate append, so a crash between the two
    // can leave both attached; finish that here before anything routes.
    let state = load_state()?;
    for succ in state
        .iter()
        .filter(|r| r.is_attached() && r.agent.eq_ignore_ascii_case(agent))
    {
        if let Some(old_id) = succ.replaces
            && let Some(old) = state
                .iter()
                .find(|x| x.attachment_id == old_id && x.is_attached())
        {
            append_session_if(
                &old.detached(Some("replaced by a new attachment".to_string())),
                |s| {
                    s.iter()
                        .any(|x| x.attachment_id == old_id && x.is_attached())
                },
            )?;
        }
    }

    let state = load_state()?;
    let reserved: Vec<Ulid> = reserved_for_agent(&state, agent)
        .into_iter()
        .map(|r| r.attachment_id)
        .collect();
    let pattern = occupancy_pattern(agent);
    let now = Utc::now();
    for claim in latest_claims()?.values() {
        if !(claim.active && claim.expires_at > now)
            || !claim.patterns.iter().any(|p| p == &pattern)
        {
            continue;
        }
        let Some(owner) = claim.owner.as_deref().and_then(|o| o.parse::<Ulid>().ok()) else {
            continue;
        };
        if reserved.contains(&owner) {
            continue;
        }
        // The owner is no longer reserved. If it was a successor whose
        // predecessor is still live, the claim goes back to the predecessor
        // rather than away: a takeover that timed out or failed after the
        // claim changed hands must not strip the live session.
        let predecessor = state
            .iter()
            .find(|x| x.attachment_id == owner)
            .and_then(|x| x.replaces)
            .filter(|old| {
                state
                    .iter()
                    .any(|x| x.attachment_id == *old && x.is_attached())
            });
        match predecessor {
            Some(old) if hand_back(claim.id, owner, old, &pattern)? => {}
            _ => {
                release_owned(claim.id, owner)?;
            }
        }
    }
    Ok(())
}

/// Why a fenced commit did not happen.
enum CommitOutcome {
    Committed,
    /// The reservation is no longer pending, or the session id is taken.
    ReservationGone,
    /// The claim is not the unexpired holder owned by this attachment.
    OccupancyLost,
}

/// Commit a reservation while holding the claims lock, so the occupancy
/// decision stays true through the session append.
///
/// Under the claims lock no other process can stake, extend, or release
/// anything in `claims.jsonl`, so once the claim is seen as the unexpired
/// holder owned by `attachment`, the only thing that can change it before
/// the lock is dropped is the clock. The sequence is therefore: check, append
/// the `attached` record (under the sessions lock, nested), then check again;
/// if the claim expired during the append, retire the record before the lock
/// is released, so no `attached` record ever survives lost occupancy. The
/// reservation must still be pending and not abandoned, and the session id
/// must not be reserved elsewhere.
fn commit_fenced(
    bound: &SessionRecord,
    claim_id: Ulid,
    attachment: Ulid,
    pattern: &str,
) -> Result<CommitOutcome> {
    let session_id = bound.session.clone();
    with_exclusive_read::<FileClaim, CommitOutcome, _>(&claims_path(), |existing, issues| {
        if !issues.is_empty() {
            bail!(
                "{} has {} unreadable record(s); refusing to commit an attachment. Repair the file, then retry.",
                claims_path().display(),
                issues.skipped.len()
            );
        }
        let ours = |claims: &HashMap<Ulid, FileClaim>| {
            holder_of(pattern, claims, Utc::now())
                .is_some_and(|h| h.id == claim_id && owned_by(&h, attachment))
        };
        let snapshot = fold_claims(existing);
        if !ours(&snapshot) {
            return Ok(CommitOutcome::OccupancyLost);
        }
        let appended = append_session_if(bound, |s| {
            reserved_for_session(s, &session_id).is_none_or(|r| r.attachment_id == attachment)
                && s.iter()
                    .any(|x| x.attachment_id == attachment && x.is_pending() && !x.is_abandoned())
        })?;
        if !appended {
            return Ok(CommitOutcome::ReservationGone);
        }
        // Same snapshot: nothing could have been written to the claims log
        // while this lock is held; only expiry can have changed.
        if !ours(&snapshot) {
            append_session_if(
                &bound.detached(Some("occupancy expired during commit".to_string())),
                |s| {
                    s.iter()
                        .any(|x| x.attachment_id == attachment && x.is_attached())
                },
            )?;
            return Ok(CommitOutcome::OccupancyLost);
        }
        Ok(CommitOutcome::Committed)
    })
}

/// Seconds left on a claim, never below one, for a hand-back that keeps
/// the existing expiry rather than granting a fresh TTL.
fn ttl_remaining(claim: &FileClaim) -> u64 {
    (claim.expires_at - Utc::now()).num_seconds().max(1) as u64
}

#[derive(Debug, Serialize)]
struct ClaimSummary {
    id: Ulid,
    pattern: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct AttachOutput {
    attachment_id: Ulid,
    agent: String,
    harness: String,
    session: String,
    kind: String,
    /// The occupancy claim this attachment owns.
    claim: Option<ClaimSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaced: Option<Ulid>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
}

pub struct AttachOptions {
    pub harness: Option<String>,
    /// Name of the push adapter, resolved on the sending host; default is the harness's built-in.
    pub adapter: Option<String>,
    pub session: String,
    pub kind: String,
    pub ttl_secs: u64,
    pub replace: Option<String>,
    /// Bind the session id to a reservation made earlier with `reserve`.
    pub attachment: Option<String>,
    pub agent: Option<String>,
    pub format: OutputFormat,
}

pub struct ReserveOptions {
    pub harness: String,
    pub adapter: Option<String>,
    pub kind: String,
    pub ttl_secs: u64,
    /// How long the reservation may stay unbound before it is abandoned.
    pub window_secs: i64,
    pub agent: Option<String>,
    pub format: OutputFormat,
}

#[derive(Debug, Serialize)]
struct ReserveOutput {
    attachment_id: Ulid,
    agent: String,
    harness: String,
    kind: String,
    claim: ClaimSummary,
    pending_until: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
}

/// Reserve this agent's identity before its harness exists, so no responder
/// can start in the gap between launching the harness and learning its
/// session id. The reservation already holds the occupancy claim; `attach
/// --attachment <id> --session <sid>` binds the session id once known. An
/// unbound reservation lapses after `window_secs` and is reconciled away.
pub fn reserve(options: ReserveOptions) -> Result<()> {
    let agent = require_agent(options.agent.as_deref())?;
    let kind = SessionKind::parse(&options.kind).with_context(|| {
        format!(
            "Unknown session kind '{}': use push, stream, or pull",
            options.kind
        )
    })?;
    if (options.ttl_secs as i64) <= options.window_secs {
        bail!(
            "--ttl ({}s) must be longer than --window ({}s): the occupancy claim has to outlive the reservation it protects",
            options.ttl_secs,
            options.window_secs
        );
    }
    reconcile(&agent)?;

    let claim_id = Ulid::new();
    let pending = SessionRecord::attaching(&agent, &options.harness, "", kind.clone(), claim_id)
        .with_pending_window(options.window_secs)
        .with_adapter(options.adapter.clone());
    let agent_name = agent.clone();
    let reserved = append_session_if(&pending, |state| {
        reserved_for_agent(state, &agent_name).is_empty()
    })?;
    if !reserved {
        let state = load_state()?;
        let current = reserved_for_agent(&state, &agent);
        bail!(
            "{} is already reserved or attached{}",
            agent,
            current
                .first()
                .map(|c| format!(" (attachment {}, session {:?})", c.attachment_id, c.session))
                .unwrap_or_default()
        );
    }
    let attachment = pending.attachment_id;
    let pattern = occupancy_pattern(&agent);
    let claim = match occupy(
        claim_id,
        attachment,
        None,
        &agent,
        &pattern,
        options.ttl_secs,
        CLAIM_MESSAGE,
    ) {
        Ok(Occupancy::Staked(c) | Occupancy::Extended(c)) => c,
        Err(error) => {
            append_session_if(
                &pending.detached(Some("occupancy refused".to_string())),
                |s| {
                    s.iter()
                        .any(|x| x.attachment_id == attachment && x.is_reserved())
                },
            )?;
            return Err(error);
        }
    };
    let output = ReserveOutput {
        attachment_id: attachment,
        agent: agent.clone(),
        harness: options.harness.clone(),
        kind: kind.as_str(),
        claim: ClaimSummary {
            id: claim.id,
            pattern: pattern.clone(),
            expires_at: claim.expires_at,
        },
        pending_until: pending.pending_until.unwrap_or(pending.ts),
        advice: vec![
            format!(
                "rite sessions attach --attachment {} --session <harness session id>   # once the harness reports it",
                attachment
            ),
            format!(
                "rite sessions detach --attachment {}   # if the harness never starts",
                attachment
            ),
        ],
    };
    match options.format {
        OutputFormat::Pretty => println!(
            "{} {} reserved as {} until {}; holds {}",
            "✓".green(),
            agent.cyan(),
            attachment.to_string().dimmed(),
            output.pending_until.format("%H:%M:%S UTC"),
            pattern
        ),
        format => print!("{}", format_output(&output, format)),
    }
    Ok(())
}

/// Bind a session id to a reservation. The reservation's claim is already
/// held; this is the commit, conditional on the reservation still pending
/// and the session id not being reserved elsewhere.
fn bind_reservation(
    attachment: Ulid,
    session: &str,
    agent: &str,
    ttl_secs: u64,
) -> Result<SessionRecord> {
    let state = load_state()?;
    let Some(pending) = state
        .iter()
        .find(|r| r.attachment_id == attachment && r.is_pending() && !r.is_abandoned())
        .cloned()
    else {
        bail!(
            "Attachment {} is not a live reservation; run rite sessions reserve again",
            attachment
        );
    };
    if !pending.agent.eq_ignore_ascii_case(agent) {
        bail!(
            "Attachment {} belongs to {}, not {}",
            attachment,
            pending.agent,
            agent
        );
    }
    // Validate ownership by renewing under the claims lock: the extend
    // predicate succeeds only if this reservation's claim is the unexpired
    // holder and owned by this attachment, and it pushes expiry out so the
    // commit below runs with a fresh TTL rather than against a deadline. A
    // claim that lapsed, was released, or belongs to someone else fails
    // here; the reservation is then retired and nothing is bound.
    let pattern = occupancy_pattern(&pending.agent);
    let Some(claim_id) = pending.claim_id else {
        bail!("Reservation {} has no claim id; reserve again", attachment);
    };
    let occupancy = occupy(
        claim_id,
        attachment,
        None,
        &pending.agent,
        &pattern,
        ttl_secs,
        CLAIM_MESSAGE,
    );
    let claim = match occupancy {
        Ok(Occupancy::Extended(c)) => c,
        Ok(Occupancy::Staked(c)) => {
            // Nobody held the identity: the claim had lapsed but the
            // reservation is inside its window, which the ttl > window rule
            // makes rare. Holding it again is the right outcome.
            c
        }
        Err(error) => {
            append_session_if(
                &pending.detached(Some("occupancy lost before bind".to_string())),
                |s| {
                    s.iter()
                        .any(|x| x.attachment_id == attachment && x.is_pending())
                },
            )?;
            return Err(error.context(format!(
                "Reservation {} no longer holds {}; run rite sessions reserve again before starting the harness",
                attachment, pattern
            )));
        }
    };
    let bound = pending.committed_with_session(session);
    let outcome = commit_fenced(&bound, claim.id, attachment, &pattern)?;
    let committed = matches!(outcome, CommitOutcome::Committed);
    if let CommitOutcome::OccupancyLost = outcome {
        append_session_if(
            &pending.detached(Some("occupancy lost before bind".to_string())),
            |s| {
                s.iter()
                    .any(|x| x.attachment_id == attachment && x.is_pending())
            },
        )?;
        bail!(
            "Reservation {} lost {} before the session could be bound; run rite sessions reserve again",
            attachment,
            pattern
        );
    }
    if !committed {
        let state = load_state()?;
        if let Some(existing) = reserved_for_session(&state, session) {
            bail!(
                "Session {} is already attached to {} as attachment {}",
                session,
                existing.agent,
                existing.attachment_id
            );
        }
        // The reservation lapsed or was detached; its claim goes with it.
        if let Some(claim_id) = pending.claim_id {
            release_owned(claim_id, attachment)?;
        }
        bail!(
            "Reservation {} lapsed before the session id was bound",
            attachment
        );
    }
    Ok(bound)
}

pub fn attach(options: AttachOptions) -> Result<()> {
    let agent = require_agent(options.agent.as_deref())?;
    if options.session.trim().is_empty() {
        bail!("--session must be the harness's session id");
    }

    if let Some(attachment) = options.attachment.as_deref() {
        let id: Ulid = attachment
            .parse()
            .with_context(|| format!("'{attachment}' is not an attachment id"))?;
        reconcile(&agent)?;
        let bound = bind_reservation(id, &options.session, &agent, options.ttl_secs)?;
        let claims = latest_claims()?;
        let claim = bound.claim_id.and_then(|cid| claims.get(&cid).cloned());
        let pattern = occupancy_pattern(&agent);
        let output = AttachOutput {
            attachment_id: id,
            agent: agent.clone(),
            harness: bound.harness.clone(),
            session: options.session.clone(),
            kind: bound.kind.as_str(),
            claim: claim.map(|c| ClaimSummary {
                id: c.id,
                pattern: pattern.clone(),
                expires_at: c.expires_at,
            }),
            replaced: None,
            advice: vec![format!(
                "rite sessions detach --session {}   # from the harness's SessionEnd hook",
                options.session
            )],
        };
        match options.format {
            OutputFormat::Pretty => println!(
                "{} {} bound to {} session {} as {}",
                "✓".green(),
                agent.cyan(),
                bound.harness,
                options.session,
                id.to_string().dimmed()
            ),
            format => print!("{}", format_output(&output, format)),
        }
        return Ok(());
    }

    let harness = options
        .harness
        .clone()
        .with_context(|| "--harness is required unless --attachment names a reservation")?;
    let kind = SessionKind::parse(&options.kind).with_context(|| {
        format!(
            "Unknown session kind '{}': use push, stream, or pull",
            options.kind
        )
    })?;
    let replace: Option<Ulid> = match options.replace.as_deref() {
        Some(id) => Some(
            id.parse()
                .with_context(|| format!("'{id}' is not an attachment id"))?,
        ),
        None => None,
    };

    reconcile(&agent)?;

    // If replacing, the successor inherits the old claim id and re-tags its
    // owner under the claims lock, so occupancy is never dropped. Otherwise
    // a fresh id. Not `unwrap_or_default`: the default ULID is the nil id.
    let inherited = replace.and_then(|id| {
        load_state()
            .ok()?
            .iter()
            .find(|r| r.attachment_id == id && r.is_attached())
            .and_then(|old| old.claim_id)
    });
    let claim_id = match inherited {
        Some(id) => id,
        None => Ulid::new(),
    };

    // 1. Reserve. Uniqueness is decided under the sessions-file lock: no
    // other reservation for this session id, and none for this agent except
    // the attachment being replaced. A loser writes nothing at all.
    let mut pending =
        SessionRecord::attaching(&agent, &harness, &options.session, kind.clone(), claim_id);
    if let Some(old) = replace {
        pending = pending.replacing(old);
    }
    pending = pending.with_adapter(options.adapter.clone());
    let (session_id, agent_name) = (options.session.clone(), agent.clone());
    let replaced_record: std::cell::RefCell<Option<SessionRecord>> = std::cell::RefCell::new(None);
    let reserved = append_session_if(&pending, |state| {
        if reserved_for_session(state, &session_id).is_some() {
            return false;
        }
        let others = reserved_for_agent(state, &agent_name);
        match (others.as_slice(), replace) {
            ([], _) => true,
            ([current], Some(id)) if current.attachment_id == id && current.is_attached() => {
                *replaced_record.borrow_mut() = Some((*current).clone());
                true
            }
            _ => false,
        }
    })?;

    if !reserved {
        // Explain from a fresh read; nothing was written.
        let state = load_state()?;
        if let Some(existing) = reserved_for_session(&state, &options.session) {
            bail!(
                "Session {} is already {} to {} as attachment {}. Detach it first: rite sessions detach --session {}",
                options.session,
                if existing.is_pending() {
                    "being attached"
                } else {
                    "attached"
                },
                existing.agent,
                existing.attachment_id,
                options.session
            );
        }
        let others = reserved_for_agent(&state, &agent);
        match (others.first(), replace) {
            (Some(current), Some(id)) => bail!(
                "--replace names {}, but {}'s live attachment is {} (session {})",
                id,
                agent,
                current.attachment_id,
                current.session
            ),
            (Some(current), None) => bail!(
                "{} is already attached to {} session {} as attachment {}. Pass --replace {} to take over, or detach it first.",
                agent,
                current.harness,
                current.session,
                current.attachment_id,
                current.attachment_id
            ),
            (None, Some(id)) => bail!(
                "--replace names {}, but {} has no live attachment",
                id,
                agent
            ),
            (None, None) => bail!("attach lost a race and could not determine why; retry"),
        }
    }
    let attachment = pending.attachment_id;
    let abort = |note: &str| -> Result<()> {
        append_session_if(&pending.detached(Some(note.to_string())), |s| {
            s.iter()
                .any(|x| x.attachment_id == attachment && x.is_reserved())
        })?;
        Ok(())
    };

    // 2. Occupy: the ordinary claim every responder hook already gates on,
    // owned by this attachment.
    let replaced = replaced_record.into_inner();
    let pattern = occupancy_pattern(&agent);
    let occupancy = match occupy(
        claim_id,
        attachment,
        replaced.as_ref().map(|r| r.attachment_id),
        &agent,
        &pattern,
        options.ttl_secs,
        CLAIM_MESSAGE,
    ) {
        Ok(o) => o,
        Err(error) => {
            abort("occupancy refused")?;
            return Err(error);
        }
    };

    // 3. Commit, fenced by the claims lock, only if the reservation is still
    // ours and the claim is still ours. A detach that landed in between
    // wins: undo the claim and report.
    let outcome = commit_fenced(&pending.committed(), claim_id, attachment, &pattern)?;
    let committed = matches!(outcome, CommitOutcome::Committed);
    if !committed {
        // A takeover that cannot commit gives the claim back to the live
        // predecessor; a fresh attach releases what it staked.
        let handed_back = match &replaced {
            Some(old) => hand_back(claim_id, attachment, old.attachment_id, &pattern)?,
            None => false,
        };
        if !handed_back {
            release_owned(claim_id, attachment)?;
        }
        bail!(
            "Attachment {} was detached before it could be committed",
            attachment
        );
    }

    // 4. Retire the replaced attachment. Its claim now belongs to us, so this
    // is a session write only; a stale detach of it can no longer release.
    if let Some(old) = &replaced {
        let old_id = old.attachment_id;
        append_session_if(
            &old.detached(Some("replaced by a new attachment".to_string())),
            |s| {
                s.iter()
                    .any(|x| x.attachment_id == old_id && x.is_attached())
            },
        )?;
    }

    let mut advice = vec![format!(
        "rite sessions detach --session {}   # from the harness's SessionEnd hook",
        options.session
    )];
    let claim = match occupancy {
        Occupancy::Staked(c) | Occupancy::Extended(c) => {
            advice.push(format!(
                "rite sessions renew --attachment {}   # from the bridge, before the claim expires",
                attachment
            ));
            Some(c)
        }
    };

    let output = AttachOutput {
        attachment_id: attachment,
        agent: agent.clone(),
        harness: harness.clone(),
        session: options.session.clone(),
        kind: kind.as_str(),
        claim: claim.map(|c| ClaimSummary {
            id: c.id,
            pattern: pattern.clone(),
            expires_at: c.expires_at,
        }),
        replaced: replaced.map(|r| r.attachment_id),
        advice,
    };

    match options.format {
        OutputFormat::Pretty => {
            println!(
                "{} {} attached to {} session {} as {}",
                "✓".green(),
                agent.cyan(),
                harness,
                options.session,
                attachment.to_string().dimmed()
            );
            if let Some(c) = &output.claim {
                println!(
                    "  holds {} until {}",
                    c.pattern,
                    c.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
                );
            }
            if let Some(old) = output.replaced {
                println!("  replaced attachment {}", old);
            }
        }
        format => print!("{}", format_output(&output, format)),
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct DetachOutput {
    /// The attachment that was retired, or `null` for a no-op.
    detached: Option<Ulid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    /// The claim released, or `null` if this attachment did not own one.
    released_claim: Option<Ulid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
}

/// Detach by harness session id or attachment id. Needs no agent identity:
/// a SessionEnd hook knows only the session id. A session that was never
/// attached, or is already detached, is a no-op with exit 0 — that is the
/// late-placeholder case, and guessing would retire the wrong attachment.
///
/// The session write is conditional on the record still being reserved, and
/// the claim release is conditional on the claim still being owned by this
/// attachment, so a detach that read a record before it was replaced does
/// nothing to the successor.
pub fn detach(
    session: Option<String>,
    attachment: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let state = load_state()?;
    let current = match (session.as_deref(), attachment.as_deref()) {
        (Some(s), _) => reserved_for_session(&state, s),
        (None, Some(a)) => {
            let id: Ulid = a
                .parse()
                .with_context(|| format!("'{a}' is not an attachment id"))?;
            state
                .iter()
                .find(|r| r.attachment_id == id && r.is_reserved())
        }
        (None, None) => bail!("Pass --session <harness session id> or --attachment <id>"),
    }
    .cloned();

    if let Some(current) = &current {
        if current.is_unknown() {
            bail!(
                "Attachment {} has a lifecycle event this rite does not understand; it stays reserved. Upgrade rite before detaching it.",
                current.attachment_id
            );
        }
        reconcile(&current.agent)?;
    }

    let output = match current {
        None => DetachOutput {
            detached: None,
            agent: None,
            session: session.clone(),
            released_claim: None,
            reason: Some("no live attachment for that session; nothing to retire".to_string()),
            advice: vec!["rite sessions list --all".to_string()],
        },
        Some(current) => {
            let id = current.attachment_id;
            let retired = append_session_if(&current.detached(None), |s| {
                s.iter().any(|x| x.attachment_id == id && x.is_reserved())
            })?;
            if !retired {
                DetachOutput {
                    detached: None,
                    agent: Some(current.agent.clone()),
                    session: Some(current.session.clone()),
                    released_claim: None,
                    reason: Some("attachment was already retired".to_string()),
                    advice: vec!["rite sessions list --all".to_string()],
                }
            } else {
                let released = match current.claim_id {
                    Some(claim_id) if release_owned(claim_id, id)? => Some(claim_id),
                    _ => None,
                };
                DetachOutput {
                    detached: Some(id),
                    agent: Some(current.agent.clone()),
                    session: Some(current.session.clone()),
                    released_claim: released,
                    reason: None,
                    advice: vec![],
                }
            }
        }
    };

    match format {
        OutputFormat::Pretty => match (&output.detached, &output.agent) {
            (Some(id), Some(agent)) => {
                println!(
                    "{} detached {} ({})",
                    "✓".green(),
                    agent.cyan(),
                    id.to_string().dimmed()
                );
                if output.released_claim.is_some() {
                    println!("  released {}", occupancy_pattern(agent));
                }
            }
            _ => println!("{} no live attachment; nothing to do", "·".dimmed()),
        },
        format => print!("{}", format_output(&output, format)),
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct RenewOutput {
    attachment_id: Ulid,
    agent: String,
    claim: Option<ClaimSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
}

/// Extend the occupancy claim an attachment owns. Called by the bridge that
/// owns the session, independently of model activity, so an idle session
/// stays occupied and a dead bridge lets it lapse.
///
/// Ownership is re-validated under the claims lock: a lapsed claim is
/// re-staked only if nobody else took the identity, never extended over a
/// newer holder, and never touched once a replacement owns it. If the
/// attachment was detached while this ran, whatever was staked is released.
pub fn renew(attachment: &str, ttl_secs: u64, format: OutputFormat) -> Result<()> {
    let id: Ulid = attachment
        .parse()
        .with_context(|| format!("'{attachment}' is not an attachment id"))?;
    let state = load_state()?;
    let Some(current) = state
        .iter()
        .find(|r| r.attachment_id == id && r.is_attached())
    else {
        bail!("No live attachment {}", id);
    };
    reconcile(&current.agent)?;
    let Some(claim_id) = current.claim_id else {
        bail!(
            "Attachment {} has no claim id; detach it and attach again",
            id
        );
    };
    let pattern = occupancy_pattern(&current.agent);
    let occupancy = occupy(
        claim_id,
        id,
        None,
        &current.agent,
        &pattern,
        ttl_secs,
        CLAIM_MESSAGE,
    )?;

    // A detach may have landed between the read above and the write. Its
    // release could not see what we just wrote, so undo it here.
    let still_live = load_state()?
        .iter()
        .any(|r| r.attachment_id == id && r.is_attached());
    if !still_live {
        release_owned(claim_id, id)?;
        bail!(
            "Attachment {} was detached while renewing; nothing kept",
            id
        );
    }

    let claim = match occupancy {
        Occupancy::Staked(c) | Occupancy::Extended(c) => c,
    };
    let output = RenewOutput {
        attachment_id: id,
        agent: current.agent.clone(),
        claim: Some(ClaimSummary {
            id: claim.id,
            pattern: pattern.clone(),
            expires_at: claim.expires_at,
        }),
        advice: vec![],
    };
    match format {
        OutputFormat::Pretty => println!(
            "{} {} occupied until {}",
            "✓".green(),
            current.agent.cyan(),
            claim.expires_at.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        format => print!("{}", format_output(&output, format)),
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub attachment_id: Ulid,
    pub agent: String,
    pub harness: String,
    pub session: String,
    pub kind: String,
    /// `attached`, `attaching`, or `detached`.
    pub state: String,
    pub attached: bool,
    pub since: DateTime<Utc>,
    /// `held`, `lapsed`, or `none`: the state of the `agent://` claim this
    /// attachment owns.
    pub occupancy: String,
}

#[derive(Debug, Serialize)]
struct ListOutput {
    sessions: Vec<SessionInfo>,
    /// Records in the session log this build could not read. Non-zero means
    /// state-changing session commands are refused until it is repaired.
    #[serde(skip_serializing_if = "is_zero")]
    unreadable_records: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
}

/// Session infos for every live attachment, for other commands to embed.
pub fn live_sessions() -> Result<Vec<SessionInfo>> {
    let state = load_state()?;
    let claims = latest_claims()?;
    let now = Utc::now();
    Ok(state
        .iter()
        .filter(|r| r.is_attached())
        .map(|r| info_for(r, &claims, now))
        .collect())
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn info_for(
    r: &SessionRecord,
    claims: &HashMap<Ulid, FileClaim>,
    now: DateTime<Utc>,
) -> SessionInfo {
    let pattern = occupancy_pattern(&r.agent);
    let owned = r.claim_id.and_then(|cid| claims.get(&cid));
    let occupancy = if !r.is_attached() {
        "none"
    } else if holder_of(&pattern, claims, now).is_some_and(|h| Some(h.id) == r.claim_id) {
        "held"
    } else if owned.is_some_and(|c| c.active) {
        "lapsed"
    } else {
        "none"
    };
    SessionInfo {
        attachment_id: r.attachment_id,
        agent: r.agent.clone(),
        harness: r.harness.clone(),
        session: if r.session.is_empty() {
            "(reserved)".to_string()
        } else {
            r.session.clone()
        },
        kind: r.kind.as_str(),
        state: if r.is_attached() {
            "attached"
        } else if r.is_pending() {
            "attaching"
        } else {
            "detached"
        }
        .to_string(),
        attached: r.is_attached(),
        since: r.ts,
        occupancy: occupancy.to_string(),
    }
}

pub fn list(
    name: Option<String>,
    all: bool,
    agent: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    let filter = name.or_else(|| resolve_agent(agent)).filter(|_| !all);
    let (state, issues) = load_state_reporting()?;
    if !issues.is_empty() {
        eprintln!(
            "warning: {} unreadable record(s) and {} damaged field(s) in {}; attach, detach, and renew are refused until it is repaired",
            issues.skipped.len(),
            issues.damaged.len(),
            sessions_path().display()
        );
    }
    let claims = latest_claims()?;
    let now = Utc::now();
    let sessions: Vec<SessionInfo> = state
        .iter()
        .filter(|r| all || r.is_attached())
        .filter(|r| {
            filter
                .as_deref()
                .is_none_or(|f| r.agent.eq_ignore_ascii_case(f))
        })
        .map(|r| info_for(r, &claims, now))
        .collect();

    let advice = if sessions.is_empty() {
        vec!["rite sessions attach --harness <h> --session <id>".to_string()]
    } else {
        vec![]
    };
    let output = ListOutput {
        sessions,
        unreadable_records: issues.skipped.len(),
        advice,
    };

    match format {
        OutputFormat::Pretty => {
            if output.sessions.is_empty() {
                println!("No live sessions.");
                return Ok(());
            }
            for s in &output.sessions {
                let mark = if s.attached {
                    "●".green()
                } else {
                    "○".dimmed()
                };
                println!(
                    "  {} {:<20} {:<7} {:<7} {:<9} {:<7} {}  {}",
                    mark,
                    s.agent.cyan(),
                    s.harness,
                    s.kind,
                    s.state,
                    s.occupancy,
                    s.session,
                    s.attachment_id.to_string().dimmed()
                );
            }
        }
        OutputFormat::Text => {
            for s in &output.sessions {
                println!(
                    "{}  {}  {}  {}  {}  {}  {}",
                    s.agent, s.harness, s.kind, s.state, s.occupancy, s.session, s.attachment_id
                );
            }
        }
        format => print!("{}", format_output(&output, format)),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Push-at-send delivery
//
// `rite send` calls this after writing a message. Nothing runs between rite
// commands: the sender's own process pushes the message into the live
// push-kind session of every agent the message addresses.
//
// What runs is decided by the *sending host*, never by the recipient. A
// session record names an adapter; the command behind that name comes from
// this host's `local/adapters.json` or the built-in table (`codex`). That
// file has the same trust as `hooks.jsonl`: whoever can write the data
// directory chooses what this host executes on a send. Records that arrive
// from anywhere else are never acted on: delivery refuses to run while
// `local/**` is tracked by sync, and `sync pull` quarantines any `local/**`
// a remote brings in.
//
// A push result never evicts a session. No adapter result can prove that
// every process it started has finished (a descendant can leave the
// process group and close its descriptors), and an eviction taken on an
// unproven result would release occupancy while something can still act.
// Occupancy ends only through the harness's SessionEnd hook, an explicit
// `rite sessions detach`, or a lapsed reservation. A push reports what
// happened, including the adapter's own claim that the session is gone
// (`session_gone`), and a launcher may act on that report with `detach`.
//
// Every adapter runs in its own process group with a minimal environment
// (PATH, HOME, USER, LANG, TERM, and the RITE_* fields), in the data
// directory's `local/`, with a bounded run time and a bounded stderr drain.
// The group is killed on every outcome and the leader reaped, as hygiene,
// not as proof. `--no-hooks` and `!nohooks` suppress delivery as they
// suppress hooks.

/// Outcome of one push attempt, reported in `rite send`'s envelope.
#[derive(Debug, Clone, Serialize)]
pub struct Delivery {
    pub agent: String,
    pub session: String,
    pub attachment_id: Ulid,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The adapter reported that the session no longer exists (Codex: no
    /// such thread; any adapter: exit `SESSION_GONE_EXIT`). A report, not an
    /// action: the attachment is kept. Detach it if you trust the report.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub session_gone: bool,
}

/// How long one push command may run.
const PUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to wait for the adapter's stderr to close after its group was
/// killed.
const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The largest body pushed into a harness. Larger messages stay on the bus
/// for the agent to read with `rite inbox`; they are not squeezed into an
/// argument vector.
const PUSH_MAX_BYTES: usize = 64 * 1024;

/// An adapter exits with this to report that the session no longer exists.
/// The report is passed on in `Delivery::session_gone`; nothing is evicted.
pub const SESSION_GONE_EXIT: i32 = 66;

/// The host's adapter table: configured entries over the built-ins.
fn load_adapters() -> AdapterTable {
    let mut table = builtin_adapters();
    let path = adapters_path();
    if !path.exists() {
        return table;
    }
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str::<AdapterTable>(&s).map_err(|e| e.to_string()))
    {
        Ok(configured) => table.extend(configured),
        Err(e) => eprintln!(
            "warning: {} is unreadable ({}); only built-in adapters are available",
            path.display(),
            e
        ),
    }
    table
}

/// The text a pushed message arrives as: provenance and the reply anchor on
/// the first line, then the body, exactly what the bridge prototype sent.
fn render_envelope(msg: &Message, channel: &str, reply_target: &str, route: &str) -> String {
    format!(
        "[rite] channel={} from={} id={} reply_target={} route={}\n{}",
        channel, msg.agent, msg.id, reply_target, route, msg.body
    )
}

/// Push `msg` into the live push session of every agent it addresses. Never
/// fails the send: problems come back as `Delivery { ok: false }`.
pub fn push_deliveries(msg: &Message, channel: &str, sender: &str) -> Vec<Delivery> {
    let (state, issues) = match load_state_reporting() {
        Ok(x) => x,
        Err(_) => return Vec::new(),
    };
    if !issues.is_empty() {
        eprintln!(
            "warning: {} has unreadable records; not pushing this message into live sessions",
            sessions_path().display()
        );
        return Vec::new();
    }
    // One target per agent: the newest live attachment, so a replacement
    // in flight never receives the same message twice.
    let mut agents: Vec<String> = state
        .iter()
        .filter(|r| r.is_attached() && !r.agent.eq_ignore_ascii_case(sender))
        .map(|r| r.agent.to_lowercase())
        .collect();
    agents.sort();
    agents.dedup();
    if agents.is_empty() {
        return Vec::new();
    }
    // Fail closed: a tracked local/ may have come from a remote.
    if crate::sync::git::local_state_is_tracked(&data_dir()) {
        eprintln!(
            "warning: local/ is tracked by sync, so its session records may not be this host's; not pushing. Run rite doctor."
        );
        return Vec::new();
    }
    let adapters = load_adapters();

    let mut out = Vec::new();
    for agent in agents {
        let Some(r) = active_for_agent(&state, &agent) else {
            continue;
        };
        let Some(argv) = r.push_command(&adapters) else {
            if r.kind == SessionKind::Push {
                eprintln!(
                    "warning: no adapter named {:?} on this host for {}; not pushing",
                    r.adapter_name(),
                    r.agent
                );
            }
            continue;
        };
        let filter = MentionFilter::new(&r.agent, true, vec![]);
        let Some((route, reply_target)) = filter.route(msg, channel) else {
            continue;
        };
        let route = route.as_str();
        let rendered = render_envelope(msg, channel, &reply_target, route);
        if rendered.len() > PUSH_MAX_BYTES {
            out.push(Delivery {
                agent: r.agent.clone(),
                session: r.session.clone(),
                attachment_id: r.attachment_id,
                ok: false,
                error: Some(format!(
                    "message is {} bytes, over the {} byte push limit; left on the bus",
                    rendered.len(),
                    PUSH_MAX_BYTES
                )),
                session_gone: false,
            });
            continue;
        }
        let subst = |arg: &str| {
            arg.replace("{id}", &msg.id.to_string())
                .replace("{channel}", channel)
                .replace("{from}", &msg.agent)
                .replace("{reply_target}", &reply_target)
                .replace("{route}", route)
                .replace("{session}", &r.session)
                .replace("{body}", &msg.body)
                .replace("{rendered}", &rendered)
        };
        let args: Vec<String> = argv.iter().map(|a| subst(a)).collect();
        let is_codex = r.adapter_name() == "codex";
        let (error, gone) = match run_push(
            &args,
            msg,
            channel,
            &reply_target,
            route,
            &r.session,
            is_codex,
        ) {
            Ok(()) => (None, false),
            Err(PushError {
                reason,
                session_gone,
            }) => (Some(reason), session_gone),
        };
        if let Some(reason) = &error {
            eprintln!(
                "warning: push into {} session {} failed ({}){}; attachment kept",
                r.agent,
                r.session,
                reason,
                if gone {
                    "; the adapter reports the session gone"
                } else {
                    ""
                }
            );
        }
        out.push(Delivery {
            agent: r.agent.clone(),
            session: r.session.clone(),
            attachment_id: r.attachment_id,
            ok: error.is_none(),
            error,
            session_gone: gone,
        });
    }
    out
}

struct PushError {
    reason: String,
    /// What the adapter said, passed on as a report.
    session_gone: bool,
}

impl PushError {
    fn failed(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            session_gone: false,
        }
    }
}

/// Does this stderr from the Codex adapter mean the thread is gone?
/// `codex queue` reports an unknown thread as `no rollout found for thread id`.
fn codex_says_gone(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("no rollout found") || s.contains("thread not found") || s.contains("unknown thread")
}

/// Kill a whole process group and report whether the kernel accepted it.
/// The child was started with `process_group(0)`, so its pgid is its pid.
fn kill_group(pid: u32) -> std::io::Result<()> {
    // SAFETY: killpg has no memory-safety preconditions; it only sends a
    // signal to a process group id we created.
    let rc = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    // ESRCH: nothing left in the group, which is the outcome we wanted.
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

/// Run one push command: own process group, minimal environment, fixed
/// working directory, bounded stderr drained concurrently, bounded run
/// time. The group is killed on every outcome. The result is a report;
/// see the module comment for why it never evicts.
#[allow(clippy::too_many_arguments)]
fn run_push(
    args: &[String],
    msg: &Message,
    channel: &str,
    reply_target: &str,
    route: &str,
    session: &str,
    is_codex: bool,
) -> std::result::Result<(), PushError> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let Some((program, rest)) = args.split_first() else {
        return Err(PushError::failed("empty push command"));
    };
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .env_clear()
        .env("RITE_MESSAGE_ID", msg.id.to_string())
        .env("RITE_CHANNEL", channel)
        .env("RITE_FROM", &msg.agent)
        .env("RITE_REPLY_TARGET", reply_target)
        .env("RITE_ROUTE", route)
        .env("RITE_SESSION", session)
        .env("RITE_BODY", &msg.body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    for key in ["PATH", "HOME", "USER", "LANG", "TERM"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    if let Ok(dir) = std::env::var("RITE_DATA_DIR") {
        cmd.env("RITE_DATA_DIR", dir);
    }
    let _ = std::fs::create_dir_all(local_dir());
    cmd.current_dir(local_dir());

    let mut child = cmd
        .spawn()
        .map_err(|e| PushError::failed(format!("could not run {}: {}", program, e)))?;
    let pid = child.id();

    // Drain stderr concurrently into a bounded buffer; the result comes
    // back over a channel so the wait for it can be bounded too.
    let stderr_pipe = child.stderr.take();
    let (drain_tx, drain_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let mut chunk = [0u8; 1024];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 4096 {
                    let cut = buf.len() - 4096;
                    buf.drain(..cut);
                }
            }
        }
        let _ = drain_tx.send(String::from_utf8_lossy(&buf).to_string());
    });

    // Wait for the leader, bounded.
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                let _ = kill_group(pid);
                let _ = child.wait();
                return Err(PushError::failed(format!("waiting on {}: {}", program, e)));
            }
        }
        if started.elapsed() > PUSH_TIMEOUT {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    // Whatever happened to the leader, the group ends here: hygiene, not
    // proof. A descendant that left the group is beyond it, which is why no
    // result below ever evicts the session.
    if let Err(e) = kill_group(pid) {
        eprintln!("warning: could not kill adapter process group {pid}: {e}");
    }
    let status = match status {
        Some(s) => Some(s),
        None => {
            // Reap the leader with a bounded wait; it may still be alive if
            // the group kill failed.
            let deadline = std::time::Instant::now() + DRAIN_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if std::time::Instant::now() > deadline => break,
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
            None
        }
    };
    let stderr = drain_rx.recv_timeout(DRAIN_TIMEOUT).unwrap_or_default();
    let detail = stderr.lines().last().unwrap_or("").trim().to_string();

    let Some(status) = status else {
        return Err(PushError::failed(format!(
            "{} timed out after {}s; process group killed",
            program,
            PUSH_TIMEOUT.as_secs()
        )));
    };
    if status.success() {
        return Ok(());
    }
    let reason = format!(
        "{} exited with {}{}",
        program,
        status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into()),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {}", detail)
        }
    );
    let session_gone =
        status.code() == Some(SESSION_GONE_EXIT) || (is_codex && codex_says_gone(&stderr));
    Err(PushError {
        reason,
        session_gone,
    })
}
