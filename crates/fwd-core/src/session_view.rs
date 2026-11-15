//! The person's view of a Claude Code session: what the session holds in fwd, as one document that
//! a pane beside the chat draws, and later the page (Daniel, 2026-11-13: it "must not be a wall of
//! text, but a really clean and readable list. I shouldn't even see any of the entity ids, but
//! rather click them to get more detail"). fwd owns what the view says, its order and what can be
//! done from it, so that whatever draws it knows no fwd verb and draws every section and every
//! record the same way: a principal is a [`Group`], a section a headed [`List`], a record a [`Row`],
//! its title and tags, that opens into its lines and its actions (docs/design/cli.md, "The person's
//! view of a session").
//!
//! First, at its root, the session's active intentions: those assigned to the principals bound to
//! its own thread, of any project, read as `fwd prime` reads them (`intentions::assigned_to`). Then
//! the principals bound to the session's own thread, in every project, then those bound to its
//! subagents, each by project name, then by name, read by the one working-set read `fwd prime`
//! makes (`working_set`), so that the two cannot disagree about what a session holds. For each
//! principal, in this order: what waits on the person, the open questions it asked them that they
//! have not answered in the question's thread, as the page's Waiting on you counts a question; what
//! it is working on, the questions asked of it and then the tasks assigned to it; and what waits on
//! others, the open questions it asked of anyone else or of no one in particular. Its unread
//! notifications are counted on its group, and the ready tasks are not in the view. No string of
//! the document holds an id: a text's references are named (`cited`), and a principal is named by
//! its name.

mod cited;
mod fit;
mod markdown;
mod person;
mod text;

pub use person::{Person, Reader};
pub use text::one_line;

use crate::decision_log::date;
use crate::intentions::{self, Parts};
use crate::live_read::LiveRead;
use crate::model::{PlacementView, PrincipalKind, PrincipalView, TaskStatus};
use crate::naming;
use crate::project_graph::{Followed, ProjectGraph, ProjectGraphError, ProjectRecord, Whose};
use crate::reason;
use crate::route::{page_path, Route};
use crate::subject;
use crate::working_set::{self, Addressee, Extent, Holding, Known, Party, Record, SectionKind};
use ankurah::property::PropertyError;
use ankurah::EntityId;
use ankurah_commons::records::{AttributionView, ClosureView, LastEditView};
use anyhow::{anyhow, Result};
use cited::Cited;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use text::{block, plain, unpaged_block, LINE_LIMIT, TAG_LIMIT, TITLE_LIMIT};

/// The document's title, the same in every state.
pub const TITLE: &str = "fwd Current Session";

/// What the section of the active intentions says when none is assigned to the session.
const NO_INTENTIONS: &str = "No active intentions recorded for this session yet.";

/// What the section of the active intentions says before fwd has read them.
const INTENTIONS_NOT_READ: &str = "fwd cannot read the intentions assigned to this session right now.";

/// What the view says while the daemon is out of reach, above what it last showed.
pub const OUT_OF_REACH: &str =
    "fwd's daemon is out of reach; this is what it last said, and it will show again when the daemon answers.";

/// How a principal the view cannot find is named, without its id.
const NOT_FOUND: &str = "someone fwd cannot find";

/// The view of one session at one moment. It holds no clock reading and no counter, so that two
/// documents are equal when nothing they show changed; an age is a tag's `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Document {
    pub title: String,
    /// The page's address, which each row's `path` follows; none while the daemon is out of reach.
    pub page: Option<String>,
    /// What is said of the whole view: that no principal is bound to the session yet, that the
    /// daemon is out of reach or a read failed, what keeps the reader from answering, and groups
    /// left out.
    pub lines: Vec<String>,
    /// The session's own sections, before its principals: its active intentions.
    pub sections: Vec<List>,
    pub principals: Vec<Group>,
}

/// A principal bound to the session, with the lists of what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    /// The principal's id, the same from one document to the next, written as its base64.
    pub id: EntityId,
    /// Its name.
    pub title: String,
    /// Its role, its project, for a subagent the task it has in progress, and how many unread
    /// notifications it has, when it has any.
    pub tags: Vec<Tag>,
    pub lines: Vec<String>,
    /// Those with records or lines, in [`ListKind`]'s order.
    pub sections: Vec<List>,
}

/// One section of the view, of the session's own or of a principal's group: a headed list of rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct List {
    pub section: ListKind,
    pub prominence: Prominence,
    pub title: String,
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

/// The sections of the view, in the order they stand: the session's own, then a principal's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    /// The intentions assigned to the principals of the session's own thread that it holds.
    ActiveIntentions,
    /// The open questions the principal asked the person that the person has not answered.
    WaitsOnYou,
    /// The questions asked of the principal, then the tasks assigned to it.
    Holds,
    /// The open questions it asked of anyone else, or of no one in particular.
    WaitsOnOthers,
}

/// How a section is drawn beside the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Prominence {
    /// Above everything else: the session's active intentions.
    Primary,
    Secondary,
}

/// A record, as a row that opens into its lines and its actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    /// The record's id, unique within its list, written as its base64.
    pub id: EntityId,
    pub kind: RowKind,
    /// Plain text on one line.
    pub title: String,
    /// Whether `title` is the start of the text the first of `lines` holds, cut short, as a
    /// question's is where its text is longer than its title. A reader that shows that text need
    /// not show the title above it.
    pub title_is_cut: bool,
    pub tags: Vec<Tag>,
    /// The record's own text as one Markdown block, cut where it is long, its references named;
    /// none for a task with no what, and none where the title holds the whole text, which `lines`
    /// never repeats. Then why it cannot be answered, where it cannot. An intention's is its words,
    /// and for one since fulfilled or withdrawn a line saying so.
    pub lines: Vec<String>,
    /// Whose words an intention's `lines` hold and when they were last written down, kept apart
    /// from the words so that a reader can show it beside them, less prominently; none for any
    /// other record.
    pub attribution: Option<Byline>,
    /// Where the page shows it, after the page's address.
    pub path: Option<String>,
    pub actions: Vec<Action>,
}

/// Whose words a row holds, and the day they were last written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Byline {
    /// The speaker's display name, plain text on one line.
    pub by: String,
    /// The day, as `YYYY-MM-DD` in this machine's time zone.
    pub at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    Ask,
    Task,
    Intention,
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
    /// It waits on the person.
    Attention,
}

/// What the person can do from a row: run the program `fwd` with `args`, no shell, from any
/// directory, with no session in its environment. The arguments name the project and the person's
/// principal there by its id, so the command acts as the principal the view checked; text the
/// person types goes to its standard input.
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

/// What the view is drawn from: the session's active intentions, the working set of every
/// principal bound to the session, the person it is for, and the names it shows in place of ids.
#[derive(Debug, Clone)]
pub struct Held {
    /// The Claude Code session's id.
    pub session: String,
    /// Those of the session's own thread first, then its subagents'
    /// ([`working_set::read_session`]).
    pub holdings: Vec<Holding>,
    pub person: Person,
    /// Every project of the node.
    pub projects: Vec<ProjectRecord>,
    /// The name of every principal of every project, by id, which names the other party of a
    /// question.
    names: HashMap<EntityId, String>,
    cited: Cited,
    /// The session's active intentions as rows, made as they are read, since no address enters
    /// them; then how many assigned intentions could not be read.
    intentions: Vec<Row>,
    unreadable_intentions: usize,
    /// The ids [`intentions::assigned_to`] gives for the principals of the session's own thread,
    /// read or not, whose changes the view listens to.
    assigned: Vec<EntityId>,
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

/// Reads what the view of the Claude Code session `session` is drawn from, for the person `reader`
/// names, through `graph`, any project's graph of the node. The projects and their principals are
/// read once.
pub async fn read(graph: &ProjectGraph, session: &str, reader: Reader<'_>) -> Result<Held> {
    let known = Known::read(graph).await?;
    let mut person = Person::resolve(&known, reader);
    let holdings = working_set::read_session(graph, &known, session, &extent()).await?;
    let waiting: Vec<EntityId> =
        holdings.iter().flat_map(Holding::asked_by).filter(|(r, to)| person.asked(r, to)).map(|(r, _)| r.id).collect();
    for question in waiting {
        let answers = graph.answers_to(question).await?;
        // An answer whose author cannot be read is not known to be the person's.
        if answers.iter().any(|m| m.author().is_ok_and(|author| person.is(EntityId::from(author)))) {
            person.answered.insert(question);
        }
    }
    let here = known
        .projects
        .iter()
        .find(|(project, _)| project.record == graph.record())
        .map(|(project, _)| project.uuid)
        .ok_or_else(|| anyhow!("the node holds no record of the project the view joined through"))?;
    let records = holdings.iter().flat_map(|h| h.sections.iter().flat_map(|s| &s.records));
    let texts = records.flat_map(|r| [Some(r.title.as_str()), r.text.as_deref()]).flatten();
    let cited = Cited::read(graph, here, texts).await?;
    let names =
        known.projects.iter().flat_map(|(_, all)| all).filter_map(|p| Some((p.id(), p.handle().ok()?))).collect();
    let projects = known.projects.into_iter().map(|(project, _)| project).collect();
    let (mut intentions, mut unreadable_intentions, mut assigned) = (Vec::new(), 0, Vec::new());
    let own_thread = holdings.iter().filter(|h| h.subject.as_deref().is_some_and(subject::is_main_thread));
    for h in own_thread {
        let there = graph.graph_of_project(&h.project);
        for (id, followed) in intentions::assigned_to(&there, h.principal).await? {
            if assigned.contains(&id) {
                continue;
            }
            assigned.push(id);
            let row = match followed {
                Followed::Held { project, found, .. } => intention_row(&there, &project, &found).await.ok(),
                Followed::NotHeld => None,
            };
            match row {
                Some(row) => intentions.push(row),
                // Said by count in the section, as a list says what of it could not be read.
                None => unreadable_intentions += 1,
            }
        }
    }
    Ok(Held {
        session: session.to_string(),
        holdings,
        person,
        projects,
        names,
        cited,
        intentions,
        unreadable_intentions,
        assigned,
    })
}

/// An intention assigned to a principal of the session's own thread, read in `there`, that
/// principal's project's graph, where `project` says whose it is, as a row: titled by its title;
/// its words its line, attributed to their speaker, by name, and the day they were last written
/// down; for one since fulfilled or withdrawn, a line after them saying so and when, and a tag; its
/// project's name as a tag when that is not its principal's; no path, since the page has none for
/// an intention yet, and no action.
async fn intention_row(there: &ProjectGraph, project: &Whose, record: &(PlacementView, Parts)) -> Result<Row> {
    let (placement, (intention, creation)) = record;
    let attribution = there.component_of::<AttributionView>(placement).await?;
    let speaker = intentions::speaker(creation, attribution.as_ref())?;
    let name = match there.follow::<PrincipalView>(speaker).await? {
        Followed::Held { found, .. } => found.display_name()?,
        Followed::NotHeld => NOT_FOUND.into(),
    };
    let edit = there.component_of::<LastEditView>(placement).await?;
    let written = intentions::last_written(creation, edit.as_ref())?;
    let mut lines: Vec<String> = unpaged_block(&intention.words()?).into_iter().collect();
    let mut tags = Vec::new();
    if let Whose::Other(p) = project {
        tags.push(Tag::plain(&p.name));
    }
    let status = intention.status()?;
    if intentions::ended(status) {
        let closed = match there.component_of::<ClosureView>(placement).await? {
            Some(closure) => closure.closed()?,
            None => None,
        };
        lines.push(match closed {
            Some(at) => format!("{status} on {}", date(at)),
            None => status.to_string(),
        });
        tags.push(Tag::plain(&status.to_string()));
    }
    Ok(Row {
        id: placement.id(),
        kind: RowKind::Intention,
        title: plain(&intention.title()?, TITLE_LIMIT),
        title_is_cut: false,
        tags,
        lines,
        attribution: Some(Byline { by: plain(&name, TITLE_LIMIT), at: date(written) }),
        path: None,
        actions: Vec::new(),
    })
}

/// The other party of a question, as the list it stands in names it.
#[derive(Clone, Copy)]
enum Asked<'a> {
    /// It asks the person.
    You,
    /// It was asked of the principal by this party.
    From(&'a Party),
    /// The principal asked it of this addressee.
    To(&'a Addressee),
}

impl Held {
    /// The live queries whose changes can change the view: the projects, each project's principals
    /// bound to the session, each principal's working set, the threads of the questions asked of
    /// the person, the intentions assigned to the principals of the session's own thread, and each
    /// of those intentions.
    pub fn live_reads(&self) -> BTreeSet<LiveRead> {
        let mut reads = BTreeSet::from([LiveRead::Projects]);
        let session = |project: &ProjectRecord| LiveRead::SessionPrincipals {
            project: project.clone(),
            session: self.session.clone(),
        };
        reads.extend(self.projects.iter().map(session));
        for h in &self.holdings {
            reads.extend(h.live_reads());
            reads.extend(
                h.asked_by().filter(|(r, to)| self.person.asked(r, to)).map(|(r, _)| LiveRead::AnswersTo(r.id)),
            );
            if h.subject.as_deref().is_some_and(subject::is_main_thread) {
                reads.insert(LiveRead::IntentionsAssignedTo(h.principal));
            }
        }
        reads.extend(self.assigned.iter().map(|id| LiveRead::AssignedIntention(*id)));
        reads
    }

    /// The document, its rows' paths following `page`, the page's address.
    pub fn document(&self, page: &str) -> Document {
        let mut lines = self.person.lines();
        let sections = vec![self.active_intentions()];
        if self.holdings.is_empty() {
            let unbound = "No fwd principal is bound to this session yet. Its agent names itself with `fwd principal add <name> --role <role>`.";
            lines.insert(0, unbound.into());
            return Document { title: TITLE.into(), page: Some(page.into()), lines, sections, principals: Vec::new() };
        }
        let principals = self.holdings.iter().map(|h| self.group(h, page)).collect();
        let mut document = Document { title: TITLE.into(), page: Some(page.into()), lines, sections, principals };
        fit::within_budget(&mut document, |g, kind| self.more(&self.holdings[g], kind));
        document
    }

    /// The section of the session's active intentions, shown whether or not it holds any: its rows,
    /// or the line saying there are none, and how many could not be read.
    fn active_intentions(&self) -> List {
        let mut lines = Vec::new();
        if self.intentions.is_empty() && self.unreadable_intentions == 0 {
            lines.push(NO_INTENTIONS.into());
        }
        if self.unreadable_intentions > 0 {
            let n = self.unreadable_intentions;
            lines.push(plain(
                &format!("fwd could not read {n} of the intentions assigned to this session."),
                LINE_LIMIT,
            ));
        }
        ListKind::ActiveIntentions.list(lines, self.intentions.clone())
    }

    /// The page's own list of what the list `kind` of `h` holds, where the page has one that lists
    /// all of it: the questions asked of the person, on their Questions page in `h`'s project, when
    /// every one of them is in that project, which is all that page lists. No page lists what one
    /// principal holds or what it asked others.
    fn more(&self, h: &Holding, kind: ListKind) -> Option<String> {
        let me = self.person.principal_in(h.project.record).filter(|_| kind == ListKind::WaitsOnYou)?;
        let mut waiting =
            h.asked_by().filter(|(r, to)| self.person.asked(r, to) && !self.person.answered.contains(&r.id));
        if waiting.any(|(r, _)| r.project.record != h.project.record) {
            return None;
        }
        let theirs = Route::Questions { question: None, whose: Some(me.to_base64()), scope: None };
        Some(page_path(h.project.uuid, &theirs))
    }

    fn group(&self, h: &Holding, page: &str) -> Group {
        let section = |kind| h.sections.iter().find(|s| s.kind == kind);
        let mut waits_on_you = Vec::new();
        let mut waits_on_others = Vec::new();
        for (r, to) in h.asked_by().filter(|(r, _)| !self.person.answered.contains(&r.id)) {
            if self.person.asked(r, to) {
                waits_on_you.push(self.ask_row(h, r, Asked::You, page));
            } else {
                waits_on_others.push(self.ask_row(h, r, Asked::To(to), page));
            }
        }
        let mut holds: Vec<Row> = h.asked_of().map(|(r, from)| self.ask_row(h, r, Asked::From(from), page)).collect();
        holds.extend(h.assigned().map(|(r, status)| self.task_row(h, r, status, page)));
        let unreadable = |kind, noun| {
            section(kind)
                .map(|s| s.unreadable.len())
                .filter(|n| *n > 0)
                .map(|n| plain(&format!("fwd could not read {n} {noun}."), LINE_LIMIT))
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
            .map(|(kind, records, lines)| kind.list(lines, records))
            .collect();
        let own_thread = h.subject.as_deref().is_some_and(subject::is_main_thread);
        Group {
            id: h.principal,
            title: plain(&h.handle, TITLE_LIMIT),
            tags: self.group_tags(h, own_thread),
            lines: Vec::new(),
            sections,
        }
    }

    /// A group's tags: what the principal is, its project, for a subagent what it is doing (the
    /// first task it has in progress), and its unread notifications.
    fn group_tags(&self, h: &Holding, own_thread: bool) -> Vec<Tag> {
        let what = match (h.kind, h.role) {
            (PrincipalKind::Human, _) => "person".to_string(),
            (_, Some(role)) => role.to_string(),
            (_, None) => "agent".to_string(),
        };
        let mut tags = vec![Tag::plain(&what), Tag::plain(&h.project.name)];
        if !own_thread {
            let doing = h.assigned().find(|(_, status)| *status == TaskStatus::InProgress);
            tags.extend(doing.map(|(r, _)| Tag::plain(&self.cited.plain(&r.title))));
        }
        let unread = h.sections.iter().find(|s| s.kind == SectionKind::Unread).map_or(0, |s| s.total);
        if unread > 0 {
            tags.push(Tag::plain(&format!("{unread} unread")));
        }
        tags
    }

    /// `r`, a question of `h`'s, as a row: titled by its first words, its references named, a cut
    /// of its text where that is longer; dated by when it was asked; and answerable where it asks
    /// the person and they have a principal of their own where it is.
    fn ask_row(&self, h: &Holding, r: &Record, asked: Asked<'_>, page: &str) -> Row {
        let text = r.text.as_deref().map(|text| self.cited.plain(text));
        let title = text
            .as_deref()
            .map_or_else(|| self.cited.plain(&r.title), |text| reason::first_words(text, reason::NAME_WORDS));
        let at = r.created.map(crate::calendar::instant_utc);
        let tag = match asked {
            Asked::You => Tag { text: "asked".into(), tone: Some(Tone::Attention), at },
            Asked::From(party) => Tag::dated(&format!("from {}", self.name(party.id)), at),
            Asked::To(Addressee::Principal(party)) => Tag::dated(&format!("to {}", self.name(party.id)), at),
            Asked::To(Addressee::Name(name)) => Tag::dated(&format!("to {}", naming::bare(name)), at),
            Asked::To(Addressee::NoOne) => Tag::dated("to no one in particular", at),
        };
        let (actions, why_not) = match (asked, self.person.principal_in(r.project.record)) {
            (Asked::You, Some(me)) => (vec![answer(r, me)], None),
            (Asked::You, None) => (Vec::new(), self.person.cannot_answer(r)),
            _ => (Vec::new(), None),
        };
        let mut row = self.row(h, r, RowKind::Ask, &title, vec![tag], page);
        // A text that could be read is the row's line unless the title, its first words, holds it
        // whole; why it cannot be answered comes after.
        row.title_is_cut = text.is_some() && !row.lines.is_empty();
        row.lines.extend(why_not);
        row.actions = actions;
        row
    }

    /// `r`, a task assigned to `h`'s principal, as a row tagged by its status.
    fn task_row(&self, h: &Holding, r: &Record, status: TaskStatus, page: &str) -> Row {
        self.row(h, r, RowKind::Task, &self.cited.plain(&r.title), vec![Tag::plain(&status_words(status))], page)
    }

    /// A row of `r`, of `h`'s, with `title` and `tags`: the record's project named when it is not
    /// `h`'s, and its own text as its line, unless the title holds it whole; where its text could
    /// not be read, a line says so.
    fn row(&self, h: &Holding, r: &Record, kind: RowKind, title: &str, mut tags: Vec<Tag>, page: &str) -> Row {
        if r.project.record != h.project.record {
            tags.push(Tag::plain(&format!("in {}", r.project.name)));
        }
        let title = plain(title, TITLE_LIMIT);
        let lines = match r.text.as_deref() {
            None => vec!["Its text could not be read.".into()],
            Some(text) if one_line(&self.cited.plain(text)) == title => Vec::new(),
            Some(text) => block(&self.cited.markdown(text, page)).into_iter().collect(),
        };
        Row {
            id: r.id,
            kind,
            title,
            title_is_cut: false,
            tags,
            lines,
            attribution: None,
            path: r.route.as_ref().map(|route| page_path(r.project.uuid, route)),
            actions: Vec::new(),
        }
    }

    /// The name of the principal `id`, without its id where fwd cannot find it.
    fn name(&self, id: EntityId) -> String {
        self.names.get(&id).cloned().unwrap_or_else(|| NOT_FOUND.into())
    }
}

/// Answer, on a question asked of the person: a reply in its thread, in the person's words on
/// standard input (the text `-`), made as `me`, the person's principal in the question's project,
/// in that project, which says nothing when it succeeds and one line on standard error when it
/// fails (`--format pane`).
fn answer(question: &Record, me: EntityId) -> Action {
    let args = [
        "--project",
        &question.project.uuid.to_string(),
        "--as",
        &me.to_base64(),
        "--format",
        "pane",
        "reply",
        &question.id.to_base64(),
        "-",
    ];
    Action {
        label: "Answer".into(),
        args: args.into_iter().map(str::to_string).collect(),
        input: Some(Input { prompt: "Your answer".into(), multiline: true }),
    }
}

impl ListKind {
    fn title(self) -> &'static str {
        match self {
            ListKind::ActiveIntentions => "Active Intentions",
            ListKind::WaitsOnYou => "Waiting On You",
            ListKind::Holds => "Working On",
            ListKind::WaitsOnOthers => "Waiting On Others",
        }
    }

    fn prominence(self) -> Prominence {
        match self {
            ListKind::ActiveIntentions => Prominence::Primary,
            ListKind::WaitsOnYou | ListKind::Holds | ListKind::WaitsOnOthers => Prominence::Secondary,
        }
    }

    /// The list of this kind that shows every one of `records`, with `lines` said of it.
    fn list(self, lines: Vec<String>, records: Vec<Row>) -> List {
        List {
            section: self,
            prominence: self.prominence(),
            title: self.title().into(),
            lines,
            shown: records.len(),
            of: records.len(),
            more: None,
            records,
        }
    }
}

impl Tag {
    fn plain(text: &str) -> Tag {
        Tag { text: plain(text, TAG_LIMIT), tone: None, at: None }
    }

    fn dated(text: &str, at: Option<String>) -> Tag {
        Tag { at, ..Tag::plain(text) }
    }
}

/// A task's status as a person reads it, as a task's row is tagged by it: `in progress`, not
/// `in_progress`.
pub fn status_words(status: TaskStatus) -> String {
    status.to_string().replace('_', " ")
}

impl Document {
    /// The document while the daemon is out of reach: what was last shown, saying so, and no page
    /// to open.
    pub fn out_of_reach(&self) -> Document {
        Document {
            title: TITLE.into(),
            page: None,
            lines: vec![OUT_OF_REACH.into()],
            sections: self.sections.clone(),
            principals: self.principals.clone(),
        }
    }

    /// The document after a read that failed with the daemon in reach: what was last shown, saying
    /// in plain words, with no id, why it could not be read again.
    pub fn unreadable(&self, error: &anyhow::Error) -> Document {
        let field = |cause: &(dyn std::error::Error + 'static)| {
            cause.is::<PropertyError>()
                || matches!(cause.downcast_ref::<ProjectGraphError>(), Some(ProjectGraphError::Property(_)))
        };
        let why = if error.chain().any(field) {
            "a field of a record it reads could not be read"
        } else {
            "the daemon did not give what it asked for"
        };
        let said =
            format!("fwd could not read what this session holds: {why}. This is what it last read; it tries again.");
        Document {
            title: TITLE.into(),
            page: self.page.clone(),
            lines: vec![said],
            sections: self.sections.clone(),
            principals: self.principals.clone(),
        }
    }

    /// The document before anything was read: the daemon out of reach, and nothing to show but the
    /// section of the session's active intentions, which stands in every document, saying fwd
    /// cannot read them yet.
    pub fn never_read() -> Document {
        let intentions = ListKind::ActiveIntentions.list(vec![INTENTIONS_NOT_READ.into()], Vec::new());
        Document {
            title: TITLE.into(),
            page: None,
            lines: Vec::new(),
            sections: vec![intentions],
            principals: Vec::new(),
        }
        .out_of_reach()
    }
}

#[cfg(test)]
mod tests;
