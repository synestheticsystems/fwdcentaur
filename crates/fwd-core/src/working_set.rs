//! What an agent holds, read in one go so that an agent whose context was reset (a new session, a
//! compaction) gets back what it was working on without a command for each part (`fwd prime`), and
//! so that the person sees what a session holds (`session_view`). For each principal bound to the
//! agent, in every project of the node, it holds the questions that wait on the principal, the
//! questions the principal asked that wait on someone else, the tasks assigned to it that are not
//! finished, its unread notifications, and, where the reader works, the ready tasks it could start:
//! each a [`Section`], most important first, each a list of records with what a reader names and
//! shows them by. A task moved to another project keeps its assignee, and its messages their
//! authors and addressees (`task_move`), so what names the principal is read wherever it now is,
//! with its project.

use crate::live_read::LiveRead;
use crate::model::*;
use crate::ops::{self, MessageContext, ASKS_KEY, ASKS_PROJECT};
use crate::project_graph::{InProjectView, ProjectGraph, ProjectRecord};
use crate::query::with_strings;
use crate::route::Route;
use crate::{reason, routing, subject};
use ankurah::ankql::ast::{Parsed, Selection};
use ankurah::EntityId;
use anyhow::{anyhow, Result};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

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

/// How much of a working set one read takes: which sections, and how many of the records of those
/// that grow without bound.
#[derive(Debug, Clone)]
pub struct Extent {
    /// The sections read, of [`SectionKind::ALL`].
    pub sections: Vec<SectionKind>,
    /// The project, by its record, whose ready tasks are read: a task is started where one works,
    /// so the frontier is read for the project the reader works in alone, and none is read without
    /// one.
    pub ready_in: Option<EntityId>,
    /// How many ready tasks, and how many unread notifications, are read: their lists grow without
    /// bound, and each notification costs a sentence built from several reads. The other sections
    /// hold what a principal took on or was asked, and are read whole.
    pub ready: usize,
    pub unread: usize,
}

/// What one principal holds.
#[derive(Debug, Clone)]
pub struct Holding {
    /// The principal's project, which a record of another project is named against.
    pub project: ProjectRecord,
    pub principal: EntityId,
    pub handle: String,
    pub kind: PrincipalKind,
    pub role: Option<AgentRole>,
    /// The agent the principal is bound to (`subject`), if any.
    pub subject: Option<String>,
    /// The sections read for it, in [`SectionKind::ALL`]'s order: the ready tasks only in the
    /// project they are read for ([`Extent::ready_in`]).
    pub sections: Vec<Section>,
}

impl Holding {
    /// The questions asked of the principal, each with whom it is from.
    pub fn asked_of(&self) -> impl Iterator<Item = (&Record, &Party)> {
        self.records(SectionKind::AskedOfYou).filter_map(|r| match &r.kind {
            RecordKind::Ask(Asking::Of { from }) => Some((r, from)),
            _ => None,
        })
    }

    /// The questions the principal asked, each with whom it asks.
    pub fn asked_by(&self) -> impl Iterator<Item = (&Record, &Addressee)> {
        self.records(SectionKind::YouAsked).filter_map(|r| match &r.kind {
            RecordKind::Ask(Asking::By { to }) => Some((r, to)),
            _ => None,
        })
    }

    /// The tasks assigned to the principal, each with its status.
    pub fn assigned(&self) -> impl Iterator<Item = (&Record, TaskStatus)> {
        self.records(SectionKind::Assigned).filter_map(|r| match r.kind {
            RecordKind::Task { status: Some(status) } => Some((r, status)),
            _ => None,
        })
    }

    /// The records of the section `kind`, none when it was not read.
    fn records(&self, kind: SectionKind) -> impl Iterator<Item = &Record> {
        self.sections.iter().filter(move |s| s.kind == kind).flat_map(|s| &s.records)
    }

    /// The live queries whose changes can change what this holding's sections hold: the live form
    /// of each read the sections came from, given the principal's id, or in its project its name.
    /// The ready tasks have none, since the frontier is read across the project's whole graph.
    pub fn live_reads(&self) -> Vec<LiveRead> {
        let has = |kind| self.sections.iter().any(|s| s.kind == kind);
        let mut reads = Vec::new();
        if has(SectionKind::AskedOfYou) || has(SectionKind::YouAsked) {
            reads.push(LiveRead::QuestionsTo(self.principal));
            reads.push(LiveRead::QuestionsBy(self.principal));
            reads.push(LiveRead::QuestionsByName {
                project: self.project.clone(),
                handle: self.handle.clone(),
                of_no_one: self.role == Some(AgentRole::Triage),
            });
        }
        if has(SectionKind::Assigned) {
            reads.push(LiveRead::TasksAssignedTo(self.principal));
        }
        if has(SectionKind::Unread) {
            reads.push(LiveRead::Unread { project: self.project.clone(), principal: self.principal });
        }
        reads
    }
}

/// A kind of record a working set lists, by how much it matters to an agent coming back to its
/// work, most first. The intentions an agent works under are to come before all of them once how
/// they are recorded is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum SectionKind {
    /// Open questions asked of the principal: by its id, wherever the question is, or by its name or
    /// of no one in particular in its project, as routing reads whom a question asks.
    AskedOfYou,
    /// Open questions the principal asked of someone, or of no one in particular, wherever they are.
    YouAsked,
    /// Tasks assigned to the principal that are not done, cancelled or rejected, wherever they are.
    Assigned,
    Unread,
    /// The frontier as the principal sees it (`ops::ready_tasks`), less the tasks assigned to it,
    /// which [`SectionKind::Assigned`] lists when it is read.
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

/// The records of one section.
#[derive(Debug, Clone)]
pub struct Section {
    pub kind: SectionKind,
    /// The records read, in order: all of them but those a limit leaves unread.
    pub records: Vec<Record>,
    /// How many records the section holds.
    pub total: usize,
    /// How many records a whole list of the section's kind holds besides `total`, which another
    /// section lists instead: the ready tasks assigned to the principal, listed as assigned.
    pub listed_elsewhere: usize,
    /// The records of the section that could not be read, by id: listed nowhere and not in
    /// `total`, so that a reader is told what is missing rather than shown less without a word.
    pub unreadable: Vec<EntityId>,
}

/// A record as a working set lists it: what it is, where it is, what a reader names it by and its
/// own text, each reader composing its own line from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: EntityId,
    pub kind: RecordKind,
    /// Its own name, its whitespace collapsed so that it is one line whatever its text holds: a
    /// task's title, a question's first words (`reason::NAME_WORDS`), a notification's inbox
    /// sentence.
    pub title: String,
    /// Its own text, whole: a task's what, a question's text; none for a notification, whose
    /// sentence is its title, and none where it could not be read.
    pub text: Option<String>,
    /// When it was made, in milliseconds since the epoch; none where it could not be read, where
    /// nothing orders by it.
    pub created: Option<i64>,
    /// The project the record is in, which a moved record has as another than its principal's.
    pub project: ProjectRecord,
    /// Where the page shows it; none where the page has no route of its own for it.
    pub route: Option<Route>,
}

#[derive(Debug, Clone, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum RecordKind {
    Ask(Asking),
    /// A task, with its status: an assigned task's always, a ready task's where it could be read.
    Task {
        status: Option<TaskStatus>,
    },
    Notification,
}

/// The other party to a question, as the list it stands in names it: who asked a question asked
/// of the principal, whom a question the principal asked asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asking {
    Of { from: Party },
    By { to: Addressee },
}

/// A principal by its id, and its name as a reader in the principal's project is shown it
/// (`ops::handle_of`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    pub id: EntityId,
    pub handle: String,
}

/// Whom a question asks, as its context says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressee {
    Principal(Party),
    /// A name that is no principal's id, as the question was written: a principal's name, with or
    /// without the at-sign, which routing reads in the question's project.
    Name(String),
    /// No one in particular, which the triage agent of the scope it concerns answers.
    NoOne,
}

/// The working set of `holder`, read through `graph`, as `extent` says: one [`Holding`] per
/// principal, the graph's own project first and the others by name, then by id, since two may
/// share a name; none for an agent no active principal is bound to.
pub async fn read(graph: &ProjectGraph, holder: Holder<'_>, extent: &Extent) -> Result<Vec<Holding>> {
    let mut projects = graph.projects().await?;
    projects.sort_by_key(|p| (p.record != graph.record(), p.name.clone(), p.uuid));
    let mut read = Read { projects: &projects, extent, handles: HashMap::new() };
    let mut held = Vec::new();
    match holder {
        Holder::Agent(agent) => {
            for project in &projects {
                let there = graph.graph_of_project(project);
                let principals = ops::principals(&there).await?;
                // A retired principal acts for no agent, so what it holds is no agent's to take up.
                match subject::principal_among(&principals, agent)? {
                    Some(p) if p.active()? => held.push(read.holding(&there, project, &p, &principals).await?),
                    _ => {}
                }
            }
        }
        Holder::Principal(id) => {
            let project = projects
                .iter()
                .find(|p| p.record == graph.record())
                .ok_or_else(|| anyhow!("the node holds no record of this graph's project"))?;
            let p: PrincipalView = graph.get(id).await?;
            held.push(read.holding(graph, project, &p, &ops::principals(graph).await?).await?);
        }
    }
    Ok(held)
}

/// Every project of the node, by name, then by id, and each one's principals, read once for a
/// reader that needs them for more than one working set.
#[derive(Debug, Clone)]
pub struct Known {
    pub projects: Vec<(ProjectRecord, Vec<PrincipalView>)>,
}

impl Known {
    pub async fn read(graph: &ProjectGraph) -> Result<Known> {
        let mut projects = graph.projects().await?;
        projects.sort_by_key(|p| (p.name.clone(), p.uuid));
        let mut known = Vec::with_capacity(projects.len());
        for project in projects {
            let principals = ops::principals(&graph.graph_of_project(&project)).await?;
            known.push((project, principals));
        }
        Ok(Known { projects: known })
    }
}

/// The working set of every active principal bound to an agent of the Claude Code session
/// `session`, in the projects `known` holds: those of its own thread first, then its subagents',
/// each in `known`'s order of projects, then by name, then by id. What the person's view of a
/// session shows (`session_view`).
pub async fn read_session(graph: &ProjectGraph, known: &Known, session: &str, extent: &Extent) -> Result<Vec<Holding>> {
    let projects: Vec<ProjectRecord> = known.projects.iter().map(|(project, _)| project.clone()).collect();
    let mut read = Read { projects: &projects, extent, handles: HashMap::new() };
    let (mut own, mut subagents) = (Vec::new(), Vec::new());
    for (project, principals) in &known.projects {
        let there = graph.graph_of_project(project);
        let mut bound: Vec<&PrincipalView> = subject::of_session(principals, session);
        bound.sort_by_key(|p| (p.handle().ok(), p.id()));
        for p in bound {
            if !p.active()? {
                continue;
            }
            let holding = read.holding(&there, project, p, principals).await?;
            if holding.subject.as_deref().is_some_and(subject::is_main_thread) {
                own.push(holding);
            } else {
                subagents.push(holding);
            }
        }
    }
    own.extend(subagents);
    Ok(own)
}

/// One read of a working set: what it reads, and what it has read once and names records by.
struct Read<'a> {
    projects: &'a [ProjectRecord],
    extent: &'a Extent,
    /// Each principal's handle as a reader in a project is shown it, by that project's record and
    /// the principal's id.
    handles: HashMap<(EntityId, EntityId), String>,
}

/// Records read for a section, and those that could not be read.
#[derive(Default)]
struct Listed {
    records: Vec<Record>,
    unreadable: Vec<EntityId>,
}

impl Read<'_> {
    async fn holding(
        &mut self,
        graph: &ProjectGraph,
        project: &ProjectRecord,
        p: &PrincipalView,
        principals: &[PrincipalView],
    ) -> Result<Holding> {
        let me = p.id();
        let wanted = self.extent.sections.clone();
        let (mut asked_of_you, mut you_asked) =
            if wanted.contains(&SectionKind::AskedOfYou) || wanted.contains(&SectionKind::YouAsked) {
                self.questions(graph, p, principals).await?
            } else {
                Default::default()
            };
        let mut assigned =
            if wanted.contains(&SectionKind::Assigned) { self.assigned(graph, me).await? } else { Listed::default() };
        let listed_as_assigned: HashSet<EntityId> = assigned.records.iter().map(|r| r.id).collect();
        let mut sections = Vec::new();
        for kind in SectionKind::ALL.into_iter().filter(|kind| wanted.contains(kind)) {
            sections.push(match kind {
                SectionKind::AskedOfYou => whole(kind, std::mem::take(&mut asked_of_you)),
                SectionKind::YouAsked => whole(kind, std::mem::take(&mut you_asked)),
                SectionKind::Assigned => whole(kind, std::mem::take(&mut assigned)),
                SectionKind::Unread => self.unread(graph, project, me).await?,
                SectionKind::Ready if self.extent.ready_in == Some(project.record) => {
                    self.ready(graph, project, me, &listed_as_assigned).await?
                }
                SectionKind::Ready => continue,
            });
        }
        Ok(Holding {
            project: project.clone(),
            principal: me,
            handle: p.handle()?,
            kind: p.kind()?,
            role: ops::role_of(p),
            // Only the view tells a session's own thread from its subagents by it.
            subject: p.subject().ok().flatten(),
            sections,
        })
    }

    /// The open questions asked of `p` and those it asked, each list by the priority it wants its
    /// answer at, then newest first, as the Questions page lists them, then by id. Only messages
    /// that ask someone are read: those that ask `p` by its id, wherever they are; in its project,
    /// those that ask it by its handle, with or without the at-sign routing reads it with, and, for
    /// a triage agent, those asked of no one in particular, which routing's rule weighs
    /// (`routing::asked_of`), since only a triage agent answers first for them; and those `p` wrote
    /// that ask someone, wherever they are. A question two of these find is listed once.
    async fn questions(
        &mut self,
        graph: &ProjectGraph,
        p: &PrincipalView,
        principals: &[PrincipalView],
    ) -> Result<(Listed, Listed)> {
        let me = p.id();
        let mut asked: Vec<MessageView> = graph.open_questions_to(me).await?;
        let mut candidates: Vec<MessageView> =
            graph.fetch(questions_by_name(&p.handle()?, ops::holds(p, AgentRole::Triage))?).await?;
        candidates.retain(|m| asked.iter().all(|a| a.id() != m.id()));
        let mut asked_of_you = Listed::default();
        for m in candidates {
            match MessageContext::from_view(&m) {
                Ok(mc) => {
                    if routing::asked_of(graph, principals, &mc).await?.is_some_and(|a| a.recipient() == me) {
                        asked.push(m);
                    }
                }
                Err(_) => asked_of_you.unreadable.push(m.id()),
            }
        }
        let wrote = graph.open_questions_by(me).await?;
        let mut you_asked = Listed::default();
        self.questions_as_records(graph, asked, true, &mut asked_of_you).await?;
        self.questions_as_records(graph, wrote, false, &mut you_asked).await?;
        Ok((asked_of_you, you_asked))
    }

    /// `messages` as records in `listed`, each named by its first words, as a message is named
    /// elsewhere (`reason::NAME_WORDS`), with whom it is from (`asked_of`) or whom it asks; one
    /// whose fields cannot be read among `listed`'s unreadable.
    async fn questions_as_records(
        &mut self,
        graph: &ProjectGraph,
        messages: Vec<MessageView>,
        asked_of: bool,
        listed: &mut Listed,
    ) -> Result<()> {
        let mut keyed = Vec::new();
        for m in messages {
            match question_facts(&m) {
                Ok(facts) => keyed.push((facts, m)),
                Err(_) => listed.unreadable.push(m.id()),
            }
        }
        keyed.sort_by_key(|(f, m)| (f.priority, Reverse(f.created), m.id()));
        for (facts, m) in keyed {
            let asking = if asked_of {
                Asking::Of { from: Party { id: facts.author, handle: self.handle(graph, facts.author).await? } }
            } else {
                let to = match facts.to.map(|to| (to.parse::<EntityId>(), to)) {
                    Some((Ok(id), _)) => Addressee::Principal(Party { id, handle: self.handle(graph, id).await? }),
                    Some((Err(_), name)) => Addressee::Name(name),
                    None => Addressee::NoOne,
                };
                Asking::By { to }
            };
            let title = reason::first_words(&facts.content, reason::NAME_WORDS);
            listed.records.push(self.record(
                m.id(),
                RecordKind::Ask(asking),
                Written { title: &title, text: Some(facts.content), created: Some(facts.created) },
                m.project_id()?,
                Some(Route::question(m.id())),
            )?);
        }
        Ok(())
    }

    /// The tasks assigned to `me` that are not done, cancelled or rejected, wherever they are:
    /// the started ones first, then those to start, those in review and those proposed, each by
    /// priority, a task with none last, then oldest first, then by id.
    async fn assigned(&mut self, graph: &ProjectGraph, me: EntityId) -> Result<Listed> {
        let tasks = graph.unfinished_tasks_assigned_to(me).await?;
        let mut listed = Listed::default();
        let mut keyed = Vec::new();
        for t in tasks {
            match task_facts(&t) {
                Ok(facts) => keyed.push((facts, t)),
                Err(_) => listed.unreadable.push(t.id()),
            }
        }
        let stage = |status: TaskStatus| match status {
            TaskStatus::InProgress => 0,
            TaskStatus::Open => 1,
            TaskStatus::InReview => 2,
            _ => 3,
        };
        keyed.sort_by_key(|(f, t)| (stage(f.status), f.priority.unwrap_or(i64::MAX), f.created, t.id()));
        for (facts, t) in keyed {
            listed.records.push(self.record(
                t.id(),
                RecordKind::Task { status: Some(facts.status) },
                Written { title: &facts.title, text: facts.what, created: Some(facts.created) },
                t.project_id()?,
                Some(Route::task(t.id())),
            )?);
        }
        Ok(listed)
    }

    /// `me`'s unread notifications, in the inbox's order, each by its inbox sentence without the
    /// way to answer it, the first [`Extent::unread`] of them read.
    async fn unread(&mut self, graph: &ProjectGraph, project: &ProjectRecord, me: EntityId) -> Result<Section> {
        let unread = ops::inbox(graph, me, true).await?;
        let mut records = Vec::new();
        for n in unread.iter().take(self.extent.unread) {
            let sentence = reason::plain(&reason::gather(graph, n, None).await?);
            let written = Written { title: &sentence, text: None, created: n.created().ok() };
            records.push(self.record(n.id(), RecordKind::Notification, written, project.record, None)?);
        }
        Ok(Section {
            kind: SectionKind::Unread,
            records,
            total: unread.len(),
            listed_elsewhere: 0,
            unreadable: Vec::new(),
        })
    }

    /// The ready tasks `me` could start in `project`, less those `listed` as assigned, the first
    /// [`Extent::ready`] of them read.
    async fn ready(
        &mut self,
        graph: &ProjectGraph,
        project: &ProjectRecord,
        me: EntityId,
        listed: &HashSet<EntityId>,
    ) -> Result<Section> {
        let frontier = ops::ready_tasks(graph, Some(me)).await?;
        let (elsewhere, ready): (Vec<TaskView>, Vec<TaskView>) =
            frontier.into_iter().partition(|t| listed.contains(&t.id()));
        let mut records = Vec::new();
        let mut unreadable = Vec::new();
        for t in ready.iter().take(self.extent.ready) {
            // Prime lists a ready task by its title; what else it is, a reader takes where it reads.
            match t.title() {
                Ok(title) => records.push(self.record(
                    t.id(),
                    RecordKind::Task { status: t.status().ok() },
                    Written { title: &title, text: ops::text(t.what()).ok(), created: t.created().ok() },
                    project.record,
                    Some(Route::task(t.id())),
                )?),
                Err(_) => unreadable.push(t.id()),
            }
        }
        let total = ready.len() - unreadable.len();
        Ok(Section { kind: SectionKind::Ready, records, total, listed_elsewhere: elsewhere.len(), unreadable })
    }

    /// A record of the project whose record is `project`, its title on one line.
    fn record(
        &self,
        id: EntityId,
        kind: RecordKind,
        written: Written<'_>,
        project: EntityId,
        route: Option<Route>,
    ) -> Result<Record> {
        let project = self
            .projects
            .iter()
            .find(|p| p.record == project)
            .ok_or_else(|| anyhow!("{} is a record of a project the node holds no record of", id.to_base64()))?
            .clone();
        let Written { title, text, created } = written;
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        Ok(Record { id, kind, title, text, created, project, route })
    }

    /// The handle of `principal` as a reader in `graph`'s project is shown it (`ops::handle_of`),
    /// read once in a read.
    async fn handle(&mut self, graph: &ProjectGraph, principal: EntityId) -> Result<String> {
        let key = (graph.record(), principal);
        if let Some(handle) = self.handles.get(&key) {
            return Ok(handle.clone());
        }
        let handle = ops::handle_of(graph, principal).await?;
        self.handles.insert(key, handle.clone());
        Ok(handle)
    }
}

/// A section of `listed`, read whole.
fn whole(kind: SectionKind, listed: Listed) -> Section {
    Section {
        kind,
        total: listed.records.len(),
        records: listed.records,
        listed_elsewhere: 0,
        unreadable: listed.unreadable,
    }
}

/// What a question is listed by.
struct QuestionFacts {
    author: EntityId,
    to: Option<String>,
    priority: Priority,
    created: i64,
    content: String,
}

fn question_facts(m: &MessageView) -> Result<QuestionFacts> {
    let context = MessageContext::from_view(m)?;
    Ok(QuestionFacts {
        author: m.author()?.into(),
        to: context.to,
        priority: m.priority()?,
        created: m.created()?,
        content: ops::text(m.content())?,
    })
}

/// What an assigned task is listed by: all of it but its what, which only the view shows.
struct TaskFacts {
    title: String,
    what: Option<String>,
    status: TaskStatus,
    priority: Option<i64>,
    created: i64,
}

fn task_facts(t: &TaskView) -> Result<TaskFacts> {
    Ok(TaskFacts {
        title: t.title()?,
        what: ops::text(t.what()).ok(),
        status: t.status()?,
        priority: t.priority()?,
        created: t.created()?,
    })
}

/// What a record says of itself, from which [`Read::record`] makes it.
struct Written<'a> {
    title: &'a str,
    text: Option<String>,
    created: Option<i64>,
}

/// The open questions of a project that ask the principal named `handle` by that name, with or
/// without the at-sign routing reads it with, and, when it answers for them (`of_no_one`, a triage
/// agent's), those asked of no one in particular, which routing's rule then weighs
/// (`routing::asked_of`): what [`Read::questions`] reads beside the questions asked by its id, and
/// the live form of it ([`LiveRead::QuestionsByName`]).
pub fn questions_by_name(handle: &str, of_no_one: bool) -> Result<Selection<Parsed>> {
    let mut by_rule = format!("status = '{}' AND (context.to = ? OR context.to = ?", MessageStatus::Active);
    if of_no_one {
        by_rule.push_str(&format!(" OR context.{ASKS_KEY} = '{ASKS_PROJECT}'"));
    }
    by_rule.push(')');
    with_strings(&by_rule, &[handle, &format!("@{handle}")])
}
