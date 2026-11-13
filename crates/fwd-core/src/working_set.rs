//! What an agent holds, read in one go so that an agent whose context was reset (a new session, a
//! compaction) gets back what it was working on without a command for each part (`fwd prime`; the
//! page is to show the same). For each principal bound to the agent, in every project of the node,
//! it holds the questions that wait on the principal, the questions the principal asked that wait
//! on someone else, the tasks assigned to it that are not finished, its unread notifications, and
//! the ready tasks it could start: each a [`Section`], most important first, each a list of records
//! named by the one line a reader knows them by.

use crate::model::*;
use crate::project_graph::{ProjectGraph, ProjectRecord};
use crate::query::with_id;
use crate::{ops, reason, routing, subject};
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use std::cmp::Reverse;

/// Whose working set is read.
#[derive(Debug, Clone, Copy)]
pub enum Holder<'a> {
    /// Every principal bound to the agent this subject names (`subject`), in every project of the
    /// node, as a command the agent runs acts as each of them in its project.
    Agent(&'a str),
    /// One principal of the graph's project, for a command that names a principal rather than an
    /// agent (`--as`, or the person).
    Principal(EntityId),
}

/// How many records the sections whose lists grow without bound read: the ready tasks, and the
/// unread notifications, each of which costs a sentence built from several reads. The other
/// sections hold what a principal took on or was asked, and are read whole.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub ready: usize,
    pub unread: usize,
}

/// What one principal holds in its project.
#[derive(Debug, Clone)]
pub struct Holding {
    pub project: ProjectRecord,
    pub principal: EntityId,
    pub handle: String,
    pub kind: PrincipalKind,
    pub role: Option<AgentRole>,
    /// The sections read, in [`SectionKind::ALL`]'s order.
    pub sections: Vec<Section>,
}

/// A kind of record a working set lists, by how much it matters to an agent coming back to its
/// work, most first. The intentions an agent works under are to come before all of them once how
/// they are recorded is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum SectionKind {
    /// Open questions asked of the principal, by name or as the triage agent that answers first.
    AskedOfYou,
    /// Open questions the principal asked of someone, or of no one in particular.
    YouAsked,
    /// Tasks assigned to the principal that are not done, cancelled or rejected.
    Assigned,
    Unread,
    /// The frontier as the principal sees it (`ops::ready_tasks`): its own ready tasks first.
    Ready,
}

impl SectionKind {
    pub const ALL: [SectionKind; 5] = [
        SectionKind::AskedOfYou,
        SectionKind::YouAsked,
        SectionKind::Assigned,
        SectionKind::Unread,
        SectionKind::Ready,
    ];
}

/// The records of one section, those read of `total`.
#[derive(Debug, Clone)]
pub struct Section {
    pub kind: SectionKind,
    pub records: Vec<Record>,
    pub total: usize,
}

/// A record as a working set lists it: what it is, and the one line a reader knows it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: EntityId,
    pub kind: RecordKind,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum RecordKind {
    Ask,
    Task,
    Notification,
}

/// The working set of `holder`, read through `graph`, of the sections `wanted` names: one
/// [`Holding`] per principal, the graph's own project first and the others by name; none for an
/// agent no active principal is bound to.
pub async fn read(
    graph: &ProjectGraph,
    holder: Holder<'_>,
    wanted: &[SectionKind],
    limits: Limits,
) -> Result<Vec<Holding>> {
    let mut projects = graph.projects().await?;
    match holder {
        Holder::Agent(agent) => {
            projects.sort_by(|a, b| (a.record != graph.record(), &a.name).cmp(&(b.record != graph.record(), &b.name)));
            let mut held = Vec::new();
            for project in projects {
                let there = graph.graph_of_project(&project);
                // A retired principal holds nothing for the agent: its work went back, or to its
                // successor.
                match subject::principal_of(&there, agent).await? {
                    Some(p) if p.active()? => held.push(holding(&there, project, &p, wanted, limits).await?),
                    _ => {}
                }
            }
            Ok(held)
        }
        Holder::Principal(id) => {
            let project = projects
                .into_iter()
                .find(|p| p.record == graph.record())
                .ok_or_else(|| anyhow!("the node holds no record of this graph's project"))?;
            let p: PrincipalView = graph.get(id).await?;
            Ok(vec![holding(graph, project, &p, wanted, limits).await?])
        }
    }
}

async fn holding(
    graph: &ProjectGraph,
    project: ProjectRecord,
    p: &PrincipalView,
    wanted: &[SectionKind],
    limits: Limits,
) -> Result<Holding> {
    let me = p.id();
    let whole = |kind, records: Vec<Record>| Section { kind, total: records.len(), records };
    // Both question sections come of one read of the project's open messages.
    let (mut asked_of_you, mut you_asked) =
        if wanted.contains(&SectionKind::AskedOfYou) || wanted.contains(&SectionKind::YouAsked) {
            questions(graph, p).await?
        } else {
            Default::default()
        };
    let mut sections = Vec::new();
    for kind in SectionKind::ALL.into_iter().filter(|kind| wanted.contains(kind)) {
        sections.push(match kind {
            SectionKind::AskedOfYou => whole(kind, std::mem::take(&mut asked_of_you)),
            SectionKind::YouAsked => whole(kind, std::mem::take(&mut you_asked)),
            SectionKind::Assigned => whole(kind, assigned(graph, me).await?),
            SectionKind::Unread => {
                let unread = ops::inbox(graph, me, true).await?;
                let mut records = Vec::new();
                for n in unread.iter().take(limits.unread) {
                    let title = reason::plain(&reason::gather(graph, n, None).await?);
                    records.push(Record { id: n.id(), kind: RecordKind::Notification, title });
                }
                Section { kind, records, total: unread.len() }
            }
            SectionKind::Ready => {
                let ready = ops::ready_tasks(graph, Some(me)).await?;
                let records = ready.iter().take(limits.ready).map(|t| task_record(t, false)).collect::<Result<_>>()?;
                Section { kind, records, total: ready.len() }
            }
        });
    }
    Ok(Holding { project, principal: me, handle: p.handle()?, kind: p.kind()?, role: ops::role_of(p), sections })
}

/// The open questions of `graph`'s project asked of `p`, as routing reads whom a question asks
/// (`routing::asked_of`), and those `p` asked, each list by the priority it wants its answer at,
/// then newest first, as the Questions page lists them. Only a triage agent answers first for a
/// question asked of no one in particular, so for any other principal those are not weighed.
async fn questions(graph: &ProjectGraph, p: &PrincipalView) -> Result<(Vec<Record>, Vec<Record>)> {
    let me = p.id();
    let triage = ops::holds(p, AgentRole::Triage);
    let principals = ops::principals(graph).await?;
    let open: Vec<MessageView> = graph.fetch(format!("status = '{}'", MessageStatus::Active).as_str()).await?;
    let mut asked_of_you = Vec::new();
    let mut you_asked = Vec::new();
    for m in &open {
        // A message whose fields cannot be read is left to the Questions page, as routing leaves it.
        let Ok(mc) = ops::MessageContext::from_view(m) else { continue };
        if mc.to.is_none() && !mc.asks_project {
            continue;
        }
        let author: EntityId = m.author()?.into();
        if (mc.to.is_some() || triage)
            && routing::asked_of(graph, &principals, &mc).await?.is_some_and(|asked| asked.recipient() == me)
        {
            asked_of_you.push((m, format!("from @{}", ops::handle_of(graph, author).await?)));
        }
        if author == me {
            let whom = match mc.to.as_deref() {
                Some(to) => match to.parse::<EntityId>() {
                    Ok(id) => format!("to @{}", ops::handle_of(graph, id).await?),
                    Err(_) => format!("to @{to}"),
                },
                None => "to no one in particular".to_string(),
            };
            you_asked.push((m, whom));
        }
    }
    Ok((ordered(asked_of_you)?, ordered(you_asked)?))
}

/// Questions as records, by priority, then newest first; each titled by whom it is from or to and
/// its first words, as a message is named elsewhere (`reason::NAME_WORDS`).
fn ordered(mut questions: Vec<(&MessageView, String)>) -> Result<Vec<Record>> {
    questions.sort_by_key(|(m, _)| (m.priority().ok(), Reverse(m.created().unwrap_or(0)), m.id()));
    questions
        .into_iter()
        .map(|(m, whom)| {
            let words = reason::first_words(&ops::text(m.content())?, reason::NAME_WORDS);
            Ok(Record { id: m.id(), kind: RecordKind::Ask, title: format!("{whom}: {words}") })
        })
        .collect()
}

/// The tasks assigned to `me` that are not done, cancelled or rejected, as `fwd task list --mine`
/// lists them: the started ones first, then those to start, those in review, and those proposed,
/// each by priority, then oldest first, then by id, so that an unchanged list prints the same.
async fn assigned(graph: &ProjectGraph, me: EntityId) -> Result<Vec<Record>> {
    let mut tasks: Vec<TaskView> = graph
        .fetch(with_id("assignee = ? AND status != 'done' AND status != 'cancelled' AND status != 'rejected'", me)?)
        .await?;
    let stage = |t: &TaskView| match t.status() {
        Ok(TaskStatus::InProgress) => 0,
        Ok(TaskStatus::Open) => 1,
        Ok(TaskStatus::InReview) => 2,
        _ => 3,
    };
    tasks.sort_by_key(|t| (stage(t), t.priority().ok().flatten().unwrap_or(9), t.created().unwrap_or(0), t.id()));
    tasks.iter().map(|t| task_record(t, true)).collect()
}

/// A task as a record, its title followed by its status where tasks of several statuses are listed.
fn task_record(t: &TaskView, with_status: bool) -> Result<Record> {
    let title = if with_status { format!("{} [{}]", t.title()?, t.status()?) } else { t.title()? };
    Ok(Record { id: t.id(), kind: RecordKind::Task, title })
}
