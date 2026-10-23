# Retraction audit: a task may be worked in several scopes

2026-10-03, for task x5y55fdl ("Let a task be worked in several scopes, after a retraction audit of the one-scope rule"), written before any code.

## The premise that changed

Until today a task was worked in one scope at a time. The rule came in with the assignee field on 2026-09-28 (commit d9719a3): a check in the client refused a second accepted `worked_in` relation, and the retraction audit recorded under D45 ("Assignment is an optional field on the task, never an edge; the scope edge means where the work happens") kept the relation "with its one-per-task invariant". Daniel's words that day were "edges to scopes"; the one-scope rule was the recorder's. On 2026-10-03 he asked "can a task currently have more than one scope? ideally it would", then ruled (D136, "A task may be worked in several scopes"): "yes, allow several scopes per task." A task may have any number of accepted `worked_in` relations, and none of them is primary.

`worked_in` was always a relation, so the data model stays. What rested on the premise is everything that refused a second scope, everything that read "the" scope of a task as the first relation it found, and everything that said a task has one scope.

## How this audit was made

Each dependent was found by searching the code, the docs, the skills, the scripts and the extension for `worked_in`, `WorkedIn`, `task_scope`, `assigned_scope_of`, `scope_id`, "one scope", "two scopes", "worked in", "the task's scope", D45 and D87, and by reading every function that reads a task's scope and every caller of one. Each gets one disposition:

- **Retract**: the premise was its reason. It goes, or it is rewritten to hold every scope a task is worked in.
- **Re-justify**: it stands, on a reason that holds with several scopes, given here.
- **Historical**: it records what was true or what was done then, and stays as written.

Six entries turned on four open questions, asked of the lead on 2026-10-03 with a recommendation each, and the lead agreed to all four the same day; each such entry says so, and "The answers" at the end gives them.

Places that mention a scope but rest on something else are not dependents and are not listed: a decision governs one scope or none (D93); a checkout finds one scope, and is refused a command that needs one where several name its branch; a scope's row counts the tasks worked in it, which holds for a task counted in two rows.

## The rulings

- **D45's clause keeping `worked_in` "with its one-per-task invariant"**, in the decision log. *Historical.* The record stays as the ruling of its day; D136 repeals that clause. The rest of D45 stands: the assignee is a field, and the relation to a scope says where the work happens.
- **D87, "Starting a task worked in another scope is refused, with a prompt to switch to that scope first"**. *Re-justify.* What it is for still holds: a start never moves a task out of the scopes someone chose for it. With several scopes, a start from any of the task's scopes goes through, and a start from a scope that is none of them is refused, naming every scope the task is worked in and how to switch to one.

## Rules and operations (crates/fwd-core)

- **`rules::can_work_in` and its refusal `RuleError::WorkedElsewhere`** ("task is already in scope …; retract that `worked_in` first"), the client check that refused a second scope. *Retract.* Both go, and nothing replaces them.
- **The unit test `a_task_is_worked_in_one_scope_at_a_time`** in rules.rs. *Retract.* It goes with the check; an operations test links a second scope instead.
- **`rules::can_start_in`'s doc**, which says a task is worked in "one, or none" scope and that "a task that a race left worked in two scopes starts from either of them, so that it is never stuck", **and the case of its test that says so**. *Re-justify.* A task worked in several scopes starts from any of them because D136 allows several, not because a race left two.
- **`RuleError::WorkedInAnotherScope`**, which names the first of the task's scopes only. *Retract* the one-scope shape: the refusal names each scope the task is worked in, with how to switch to one; for a task in one scope it keeps today's sentence word for word.
- **`ops::link`'s doc ("a task is worked in one scope at a time") and its call to `rules::can_work_in`**. *Retract.* Linking a task to another scope adds that scope.
- **`ops::link` answering a second link to the scope a task is already worked in with the relation that stands**. *Re-justify.* A task is worked in each scope once, so linking the same scope again makes no second relation.
- **`ops::worked_in`**, "the `to` of its accepted `worked_in` relation, if it has one", the first it finds: `fwd task show`, `fwd task explain` and `fwd graph` read it. *Retract.* It answers every scope the task is worked in, in the order the task came to be worked in them, so that every reader lists them alike.
- **`ops::task_scope`**, the one scope whose triage agent answers first for a task: the first `worked_in`, else the `scope_id` the task's context records. *Retract.* It becomes the task's scopes, read from its `worked_in` relations alone, so that a task whose last scope was taken off is in none (the task's text: "context.scope_id stops being read as a scope when the task has none").
- **`ops::scopes_worked_in`'s doc ("one, or none")**. *Retract* the wording: any number.
- **`ops::start` and `ops::next_task`**. *Re-justify.* A start from any of the task's scopes goes through and writes no relation; a task worked in none comes to be worked in the starter's scope, as before; `fwd task next` passes over a task none of whose scopes is the caller's.
- **`ops::create_task` and `ops::start` writing `context.scope_id`**, the scope a task was made or started in, which only `ops::task_scope` read. *Retract* (asked of the lead, who agreed): both stop writing it, since nothing reads it once routing stops and the `worked_in` relation records where the task was made or started, with who and when. What older tasks recorded stays, read by nothing.
- **`CreateTask.scope`**, the one scope a task is made in. *Re-justify.* A task is made in the scope its maker works in, which is one; more are added afterwards (`fwd link <task> worked_in <scope>`, the page's tags).
- **`ops::migrate_assignments` (`fwd admin migrate`)**, which keeps one accepted relation per task and retracts the others with the reason "a task is in one scope at a time; the migration kept one". *Retract* for every run from now on: it keeps every accepted relation it renames from `assigned_to` to `worked_in`, and gives the task its assignee from the newest of them, as before.
- **The relations the migration of 2026-09-28 retracted in this graph**, with that reason. *Historical.* They stay retracted.
- **The doc of `Task.context` in model.rs** ("routing reads `scope_id` only for a task worked in no scope"). *Retract,* with `ops::task_scope`.

## Routing (crates/fwd-core/src/routing.rs)

- **`plan_for_task_created`**: a proposed task reached the triage agent of its one scope. *Retract.* It reaches the triage agent of each of its scopes, each scope's own, else the one that serves the whole project, each agent once (D136: "A proposal reaches the triage agent of each of its scopes").
- **`plan_for_task_status`, a review (agents-and-inbox.md, rule 8)**: the triage agent of the task's one scope took it, else the one that serves the whole project. *Retract* (asked of the lead, who agreed): the triage agent of each of the task's scopes takes it, never the submitter, else the one that serves the whole project, else the approver or the creator as now, since no scope is primary.
- **`plan_for_task_status`, an unassigned task that became ready (rule 9)**: the triage agent of its one scope. *Retract* (asked of the lead, who agreed): the triage agent of each of its scopes.
- **`plan_for_relation` and `relation_scope`, a proposed relation (rule 4)**: the triage agent of the first scope of the task at its `from` end, else at its `to` end. *Retract* (asked of the lead, who agreed): the triage agent of each scope of the task at its `from` end, else of the task at its `to` end.
- **`scopes_concerned`**, the scopes a comment concerns (rules 1 to 3). *Re-justify.* It already gathers a list; it now holds each scope of the task a comment is on, and of the tasks a highlight is a step of or is near, so a comment on a task worked in two scopes reaches both triage agents.
- **`asked_scope` and `ops::post_message`**: a question asked of no one in particular asks "the first scope what it is about concerns". *Re-justify* (asked of the lead, who agreed): a question reaches one triage agent (D111), so it still asks one scope; of a task's several scopes, the one its asker works in when it is one of them, else the scope the task came to be worked in first.
- **The comments in reason.rs** that call the recipient of a review "the triage agent of the task's scope". *Re-justify.* Each notification names the one scope its own recipient triages (`triage_scope_id`), which holds with several; the wording says "one of the task's scopes".

## Names (crates/fwd-core/src/naming.rs)

- **`naming::scopes_of`**, the scopes a principal's work is in, which tells apart principals sharing a name: it read one scope per task through a map from task to scope, so a task in two scopes counted in one. *Retract.* Each scope of each task counts.

## The command line (crates/fwd/src/main.rs)

- **`fwd task show` (`task_details`)**, printing `worked_in` and `worked_in_title`, the first scope. *Retract.* It prints `scopes`, each with its id and its name; `worked_in` and `worked_in_title` stay for one release, holding the first of `scopes`, for readers that read one (the task's text), and cli.md says when they go.
- **`task_text`**, "scope: X". *Retract* the one-scope shape: "scope: X" for one scope, "scopes: X, Y" for several.
- **`fwd task explain` and `fwd graph`**, each printing `worked_in`. *Retract.* Each prints `scopes`, keeping `worked_in` for one release.
- **The help of `fwd task start` and `fwd task next`** ("A task in another scope is refused, naming that scope"). *Re-justify* the wording: a task none of whose scopes is yours, naming them.
- **The migration's text** (", and retracted: a task is in one scope at a time"). *Retract,* with the migration's retraction.

## The page (crates/fwd-web)

- **`store::assigned_scope_of`**, the first accepted `worked_in` (or `assigned_to`, the name before 2026-09-28) of a task, which the Tasks filter, the panel, Start, the Graph, the Questions page and the page's opening scope read. *Retract.* The task's scopes, every one, in the order the task came to be worked in them.
- **`ScopeChoice::keeps`**, which took the one scope a thing is part of. *Retract.* A scope chosen keeps what is part of it among others; "no scope" keeps what is part of none; "all scopes" keeps everything.
- **The Tasks page's list**, filtered by one scope and naming one in its Scope column. *Retract.* It keeps a task under each of its scopes and names them all.
- **The task panel's scope field** (`Assignment`: one scope's lozenge, or "no scope"). *Retract.* A lozenge for each scope; task oLv1sJ3v makes them tags that a click takes off, with a [+] that adds one.
- **Start (`claim::offer`, whose `Offer::Its` held one scope)**. *Retract.* It holds the task's scopes, from any of which the start goes through, as `rules::can_start_in` judges. Start starts the task in the first of them; its button names no scope and offers no picker, since starting in any of them changes none of the task's scopes, so which one the page passes changes nothing but that the start goes through (the lead agreed, 2026-10-03, and noted it on D136).
- **`scope_filter::start_on`**: a route that names a task opens on "the scope it is worked in". *Re-justify.* It opens on one of the task's scopes, the first, which lists the task.
- **`scope_filter::opening_scope`**: the scope of each task in progress the acting principal holds. *Retract* the one-scope read: each of those tasks' scopes counts.
- **The Questions page (`scope_about`)**: a question was about one scope. *Retract.* A question about a task is about each of the task's scopes, and the scope filter keeps it under each; one asked of no one is about the scope it asks.
- **The lozenge preview of a task**, one scope pill. *Retract.* A pill for each scope.
- **The lozenge table's treatment of a task's scope**, "the scope in its own look, one, with no label" (Daniel, 2026-09-26, "Not all relationship types should be rendered the same"), shown one then counted. *Re-justify.* "One" was how many scopes a task had; each scope shows in its own look, all of them, with no label.
- **The Graph (`graph.rs`: `TaskFacts.scope`, and a stub's scope name)**. *Retract.* A task is drawn when any of its scopes is chosen, and a stub names its scopes. This file is another implementer's; the change is kept to those lines.

## The host and the extension

- **The doc of fwd-host's `claim`**, a task "worked in another scope is refused". *Re-justify* the wording: one none of whose scopes is the one named.
- **The comments in the extension's comments.ts and current.ts** ("the current task's scope"). *Re-justify* the wording: no code there reads a task's scope; the CLI chooses the scope a question asks.

## The docs and the skills

- **data-model.md, the table of relations**: "at most one accepted `worked_in` per task at a time". *Retract.*
- **data-model.md, "Starting" and the transition table's start row**, among them "A task that a race left worked in two scopes starts from either of them". *Re-justify,* as D87 above.
- **data-model.md, the `context` comment on Task** (routing reads `scope_id` back). *Retract.*
- **data-model.md, the derived views** ("the scope it is worked in"; diagnostics listing "a task worked in two scopes"). *Retract.*
- **daemon.md**: the pre-check refusing "a second accepted `worked_in` for a task", and the validation service, not built, that would mark "a task worked in two scopes". *Retract.*
- **cli.md**: `fwd link` ("A task is worked in one scope at a time, so a second `worked_in` is refused"), `task show` and `task start`. *Retract* the link rule; *re-justify* start; `task show` gains `scopes`.
- **agents-and-inbox.md, rules 2, 4, 5, 8 and 9** ("the triage agent of the task's scope"). *Re-justify* rule 2 (each scope's, as `scopes_concerned` already reads); rules 4, 5, 8 and 9 follow the answers below.
- **agents-and-inbox.md, the paragraph before the rules** ("A task concerns the scope it is worked in, else the one it was made in, which its context records", citing `ops::task_scope`), found by the review of 2026-10-03. *Retract.* A task concerns each scope it is worked in, and one worked in none concerns no scope, so what concerns it reaches the triage agent that serves the whole project; the record is history, which `fwd admin migrate` carries into a `worked_in`.
- **cli.md, `task create`** (a task filed in a scope before 2026-09-30, "its context recording the scope it was made in, which routing still reads for such a task"), found by the same review. *Retract,* in the same words as the paragraph above; it also says `task create` prints `scopes`.
- **web-ui.md**: the Tasks list ("its scope's name"), the panel's scope field, the scope filter ("a scope keeps the tasks worked in it"), the page's opening rule, and the lozenge table's scope row ("one"). *Retract* each one-scope sentence.
- **using-fwd.md, the fwd-implementer and fwd-lead skills, and the guide to the extension (docs/guides/vscode-extension.html)**, where a rule says "its scope" or "the task's scope". *Re-justify* the wording: "its scopes", "one of its scopes".
- **reviews-and-highlights.md's summaries of D45 and D87**. *Historical.*

## The tests and the smoke script

- **crates/fwd-core/tests/ops.rs, `a_task_is_assigned_started_and_worked_in_one_scope`, its part "One scope at a time: a second `worked_in` is refused"**. *Retract.* A second scope is linked, and the task starts from either.
- **The same file, `a_task_made_in_a_scope_is_worked_in_it_from_the_start`, its last part** (`ops::task_scope` reads the context's `scope_id`). *Retract.* A task whose context records a scope and which has no `worked_in` is in no scope.
- **The same file, the migration test** ("a task keeps one accepted `worked_in`"). *Retract.* It keeps every accepted relation.
- **crates/fwd/tests/task_create.rs and scripts/smoke-roles.sh**, which read `worked_in` from `fwd task create`, `task show`, `task explain` and `task start`. *Re-justify.* `worked_in` is still printed for one release; the tests read `scopes` too.
- **The page's unit tests in claim.rs, scope_filter.rs, graph.rs, questions.rs and lozenges.rs** that pass one scope. *Retract* the one-scope inputs; they pass lists.

## The live graph

- **Tasks made in a scope before 2026-09-30, when making a task wrote no `worked_in`, that were never started**: their context alone records the scope, and only routing read it. In this graph four are open (LM-OrKvY, hGcyWpR0, on6WtFb4, ldqHcux9), and none of them has a `worked_in` relation of any status; the page already shows them in no scope. *Retract* the context as their scope (asked of the lead, who agreed): `fwd admin migrate` gives each task that records a scope and has no `worked_in` relation at all a `worked_in` to that scope, so that a task whose scope was taken off on purpose stays in none; the lead runs it after install.

## Counts

61 dependents: 41 retract, 17 re-justify, 3 historical. Six of them turned on the questions below. Two, the paragraph before the routing rules in agents-and-inbox.md and `task create` in cli.md, were found by the review of the branch on 2026-10-03, after the first count of 59.

## The answers

The lead, 2026-10-03, yes to each recommendation, the fifth asked later:

1. A proposed relation (rule 4), a review (rule 8) and an unassigned task that became ready (rule 9) reach the triage agent of each of the task's scopes, each agent once, as a comment on a task already does.
2. A question asked of no one in particular asks the asker's own scope when the task is worked in it, else the scope the task was first worked in.
3. `fwd admin migrate` gives each task with a recorded `scope_id` and no `worked_in` relation of any status a `worked_in` to that scope.
4. `fwd task create` and `fwd task start` stop writing `scope_id`.
5. Start on the page starts a task worked in several scopes in its first scope, with no picker, since a start from any of its scopes writes nothing (put to the lead after the code was written; the lead agreed the same day and noted it on D136).
