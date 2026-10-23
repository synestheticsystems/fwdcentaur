# Data model

Draft for discussion, 2026-09-24; revised the same day after Daniel's answers to Q1 and Q2. The models are ordinary single-collection Ankurah Models (`#[derive(Model)]`) on 0.9.x/0.10, which the Manifold probe showed are sufficient for relation entities and typed reverse live queries. The message, notification and task shapes follow the simplikeys models Daniel pointed at (`~/sk/simplikeys/model/src/{message,notification,task}.rs`): durable, UI-visible messages and tasks each with one status field; thin per-recipient notifications with Json `target` and `originator` refs; Json `context` maps for relationship context. In Ankurah 0.9.3 JSON-path predicates failed over the wire on SQLite, so what a message is about moved out of its map into typed fields, and the code reads every `context` in Rust rather than querying it by path (`../DECISIONS.md`, D58, "Message subjects as typed reference fields"; "Json fields", below).

## Conventions

- **Project**: the task graph and its database, a UUID and a name; a set of repositories identified by their upstream remotes, never by a clone or a worktree (R, 2026-09-25). One durable node per project; every model below lives in it. Checkouts (clones and worktrees) are local, dynamic and never in the graph.
- **Identity**: every entity has an Ankurah `EntityId` (ULID, shown as its base64 string), and in v1 that full id is the identifier everywhere: CLI arguments, JSON output, `context` maps, notification refs (Daniel, Q3: sequential numbers are the most human-friendly and the least Ankurah-friendly; use the full id for now). The `T72`, `M15`, `S3`, `N120`, `C40` forms in these docs are placeholders for readability, not a feature. Human-friendly short ids are filed as a later task (T18: options are unique base64 prefixes like git short SHAs, which need no coordination, or project-sequential numbers, which need a single numberer). Since 2026-09-26 two narrow steps have been taken, both with the decision records: the CLI takes the first six characters or more of an id when they begin the id of exactly one entity the command could use (the first option, for the command line only), and a decision carries its project's number, cited as `D<number>` (the second option, for decisions only, numbered from the decisions themselves rather than by a numbering service).
- **Timestamps** are i64 milliseconds since epoch, wall clock, informational only; Ankurah's causal ordering decides merges.
- **Enumerations** are Rust enums stored as strings (the simplikeys `impl_wrapper_type!` plus `Property` pattern); unknown values are preserved, displayed as-is, and treated as "unknown" by rules.
- **Attribution**: every entity carries who made it and when. In v1 (single-user, `PermissiveAgent`) the client asserts it; once multi-user arrives the authenticated principal is what counts.
- **Prose fields** (task `what`, `why`, `done_when`; message content; scope intention) use Ankurah's Yrs-backed text so two people or an agent and a person can edit the same field without clobbering.
- **Json fields** are used pragmatically, as in simplikeys. `context` on messages, tasks and scopes is a map for what needs no field of its own because no query selects by it: the code loads the entity and reads the map in Rust, since a JSON-path predicate on it (`context.task_id = ?`) failed over the wire on SQLite in Ankurah 0.9.3 ("Implementation notes", below). A message keeps there whom it asks (`to`, or `asks` with the scope it asks, `asks_scope`), the scope its author wrote it in (`written_in`), on a step's first comment written with the step, that step (`first_of_step`), and, on the comment that records why a decision moved, the move (`move`); what a message is about is in its typed fields `task`, `anchor`, `scope` and `decision`, each one reverse query away (`../DECISIONS.md`, D58, "Message subjects as typed reference fields"; Message, below). A task keeps there the ids of the scope it was made or started in, of the parent it was made under and of whoever assigned, submitted, approved or accepted it, and the evidence it was submitted with; a scope keeps its repository's upstream key and its branches (`repo`, `branches`), which are names, not ids (Scope, below). `target` and `originator` on notifications are `{type, id}` refs resolved generically by the UI; `pieces` on anchors records how the anchored lines stood against HEAD and the index when the anchor was made. Where an anchor is now in some version of its file is per-checkout knowledge, cached in the daemon's data directory beside the project's database (`projects/<id>/anchors.sqlite`), never on the entity (`anchoring.md`, "Resolution"). Relation endpoints are plain `EntityId` fields (stored exactly as a `Ref` is), so a reverse live query still binds a typed EntityId literal (`query::with_id`).
- **Local-only data** (working directory, worktree path, agent session ids, socket paths, pids) is not stored in Ankurah. The daemon keeps it in its own SQLite table keyed by scope id and serves it to local clients. (U: whether Ankurah should grow local-only collections.)

## Entities

### Principal

A person or an agent identity. Agents are principals in their own right so attribution and inboxes work the same way for both.

```rust
#[derive(Model)]
pub struct Principal {
    pub kind: PrincipalKind,          // Human | Agent
    pub handle: String,               // a name, not a key: the GitHub login for people ("dnorman"); the name an agent gave itself ("impl-alice", "triage-bot")
    pub display_name: String,
    pub auth_subject: Option<String>, // "github:<numeric id>" captured at fwd init (R: a user id from the start); JWT sub later
    pub agent_role: Option<AgentRole>,// Lead | Triage | Implementer | Reviewer: the role an agent selected; none for a person
    pub active: bool,                 // false once retired: kept for attribution, offered by no picker
    pub member: Option<bool>,         // whether it belongs to the project; none on a principal recorded before 2026-09-29
    pub subject: Option<String>,      // "claude-code:<session>/<agent>": the Claude Code agent whose commands act as it; none for a person, and for an agent no agent is bound to
    pub succeeded_by: Option<Ref<Principal>>, // the principal that took over its job (`fwd principal add --succeeds`, `fwd principal succeed`), to whom routing delivers what reaches it
    pub serves: Option<String>,       // a triage agent's scopes and endorsement, as JSON text: {"scopes": [...], "project_wide": true}; none for any other principal, and for a triage agent that serves nothing
    pub last_active: Option<i64>,     // when it last wrote to the graph, which shows an agent quiet for twelve hours as dormant; none until its first write since 2026-09-30
}
```

Principals live in each project's graph, so every project has its own: a person or an agent that works in two projects is two principals with one handle. An agent selects its role from a fixed set as it names itself (`fwd principal add <name> --role lead|triage|implementer|reviewer`) and may change it later (`fwd principal role <name> <role>`); a person holds none, and no role is ever read from a handle (`agents-and-inbox.md`, "Roles"). A triage agent serves scopes, and a project has, among its principals that are not retired, at most one triage agent per scope and one endorsed to serve it whole (`../DECISIONS.md`, D122), and a lead per session or worktree that coordinates its work (D119): a second triage agent for a scope, or for the whole project, is refused, naming the principal that serves it, and so is a second lead bound to an agent of one Claude Code session, naming that session's lead (`ops::add_principal`, `ops::set_role`, `ops::serve`, and the succession and binding operations), a retired one brought back serving what it served, or holding lead, included. `serves` records what a triage agent serves: the scopes it took (with `--scope`, from its checkout, or with `fwd principal scope add`) and whether it is endorsed to serve the whole project (`--project-wide`, `fwd principal scope project-wide`), which suits only a small project with small scopes (`agents-and-inbox.md`, "What a triage agent serves"). It is JSON text in a string because the Model derive of the pinned Ankurah gives every field a getter for the page's wasm build and an optional `Json` has none, while a field added after principals were written must be optional; succession moves it with the role; another role clears it. A triage agent from before 2026-09-30 records none, and serves nothing until its lead binds or endorses it. An agent principal with no role, which a build before 2026-09-29 could leave and `fwd admin migrate` leaves when two principals held the old role, reads as before, and its next write is refused until it selects one. Until 2026-09-29 the roles were `Implementer` and `Supervisor`, the second taken by any handle starting with `supervisor`; `fwd admin migrate` gives the role triage to the principal that held `supervisor` and wrote most recently, keeping its handle, and clears the role from any other that held it (`cli.md`, "Maintenance").

`handle` is the principal's name, a convenience label that several principals may share, within a project as across projects; a principal's identity is its id, which every reference to it stores (`../DECISIONS.md`, D107 and D110). A typed name is resolved within the project, and a name that two or more principals of the project share is refused with them listed, never guessed (`agents-and-inbox.md`, "A principal's name"). An agent changes its own with `fwd principal rename <name>`, which keeps its id and so everything that refers to it; a person's is the handle `fwd init` recorded.

`member` records whether the principal belongs to the project, which decides whether a picker offers it (`agents-and-inbox.md`, "Who belongs to a project"; `../DECISIONS.md`, D106): true for a person from `fwd init`, for an agent from `fwd principal add`, by which it names itself, and for any principal from its first command that writes work into the project; false for the person a command that only reads brought into a project they never joined, and for an agent `fwd admin migrate` found had only read, or only written messages, there. A principal belongs while it is active and its `member` is true, or is none: one recorded before the field existed belongs while active, until `fwd admin migrate` records whether it does (`ops::belongs`). `fwd principal retire` sets `active` to false, and nothing but `fwd principal add`, run as that principal, sets it back: every other write of a retired principal is refused, naming that command, while its reads run (task Wbkkwpyq; `cli.md`, Principals). Until 2026-09-30 its writes went through and left it retired.

`subject` names the Claude Code agent whose commands act as the principal: the session's id and the agent's (`main` for the session's own thread), which the fwd plugin's hook puts into each command it stamps and fwd reads as `claude-code:<session>/<agent>` (`agents-and-inbox.md`, "An agent's identity"; `../DECISIONS.md`, D112 and D118). A command that carries a subject acts as the principal of its project whose `subject` it is; `fwd principal add` sets it on the principal it makes, and `fwd principal bind` on a principal from before the plugin, whose `subject` is none, once. A principal carries one subject at most, and a subject is bound to one principal; a person carries none. `auth_subject` is the person's account elsewhere (D59), which nothing writes yet; `subject` is an agent's, which fwd reads on every command.

`succeeded_by` records the principal that took over this one's job when an agent was relaunched to carry it on (`fwd principal add <name> --succeeds <old>`, or `fwd principal succeed <old>` for a principal named already; D114): the old principal is retired as it is set, its role moves to the successor, and its unread notifications with it. Routing reads it for every recipient it plans, so whatever would reach a succeeded principal reaches its successor, and a successor's successor after that (`routing::Successors`), with the principal it was for recorded in the notification (`originator.for_id`). The same successors edit the messages the succeeded principal wrote (Message, "Editing"). `fwd principal add`, run as a succeeded principal, brings it back and clears it, and with it its successors' right to edit its messages.

`last_active` records when the principal last wrote to the graph, so that the pickers and `fwd principal list` can show an agent that has written nothing for twelve hours as dormant, below the others, where the users left behind by finished agents no longer crowd out the agents at work (`../DECISIONS.md`, D123: a subagent retires its own user when its job ends, and nothing retires a user by time, since fwd cannot tell a quiet agent from a gone one; `crates/fwd-core/src/dormancy.rs`). Every client stamps it through one function, `dormancy::wrote`, once a write of its has succeeded, but for two kinds of write that are no agent's work on the graph: marking notifications read or dismissed, which reads an inbox, and `fwd admin migrate`, the person's upkeep of the graph (task bWI438nh counts a message, a task change, a relation, a decision and a principal change as an agent's activity). The CLI stamps the principal a command acts as after every command that writes work into the project or changes a principal, and never after marking notifications or after `fwd admin migrate`; it stamps only once the command has run without error, so that a refused command stamps no one, and within the part of the command that ends it when its daemon went away, so that a stamp the daemon never got ends the command as a lost write does. The page and the extension stamp the principal they act as after each of their other writes. `fwd principal add`, anew or with `--succeeds`, stamps the agent it makes as it makes it, since the command that names an agent acts as another principal, or as none. The field is stored rather than read from what each principal wrote because two of the kinds of write that count leave nothing a reader could find: a task's changes after its creation set its `updated` and record no author, and a principal's changes record no time at all; an entity's id is the hash of its creation event rather than a time, and Ankurah's events carry a time but no author (D123's consequences name the field; where each client stamps it, and that an agent with none reads as dormant, are the lead's rulings of 2026-09-30 on task bWI438nh). An agent that is not retired is dormant when its `last_active` is more than twelve hours old by the reader's clock, or when it has none, as every agent recorded before 2026-09-30 has none until its next write: the finished agents of before then go below the others at once, one still at work comes back at its next write, and nothing fills the field in for them. A person is never dormant. Dormant is an overlay for display, derived as a task's availability overlays are (Task, below), and never enforced: a dormant agent is still offered, below the others, still addressable, still reached by routing, and still writes, and its next write makes it active again.

### Repo

A registered repository. Anchors reference it so a project can span several repositories.

```rust
#[derive(Model)]
pub struct Repo {
    pub name: String,
    pub remote_urls: Json,     // ["git@github.com:org/x.git", ...]
    pub default_branch: String,
}
```

### Task

The unit of work, as fine-grained as the work actually divides. Three prose fields answer three questions: what, why, and how we will know it is done. One status field (Daniel, Q2); one optional assignee, the person or agent doing it, so a task may wait unassigned; readiness is derived. Assignment was a separate relation to a scope, from the answer to Q2, until Daniel made it a field on 2026-09-28 (`../DECISIONS.md`, "Assignment is an optional field on the task, never an edge"); the relation to a scope, now `worked_in`, says where the work happens. A task may be worked in any number of scopes, none of them primary (D136, "A task may be worked in several scopes", 2026-10-03: "yes, allow several scopes per task."), or in none; until then it was worked in one at a time (`audits/several-scopes-2026-10-03.md` lists what that changed).

```rust
#[derive(Model)]
pub struct Task {
    #[active_type(LWW)] pub title: String,
    #[active_type(YrsString)] pub what: String,      // spec / description
    #[active_type(YrsString)] pub why: String,       // motivation in prose; motivated_by relations carry the graph form
    #[active_type(YrsString)] pub done_when: String, // acceptance criteria
    #[active_type(LWW)] pub status: TaskStatus,
    pub assignee: Option<Ref<Principal>>, // who does the work; none until someone is assigned or starts it (a task from before the field reads as none)
    pub needs_review: bool,    // if true, Done requires accept by someone other than the submitter
    pub hold: Option<String>,  // manual hold with reason; an availability overlay, not a status
    pub priority: Option<i32>, // 0 (urgent) .. 4; None = unranked
    pub labels: Json,          // ["bug", "cli"]: what kind of work it is, in words ("Labels", below)
    pub context: Json,         // read in Rust, never queried ("Json fields", above): who assigned, submitted, approved and
                               // accepted it ("assigned_by", "submitted_by" with its "evidence", "approved_by", "accepted_by"), and the
                               // parent it was made under ("parent_id"), which its subtask_of relation says too; its worked_in relations
                               // say where it is worked (../DECISIONS.md, D45, "Assignment is an optional field on the task, never an edge;
                               // the scope edge means where the work happens"). Until 2026-10-03 it also recorded the scope the task was
                               // made or started in ("scope_id"), which routing read for a task worked in no scope; nothing writes or
                               // reads it now but `fwd admin migrate`, which gives a task that records one and has no worked_in relation
                               // at all a worked_in to it (ops::migrate_recorded_scopes): a task made in a scope before 2026-09-30, when
                               // making a task wrote no worked_in, and never started
    pub created_by: Ref<Principal>,
    pub created: i64,
    pub updated: i64,
    pub closed: Option<i64>,
    pub closed_reason: Option<String>,
}

pub enum TaskStatus {
    Proposed,   // "I think this should exist; tell me if not": drawn as a ghost, never ready, waiting for approval or feedback
    Open,       // sanctioned, not started
    InProgress, // started; whoever started it is its assignee
    InReview,   // submitted with evidence; awaiting acceptance
    Done,
    Cancelled,  // was sanctioned, then abandoned, with reason
    Rejected,   // a proposal that was declined, with reason
}
```

**Transitions** (verbs in the CLI; the enum is never set directly):

| Verb | From | To | Notes |
|---|---|---|---|
| create | | Open, or Proposed with `--propose` | agents default to Proposed for anything outside their own tasks; the task is `worked_in` the scope the command works in (`--scope`, else the one the checkout finds; the page's, the header's), in the transaction that makes it, and in none where none is found (task cToJvD22); more scopes are linked afterwards (`fwd link <task> worked_in <scope>`, the scopes on its panel: `web-ui.md`, the Tasks page) |
| approve | Proposed | Open | by any principal, with its reason as a comment on the task (`../DECISIONS.md`, D32) |
| reject | Proposed | Rejected | with reason |
| start | Open | InProgress | also makes the caller the assignee and, when the caller works in a scope and the task is worked in none, the task `worked_in` that scope, atomically; refused while the task is assigned to someone else, or worked in scopes none of which is the caller's ("Starting", below) |
| assign / unassign | any | same | sets or clears `assignee`; the new assignee is told |
| submit | InProgress | InReview, or Done when `needs_review` is false | with evidence |
| accept | InReview | Done | by a principal other than the submitter |
| return | InReview | InProgress | with reason |
| done | Open, InProgress | Done | only when `needs_review` is false |
| cancel | Open, InProgress, InReview | Cancelled | with reason |
| reopen | Done, Cancelled, Rejected | Open | with reason |
| rejustify | Open, InProgress | Proposed | a premise was repealed; the assignment stays, so the panel shows "assigned, under re-justification" |
| hold / release | any non-terminal | same | sets or clears `hold` |

A reason given to a move (a reject's, a return's, a cancel's, a reopen's or a rejustify's) is kept as a comment on the task by whoever made the move, written in the move's own transaction, opened by the move ("Returned: cover the retry too") and naming it in its `context.move`, as a decision's move keeps its reason (`ops::transition`; task jJp3bcF7); a reject's and a cancel's is `closed_reason` too, and a reopen clears that with the close. Until 2026-10-03 a return's, a reopen's and a rejustify's reason was asked for and kept nowhere. A submission's evidence stays in the task's context, not repeated as a comment, so that it does not reach the task's reviewer twice.

An agent's "I finished" is `submit`; acceptance is a separate verb by a different principal. Adding a new dependency to a Done task does not reopen it; the panel shows "completion predates dependency" until someone reopens or dismisses.

**Availability overlays** are derived, not statuses: "waiting on" (open `depends_on` targets), "delegated" (assigned to an agent), "held" (`hold` is set). Until 2026-09-28 "delegated" read the scope an `assigned_to` relation named (`../DECISIONS.md`, "Assignment is an optional field on the task, never an edge"). Until 2026-09-29 the design also had "stale", a task worked in a scope that had missed its heartbeats, which went with the heartbeat when scopes lost their owner (`../DECISIONS.md`, D108, "A scope has no owner: scopes are not assigned to principals"); it was never built. A principal has one overlay, "dormant", derived in the same way from when it last wrote (Principal, `last_active`).

**Steps** are what a task asks its assignee to look at or do, in order: each is a `step_of` relation from the thing (a highlight, a message, another task) to the task, and its status is the checkbox (`../DECISIONS.md`, "A step is an edge from any entity to a task, and the highlight is the step" and "The checked state is the step edge's status", 2026-09-28). The checklist is the task's accepted and checked steps, ordered by the relation's `position`, then by when each was added, then by id; its progress is how many are checked of how many there are. A step is added at the end; a task is finished when its assignee says so, not when its last step is checked. A step that is a highlight moves with the first thread on the highlight: checking it resolves that thread, and resolving the thread checks it (Rules, below; `../DECISIONS.md`, D99, "Checking a step resolves its highlight's first thread, and resolving that thread checks the step; each control says it does both").

**Labels** say what kind of work a task is, so that people and agents find the tasks of one kind and filter a list by them: free text, with `bug` and `feature` the labels the project uses by convention, since a task's kind is a label and not a task type (`../DECISIONS.md`, D97, "A task's kind, such as bug or feature, is a label, not a task type", 2026-09-28). A label is stored in one form: trimmed, each run of whitespace inside it made one space, and in lower case, so that `Bug`, ` bug ` and `BUG` are the one label `bug` and find the same tasks; the case a label was typed in is not kept, because a label is there to filter by. A label is never blank and never holds a comma, since a comma separates labels wherever several are typed at once, and a task carries each label once. A label stored before these rules compares in the same form, so taking off `web` takes off a `Web` written earlier. A task's labels change after it is filed (`fwd task amend --label` and `--unlabel`, or its panel on the page), and nothing in the rules reads them (`crates/fwd-core/src/labels.rs`).

### Relation

An edge is an entity with two named endpoints and full provenance. Binary only in v1 (Ankurah has `Ref<T>` but no `RefSet`); a multi-target relation is several binary relations sharing a `via`.

```rust
#[derive(Model)]
pub struct Relation {
    pub kind: RelationKind,
    pub from_id: EntityId,     // `from` is reserved in ankql predicates; plain ids: an endpoint may be any entity
    pub to_id: EntityId,
    pub status: RelationStatus,// Proposed | Accepted | Retracted | Rejected | Resolved (a checked step, and nothing else)
    pub created_by: Ref<Principal>,
    pub created: i64,
    pub via: Json,             // what caused this assertion: {"message_id": "..."} or {"actor_id": "..."}; a mention's {"inline": true, "ordinal": n}
    pub note: Option<String>,  // one line of rationale
    pub position: Option<String>, // a step's place in its task's checklist: base-62 digits sorting as text (crates/fwd-core/src/position.rs); none on other kinds
    pub closed_by: Option<Ref<Principal>>,
    pub closed: Option<i64>,
    pub closed_reason: Option<String>,
}
```

| kind | from -> to | Meaning | Invariants |
|---|---|---|---|
| `depends_on` | Task -> Task | `from` is not ready until `to` is Done | Acyclic over accepted relations; a create that would close a cycle is refused with the cycle shown; a cycle arriving by sync is flagged, never dropped |
| `subtask_of` | Task -> Task | `from` is part of `to` | Acyclic; a parent cannot be marked Done while accepted subtasks are open unless `--force` with reason; parent readiness is independent |
| `motivated_by` | Task -> Task or Message | why this task exists, in graph form | none |
| `discovered_from` | Task -> Task or Message | provenance: the work that surfaced this task | none |
| `relates_to` | any -> any | free association, symmetric in display | none |
| `supersedes` | Task -> Task; Decision -> Decision | lineage; the old task is usually cancelled with reason; the old decision is Superseded by the new one | between decisions, made only by `fwd decision supersede`, which marks the old one Superseded in the same write (`fwd link` refuses it), and never undone: nothing moves a superseded decision back, so `fwd unlink` refuses it too, and a new decision supersedes the successor instead |
| `duplicates` | Task -> Task | lineage | none |
| `anchored_at` | Task -> CodeAnchor | the code this task concerns (a message names the anchor it is on in its own `anchor` field instead: `../DECISIONS.md`, D58, "Message subjects as typed reference fields") | none |
| `worked_in` | Task -> Scope | where the work on the task happens; who does it is the task's `assignee` | one accepted `worked_in` per task and scope, and any number of scopes per task, none of them primary (`../DECISIONS.md`, D136, "A task may be worked in several scopes", 2026-10-03; until then at most one per task, and a second was refused); its readers list a task's scopes in the order it came to be worked in them, by each relation's `created`, then its id (`ops::scopes_in`); taken off by retracting it (`fwd unlink`, the scopes on the task's panel through `ops::change_scopes`, which adds and takes off in one write), which tells no one; a start never moves a task out of its scopes ("Starting", below). It was `assigned_to`, meaning who works the task, until assignment became a field (`../DECISIONS.md`, "Assignment is an optional field on the task, never an edge", 2026-09-28); `fwd admin migrate` renames a graph's old ones once, keeping each, and nothing makes one any more |
| `step_of` | anything -> Task | `from` is a step of the task: a highlight to look at, a message, another task | acyclic between tasks, as `subtask_of` is (a checked step counts); made once per thing and task; `position` orders the checklist; checked by moving it to `resolved`, with `closed_by` and `closed` saying who and when, and unchecked by moving it back to `accepted` with them cleared; no other kind is ever `resolved`; a highlight's steps move with the first thread on it (Rules, Steps); taken off the checklist by moving it to `retracted` with the reason in `closed_reason`, by the task's creator or its assignee (Rules, "Taking a step off a checklist") |
| `mention` | Message -> anything | the message's text names `to` inline, stored there as `[[<full id>]]` (a whole file at a version as its whole-file highlight's, `[[<full id>@<version>]]`) | one per reference in the text, written with the message when it is posted and never by `fwd link`; `via` is `{"inline": true, "ordinal": n}`, its place among the text's references from 1, with `"edited": true` added on one an edit of the text wrote; an edit keeps the relations of what the new text still names, taking their new places, and retracts those of what it no longer names, with the reason (Message, "Editing"); a page leaves it out of the message's own lozenges, since the text shows it, and lists it as "mentioned in" on what it names |
| `concerns` | Message -> Task, Relation, Scope | the triage agent's linking of a human utterance to graph nodes | none |
| `cites` | Task, Message or Decision -> Decision; Decision -> Task or Message | `from` rests on the ruling `to`, so it needs re-justifying when the ruling is repealed or superseded; from a decision to a task or a message, the ruling was made in that task's or that question's context | a decision at one end or both, never the same entity at both; made once (citing again answers the relation that stands, and accepts it, with the new note when one is given, when it was only proposed) |

`fwd link`, and `fwd task create` for the relations its `--after`, `--under`, `--from` and `--motivated-by` make, refuse a relation whose ends are not of the kinds its row gives (`ops::joins`), naming them and what the ends are, since what every page reads from a relation rests on them (`cli.md`, "Relations").

`blocks` is not stored; it is the inverse of `depends_on` and is derived for display and queries.

### CodeAnchor

An interior sub-entity of a file (Manifold's pointing contract applied to source code): lines of one version of the file, its basis, named by git's object id of the content the person was looking at, committed or not; or with no lines, the whole file at that version (Daniel, 2026-09-28, "A whole-file reference is an anchor with no line range"). It is created when someone points at lines or at a file, once per key: the same lines of the same content in the same repository are one anchor on every machine. It is never mutated afterwards, except that an anchor made before keys existed gains its key and basis when a checkout can supply them. See `anchoring.md`.

```rust
#[derive(Model)]
pub struct CodeAnchor {
    pub repo: Ref<Repo>,
    pub path: String,                // repo-relative, at the anchored version
    pub anchor_key: Option<String>,  // "fwd-anchor:1:" + sha256(repo, path, basis_blob, range); absent on anchors made before keys until backfilled
    pub span_key: Option<String>,    // "fwd-span:1:" + sha256(anchored text, whitespace runs collapsed); absent for a whole file
    pub basis_blob: Option<String>,  // git's id of the content seen, written and pinned under refs/fwd/basis/; the lines count it
    pub head_blob: Option<String>,   // the file's HEAD blob when the anchor was made, if any
    pub index_blob: Option<String>,  // the file's index blob when the anchor was made, if any
    pub nearest_blob: Option<String>,// whichever of those two differs least from the basis
    pub basis_patch: Option<String>, // unified diff from nearest_blob (or an empty file) to the basis, up to 256 KB
    pub blob_sha: Option<String>,    // the first cut's basis, on anchors made before basis_blob; never written now
    pub commit_sha: Option<String>,  // context: the commit checked out when the anchor was made, if any
    pub side: DiffSide,              // Worktree (the file on disk) | Base | Modified (the left or right of a diff showing a revision)
    pub start_line: i64,             // 0-based, inclusive; 0 for a whole file
    pub end_line: i64,               // 0-based, exclusive; 0 for a whole file
    pub whole_file: Option<bool>,    // true for an anchor on the whole file; absent on anchors made before them
    pub anchored_text: String,       // the exact selected lines; empty for a whole file
    pub context_before: String,      // up to 3 lines
    pub context_after: String,       // up to 3 lines
    pub pieces: Json,                // [{basis_lines, head_lines, index_lines}]: runs committed, staged only, or in neither (the working tree, or the revision the anchor was made on)
    pub symbol_path: Option<String>, // optional tree-sitter path, later
    pub created_by: Ref<Principal>,
    pub created: i64,
}
```

### Message

A comment or note: durable, visible in the UI, with a status (Daniel, Q2). Everything a human or agent *says* is a message: a comment on a line of code, a comment on a task, a question to someone, a reply, a suggestion. There is no proposal model and no message kind (Daniel): a message is free text, and some messages ask questions or make proposals at the writer's discretion; whoever reads one decides what to do. The only structured notion of "proposed" is the Proposed status on a task or relation. What the message is about lives in typed fields of its own, and whom it asks in `context` (below).

```rust
#[derive(Model)]
pub struct Message {
    pub author: Ref<Principal>,
    pub created: i64,
    #[active_type(YrsString)] pub content: String, // markdown
    pub priority: Priority,     // Now | Next | Later; set by the author, editable by the triage agent (re-triage)
    pub status: MessageStatus,  // Active | Resolved | Deleted
    pub task: Option<Ref<Task>>,         // what it is about (below): a task,
    pub anchor: Option<Ref<CodeAnchor>>, // code (a highlight, on lines or on the whole file),
    pub scope: Option<Ref<Scope>>,       // a scope,
    pub decision: Option<Ref<Decision>>, // or a decision; several of these, or none
    pub reply_to: Option<Ref<Message>>,  // the message it answers
    pub thread: Option<Ref<Message>>,    // the first message of its thread, on a reply
    pub context: Json,          // see below
    pub resolved_by: Option<Ref<Principal>>,
    pub resolved: Option<i64>,
    pub edited: Option<i64>,    // when its author, or an agent that succeeded its author, last edited its text; absent until then, and on messages from before 2026-09-28
    pub edited_by: Option<Ref<Principal>>, // who last edited its text: its author, or an agent that succeeded its author; absent until then, and on a message last edited before 2026-09-30
}
```

**Editing.** A message's author may correct its text where it stands (`ops::edit_message`, `fwd message edit`), so that a correction does not split the discussion across two threads and leave the old wording in sight (Daniel, 2026-09-28, of a question an agent had sent him: "Are you able to edit this question?"). So may an agent that succeeded its author (Principal, `succeeded_by`), directly or through a chain of successions: an agent relaunched to carry on another's job answers for what its predecessor wrote, and corrects an answer the retired agent gave where it stands (task sVCUXcEg, after Daniel asked for an edit of an answer whose author, the triage agent, had been retired and succeeded; `../DECISIONS.md`, D114, a relaunched agent's user succeeds its predecessor's). No one else edits a message (`rules::can_edit_message`, given the author's successors in turn, which `ops::edit_message` reads from the principals as routing does: `routing::Successors::chain`); anyone else answers it with a reply, which keeps who said what. The edit replaces the whole text and changes nothing else: the message keeps its id, its thread, its place (`created`) and its author, `edited` records when its text changed and `edited_by` who changed it, set by every edit, the author's own included, and the text it had stays in the entity's history, not in a field. The page's message row and `fwd message show --format text` name whoever made the last edit when it is not the author, since the words are then not all the author's (`web-ui.md`, "A message row"; `cli.md`, "Messages"). The new text is read as a post's is (below): its references are resolved and stored as full ids, and one that names nothing refuses the edit, writing nothing. A blank text is refused, and a text that stores as the message's own changes nothing.

A comment on a decision names it in a typed `decision` field, beside `task`, `anchor` and `scope` (which moved out of `context` into typed fields, see "Implementation notes"); every move of a decision records its reason as such a comment, and a reply to one carries the decision on.

**Inline references.** A writer names a person in a message as `@handle`, and anything by its id, the first six characters or more of it, or `D12` for a decision, after an at-sign (`@vwugaN8h`; the handle is tried first) or in double brackets (`[[vwugaN8h]]`, `[[D12]]`) (`../DECISIONS.md`, "Inline references are shadowed by `mention` edges", 2026-09-28). Posting a message (`ops::post_message`, which the CLI, the page and the extension all post through) resolves each one under the command line's prefix rules, stores it in the text as `[[<full id>]]` whatever was typed, so every reader finds the thing without a lookup, and writes one `mention` relation per reference in the same transaction (`ops::write_mentions`, which also writes those of an edit and of the comment a decision's move writes). A reference that names nothing, or a start of an id that several ids share, refuses the post with a sentence naming it, so a typo is caught before anyone reads it. Text in code (between backticks, or in a fenced block) names nothing. A writer names a whole file at a version by its path from the repository's root in double brackets (task nVE7KTeSqiX0; `../DECISIONS.md`, D125, "Anything a message names in its text is a mention, whatever its kind"): `[[docs/cli.md]]` as it stands on disk, unstaged changes included, `[[docs/cli.md@index]]` its staged copy, `[[docs/cli.md@e13b8e2]]` (or a branch, a tag, `HEAD`) the file at that commit; a slash or a dot tells a path from an id (`crates/fwd-core/src/file_mention.rs`; `cli.md`, "Messages"). Posting reads the file in the checkout the command runs in and refuses one that is not there at that version; the file becomes its whole-file highlight (CodeAnchor, below), found by its key or made, and the text stores the reference as that highlight's full id, an at-sign and the version, `[[<full id>@<version>]]`, the version `worktree`, `index` or the full id of the commit a revision named when the message was posted. The version goes in the text, with the mention, because the highlight is keyed by its content and serves every version that holds the same bytes, so it cannot say which version the writer named (the lead's ruling of 2026-09-30 on that task); the `mention` relation points at the highlight as any mention points at what it names. That stored form reads again anywhere, with no checkout, as a forward's quote and an edit read their text, which an edit box writes back as the start of the highlight's id and the version; a reader draws it as the highlight's path and that version, the short commit, `index` or `on disk` (fwd-core's `anchor_name::AnchorName::file_at`). Messages written before 2026-09-28 keep their text as it was. An edit resolves the new text the same way and, in the same transaction, brings the message's `mention` relations to what the new text names: a reference to what the text named before keeps its relation, which takes the reference's new place among the references; a reference to anything else gets a new relation, whose `via` says an edit wrote it, and whose person or agent, not named before, hears of it (`agents-and-inbox.md`, rule 13); and a relation to what the text no longer names is retracted with the reason, which tells no one.

What a message is about is in typed fields, so that a task's, a highlight's, a scope's or a decision's messages are one reverse query away over the wire, which a key inside `context` was not (`../DECISIONS.md`, D58, "Message subjects as typed reference fields"; "Implementation notes", below): `task`, `anchor` (code: a highlight, on lines or on the whole file), `scope` and `decision`, each optional, so a message is about one of them, several (a comment on a task about some of its code has `task` and `anchor`) or none (a question from the page's Ask box). A reply names the message it answers in `reply_to` and the first message of its thread in `thread`, and carries on what that message is about (`ops::post_message`). `context` keeps the rest: when the message asks something, either `to`, the id of the one principal it asks, or `asks`, set to `"project"` on a question asked of no one in particular (until 2026-09-29 `to` could hold the string `"supervisor"`, the role, which `fwd admin migrate` replaces with `asks`), with beside it `asks_scope`, the id of the scope the question asks, fixed as it is posted: the scope `fwd ask --scope` or the page's Ask box names, else the first scope what it is about concerns, else the one its asker wrote it in; absent where none of these finds one. The triage agent of that scope answers it, else the one that serves the whole project, and while neither exists it waits, listed under its scope, or under the project for one that asks none (`../DECISIONS.md`, D122; `agents-and-inbox.md`, rule 5). `written_in` names the scope the author wrote a message in, the scope its command found (`--scope`, else the one its checkout finds), which routing reads for a message whose subject concerns no scope's work, so that a comment on code no task is near reaches the triage agent of the scope its author works in. `first_of_step` names, by its relation's id, the step that the command which added it wrote the message with as the step's first comment (`fwd step add <task> --path ... <text>`, `ops::add_highlight_step`), a fact the writer records rather than a time its clock gave, so that the step's list of the threads open on its lines leaves the comment out and rule 14 reports it as the step's first comment, whichever of the two routing reaches first (`agents-and-inbox.md`, rule 10; task WTCAd9la). And `move` names the move on the comment that records why a decision moved (Decision, below). A comment on a line of code is a message with `anchor`; a question to Daniel about T72 has `to` and `task`; an agent's note after acting on a notification is a reply with `reply_to`. A message is `Resolved` when whoever it concerned has dealt with it: a question answered, a suggestion acted on or declined, a comment addressed. When a task is created in response to a message (a `discovered_from` or `motivated_by` relation to it), the message's author is notified that the task exists, which usually says more than a reply would.

### Notification

Created only when a specific principal needs to know about a thing (Daniel). Thin: it points at the target and at what triggered it; the content stays on the message or task. One per recipient. What one author does on one task (steps added, comments on it or on its steps' highlights) reaches each recipient as one notification, which grows while it stays unread (D95; `agents-and-inbox.md`, rule 10).

```rust
#[derive(Model)]
pub struct Notification {
    pub recipient: Ref<Principal>,
    pub notification_type: NotificationType,
    pub status: NotificationStatus, // Unread | Read | Dismissed
    pub priority: Priority,         // denormalized from the originating message for inbox ordering
    pub created: i64,
    pub updated: Option<i64>,       // when the last item joined its batch, so it lists as new; absent on one that never grew
    pub target: Json,               // the primary entity this is about: {"type": "Task", "id": "..."}
    pub originator: Json,           // what triggered it: {"message_id": "...", "actor_id": "...", "scope_id": "..."},
                                    // why routing chose this recipient: {"why": "parent_author"} (agents-and-inbox.md),
                                    // and for a batch its items: {"batch": {"task_id": "...", "items": [...]}}
}

pub enum NotificationType {
    TaskAssigned,    // a task's assignee became you (until 2026-09-28: a task was assigned to your scope)
    TaskCommented,   // someone commented on a task assigned to you
    CodeCommented,   // someone commented on code near a task assigned to you, or on a highlight that is a step of your task
                     // (until 2026-09-29: near a task worked in a scope you owned, D108)
    Mentioned,       // a message names you inline (@you)
    QuestionAsked,   // a question addressed to you, or asked of no one when you triage the scope it asks, or serve the whole project where no agent serves that scope
    Replied,         // a reply in a thread you are in, addressed to no one (one with a `to` is a QuestionAsked to its addressee)
    TaskProposed,    // a task or relation appeared in Proposed status that you may approve or reject
    TaskCreated,     // a task was created in response to your message (discovered_from or motivated_by it)
    TaskUnblocked,   // a task assigned to you became ready
    ReviewRequested, // a task with needs_review was submitted; you triage one of its scopes, or serve the whole project where no agent serves one, or (with no triage agent to take it) you are its approver or creator
    Digest,          // what one author did on one task, collected while unread: steps added and comments (D95);
                     // until 2026-09-28 meant for status changes within 30 seconds, never built
    DecisionProposed,   // a decision was proposed that you may accept or reject
    DecisionCommented,  // someone commented on a decision you filed or ruled on (a move's reason is such a comment)
    DecisionRepealed,   // a decision that something of yours cites was repealed: re-justify what rests on it
    DecisionSuperseded, // a decision that something of yours cites was superseded by another
    StepAdded,          // someone added a step to a task you created or are assigned, with the threads open on its lines
                        // that you did not write in, are not asked by and were not told of (task WTCAd9la); their next items join it (a Digest)
}
```

"Delivered into an agent's context" is `Read` (the hook sets it on injection). A human's notification stays `Unread` until opened or marked. An agent's disposition is a reply message, not a field here. The daemon's routing service creates notifications (`agents-and-inbox.md`), and grows a batch while it stays unread (its type, target, priority, `originator` and `updated`); clients change only `status`, and reading or dismissing a batch ends it. The one exception is succession, which moves the old principal's unread notifications to its successor, rewriting each one's `recipient` once and recording whom it was for (`originator.for_id`; `agents-and-inbox.md`, "An agent's identity"; D114).

A notification's `status` is its recipient's to change: only the principal `recipient` names reads it, dismisses it, or settles it by replying through it or forwarding it, as the page marks no card of another principal's read (task AfhuiHYS; `rules::can_settle`). `fwd read`, `fwd dismiss`, `fwd reply` and `fwd forward` given a notification addressed to another principal refuse it, naming whose it is, and change nothing, and `fwd inbox --for <another> pop`, which marks what it prints read, refuses that principal's inbox before it prints anything (`cli.md`, "Inbox and notifications"); anyone may still answer the message behind it by the message's own id, which settles no notification. A successor is refused the notifications its predecessor kept, those it had read or dismissed, since the unread ones moved to it. Until 2026-09-30 those commands changed any notification's `status`, so that an agent that mishandled an id could take a notification out of another's unread inbox, and the triage agent kept a standing rule never to mark Daniel's notifications read.

### Scope

An area of work, which anyone may work in and several people and agents share (`../DECISIONS.md`, D85, which gave this project its scopes, and D108, "A scope has no owner: scopes are not assigned to principals", 2026-09-29). Tasks attach through `worked_in` relations, which say the work happens there (`assigned_to` until 2026-09-28); who does a task is its `assignee`.

```rust
#[derive(Model)]
pub struct Scope {
    pub status: ScopeStatus,   // Planned | Active | Paused | Blocked | Done | Abandoned
    pub title: String,
    #[active_type(YrsString)] pub intention: String, // what the work in it is trying to achieve
    pub context: Json,         // {"repo": "<upstream key>", "branches": ["feat/inbox"]}; older scopes carry a single "branch"
    pub started: Option<i64>,
    pub ended: Option<i64>,
    pub created_by: Ref<Principal>, // who made it: a record, which decides nothing
    pub created: i64,
}
```

Until 2026-09-29 a scope also had an `owner`, whoever made it and never changed, left over from the vision's scopes per team member (`../DECISIONS.md`, D1), and a heartbeat (`last_heartbeat`, `heartbeat_ttl_s`, default 600) that the implementer's hook recorded and nothing read but the page. The owner decided which scope a session could work in and bind its checkout to, who heard a comment on code near a task or on the scope, and where the page opened and what Claim offered; each of those now reads the checkout, the task's assignee or the triage agent of the scope instead (D108; per scope since D122). `fwd admin migrate` clears the three fields from the scopes written before (`cli.md`, "Maintenance"), and the `ScopeStale` notification, never sent, went with the heartbeat.

A scope may move from any status to any other; nothing orders them. `started` is when it first took a status past planned: a scope made planned has none until then (2026-09-28, when the page began making scopes with a status). `ended` is set when it becomes done or abandoned and cleared when it leaves them, as a reopened task's `closed` is. Its name cannot be blank, and it names branches only with the repository they are in, since a session finds its scope by the repository and the branch its checkout is on (`ops::create_scope`, `ops::edit_scope`, which every client's writes go through).

Local-only, kept by the daemon: the checkout registry (which clones and worktrees exist on this machine, and which scope each is bound to through `fwd scope attach-dir`: any scope, whoever made it, honoured while it is active), attached agent sessions (harness, session id, messaging socket path, pid, attached_at), and the last `fwd prime` output hash.

### Decision

A ruling, recorded so that it has a stable name people and agents cite (`D12`), and so that when a premise changes (a ruling repealed or replaced) everything that rested on it can be found and re-justified rather than remembered and grepped for (Daniel's working rule for a repealed premise). Rulings are not only architectural: product, process, UX, naming and dependency rulings are cited and repealed the same way, so each carries an area rather than being an architecture record. A decision is not a task: the two stay separate kinds (Daniel, 2026-09-26: "proposed tasks and proposed/draft decisions may overlap, but the distinction is worth keeping"), and where they meet (a task that waits on a ruling, a ruling that implies work) the link is a `cites` relation, never a status on either.

```rust
#[derive(Model)]
pub struct Decision {
    pub number: i64,                    // the project's counter, cited as D<number>: one past the highest, at creation
    #[active_type(LWW)] pub title: String,
    #[active_type(LWW)] pub status: DecisionStatus,  // Proposed | Accepted | Rejected | Superseded | Repealed
    #[active_type(LWW)] pub area: Option<String>,    // free text; by convention architecture, product, process, ux, naming, dependency
    pub scope: Option<Ref<Scope>>,          // the scope whose work it governs; none for the whole project
    #[active_type(YrsString)] pub context: String,   // why it came up
    #[active_type(YrsString)] pub decision: String,  // the ruling, in the decider's own words when they are quoted
    #[active_type(YrsString)] pub consequences: String,
    pub author: Ref<Principal>,             // who filed it
    pub decided_by: Option<Ref<Principal>>, // who ruled: the filer by default; Daniel for his rulings, whoever records them
    pub created: i64,
    #[active_type(LWW)] pub decided: Option<i64>,    // when it was ruled on (a day given for a ruling recorded later)
    pub closed_by: Option<Ref<Principal>>,  // who repealed or superseded it, whom routing leaves out
    #[active_type(LWW)] pub closed: Option<i64>,
    #[active_type(LWW)] pub closed_reason: Option<String>, // why it was rejected, repealed or superseded
    pub closed_reason_message: Option<Ref<Message>>, // the comment a repeal or supersession wrote, in the same transaction; its notifications carry it
}
```

**Moves** (verbs in the CLI; the status is never set directly; every move records its reason as a comment on the decision, written with the move):

| Verb | From | To | Notes |
|---|---|---|---|
| create | | Accepted, or Proposed with `--propose` | a ruling recorded as given names who ruled (`--decided-by`, else the filer) and may give the day it was ruled on (`--decided`); `--from` records the task, message or decision it was decided in, as a `cites` from the new decision |
| accept | Proposed | Accepted | the one who accepts is its decider; one who records a person's ruling (`--decided-by`) names that person instead |
| reject | Proposed | Rejected | the one who rejects is its decider and its closer; one who records a person's ruling (`--decided-by`) names that person as both; the reason is `closed_reason` too |
| supersede | Accepted | Superseded | names the ruling that replaces it, which must be another decision and already Accepted; creates the `supersedes` relation from the new one |
| repeal | Accepted | Repealed | nothing takes its place; what cites it needs re-justifying |

Nothing else moves: a rejected, superseded or repealed decision stays so, and a new decision takes its place. The number is one past the highest any decision of the project has, computed at creation from the decisions themselves; two clients creating at once could draw the same number, which is acceptable while v1 is single-user and would be a diagnostic later. A move has the same window: it checks the decision's status before its transaction, and Ankurah has no compare-and-set, so two clients moving one decision at once both commit; the status the last write sets stands (last writer wins) and both reasons stay as comments. Also acceptable while v1 is single-user, and a diagnostic later. The day a ruling was made is kept at noon UTC when it is given as a day, so it is that day in every time zone people are likely to read it in; logs show days in the reader's own time zone.

**Scope** (`../DECISIONS.md`, D93, "A decision may be attached to the scope it governs; one attached to none governs the whole project, and its area is its type", 2026-09-28). A ruling given about one scope's work, such as one branch's review, read on the Decisions page as a rule for the whole project, since nothing said how far it reached (Daniel, 2026-09-28: "decisions should be attached to their relevant scope (and have their type specified) such that in any given context (project wide, or a specific scope) the list of decisions isn't overwhelmingly long"). So a decision may be attached to the one scope whose work it governs, as it is recorded (`fwd decision create --attach`) or later (`fwd decision attach`), and detached (`fwd decision detach`), after which it governs the whole project again. A decision attached to no scope governs the whole project: one recorded without `--attach`, one detached, and every decision recorded before the field existed, where the field reads as none (`fwd admin migrate` has nothing to do). In the context of one scope, the rulings that apply are the project-wide ones and those attached to that scope (`rules::applies_to`), which is what the Decisions page lists for a scope and `fwd decision list --attached <scope> --project-wide` prints. The area says what kind of ruling it is; the scope says how far it reaches. Attaching and detaching change neither the ruling nor its status, so they tell nobody (routing reads a decision's status alone) and ask no permission of their own: whoever may comment on a decision may attach it (`ops::attach_decision` and `ops::detach_decision`, which refuse a scope that does not exist, as `ops::create_decision` does). Which answers become records at all, and when a ruling is attached, is the primer's to say (`../using-fwd.md`, "Decisions").

**Routing** (`agents-and-inbox.md`, rules 11 and 12): a proposal reaches the triage agent of the scope it governs, else the one that serves the whole project, and the project's people; a comment on a decision (every move's reason among them) reaches whoever filed it and whoever ruled; a repeal or a supersession reaches the authors of everything that cites the decision, which is the retraction audit's inbox side, carrying the comment the move wrote (`closed_reason_message`), and that comment's own notifications leave those authors out (the comment names its move in `context.move`), so no one hears of one move twice. None of them reaches whoever caused it.

**The log.** `fwd decision export` renders every decision as markdown in the style `docs/DECISIONS.md` was written in by hand, headed by a comment saying it is a rendering: the rulings that govern the whole project first, under "Project-wide", then each scope's under the scope's name, the scopes by name, so that whoever works in one scope reads the project's rulings and that scope's; within each, in force, proposed, then rejected, superseded and repealed, each by number (a status with no ruling is left out); each entry `- **D<n>: title** (area, governs <scope>, day, decided by …): the ruling`, a project-wide one naming no scope, with its context, consequences and links. `scripts/import-decisions.py` turned the hand-written file into records once; from then on the file is that rendering.

## Rules (implemented once, in fwd-core)

**Readiness** of a task is three-valued, following Manifold: `ready`, `not_ready`, `unknown`.
- `ready` when: `status == Open`, `hold` is None, and every accepted `depends_on` target is `Done`.
- `not_ready` when any of those fails for a known reason (the reason list is returned: `blocked_by: [T70, T71]`, `hold`, `proposed`, `in_progress`).
- `unknown` when a dependency target cannot be loaded or is not authorized; the CLI never reports "no blockers" for an incomplete domain.
- A Cancelled or Rejected dependency target makes the dependent `not_ready` with reason `dependency_closed`, which the triage agent brings to its lead to resolve (retract the relation or cancel the dependent), as it brings everything that needs a decision (`../DECISIONS.md`, D129).

**Cycles**: `depends_on`, `subtask_of` and `step_of` are each acyclic over the relations that hold (accepted ones, and for steps checked ones too). A create or approve that would close a cycle is refused (exit 5) with the cycle listed. A cycle that arrives through sync is recorded as a diagnostic and the involved tasks show `readiness = unknown` until repaired; no edge is dropped automatically.

**Starting**: `fwd task start T72` sets `status = InProgress` and makes the caller its assignee, and, when the caller works in scope S3 and the task is worked in no scope, makes the task `worked_in` S3, in one transaction. It is refused while the task is assigned to someone else, so two people never hold one task; `fwd task assign` reassigns it first. It is refused while the task is worked in scopes none of which is S3 as well, so that a start never moves a task out of the scopes someone chose for it (`../DECISIONS.md`, D87, "Starting a task worked in another scope is refused, with a prompt to switch to that scope first", 2026-09-28): the refusal names each of the task's scopes and how to switch to one, `--scope` for each and a binding of the checkout for each active one (`rules::can_start_in`), and `fwd task next` passes such a task over. A task worked in several scopes starts from any of them and stays in each (D136, "A task may be worked in several scopes"; until 2026-10-03 only a race could leave a task in two, and it started from either). Before D87, a start from another scope moved the task there, retracting its `worked_in` to the scope it left. A person with no scope starts a task all the same, and the task's scopes stay as they are. The page's Start button makes the same start, in the first of the task's own scopes, which changes none of them, or, for a task worked in no scope, in the active scope the button names, or with several active the one chosen in the picker after it, and shows only where the start would be let through, so never on a task assigned to someone else (`web-ui.md`, the Tasks page); until 2026-10-03 it read "Claim, in <scope>" and showed on every open task. Until 2026-10-03 a start also wrote the scope it was made in into the task's `context` (`scope_id`), which nothing reads now. Until 2026-09-28 a claim was the start plus an `assigned_to` relation, refused while another accepted one existed (the answer to Q2; `../DECISIONS.md`, "Assignment is an optional field on the task, never an edge"). Until 2026-09-29 agent scopes recorded a heartbeat, and the design marked a scope whose heartbeat was older than its time to live "stale", for the triage agent to reassign its tasks; nothing was built on it, and it went with the owner (D108).

**Steps**: checking a step moves its `step_of` relation from `accepted` to `resolved`, and unchecking moves it back; the rules refuse `resolved` on every other kind, and a proposed, retracted or rejected step is on no checklist to be checked (`rules::check_step`). A highlight's steps and the first thread on it, the thread started first that answers no other message and is not deleted (`rules::first_thread`), move together, so that a checklist and the conversation on its lines never disagree (`../DECISIONS.md`, D99, "Checking a step resolves its highlight's first thread, and resolving that thread checks the step; each control says it does both", which superseded D91): checking or unchecking any of the highlight's steps resolves that thread's first message or sets it back to active, and resolving it or setting it back checks or unchecks every step the highlight is, each member that moves recording who moved it and when (a message's `resolved_by` and `resolved`, a step's `closed_by` and `closed`), in one transaction. A later thread, a reply, a step that is a message or a task, and a highlight with no thread move alone (`rules::move_step_and_thread`). None of it notifies anyone: it adds nothing for routing to react to.

**Taking a step off a checklist**: a step that no longer belongs on its task's checklist is retracted rather than checked, with who, when and the reason in its closing fields, and what the step is stays (task 9dvGW0EI, "Take a step off a checklist and retire its highlight in one command: fwd step remove"). Only the task's creator and its assignee, whose checklist it is, take a step off (`rules::can_remove_step`); anyone else is refused with a sentence naming them. Retiring a highlight as its step comes off resolves the first message of every thread on it that is open, so the editor stops drawing it, and resolving its first thread checks the highlight's steps on other checklists, as the rule above (D99) moves them (`rules::retire_highlight`); all of it is one transaction, which adds nothing and so notifies no one.

**Acceptance**: with `needs_review`, `accept` must come from a principal other than the one who submitted.

**Editing a message**: only its author edits it, and an agent that succeeded its author, directly or through a chain of successions (`rules::can_edit_message`); anyone else is refused with a sentence naming the author and pointing to a reply instead, or to the agent that carries on the author's job now (Message, "Editing").

**Rights** (policy configuration, not code): by default humans do everything directly; a triage agent may, in the scopes it serves or, endorsed, in the whole project, create `concerns` and other relations, append to `why`, `what` and `done_when`, and create tasks (Proposed) and subtasks directly; it approves or rejects Proposed tasks and relations, and accepts or returns a review, only where a person's words or a recorded decision plainly settle it, bringing every other to its lead (`../DECISIONS.md`, D129, "The triage agent is a Sonnet agent that annotates and relays"), and asks a person, in a message, for cancels and retractions; implementer agents move their own assigned tasks' status, create subtasks of them directly, create everything else as Proposed, and ask for the rest in a message. Whoever may comment on a decision may attach it to a scope or detach it, the ruling being unchanged (D93). (Provisional, Q5.)

**Where rules run**: each rule is a `fwd-core` function used twice: as a pre-check in the client (to explain a refusal before committing) and in the daemon's validation service (to catch races and misbehaving clients after the fact, marking violations with a diagnostic and, for relations, `status = Rejected`). With multi-user the second use moves into `PolicyAgent::check_event` and becomes a refusal at commit.

## Derived views the daemon serves

- Task with computed readiness, blockers, dependents, subtasks, its assignee and the scopes it is worked in, its steps with how many are checked, open messages.
- The "frontier": ready tasks, the caller's first, then unassigned ones; tasks assigned to someone else are left out.
- Scope with tasks, inbox counts, and local-only fields.
- Graph neighbourhood: bounded traversal from roots over selected relation kinds, with explicit `omitted` counts (Manifold's rule: no implied completeness).
- Inbox: `recipient = ? AND status != 'Dismissed'` ordered by priority then created, the simplikeys query.
- Project diagnostics: cycles, orphaned anchors, old proposals. (Until 2026-10-03 also a task worked in two scopes, which a task may be since D136.)

## What v1 does not model

Time (due dates, estimates, recurrence), multi-target relations (RefSet), components on one entity, cross-project references, and per-field permissions. Each has a place to grow without changing the CLI.

## Implementation notes (T2, 2026-09-24)

The models live in `crates/fwd-core/src/model.rs` and round-trip through SQLite in `crates/fwd-core/tests/roundtrip.rs`. Facts learned against Ankurah 0.9.3 that the design above depends on (since 2026-09-25 fwdcentaur builds against the `storage-engine-contract` line pinned by hash, see `DECISIONS.md`; the round trip, the CLI tests and the roles smoke pass there, and the quirks below have not been re-verified on it):

- Ankurah was pinned to the published `=0.9.3` until 2026-09-25; since 2026-09-30 it is the `feat/storage-node-sqlite-wasm` branch at `f8e85d2e` from GitHub, which is the `storage-engine-contract` branch's `90dff659` (0.10.0, unpublished) with storage/sqlite-node and the durable node's publication fix added. Both build and test on stable Rust; no nightly toolchain is needed.
- Ankurah's `#[derive(Property)]` stores a custom type as its serde JSON encoding, so a unit enum would be stored as `"\"open\""` and predicates would have to match that. fwd-core instead implements `Property` for each enum by hand through a small macro that stores the strum `snake_case` name, so `status = 'in_progress'` works as written in this document.
- Relation endpoints, which may point at any entity, are plain `EntityId` fields (2026-09-25; they were `Ref<Any>` with a marker struct). `EntityId` implements `Property` and stores the same `Value::EntityId` a `Ref<T>` does, so nothing on disk changed; the wasm derive was the reason: it wraps every `Ref<T>` field and needs `T` to be a Model.
- Custom-typed and `Option<String>` fields need an explicit `#[active_type(LWW)]`; `String` defaults to the Yrs text backend, and `i64`, `bool`, `Option<i64>`, `Json` and `Ref<T>` default to LWW.
- A typed reverse query binds an `EntityId` literal into the predicate (`"to = ?"` populated with `Value::EntityId`), which is required for live queries to receive updates (Ankurah issue #259). JSON-path predicates on `context` (`context.anchor_id = '<base64>'`) work on SQLite.
- **JSON-path predicates are not usable over the wire on SQLite in 0.9.3.** `context.task_id = '...'` worked on an in-process durable node but failed with "malformed JSON" on the daemon for entities that arrived from a client. The message's subjects therefore moved from the `context` map to typed reference fields (`task`, `anchor`, `scope`, `reply_to`, `thread`), which reverse-query correctly in both cases; `context` kept only `to`. Filed as an Ankurah finding (T19). Since 2026-09-26 `context` also keeps `move`, on the comment that records why a decision moved, which every move writes and routing reads so that no one hears of a repeal or a supersession twice (`agents-and-inbox.md`, rules 11 and 12; `../DECISIONS.md`, D34, "Decision records live in the graph, with statuses, an area, links to what they govern, and a page listing them all").
- **Some predicate shapes are not indexed by the reactor.** A live query on `created >= 0` never notified (bare integer literal with `>=`), while `created > 0`, `created >= '0'` and string equalities did. A boolean literal (`active = true`) did not match entities that arrived over the wire; `active` is filtered in Rust. Also filed under T19.
- **An empty Yrs text reads as `PropertyError::Missing`**, not as an empty string; prose fields go through a helper that maps that to `""`.
- **A fresh entity appears in a change set as both `added` and `appeared`**; the routing service consumes `added` only and keeps a bounded set of handled ids.
- **An optional field added later reads as none on what was written before it** (2026-09-28: `Task.assignee`, `Relation.position`, `Message.edited`, `Notification.updated`; 2026-09-30: `Message.edited_by`): Ankurah reads a missing `Option` property as `None`, so old tasks, relations and messages load unchanged, and `fwd admin migrate` has nothing to do for them. A kind that is renamed is another matter, since an enum reads a stored name it does not know as an error: `RelationKind` keeps `assigned_to` beside `worked_in` so the relations written before the rename load until `fwd admin migrate` renames them, and it can go once every graph is migrated.
