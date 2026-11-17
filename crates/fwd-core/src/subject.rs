//! Which Claude Code agent runs a command, its subject, so that what an agent writes is attributed
//! to that agent and not to its lead, the person or an agent before it (Daniel, 2026-09-29:
//! "ideally there would be one user per agent, and each agent would name itself";
//! docs/DECISIONS.md, D112 and D118; docs/design/agents-and-inbox.md, "An agent's identity"). The
//! fwd plugin's hook (integrations/claude-code) puts the ids Claude Code gives the session and the
//! agent into each command it stamps, as [`SESSION_VAR`] and [`AGENT_VAR`], and fwd reads the two
//! as the subject `claude-code:<session>/<agent>`.
//!
//! A Codex thread that calls the daemon's MCP endpoint is a subject too, `codex:<session>/<thread>`
//! ([`codex`]): the ids Codex attaches to each tool call the model makes (the thread's own id, and
//! the id of the session tree it belongs to, which a subagent shares with the thread that spawned it
//! and a fork does not), which the daemon reads at the transport boundary, never from the model's
//! arguments (crates/fwd/src/mcp.rs). Local metadata is attribution under local trust, not a
//! security boundary.

use crate::model::*;
use crate::project_graph::{ProjectGraph, ProjectTransaction};
use crate::query::{with_id, with_string, with_strings};
use crate::{naming, ops};
use ankurah::ankql::ast::{Parsed, Selection};
use ankurah::model::Mutable;
use ankurah::property::Ref;
use ankurah::EntityId;
use ankurah_commons::identity::{IdentityBinding, IdentityBindingView};
use ankurah_commons::property::Timestamp;
use ankurah_commons::records::{Closed, Created};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

/// The variable a stamped command carries the Claude Code session's id in.
pub const SESSION_VAR: &str = "FWD_CLAUDE_SESSION";

/// The variable a stamped command carries the agent's id in: a subagent's own, or [`MAIN`].
pub const AGENT_VAR: &str = "FWD_CLAUDE_AGENT";

/// What [`AGENT_VAR`] holds for the session's own thread, to which Claude Code gives no agent id.
pub const MAIN: &str = "main";

/// The variable Claude Code itself sets in every command it runs, stamped or not: the session's
/// id, the same for a session's subagents.
pub const CLAUDE_CODE_SESSION_VAR: &str = "CLAUDE_CODE_SESSION_ID";

/// What every subject of a Claude Code agent starts with.
const CLAUDE_CODE: &str = "claude-code:";

/// What every subject of a Codex thread starts with.
const CODEX: &str = "codex:";

/// Whether `id` is an id the stamp may carry and a subject hold: 1 to 128 letters, digits, `.`,
/// `_` and `-`, which a command can quote without escaping and a subject can be split at `/` again.
/// Claude Code's session ids are UUIDs and its agent ids hex, so every real one is.
pub fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The subject of a command that `agent` (a subagent's id, or [`MAIN`]) of the Claude Code session
/// `session` runs, refused for an id [`valid_id`] refuses.
pub fn claude_code(session: &str, agent: &str) -> Result<String> {
    for (what, id) in [("session", session), ("agent", agent)] {
        if !valid_id(id) {
            bail!(
                "`{id}` is not a Claude Code {what} id: an id is 1 to 128 letters, digits, `.`, `_` and `-`, as the fwd plugin's stamp writes it"
            );
        }
    }
    Ok(format!("{CLAUDE_CODE}{session}/{agent}"))
}

/// The subject of the Codex thread `thread`, of the session tree `session` (the id of the thread at
/// its root, which is `thread` itself for a thread that no other spawned), refused for an id
/// [`valid_id`] refuses. Codex's ids are UUIDs, so every real one is valid.
pub fn codex(session: &str, thread: &str) -> Result<String> {
    for (what, id) in [("session", session), ("thread", thread)] {
        if !valid_id(id) {
            bail!("`{id}` is not a Codex {what} id: an id is 1 to 128 letters, digits, `.`, `_` and `-`");
        }
    }
    Ok(format!("{CODEX}{session}/{thread}"))
}

/// The session and the agent or thread a subject names, and which harness, when it is a Claude
/// Code agent's or a Codex thread's.
fn parts(subject: &str) -> Option<(&'static str, &str, &str)> {
    let (harness, rest) = match (subject.strip_prefix(CLAUDE_CODE), subject.strip_prefix(CODEX)) {
        (Some(rest), _) => ("Claude Code", rest),
        (_, Some(rest)) => ("Codex", rest),
        _ => return None,
    };
    let (session, agent) = rest.split_once('/')?;
    Some((harness, session, agent))
}

/// A subject as the sentences name it: `the Claude Code agent claude-code:<session>/<agent>`, or
/// `the Codex thread codex:<session>/<thread>`.
pub fn words(subject: &str) -> String {
    match parts(subject) {
        Some(("Codex", ..)) => format!("the Codex thread {subject}"),
        _ => format!("the Claude Code agent {subject}"),
    }
}

/// The session of a subject as the sentences name it: `the Claude Code session <id>`, or `the
/// Codex session <id>`.
pub fn session_words(subject: &str) -> String {
    match parts(subject) {
        Some((harness, session, _)) => format!("the {harness} session {session}"),
        None => format!("the session of {subject}"),
    }
}

/// The subject a command's environment names, given the values of [`SESSION_VAR`] and
/// [`AGENT_VAR`] (an empty value counts as unset): none when neither is set, and refused when only
/// one is, since the plugin's stamp always sets both, and no command should act as half an agent.
pub fn from_env(session: Option<&str>, agent: Option<&str>) -> Result<Option<String>> {
    match (session.filter(|s| !s.is_empty()), agent.filter(|a| !a.is_empty())) {
        (None, None) => Ok(None),
        (Some(session), Some(agent)) => claude_code(session, agent).map(Some),
        _ => bail!(
            "{SESSION_VAR} and {AGENT_VAR} name the Claude Code agent that runs a command, and the fwd plugin's stamp sets both: set both, or neither"
        ),
    }
}

/// The session a subject names, when it is a Claude Code agent's or a Codex thread's.
pub fn session_of(subject: &str) -> Option<&str> {
    parts(subject).map(|(_, session, _)| session)
}

/// Whether two subjects are agents of the same session of the same harness: a Codex session's id
/// is never taken for a Claude Code session's.
pub fn same_session(a: &str, b: &str) -> bool {
    match (parts(a), parts(b)) {
        (Some((ha, sa, _)), Some((hb, sb, _))) => ha == hb && sa == sb,
        _ => false,
    }
}

/// Whether a subject is a session's own thread, and not a subagent: a Claude Code session's
/// ([`MAIN`]), or a Codex thread at the root of its session tree, whose id is the session's.
pub fn is_main_thread(subject: &str) -> bool {
    match parts(subject) {
        Some(("Codex", session, thread)) => session == thread,
        Some((_, _, agent)) => agent == MAIN,
        None => false,
    }
}

/// The project's identity bindings `args` selects that hold, the undone ones left out: what binds
/// each subject to the principal it acts as in the project, placed there.
async fn holding(graph: &ProjectGraph, args: Selection<Parsed>) -> Result<Vec<(PlacementView, IdentityBindingView)>> {
    let mut held = Vec::new();
    for (placement, binding) in graph.fetch_placed::<IdentityBindingView>(args).await? {
        if !graph.is_closed(&binding).await? {
            held.push((placement, binding));
        }
    }
    Ok(held)
}

/// The principals of the project the bindings `args` selects bind, each once, in the order of their
/// ids.
async fn principals_bound(graph: &ProjectGraph, args: Selection<Parsed>) -> Result<Vec<EntityId>> {
    let mut ids = Vec::new();
    for (_, binding) in holding(graph, args).await? {
        ids.push(EntityId::from(binding.principal()?));
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The principals of the project bound to an agent of the Claude Code session `session`, its own
/// thread's or a subagent's.
pub async fn of_session(graph: &ProjectGraph, session: &str) -> Result<Vec<EntityId>> {
    principals_bound(graph, of_session_selection(session)?).await
}

/// The bindings of every agent of the session `prefix` and `session` name: every subject that
/// starts with `<prefix><session>/`, as one range of the text the store compares by its bytes, the
/// subjects from there up to, not including, `<prefix><session>0`, `0` being the character after
/// `/`.
fn session_selection(prefix: &str, session: &str) -> Result<Selection<Parsed>> {
    let from = format!("{prefix}{session}/");
    let before = format!("{prefix}{session}0");
    with_strings("subject >= ? AND subject < ?", &[&from, &before])
}

/// A selection of the identity bindings of the agents of the Claude Code session `session`, as
/// [`of_session`] reads them: what a view of the session listens to for an agent of it naming
/// itself or being unbound (`live_read`), rather than to every binding.
pub fn of_session_selection(session: &str) -> Result<Selection<Parsed>> {
    session_selection(CLAUDE_CODE, session)
}

/// The principals of the project bound to an agent of the session `subject` is an agent of, its
/// own thread's or a subagent's, of the same harness ([`same_session`]).
pub async fn of_same_session(graph: &ProjectGraph, subject: &str) -> Result<Vec<EntityId>> {
    let prefix = if subject.starts_with(CODEX) { CODEX } else { CLAUDE_CODE };
    let Some(session) = session_of(subject) else { return Ok(Vec::new()) };
    principals_bound(graph, session_selection(prefix, session)?).await
}

/// The agent's session or thread bound to `principal` in the project, as `fwd principal list`
/// prints it: none for a principal no agent is bound to, as a person is, whose account at a service
/// that signed them in is a binding too. Should two hold, the first by their text.
pub async fn session_bound_to(graph: &ProjectGraph, principal: EntityId) -> Result<Option<String>> {
    let mut sessions = Vec::new();
    for (_, binding) in holding(graph, with_id("principal = ?", principal)?).await? {
        let subject = binding.subject()?;
        if session_of(&subject).is_some() {
            sessions.push(subject);
        }
    }
    Ok(sessions.into_iter().min())
}

/// The agent's session or thread bound to each of the project's principals that one is bound to,
/// read at once for a list of them, as [`session_bound_to`] reads one principal's.
pub async fn sessions_bound(graph: &ProjectGraph) -> Result<BTreeMap<EntityId, String>> {
    let mut sessions: BTreeMap<EntityId, String> = BTreeMap::new();
    for prefix in [CLAUDE_CODE, CODEX] {
        for (_, binding) in harness_bindings(graph, prefix).await? {
            let subject = binding.subject()?;
            let principal = EntityId::from(binding.principal()?);
            match sessions.get(&principal) {
                Some(first) if *first <= subject => {}
                _ => {
                    sessions.insert(principal, subject);
                }
            }
        }
    }
    Ok(sessions)
}

/// The project's bindings that hold of the harness whose subjects start with `prefix` (`codex:`,
/// `claude-code:`): every subject from the prefix up to, not including, the prefix with its last
/// character, `:`, made `;`, the character after it.
async fn harness_bindings(graph: &ProjectGraph, prefix: &str) -> Result<Vec<(PlacementView, IdentityBindingView)>> {
    let before = format!("{};", prefix.trim_end_matches(':'));
    holding(graph, with_strings("subject >= ? AND subject < ?", &[prefix, &before])?).await
}

/// The project's bindings of Codex threads that hold.
pub async fn codex_bindings(graph: &ProjectGraph) -> Result<Vec<(PlacementView, IdentityBindingView)>> {
    harness_bindings(graph, CODEX).await
}

/// The bindings of the project that hold `subject`, which a binding of it to another principal
/// undoes or is refused by.
pub async fn bindings_of(graph: &ProjectGraph, subject: &str) -> Result<Vec<(PlacementView, IdentityBindingView)>> {
    holding(graph, with_string("subject = ?", subject)?).await
}

/// Binds `subject` to the principal `principal` in `trx`, as `by` does now: an identity binding
/// placed in the transaction's project, marked as made by the migration that `migrated` names,
/// where one did. Its id. Nothing routes a binding: should something come to, a binding a
/// migration made is told by its `Migrated`, and it is made with a creation event as any is.
pub async fn bind(
    trx: &ProjectTransaction,
    principal: EntityId,
    subject: &str,
    by: EntityId,
    migrated: Option<&str>,
) -> Result<EntityId> {
    let made =
        trx.create_placed(&IdentityBinding { principal: Ref::new(principal), subject: subject.to_string() }).await?;
    let record = Mutable::entity(&*made);
    trx.add(record, &Created { created: Timestamp::from_unix_millis(crate::now_ms()), by: Ref::new(by) }).await?;
    if let Some(migration) = migrated {
        trx.add(record, &Migrated { migration: migration.to_string() }).await?;
    }
    Ok(record.id())
}

/// Undoes `binding` in `trx`, as `by` does now: it is closed, and its subject no longer acts as
/// its principal.
pub async fn unbind(
    trx: &ProjectTransaction,
    (placement, _): &(PlacementView, IdentityBindingView),
    by: EntityId,
) -> Result<()> {
    let closing = Closed {
        closed: Some(Timestamp::from_unix_millis(crate::now_ms())),
        by: Some(Ref::new(by)),
        outcome: None,
        reason: None,
    };
    Ok(trx.close(placement, &closing).await?)
}

/// The principal of the project bound to `subject`, as whom a command that carries the subject
/// acts: none while the agent has not named itself. Should two be bound to it (two `fwd principal
/// add`s of one agent at once), the one that is not retired; two that are not are refused, listed
/// by id, for the agent to retire the one it does not mean. A principal acts for one session, as
/// the one field that held its session before the Commons did held one: one bound to another
/// session's agent as well (two binds of one principal at once) is refused, naming both.
pub async fn principal_of(graph: &ProjectGraph, subject: &str) -> Result<Option<(RoleView, PrincipalView)>> {
    let mut bound = Vec::new();
    for id in principals_bound(graph, with_string("subject = ?", subject)?).await? {
        bound.push(graph.principal(id).await?);
    }
    let mut at_work = Vec::new();
    for p in &bound {
        if !ops::retired(&p.0)? {
            at_work.push(p);
        }
    }
    let one = match (bound.as_slice(), at_work.as_slice()) {
        ([], _) => return Ok(None),
        ([one], _) => one,
        (_, [one]) => one,
        (several, _) => {
            let mut listed: Vec<String> = several
                .iter()
                .map(|p| format!("{} ({})", p.0.id().to_base64().chars().take(8).collect::<String>(), naming::what(p)))
                .collect();
            listed.sort();
            bail!(
                "{} principals of this project are bound to {}, and a command acts as one: retire the one it is not (`fwd principal retire <id>`): {}",
                several.len(),
                words(subject),
                listed.join(", ")
            )
        }
    };
    let (role, principal) = one;
    let mut others = Vec::new();
    for (_, binding) in holding(graph, with_id("principal = ?", role.id())?).await? {
        let bound_to = binding.subject()?;
        if bound_to != subject && session_of(&bound_to).is_some() {
            others.push(bound_to);
        }
    }
    if let Some(other) = others.into_iter().min() {
        bail!(
            "@{} is bound to {} and to {}, and a principal acts for one session, so neither acts as it: retire it (`fwd principal retire {}`), and the agent of each session names itself anew",
            principal.handle()?,
            words(subject),
            words(&other),
            role.id().to_base64()
        );
    }
    Ok(Some(one.clone()))
}

/// Whom a command that carries `subject` acts as, given the `--as` typed on its command line, when
/// one was (`FWD_AS` is not read where a subject is, since a settings file's `env` block reaches
/// every agent of the session): the principal bound to the subject, and none while the agent has
/// not named itself. A typed `--as` must name that principal, since an agent acts only as itself;
/// one that names another principal, or any `--as` while the agent has not named itself, is refused
/// with the sentence saying so. `fwd principal bind` reads its `--as` itself.
pub async fn caller(graph: &ProjectGraph, subject: &str, typed_as: Option<&str>) -> Result<Option<EntityId>> {
    let bound = principal_of(graph, subject).await?;
    if let Some(typed) = typed_as {
        let named = naming::principal_named(graph, naming::bare(typed), None).await?;
        match (&bound, named) {
            (Some(b), Some(n)) if n.0.id() == b.0.id() => {}
            (Some(b), _) => bail!(
                "`--as {typed}` asks to act as another principal, but this command runs as {}, whose principal is @{}, and an agent acts only as itself: leave `--as` off",
                words(subject),
                b.1.handle()?
            ),
            (None, _) => bail!(
                "this command runs as {}, which has not named itself in this project, and an agent acts only as itself, so `--as {typed}` is refused: {}",
                words(subject),
                how_to_name_itself()
            ),
        }
    }
    Ok(bound.map(|(role, _)| role.id()))
}

/// How an agent that has not named itself does, for the sentences that refuse it.
fn how_to_name_itself() -> String {
    format!(
        "name yourself and select your role, `fwd principal add <name> --role {}`, with a name no other principal of this project has (`fwd principal list --all`); or take over, once, a principal from before the fwd plugin that no agent is bound to: `fwd --as <its name> principal bind <its name>`",
        ops::ROLE_WORDS
    )
}

/// The refusal of a command that writes, or that reads or marks the caller's own inbox or reads its
/// own tasks, from the Claude Code agent `subject` while no principal of the project is bound to it.
pub fn unnamed(subject: &str) -> String {
    format!(
        "this command runs as {}, which has not named itself in this project, and an agent writes, and reads its own inbox and its own tasks, only as itself: {}",
        words(subject),
        how_to_name_itself()
    )
}

/// The refusal of a command that writes or marks notifications, or that reads the inbox or the
/// tasks of the principal it acts as, in the Claude Code session `session`, when it carries no
/// agent's stamp and no `--as` typed on its command line (and for a read, no FWD_AS either), and
/// `bound` is a principal bound to one of the session's agents: fwd cannot tell which of them runs
/// it, and FWD_AS cannot tell it either, since every agent of the session inherits it; nor is it the
/// person's, since any of the session's agents may have run it (task JFASz_E2). The sentence names
/// both ways out: the plugin's stamp, and a typed `--as`, which no agent inherits
/// (docs/DECISIONS.md, D120).
pub fn unstamped(session: &str, bound: &str) -> String {
    format!(
        "this command runs in the Claude Code session {session}, whose agents act as themselves (@{bound} is bound to one of them), but it carries no agent's stamp, so fwd cannot tell which agent runs it, and FWD_AS cannot tell it either, since every agent of the session inherits it: run fwd on its own, which the fwd plugin stamps with the agent that runs it (a command made only of fwd, cd and the filters head, tail, grep, wc, jq, sort, uniq and cut reading fwd's output, with no `$` or backtick outside single quotes and no redirection but 2>&1 and /dev/null, in a call that does not ask to run outside the sandbox with dangerouslyDisableSandbox) and which acts as that agent once it has named itself (`fwd principal add <name> --role {}`), or type `--as <name>` on the command, which acts as the principal it names",
        ops::ROLE_WORDS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subject_names_the_session_and_the_agent_and_only_real_ids() {
        assert_eq!(claude_code("56693b92-71ca", "a7b8a42").unwrap(), "claude-code:56693b92-71ca/a7b8a42");
        assert_eq!(claude_code("s1", MAIN).unwrap(), "claude-code:s1/main");
        assert!(claude_code("s1", "a/b").is_err(), "a slash would split the subject elsewhere");
        assert!(claude_code("s 1", "a").is_err());
        assert!(claude_code("", "a").is_err());
        assert!(claude_code("s1", "'; rm -rf ~").is_err());
        assert_eq!(session_of("claude-code:s1/a1"), Some("s1"));
        assert_eq!(session_of("github:1234"), None);
        assert!(is_main_thread("claude-code:s1/main"));
        assert!(!is_main_thread("claude-code:s1/a1"));
        assert!(!is_main_thread("github:main"));
    }

    #[test]
    fn a_codex_subject_names_its_session_tree_and_thread_and_is_another_harness_session() {
        assert_eq!(codex("t-root", "t-root").unwrap(), "codex:t-root/t-root");
        assert!(codex("s", "a/b").is_err());
        assert!(codex("", "t").is_err());
        assert_eq!(session_of("codex:s1/t2"), Some("s1"));
        assert!(is_main_thread("codex:s1/s1"), "a thread at the root of its session tree is its own");
        assert!(!is_main_thread("codex:s1/t2"), "a subagent shares its session with the thread that spawned it");
        assert!(same_session("codex:s1/s1", "codex:s1/t2"));
        assert!(!same_session("codex:s1/s1", "claude-code:s1/main"), "the harnesses' sessions are apart");
        assert_eq!(words("codex:s1/t2"), "the Codex thread codex:s1/t2");
        assert_eq!(words("claude-code:s1/main"), "the Claude Code agent claude-code:s1/main");
        assert_eq!(session_words("codex:s1/t2"), "the Codex session s1");
    }

    #[test]
    fn the_environment_names_a_subject_with_both_variables_or_none() {
        assert_eq!(from_env(None, None).unwrap(), None);
        assert_eq!(from_env(Some(""), Some("")).unwrap(), None, "empty counts as unset");
        assert_eq!(from_env(Some("s1"), Some("main")).unwrap().as_deref(), Some("claude-code:s1/main"));
        assert!(from_env(Some("s1"), None).is_err(), "half an agent is refused");
        assert!(from_env(None, Some("a1")).is_err());
    }
}
