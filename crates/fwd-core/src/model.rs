//! The Ankurah models. One collection per model, on Ankurah 0.9.3. Shapes and reasons are in
//! docs/design/data-model.md; the message, notification and task shapes follow the simplikeys
//! models Daniel pointed at (durable, visible messages and tasks with one status field; thin
//! per-recipient notifications with Json `target` and `originator` refs; Json `context` maps).
//!
//! Enumerations (`string_enum_property!`) are stored as their snake_case names (`"in_progress"`), so predicates read
//! `status = 'in_progress'` and unknown values survive as strings. Identity is the Ankurah
//! `EntityId`; there are no short ids in v1 (docs/DECISIONS.md, "Identifiers"), with one
//! exception: a decision also carries its project's number, which people cite as `D<number>`.

use ankurah::{
    property::{Json, Ref},
    EntityId, Model,
};
use serde::{Deserialize, Serialize};

/// Implements `ankurah::Property` for a unit enum by storing its strum `Display` string and
/// parsing it back with `FromStr`. Used instead of `#[derive(Property)]`, which stores the
/// serde JSON encoding (`"\"open\""`) and makes predicates awkward.
macro_rules! string_enum_property {
    ($t:ty) => {
        impl ::ankurah::Property for $t {
            // Stored as its strum string, so the catalog records the property as a string.
            const VALUE_TYPE: &'static str = "string";

            fn into_value(&self) -> Result<Option<::ankurah::value::Value>, ::ankurah::property::PropertyError> {
                Ok(Some(::ankurah::value::Value::String(self.to_string())))
            }
            fn from_value(value: Option<::ankurah::value::Value>) -> Result<Self, ::ankurah::property::PropertyError> {
                match value {
                    Some(v) => v.parse_as_string().map_err(|_| ::ankurah::property::PropertyError::InvalidValue {
                        value: v.to_string(),
                        ty: stringify!($t).to_owned(),
                    }),
                    None => Err(::ankurah::property::PropertyError::Missing),
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// Principal

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Agent,
}
string_enum_property!(PrincipalKind);

/// The role an agent selects for its own principal, from this fixed set, as it names itself (`fwd
/// principal add --role`) or later (`fwd principal role`); a person holds none, and no role is ever
/// read from a handle (Daniel, 2026-09-29: "probably best to have the set of selectable roles be
/// fixed: lead, triage, implementer, reviewer - and require each agent to select one, allowing only
/// one lead", which docs/DECISIONS.md, D119, reads as one lead per session or worktree, not per
/// project; docs/design/agents-and-inbox.md, "Roles"). Of the principals that are not retired, a
/// scope has at most one triage agent and a project one that serves it whole (docs/DECISIONS.md,
/// D122; [`Principal::serves`]), and a project has as many leads as sessions coordinate its work,
/// one per Claude Code session where fwd can tell the sessions apart (`ops::add_principal`,
/// `ops::set_role`). A principal whose stored role is a word outside the set, as `supervisor` is
/// until `fwd admin migrate` carries it over (`ops::migrate_roles`), reads as having none.
#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum AgentRole {
    /// Coordinates the project's work with its person. No rule routes to it for its role: it hears
    /// what is addressed to it and what mentions it, and fwd tells it how to spawn a triage agent
    /// while no triage agent answers first for the scope it works in (`guidance`).
    Lead,
    /// Answers first for the scopes it serves, or, endorsed, for the whole project
    /// ([`Principal::serves`]): the questions asked of no one in particular about their work reach
    /// it, and so does everything that reached the supervisors until 2026-09-29, comments on tasks
    /// and on code, proposals, reviews and unblocked tasks nobody holds (`routing`, `Why::Triage`).
    /// Until 2026-09-30 a project had at most one, which answered first for all of it.
    Triage,
    /// Owns a scope and works tasks.
    Implementer,
    /// Reviews what it is asked to review. No rule routes to it for its role: it hears what is
    /// addressed to it and what mentions it.
    Reviewer,
}
string_enum_property!(AgentRole);

/// A person or an agent identity. Agents are principals in their own right so attribution and
/// inboxes work the same way for both. Each project's graph holds its own principals, so a person
/// or an agent in two projects is two principals with one handle.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    #[active_type(LWW)]
    pub kind: PrincipalKind,
    /// A name, not a key, which several principals may share: the GitHub login for people
    /// ("dnorman"), the name an agent gave itself ("impl-alice", "triage-bot").
    #[active_type(LWW)]
    pub handle: String,
    #[active_type(LWW)]
    pub display_name: String,
    /// "github:<numeric id>", captured at `fwd init`; a JWT subject later.
    #[active_type(LWW)]
    pub auth_subject: Option<String>,
    /// The role an agent selected; none for a person, and for an agent made before the roles were
    /// a fixed set that has not selected one, whose writes are refused until it does.
    #[active_type(LWW)]
    pub agent_role: Option<AgentRole>,
    /// False once retired (`fwd principal retire`): the principal stays, so that what it did is
    /// still attributed to it, and no picker offers it and no rule reaches it for its role.
    pub active: bool,
    /// Whether the principal belongs to the project whose graph holds it, which is what the page's
    /// pickers offer (docs/design/agents-and-inbox.md, "Who belongs to a project"): true for a
    /// person from `fwd init`, for an agent from `fwd principal add`, by which it names itself, and
    /// for any principal from its first command that writes work into the project; false for the
    /// person a command that only reads brought into a project they never joined, and for an agent
    /// `fwd admin migrate` found had only read, or only written messages, there. None on a
    /// principal recorded before the field existed (2026-09-29), which belongs while it is active
    /// until `fwd admin migrate` records whether it does (`ops::belongs`).
    #[active_type(LWW)]
    pub member: Option<bool>,
    /// The Claude Code agent whose commands act as this principal, `claude-code:<session>/<agent>`
    /// (`fwd_core::subject`), so that what an agent writes is attributed to that agent
    /// (docs/design/agents-and-inbox.md, "An agent's identity"): set as an agent names itself from
    /// a command the fwd plugin stamped (`fwd principal add`), or takes over a principal from before
    /// the plugin, once (`fwd principal bind`). None for a person, for an agent no agent is bound
    /// to, and on a principal recorded before the field existed (2026-09-29).
    #[active_type(LWW)]
    pub subject: Option<String>,
    /// The principal that took over this one's job when an agent was relaunched to carry it on
    /// (`fwd principal succeed`; docs/DECISIONS.md, D114): this one was retired as it was set, and
    /// routing delivers to the successor whatever would reach it (`routing::Successors`). None for
    /// a principal no one succeeded, and on one recorded before the field existed (2026-09-29).
    pub succeeded_by: Option<Ref<Principal>>,
    /// What a triage agent answers first for (docs/DECISIONS.md, D122: "it should be effectively
    /// per scope"; `ops::Serving`), as JSON text: `{"scopes": ["<scope id>", ...]}`, the scopes it
    /// serves, and `"project_wide": true` where it is endorsed to serve the whole project, which
    /// takes what no scope's own triage agent does. Text rather than a `Json` field because the
    /// Model derive of the pinned Ankurah (0.9.3) gives every field a getter for the page's wasm
    /// build, and an optional `Json` has none, while a field added after principals were written
    /// must be optional, so that those read it as none. Kept on the principal, as a task's assignee
    /// is on the task (docs/DECISIONS.md, D45), so that the one write that gives an agent triage
    /// says what it serves, succession moves it with the role in one transaction, and the page
    /// reads it from the principals it holds. None for every other principal, for a triage agent
    /// that serves nothing, which receives nothing for its role, and on a principal recorded before
    /// the field existed (2026-09-30), the triage agents of then among them, until their leads bind
    /// or endorse them (`fwd principal scope`).
    #[active_type(LWW)]
    pub serves: Option<String>,
    /// When the principal last wrote to the graph, in milliseconds since the epoch by the writer's
    /// clock, so that the pickers and `fwd principal list` show an agent quiet for twelve hours as
    /// dormant, below the others (docs/DECISIONS.md, D123; `dormancy`). `dormancy::wrote` stamps
    /// it after each write: the CLI after every command that writes work or changes a principal,
    /// and the page and the extension after each of their writes; an agent that names itself, anew
    /// or as a successor, has it from the moment its principal is made (`ops::add_principal`,
    /// `ops::add_successor`). Stored rather than read from what the principal wrote, because a
    /// task's changes after its creation and a principal's changes record no author and no time
    /// that a reader could find. None on a principal recorded before the field existed
    /// (2026-09-30), which reads as dormant until its next write, and on a person no write has
    /// stamped yet.
    #[active_type(LWW)]
    pub last_active: Option<i64>,
}

// ---------------------------------------------------------------------------------------------
// Repo

/// A registered repository, keyed by its upstream (`remote_urls`). Anchors reference it so a project can span several repositories.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    #[active_type(LWW)]
    pub name: String,
    /// `["git@github.com:org/x.git", ...]`
    pub remote_urls: Json,
    #[active_type(LWW)]
    pub default_branch: String,
}

// ---------------------------------------------------------------------------------------------
// Task

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum TaskStatus {
    /// "I think this should exist; tell me if not": drawn as a ghost, never ready.
    Proposed,
    /// Sanctioned, not started.
    Open,
    /// Started; starting sets the assignee to whoever started it.
    InProgress,
    /// Submitted with evidence; awaiting acceptance.
    InReview,
    Done,
    /// Was sanctioned, then abandoned, with reason.
    Cancelled,
    /// A proposal that was declined, with reason.
    Rejected,
}
string_enum_property!(TaskStatus);

/// The unit of work. Three prose fields answer three questions: what, why, and how we will know
/// it is done. One status field; one optional assignee, the person or agent doing it; where the
/// work happens is a `worked_in` relation to a scope; readiness is derived, never stored.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    #[active_type(LWW)]
    pub title: String,
    /// Spec / description.
    #[active_type(YrsString)]
    pub what: String,
    /// Motivation in prose; `motivated_by` relations carry the graph form.
    #[active_type(YrsString)]
    pub why: String,
    /// Acceptance criteria.
    #[active_type(YrsString)]
    pub done_when: String,
    #[active_type(LWW)]
    pub status: TaskStatus,
    /// Who is doing the work: a field, never an edge, and optional, since a task may wait for
    /// someone to take it (docs/DECISIONS.md, "Assignment is an optional field on the task, never
    /// an edge", 2026-09-28). Starting a task sets it to whoever started it. A task written before
    /// the field existed reads as unassigned.
    pub assignee: Option<Ref<Principal>>,
    /// If true, Done requires `accept` by someone other than the submitter.
    pub needs_review: bool,
    /// Manual hold with reason; an availability overlay, not a status.
    #[active_type(LWW)]
    pub hold: Option<String>,
    /// 0 (urgent) .. 4; None = unranked. The CLI takes it as a word, now, next or later
    /// ([`Priority::task_number`]).
    #[active_type(LWW)]
    pub priority: Option<i64>,
    /// What kind of work it is, in words (`["bug", "cli"]`): free text, `bug` and `feature` by
    /// convention, each once and in the form `labels.rs` stores (docs/DECISIONS.md, D97, "A
    /// task's kind, such as bug or feature, is a label, not a task type").
    pub labels: Json,
    /// Who moved the task, kept so that later moves and routing can act on it, and read in Rust,
    /// never queried (docs/design/data-model.md, "Json fields"): who last set its assignee
    /// (`assigned_by`, or `"migration"` for one `fwd admin migrate` carried over), whom routing
    /// leaves out of the notification the assignment causes; who submitted it (`submitted_by`),
    /// whom accepting refuses, and the evidence they gave (`evidence`), which the page shows; who
    /// approved it (`approved_by`), whom a review goes to when no triage agent can take it; and who
    /// accepted it (`accepted_by`). It also records the parent it was made under (`parent_id`),
    /// which its `subtask_of` relation says too. Its `worked_in` relations say where it is worked
    /// (docs/DECISIONS.md, D45, "Assignment is an optional field on the task, never an edge; the
    /// scope edge means where the work happens"). Until 2026-10-03 the context also recorded the
    /// scope the task was made or started in (`scope_id`), which routing read for a task worked in
    /// no scope; nothing writes or reads it now but `fwd admin migrate`, which gives a task that
    /// records one and has no `worked_in` that scope (`ops::migrate_recorded_scopes`).
    pub context: Json,
    pub created_by: Ref<Principal>,
    pub created: i64,
    pub updated: i64,
    #[active_type(LWW)]
    pub closed: Option<i64>,
    #[active_type(LWW)]
    pub closed_reason: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Relation

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum RelationKind {
    /// Task -> Task: `from` is not ready until `to` is Done. Acyclic over accepted relations.
    DependsOn,
    /// Task -> Task: `from` is part of `to`. Acyclic.
    SubtaskOf,
    /// Task -> Task or Message: why this task exists, in graph form.
    MotivatedBy,
    /// Task -> Task or Message: the work that surfaced this task.
    DiscoveredFrom,
    /// any -> any: free association.
    RelatesTo,
    /// Task -> Task: lineage. Decision -> Decision: the new ruling replaces the old one, which is
    /// Superseded (`ops::supersede_decision` makes both changes at once).
    Supersedes,
    /// Task -> Task: lineage.
    Duplicates,
    /// Task -> CodeAnchor: the code this task concerns.
    AnchoredAt,
    /// The name `worked_in` had until 2026-09-28, when assignment became a field on the task
    /// (docs/DECISIONS.md, "Assignment is an optional field on the task, never an edge"). Edges
    /// written before then keep it until `fwd admin migrate` renames them; nothing new is made
    /// with it.
    AssignedTo,
    /// Message -> Task, Relation or Scope: a comment linked to what it concerns.
    Concerns,
    /// Task, Message or Decision -> Decision: `from` rests on the ruling, so it needs
    /// re-justifying when the ruling is repealed or superseded. Decision -> Task or Message: the
    /// ruling was made in that task's or that question's context.
    Cites,
    /// Task -> Scope: where the work on the task happens. At most one accepted per task. Who
    /// does the work is the task's `assignee`, not this edge.
    WorkedIn,
    /// Any entity -> Task: `from` is a step of the task, such as a highlight to look at, a
    /// message or another task. Checking the step moves the edge to Resolved; its `position`
    /// orders the task's checklist. Acyclic between tasks, like `subtask_of`. A highlight's steps
    /// and the first thread on the highlight move together (`rules::move_step_and_thread`).
    StepOf,
    /// Message -> any entity: the message's text names `to` inline (`@handle`, `[[id]]`,
    /// `[[D12]]`), one edge per reference, written with the text; `via` records that it was
    /// inline and its place among the message's references.
    Mention,
}
string_enum_property!(RelationKind);

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum RelationStatus {
    Proposed,
    Accepted,
    Retracted,
    Rejected,
    /// A `step_of` edge whose step is checked, with `closed_by` and `closed` saying who checked
    /// it and when; unchecking returns it to Accepted. No other kind takes it.
    Resolved,
}
string_enum_property!(RelationStatus);

/// An edge is an entity with two named endpoints and full provenance. Binary only in v1.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    #[active_type(LWW)]
    pub kind: RelationKind,
    /// Named `from_id`/`to_id` because `from` is a reserved word in ankql predicates. Plain ids
    /// rather than `Ref<T>`: an endpoint may be a task, a message, a scope or an anchor, and a
    /// `Ref` needs one model. Stored identically (`Value::EntityId`).
    pub from_id: EntityId,
    pub to_id: EntityId,
    #[active_type(LWW)]
    pub status: RelationStatus,
    pub created_by: Ref<Principal>,
    pub created: i64,
    /// What caused this assertion: `{"message_id": "..."}` or `{"actor_id": "..."}`.
    pub via: Json,
    #[active_type(LWW)]
    pub note: Option<String>,
    /// A `step_of` edge's place in its task's checklist: a fractional key compared as text, so a
    /// step can go between two others without renumbering them (`position.rs`). Steps sort by it,
    /// then by `created`, then by id; a step a client made without one sorts after those with one.
    /// None on every other kind.
    #[active_type(LWW)]
    pub position: Option<String>,
    pub closed_by: Option<Ref<Principal>>,
    #[active_type(LWW)]
    pub closed: Option<i64>,
    #[active_type(LWW)]
    pub closed_reason: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// CodeAnchor

/// Where an anchor's lines were chosen, so that the editor opens the anchor there again
/// (extensions/vscode/src/reveal.ts): on the file on disk, or on a side of a diff that shows a
/// revision of it. An anchor made on a revision (`fwd comment --against`: the index, HEAD or a
/// commit) has that revision's content for its basis.
#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum DiffSide {
    /// The left side of a diff, the older version: the index in the working tree's changes, HEAD
    /// in the staged changes, a commit's parent in that commit's changes.
    Base,
    /// The right side of a diff that shows a revision rather than the file on disk: the index in
    /// the staged changes, a commit in its own changes.
    Modified,
    /// The file on disk, in an editor of its own or on the working tree's side of its changes: the
    /// side of every anchor made before anchors on a revision existed (2026-09-29).
    Worktree,
}
string_enum_property!(DiffSide);

/// A run of an anchor's lines that stood the same way against HEAD and the index when the anchor
/// was made, so a span crossing staged and unstaged hunks can later be answered piece by piece
/// (`partial`) rather than all or nothing. Ranges are 0-based with the end exclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Piece {
    /// The run's lines in the basis.
    pub basis_lines: [i64; 2],
    /// The same lines in the HEAD blob of that moment, when they were committed there.
    pub head_lines: Option<[i64; 2]>,
    /// The same lines in the index blob of that moment, when they were staged there.
    pub index_lines: Option<[i64; 2]>,
}

impl Piece {
    /// Where the run's text stood, on an anchor made on `side`: `head` (committed), `index`
    /// (staged only), or in neither of them, `worktree` when the anchor was made on the file on
    /// disk, and `revision` when it was made on a revision whose lines neither HEAD nor the index
    /// held (a commit whose lines were changed since, or one on another branch).
    pub fn stage(&self, side: DiffSide) -> &'static str {
        if self.head_lines.is_some() {
            "head"
        } else if self.index_lines.is_some() {
            "index"
        } else if side == DiffSide::Worktree {
            "worktree"
        } else {
            "revision"
        }
    }
}

/// An interior sub-entity of a file: a line range of one version of the file (its basis) with an
/// identity of its own, or with no line range, the whole file at that version (Daniel,
/// 2026-09-28, "A whole-file reference is an anchor with no line range"). The version is named by
/// git's object id of the content the person was looking at, whether or not it was ever
/// committed, so the same span of the same content is the same anchor on every machine. Nothing
/// here changes after creation, except that an anchor made before these fields existed gains its
/// key and basis once a checkout can supply them. Where the span is now in another version is
/// computed on demand and cached outside the graph (`sidecar`). See docs/design/anchoring.md.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct CodeAnchor {
    pub repo: Ref<Repo>,
    /// Repo-relative, at the anchored version.
    #[active_type(LWW)]
    pub path: String,
    /// `fwd-anchor:1:` and the SHA-256 of the repository, the path, the basis and the line range
    /// (empty for a whole-file anchor): what makes creating the same anchor twice return the first.
    /// Absent on an anchor made before keys existed until a checkout supplies its basis.
    #[active_type(LWW)]
    pub anchor_key: Option<String>,
    /// `fwd-span:1:` and the SHA-256 of the anchored text with whitespace runs collapsed, for
    /// recognising two anchors on the same text. Absent on a whole-file anchor.
    #[active_type(LWW)]
    pub span_key: Option<String>,
    /// git's object id of the file content the anchor was made on (`git hash-object --path`, or
    /// the index blob when the file matched it; on a revision, that revision's blob of the file),
    /// written to the repository and pinned under `refs/fwd/basis/`. Its lines are the lines
    /// `start_line` and `end_line` count.
    #[active_type(LWW)]
    pub basis_blob: Option<String>,
    /// The file's HEAD blob when the anchor was made; absent when the file was untracked or the
    /// branch unborn.
    #[active_type(LWW)]
    pub head_blob: Option<String>,
    /// The file's index blob when the anchor was made; absent when it was not in the index.
    #[active_type(LWW)]
    pub index_blob: Option<String>,
    /// Whichever of `head_blob` and `index_blob` differs least from the basis, which
    /// `basis_patch` starts from; absent when neither existed.
    #[active_type(LWW)]
    pub nearest_blob: Option<String>,
    /// A unified diff taking `nearest_blob` (or an empty file, when there is none) to the basis,
    /// so a machine without the basis rebuilds it and checks its id; empty when the basis is the
    /// nearest blob, absent when it would exceed 256 KB or the file is not text.
    #[active_type(LWW)]
    pub basis_patch: Option<String>,
    /// The first cut's basis, on anchors made before `basis_blob` existed: the HEAD blob a
    /// selection unchanged since HEAD was mapped onto. Read to resolve and backfill those
    /// anchors; never written now.
    #[active_type(LWW)]
    pub blob_sha: Option<String>,
    /// The commit checked out when the anchor was made, if any.
    #[active_type(LWW)]
    pub commit_sha: Option<String>,
    /// Where the lines were chosen: the file on disk, or a side of a diff showing a revision.
    #[active_type(LWW)]
    pub side: DiffSide,
    /// 0-based, inclusive; 0 for a whole-file anchor.
    pub start_line: i64,
    /// 0-based, exclusive; 0 for a whole-file anchor, whose range is empty.
    pub end_line: i64,
    /// True for an anchor on the whole file, which has no line range. Absent on anchors made
    /// before whole-file anchors existed, all of which name lines.
    pub whole_file: Option<bool>,
    /// The exact selected lines; empty for a whole-file anchor.
    #[active_type(LWW)]
    pub anchored_text: String,
    /// Up to 3 lines each.
    #[active_type(LWW)]
    pub context_before: String,
    #[active_type(LWW)]
    pub context_after: String,
    /// The anchored lines as `[Piece]`: maximal runs that were committed, staged only, or in
    /// neither (only in the working tree, or on an anchor made on a revision, only in that
    /// revision) when the anchor was made. Empty for a whole-file anchor; absent on anchors made
    /// before pieces existed.
    pub pieces: Json,
    /// Optional tree-sitter path, later.
    #[active_type(LWW)]
    pub symbol_path: Option<String>,
    pub created_by: Ref<Principal>,
    pub created: i64,
}

// ---------------------------------------------------------------------------------------------
// Message

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum Priority {
    Now,
    Next,
    Later,
}
string_enum_property!(Priority);

impl Priority {
    /// The number a task stores for this word, so that a task's priority is given in the words a
    /// message's is (task YtkOevUc, "Give fwd task create and fwd ask one priority vocabulary"):
    /// now 1, next 2 and later 3, which the page shows as P1, P2 and P3. 0 and 4, which the page's
    /// form offers too, have no word.
    pub fn task_number(self) -> i64 {
        match self {
            Priority::Now => 1,
            Priority::Next => 2,
            Priority::Later => 3,
        }
    }
}

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum MessageStatus {
    Active,
    Resolved,
    Deleted,
}
string_enum_property!(MessageStatus);

/// A comment or note: free text, durable, visible, with a status. Some messages ask questions or
/// make proposals; that is the writer's choice, not a type. What it is about lives in typed
/// reference fields (`task`, `anchor`, `scope`, `decision`, `reply_to`, `thread`) so reverse
/// queries work over the wire; `context` keeps the rest: on a message that asks something, either
/// `to`, the id of the one principal it asks, or `asks` set to `"project"` (`ops::ASKS_KEY`), a
/// question asked of no one in particular, which the triage agent of the scope `asks_scope` names
/// answers (`ops::ASKS_SCOPE_KEY`), else the one that serves the whole project (until 2026-09-29
/// `to` could hold `"supervisor"`, the role, which `fwd admin migrate` replaces with `asks`); the
/// scope its author wrote it in, `written_in` (`ops::WRITTEN_IN_KEY`); and `move` (`ops::MOVE_KEY`)
/// on the comment that records why a decision moved, or a task (a return, a reject, a cancel, a
/// reopen or a rejustify: `ops::transition`), which every such move writes and routing reads so that
/// no one hears of a repeal or a supersession twice (docs/design/agents-and-inbox.md, rules 11 and 12; docs/DECISIONS.md,
/// D34, "Decision records live in the graph, with statuses, an area, links to what they govern,
/// and a page listing them all").
/// Its author, or an agent that succeeded its author, may edit its text later
/// (`ops::edit_message`), which changes nothing else about it but saying when and by whom.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub author: Ref<Principal>,
    pub created: i64,
    /// Markdown, in a collaborative text type. Only the author, or an agent that succeeded the
    /// author, edits it (`rules::can_edit_message`), replacing it whole; the text it had before
    /// stays in the entity's history.
    #[active_type(YrsString)]
    pub content: String,
    /// Set by the author, editable by the triage agent (re-triage).
    #[active_type(LWW)]
    pub priority: Priority,
    #[active_type(LWW)]
    pub status: MessageStatus,
    pub task: Option<Ref<Task>>,
    pub anchor: Option<Ref<CodeAnchor>>,
    pub scope: Option<Ref<Scope>>,
    /// A comment on a decision, such as the reason recorded when the decision moved.
    pub decision: Option<Ref<Decision>>,
    pub reply_to: Option<Ref<Message>>,
    /// The root message of the thread, for replies.
    pub thread: Option<Ref<Message>>,
    pub context: Json,
    pub resolved_by: Option<Ref<Principal>>,
    #[active_type(LWW)]
    pub resolved: Option<i64>,
    /// When its author, or an agent that succeeded its author, last edited its text (`edited_by`
    /// says which), so a reader knows the words changed after they were sent. Absent on a message
    /// never edited, and on every message written before the field existed, which older readers
    /// never ask for.
    #[active_type(LWW)]
    pub edited: Option<i64>,
    /// Who last edited its text, set with `edited` by every edit: its author, or an agent that
    /// succeeded its author (task sVCUXcEg), so that a reader knows whose words the text holds now
    /// when they are not the author's. Absent on a message never edited, and on one last edited
    /// before the field existed (2026-09-30), when only its author edited a message.
    pub edited_by: Option<Ref<Principal>>,
}

// ---------------------------------------------------------------------------------------------
// Notification

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum NotificationType {
    /// A task's assignee became you.
    TaskAssigned,
    /// Someone commented on a task assigned to you.
    TaskCommented,
    /// Someone commented on code near a task assigned to you (until 2026-09-29, near a task worked
    /// in a scope you owned: D108), or on a highlight that is a step of a task you created or are
    /// assigned.
    CodeCommented,
    /// A message names you inline (`@you`).
    Mentioned,
    /// A message addressed to you, or a question asked of no one in particular while you triage
    /// the scope it asks, or serve the whole project where no agent serves that scope (until
    /// 2026-09-29, also one addressed to the supervisor role that you held).
    QuestionAsked,
    /// A reply in a thread you are in, addressed to no one (one addressed to someone reaches them
    /// as `QuestionAsked`, and no one else in the thread).
    Replied,
    /// A task or relation appeared in Proposed status that you may approve or reject.
    TaskProposed,
    /// A task was created in response to your message.
    TaskCreated,
    /// A task assigned to you became ready.
    TaskUnblocked,
    /// A task with `needs_review` was submitted; you triage one of its scopes, or serve the whole
    /// project where no agent serves one of them, or, with no triage agent to take it, you are the
    /// task's approver or creator.
    ReviewRequested,
    /// What one author did on one task, collected in one notification while it stays unread:
    /// steps added to the task, and comments on it or on its steps' highlights (D95, "A burst of
    /// comments and additions from one author on one task reaches a person as one notification";
    /// docs/design/agents-and-inbox.md, rule 10). A notification becomes one when a second item
    /// joins it, and its `originator.batch` lists the items (`routing::Batch`). Until 2026-09-28
    /// it was to collect status changes within 30 seconds, which was never built.
    Digest,
    /// A decision was proposed that you may accept or reject.
    DecisionProposed,
    /// Someone commented on a decision you filed or ruled on.
    DecisionCommented,
    /// A decision that something of yours cites was repealed: what cites it needs re-justifying.
    DecisionRepealed,
    /// A decision that something of yours cites was superseded by another.
    DecisionSuperseded,
    /// Someone added a step to a task you created or are assigned. Their next steps and comments
    /// on the task join it while it stays unread, and it becomes a `Digest`.
    StepAdded,
}
string_enum_property!(NotificationType);

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum NotificationStatus {
    Unread,
    /// Injected into an agent's context, or opened or marked by a person.
    Read,
    Dismissed,
}
string_enum_property!(NotificationStatus);

/// Created only when a specific principal needs to know about a thing. Thin: it points at the
/// target and at what triggered it; the content stays on the message or task. One per recipient.
/// A notification about what one author does on one task grows while it stays unread, so a burst
/// of steps and comments reaches its recipient once (docs/design/agents-and-inbox.md, rule 10).
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub recipient: Ref<Principal>,
    #[active_type(LWW)]
    pub notification_type: NotificationType,
    #[active_type(LWW)]
    pub status: NotificationStatus,
    /// Denormalized from the originating message for inbox ordering.
    #[active_type(LWW)]
    pub priority: Priority,
    pub created: i64,
    /// When the last item joined the notification's batch, so a batch that grew lists as new.
    /// Absent on a notification that never grew, whose time is `created`
    /// (`ops::notification_time`).
    #[active_type(LWW)]
    pub updated: Option<i64>,
    /// The primary entity this is about: `{"type": "Task", "id": "..."}`.
    pub target: Json,
    /// What triggered it: `{"message_id": "...", "actor_id": "...", "scope_id": "..."}`, why
    /// routing chose the recipient (`why`), and for a batch the items it holds (`batch`,
    /// `routing::Batch`).
    pub originator: Json,
}

// ---------------------------------------------------------------------------------------------
// Scope

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum ScopeStatus {
    Planned,
    Active,
    Paused,
    Blocked,
    Done,
    Abandoned,
}
string_enum_property!(ScopeStatus);

/// An area of work, which anyone may work in and several people and agents share
/// (docs/DECISIONS.md, D108, "A scope has no owner: scopes are not assigned to principals"). Tasks
/// attach through `worked_in` relations; who does a task is its `assignee`. Local-only fields
/// (working directory, worktree, attached sessions) live in the daemon, not here. Until 2026-09-29
/// a scope also had an owner and a heartbeat, which [`before_d108::Scope`] still declares so that
/// `fwd admin migrate` can clear them.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    #[active_type(LWW)]
    pub status: ScopeStatus,
    #[active_type(LWW)]
    pub title: String,
    /// What the work in it is trying to achieve.
    #[active_type(YrsString)]
    pub intention: String,
    /// Where the scope's work happens, so that a session finds its scope from the checkout it runs
    /// in: `{"repo": "<upstream key>", "branches": ["feat/inbox"]}`, the repository named by its
    /// upstream's key (`ops::repo_key`), never by a directory (docs/DECISIONS.md, D17,
    /// "Repositories are identified by their upstream remote, never by a worktree or a
    /// directory"). A scope from before 2026-09-25 carries a single `"branch"` and the directory
    /// it was made in (`"root"`) instead; its branch joins the list the next time its repository
    /// or its branches change. No base the branches fork from is recorded.
    pub context: Json,
    #[active_type(LWW)]
    pub started: Option<i64>,
    #[active_type(LWW)]
    pub ended: Option<i64>,
    /// Who made it: a record, which decides nothing about who works in it.
    pub created_by: Ref<Principal>,
    pub created: i64,
}

/// The fields a [`Scope`] had until 2026-09-29, when scopes lost their owner (docs/DECISIONS.md,
/// D108, "A scope has no owner: scopes are not assigned to principals"): declared here, under the
/// scope collection's own name, only so that `ops::migrate_scopes` can clear them from the scopes
/// written before, since a field no model declares cannot be written. Nothing else reads or writes
/// them.
pub mod before_d108 {
    use super::Principal;
    use ankurah::{Model, Ref};
    use serde::{Deserialize, Serialize};

    /// The scope as it was stored before scopes lost their owner, less the fields [`super::Scope`]
    /// still has. Each
    /// field is optional here, so that the migration can set it to none: `owner`, whoever made the
    /// scope, which decided who could work in it and who heard comments on its code; and its
    /// heartbeat, `last_heartbeat` and `heartbeat_ttl_s` (600), which the implementer's hook
    /// recorded. The struct takes the model's name, `Scope`, since a model is registered by its
    /// name as well as its collection, and `no_ffi`, since no binding needs it.
    #[derive(Model, Debug, Clone, Serialize, Deserialize)]
    #[model(no_ffi)]
    pub struct Scope {
        pub owner: Option<Ref<Principal>>,
        #[active_type(LWW)]
        pub last_heartbeat: Option<i64>,
        pub heartbeat_ttl_s: Option<i64>,
    }
}

// ---------------------------------------------------------------------------------------------
// Decision

#[cfg_attr(feature = "wasm", ::wasm_bindgen::prelude::wasm_bindgen)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "snake_case")]
pub enum DecisionStatus {
    /// Filed for a ruling that has not been given: accepted or rejected later, with a reason.
    Proposed,
    /// In force: what cites it may rest on it.
    Accepted,
    /// A proposal that was declined, with reason.
    Rejected,
    /// Replaced by a later ruling, which has a `supersedes` relation to it.
    Superseded,
    /// Withdrawn with a reason and nothing in its place: what cites it needs re-justifying.
    Repealed,
}
string_enum_property!(DecisionStatus);

/// A ruling recorded so that tasks, messages and other rulings can cite it by a stable name
/// (`D<number>`), and so that when it is repealed or superseded everything resting on it can be
/// found and re-justified. Architecture is one area among several: product, process, UX, naming
/// and dependency rulings are cited and repealed the same way. A ruling governs the scope it is
/// attached to, or the whole project when it is attached to none. Moves are verbs (accept,
/// reject, supersede, repeal), each recording its reason as a comment on the decision.
#[derive(Model, Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// The project's counter, shown as `D<number>`: the next after the highest existing, assigned
    /// at creation. Two clients creating at once can draw the same number; v1 is single-user.
    pub number: i64,
    #[active_type(LWW)]
    pub title: String,
    #[active_type(LWW)]
    pub status: DecisionStatus,
    /// What kind of ruling it is, free text: by convention architecture, product, process, ux,
    /// naming or dependency.
    #[active_type(LWW)]
    pub area: Option<String>,
    /// The scope whose work the ruling governs, so that in the context of one scope the rulings
    /// that apply are that scope's and the project's, and neither list is too long to read; none
    /// for a ruling that governs the whole project, as every decision recorded before the field
    /// existed does (docs/DECISIONS.md, D93, "A decision may be attached to the scope it governs;
    /// one attached to none governs the whole project, and its area is its type", 2026-09-28).
    /// The area says what kind of ruling it is; this says how far it reaches.
    pub scope: Option<Ref<Scope>>,
    /// Why it came up.
    #[active_type(YrsString)]
    pub context: String,
    /// The ruling, in the decider's own words when they are quoted.
    #[active_type(YrsString)]
    pub decision: String,
    #[active_type(YrsString)]
    pub consequences: String,
    /// Who filed it.
    pub author: Ref<Principal>,
    /// Who ruled: set when it is accepted or rejected, or at creation for a ruling recorded as
    /// given.
    pub decided_by: Option<Ref<Principal>>,
    pub created: i64,
    /// When it was ruled on.
    #[active_type(LWW)]
    pub decided: Option<i64>,
    /// Who repealed or superseded it, so routing leaves them out of the notifications it causes.
    pub closed_by: Option<Ref<Principal>>,
    #[active_type(LWW)]
    pub closed: Option<i64>,
    /// Why it was rejected, repealed or superseded: the note as it was typed, which a decision's
    /// JSON carries. The page and the exported log give the note from the comment the move wrote
    /// instead (`ops::closing_note`), whose references name what the note named, and this one only
    /// for a decision closed before moves wrote comments.
    #[active_type(LWW)]
    pub closed_reason: Option<String>,
    /// The comment that gave the reason for its repeal or supersession, written in the same
    /// transaction, so the notifications the move causes carry that comment and no later one.
    /// Absent on a decision closed before this field existed.
    pub closed_reason_message: Option<Ref<Message>>,
}
