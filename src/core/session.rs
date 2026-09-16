//! Live harness sessions: which agent is reachable in which running harness.
//!
//! An *attachment* binds one rite agent to one exact harness session (a
//! Claude Code or Codex session id). It is the fact the routing layer needs
//! and that nothing else records: registration says an agent exists,
//! presence says it ran a rite command recently, a claim says it is occupied.
//! None of them says "there is a live process that can take a message for
//! this agent, and this is its id".
//!
//! Attachments are host-local and append-only (`local/sessions.jsonl`). They
//! never sync: a session id only means something on the machine that hosts
//! the harness. The file's line order is the fold order — one host, one file,
//! one writer lock — so no separate sequence number is needed.
//!
//! Attaching stakes the ordinary `agent://<name>` claim, which is what the
//! deployed responder hooks already gate on. Detaching releases only the
//! claim that attachment staked. Both are recorded here so a detach for a
//! session that was never attached is a no-op instead of a guess: a harness
//! can emit a late SessionEnd for a placeholder thread a minute after the real
//! thread started, and that must not retire the real one.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ulid::Ulid;

use crate::core::wire::{self, ForwardCompatible};

/// How long an `attaching` record may stay uncommitted before any session
/// command treats it as an abandoned attach and retires it. An attach that
/// takes longer than this has crashed between its two appends.
pub const PENDING_TTL_SECS: i64 = 30;

/// The claim pattern that marks an agent identity as occupied.
pub fn occupancy_pattern(agent: &str) -> String {
    format!("agent://{agent}")
}

/// One event in an attachment's life. The latest record per
/// `attachment_id` is the attachment's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub ts: DateTime<Utc>,

    /// Immutable id of the attachment this record belongs to.
    pub attachment_id: Ulid,

    /// rite agent name.
    pub agent: String,

    /// Which harness hosts the session: `claude`, `codex`, or anything else.
    /// Informational.
    pub harness: String,

    /// The harness's own session id, verbatim.
    pub session: String,

    /// How text enters the session.
    pub kind: SessionKind,

    pub event: SessionEvent,

    /// The `agent://` claim this attachment staked, if it staked one. Absent
    /// when the claim was already held by the same agent, so detach releases
    /// nothing it did not take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<Ulid>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    /// For a successor created by `attach --replace`: the attachment it
    /// takes over from. Recorded so a crash after the claim changed hands but
    /// before the successor committed can be undone: reconciliation hands the
    /// claim back to this attachment instead of releasing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<Ulid>,

    /// For a pending record: when the reservation lapses if it is never
    /// committed. Absent means `ts` plus [`PENDING_TTL_SECS`]. A launcher's
    /// `rite sessions reserve` sets a longer window because the harness it
    /// is about to start may take a while to report its session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_until: Option<DateTime<Utc>>,
}

impl SessionRecord {
    /// The pending record an attach writes first. It reserves the agent and
    /// session id under the sessions lock; it becomes live only once
    /// [`SessionRecord::committed`] follows it, after the claim exists.
    pub fn attaching(
        agent: impl Into<String>,
        harness: impl Into<String>,
        session: impl Into<String>,
        kind: SessionKind,
        claim_id: Ulid,
    ) -> Self {
        Self {
            ts: Utc::now(),
            attachment_id: Ulid::new(),
            agent: agent.into(),
            harness: harness.into(),
            session: session.into(),
            kind,
            event: SessionEvent::Attaching,
            claim_id: Some(claim_id),
            note: None,
            replaces: None,
            pending_until: None,
        }
    }

    /// Give this reservation an explicit window instead of the default.
    pub fn with_pending_window(mut self, secs: i64) -> Self {
        self.pending_until = Some(self.ts + chrono::Duration::seconds(secs));
        self
    }

    /// The commit record for a reservation made before the session id was
    /// known (`rite sessions reserve`), binding it now.
    pub fn committed_with_session(&self, session: impl Into<String>) -> Self {
        Self {
            session: session.into(),
            ..self.committed()
        }
    }

    /// Mark this reservation as a takeover of `old`.
    pub fn replacing(mut self, old: Ulid) -> Self {
        self.replaces = Some(old);
        self
    }

    /// A record that is live from the start. Tests and callers that already
    /// hold occupancy use it; `attach` goes through [`Self::attaching`].
    pub fn attached(
        agent: impl Into<String>,
        harness: impl Into<String>,
        session: impl Into<String>,
        kind: SessionKind,
        claim_id: Option<Ulid>,
    ) -> Self {
        Self {
            ts: Utc::now(),
            attachment_id: Ulid::new(),
            agent: agent.into(),
            harness: harness.into(),
            session: session.into(),
            kind,
            event: SessionEvent::Attached,
            claim_id,
            note: None,
            replaces: None,
            pending_until: None,
        }
    }

    /// The commit record for a pending attach.
    pub fn committed(&self) -> Self {
        Self {
            ts: Utc::now(),
            event: SessionEvent::Attached,
            note: None,
            ..self.clone()
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.event, SessionEvent::Attaching)
    }

    /// Pending or attached: either reserves the agent and the session id.
    /// Pending, attached, or an event this build does not know. An unknown
    /// latest event was written by a newer rite and may mean "still live",
    /// so it reserves the identity until a build that understands it says
    /// otherwise: failing open here would let an older binary attach or
    /// spawn a second agent beside a live session after an upgrade.
    pub fn is_reserved(&self) -> bool {
        self.is_pending() || self.is_attached() || self.is_unknown()
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self.event, SessionEvent::Unknown(_))
    }

    /// A pending record older than [`PENDING_TTL_SECS`]: its attach crashed
    /// between the two appends.
    pub fn is_abandoned(&self) -> bool {
        if !self.is_pending() {
            return false;
        }
        let until = self
            .pending_until
            .unwrap_or_else(|| self.ts + chrono::Duration::seconds(PENDING_TTL_SECS));
        Utc::now() > until
    }

    /// The detach record for this attachment.
    pub fn detached(&self, note: Option<String>) -> Self {
        Self {
            ts: Utc::now(),
            event: SessionEvent::Detached,
            note,
            ..self.clone()
        }
    }

    pub fn is_attached(&self) -> bool {
        matches!(self.event, SessionEvent::Attached)
    }
}

/// How a message gets into the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", remote = "Self")]
pub enum SessionKind {
    /// A local adapter runs a command for the exact session (Codex: `codex queue`).
    Push,
    /// A connected adapter writes harness notifications (Claude channel server).
    Stream,
    /// Nothing attached; a finishing-turn hook takes pending work.
    Pull,
    /// A kind written by a newer rite, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl SessionKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "push" => Some(Self::Push),
            "stream" => Some(Self::Stream),
            "pull" => Some(Self::Pull),
            _ => None,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::Push => "push".to_string(),
            Self::Stream => "stream".to_string(),
            Self::Pull => "pull".to_string(),
            Self::Unknown(v) => v.as_str().unwrap_or("unknown").to_string(),
        }
    }
}

impl ForwardCompatible for SessionKind {
    const WIRE_NAME: &'static str = "session kind";
    const KNOWN_TAGS: &'static [&'static str] = &["push", "stream", "pull"];

    fn tag(value: &Value) -> Option<&str> {
        wire::external_tag(value)
    }

    fn parse_known(value: &Value) -> Result<Self, serde_json::Error> {
        SessionKind::deserialize(value)
    }

    fn unknown(value: Value) -> Self {
        SessionKind::Unknown(value)
    }

    fn is_unknown(&self) -> bool {
        matches!(self, SessionKind::Unknown(_))
    }
}

impl Serialize for SessionKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SessionKind::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for SessionKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        wire::deserialize(deserializer)
    }
}

/// Why a [`SessionRecord`] was written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", remote = "Self")]
pub enum SessionEvent {
    /// Reserved, claim not yet staked. Not live.
    Attaching,
    Attached,
    Detached,
    /// An event written by a newer rite, kept verbatim. Not treated as
    /// attached: an unknown event must not keep an identity occupied.
    #[serde(untagged)]
    Unknown(Value),
}

impl ForwardCompatible for SessionEvent {
    const WIRE_NAME: &'static str = "session event";
    const KNOWN_TAGS: &'static [&'static str] = &["attaching", "attached", "detached"];

    fn tag(value: &Value) -> Option<&str> {
        wire::external_tag(value)
    }

    fn parse_known(value: &Value) -> Result<Self, serde_json::Error> {
        SessionEvent::deserialize(value)
    }

    fn unknown(value: Value) -> Self {
        SessionEvent::Unknown(value)
    }

    fn is_unknown(&self) -> bool {
        matches!(self, SessionEvent::Unknown(_))
    }
}

impl Serialize for SessionEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        SessionEvent::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for SessionEvent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        wire::deserialize(deserializer)
    }
}

/// Whether `agent` is reserved by a live or pending attachment on this host.
///
/// Used by hook admission so a responder does not cold-spawn beside a
/// session that is between its reservation and its claim. Fails closed: a
/// session log this build cannot read cleanly counts as reserved, because
/// spawning beside a possibly live session is the worse error.
pub fn agent_is_reserved(agent: &str) -> bool {
    use crate::core::project::sessions_path;
    use crate::storage::jsonl::read_records_reporting;
    let path = sessions_path();
    if !path.exists() {
        return false;
    }
    match read_records_reporting::<SessionRecord>(&path) {
        Ok((records, issues)) if issues.is_empty() => {
            !reserved_for_agent(&fold(&records), agent).is_empty()
        }
        _ => true,
    }
}

/// Fold the log into current state: the latest record per attachment, in
/// first-seen order.
pub fn fold(records: &[SessionRecord]) -> Vec<SessionRecord> {
    let mut order: Vec<Ulid> = Vec::new();
    let mut latest: HashMap<Ulid, SessionRecord> = HashMap::new();
    for record in records {
        if !latest.contains_key(&record.attachment_id) {
            order.push(record.attachment_id);
        }
        latest.insert(record.attachment_id, record.clone());
    }
    order
        .into_iter()
        .filter_map(|id| latest.remove(&id))
        .collect()
}

/// The live attachment for an agent, if any. During a replace two are
/// briefly live; the newest wins.
pub fn active_for_agent<'a>(state: &'a [SessionRecord], agent: &str) -> Option<&'a SessionRecord> {
    state
        .iter()
        .filter(|r| r.is_attached() && r.agent.eq_ignore_ascii_case(agent))
        .max_by_key(|r| (r.ts, r.attachment_id))
}

/// Every record that reserves this agent: live, or pending and not yet
/// abandoned.
pub fn reserved_for_agent<'a>(state: &'a [SessionRecord], agent: &str) -> Vec<&'a SessionRecord> {
    state
        .iter()
        .filter(|r| r.agent.eq_ignore_ascii_case(agent))
        .filter(|r| r.is_attached() || r.is_unknown() || (r.is_pending() && !r.is_abandoned()))
        .collect()
}

/// The live attachment for a harness session id, if any.
pub fn active_for_session<'a>(
    state: &'a [SessionRecord],
    session: &str,
) -> Option<&'a SessionRecord> {
    if session.is_empty() {
        return None;
    }
    state
        .iter()
        .find(|r| r.is_attached() && r.session == session)
}

/// The record reserving a session id, live or pending, if any.
pub fn reserved_for_session<'a>(
    state: &'a [SessionRecord],
    session: &str,
) -> Option<&'a SessionRecord> {
    if session.is_empty() {
        return None;
    }
    state.iter().find(|r| {
        r.session == session
            && (r.is_attached() || r.is_unknown() || (r.is_pending() && !r.is_abandoned()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let r = SessionRecord::attached("rite-dev", "codex", "01a0a711", SessionKind::Push, None);
        let json = serde_json::to_string(&r).unwrap();
        let back: SessionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.attachment_id, r.attachment_id);
        assert_eq!(back.kind, SessionKind::Push);
        assert!(back.is_attached());
        assert!(json.contains("\"kind\":\"push\""));
        assert!(json.contains("\"event\":\"attached\""));
    }

    #[test]
    fn unknown_kind_and_event_are_preserved_not_fatal() {
        let json = r#"{"ts":"2026-09-15T18:02:11Z","attachment_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","agent":"a","harness":"codex","session":"s","kind":"teleport","event":"suspended"}"#;
        let r: SessionRecord = serde_json::from_str(json).unwrap();
        assert!(matches!(r.kind, SessionKind::Unknown(_)));
        assert!(matches!(r.event, SessionEvent::Unknown(_)));
        assert!(
            !r.is_attached(),
            "an unknown event must not count as attached"
        );
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains("\"kind\":\"teleport\""));
        assert!(out.contains("\"event\":\"suspended\""));
    }

    #[test]
    fn fold_keeps_latest_per_attachment_in_first_seen_order() {
        let a = SessionRecord::attached("a", "codex", "s-a", SessionKind::Push, None);
        let b = SessionRecord::attached("b", "claude", "s-b", SessionKind::Stream, None);
        let a_done = a.detached(None);
        let state = fold(&[a.clone(), b.clone(), a_done]);
        assert_eq!(state.len(), 2);
        assert_eq!(state[0].attachment_id, a.attachment_id);
        assert!(!state[0].is_attached());
        assert!(state[1].is_attached());
        assert!(active_for_agent(&state, "a").is_none());
        assert_eq!(active_for_agent(&state, "B").unwrap().session, "s-b");
        assert_eq!(active_for_session(&state, "s-b").unwrap().agent, "b");
        assert!(active_for_session(&state, "s-a").is_none());
    }

    #[test]
    fn pending_reserves_but_is_not_live_until_committed() {
        let p = SessionRecord::attaching("a", "codex", "s", SessionKind::Push, Ulid::new());
        assert!(p.is_pending() && p.is_reserved() && !p.is_attached() && !p.is_abandoned());
        let state = fold(&[p.clone()]);
        assert!(active_for_agent(&state, "a").is_none());
        assert_eq!(reserved_for_agent(&state, "a").len(), 1);
        assert!(reserved_for_session(&state, "s").is_some());
        let state = fold(&[p.clone(), p.committed()]);
        assert!(active_for_agent(&state, "a").is_some());
        let mut old = p.clone();
        old.ts = Utc::now() - chrono::Duration::seconds(PENDING_TTL_SECS + 1);
        assert!(old.is_abandoned());
        assert!(reserved_for_agent(&fold(&[old]), "a").is_empty());
    }

    #[test]
    fn an_unknown_latest_event_reserves_the_identity() {
        let a = SessionRecord::attached("a", "codex", "s", SessionKind::Push, None);
        let json = format!(
            r#"{{"ts":"2026-09-16T00:00:00Z","attachment_id":"{}","agent":"a","harness":"codex","session":"s","kind":"push","event":"suspended"}}"#,
            a.attachment_id
        );
        let newer: SessionRecord = serde_json::from_str(&json).unwrap();
        let state = fold(&[a, newer]);
        assert!(state[0].is_unknown() && state[0].is_reserved() && !state[0].is_attached());
        assert_eq!(reserved_for_agent(&state, "a").len(), 1);
        assert!(reserved_for_session(&state, "s").is_some());
        assert!(
            active_for_agent(&state, "a").is_none(),
            "not live, but not free either"
        );
    }

    #[test]
    fn known_tags_match_variants() {
        for tag in SessionKind::KNOWN_TAGS {
            assert!(SessionKind::parse(tag).is_some(), "{tag}");
        }
        assert_eq!(SessionKind::parse("push").unwrap().as_str(), "push");
    }
}
