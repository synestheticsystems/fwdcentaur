//! What a name typed for a principal names. A principal's name (its `handle`) is a convenience
//! label that several principals of a project may share, and its id is its identity (Daniel,
//! 2026-09-29, docs/DECISIONS.md, D107 and D110: "usernames are just convenience labels on user
//! entities. There should be no expectation of uniqueness"). So that a repeated name never picks a
//! principal silently, every place where someone types one reads it by this module's rule, within
//! the project whose graph it is: `--as` and `FWD_AS`, `--to`, `--for`, `fwd task
//! assign`, `--decided-by`, the `principal` commands, an at-sign in a message, the page's `?as=`
//! and `?for=`, and the extension's `fwd.as` (docs/design/agents-and-inbox.md, "A principal's
//! name"). A name two or more principals of the project share is refused, listing each of them by
//! its id with what it is and the scopes its work is in, never guessed; the pickers show those
//! scopes beside such a name ([`shows_scopes`]).
//!
//! The rule itself reads the principals it is handed ([`resolve`], [`by_name`]), so the page
//! applies it to its standing queries and a command to what it fetched ([`principal_named`]).

use crate::dormancy;
use crate::model::*;
use crate::ops;
use crate::query::with_id;
use ankurah::{Context, EntityId};
use anyhow::Result;
use std::collections::HashMap;

/// What a typed name names among a project's principals ([`resolve`]).
#[derive(Debug)]
pub enum Named<'a> {
    /// The one principal it names.
    One(&'a PrincipalView),
    /// No principal of the kind asked for has the name, the id or an id the name starts.
    NoOne,
    /// Two or more principals have the name, so it names none of them.
    Shared(Vec<&'a PrincipalView>),
    /// No principal has the name, and it starts the ids of two or more.
    Started(Vec<&'a PrincipalView>),
}

/// A typed name without its at-sign and the spaces around it: `@impl-alice` is `impl-alice`.
pub fn bare(name: &str) -> &str {
    name.trim().trim_start_matches('@')
}

/// The principals of `principals` whose name is `name` (its at-sign optional), of `kind` when one
/// is given: a command that takes only a person does not count an agent that shares its name.
pub fn by_name<'a>(principals: &'a [PrincipalView], name: &str, kind: Option<PrincipalKind>) -> Vec<&'a PrincipalView> {
    let name = bare(name);
    principals
        .iter()
        .filter(|p| kind.is_none() || p.kind().ok() == kind)
        .filter(|p| p.handle().ok().as_deref() == Some(name))
        .collect()
}

/// What `name` names among `principals`, of `kind` when one is given, retired principals
/// included: first the principals that have it as their name, one or several; else the principal
/// whose whole id it is; else, for six characters or more, the principals whose id it starts.
/// The name comes first so that a name that happens to read as an id still names its principal.
pub fn resolve<'a>(principals: &'a [PrincipalView], name: &str, kind: Option<PrincipalKind>) -> Named<'a> {
    let name = bare(name);
    match by_name(principals, name, kind).as_slice() {
        [] => {}
        [one] => return Named::One(one),
        several => return Named::Shared(several.to_vec()),
    }
    let of_kind = || principals.iter().filter(|p| kind.is_none() || p.kind().ok() == kind);
    if let Ok(id) = name.parse::<EntityId>() {
        if let Some(p) = of_kind().find(|p| p.id() == id) {
            return Named::One(p);
        }
    }
    if !ops::may_start_an_id(name) {
        return Named::NoOne;
    }
    let started: Vec<&PrincipalView> = of_kind().filter(|p| p.id().to_base64().starts_with(name)).collect();
    match started.as_slice() {
        [] => Named::NoOne,
        [one] => Named::One(one),
        _ => Named::Started(started),
    }
}

/// The principal `name` names in the project's graph, by [`resolve`]'s rule, of `kind` when one is
/// given: none when no principal has the name or an id it names, and a refusal ([`Refused`]) when
/// it names several, listing each with the scopes its work is in. The caller words what none
/// means where it is: the command that makes a principal, or that a person was wanted.
pub async fn principal_named(ctx: &Context, name: &str, kind: Option<PrincipalKind>) -> Result<Option<PrincipalView>> {
    let principals = ops::principals(ctx).await?;
    match resolve(&principals, name, kind) {
        Named::One(p) => Ok(Some(p.clone())),
        Named::NoOne => Ok(None),
        Named::Shared(several) => Err(refused(ctx, name, &several, false).await?.into()),
        Named::Started(several) => Err(refused(ctx, name, &several, true).await?.into()),
    }
}

/// A name refused because it names several principals, whose sentence lists them: an error of its
/// own, so that a caller can tell it from a failure to read the graph, as posting a message does to
/// refuse a shared handle apart from the references that name nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// The refusal of `name`, which `several` share, or whose ids it starts (`started`), each listed
/// with the scopes its work is in, read from the graph.
pub async fn refused(ctx: &Context, name: &str, several: &[&PrincipalView], started: bool) -> Result<Refused> {
    let loaded = Loaded::of(ctx, &several.iter().map(|p| p.id()).collect::<Vec<_>>()).await?;
    let work = loaded.work();
    Ok(Refused(refusal(name, several, started, |id| scope_titles(&scopes_of(id, &work), work.scopes))))
}

/// The sentence refusing `name`, which the `several` principals share (or, with `started`, whose
/// ids it starts): each by the first eight characters of its id (more for a longer start), in the
/// order of their ids, so every reader lists them alike, with what it is, whether it is retired or
/// does not belong here, and the scopes `scopes` gives for it, so the writer can give the id of
/// the one they mean.
pub fn refusal(
    name: &str,
    several: &[&PrincipalView],
    started: bool,
    scopes: impl Fn(EntityId) -> Vec<String>,
) -> String {
    let name = bare(name);
    let chars = if started { (name.len() + 4).max(8) } else { 8 };
    let mut several = several.to_vec();
    several.sort_by_key(|p| p.id().to_base64());
    let listed = several.iter().map(|p| candidate(p, chars, &scopes(p.id()))).collect::<Vec<_>>().join("; ");
    if started {
        format!(
            "`{name}` starts the ids of {} principals of this project ({listed}); give more of the id",
            several.len()
        )
    } else {
        format!(
            "{} principals of this project are named `{name}` ({listed}); give one's id, or its first eight characters, in place of the name",
            several.len()
        )
    }
}

/// One principal as a refusal lists it: `Ab3dEf12: implementer, retired, in Replication and
/// Anchoring`.
fn candidate(p: &PrincipalView, chars: usize, scopes: &[String]) -> String {
    let id: String = p.id().to_base64().chars().take(chars).collect();
    format!("{id}: {}{}, {}", what(p), standing(p), in_scopes(scopes))
}

/// `, retired` or `, does not belong here` for a principal no picker offers, else nothing.
fn standing(p: &PrincipalView) -> &'static str {
    if !p.active().unwrap_or(false) {
        ", retired"
    } else if !ops::belongs(p) {
        ", does not belong here"
    } else {
        ""
    }
}

/// `p` as a sentence names it for a reader who must tell it from others of its name, read from the
/// graph: the first eight characters of its id, which name it wherever a name is typed, then what
/// it is, whether it is retired or does not belong here, and the scopes its work is in:
/// `Ab3dEf12 (implementer, in Replication)`.
pub async fn described(ctx: &Context, p: &PrincipalView) -> Result<String> {
    let loaded = Loaded::of(ctx, &[p.id()]).await?;
    let work = loaded.work();
    let scopes = scope_titles(&scopes_of(p.id(), &work), work.scopes);
    let id: String = p.id().to_base64().chars().take(8).collect();
    Ok(format!("{id} ({}{}, {})", what(p), standing(p), in_scopes(&scopes)))
}

/// What `p` is, as a refusal and a mention search say it: `person`, the role an agent selected, or
/// `agent` for one with none.
pub fn what(p: &PrincipalView) -> String {
    match (p.kind().ok(), ops::role_of(p)) {
        (Some(PrincipalKind::Human), _) => "person".to_string(),
        (_, Some(role)) => role.to_string(),
        _ => "agent".to_string(),
    }
}

/// How many scopes a list names before it counts the rest.
pub const SCOPES_NAMED: usize = 3;

/// `in Replication and Anchoring`, `in A, B, C and 2 more`, or `in no scope`: a principal's scopes
/// as a sentence lists them beside its name.
pub fn in_scopes(scopes: &[String]) -> String {
    let named: Vec<&str> = scopes.iter().take(SCOPES_NAMED).map(String::as_str).collect();
    let more = scopes.len().saturating_sub(SCOPES_NAMED);
    match (named.as_slice(), more) {
        ([], _) => "in no scope".to_string(),
        ([one], _) => format!("in {one}"),
        (_, 0) => format!("in {} and {}", named[..named.len() - 1].join(", "), named[named.len() - 1]),
        (_, more) => format!("in {} and {more} more", named.join(", ")),
    }
}

/// Whether a picker shows the scopes of a principal named `handle` beside its name: where two or
/// more principals of the project share the name, so that they can be told apart (docs/DECISIONS.md,
/// D107, as the lead read it on 2026-09-29: beside a repeated name only). Answering true shows them
/// beside every name, as D107's words ask.
pub fn shows_scopes(principals: &[PrincipalView], handle: &str) -> bool {
    by_name(principals, handle, None).len() > 1
}

/// The principals a mention search offers for `typed`, what follows the `@` up to the caret: those
/// that belong to the project (the pickers' principals, `ops::belongs`) whose name starts with it,
/// without regard to case, by name and then by id, at most `most` of them. The agents dormant at
/// `now`, the reader's clock, come after all the others, by name and then by id among themselves,
/// so that an agent quiet for twelve hours is offered only once the agents at work are
/// (`dormancy`; docs/DECISIONS.md, D123).
pub fn mentionable<'a>(principals: &'a [PrincipalView], typed: &str, most: usize, now: i64) -> Vec<&'a PrincipalView> {
    let typed = typed.to_lowercase();
    let mut found: Vec<(bool, String, &PrincipalView)> = principals
        .iter()
        .filter(|p| ops::belongs(p))
        .filter_map(|p| Some((dormancy::is_dormant(p, now), p.handle().ok()?, p)))
        .filter(|(_, handle, _)| handle.to_lowercase().starts_with(&typed))
        .collect();
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)).then(a.2.id().cmp(&b.2.id())));
    found.into_iter().take(most).map(|(_, _, p)| p).collect()
}

/// How a message's text names `p` so that posting reads it back as `p`: `@` and its name while no
/// other principal of the project has it, else `@` and the first eight characters of its id, since
/// posting refuses a name two or more share. Posting stores either as the id.
pub fn mention_of(principals: &[PrincipalView], p: &PrincipalView) -> String {
    let handle = p.handle().unwrap_or_default();
    if by_name(principals, &handle, None).len() == 1 {
        format!("@{handle}")
    } else {
        format!("@{}", p.id().to_base64().chars().take(8).collect::<String>())
    }
}

/// One principal a picker offers: its id, its name, and the titles of the scopes its work is in
/// where the picker shows them ([`shows_scopes`]), the most recent first.
pub type Offered = (EntityId, String, Option<Vec<String>>);

/// What a picker's option says for each principal of `offered`, in its order: the name, then, where
/// the scopes show, each in brackets, as text names a scope where a lozenge cannot go (an option
/// holds only text; docs/design/web-ui.md, "Who acts"), three of them and a count of the rest, or
/// `(no scope)`; and where two options would still read alike, the first eight characters of each
/// one's id after them.
pub fn option_texts(offered: &[Offered]) -> Vec<String> {
    let texts: Vec<String> = offered
        .iter()
        .map(|(_, name, scopes)| match scopes {
            None => name.clone(),
            Some(scopes) if scopes.is_empty() => format!("{name} (no scope)"),
            Some(scopes) => {
                let mut text = name.clone();
                for title in scopes.iter().take(SCOPES_NAMED) {
                    text.push_str(&format!(" [{title}]"));
                }
                if scopes.len() > SCOPES_NAMED {
                    text.push_str(&format!(" +{}", scopes.len() - SCOPES_NAMED));
                }
                text
            }
        })
        .collect();
    texts
        .iter()
        .zip(offered)
        .map(|(text, (id, _, _))| {
            if texts.iter().filter(|t| *t == text).count() > 1 {
                format!("{text} · {}", id.to_base64().chars().take(8).collect::<String>())
            } else {
                text.clone()
            }
        })
        .collect()
}

/// The work a principal's scopes are read from, as the page's standing queries or a command's
/// fetches hold it ([`scopes_of`]).
#[derive(Clone, Copy)]
pub struct Work<'a> {
    pub tasks: &'a [TaskView],
    /// The `worked_in` relations that say where each task is worked (`assigned_to` on a graph
    /// `fwd admin migrate` has not renamed yet); other relations are passed over.
    pub relations: &'a [RelationView],
    pub scopes: &'a [ScopeView],
    /// The messages whose authors' scopes are asked for; others are passed over.
    pub messages: &'a [MessageView],
}

/// The scopes `principal`'s work is in, the scope it worked in most recently first
/// (docs/design/agents-and-inbox.md, "A principal's name"): the scopes of the tasks assigned to it
/// and of the tasks it made, the scopes it made, and the scopes of the tasks and scopes it wrote
/// messages about; a task worked in several scopes counts in each (docs/DECISIONS.md, D136, "A task
/// may be worked in several scopes"). Never a scope's owner, which says nothing of who works there
/// (D108); a scope the work names that `work` does not hold is left out.
pub fn scopes_of(principal: EntityId, work: &Work) -> Vec<EntityId> {
    let mut worked_in: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for r in work.relations {
        let counts = matches!(r.kind(), Ok(RelationKind::WorkedIn | RelationKind::AssignedTo))
            && r.status().ok() == Some(RelationStatus::Accepted);
        if let (true, Ok(task), Ok(scope)) = (counts, r.from_id(), r.to_id()) {
            worked_in.entry(task).or_default().push(scope);
        }
    }
    let scopes_of_task = |task: EntityId| worked_in.get(&task).cloned().unwrap_or_default();
    let mut latest: HashMap<EntityId, i64> = HashMap::new();
    let mut saw = |scopes: Vec<EntityId>, when: i64| {
        for scope in scopes {
            let at = latest.entry(scope).or_insert(when);
            *at = (*at).max(when);
        }
    };
    for t in work.tasks {
        if ops::assignee_of(t).ok().flatten() == Some(principal) {
            saw(scopes_of_task(t.id()), t.updated().unwrap_or(0));
        }
        if t.created_by().ok().map(EntityId::from) == Some(principal) {
            saw(scopes_of_task(t.id()), t.created().unwrap_or(0));
        }
    }
    for s in work.scopes {
        if s.created_by().ok().map(EntityId::from) == Some(principal) {
            saw(vec![s.id()], s.created().unwrap_or(0));
        }
    }
    for m in work.messages {
        if m.author().ok().map(EntityId::from) != Some(principal) {
            continue;
        }
        let when = m.created().unwrap_or(0);
        saw(m.scope().ok().flatten().map(EntityId::from).into_iter().collect(), when);
        if let Some(task) = m.task().ok().flatten().map(EntityId::from) {
            saw(scopes_of_task(task), when);
        }
    }
    let held: std::collections::HashSet<EntityId> = work.scopes.iter().map(|s| s.id()).collect();
    let mut found: Vec<(EntityId, i64)> = latest.into_iter().filter(|(s, _)| held.contains(s)).collect();
    found.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    found.into_iter().map(|(s, _)| s).collect()
}

/// The titles of `scopes`, in their order, as `held` holds them.
pub fn scope_titles(scopes: &[EntityId], held: &[ScopeView]) -> Vec<String> {
    scopes.iter().filter_map(|id| held.iter().find(|s| s.id() == *id)).map(|s| s.title().unwrap_or_default()).collect()
}

/// The work of some principals, fetched for a command: every task, the relations that say where
/// tasks are worked, every scope, and the messages those principals wrote.
pub struct Loaded {
    tasks: Vec<TaskView>,
    relations: Vec<RelationView>,
    scopes: Vec<ScopeView>,
    messages: Vec<MessageView>,
}

impl Loaded {
    /// The work of `principals` in the project's graph.
    pub async fn of(ctx: &Context, principals: &[EntityId]) -> Result<Loaded> {
        let mut relations: Vec<RelationView> = ctx.fetch("kind = 'worked_in' AND status = 'accepted'").await?;
        relations.extend(ctx.fetch::<RelationView>("kind = 'assigned_to' AND status = 'accepted'").await?);
        let mut messages = Vec::new();
        for p in principals {
            messages.extend(ctx.fetch::<MessageView>(with_id("author = ?", *p)?).await?);
        }
        Ok(Loaded {
            tasks: ctx.fetch("created > 0").await?,
            relations,
            scopes: ctx.fetch("created > 0").await?,
            messages,
        })
    }

    pub fn work(&self) -> Work<'_> {
        Work { tasks: &self.tasks, relations: &self.relations, scopes: &self.scopes, messages: &self.messages }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picker_shows_the_scopes_beside_a_repeated_name_and_the_id_where_two_still_read_alike() {
        let id = |n: u8| EntityId::from_bytes([n; 32]);
        let short = |n: u8| id(n).to_base64().chars().take(8).collect::<String>();
        let scopes = |titles: &[&str]| Some(titles.iter().map(|t| t.to_string()).collect::<Vec<_>>());
        let offered: Vec<Offered> = vec![
            (id(1), "dnorman".into(), None),
            (id(2), "implementer".into(), scopes(&["Replication"])),
            (id(3), "implementer".into(), scopes(&["Anchoring", "Hosting", "Pages", "Search"])),
            (id(4), "reviewer".into(), scopes(&[])),
            (id(5), "reviewer".into(), scopes(&[])),
        ];
        assert_eq!(
            option_texts(&offered),
            vec![
                "dnorman".to_string(),
                "implementer [Replication]".to_string(),
                "implementer [Anchoring] [Hosting] [Pages] +1".to_string(),
                format!("reviewer (no scope) · {}", short(4)),
                format!("reviewer (no scope) · {}", short(5)),
            ]
        );
    }
}
