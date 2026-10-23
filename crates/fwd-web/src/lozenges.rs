//! The lozenges on a row: every entity joined to the row's entity by a relation, in either
//! direction, named in a few words on the row's own line, so that what hangs beneath a card is
//! the conversation alone and a reference-happy writer cannot bury it (Daniel, 2026-09-26: "I
//! think a better approach would be to have each referent be merely a rendered as lozenge, and
//! the user can mouse over to get more detail, or click to navigate to that thing"; asked whether
//! that includes replies: "Not referring to replies. Just things like relates to, and such").
//! Replies, comments and forwards stay rows (`thread.rs`); a relation of any kind is a lozenge
//! here, and so is what a message was written on (its task, decision, scope or code).
//!
//! What a row's lozenges are is worked out as data first ([`plan`]): each relation's kind and
//! direction as seen from the row becomes a [`Facet`], each facet one group, in the order
//! [`TABLE`] lists them, each group's lozenges in order (a fallen ruling first, since it is the
//! warning), and how much of each group shows before anyone expands it. Not every kind is drawn
//! alike (Daniel: "Not all relationship types should be rendered the same"): [`TABLE`] gives each
//! facet its label, its look, and whether its lozenges show one by one up to a cap or start as
//! one lozenge counting them, so changing how one kind reads is changing one row there. That part
//! touches no Leptos, Ankurah or browser code and is tested natively.
//!
//! The rest of the module draws a plan ([`strip`] on a row's line, [`section`] under a panel's
//! heading), a lozenge standing alone ([`single`]: a reference in a message's text, the assignee
//! beside a task's scope), the one preview of whatever lozenge the pointer rests on or the
//! keyboard is on ([`LozengePreview`]; on a touch screen the first tap opens it and it carries an
//! "open" link), and where a click leads ([`destination`], [`follow`]). All of it reads the page's
//! standing queries and opens none of its own. These are the lozenge mode of every kind
//! (`modes.rs`).

use crate::referents::Models;
use crate::route::{self, Route};
use crate::thread::Model;
use crate::{anchor_row, decision_row, empty, fmt, message_row, person, references, session, store};
use ankurah::EntityId;
use fwd_core::anchor_name::{self, AnchorName};
use fwd_core::file_mention::Version;
use fwd_core::model::*;
use fwd_core::ops;
use fwd_core::rules::{NotReadyReason, Readiness};
use leptos::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Whether a ruling no longer stands, so what rests on it needs re-justifying. A rejected proposal
/// was never a ruling to rest on.
pub fn fallen(status: DecisionStatus) -> bool {
    matches!(status, DecisionStatus::Repealed | DecisionStatus::Superseded)
}

/// How another entity is joined to the row's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// By a relation of this kind.
    Relation(RelationKind),
    /// As what a message was written on: its task, decision, scope or code. A field of the
    /// message rather than a relation, but a referent all the same.
    Subject,
}

/// What the page knows of the entity at the other end, which decides how its lozenge looks and
/// where it sorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// A task: its status, and whether something besides its status holds it back (a hold, an
    /// unfinished or closed dependency). Only a dependency's lozenge asks the second.
    Task {
        status: TaskStatus,
        blocked: bool,
    },
    Decision {
        number: i64,
        status: DecisionStatus,
    },
    Message,
    Scope,
    Anchor,
    /// A person or an agent.
    Principal,
    /// Nothing the page holds: not arrived yet, or of a kind it draws no lozenge for.
    Unknown,
}

/// One entity joined to the row's, as the page's standing queries report it.
#[derive(Clone, Debug, PartialEq)]
pub struct Joined<I> {
    pub join: Join,
    /// Whether the row's entity is the relation's `from` end. A subject always is.
    pub outward: bool,
    pub status: RelationStatus,
    pub other: I,
    pub end: End,
    /// When the other entity was made (the relation, while the page does not hold it): the last
    /// word on the order within a group, after a fallen ruling first and rulings by number.
    pub created: i64,
}

/// A relation's kind and direction as the row it shows on reads it: a task "cites" a ruling that
/// is, from the ruling's row, "cited by" the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Facet {
    /// What a message was written on: its task, decision, scope or code.
    On,
    /// What a message is linked to by a `concerns` relation. (The messages about a task hang
    /// beneath it as rows, since they are the conversation.)
    About,
    /// The rulings it rests on.
    Cites,
    /// What rests on the ruling: the tasks, messages and rulings citing it.
    CitedBy,
    /// Where the ruling was made: the task or the question it was decided in.
    DecidedIn,
    /// The rulings made in a task's or a question's context.
    DecidedHere,
    /// The tasks it waits for.
    DependsOn,
    /// The tasks waiting for it.
    Blocks,
    /// The task it is part of.
    SubtaskOf,
    /// Its parts.
    Subtasks,
    /// The task or message the work surfaced from.
    DiscoveredFrom,
    /// The tasks that surfaced from it (a task filed from an answer).
    LedTo,
    MotivatedBy,
    Motivates,
    /// The task or ruling it replaces.
    Supersedes,
    /// The task or ruling that replaced it.
    SupersededBy,
    Duplicates,
    DuplicatedBy,
    /// The task it is a step of. (A task's own steps are its checklist, not lozenges.)
    StepOf,
    /// The messages that name it in their text (a `mention` edge, from the message). A message's
    /// own mentions are in its text, not in its lozenges.
    MentionedIn,
    /// Free association, either way.
    RelatesTo,
    /// A scope the task is worked in, one lozenge each.
    AssignedTo,
    /// The tasks a scope works.
    Assigned,
    /// The code a task concerns.
    AnchoredAt,
    /// The tasks concerning the highlight.
    CodeOf,
}

/// The families of facets, in the order a row lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    /// What a message is on or about: the reader's first question of a message.
    Subject,
    Citations,
    Dependencies,
    Structure,
    Lineage,
    /// The messages that name it.
    Mentions,
    Association,
    Assignment,
    Code,
}

/// How a facet's lozenges look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Named plainly as a link.
    Plain,
    /// A ruling's look: `D<n>` in monospace, then its title; a fallen ruling in the warning
    /// colour, a proposed one muted, a rejected one struck through.
    Ruling,
    /// The readiness-bearing kind: the other task's state shows, a done one muted and struck
    /// through, an open or in-progress one in the accent, a blocked one in the warning colour.
    Readiness,
    /// Part of a tree of tasks: a quiet glyph before the title, a done part struck through.
    Tree,
    /// Muted: where something came from, or what came of it.
    Lineage,
    /// Lineage that marks a fallen ruling: in the warning colour on a decision (the row's ruling
    /// was superseded), muted on a task.
    Succession,
    /// The quietest: free association.
    Quiet,
    /// A scope: its name in the green outline.
    Scope,
    /// Code: `path:lines` in monospace.
    Code,
}

/// A group whose lozenges all show whatever their number.
pub const ALL: usize = usize::MAX;

/// How a count lozenge says how many it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    /// The label, then the number: "blocks 4", "relates to 3".
    After,
    /// The number and the noun, then how many of them are done when any is: "3 dependencies
    /// (1 done)", "1 subtask".
    Noun { one: &'static str, many: &'static str },
}

/// How much of a group shows before anyone expands it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Each lozenge on its own, up to `cap`; past it the rest wait behind one "+N more"
    /// lozenge, which shows them in place.
    Each { cap: usize },
    /// Each lozenge on its own while there are at most `over`; past that, one lozenge counting
    /// them all, which shows them in place.
    Counted { over: usize, words: Words },
}

/// How one facet is drawn. One row per facet, in the order a row lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Treatment {
    pub facet: Facet,
    pub family: Family,
    /// The words before the group's lozenges ("cites", "cited by"); empty for none.
    pub label: &'static str,
    /// A quiet mark before each lozenge's name; empty for none.
    pub glyph: &'static str,
    pub style: Style,
    pub shown: Shown,
}

const fn row(
    facet: Facet,
    family: Family,
    label: &'static str,
    glyph: &'static str,
    style: Style,
    shown: Shown,
) -> Treatment {
    Treatment { facet, family, label, glyph, style, shown }
}

const fn each(cap: usize) -> Shown {
    Shown::Each { cap }
}

const fn counted(over: usize, words: Words) -> Shown {
    Shown::Counted { over, words }
}

const fn noun(one: &'static str, many: &'static str) -> Words {
    Words::Noun { one, many }
}

const DEPENDENCIES: Words = noun("dependency", "dependencies");
const SUBTASKS: Words = noun("subtask", "subtasks");
const ASSIGNED: Words = noun("task in this scope", "tasks in this scope");

/// Every facet's treatment, in the order a row lists its groups: what a message is on, then
/// what it rests on and what rests on it and where it was decided, dependencies, structure,
/// lineage, the messages naming it, association, the scopes, code (Daniel, 2026-09-26, "Not all
/// relationship types should be rendered the same": citations as rulings, capped at three; the
/// scope in its own look, one, with no label; dependencies carrying the other task's state and
/// counted past two; subtasks counted first; lineage muted, two then a count; relates to always
/// one count; code in monospace, two then a count). "One" was how many scopes a task had until a
/// task could be worked in several (docs/DECISIONS.md, D136, "A task may be worked in several
/// scopes", 2026-10-03): each of its scopes shows in that look, all of them, with no label. The
/// task a step belongs to sits with the structure, and "mentioned in" lists the messages naming
/// it, three then a count (2026-09-28, "Inline references are shadowed by `mention` edges": the
/// thing named lists what mentions it).
pub const TABLE: &[Treatment] = &[
    row(Facet::On, Family::Subject, "on", "", Style::Plain, each(ALL)),
    row(Facet::About, Family::Subject, "about", "", Style::Plain, each(2)),
    row(Facet::Cites, Family::Citations, "cites", "", Style::Ruling, each(3)),
    row(Facet::CitedBy, Family::Citations, "cited by", "", Style::Ruling, each(3)),
    row(Facet::DecidedIn, Family::Citations, "decided in", "", Style::Ruling, each(3)),
    row(Facet::DecidedHere, Family::Citations, "decided here", "", Style::Ruling, each(3)),
    row(Facet::DependsOn, Family::Dependencies, "depends on", "", Style::Readiness, counted(2, DEPENDENCIES)),
    row(Facet::Blocks, Family::Dependencies, "blocks", "", Style::Readiness, counted(2, Words::After)),
    row(Facet::SubtaskOf, Family::Structure, "subtask of", "↰", Style::Tree, each(ALL)),
    row(Facet::Subtasks, Family::Structure, "subtasks", "↳", Style::Tree, counted(0, SUBTASKS)),
    row(Facet::StepOf, Family::Structure, "step of", "↰", Style::Tree, each(ALL)),
    row(Facet::DiscoveredFrom, Family::Lineage, "discovered from", "", Style::Lineage, each(2)),
    row(Facet::LedTo, Family::Lineage, "led to", "", Style::Lineage, each(2)),
    row(Facet::MotivatedBy, Family::Lineage, "motivated by", "", Style::Lineage, each(2)),
    row(Facet::Motivates, Family::Lineage, "motivates", "", Style::Lineage, each(2)),
    row(Facet::Supersedes, Family::Lineage, "supersedes", "", Style::Lineage, each(2)),
    row(Facet::SupersededBy, Family::Lineage, "superseded by", "", Style::Succession, each(2)),
    row(Facet::Duplicates, Family::Lineage, "duplicates", "", Style::Lineage, each(2)),
    row(Facet::DuplicatedBy, Family::Lineage, "duplicated by", "", Style::Lineage, each(2)),
    row(Facet::MentionedIn, Family::Mentions, "mentioned in", "", Style::Plain, each(3)),
    row(Facet::RelatesTo, Family::Association, "relates to", "", Style::Quiet, counted(0, Words::After)),
    row(Facet::AssignedTo, Family::Assignment, "", "", Style::Scope, each(ALL)),
    row(Facet::Assigned, Family::Assignment, "in this scope", "", Style::Readiness, counted(0, ASSIGNED)),
    row(Facet::AnchoredAt, Family::Code, "code", "", Style::Code, each(2)),
    row(Facet::CodeOf, Family::Code, "code of", "", Style::Tree, each(2)),
];

/// The treatment of `facet`, and its place in [`TABLE`].
pub fn treatment(facet: Facet) -> (usize, &'static Treatment) {
    TABLE.iter().enumerate().find(|(_, t)| t.facet == facet).expect("every facet has a row in TABLE")
}

/// How a relation reads from the row it shows on; none for one the row shows otherwise: the
/// messages about a task hang beneath it as rows, since they are the conversation; a task's steps
/// are the checklist on its panel; a message's mentions are drawn where its text names them.
/// `own_decision` says whether the row's entity is a decision.
pub fn facet(join: Join, outward: bool, own_decision: bool, end: &End) -> Option<Facet> {
    use RelationKind as K;
    let kind = match join {
        Join::Subject => return outward.then_some(Facet::On),
        Join::Relation(kind) => kind,
    };
    Some(match (kind, outward) {
        (K::Concerns, true) => Facet::About,
        (K::Concerns, false) => return None,
        // A ruling cites the rulings it rests on, and the task or question it was decided in.
        (K::Cites, true) if own_decision && matches!(end, End::Task { .. } | End::Message) => Facet::DecidedIn,
        (K::Cites, true) => Facet::Cites,
        (K::Cites, false) if own_decision => Facet::CitedBy,
        (K::Cites, false) => Facet::DecidedHere,
        (K::DependsOn, true) => Facet::DependsOn,
        (K::DependsOn, false) => Facet::Blocks,
        (K::SubtaskOf, true) => Facet::SubtaskOf,
        (K::SubtaskOf, false) => Facet::Subtasks,
        (K::DiscoveredFrom, true) => Facet::DiscoveredFrom,
        (K::DiscoveredFrom, false) => Facet::LedTo,
        (K::MotivatedBy, true) => Facet::MotivatedBy,
        (K::MotivatedBy, false) => Facet::Motivates,
        (K::Supersedes, true) => Facet::Supersedes,
        (K::Supersedes, false) => Facet::SupersededBy,
        (K::Duplicates, true) => Facet::Duplicates,
        (K::Duplicates, false) => Facet::DuplicatedBy,
        (K::RelatesTo, _) => Facet::RelatesTo,
        (K::AssignedTo | K::WorkedIn, true) => Facet::AssignedTo,
        (K::AssignedTo | K::WorkedIn, false) => Facet::Assigned,
        (K::AnchoredAt, true) => Facet::AnchoredAt,
        (K::AnchoredAt, false) => Facet::CodeOf,
        (K::StepOf, true) => Facet::StepOf,
        (K::StepOf, false) => return None,
        (K::Mention, true) => return None,
        (K::Mention, false) => Facet::MentionedIn,
    })
}

/// How one lozenge looks, resolved from its facet's style and the entity it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// In the accent: a link like any other.
    Link,
    /// In the warning colour: a fallen ruling, a blocked dependency.
    Warn,
    /// Muted and struck through: done, closed, rejected.
    Done,
    Muted,
    /// The quietest.
    Quiet,
    /// The scope's green outline.
    Scope,
    /// Monospace.
    Code,
    /// A person or an agent: the handle in the ink colour.
    Person,
}

fn closed_task(end: &End) -> bool {
    matches!(end, End::Task { status: TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Rejected, .. })
}

/// The look of a lozenge naming `end` in a facet of `style`, on the row of an entity whose
/// decision status is `own` (none when it is not a decision).
pub fn tone(style: Style, end: &End, own: Option<DecisionStatus>) -> Tone {
    match style {
        Style::Plain => Tone::Link,
        Style::Ruling => match end {
            End::Decision { status, .. } if fallen(*status) => Tone::Warn,
            End::Decision { status: DecisionStatus::Rejected, .. } => Tone::Done,
            End::Decision { status: DecisionStatus::Proposed, .. } => Tone::Muted,
            _ => Tone::Link,
        },
        Style::Readiness if closed_task(end) => Tone::Done,
        Style::Readiness if matches!(end, End::Task { blocked: true, .. }) => Tone::Warn,
        Style::Readiness => Tone::Link,
        Style::Tree if closed_task(end) => Tone::Done,
        Style::Tree => Tone::Link,
        Style::Lineage => Tone::Muted,
        Style::Succession if own.is_some() => Tone::Warn,
        Style::Succession => Tone::Muted,
        Style::Quiet => Tone::Quiet,
        Style::Scope => Tone::Scope,
        Style::Code => Tone::Code,
    }
}

/// One lozenge: the entity it names and how it looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Lozenge<I> {
    pub other: I,
    pub end: End,
    pub tone: Tone,
    /// Every relation behind it is only proposed.
    pub proposed: bool,
}

impl<I> Lozenge<I> {
    /// Whether it names a ruling that has fallen: the warning, first in its group.
    pub fn fallen(&self) -> bool {
        matches!(self.end, End::Decision { status, .. } if fallen(status))
    }
}

/// How a group shows before anyone expands it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rest {
    /// Every lozenge.
    All,
    /// The first `shown`, then one "+`hidden` more" lozenge; `warns` when a fallen ruling is
    /// among those held back.
    First { shown: usize, hidden: usize, warns: bool },
    /// One lozenge with these words, standing for them all.
    Counted(String),
}

/// The lozenges of one facet.
#[derive(Clone, Debug, PartialEq)]
pub struct Group<I> {
    pub facet: Facet,
    pub lozenges: Vec<Lozenge<I>>,
    pub rest: Rest,
    /// The label warns: what cites a ruling that has fallen needs re-justifying.
    pub label_warns: bool,
}

/// What a count lozenge says: `total` entities, `done` of them tasks that are done.
pub fn count_words(label: &str, words: Words, total: usize, done: usize) -> String {
    match words {
        Words::After => format!("{label} {total}"),
        Words::Noun { one, many } => {
            let noun = if total == 1 { one } else { many };
            if done > 0 {
                format!("{total} {noun} ({done} done)")
            } else {
                format!("{total} {noun}")
            }
        }
    }
}

fn rest<I>(t: &Treatment, lozenges: &[Lozenge<I>]) -> Rest {
    let total = lozenges.len();
    match t.shown {
        Shown::Each { cap } if total <= cap => Rest::All,
        Shown::Each { cap } => {
            Rest::First { shown: cap, hidden: total - cap, warns: lozenges[cap..].iter().any(Lozenge::fallen) }
        }
        Shown::Counted { over, .. } if total <= over => Rest::All,
        Shown::Counted { words, .. } => {
            let done = lozenges.iter().filter(|l| matches!(l.end, End::Task { status: TaskStatus::Done, .. })).count();
            Rest::Counted(count_words(t.label, words, total, done))
        }
    }
}

/// The lozenges a row shows for what is `joined` to its entity (whose decision status is `own`,
/// none when it is not a decision): one per entity and facet, for each relation that still
/// stands (accepted, resolved as a checked step is, or proposed, marked while only proposed),
/// none for the entity of the row it hangs under (`parent`, which shows just above it), grouped by
/// facet in [`TABLE`]'s order, a fallen ruling first in its group, then rulings by number, then
/// oldest first. `leave_out` names facets the page shows elsewhere (a task's scope beside its
/// assignee).
pub fn plan<I: Copy + Ord>(
    own: Option<DecisionStatus>,
    joined: &[Joined<I>],
    parent: Option<I>,
    leave_out: &[Facet],
) -> Vec<Group<I>> {
    // By place in the table and entity: the lozenge, and when it sorts.
    let mut found: BTreeMap<(usize, I), (Lozenge<I>, i64)> = BTreeMap::new();
    for j in joined {
        let stands = matches!(j.status, RelationStatus::Accepted | RelationStatus::Proposed | RelationStatus::Resolved);
        if !stands || Some(j.other) == parent {
            continue;
        }
        let Some(facet) = facet(j.join, j.outward, own.is_some(), &j.end).filter(|f| !leave_out.contains(f)) else {
            continue;
        };
        let (place, t) = treatment(facet);
        let lozenge = Lozenge { other: j.other, end: j.end, tone: tone(t.style, &j.end, own), proposed: true };
        let entry = found.entry((place, j.other)).or_insert((lozenge, j.created));
        entry.0.proposed &= j.status == RelationStatus::Proposed;
        entry.1 = entry.1.min(j.created);
    }
    let mut groups = Vec::new();
    for (place, t) in TABLE.iter().enumerate() {
        let mut members: Vec<(Lozenge<I>, i64)> =
            found.iter().filter(|((p, _), _)| *p == place).map(|(_, member)| member.clone()).collect();
        if members.is_empty() {
            continue;
        }
        members.sort_by_key(|(l, created)| {
            let number = match l.end {
                End::Decision { number, .. } => number,
                _ => i64::MAX,
            };
            (!l.fallen(), number, *created, l.other)
        });
        let lozenges: Vec<Lozenge<I>> = members.into_iter().map(|(l, _)| l).collect();
        let rest = rest(t, &lozenges);
        let label_warns = t.facet == Facet::CitedBy && own.is_some_and(fallen);
        groups.push(Group { facet: t.facet, lozenges, rest, label_warns });
    }
    groups
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// The entity a lozenge names, as its preview and a click on it need it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Task(EntityId),
    Decision(EntityId),
    Message(EntityId),
    Scope(EntityId),
    Anchor(EntityId),
    /// A whole file a message's text names at a version (`fwd_core::file_mention`): the
    /// whole-file highlight that holds it, and the version the mention names, which the highlight,
    /// serving every version with the same bytes, does not.
    File(EntityId, FileVersion),
    Principal(EntityId),
    Unknown(EntityId),
}

/// The version of a file a mention names (`fwd_core::file_mention::Version`), held by value, so
/// that a lozenge naming the file at it is `Copy`, as every lozenge's target is: a commit by the
/// hexadecimal digits of its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileVersion {
    Worktree,
    Index,
    Commit { digits: [u8; 64], len: u8 },
}

impl FileVersion {
    pub fn of(version: &Version) -> FileVersion {
        match version {
            Version::Worktree => FileVersion::Worktree,
            Version::Index => FileVersion::Index,
            Version::Commit(commit) => {
                let mut digits = [0; 64];
                let len = commit.len().min(64);
                digits[..len].copy_from_slice(&commit.as_bytes()[..len]);
                FileVersion::Commit { digits, len: len as u8 }
            }
        }
    }

    pub fn version(self) -> Version {
        match self {
            FileVersion::Worktree => Version::Worktree,
            FileVersion::Index => Version::Index,
            FileVersion::Commit { digits, len } => {
                Version::Commit(String::from_utf8_lossy(&digits[..usize::from(len)]).into_owned())
            }
        }
    }
}

impl Target {
    fn of(id: EntityId, end: End) -> Target {
        match end {
            End::Task { .. } => Target::Task(id),
            End::Decision { .. } => Target::Decision(id),
            End::Message => Target::Message(id),
            End::Scope => Target::Scope(id),
            End::Anchor => Target::Anchor(id),
            End::Principal => Target::Principal(id),
            End::Unknown => Target::Unknown(id),
        }
    }

    /// The lozenge of the entity `id` by the models it carries: the first the page draws, else
    /// the short id of something it does not hold.
    pub fn by_models(id: EntityId, models: &Models<Model>) -> Target {
        let drawn = [
            (Model::Task, Target::Task(id)),
            (Model::Decision, Target::Decision(id)),
            (Model::Message, Target::Message(id)),
            (Model::Scope, Target::Scope(id)),
            (Model::Anchor, Target::Anchor(id)),
            (Model::Principal, Target::Principal(id)),
        ];
        drawn.into_iter().find(|(model, _)| models.contains(*model)).map_or(Target::Unknown(id), |(_, t)| t)
    }

    pub fn id(self) -> EntityId {
        match self {
            Target::Task(id)
            | Target::Decision(id)
            | Target::Message(id)
            | Target::Scope(id)
            | Target::Anchor(id)
            | Target::File(id, _)
            | Target::Principal(id)
            | Target::Unknown(id) => id,
        }
    }
}

/// Where a click on a lozenge naming `target` leads: a task to its panel, a decision to its panel,
/// a scope to the tasks worked in it, a message to its question's card on the Questions page, else
/// to the panel of the task or the decision it is on, else to the inbox; a highlight, and a file a
/// mention names, to the highlight's own page (in the VS Code webview [`follow`] asks the editor to
/// show it instead, a file at its version). A person leads nowhere yet: there is no page for one.
/// Only a scope's names a scope: the page the others lead to lists by the page's scope choice, as
/// the page it was clicked on did (`scope_filter::follow`).
pub fn destination(target: Target) -> Option<Route> {
    match target {
        Target::Task(id) => Some(Route::task(id)),
        Target::Decision(id) => Some(Route::decision(id)),
        Target::Scope(id) => Some(Route::scope_tasks(id)),
        Target::Message(id) => Some(message_route(id)),
        Target::Anchor(id) | Target::File(id, _) => Some(Route::Anchor(id)),
        Target::Principal(_) | Target::Unknown(_) => None,
    }
}

/// Asks the VS Code webview's extension to show what `target` names in the editor, when it names
/// code: a highlight at its lines, a file a mention names at its version (`session::reveal`).
/// False where nothing took the request (a browser) and for anything else.
fn revealed(target: Target) -> bool {
    match target {
        Target::Anchor(id) => session::reveal(id, None),
        Target::File(id, version) => session::reveal(id, Some(&version.version())),
        _ => false,
    }
}

/// Goes where a click on a lozenge naming `target` leads, for what is not a link itself: Enter
/// on a step, a click on a step's line. A highlight opens in the editor when the page runs in
/// the VS Code webview and the extension takes it (`session::reveal`), else on its own page.
pub fn follow(target: Target) {
    if revealed(target) {
        return;
    }
    if let Some(to) = destination(target) {
        route::go(to);
    }
}

fn is_question(id: EntityId) -> bool {
    store::message(id).is_some_and(|m| message_row::addressee(&m).is_some())
}

/// Where a message shows best: its own card on the Questions page when it asks someone, its
/// question's card when it answers one, else the panel of what it is on, else the inbox.
fn message_route(id: EntityId) -> Route {
    let Some(m) = store::message(id) else { return Route::inbox() };
    if message_row::addressee(&m).is_some() {
        return Route::question(id);
    }
    let root = m.thread().ok().flatten().or(m.reply_to().ok().flatten()).map(EntityId::from);
    if let Some(root) = root.filter(|root| is_question(*root)) {
        return Route::question(root);
    }
    if let Some(task) = m.task().ok().flatten() {
        return Route::task(task.into());
    }
    if let Some(decision) = m.decision().ok().flatten() {
        return Route::decision(decision.into());
    }
    Route::inbox()
}

/// What the page knows of `id`, and when it was made, looked up by id in the page's standing
/// queries; `readiness` asks whether a task is blocked, which only a dependency's lozenge shows.
fn end(id: EntityId, readiness: bool) -> (End, Option<i64>) {
    if let Some(d) = store::decision(id) {
        let end =
            End::Decision { number: d.number().unwrap_or(0), status: d.status().unwrap_or(DecisionStatus::Accepted) };
        return (end, d.created().ok());
    }
    if let Some(t) = store::task(id) {
        let blocked = readiness
            && match store::readiness(&t) {
                Readiness::NotReady(reasons) => reasons.iter().any(|r| !matches!(r, NotReadyReason::Status(_))),
                _ => false,
            };
        let end = End::Task { status: t.status().unwrap_or(TaskStatus::Proposed), blocked };
        return (end, t.created().ok());
    }
    if let Some(m) = store::message(id) {
        return (End::Message, m.created().ok());
    }
    if let Some(s) = store::scope(id) {
        return (End::Scope, s.created().ok());
    }
    if let Some(a) = store::anchor(id) {
        return (End::Anchor, a.created().ok());
    }
    if store::principal(id).is_some() {
        return (End::Principal, None);
    }
    (End::Unknown, None)
}

/// The plan for `id`'s lozenges, kept current from the page's standing queries. It changes only
/// when the lozenges would, so a row does not redraw them for every message that arrives, and it
/// reads only what is joined to `id`, so working it out again costs a row its own relations.
fn planned(id: EntityId, parent: Option<EntityId>, leave_out: &'static [Facet]) -> Memo<Vec<Group<EntityId>>> {
    Memo::new(move |_| {
        let relations = store::relations_of(id);
        if !store::standing_loaded() {
            return Vec::new();
        }
        let own = store::decision(id).and_then(|d| d.status().ok());
        let mut joined = Vec::new();
        for r in &relations {
            let (Ok(from), Ok(to), Ok(kind), Ok(status)) = (r.from_id(), r.to_id(), r.kind(), r.status()) else {
                continue;
            };
            let (other, outward) = match (from == id, to == id) {
                (true, false) => (to, true),
                (false, true) => (from, false),
                _ => continue,
            };
            let (end, created) = end(other, kind == RelationKind::DependsOn);
            let created = created.unwrap_or_else(|| r.created().unwrap_or(0));
            joined.push(Joined { join: Join::Relation(kind), outward, status, other, end, created });
        }
        // What a message was written on, when it opens a thread: a reply carries its thread's
        // task or decision too, which the thread's first message already names.
        let opens_thread = |m: &MessageView| m.reply_to().ok().flatten().is_none();
        if let Some(m) = store::message(id).filter(opens_thread) {
            let subjects = [
                m.task().ok().flatten().map(EntityId::from),
                m.decision().ok().flatten().map(EntityId::from),
                m.scope().ok().flatten().map(EntityId::from),
                m.anchor().ok().flatten().map(EntityId::from),
            ];
            for other in subjects.into_iter().flatten() {
                let (end, created) = end(other, false);
                let (status, created) = (RelationStatus::Accepted, created.unwrap_or(0));
                joined.push(Joined { join: Join::Subject, outward: true, status, other, end, created });
            }
        }
        plan(own, &joined, parent, leave_out)
    })
}

fn next_serial() -> u32 {
    static SERIAL: AtomicU32 = AtomicU32::new(0);
    SERIAL.fetch_add(1, Ordering::Relaxed)
}

/// The lozenges of every entity joined to `id`, grouped, for a row's line: nothing while none is.
/// `parent` is the entity of the row it hangs under, which shows just above it and gets none.
pub fn strip(id: EntityId, parent: Option<EntityId>) -> AnyView {
    let groups = planned(id, parent, &[]);
    view! { <span class="loz-strip">{drawn(groups)}</span> }.into_any()
}

/// The same lozenges under a heading, one group a line, for a task's, a decision's or a
/// highlight's own page; nothing at all while no entity is joined to it. `leave_out` names the
/// facets the page shows elsewhere.
pub fn section(id: EntityId, heading: &'static str, leave_out: &'static [Facet]) -> AnyView {
    let groups = planned(id, None, leave_out);
    view! {
        <section class="prose loz-section" class:empty=move || groups.with(Vec::is_empty)>
            <h4>{heading}</h4>
            <div class="loz-strip">{drawn(groups)}</div>
        </section>
    }
    .into_any()
}

/// The groups of a plan as they are drawn now. Which groups a person expanded is kept beside the
/// plan, so a group stays expanded while the plan changes around it.
fn drawn(groups: Memo<Vec<Group<EntityId>>>) -> impl Fn() -> Vec<AnyView> + Send + Sync + 'static {
    let expanded = RwSignal::new(HashSet::<Facet>::new());
    let strip = next_serial();
    move || groups.get().into_iter().map(|g| group_view(g, expanded, strip)).collect()
}

fn group_view(group: Group<EntityId>, expanded: RwSignal<HashSet<Facet>>, strip: u32) -> AnyView {
    let (_, t) = treatment(group.facet);
    let facet = group.facet;
    let is_open = move || expanded.with(|e| e.contains(&facet));
    // A count lozenge says what the group is; any other group is headed by its label.
    let counted = matches!(group.rest, Rest::Counted(_));
    let label = (!t.label.is_empty() && !counted).then(|| {
        let text = if group.label_warns { format!("{}, re-justify", t.label) } else { t.label.to_string() };
        view! { <span class="loz-label" class:warn=group.label_warns>{text}</span> }
    });
    let body = move || {
        let open = is_open();
        let each = |n: usize| {
            group.lozenges.iter().take(n).map(|l| lozenge_view(l.clone(), t, strip)).collect::<Vec<AnyView>>()
        };
        match &group.rest {
            Rest::Counted(words) => {
                let hint = if open { format!("{words}: show fewer") } else { format!("{words}: show each") };
                let toggle = view! {
                    <button class="loz count" class:quiet=t.style == Style::Quiet title=hint
                        aria-expanded=open.to_string()
                        on:click=move |_| expanded.update(|e| if !e.remove(&facet) { e.insert(facet); })>
                        {words.clone()}
                    </button>
                }
                .into_any();
                let mut shown = vec![toggle];
                if open {
                    shown.extend(each(ALL));
                }
                shown
            }
            Rest::First { shown, hidden, warns } if !open => {
                let mut lozenges = each(*shown);
                let more = format!("+{hidden} more");
                let hint = format!("show the other {hidden}");
                lozenges.push(
                    view! {
                        <button class="loz more" class:warn=*warns title=hint
                            on:click=move |_| expanded.update(|e| { e.insert(facet); })>
                            {more}
                        </button>
                    }
                    .into_any(),
                );
                lozenges
            }
            _ => each(ALL),
        }
    };
    view! { <span class="loz-group">{label}{body}</span> }.into_any()
}

fn tone_class(tone: Tone) -> &'static str {
    match tone {
        Tone::Link => "link",
        Tone::Warn => "warn",
        Tone::Done => "done",
        Tone::Muted => "muted",
        Tone::Quiet => "quiet",
        Tone::Scope => "scope",
        Tone::Code => "code",
        Tone::Person => "person",
    }
}

fn lozenge_view(l: Lozenge<EntityId>, t: &'static Treatment, strip: u32) -> AnyView {
    let target = Target::of(l.other, l.end);
    let element = format!("loz-{strip}-{:?}-{}", t.facet, l.other.to_base64());
    let class = format!(
        "loz {}{}{}",
        tone_class(l.tone),
        if l.proposed { " proposed" } else { "" },
        if t.glyph.is_empty() { "" } else { " tree" }
    );
    let (label, proposed) = (t.label, l.proposed);
    let aria = move || {
        let name = plain_name(target);
        let name = if label.is_empty() { name } else { format!("{label} {name}") };
        if proposed {
            format!("{name} (proposed)")
        } else {
            name
        }
    };
    element_view(target, element, move || class.clone(), aria, t.glyph, proposed)
}

/// One lozenge standing alone, in no group and with no label: the lozenge mode of every kind,
/// as a reference in a message's text or the assignee beside a task's scope draw it. It looks
/// as what it names looks in a group (a closed task struck through, a fallen ruling in the
/// warning colour, a scope in green, code in monospace, a person's handle in the ink colour).
pub fn single(target: Target) -> AnyView {
    let element = format!("loz-alone-{}-{}", next_serial(), target.id().to_base64());
    let tone = Memo::new(move |_| {
        let (end, _) = end(target.id(), false);
        match Target::of(target.id(), end) {
            Target::Decision(_) => tone(Style::Ruling, &end, None),
            Target::Task(_) => tone(Style::Tree, &end, None),
            Target::Scope(_) => Tone::Scope,
            Target::Anchor(_) | Target::File(..) => Tone::Code,
            Target::Principal(_) => Tone::Person,
            Target::Message(_) => Tone::Link,
            Target::Unknown(_) => Tone::Quiet,
        }
    });
    element_view(
        target,
        element,
        move || format!("loz alone {}", tone_class(tone.get())),
        move || plain_name(target),
        "",
        false,
    )
}

/// The lozenge itself: a link to where it leads (none for what leads nowhere yet), whose preview
/// opens on hover or on the keyboard's focus, and on a touch screen on the first tap.
fn element_view(
    target: Target,
    element: String,
    class: impl Fn() -> String + Send + Sync + 'static,
    aria: impl Fn() -> String + Send + Sync + 'static,
    glyph: &'static str,
    proposed: bool,
) -> AnyView {
    let href = move || destination(target).map(|to| to.hash());
    // Each handler keeps the lozenge's element id, by which the preview finds it.
    let (entered, left, clicked_on, focused, blurred) =
        (element.clone(), element.clone(), element.clone(), element.clone(), element.clone());
    view! {
        <a class=class id=element href=href tabindex="0" aria-label=aria
            on:pointerenter=move |ev| {
                if ev.pointer_type() == "mouse" {
                    show(target, &entered, false);
                }
            }
            on:pointerleave=move |ev| {
                if ev.pointer_type() == "mouse" {
                    hide_unpinned(&left);
                }
            }
            on:pointerdown=move |ev| TOUCHED.store(ev.pointer_type() != "mouse", Ordering::Relaxed)
            on:click=move |ev| clicked(ev, target, &clicked_on)
            on:focus=move |ev| {
                // The keyboard's focus shows the preview; a click's or a tap's does not.
                let keyboard = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|e| e.matches(":focus-visible").unwrap_or(false));
                if keyboard {
                    show(target, &focused, false);
                }
            }
            on:blur=move |_| hide_unpinned(&blurred)>
            {(!glyph.is_empty()).then(|| view! { <span class="loz-glyph" aria-hidden="true">{glyph}</span> })}
            {name_view(target)}
            {proposed.then(|| view! { <span class="loz-mark">"proposed"</span> })}
        </a>
    }
    .into_any()
}

/// The entity's name in a few words, as its lozenge shows it: a task by its title, a ruling as
/// `D<n>` and its title, a message by its author and how it begins, a scope by its name, code as
/// `path:lines`, a file a mention names by its path and the version named, a person by the
/// handle, with the role or "agent" beside an agent's.
fn name_view(target: Target) -> AnyView {
    match target {
        Target::File(id, version) => {
            let path = Memo::new(move |_| store::anchor(id).and_then(|a| a.path().ok()));
            let named = anchor_name::file_version(&version.version());
            view! {
                <span class="loz-name">{move || path.get().unwrap_or_else(|| fmt::short(id))}</span>
                <span class="loz-version">{named}</span>
            }
            .into_any()
        }
        Target::Principal(id) => {
            let principal = Memo::new(move |_| {
                store::principal(id).map(|p| {
                    let agent = p.kind().ok() == Some(PrincipalKind::Agent);
                    (p.handle().unwrap_or_default(), agent.then(|| person::role(&p)))
                })
            });
            view! {
                <span class="loz-who">{move || principal.get().map(|(handle, _)| handle).unwrap_or_else(|| fmt::short(id))}</span>
                {move || principal.get().and_then(|(_, role)| role).map(|role| view! { <span class="loz-mark">{role}</span> })}
            }
            .into_any()
        }
        Target::Decision(id) => {
            let decision = Memo::new(move |_| {
                store::decision(id).map(|d| (d.number().unwrap_or(0), d.title().unwrap_or_default()))
            });
            view! {
                <span class="loz-num">{move || decision.get().map(|(n, _)| format!("D{n}")).unwrap_or_else(|| fmt::short(id))}</span>
                <span class="loz-name">{move || decision.get().map(|(_, title)| fmt::truncate(&title, 36)).unwrap_or_default()}</span>
            }
            .into_any()
        }
        Target::Message(id) => {
            let message = Memo::new(move |_| {
                store::message(id).map(|m| {
                    let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
                    (author, first_words(&references::plain(&ops::text(m.content()).unwrap_or_default())))
                })
            });
            view! {
                <span class="loz-who">{move || message.get().map(|(a, _)| a).unwrap_or_else(|| fmt::short(id))}</span>
                <span class="loz-name">{move || message.get().map(|(_, words)| format!("“{words}”")).unwrap_or_default()}</span>
            }
            .into_any()
        }
        _ => view! { <span class="loz-name">{move || few_words(target)}</span> }.into_any(),
    }
}

/// A task's title, a scope's name or code's place, shortened for a lozenge; a short id while the
/// page does not hold it.
fn few_words(target: Target) -> String {
    let (id, words) = match target {
        Target::Task(id) => (id, store::task(id).and_then(|t| t.title().ok()).map(|t| fmt::truncate(&t, 44))),
        Target::Scope(id) => (id, store::scope(id).and_then(|s| s.title().ok())),
        Target::Anchor(id) => (id, anchor_row::name(id)),
        Target::File(id, version) => (id, anchor_row::file_name(id, &version.version())),
        Target::Principal(id) => (id, person::name(id)),
        Target::Decision(id) | Target::Message(id) | Target::Unknown(id) => (id, None),
    };
    words.unwrap_or_else(|| fmt::short(id))
}

/// How a message begins, in a few words.
fn first_words(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let head = words.iter().take(5).copied().collect::<Vec<_>>().join(" ");
    let clipped = fmt::truncate(&head, 32);
    if words.len() > 5 && !clipped.ends_with('…') {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// The entity's name as plain words, for a screen reader.
fn plain_name(target: Target) -> String {
    match target {
        Target::Task(id) => crate::task_row::name(id),
        Target::Decision(id) => decision_row::name(id),
        Target::Message(id) => message_row::name(id),
        Target::Scope(id) => crate::scope_row::name(id),
        Target::Anchor(id) => anchor_row::name(id),
        Target::File(id, version) => anchor_row::file_name(id, &version.version()),
        Target::Principal(id) => person::name(id),
        Target::Unknown(_) => None,
    }
    .unwrap_or_else(|| "something this page does not hold".into())
}

// ---------------------------------------------------------------------------------------------
// The preview

/// The preview's element id.
const PREVIEW: &str = "loz-preview";

/// The lozenge the preview shows, by its element id.
#[derive(Clone, Debug, PartialEq)]
struct Open {
    target: Target,
    element: String,
    /// Opened by a tap: it stays until a tap elsewhere, and carries an "open" link.
    pinned: bool,
}

fn preview() -> ArcRwSignal<Option<Open>> {
    static OPEN: OnceLock<ArcRwSignal<Option<Open>>> = OnceLock::new();
    OPEN.get_or_init(|| ArcRwSignal::new(None)).clone()
}

/// Whether the preview is open: Escape then closes it and means nothing else (`panel.rs`).
pub fn previewing() -> bool {
    preview().with_untracked(Option::is_some)
}

/// Whether the last press on a lozenge came from a finger or a pen rather than a mouse.
static TOUCHED: AtomicBool = AtomicBool::new(false);

fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|w| w.document())
}

/// The preview's element, while the page has drawn it.
fn preview_element() -> Option<web_sys::HtmlElement> {
    document()?.get_element_by_id(PREVIEW)?.dyn_into().ok()
}

/// Gives the preview its place and its visibility in the VS Code webview as in a browser: sets
/// each of `properties` on the element's style object, or removes it where its value is None,
/// never through the `style` attribute. The webview's Content Security Policy (`pageHtml` in the
/// extension) takes styles only from the page's stylesheet and from elements carrying the
/// document's nonce, so it refuses a style attribute the page writes, which then never applies;
/// it does not govern the style object. Written as an attribute, the place was refused, and the
/// preview opened at the webview's top left instead of beside its lozenge (Daniel, 2026-09-28).
fn restyle(pop: &web_sys::HtmlElement, properties: &[(&str, Option<&str>)]) {
    let style = pop.style();
    for (name, value) in properties {
        let _ = match value {
            Some(value) => style.set_property(name, value),
            None => style.remove_property(name).map(|_| ()),
        };
    }
}

fn show(target: Target, element: &str, pinned: bool) {
    // Out of sight until it is placed beside its lozenge, and at the view's left edge, where it
    // takes its full width to be measured.
    if let Some(pop) = preview_element() {
        restyle(&pop, &[("visibility", Some("hidden")), ("left", Some("0")), ("top", Some("0"))]);
    }
    preview().set(Some(Open { target, element: element.to_string(), pinned }));
}

fn hide() {
    let open = preview();
    if open.with_untracked(Option::is_some) {
        open.set(None);
    }
}

/// Closes the preview when it is this lozenge's and no tap pinned it.
fn hide_unpinned(element: &str) {
    let open = preview();
    if open.with_untracked(|o| o.as_ref().is_some_and(|o| !o.pinned && o.element == element)) {
        open.set(None);
    }
}

/// Whether the device has no pointer that hovers (a phone, a tablet).
fn cannot_hover() -> bool {
    web_sys::window().and_then(|w| w.match_media("(hover: none)").ok().flatten()).is_some_and(|m| m.matches())
}

/// A click on a lozenge. A mouse or the keyboard (Enter) follows its link. A tap on a touch
/// screen, which has no hover, opens the preview first, pinned; a second tap on the same lozenge
/// follows the link.
fn clicked(ev: web_sys::MouseEvent, target: Target, element: &str) {
    let keyboard = ev.detail() == 0;
    let touch = !keyboard && (TOUCHED.load(Ordering::Relaxed) || cannot_hover());
    let pinned_here = preview().with_untracked(|o| o.as_ref().is_some_and(|o| o.pinned && o.element == element));
    if !touch || pinned_here {
        hide();
        revealed_instead(&ev, target);
        return;
    }
    ev.prevent_default();
    show(target, element, true);
}

/// A highlight's link, and a file mention's, opens it in the editor instead when the page runs in
/// the VS Code webview and the extension takes it (`session::reveal`): the link is not followed. A
/// lozenge's link asks this, and so does a highlight's line beneath a message's text
/// (`attachments.rs`).
pub fn revealed_instead(ev: &web_sys::MouseEvent, target: Target) {
    if revealed(target) {
        ev.prevent_default();
    }
}

/// Where a lozenge sits in the view: its left edge, its top and its bottom, in CSS pixels from the
/// view's top left corner, as the browser measures it.
#[derive(Clone, Copy, Debug)]
struct Spot {
    left: f64,
    top: f64,
    bottom: f64,
}

/// A width and a height, in CSS pixels.
#[derive(Clone, Copy, Debug)]
struct Size {
    width: f64,
    height: f64,
}

/// The room between a lozenge and its preview, in CSS pixels.
const GAP: f64 = 6.0;
/// The least room the preview leaves between itself and the view's edges, in CSS pixels.
const MARGIN: f64 = 8.0;

/// Where the preview's top left corner goes, so that it shows beside its lozenge and inside the
/// view: beneath the lozenge when the view has room for it there, above the lozenge when the view
/// has room there instead, and else at the foot of the view (at its top, in a view shorter than
/// the preview); its left edge at the lozenge's, moved left as far as the view's right edge needs
/// and never past the view's left edge. `card` is the preview's size and `view` the view's.
fn beside(lozenge: Spot, card: Size, view: Size) -> (f64, f64) {
    let top = if lozenge.bottom + GAP + card.height + MARGIN <= view.height {
        lozenge.bottom + GAP
    } else if lozenge.top - GAP - card.height >= MARGIN {
        lozenge.top - GAP - card.height
    } else {
        (view.height - card.height - MARGIN).max(MARGIN)
    };
    let left = lozenge.left.min(view.width - card.width - MARGIN).max(MARGIN);
    (left, top)
}

/// Puts the preview beside its lozenge where [`beside`] says, and shows it; closes it once the
/// lozenge has gone or scrolled out of sight.
fn place() {
    let Some(open) = preview().get_untracked() else { return };
    let Some(document) = document() else { return };
    let (Some(lozenge), Some(pop)) = (document.get_element_by_id(&open.element), preview_element()) else {
        hide();
        return;
    };
    let Some(window) = web_sys::window() else { return };
    let size = |v: Result<JsValue, JsValue>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    let view = Size { width: size(window.inner_width()), height: size(window.inner_height()) };
    let r = lozenge.get_bounding_client_rect();
    if r.bottom() < 0.0 || r.top() > view.height {
        hide();
        return;
    }
    let card = Size { width: f64::from(pop.offset_width()), height: f64::from(pop.offset_height()) };
    let (left, top) = beside(Spot { left: r.left(), top: r.top(), bottom: r.bottom() }, card, view);
    let (left, top) = (format!("{left:.0}px"), format!("{top:.0}px"));
    restyle(&pop, &[("left", Some(left.as_str())), ("top", Some(top.as_str())), ("visibility", None)]);
}

/// Escape closes the preview; a press anywhere but on its lozenge or on the preview itself closes
/// it (a tap elsewhere, on a touch screen); scrolling carries it along with its lozenge. Installed
/// once, for the page's life.
fn listen() {
    static LISTENING: OnceLock<()> = OnceLock::new();
    LISTENING.get_or_init(|| {
        let Some(window) = web_sys::window() else { return };
        let keydown = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(|ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                hide();
            }
        });
        let _ = window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());
        keydown.forget();
        let press = Closure::<dyn FnMut(web_sys::Event)>::new(|ev: web_sys::Event| {
            let Some(open) = preview().get_untracked() else { return };
            let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()) else { return };
            let within =
                |id: &str| document().and_then(|d| d.get_element_by_id(id)).is_some_and(|e| e.contains(Some(&target)));
            if !within(PREVIEW) && !within(&open.element) {
                hide();
            }
        });
        let _ = window.add_event_listener_with_callback_and_bool("pointerdown", press.as_ref().unchecked_ref(), true);
        press.forget();
        let scroll = Closure::<dyn FnMut()>::new(|| {
            if preview().with_untracked(Option::is_some) {
                place();
            }
        });
        let _ = window.add_event_listener_with_callback_and_bool("scroll", scroll.as_ref().unchecked_ref(), true);
        scroll.forget();
    });
}

/// The one preview on the page: what the lozenge the pointer rests on names, beside it, as long
/// as the pointer stays, the keyboard's focus is on it, or (on a touch screen) until a tap
/// elsewhere. It reads the page's standing queries and opens none of its own.
#[component]
pub fn LozengePreview() -> impl IntoView {
    listen();
    let open = preview();
    Effect::new({
        let open = open.clone();
        move |_| {
            if open.with(Option::is_some) {
                request_animation_frame(place);
            }
        }
    });
    // It closes whenever the page goes somewhere else.
    let route = route::route();
    Effect::new(move |ran: Option<()>| {
        route.track();
        if ran.is_some() {
            hide();
        }
    });
    let shown = open.clone();
    view! {
        <div class="loz-preview" id=PREVIEW role="tooltip" hidden=move || shown.with(Option::is_none)>
            {move || open.get().map(|o| content(o.target, o.pinned))}
        </div>
    }
}

fn content(target: Target, pinned: bool) -> AnyView {
    let body = match target {
        Target::Task(id) => task_preview(id),
        Target::Decision(id) => decision_preview(id),
        Target::Message(id) => message_preview(id),
        Target::Scope(id) => scope_preview(id),
        Target::Anchor(id) => anchor_preview(id),
        Target::File(id, version) => file_preview(id, version),
        Target::Principal(id) => person_preview(id),
        Target::Unknown(id) => {
            view! { <p class="muted">{move || format!("{}: {}", fmt::short(id), empty::absent(None))}</p> }.into_any()
        }
    };
    // A touch screen has no hover to preview with, so the first tap shows this; the link goes on.
    let open = pinned.then(|| destination(target)).flatten().map(|to| {
        view! {
            <a class="btn tiny loz-open" href=to.hash()
                on:click=move |ev| {
                    hide();
                    revealed_instead(&ev, target);
                }>
                "open"
            </a>
        }
    });
    view! { {body} {open} }.into_any()
}

fn task_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(t) = store::task(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Task))}</p> }.into_any();
        };
        let status = t.status().ok();
        let priority = t.priority().ok().flatten().map(|p| format!("P{p}"));
        let scopes: Vec<String> = store::scopes_of_task(id).into_iter().map(store::scope_label).collect();
        let assignee = ops::assignee_of(&t).ok().flatten().map(|p| format!("assignee {}", store::handle_of(p)));
        let why = ops::text(t.why()).unwrap_or_default();
        view! {
            <div class="loz-preview-head">
                <span class="mono">{fmt::short(id)}</span>" "<strong>{t.title().unwrap_or_default()}</strong>
                {crate::labels::pills(crate::labels::of(&t))}
            </div>
            <div class="pills">
                {status.map(|s| view! { <span class=format!("pill status-{s}")>{fmt::status_label(s)}</span> })}
                {priority.map(|p| view! { <span class="pill">{p}</span> })}
                {scopes.into_iter().map(|s| view! { <span class="pill scope-active">{s}</span> }).collect_view()}
                {assignee.map(|a| view! { <span class="pill">{a}</span> })}
            </div>
            {(!why.trim().is_empty()).then(|| view! { <p class="clamp clamp-3"><span class="muted">"why: "</span>{why}</p> })}
        }
        .into_any()
    })
    .into_any()
}

fn decision_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(d) = store::decision(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Decision))}</p> }.into_any();
        };
        let ruling = ops::text(d.decision()).unwrap_or_default();
        view! {
            <div class="loz-preview-head">
                <span class="decision-number">{format!("D{}", d.number().unwrap_or(0))}</span>" "<strong>{d.title().unwrap_or_default()}</strong>
            </div>
            <div class="pills">
                {d.status().ok().map(|s| view! { <span class=format!("pill decision-{s}")>{s.to_string()}</span> })}
                {d.area().ok().flatten().map(|a| view! { <span class="pill area">{a}</span> })}
            </div>
            <div class="muted">{decision_row::ruled(&d)}</div>
            {(!ruling.trim().is_empty()).then(|| view! { <p class="clamp clamp-2">{ruling}</p> })}
        }
        .into_any()
    })
    .into_any()
}

fn message_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(m) = store::message(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Message))}</p> }.into_any();
        };
        let author = m.author().ok().map(|a| store::handle_of(EntityId::from(a))).unwrap_or_default();
        let to = message_row::addressee(&m);
        view! {
            <div class="loz-preview-head">
                <strong>{author}</strong>
                {to.map(|to| view! { <span class="muted">" to "</span><span>{to}</span> })}
                " "<span class="muted">{fmt::ago(m.created().unwrap_or(0))}</span>
            </div>
            <p class="clamp clamp-6">{references::plain(&ops::text(m.content()).unwrap_or_default())}</p>
        }
        .into_any()
    })
    .into_any()
}

fn scope_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(s) = store::scope(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Scope))}</p> }.into_any();
        };
        let intention = ops::text(s.intention()).unwrap_or_default();
        view! {
            <div class="loz-preview-head"><strong>{s.title().unwrap_or_default()}</strong></div>
            <div class="pills">
                {s.status().ok().map(|st| view! { <span class=format!("pill scope-{st}")>{st.to_string()}</span> })}
            </div>
            {(!intention.trim().is_empty()).then(|| view! { <p class="clamp clamp-3">{intention}</p> })}
        }
        .into_any()
    })
    .into_any()
}

fn anchor_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(a) = store::anchor(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Anchor))}</p> }.into_any();
        };
        let shown = anchor_row::Shown::of(&a);
        // Where the lines are now is per checkout, kept on the machine that resolved them, so the
        // browser shows the lines as anchored.
        let head: String = shown.text.lines().take(4).collect::<Vec<_>>().join("\n");
        view! {
            <div class="loz-preview-head mono">{shown.heading()}</div>
            {(!head.trim().is_empty()).then(|| view! { <pre class="anchor-text preview-text">{head}</pre> })}
        }
        .into_any()
    })
    .into_any()
}

/// A file a mention names: its path and the version named (`anchor_name::AnchorName::file_at`),
/// and which version that is in a sentence, the whole commit's id among them.
fn file_preview(id: EntityId, version: FileVersion) -> AnyView {
    (move || {
        let Some(a) = store::anchor(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Anchor))}</p> }.into_any();
        };
        let version = version.version();
        let name = AnchorName::file_at(&a.path().unwrap_or_default(), &version);
        view! {
            <div class="loz-preview-head">
                <span class="mono">{name.place}</span>" "<span class="pill basis">{name.version}</span>
            </div>
            <p class="muted">{file_version_sentence(&version)}</p>
        }
        .into_any()
    })
    .into_any()
}

/// Which version of a file a mention names, in a sentence for its preview.
pub fn file_version_sentence(version: &Version) -> String {
    match version {
        Version::Worktree => {
            "The whole file as it stood on disk when the message was written, unstaged changes included.".into()
        }
        Version::Index => "The whole file as the index held it when the message was written.".into(),
        Version::Commit(commit) => format!("The whole file at commit {commit}."),
    }
}

fn person_preview(id: EntityId) -> AnyView {
    (move || {
        let Some(p) = store::principal(id) else {
            return view! { <p class="muted">{empty::absent(Some(Model::Principal))}</p> }.into_any();
        };
        let inactive = !p.active().unwrap_or(false);
        view! {
            <div class="loz-preview-head">
                <strong>{p.display_name().unwrap_or_default()}</strong>" "<span class="muted">{p.handle().unwrap_or_default()}</span>
            </div>
            <div class="pills">
                <span class="pill">{person::role(&p)}</span>
                {inactive.then(|| view! { <span class="pill">"inactive"</span> })}
            </div>
            <p class="muted">"No page of its own yet: this lozenge leads nowhere."</p>
        }
        .into_any()
    })
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use DecisionStatus as D;
    use RelationKind as K;

    fn ruling(number: i64, status: DecisionStatus) -> End {
        End::Decision { number, status }
    }

    fn task(status: TaskStatus) -> End {
        End::Task { status, blocked: false }
    }

    fn joined(kind: RelationKind, outward: bool, other: u32, end: End) -> Joined<u32> {
        Joined {
            join: Join::Relation(kind),
            outward,
            status: RelationStatus::Accepted,
            other,
            end,
            created: i64::from(other),
        }
    }

    fn facets(groups: &[Group<u32>]) -> Vec<Facet> {
        groups.iter().map(|g| g.facet).collect()
    }

    fn others(group: &Group<u32>) -> Vec<u32> {
        group.lozenges.iter().map(|l| l.other).collect()
    }

    /// The plan of a row's lozenges with no facet left out, as most of these tests read it.
    fn plan(own: Option<DecisionStatus>, joined: &[Joined<u32>], parent: Option<u32>) -> Vec<Group<u32>> {
        super::plan(own, joined, parent, &[])
    }

    #[test]
    fn every_facet_has_one_row_and_the_families_run_in_the_briefed_order() {
        let facets: Vec<Facet> = TABLE.iter().map(|t| t.facet).collect();
        let mut unique = facets.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), facets.len(), "no facet has two rows");
        assert_eq!(facets.len(), 25, "every facet has a row");
        let mut families: Vec<Family> = TABLE.iter().map(|t| t.family).collect();
        families.dedup();
        assert_eq!(
            families,
            vec![
                Family::Subject,
                Family::Citations,
                Family::Dependencies,
                Family::Structure,
                Family::Lineage,
                Family::Mentions,
                Family::Association,
                Family::Assignment,
                Family::Code,
            ],
            "each family once, contiguous, in the order a row lists them"
        );
        let order: Vec<Facet> = TABLE.iter().filter(|t| t.family == Family::Citations).map(|t| t.facet).collect();
        assert_eq!(order, vec![Facet::Cites, Facet::CitedBy, Facet::DecidedIn, Facet::DecidedHere]);
    }

    #[test]
    fn each_kind_keeps_its_own_look_and_default() {
        let of = |facet| *treatment(facet).1;
        assert_eq!((of(Facet::Cites).style, of(Facet::Cites).shown), (Style::Ruling, Shown::Each { cap: 3 }));
        assert_eq!(of(Facet::CitedBy).shown, Shown::Each { cap: 3 });
        assert_eq!(of(Facet::DependsOn).style, Style::Readiness);
        assert!(matches!(of(Facet::DependsOn).shown, Shown::Counted { over: 2, .. }), "counted past two");
        assert!(matches!(of(Facet::Blocks).shown, Shown::Counted { over: 2, words: Words::After }));
        assert_eq!(of(Facet::SubtaskOf).shown, Shown::Each { cap: ALL }, "there is one, always shown");
        assert!(matches!(of(Facet::Subtasks).shown, Shown::Counted { over: 0, .. }), "a count first");
        assert_eq!(of(Facet::SubtaskOf).style, Style::Tree);
        assert!(!of(Facet::Subtasks).glyph.is_empty());
        for lineage in [Facet::DiscoveredFrom, Facet::LedTo, Facet::MotivatedBy, Facet::Supersedes, Facet::Duplicates] {
            assert_eq!((of(lineage).style, of(lineage).shown), (Style::Lineage, Shown::Each { cap: 2 }));
        }
        assert_eq!(of(Facet::SupersededBy).style, Style::Succession);
        assert_eq!(of(Facet::RelatesTo).shown, Shown::Counted { over: 0, words: Words::After }, "always one count");
        assert_eq!(of(Facet::RelatesTo).style, Style::Quiet);
        // Each scope a task is worked in, however many (docs/DECISIONS.md, D136), in its own look.
        let assigned = of(Facet::AssignedTo);
        assert_eq!((assigned.style, assigned.shown, assigned.label), (Style::Scope, Shown::Each { cap: ALL }, ""));
        assert_eq!((of(Facet::AnchoredAt).style, of(Facet::AnchoredAt).shown), (Style::Code, Shown::Each { cap: 2 }));
    }

    #[test]
    fn a_relation_reads_from_the_row_it_shows_on() {
        let t = task(TaskStatus::Open);
        let d = ruling(3, D::Accepted);
        assert_eq!(facet(Join::Relation(K::Cites), true, false, &d), Some(Facet::Cites));
        assert_eq!(facet(Join::Relation(K::Cites), false, false, &d), Some(Facet::DecidedHere), "a ruling made here");
        assert_eq!(facet(Join::Relation(K::Cites), false, true, &t), Some(Facet::CitedBy));
        assert_eq!(facet(Join::Relation(K::Cites), true, true, &End::Message), Some(Facet::DecidedIn));
        assert_eq!(facet(Join::Relation(K::Cites), true, true, &d), Some(Facet::Cites), "a ruling resting on a ruling");
        assert_eq!(facet(Join::Relation(K::DependsOn), false, false, &t), Some(Facet::Blocks));
        assert_eq!(facet(Join::Relation(K::SubtaskOf), false, false, &t), Some(Facet::Subtasks));
        assert_eq!(facet(Join::Relation(K::DiscoveredFrom), false, false, &t), Some(Facet::LedTo));
        assert_eq!(facet(Join::Relation(K::Supersedes), false, true, &d), Some(Facet::SupersededBy));
        assert_eq!(facet(Join::Relation(K::RelatesTo), true, false, &t), Some(Facet::RelatesTo));
        assert_eq!(facet(Join::Relation(K::RelatesTo), false, false, &t), Some(Facet::RelatesTo), "either way");
        assert_eq!(facet(Join::Relation(K::AssignedTo), true, false, &End::Scope), Some(Facet::AssignedTo));
        assert_eq!(facet(Join::Relation(K::Concerns), true, false, &t), Some(Facet::About));
        assert_eq!(facet(Join::Relation(K::Concerns), false, false, &End::Message), None, "they hang beneath it");
        assert_eq!(facet(Join::Subject, true, false, &t), Some(Facet::On));
        assert_eq!(facet(Join::Relation(K::WorkedIn), true, false, &End::Scope), Some(Facet::AssignedTo), "as before");
        assert_eq!(facet(Join::Relation(K::StepOf), true, false, &t), Some(Facet::StepOf));
        assert_eq!(facet(Join::Relation(K::StepOf), false, false, &End::Anchor), None, "the checklist shows them");
        assert_eq!(facet(Join::Relation(K::Mention), false, false, &End::Message), Some(Facet::MentionedIn));
        assert_eq!(facet(Join::Relation(K::Mention), true, false, &t), None, "the message's text shows them");
        assert_eq!(facet(Join::Relation(K::AnchoredAt), false, false, &t), Some(Facet::CodeOf));
    }

    #[test]
    fn what_mentions_a_thing_is_one_group_and_its_own_mentions_are_none() {
        // A task named in two messages (one of them twice, from two places in its text) that
        // itself is a step of another task, checked; a message that names two things.
        let mut mentioned = vec![
            joined(K::Mention, false, 7, End::Message),
            joined(K::Mention, false, 5, End::Message),
            joined(K::Mention, false, 7, End::Message),
            joined(K::StepOf, true, 90, task(TaskStatus::InReview)),
        ];
        mentioned[3].status = RelationStatus::Resolved;
        let groups = plan(None, &mentioned, None);
        assert_eq!(facets(&groups), vec![Facet::StepOf, Facet::MentionedIn]);
        assert_eq!(others(&groups[1]), vec![5, 7], "each message once, oldest first");
        assert_eq!(groups[1].rest, Rest::All);
        assert!(!groups[0].lozenges[0].proposed, "a checked step still stands");
        // Four messages: three show and one waits behind "+1 more".
        let four: Vec<Joined<u32>> = (1..=4).map(|n| joined(K::Mention, false, n, End::Message)).collect();
        assert_eq!(plan(None, &four, None)[0].rest, Rest::First { shown: 3, hidden: 1, warns: false });
        // The message's own strip leaves what it names to its text.
        let naming =
            vec![joined(K::Mention, true, 90, task(TaskStatus::Open)), joined(K::Mention, true, 91, End::Principal)];
        assert!(plan(None, &naming, None).is_empty());
        // A retracted mention (the text no longer names it) is gone.
        let mut retracted = joined(K::Mention, false, 8, End::Message);
        retracted.status = RelationStatus::Retracted;
        assert!(plan(None, &[retracted], None).is_empty());
    }

    /// A task worked in several scopes (docs/DECISIONS.md, D136) shows a lozenge for each, in the
    /// scope's look, none held back behind a count.
    #[test]
    fn a_task_in_several_scopes_shows_each_of_them() {
        let all = vec![joined(K::WorkedIn, true, 50, End::Scope), joined(K::WorkedIn, true, 51, End::Scope)];
        let groups = plan(None, &all, None);
        assert_eq!(facets(&groups), vec![Facet::AssignedTo]);
        assert_eq!((others(&groups[0]), groups[0].rest.clone()), (vec![50, 51], Rest::All));
    }

    #[test]
    fn a_facet_the_page_shows_elsewhere_is_left_out() {
        // The task panel shows the scope beside the assignee, so its Relations leave it out.
        let all =
            vec![joined(K::WorkedIn, true, 50, End::Scope), joined(K::RelatesTo, true, 40, task(TaskStatus::Open))];
        assert_eq!(facets(&super::plan(None, &all, None, &[Facet::AssignedTo])), vec![Facet::RelatesTo]);
        assert_eq!(facets(&plan(None, &all, None)), vec![Facet::RelatesTo, Facet::AssignedTo]);
    }

    #[test]
    fn groups_follow_the_table_and_a_fallen_ruling_leads_its_group() {
        // A task that cites five rulings (4 repealed, 5 superseded), depends on three tasks (one
        // done, one blocked), relates to two, is worked in a scope and is part of another task.
        let mut all = vec![
            joined(K::RelatesTo, true, 40, task(TaskStatus::Open)),
            joined(K::AssignedTo, true, 50, End::Scope),
            joined(K::RelatesTo, false, 41, task(TaskStatus::Open)),
            joined(K::SubtaskOf, true, 60, task(TaskStatus::InProgress)),
        ];
        for (id, number, status) in
            [(1, 1, D::Accepted), (2, 2, D::Accepted), (3, 3, D::Accepted), (4, 4, D::Repealed), (5, 5, D::Superseded)]
        {
            all.push(joined(K::Cites, true, id, ruling(number, status)));
        }
        all.push(joined(K::DependsOn, true, 30, task(TaskStatus::Done)));
        all.push(joined(K::DependsOn, true, 31, End::Task { status: TaskStatus::Open, blocked: true }));
        all.push(joined(K::DependsOn, true, 32, task(TaskStatus::InProgress)));
        let groups = plan(None, &all, None);
        assert_eq!(
            facets(&groups),
            vec![Facet::Cites, Facet::DependsOn, Facet::SubtaskOf, Facet::RelatesTo, Facet::AssignedTo]
        );
        let cites = &groups[0];
        assert_eq!(others(cites), vec![4, 5, 1, 2, 3], "the fallen rulings first, then by number");
        assert_eq!(cites.rest, Rest::First { shown: 3, hidden: 2, warns: false });
        assert_eq!(cites.lozenges[0].tone, Tone::Warn);
        assert_eq!(cites.lozenges[2].tone, Tone::Link);
        let depends = &groups[1];
        assert_eq!(depends.rest, Rest::Counted("3 dependencies (1 done)".into()));
        let tones: Vec<Tone> = depends.lozenges.iter().map(|l| l.tone).collect();
        assert_eq!(tones, vec![Tone::Done, Tone::Warn, Tone::Link], "done, blocked, in progress");
        assert_eq!(groups[2].rest, Rest::All);
        assert_eq!((others(&groups[3]), &groups[3].rest), (vec![40, 41], &Rest::Counted("relates to 2".into())));
        assert_eq!((groups[4].rest.clone(), groups[4].lozenges[0].tone), (Rest::All, Tone::Scope));
    }

    #[test]
    fn past_the_cap_one_more_lozenge_waits_and_the_fallen_count_within_the_cap() {
        // Seven rulings, five of them fallen: three show, all fallen, and the "+4 more" warns,
        // since two fallen ones wait behind it.
        let mut all = Vec::new();
        for n in 1..=7 {
            let status = if n <= 5 { D::Repealed } else { D::Accepted };
            all.push(joined(K::Cites, true, n as u32, ruling(n, status)));
        }
        let groups = plan(None, &all, None);
        assert_eq!(others(&groups[0])[..3], [1, 2, 3]);
        assert_eq!(groups[0].rest, Rest::First { shown: 3, hidden: 4, warns: true });
        // Exactly three: all show, nothing waits.
        let groups = plan(None, &all[..3], None);
        assert_eq!(groups[0].rest, Rest::All);
        // Two dependencies show each on its own; the third makes them one count.
        let deps: Vec<Joined<u32>> = (10..13).map(|n| joined(K::DependsOn, true, n, task(TaskStatus::Open))).collect();
        assert_eq!(plan(None, &deps[..2], None)[0].rest, Rest::All);
        assert_eq!(plan(None, &deps, None)[0].rest, Rest::Counted("3 dependencies".into()));
    }

    #[test]
    fn the_parent_and_withdrawn_relations_get_no_lozenge_and_a_proposed_one_is_marked() {
        let mut all = vec![
            joined(K::Cites, true, 1, ruling(1, D::Accepted)),
            joined(K::Cites, true, 2, ruling(2, D::Accepted)),
            joined(K::Cites, true, 3, ruling(3, D::Accepted)),
            joined(K::RelatesTo, true, 9, task(TaskStatus::Open)),
        ];
        all[1].status = RelationStatus::Retracted;
        all[2].status = RelationStatus::Proposed;
        all[3].status = RelationStatus::Rejected;
        // Ruling 1 is joined twice, once only proposed: one lozenge, not marked.
        let mut again = all[0].clone();
        again.status = RelationStatus::Proposed;
        all.push(again);
        let groups = plan(None, &all, None);
        assert_eq!(facets(&groups), vec![Facet::Cites]);
        let marks: Vec<(u32, bool)> = groups[0].lozenges.iter().map(|l| (l.other, l.proposed)).collect();
        assert_eq!(marks, vec![(1, false), (3, true)]);
        // A comment beneath the task it is on names neither the task nor anything else twice.
        let comment = vec![
            Joined {
                join: Join::Subject,
                outward: true,
                status: RelationStatus::Accepted,
                other: 7,
                end: task(TaskStatus::Open),
                created: 0,
            },
            joined(K::Cites, true, 1, ruling(1, D::Accepted)),
        ];
        assert_eq!(facets(&plan(None, &comment, Some(7))), vec![Facet::Cites], "the parent shows just above");
        assert_eq!(facets(&plan(None, &comment, None)), vec![Facet::On, Facet::Cites]);
    }

    #[test]
    fn a_fallen_rulings_own_row_warns() {
        // D5, superseded by D17 and cited by two tasks: its successor and its label warn.
        let all = vec![
            joined(K::Supersedes, false, 17, ruling(17, D::Accepted)),
            joined(K::Cites, false, 20, task(TaskStatus::Open)),
            joined(K::Cites, false, 21, task(TaskStatus::Done)),
        ];
        let groups = plan(Some(D::Superseded), &all, None);
        assert_eq!(facets(&groups), vec![Facet::CitedBy, Facet::SupersededBy]);
        assert!(groups[0].label_warns);
        assert_eq!(groups[1].lozenges[0].tone, Tone::Warn);
        // What cites a ruling in force needs nothing, and a task superseded by another is lineage.
        assert!(!plan(Some(D::Accepted), &all, None)[0].label_warns);
        let on_task = plan(None, &[joined(K::Supersedes, false, 8, task(TaskStatus::Open))], None);
        assert_eq!(on_task[0].lozenges[0].tone, Tone::Muted);
        assert!(!fallen(D::Rejected), "a rejected proposal was never a ruling to rest on");
    }

    #[test]
    fn a_count_says_how_many_and_how_many_are_done() {
        assert_eq!(count_words("depends on", DEPENDENCIES, 1, 0), "1 dependency");
        assert_eq!(count_words("depends on", DEPENDENCIES, 3, 1), "3 dependencies (1 done)");
        assert_eq!(count_words("blocks", Words::After, 4, 2), "blocks 4");
        assert_eq!(count_words("relates to", Words::After, 3, 0), "relates to 3");
        let subtasks: Vec<Joined<u32>> = [TaskStatus::Done, TaskStatus::Done, TaskStatus::Open, TaskStatus::InProgress]
            .into_iter()
            .enumerate()
            .map(|(n, status)| joined(K::SubtaskOf, false, n as u32, task(status)))
            .collect();
        assert_eq!(plan(None, &subtasks, None)[0].rest, Rest::Counted("4 subtasks (2 done)".into()));
        assert_eq!(plan(None, &subtasks[..1], None)[0].rest, Rest::Counted("1 subtask (1 done)".into()));
    }

    /// A scope's row counts the tasks in it in the words the page uses for a task's scope, never
    /// by the stored relation's name (task PHfp4WUc).
    #[test]
    fn a_scopes_row_counts_its_tasks_in_this_scope() {
        let tasks: Vec<Joined<u32>> = [TaskStatus::Done, TaskStatus::Open, TaskStatus::InProgress]
            .into_iter()
            .enumerate()
            .map(|(n, status)| joined(K::WorkedIn, false, n as u32, task(status)))
            .collect();
        let groups = plan(None, &tasks, None);
        assert_eq!(groups[0].facet, Facet::Assigned);
        assert_eq!(groups[0].rest, Rest::Counted("3 tasks in this scope (1 done)".into()));
        assert_eq!(plan(None, &tasks[1..2], None)[0].rest, Rest::Counted("1 task in this scope".into()));
        // Each task the count opens to reads so to a screen reader too, before its title.
        assert_eq!(treatment(Facet::Assigned).1.label, "in this scope");
    }

    /// Where the preview of a lozenge 19 px high opens, case by case: the lozenge's left edge and
    /// top, the preview's size, the view's, and the preview's top left corner. The view is 600 by
    /// 900 and the preview 360 by 126 unless a case says otherwise.
    #[test]
    fn a_preview_opens_beneath_its_lozenge_else_above_it_and_stays_inside_the_view() {
        let (view, card) = (Size { width: 600.0, height: 900.0 }, Size { width: 360.0, height: 126.0 });
        let short = |height| Size { width: 600.0, height };
        let tall = Size { width: 360.0, height: 150.0 };
        let cases = [
            ("room beneath: just beneath, at the lozenge's left edge", (40.0, 100.0), card, view, (40.0, 125.0)),
            ("just room beneath, the margin kept at the foot", (40.0, 741.0), card, view, (40.0, 766.0)),
            ("a pixel lower, no room beneath: above", (40.0, 742.0), card, view, (40.0, 610.0)),
            ("at the view's foot: above", (40.0, 857.0), card, view, (40.0, 725.0)),
            ("near the right edge: moved left, the margin kept", (500.0, 100.0), card, view, (232.0, 125.0)),
            ("near the left edge: the margin kept", (2.0, 100.0), card, view, (8.0, 125.0)),
            ("room neither beneath nor above: at the view's foot", (40.0, 90.0), tall, short(200.0), (40.0, 42.0)),
            ("a view shorter than the preview: at its top", (40.0, 30.0), tall, short(100.0), (40.0, 8.0)),
        ];
        for (case, (left, top), card, view, corner) in cases {
            assert_eq!(beside(Spot { left, top, bottom: top + 19.0 }, card, view), corner, "{case}");
        }
    }
}
