//! The person's view of a Claude Code session: what the session holds in fwd, as one document that
//! a pane beside the chat draws, and later the page (Daniel, 2026-11-13: it "must not be a wall of
//! text, but a really clean and readable list. I shouldn't even see any of the entity ids, but
//! rather click them to get more detail"). fwd owns what the view says, its order and what can be
//! done from it, so that whatever draws it knows no fwd verb and draws every section and every
//! record the same way: a principal is a [`Group`], a section a headed [`List`], a record a [`Row`],
//! its title and tags, that opens into its lines and its actions (docs/design/cli.md, "The person's
//! view of a session").
//!
//! It covers the principals bound to the session's own thread, in every project, then those bound
//! to its subagents, read by the one working-set read `fwd prime` makes (`working_set`), so that the
//! two cannot disagree about what a session holds. For each principal, in this order: what waits on
//! the person, the open questions it asked them that they have not answered in the question's
//! thread, as the page's Waiting on you counts a question; what it holds, the questions asked of it
//! and then the tasks assigned to it; and what waits on others, the open questions it asked of
//! anyone else or of no one in particular. Its unread notifications are counted on its group, and
//! the ready tasks are not in the view.

mod fit;
mod text;

use crate::live_read::LiveRead;
use crate::model::{PrincipalKind, TaskStatus};
use crate::naming;
use crate::ops;
use crate::project_graph::{ProjectGraph, ProjectRecord};
use crate::route::{page_path, Route};
use crate::subject;
use crate::working_set::{self, Addressee, Extent, Holder, Holding, Record, RecordKind, SectionKind};
use ankurah::EntityId;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeSet, HashSet};

/// The document's title, the same in every state.
const TITLE: &str = "fwd: what this session holds";

/// The view of one session at one moment. It holds no clock reading and no counter, so that two
/// documents are equal when nothing they show changed; an age is a tag's `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Document {
    pub title: String,
    /// The page's address, which each row's `path` follows; none while the daemon is out of reach.
    pub page: Option<String>,
    /// What is said of the whole view: that no principal is bound to the session yet, or that the
    /// daemon is out of reach.
    pub lines: Vec<String>,
    pub principals: Vec<Group>,
}

/// A principal bound to the session, with the lists of what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    /// The principal's id, the same from one document to the next.
    pub id: String,
    /// Its name.
    pub title: String,
    /// Its role, its project, and how many unread notifications it has, when it has any.
    pub tags: Vec<Tag>,
    /// A subagent's group is folded unless something in it waits on the person.
    pub folded: bool,
    pub lines: Vec<String>,
    /// Those with records or lines, in [`ListKind`]'s order.
    pub sections: Vec<List>,
}

/// One section of a principal's group: a headed list of rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct List {
    pub section: ListKind,
    pub title: String,
    pub folded: bool,
    /// What is said of the list: records left out, and records that could not be read.
    pub lines: Vec<String>,
    /// How many rows it shows, of how many it holds.
    pub shown: usize,
    pub of: usize,
    /// The page's own list of what it holds, after the page's address, where there is one and
    /// rows were left out.
    pub more: Option<String>,
    pub records: Vec<Row>,
}

/// The sections of a principal's group, in the order they stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    /// The open questions the principal asked the person that the person has not answered.
    WaitsOnYou,
    /// The intentions the principal works under and its readings of the person's words, which a
    /// later change fills once intentions are recorded; nothing makes this list yet.
    WorksUnder,
    /// The questions asked of the principal, then the tasks assigned to it.
    Holds,
    /// The open questions it asked of anyone else, or of no one in particular.
    WaitsOnOthers,
}

/// A record, as a row that opens into its lines and its actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    /// The record's id, unique within its list.
    pub id: String,
    pub kind: RowKind,
    /// Plain text on one line.
    pub title: String,
    pub tags: Vec<Tag>,
    /// The record's own text as one Markdown block, cut where it is long; none for a task with no
    /// what.
    pub lines: Vec<String>,
    /// Where the page shows it, after the page's address.
    pub path: Option<String>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    Ask,
    Task,
}

/// A short label beside a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tag {
    /// Plain text on one line.
    pub text: String,
    pub tone: Option<Tone>,
    /// The instant the label dates, in UTC (`2026-11-13T21:52:49Z`), which a reader draws as an age
    /// by its own clock.
    pub at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// It waits on the person: neither it nor what holds it is ever folded.
    Attention,
}

/// What the person can do from a row: run the program `fwd` with `args`, no shell, from any
/// directory, with no session in its environment. The arguments name the project and the person,
/// so the command acts as the person; text the person types goes to its standard input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Action {
    pub label: String,
    pub args: Vec<String>,
    pub input: Option<Input>,
}

/// The text an action asks the person for, which goes to the command's standard input whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Input {
    pub prompt: String,
    pub multiline: bool,
}

/// What the view is drawn from: the working set of every principal bound to the session, and
/// what it needs of the person.
#[derive(Debug, Clone)]
pub struct Held {
    /// Those of the session's own thread first, then its subagents' ([`Holder::Session`]).
    pub holdings: Vec<Holding>,
    pub person: Person,
    /// Every project of the node, whose principals the view reads.
    pub projects: Vec<ProjectRecord>,
}

/// The person the view is for.
#[derive(Debug, Clone)]
pub struct Person {
    /// The name the person's principals have, by which an action acts as the person in whichever
    /// project it runs.
    pub handle: String,
    /// The person's principal in each project that has one by that name: a question asked of the
    /// person names one of them.
    pub principals: HashSet<EntityId>,
    /// The questions asked of the person that the person has answered in their threads.
    pub answered: HashSet<EntityId>,
}

/// What a working set is read for the view: no ready tasks, and the unread notifications counted
/// rather than listed.
fn extent() -> Extent {
    Extent {
        sections: vec![SectionKind::AskedOfYou, SectionKind::YouAsked, SectionKind::Assigned, SectionKind::Unread],
        ready_in: None,
        ready: 0,
        unread: 0,
    }
}

/// Reads what the view of the Claude Code session `session` is drawn from, for the person whose
/// principals are named `person`, through `graph`, any project's graph of the node.
pub async fn read(graph: &ProjectGraph, session: &str, person: &str) -> Result<Held> {
    let holdings = working_set::read(graph, Holder::Session(session), &extent()).await?;
    let projects = graph.projects().await?;
    let mut principals = HashSet::new();
    for project in &projects {
        let all = ops::principals(&graph.graph_of_project(project)).await?;
        principals.extend(naming::by_name(&all, person, None).iter().map(|p| p.id()));
    }
    let mut person = Person { handle: naming::bare(person).to_string(), principals, answered: HashSet::new() };
    let asked: Vec<EntityId> = holdings
        .iter()
        .flat_map(|h| you_asked(h).filter(|r| person.asked(r)).map(|r| r.id).collect::<Vec<_>>())
        .collect();
    for question in asked {
        let answers = graph.answers_to(question).await?;
        // An answer whose author cannot be read is not known to be the person's.
        if answers.iter().any(|m| m.author().is_ok_and(|a| person.principals.contains(&EntityId::from(a)))) {
            person.answered.insert(question);
        }
    }
    Ok(Held { holdings, person, projects })
}

impl Person {
    /// Whether `question` asks the person: by one of the person's principals' ids, or by their name.
    fn asked(&self, question: &Record) -> bool {
        match &question.kind {
            RecordKind::Ask { to: Addressee::Principal { id, .. }, .. } => self.principals.contains(id),
            RecordKind::Ask { to: Addressee::Name(name), .. } => naming::bare(name) == self.handle,
            _ => false,
        }
    }
}

/// The open questions `h`'s principal asked.
fn you_asked(h: &Holding) -> impl Iterator<Item = &Record> {
    h.sections.iter().filter(|s| s.kind == SectionKind::YouAsked).flat_map(|s| &s.records)
}

impl Held {
    /// The live queries whose changes can change the view: the projects, each project's principals,
    /// each principal's working set, and the threads of the questions asked of the person.
    pub fn live_reads(&self) -> BTreeSet<LiveRead> {
        let mut reads = BTreeSet::from([LiveRead::Projects]);
        reads.extend(self.projects.iter().cloned().map(LiveRead::Principals));
        for h in &self.holdings {
            reads.extend(h.live_reads());
            reads.extend(you_asked(h).filter(|r| self.person.asked(r)).map(|r| LiveRead::AnswersTo(r.id)));
        }
        reads
    }

    /// The document, its rows' paths following `page`, the page's address.
    pub fn document(&self, page: &str) -> Document {
        if self.holdings.is_empty() {
            let unbound = "No fwd principal is bound to this session yet. Its agent names itself with `fwd principal add <name> --role <role>`.";
            return Document {
                title: TITLE.into(),
                page: Some(page.into()),
                lines: vec![unbound.into()],
                principals: Vec::new(),
            };
        }
        let mut principals: Vec<Group> = self.holdings.iter().map(|h| self.group(h)).collect();
        fit::within_budget(&mut principals, |g, kind| self.more(&self.holdings[g], kind));
        Document { title: TITLE.into(), page: Some(page.into()), lines: Vec::new(), principals }
    }

    /// The page's own list of what the list `kind` of `h` holds, where there is one: the questions
    /// asked of the person, on their Questions page in `h`'s project. No page lists what one
    /// principal holds or what it asked others.
    fn more(&self, h: &Holding, kind: ListKind) -> Option<String> {
        let theirs = Route::Questions { question: None, whose: Some(self.person.handle.clone()), scope: None };
        (kind == ListKind::WaitsOnYou).then(|| page_path(h.project.uuid, &theirs))
    }

    fn group(&self, h: &Holding) -> Group {
        let section = |kind| h.sections.iter().find(|s| s.kind == kind);
        let mut waits_on_you = Vec::new();
        let mut waits_on_others = Vec::new();
        for r in you_asked(h).filter(|r| !self.person.answered.contains(&r.id)) {
            if self.person.asked(r) {
                waits_on_you.push(self.row(h, r, ListKind::WaitsOnYou));
            } else {
                waits_on_others.push(self.row(h, r, ListKind::WaitsOnOthers));
            }
        }
        let holds: Vec<Row> = [SectionKind::AskedOfYou, SectionKind::Assigned]
            .into_iter()
            .filter_map(section)
            .flat_map(|s| &s.records)
            .map(|r| self.row(h, r, ListKind::Holds))
            .collect();
        let unreadable = |kind, noun| {
            section(kind)
                .map(|s| s.unreadable.len())
                .filter(|n| *n > 0)
                .map(|n| format!("fwd could not read {n} {noun}."))
        };
        let lists = [
            (ListKind::WaitsOnYou, waits_on_you, Vec::new()),
            (
                ListKind::Holds,
                holds,
                [
                    unreadable(SectionKind::AskedOfYou, "of the questions asked of it"),
                    unreadable(SectionKind::Assigned, "of the tasks assigned to it"),
                ]
                .into_iter()
                .flatten()
                .collect(),
            ),
            (
                ListKind::WaitsOnOthers,
                waits_on_others,
                unreadable(SectionKind::YouAsked, "of the questions it asked, so whom they ask is not known")
                    .into_iter()
                    .collect(),
            ),
        ];
        let sections: Vec<List> = lists
            .into_iter()
            .filter(|(_, rows, lines)| !rows.is_empty() || !lines.is_empty())
            .map(|(kind, records, lines)| List {
                section: kind,
                title: kind.title().into(),
                folded: kind == ListKind::WaitsOnOthers,
                lines,
                shown: records.len(),
                of: records.len(),
                more: None,
                records,
            })
            .collect();
        let own_thread = h.subject.as_deref().is_some_and(subject::is_main_thread);
        let waits = sections.iter().any(|s| s.section == ListKind::WaitsOnYou);
        Group {
            id: h.principal.to_base64(),
            title: text::plain(&h.handle),
            tags: self.group_tags(h),
            folded: !own_thread && !waits,
            lines: Vec::new(),
            sections,
        }
    }

    /// A group's tags: what the principal is, its project, and its unread notifications.
    fn group_tags(&self, h: &Holding) -> Vec<Tag> {
        let what = match (h.kind, h.role) {
            (PrincipalKind::Human, _) => "person".to_string(),
            (_, Some(role)) => role.to_string(),
            (_, None) => "agent".to_string(),
        };
        let unread = h.sections.iter().find(|s| s.kind == SectionKind::Unread).map_or(0, |s| s.total);
        let mut tags = vec![Tag::plain(&what), Tag::plain(&h.project.name)];
        if unread > 0 {
            tags.push(Tag::plain(&format!("{unread} unread")));
        }
        tags
    }

    /// `r`, a record of `h`'s, as a row of the list `kind`.
    fn row(&self, h: &Holding, r: &Record, kind: ListKind) -> Row {
        let asked = Some(crate::calendar::instant_utc(r.created));
        let (row_kind, mut tags) = match (&r.kind, kind) {
            (RecordKind::Task { status }, _) => (RowKind::Task, vec![Tag::plain(&status_words(*status))]),
            (RecordKind::Ask { .. }, ListKind::WaitsOnYou) => {
                (RowKind::Ask, vec![Tag { text: "asked".into(), tone: Some(Tone::Attention), at: asked }])
            }
            (RecordKind::Ask { from, .. }, ListKind::Holds) => {
                (RowKind::Ask, vec![Tag { text: text::plain(&format!("from {from}")), tone: None, at: asked }])
            }
            (RecordKind::Ask { to, .. }, _) => {
                let to = match to {
                    Addressee::Principal { handle, .. } => format!("to {handle}"),
                    Addressee::Name(name) => format!("to {}", naming::bare(name)),
                    Addressee::NoOne => "to no one in particular".to_string(),
                };
                (RowKind::Ask, vec![Tag { text: text::plain(&to), tone: None, at: asked }])
            }
            (RecordKind::Notification, _) => unreachable!("the view reads no notification as a record"),
        };
        if r.project.record != h.project.record {
            tags.push(Tag::plain(&format!("in {}", r.project.name)));
        }
        let actions = match kind {
            ListKind::WaitsOnYou => vec![self.answer(r)],
            _ => Vec::new(),
        };
        Row {
            id: r.id.to_base64(),
            kind: row_kind,
            title: text::plain(&r.title),
            tags,
            lines: text::block(&r.text).into_iter().collect(),
            path: r.route.as_ref().map(|route| page_path(r.project.uuid, route)),
            actions,
        }
    }

    /// Answer, on a question asked of the person: a reply in its thread, in the person's words on
    /// standard input, made as the person in the question's project, which says nothing when it
    /// succeeds and one line on standard error when it fails (`--format pane`).
    fn answer(&self, question: &Record) -> Action {
        let args = [
            "--project",
            &question.project.uuid.to_string(),
            "--as",
            &self.person.handle,
            "--format",
            "pane",
            "reply",
            &question.id.to_base64(),
            "--text-from-stdin",
        ];
        Action {
            label: "Answer".into(),
            args: args.into_iter().map(str::to_string).collect(),
            input: Some(Input { prompt: "Your answer".into(), multiline: true }),
        }
    }
}

impl ListKind {
    fn title(self) -> &'static str {
        match self {
            ListKind::WaitsOnYou => "Waiting on you",
            ListKind::WorksUnder => "Works under",
            ListKind::Holds => "Holds",
            ListKind::WaitsOnOthers => "Waiting on others",
        }
    }
}

impl Tag {
    fn plain(text: &str) -> Tag {
        Tag { text: text::plain(text), tone: None, at: None }
    }
}

/// A task's status as a person reads it: `in progress`, not `in_progress`.
fn status_words(status: TaskStatus) -> String {
    status.to_string().replace('_', " ")
}

impl Document {
    /// The document while the daemon is out of reach: what was last shown, saying so, and no page
    /// to open.
    pub fn out_of_reach(&self) -> Document {
        let said =
            "fwd's daemon is out of reach; this is what it last said, and it will show again when the daemon answers.";
        Document { title: TITLE.into(), page: None, lines: vec![said.into()], principals: self.principals.clone() }
    }

    /// The document after a read that failed with the daemon in reach: what was last shown, saying
    /// why it could not be read again.
    pub fn unreadable(&self, why: &str) -> Document {
        let said = format!(
            "fwd could not read what this session holds ({}); this is what it last read, and it will show again when a read succeeds.",
            text::plain(why)
        );
        Document {
            title: TITLE.into(),
            page: self.page.clone(),
            lines: vec![said],
            principals: self.principals.clone(),
        }
    }

    /// The document before anything was read: the daemon out of reach, and nothing to show.
    pub fn never_read() -> Document {
        Document { title: TITLE.into(), page: None, lines: Vec::new(), principals: Vec::new() }.out_of_reach()
    }
}

#[cfg(test)]
mod tests;
