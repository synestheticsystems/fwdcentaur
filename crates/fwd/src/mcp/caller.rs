//! Who calls a tool of the daemon's MCP endpoint, as the transport attests it, so that what a
//! thread writes is attributed to that thread and never to the person, to its lead, or to whatever
//! actor the model would name. The operation layer (`super::Operations`) takes a [`Caller`] and
//! never reads a transport: the local endpoint makes one from the metadata the Codex host attaches
//! to every tool call ([`Caller::from_codex_meta`]), and a hosted endpoint can make one from an
//! authenticated session later, without touching the operations.
//!
//! Local metadata is attribution under local trust, not a security boundary: a process running as
//! the same user can send any metadata it likes, as it can run `fwd --as` (docs/design/daemon.md).
//! What it does exclude is the model choosing whom to act as, since Codex puts the ids in the
//! request's `_meta`, which the model's arguments cannot reach, and an accidental caller that sends
//! no such metadata, which is refused rather than taken for anyone.

use anyhow::{bail, Result};
use fwd_core::model::AgentRole;
use fwd_core::subject;
use serde_json::{Map, Value};

/// The `_meta` key Codex puts the thread's turn metadata under: an object, on every call a model
/// made over stdio and over Streamable HTTP alike (identity-report.md, http-identity-report.md). A
/// JSON string of one, which is what the app-server's direct `mcpServer/tool/call` forwards from
/// its client, is refused; not as a security boundary, since a forging local process can send an
/// object as easily, but because no call a model makes takes that shape.
pub const TURN_METADATA: &str = "x-codex-turn-metadata";

/// The caller of a tool, as the transport attests it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The subject the principal acting for this caller is bound to (`fwd_core::subject`):
    /// `codex:<session>/<thread>` for a Codex thread.
    pub subject: String,
    /// The thread's own id.
    pub thread: String,
    /// The id of the thread at the root of its session tree: its own for a thread no other spawned
    /// (a fork's included), its root's for a subagent.
    pub session: String,
    /// The thread that spawned it, for a subagent.
    pub parent: Option<String>,
}

impl Caller {
    /// The caller of a tool call whose request carries `meta`, its `_meta`, as Codex attaches it to
    /// every call the model makes (task jDv1EY7K; the identity research in
    /// /private/tmp/fwd-connections-active/identity-report.md): `threadId` and `sessionId`, and the
    /// turn's metadata, whose `thread_id` and `session_id` must be the same two ids. Refused when
    /// any of them is missing, blank or not an id, or when the two disagree, saying which; a call
    /// fwd cannot attribute is never run as anyone.
    pub fn from_codex_meta(meta: &Map<String, Value>) -> Result<Caller> {
        let field = |key: &str| meta.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let (Some(thread), Some(session)) = (field("threadId"), field("sessionId")) else {
            bail!(unattributed("the request's _meta names no threadId and sessionId"));
        };
        let turn: Map<String, Value> = match meta.get(TURN_METADATA) {
            Some(Value::Object(turn)) => turn.clone(),
            Some(_) => bail!(unattributed(&format!("its _meta's {TURN_METADATA} is not an object"))),
            None => bail!(unattributed(&format!("the request's _meta has no {TURN_METADATA}"))),
        };
        let turn_field = |key: &str| turn.get(key).and_then(Value::as_str).map(str::trim);
        if turn_field("thread_id") != Some(thread) || turn_field("session_id") != Some(session) {
            bail!(unattributed(&format!(
                "its _meta's threadId and sessionId ({thread}, {session}) are not the thread_id and session_id of its {TURN_METADATA} ({}, {})",
                turn_field("thread_id").unwrap_or("none"),
                turn_field("session_id").unwrap_or("none")
            )));
        }
        let subject = subject::codex(session, thread)?;
        let parent = turn_field("parent_thread_id").filter(|p| !p.is_empty()).map(str::to_string);
        Ok(Caller { subject, thread: thread.to_string(), session: session.to_string(), parent })
    }

    /// Whether the caller is the thread at the root of its session tree, which coordinates the
    /// session's work as a Claude Code session's own thread does, rather than a subagent.
    pub fn main_thread(&self) -> bool {
        subject::is_main_thread(&self.subject)
    }

    /// The role an agent made for this caller on its first call takes: implementer, whatever the
    /// thread's place in its session tree, since a top-level thread is as often a worker or a
    /// reviewer that a lead started as it is a lead (lead-codex's ruling on the early review,
    /// 2026-10-09). A thread selects another with `principal_role`, or names itself with its role
    /// before its first write with `principal_add`.
    pub fn default_role(&self) -> AgentRole {
        AgentRole::Implementer
    }

    /// The names an agent made for this caller tries, in order, the first no principal of the
    /// project has: `codex-` and the last 8 letters and digits of the thread's id, then 12, then 16,
    /// then all of them. The end of a Codex thread id is its random part (its start is a timestamp,
    /// which threads spawned together share), so the name is the same for the thread every time and
    /// another for each thread.
    pub fn handles(&self) -> Vec<String> {
        let plain: Vec<char> = self.thread.chars().filter(char::is_ascii_alphanumeric).collect();
        let mut out: Vec<String> = Vec::new();
        for n in [8, 12, 16, plain.len()] {
            let tail: String = plain[plain.len().saturating_sub(n)..].iter().collect::<String>().to_ascii_lowercase();
            let handle = format!("codex-{tail}");
            if !out.contains(&handle) {
                out.push(handle);
            }
        }
        out
    }
}

/// The refusal of a call fwd cannot attribute to a thread.
fn unattributed(why: &str) -> String {
    format!(
        "fwd cannot tell which thread makes this call, so it acts as no one and runs nothing: {why}. The daemon's MCP endpoint attributes every call to the Codex thread that makes it, from the threadId, sessionId and {TURN_METADATA} Codex attaches to the tool calls a model makes; no argument names whom to act as"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("an object")
    }

    #[test]
    fn a_call_is_the_thread_its_metadata_names_twice_alike() {
        let m = meta(json!({
            "threadId": "019a-child", "sessionId": "019a-root", "progressToken": 1,
            TURN_METADATA: {"thread_id": "019a-child", "session_id": "019a-root", "parent_thread_id": "019a-root", "turn_id": "t1"},
        }));
        let caller = Caller::from_codex_meta(&m).expect("attributed");
        assert_eq!(caller.subject, "codex:019a-root/019a-child");
        assert_eq!(caller.parent.as_deref(), Some("019a-root"));
        assert!(!caller.main_thread());
        assert_eq!(caller.default_role(), AgentRole::Implementer);
        let caller = Caller::from_codex_meta(&meta(
            json!({"threadId": "s1", "sessionId": "s1", TURN_METADATA: {"thread_id": "s1", "session_id": "s1"}}),
        ))
        .unwrap();
        assert!(caller.main_thread());
        assert_eq!(caller.default_role(), AgentRole::Implementer, "a top-level thread is not taken for a lead");
    }

    #[test]
    fn a_call_without_consistent_metadata_is_refused() {
        let turn = json!({"thread_id": "a", "session_id": "s"});
        for (m, why) in [
            (json!({}), "no threadId"),
            (json!({"threadId": "a"}), "no threadId and sessionId"),
            (json!({"threadId": "a", "sessionId": "s"}), "has no x-codex-turn-metadata"),
            (json!({"threadId": "a", "sessionId": "s", TURN_METADATA: 7}), "is not an object"),
            (json!({"threadId": "a", "sessionId": "s", TURN_METADATA: turn.to_string()}), "is not an object"),
            (json!({"threadId": "b", "sessionId": "s", TURN_METADATA: turn}), "are not the thread_id"),
            (json!({"threadId": "a", "sessionId": "other", TURN_METADATA: turn}), "are not the thread_id"),
            (
                json!({"threadId": "a/b", "sessionId": "s", TURN_METADATA: {"thread_id": "a/b", "session_id": "s"}}),
                "not a Codex thread id",
            ),
        ] {
            let refused = Caller::from_codex_meta(&meta(m.clone())).expect_err("refused").to_string();
            assert!(refused.contains(why), "{m}: {refused}");
        }
    }

    #[test]
    fn a_threads_name_is_the_end_of_its_id_and_longer_on_a_clash() {
        let caller = Caller {
            subject: "codex:x/x".into(),
            thread: "019A3B2C-7D4E-7F00-8A1B-C2D3E4F5A6B7".into(),
            session: "x".into(),
            parent: None,
        };
        assert_eq!(
            caller.handles(),
            [
                "codex-e4f5a6b7",
                "codex-c2d3e4f5a6b7",
                "codex-8a1bc2d3e4f5a6b7",
                "codex-019a3b2c7d4e7f008a1bc2d3e4f5a6b7"
            ]
        );
    }
}
