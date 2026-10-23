# Using fwd: setting up a project and working in it

fwd exists so that the thread between people and coding agents is not lost: what is to be done and why, the comments people leave on particular lines of code, and the questions and answers that pass between them. It keeps all of that in one graph per project, rather than in chat transcripts and pull-request comments that go stale, and it routes each comment and question to the person or agent it concerns while they are still at work. Agents do everything through the `fwd` command; people use the same command or a page in the browser.

This primer covers starting a project and working in it day to day; why each piece is shaped as it is lives in `docs/design/` (`daemon.md`, `agents-and-inbox.md`, `data-model.md`).

## Before you start

`fwd` has to be on your `PATH`, including the one Claude Code runs its hooks with. In a fwdcentaur checkout, `scripts/build-web.sh` builds the page (it needs trunk and the `wasm32-unknown-unknown` target), then `cargo install --path crates/fwd` installs `fwd` with the page inside. The same binary is the daemon, started by the first command that needs it from that command's own executable; after installing a new build, run `fwd daemon stop` so the next command starts the new one.

Agents in Claude Code act as themselves through the fwd plugin, installed once per machine from a fwdcentaur checkout. The checkout stays where it is, since the plugin is installed from it: `claude plugin install` copies the plugin into Claude Code's plugin cache, and sessions run that copy, so after pulling a fwdcentaur that changes the plugin, run `scripts/refresh-plugin.sh` in the checkout: it refreshes the copy with `claude plugin update fwd@fwdcentaur` and then checks that the copy matches the checkout, naming each file that differs. A new session loads the new copy, and a running one at `/reload-plugins`. The plugin brings the three skills (`fwd:fwd-lead`, `fwd:fwd-triage`, `fwd:fwd-implementer`) and one hook, which stamps each command made only of `fwd` with the ids of the Claude Code session and agent that run it, and approves that command without a permission prompt ("Who is acting", below; `integrations/claude-code/README.md` says why, and how to check it works):

```sh
claude plugin marketplace add /path/to/fwdcentaur    # the checkout; its .claude-plugin/marketplace.json lists the plugin
claude plugin install fwd@fwdcentaur                 # for every project on this machine
```

The repository needs an `origin` remote. fwd knows a repository by its upstream, the host and path of that remote (`git@github.com:acme/widgets.git` and `https://github.com/acme/widgets.git` are both `github.com/acme/widgets`), never by a directory, so every clone and worktree of it belongs to the same project.

## Setting up a project

In a checkout of the repository:

```sh
fwd init --handle dnorman --display-name "Daniel Norman"
```

`fwd init` joins the project the repository already belongs to (the one a `.fwd/project.toml` marker at the checkout's root names, else the one this machine's index of projects lists for the repository's upstream), or creates one named after the directory (`--name` chooses another). It records the upstream in the index, writes the marker, stores your handle the first time (from `--handle`, else `$USER`) and whenever `--handle` is given, starts the daemon if none runs, and adds you to the project's graph as a person who belongs to the project. It makes no agent principal: each agent names itself and selects its role ("Who is acting", below). Its output, trimmed (JSON whenever it is piped):

```json
{
  "checkout": { "branch": "main", "upstream": "github.com/acme/widgets", ... },
  "daemon_port": 62817,
  "principal": { "handle": "dnorman", "id": "9t2DlJph..." },
  "project": { "created": true, "id": "c8e250be-a6b2-4c17-a0b3-cdaaa4134674", "name": "widgets" },
  ...
}
```

A project made before 2026-09-29 may have principals holding the old role `supervisor`: run `fwd admin migrate` there once, after a backup. Of those, the one that wrote most recently holds the role triage from then on, under its own name, serving no scope, so that nothing reaches it for its role until you, or its lead, bind it to its scopes (`fwd principal scope add <scope id> --for <its name>`) or endorse it for the whole project (`fwd principal scope project-wide --for <its name>`), which suits only a small project with small scopes; the others hold no role, named in the output for you to retire (`docs/design/cli.md`, "Maintenance").

Commit `.fwd/project.toml` if a fresh clone on another machine should find the project; on this machine the index already knows the upstream.

**Another clone, a worktree, another repository.** A second clone or a worktree of the same repository needs nothing: every command there finds the project through the upstream, whatever the branch, and `fwd init` there only reports `"created": false` and writes the marker. A different repository joins with `fwd init --project widgets` (the project's name or id), and `fwd project list` shows each project with the upstreams it claims. `fwd init --new` makes a separate project even when the upstream already belongs to one; from then on, a checkout of that repository without a marker has to name its project with `--project`. A repository without an `origin` remote gets a project that only its own marker names, and none of its lines can be commented on, so add the remote first.

One daemon per OS user hosts every project on the machine, each in its own database; a project created while the daemon runs opens without a restart.

## Who is acting

You are the person `fwd init` recorded. The handle is kept once per machine, so a second project needs no `--handle`. An agent names itself and selects its role before it acts, in one command, and acts as itself from then on:

```sh
fwd principal add impl-alice --role implementer   # lead, triage, implementer or reviewer
```

In Claude Code that is the fwd plugin's doing ("Before you start"). Its hook stamps each `fwd` command an agent runs with the ids of its session and of the agent (the subagent's own, inside a subagent); `principal add` binds the principal it makes to them, and every later command from that agent acts as it, with no `--as`. A `--as` naming anyone else is refused there, and `FWD_AS` is not read, since a settings file's `env` block reaches every agent of the session. An agent that has not named itself acts as no one: it is refused every write, and every read of its own inbox or tasks, with the command above. A principal from before the plugin is taken over once, from the session that acts as it, by saying its name twice: `fwd --as claude principal bind claude`. An agent relaunched to carry on another's job names itself as the other's successor in one step, `fwd principal add <name> --succeeds <the other's name>`: the other is retired, and its role (a lead's or a triage agent's included), its unread notifications and whatever reaches it from then on come to the new agent; a principal that exists already does the same with `fwd principal succeed <the other's name>` (`docs/design/agents-and-inbox.md`, "An agent's identity"). The hook stamps only a command made of `fwd`, `cd` and the filters `head`, `tail`, `grep`, `wc`, `jq`, `sort`, `uniq` and `cut`, with no `$`, no backtick and no redirection but `2>&1` and `/dev/null`. In a session whose agents are bound, a write in any other command is refused, saying so, unless `--as` is typed on it; and such a command with neither `--as` nor `FWD_AS` acts as no one, not as you, since any agent of the session may have run it, so its read of the caller's own inbox or tasks is refused the same way, while a read about the graph alone runs (task JFASz_E2): run fwd on its own, with text in single quotes, or type `--as <your name>` on the command. The hooks Claude Code runs itself carry no stamp either, but Claude Code hands each hook the ids of its session and agent on standard input, and a hook's command acts for the principal bound to the agent whose hook fired. Until that agent has named itself, a subagent's hook acts for no one, printing nothing, and the session's own thread's hook acts for the principal `FWD_AS` names, as the hooks did before (`integrations/claude-code/README.md`). Where nothing stamps a command (in a terminal, in a script, or for an agent that does not use the plugin, which is optional), `--as <name>` or `FWD_AS=<name>` in the environment acts as a principal, except that in a Claude Code session whose agents are bound only a typed `--as` lets a command write, since every agent of the session inherits `FWD_AS` (`docs/design/agents-and-inbox.md`, "Three ways in"):

```sh
export FWD_AS=impl-alice    # a script's principal
```

The roles are a fixed set. A triage agent serves scopes, so that each scope's questions have a front line of their own (D122): as it names itself it takes the scope `--scope` names, else the one its checkout finds, or with `--project-wide` the whole project, a deliberate endorsement that suits only a small project with small scopes; with neither it is refused, naming the two ways out. Among the principals not retired, a scope has at most one triage agent and a project one that serves it whole: a second is refused, naming the one that has it and how to free it. `fwd principal scope add <scope>` and `remove <scope>` give a triage agent another scope or take one away, and `fwd principal scope project-wide [--withdraw]` endorses it for the whole project, or no longer, each `--for <its name>` when its lead runs it; a triage agent that serves nothing, as those from before 2026-09-30 do until their lead binds them, hears nothing for its role, and each of its commands says so. It has a lead per session or worktree that coordinates its work, and a Claude Code session has one, its own thread: where the fwd plugin names a session's agents, a second agent of the session is refused lead, naming the session's lead, since the agents a lead dispatches select implementer, reviewer or triage. Each lead coordinates the project's work with you; each triage agent answers first for the scopes it serves, hearing the questions asked of no one about their work, the comments on their tasks and code, their proposals and reviews, and the one that serves the whole project hears what no scope's own agent hears; implementers work tasks in scopes; reviewers review. A name is only a name: no role is read from it, and `--as` with a name no principal of the project has is refused, with the command that makes it. Nor is a name a key: two principals may share one, and wherever a name is typed (`--as`, `FWD_AS`, `--to`, `--for`, `task assign`, `--decided-by`, the `principal` commands, an at-sign in a message) one that two or more principals of the project share is refused, listing each by the first eight characters of its id, with its role and the scopes its work is in. Each of those places takes the id too, whole or its first eight characters, so a principal whose name is shared still acts. So choose a name that no other principal of the project has (`fwd principal list --all` lists them; a role and a qualifier reads well, `implementer-replication`), and never mind other projects' names, which never meet this project's. A name that one principal has is that principal's alone: `fwd principal add` refuses it, saying whose it is and to choose another, unless you act as that principal (the one your agent is bound to, or, where nothing stamps the command, the one `--as` or `FWD_AS` names), when it brings a retired one back or changes its role. An agent with no role (one made before the roles were fixed) is refused its next write until it selects one, with `fwd principal role <name> <role>`, and a stored role counts as selected. Nothing checks who may use a name; this is local trust. `fwd whoami` shows whom a command acts as (`principal`, with its `handle`, `kind`, `agent_role` and whether it `belongs`), in which `project` and `checkout` (branch and upstream), and in which `scope`. For a Claude Code session, `integrations/claude-code/README.md` says how the plugin is installed and checked, and where the hooks go. `fwd whoami` also shows the command's `subject` when the plugin stamped it, and a `principal` of `null` while no principal is bound to it, and for a command the plugin did not stamp in a session whose agents are bound, with neither `--as` nor `FWD_AS`, saying why on standard error.

When an agent selects lead where no triage agent answers first for the scope it works in (none serves that scope, and none serves the whole project), and whenever a lead's inbox or a question asked of no one meets such a scope, `fwd` prints why and how to create a triage subagent for it, naming the scope (on standard error, or in a hook's context): the questions asked of no one, comments and proposals reach the triage agent of the scope they concern, and without one the questions wait and the rest fills a lead's context; so the lead spawns a background subagent on Sonnet (D129, "The triage agent is a Sonnet agent that annotates and relays"), briefed with the `fwd-triage` skill, in a checkout of its scope, which names itself and selects triage for that scope. Each lead keeps one for the scope it coordinates; a second for a scope is refused, naming the one that serves it, and stops.

The page offers, wherever it asks for a principal, only those who belong to the project: its people, and each agent from `fwd principal add` (a principal the project holds without belonging, such as a person a read brought in, belongs from its first command that writes there: a task, a message, a step, a relation, a decision, a scope, a claim). A command that only reads makes no one belong, so a session that looks at another project's graph is offered by none of its pickers. `docs/design/agents-and-inbox.md`, "Who belongs to a project", lists which commands write.

An agent's user lasts as long as its job, so that the pickers offer the agents at work (`docs/DECISIONS.md`, D123, "A subagent retires its own user when its job ends"): a subagent's last fwd command before its report retires it, `fwd principal retire <its name>`; resumed, it runs `fwd principal add <its name> --role <role>` again, which brings it back, and until then its writes are refused, the refusal naming that command, while its reads run; an agent relaunched in its place succeeds it. A subagent that only reports to its lead writes nothing to the graph and names itself not at all. The lead retires the users left behind by finished agents when it accepts their work or merges their branches, since nothing retires a user by time: fwd cannot tell a quiet agent from a gone one.

```sh
fwd principal list [--all]                    # who belongs here, and each one's role; an agent with no write for twelve hours comes last, marked dormant, until its next write; --all adds the rest, saying why each does not; a name two share shows each one's id and scopes
fwd principal role claude lead                # an agent selects another role
fwd principal retire impl-bob                 # finished: its work stays attributed to it, no picker offers it, and it writes nothing until it comes back
fwd --as impl-bob principal add impl-bob      # brings a retired agent back, as only the agent itself may (in Claude Code, the agent bound to it runs `fwd principal add impl-bob`)
fwd principal add triage-2 --succeeds triage-1  # a relaunched agent names itself as triage-1's successor: triage-1's role, scopes and mail are its own from now on
fwd principal succeed triage-1                # the same, run by an agent named already
```

`--to supervisor`, the supervisor role's word until 2026-09-29, is read this build as a question asked of no one in particular, with a line on standard error saying to leave `--to` off; the next build refuses it. `--as supervisor` is a name like any other, which a principal `fwd admin migrate` carried over from the role keeps.

### A Codex thread, over MCP

A harness that speaks MCP, such as Codex, uses the daemon's MCP endpoint instead of the CLI: the same commands as tools (`fwd task create` is `task_create`). In Codex, install the fwd plugin (`integrations/codex/README.md`), which runs `fwd mcp stdio` to reach the daemon and starts it when needed. A client can also name the endpoint at `http://127.0.0.1:<port>/mcp`, the port `fwd daemon status` gives, which moves when the daemon cannot have its old one; in Codex's `config.toml`:

```toml
[mcp_servers.fwd]
url = "http://127.0.0.1:<port>/mcp"
```

Codex asks the person before each fwd tool call, as its approval policy says. Preapproving them, `default_tools_approval_mode = "approve"` under the server, is an optional choice for threads that run unattended with `approval_policy = "never"`, where Codex otherwise refuses every MCP call; it approves every fwd tool, writes included, and is not part of setting fwd up.

A thread calls `project_list` to choose a project and passes it as `project` on every call, with `scope` where it works in one. It acts as itself: fwd reads which thread calls from the metadata Codex attaches to each call, and makes the thread's principal on its first write, an implementer named `codex-` and the end of its thread id, unless the thread names itself first, `principal_add <name> --role <role>` (or `succeeds`); `whoami` says who it is. No tool takes `--as`. A lead that worked by `--as` before is carried over to its thread by you, in a terminal, with the subject the thread's `whoami` gives: `fwd admin bind <its name> --subject codex:<session>/<thread>`, before the thread's first call that writes or reads its own inbox or tasks, even one that is refused; if one already made the thread a principal of its own, retire that one first (`fwd principal retire <its id>`), then bind. `docs/design/mcp.md` lists the tools and what is left out (the daemon's lifecycle, files in a checkout, the watch).

## Scopes

A scope is an area of work: what the work in it is trying to achieve, on which branches, and which tasks are worked in it, so people and the triage agent can see where the work is happening. Anyone works in any scope, and several people and agents may share one: a scope has no owner (D108, "A scope has no owner: scopes are not assigned to principals"). Who does a task is its assignee, not its scope ("Assignment and steps", below), and a comment on the code near a task reaches its assignee. An implementer works in the scope its work belongs in; for work no scope covers, it makes one in its own checkout, normally a worktree on its own branch:

```sh
git worktree add ../widgets-retry -b feat/retry && cd ../widgets-retry
export FWD_AS=impl-alice
fwd scope create "Reconnects that survive NAT drops" --intention "make the websocket client retry with backoff"
```

The scope records the checkout's repository and its current branch, or the branches given with `--branch` (repeatable). From then on, a command finds the scope from where it runs, whoever runs it: the scope bound to the checkout with `fwd scope attach-dir`, else the one active scope naming the checkout's branch. `--scope <id>` or `FWD_SCOPE` names one outright. Where several active scopes name the branch (a checkout of `main` where two scopes name `main`), the checkout is ambiguous: `fwd task start` and `fwd task next --claim` are refused with the scopes listed, and every other command runs with no scope, so bind the checkout to the one you work in. Until 2026-09-29 only the scope's owner found it, and anyone else in the same worktree got `"scope": null`.

```sh
fwd scope show <scope id>                   # the scope and the tasks worked in it
fwd scope attach-dir <scope id> [--clear]   # bind this checkout to the scope, whoever made it and whatever its branch, or unbind it
fwd scope branches <scope id> --add feat/retry-2
fwd scope status <scope id> paused          # planned, active, paused, blocked, done or abandoned
fwd scope rename <scope id> "<new name>"
fwd scope intention <scope id> --set "<what it is for>"   # or --append "<a paragraph more>"
```

The page's Scopes tab does the same: "New scope" makes one, made by whoever the page acts as, and each line's Edit changes its name, intention, repository, branches and status.

## The implementer's loop

Start an implementer's session inside the project's checkout (or set `FWD_PROJECT`, and install the hooks by hand), and have it arm a watch (`fwd watch --count 1 --timeout 7100` in the background, armed again after each `timeout` event, "Waiting for an answer" below) before it does anything else: otherwise the hooks never run, and a person's comment can sit unread for an hour (the Koinon pilot, 2026-09-26).

In the agent's own checkout, once it has named itself ("Who is acting", above), so that its hooks act for it:

1. `fwd prime` shows the principal, the project, the scope, up to ten ready tasks and the unread notifications. The session-start hook runs it as `fwd prime --format hook`:
   ```text
   fwdcentaur context for impl-alice: 1 ready task(s), 0 unread notification(s).
   Ready: gA3lqGfsNDEnWOu9bF6ATxx7FQt_vkOId03OD_HkEnE Retry on disconnect
   Unread:

   Use `fwd task next --claim` to take work, `fwd inbox pop` to read notifications.
   ```
   A task already started is no longer ready, so it is not listed; `fwd task list --mine` lists the tasks assigned to the agent. `fwd task show <task id>` prints one task with its checklist; `--include messages` adds its comments and questions, each thread under its first message, and `--include relations` what it points at and what points at it.
2. `fwd task next --claim` takes the highest-priority ready task (one already assigned to the agent first, then unassigned ones; one assigned to someone else never), starts it, makes the agent its assignee and works it in the scope the agent works in, in one step, and prints it with its `what`, `why`, `done_when`, `steps` and `messages`, `"status": "in_progress"` and `"claimed": true`. It passes over a task worked in a scope other than the one the agent works in, which waits for whoever works in that scope, and so does `fwd task next` without `--claim`. With nothing ready it prints `{"reason": "no ready task", "task": null}`; when the only ready tasks are worked in other scopes, the `reason` says so. Without a scope it starts the task all the same; in a checkout several active scopes name, it is refused, naming them.
3. Work, and write down what turns up. A task is filed open, or proposed when its filer is unsure it should exist; a proposed task reaches the triage agent of each of its scopes. A title states the assignment verb first ("Add jitter to the backoff", not "Backoff jitter"), so a list of tasks reads as a list of things to do (Daniel's convention):
   ```sh
   fwd task create "Add jitter to the backoff" --under <task id> --why "clients reconnecting together stampede the server"   # a subtask of this task
   fwd task create "Reuse the TLS session on reconnect" --from <task id> --label feature --why "a handshake per retry costs 300ms"   # other work found on the way
   fwd task create "Stop the retry loop spinning" --from <task id> --label bug --why "it retries with no wait when the daemon is down"   # a bug found on the way
   fwd task create "Retry over QUIC" --from <task id> --propose --why "might avoid the NAT drops altogether"   # work you are unsure should exist
   fwd comment <task id> "the first retry stays immediate"   # a note on the task; its assignee and the triage agent of each of its scopes hear
   ```
   File a task with its kind as a label, `bug` or `feature` by the project's convention (`docs/DECISIONS.md`, D97, "A task's kind, such as bug or feature, is a label, not a task type"), and any other word that helps find it (`--label bug,ui` gives two). `fwd task amend <task id> --label bug` adds a label later and `--unlabel bug` takes one off; labels are stored in lower case, so `Bug` and `bug` are one label. `--priority now`, `next` or `later` ranks it, in the words a question's priority takes (`fwd ask --priority later`): a task stores them as 1, 2 and 3, which `fwd task next` takes lower first and the page shows as P1 to P3. This build still takes a task's number from 0 to 4 as well, saying on standard error that the next build refuses it.

   Any principal moves a task into or out of Proposed (`fwd task approve <task id>`, `fwd task reject <task id> --reason "..."`, `fwd task rejustify <task id> --reason "..."`) and says why. A reject's and a rejustify's reason is kept as a comment on the task by the move itself, as a return's, a cancel's and a reopen's are; an approval takes no reason yet, so follow it with a comment on the task saying why, such as `fwd comment <task id> "marking open: Daniel said it was good"`.
4. Ask when someone else has to decide. `fwd ask --to @dnorman` is how a question reaches Daniel:
   ```sh
   fwd ask --about <task id> "Should the retry cap be configurable?"   # asked of no one in particular: the triage agent of the task's scope answers (of several, yours, else the first)
   fwd ask --to @dnorman --about <task id> "Is Windows in scope?"
   ```
   A question always notifies the party it was addressed to, even when that party asked it. The answer arrives as a `replied` notification; keep working meanwhile (see "Waiting for an answer").
5. Act on each notification. The hooks print new ones between the agent's steps and mark them read as they do:
   ```text
   You have 1 new fwdcentaur notification(s) as impl-alice:
   dnorman replied to your question on Retry on disconnect; reply with fwd reply z_1_4NVX (replied, id z_1_4NVX, priority next, 2m ago)
       30s is fine for now; keep it a named constant
   When you have acted on one, reply with `fwd reply <notification id> '<what you did>'`; to leave it, `fwd dismiss <id>`. ...
   ```
   Each notification opens with a sentence saying why it is in this inbox (the JSON carries it as `reason`), which for a question, a reply, a mention or a comment ends with how to answer it, then its type, the start of its id, which commands take, its priority and age, and the message behind it. Act, then `fwd reply <notification id> "<what you did>"`, which answers inside the thread of the message behind the notification, where `fwd comment` would start a thread of its own; or hand it on with `fwd forward <notification id> --note "<why>"`, to no one in particular, which the triage agent of the scope it concerns hears (`--to <name>` for someone in particular); or `fwd dismiss <notification id>`. Each of the three settles the notification, so that nothing prints it as unread again, a watch included ("Waiting for an answer", below): `reply` and `forward` mark it read in the transaction that posts their message, and name it in their JSON as `settled`; given the message's id instead, they settle nothing. `reply` and `forward` need a message behind the notification, so one about a task alone (`task_unblocked`, say) is acted on and dismissed. Without hooks, `fwd inbox pop` prints and marks them the same way, and `fwd inbox` lists them without marking.
6. `fwd task submit <task id> --evidence "commit 1a2b3c; cargo test green"` moves a task that needs review to `in_review` and tells the triage agent of each of its scopes, else the one that serves the whole project, which accepts or returns it only where a person's words or a recorded decision settle it, and otherwise brings it to its lead, which decides or has it reviewed (D129); with no triage agent to take it (none answers for its scopes, or the only ones that do submitted it), whoever approved the task, else its creator, is asked. Any other task goes straight to `done`. The submitter is never asked, so in a project with no triage agent a task the agent filed or approved itself asks nobody: the agent asks for the review, `fwd ask --to @dnorman --about <task id> "Review requested: <title>"`. Review stays with people and the leads; the submitter cannot accept its own submission.
7. When the agent tries to stop, the Stop hook runs `fwd session stop-check --format hook`, which prints `{"decision":"block","reason":"1 unread fwdcentaur notification(s) for impl-alice. ..."}` while anything is unread, and `{}` otherwise.

To look around: `fwd task list --ready`, `fwd task list --label bug` (the tasks with that label), `fwd task show <task id>`, `fwd task explain <task id>` (readiness and its reasons, relations, messages), `fwd graph` (every open task with its relations, and the active scopes), `fwd decision list` (every ruling, by number) and `fwd decision show D12` (one ruling, with what cites it).

## Assignment and steps

A task has at most one assignee, the person or agent doing it, and may have none while it waits for someone. Anyone assigns it, and the assignee hears of it; starting a task makes the starter its assignee, and a task assigned to someone is started by them alone:

```sh
fwd task assign <task id> @dnorman     # dnorman is told it is his; `fwd task unassign <task id>` leaves it with no one
fwd task list --mine                   # the tasks assigned to you
```

Comments on a task reach its assignee (and the triage agent of each of its scopes), and so does the news that it became ready. A scope is where the work happens, and a task may be worked in several (D136, "A task may be worked in several scopes"): `fwd link <task id> worked_in <scope id>` adds one, `fwd unlink <task id> worked_in <scope id> --reason "..."` takes one off, and `fwd task show` lists them under `scopes`. A start never moves a task out of the scopes someone chose for it, and goes through from any of them. Starting a task worked in no scope makes it worked in the scope the starter works in. Starting one worked only in other scopes is refused, and the refusal names each of them and how to switch to one: run the command again with `--scope <its id>`, or, for one that is active, bind the checkout to it with `fwd scope attach-dir <its id>`, whoever made it. A person needs no scope to start a task.

A task may carry a checklist of steps: lines of code to look at, a message, another task. A review is such a task: its author puts each highlight to read on the checklist with a comment on it, and the reviewer, its assignee, checks the steps off and finishes the task when satisfied. A highlight's step and the first thread on the highlight, the one its first comment started, move together: checking the step resolves that thread, resolving the thread checks the step (`fwd message resolve`, or Resolve and Check the Step in VS Code), and unchecking the step, or setting the thread back to active (`fwd message unresolve`), undoes both; later threads and replies move alone.

```sh
fwd step add <task id> --path src/net/ws.rs --lines 120-134 "this mutex serializes X and Y"   # a highlight and its first comment
fwd step add <task id> <message or task id>   # anything else as the next step
fwd step list <task id>                       # [x] 1. src/net/ws.rs:120-134 2418c54 `fn reconnect() {`, thread resolved
fwd step check <task id> 1                    # by its number, or the id of its relation or of what it is; resolves its thread too; `fwd step uncheck` undoes both
fwd step remove <task id> <highlight id> --reason "moved to its own review" --retire   # takes it off the checklist; --retire resolves its threads too
```

`fwd task show <task id>` prints the checklist after the task's fields, with how much of it is checked ("steps: 3 of 8 checked"). A comment on a highlight that is a step reaches the task's creator and its assignee, saying whose step it is, and so does the step itself. What one author does on one task (steps added, comments on it or on its steps) reaches each of them as one notification while they leave it unread, which says how many of what and lists them ("impl-x added 12 steps and commented 12 times on your task Review: the retry loop"); once they read it, the author's next step starts another.

Check a step off when its work is done, or its lines read. Take it off the checklist when it no longer belongs there, because its work moved to other tasks or it was added by mistake, so that the checklist does not record as done what nobody did (task 9dvGW0EI, "Take a step off a checklist and retire its highlight in one command: fwd step remove"): `fwd step remove` retracts the step with the reason, and the thing it was stays. `--retire` also resolves every thread on a highlight, so the editor stops drawing lines no one is talking about any more, while `fwd anchor show` and the highlight's page still list them; resolving its first thread checks the highlight's steps on other checklists, and the command lists any it checked, so leave `--retire` out for a highlight another task still holds. Only the task's creator and its assignee take a step off, on the page too, where each step's line has a remove control for them; no one is told. The command prints the checklist that is left; the steps after the one removed move up a number, so name steps by id when taking off several.

In any message, `@handle` names a person or an agent (a handle two or more principals of the project share is refused, listing them: `@` and the first eight characters of one's id names that one), and `@<id or its first six characters>`, `[[<the same>]]` or `[[D12]]` names anything; the person named hears of it, and `fwd message show` prints each name in place (`@dnorman`, `[Retry on disconnect]`). A name that fits nothing is refused when you post, so a typo never goes out; put text in backticks to keep an `@` or brackets as they are. `fwd comment --help` (and `reply`, `ask` and `forward`) says the same, with an example.

To point a reader at a whole file as you saw it, name it by its path from the repository's root in double brackets (never after an at-sign, which names people, agents and ids): `[[docs/using-fwd.md]]` is the file as it stands on disk, unstaged changes included; `[[docs/using-fwd.md@index]]` its staged copy; `[[docs/using-fwd.md@HEAD]]` the file at a commit, which a sha, a branch or a tag names as well, and which is kept as the commit it named when you posted, so it does not move with `HEAD`. A path needs a slash or a dot to be told from an id, so a file at the root with neither is `[[./Makefile]]`. The file is read in the checkout the command runs in, and a file that is not there at that version is refused, naming it; write it with the CLI or in a comment in VS Code's editor, since the page has no checkout to read it in. It shows as a lozenge naming the path and the version, `docs/using-fwd.md on disk`, `docs/using-fwd.md index` or `docs/using-fwd.md e13b8e2`; on the page a click goes to its whole-file highlight, and in VS Code it opens the file at that version (`docs/design/cli.md`, "Messages").

Naming a highlight in a message attaches it beneath the message, so a reply can point at code: on the page the message shows each highlight it names under its text, by its first line and where it is, with an expander that opens its lines and its threads in place, and a click on it goes to the highlight's page (in VS Code, to the lines in the editor). A task, a decision or a person named stays a name in the text, and so does a whole file named at a version (above). Make the highlight, then name it:

```sh
fwd anchor create --path src/net/ws.rs --lines 120-134    # prints the highlight, with its id
fwd reply <message id> "the lock is taken twice, here: [[<the highlight's id>]]"
```

To correct a message you sent, edit it where it stands rather than sending a second one: `fwd message edit <message id> "<the whole new text>"`. It keeps its id and its place in its thread, and shows that it was edited. Only its author may edit it, and an agent that succeeded its author (`fwd principal add --succeeds`), so that an agent relaunched to carry on another's job corrects what its predecessor wrote; the message then shows which agent edited it. Whoever the new text names and the old one did not hears of it, and no one else is told.

## Waiting for an answer

An agent never blocks its turn on the inbox, and never polls it, so that it costs no tokens until a notification arrives, but for arming its watch again every two hours (below; `docs/DECISIONS.md`, D130, "No agent polls: fwd costs no tokens until a notification arrives"): no timer, no loop, no `fwd inbox` run again and again. Answers normally arrive through the hooks: `fwd inbox pop --format hook` runs on the hook events `integrations/claude-code/settings.example.json` wires (each prompt, and each batch of tool calls) and puts whatever arrived into the agent's context between its steps, and the Stop hook refuses to let the session stop while unread notifications remain.

When an agent is truly blocked, having asked a question it cannot proceed without, it starts one watch as a background command, which is the whole of its waiting, and keeps working on anything else:

```sh
fwd watch --count 1 --timeout 7100 --format json
```

In Claude Code that is a Bash call with `run_in_background` and a `timeout` of 7200000, and the command's exit wakes the agent. The harness ends a background command after 30 minutes by default and after two hours (7,200,000 ms) at most, so the watch is given `--timeout 7100`, which ends it with `{"event":"timeout"}` and exit 0 a little before the harness would stop it, and the agent arms it again: the new watch prints first whatever arrived meanwhile, so nothing is missed, and the wait costs one short turn every two hours (the Ankurah lead verified the harness's limits on 2026-10-03; until then this said the watch had no timeout, as holds for `fwd watch` without `--timeout`). Otherwise the agent is woken only when there is something to read: once the answer arrives, the output holds the `notification` line with the message, then `{"count":1,"event":"done"}`, and the watch exits 0. A notification already unread when the watch starts is printed first as a `snapshot` line and counts too, because the hooks mark read whatever the agent has already seen, so anything still unread arrived after its last step; with such a line waiting, the watch prints it and `done` at once. A daemon restart neither ends the watch nor shows in its output: the watch reconnects by itself, on the daemon's new port if it moved, and prints what arrived meanwhile. One more end is a daemon out of reach for ten minutes: the watch prints `{"event":"unreachable","since":<ms>,"reason":"..."}` and exits 7, and the agent may run it again. A script that must bound its wait gives `--timeout <seconds>` too, which ends the watch once that many seconds have passed, with `{"event":"timeout"}` and exit 0. The JSON lines are on standard output; Ankurah may log warnings on standard error. The watch marks nothing read: a notification stays unread until a hook shows it, `fwd reply <notification id>` or `fwd forward <notification id>` acts on it, or `fwd dismiss` or `fwd read` settles it, and a watch armed while one is unread prints it again at once.

## The triage agent's loop

A triage agent answers first for the scopes it serves, or, endorsed, for the whole project: the questions asked of no one in particular about their work reach it (`fwd ask` without `--to`, the page's Ask box by default), and so does what reached the supervisors before 2026-09-29, by the scope it concerns: comments on tasks and on code, proposals, reviews, unblocked tasks nobody holds (`docs/design/agents-and-inbox.md`, "Roles"). It is usually a background subagent on Sonnet that a lead spawns, briefed with the `fwd-triage` skill, in a checkout of the lead's scope; it names itself and selects its role first (`fwd principal add triage-bot --role triage`), which takes the scope its checkout finds, or the one `--scope <id>` names, or with `--project-wide`, in a small project with small scopes, the whole project, then acts as that name. It takes no tasks, and a scope it serves stays no one's. It annotates and relays what reaches it rather than judging the work (D129): it decides a proposal or a review only where a person's words or a recorded decision plainly settle it, saying which, and brings the rest to its lead with what it found. It talks to its lead and the agents its lead runs by Claude message, and keeps fwd for its watch, its replies and its actions on the graph (D131, "A triage agent talks to reachable agents by Claude messaging; fwd is for its watch, its replies and its actions"). Its commands act as it by the plugin's stamp. A triage agent run as a session of its own, rather than as a subagent, has the hooks too, which act for it once it has named itself, and a detached worktree of its own keeps those settings apart (`git worktree add --detach ../widgets-triage`); it finds no scope there, so it names its scope as it names itself, `--scope <id>`.

1. `fwd inbox pop` prints the unread notifications and marks them read. Each has a `type`, a `target` (the message, task, relation, code anchor or scope it is about) and, when a message is behind it, the `message` (trimmed):
   ```json
   [
     { "id": "RQY4tLFZ...", "type": "question_asked", "priority": "now",
       "target": { "type": "Message", "id": "eJHlQbv-..." },
       "message": { "content": "Should the retry cap be configurable, or is 30s fine?", "task": "gA3lqGfs...", ... } },
     { "id": "mQ1BTD6x...", "type": "task_proposed", "target": { "type": "Task", "id": "BFzDXcBv..." }, ... }
   ]
   ```
2. It settles each one. The `fwd-triage` skill says which command fits which notification, and the caps per pass:
   ```sh
   fwd reply <notification id> "D12 caps the retry delay at 30s"   # an answer the graph records, in the question's thread
   fwd reply <notification id> "From impl-alice: the cap is a named constant"   # an answer it asked an agent for by Claude message, once, saying whose
   fwd message resolve <message id>                             # the question is settled
   fwd forward <notification id> --to @dnorman --note "a product call"   # what only a person can answer (a scope has no owner to forward to)
   fwd link <message id> concerns <task id>                     # this comment is about that task
   fwd link <task id> anchored_at <anchor id>                   # the task's code: later comments near those lines reach its implementer
   fwd task amend <task id> --why-append "..."                  # or --done-when-append "..."
   fwd task create "<title>" --from <message id> --propose --why "..."   # new work from a comment; its author is told it exists
   fwd task approve <task id>                                   # or: fwd task reject <task id> --reason "...", where a person's words or a ruling settle it
   fwd comment <task id> "marking open: it is the retry work Daniel asked for"   # which words or ruling moved a task into or out of Proposed
   fwd relation approve <relation id>                           # or: fwd relation reject <relation id> --reason "...", as for a task
   fwd task accept <task id> --note "..."                       # or: fwd task return <task id> --reason "..." (a review a person's words or a ruling settle)
   fwd decision accept D12 --decided-by @dnorman --note "..."   # a proposed ruling the person gave, in their words; or: fwd decision reject D12 ...
   fwd cite <task id> D12                                       # the task rests on ruling D12: a repeal or supersession reaches its author
   fwd ask --to @dnorman --about <task id> "Review requested: can you review it?"   # when its lead says to ask the person
   fwd dismiss <notification id>
   ```
   A notification about a task or a relation alone (`task_proposed`, `review_requested`, `task_unblocked`) has no message behind it, and `reply` and `forward` refuse it (`that notification has no message behind it`), so the triage agent acts on it and dismisses it; a `task_proposed` notification's `target.type` says whether a task or a relation awaits a decision. Every proposal and review request reaches the triage agent first, which judges no evidence, design or implementation: it approves, rejects, accepts or returns only where a person's words or a recorded decision plainly settle it, saying which, and otherwise brings it to its lead by Claude message, with what it found (a task a proposal duplicates, the tasks related to it, the evidence a submission names), and dismisses the notification; the lead decides, has the work reviewed, or tells it to ask the person. `fwd ask --to @dnorman` is how a question reaches Daniel, and a question always notifies the party it was addressed to.
3. It waits for the next batch with one watch, as an agent waits for an answer ("Waiting for an answer", above), armed again after each pass and after each `timeout` event, and never on a timer or in a loop.

## Decisions

A decision records a ruling, usually your person's, so that it has a name people cite (`D12`) and so that when it is repealed or replaced, everything resting on it is found and its authors are told. Product, process, UX, naming and dependency rulings are recorded the same way as architecture ones; `--area` says which (any word). A decision is never a task with a flag: a task that waits on a ruling cites it.

Record the answers that rule beyond the task at hand: how something is to be done from now on, which later work will rest on and cite. An answer that settles one task, one question or one branch's review stays where it was given, in that task's or that question's thread, where whoever picks the work up reads it; recorded as a decision, it would read as a rule for everyone.

How far a ruling reaches is the scope it is attached to (D93, "A decision may be attached to the scope it governs; one attached to none governs the whole project, and its area is its type"). A ruling given for one scope's work, such as the work of the branch under review or the page's polish, is attached to that scope: `--attach <scope>` as you record it, or `fwd decision attach D12 <scope>` afterwards, `<scope>` being its title, its id or the start of its id. A ruling attached to no scope governs the whole project, which is the default, so leave `--attach` off only for a ruling that holds for all of the project's work. The global `--scope` is the scope your session works in, not the scope a ruling governs: a ruling you record while working in a scope is attached to it only when you say so. In one scope's context the rulings that apply are the project-wide ones and that scope's own: `fwd decision list --attached <scope> --project-wide` prints them, and the Decisions page opens on your scope with the project-wide rulings listed too. `fwd decision detach D12` makes an attached ruling project-wide again, when it turns out to hold everywhere; neither attaching nor detaching tells anyone, since the ruling itself is unchanged. The area says what kind of ruling it is; the scope says how far it reaches.

When your person gives you a ruling, record it at once, in their words:

```sh
fwd decision create "Retries back off exponentially" --area architecture \
    --decision "back off exponentially, capped at 30s" --decided-by @dnorman --decided 2026-09-26 \
    --context "the question about the retry cap" --from <the question or task it answered>
```

It is accepted at once and decided by them, with you recorded as its filer; `--decided` is the day they gave it (today when left out), and `--from` joins it to the question or task it answered, whose row on the page then names it ("decided here").

When you need a ruling you cannot give, propose one: the same command with `--propose` instead of `--decided-by` and `--decided`. The proposal reaches the person and the triage agent of the scope it governs, else the one that serves the whole project, who accept or reject it with a note. When the person gives you their answer to a proposal, record it on the proposal rather than filing a new decision: `fwd decision accept D78 --decided-by @dnorman --note "<their words>"` (or `reject`) makes them its decider, and the note is your comment on it.

When your work rests on a ruling, cite it from the task or the message, so that a repeal or a supersession of it reaches you with the reason:

```sh
fwd cite <task id> D12    # or a message id
```

The moves are `fwd decision accept D12 --note "..."` and `fwd decision reject D12 --note "..."` for a proposal, `fwd decision supersede D12 D15 --note "..."` to replace a ruling in force with another accepted one, and `fwd decision repeal D12 --note "..."` to withdraw one with nothing in its place. Each needs `--note`, which is kept as a comment on the decision. A rejected, superseded or repealed decision stays so; a new decision takes its place.

`fwd decision list [--status accepted] [--area architecture] [--attached <scope>] [--project-wide]` prints each decision as its number, status, area and title, and the scope it governs when it is attached to one, and `fwd decision show D12` prints one with the scope it governs, what cites it, what it cites, what superseded it and its comments. `D12` works wherever an id does: `fwd comment D12 "..."`, `fwd ask --to @dnorman --about D12 "..."`.

The notifications a decision brings, and what to do on each:
- `decision_proposed` (the triage agent of the scope it governs, else the one that serves the whole project, and the people): a proposal waits for a ruling. Accept or reject it with a note when it is yours to settle; otherwise ask the person, `fwd ask --to @dnorman --about D12 "..."`.
- `decision_commented`: someone commented on a decision you filed or ruled on; the reason for a move arrives this way. Answer if it asks you something.
- `decision_repealed` and `decision_superseded`: a ruling that something of yours cites no longer stands, and the notification carries the reason. Re-justify or amend what cites it (`fwd task amend <task id> --why-append "..."`; after a supersession, usually `fwd cite <task id> D15` for the new ruling), say so in a comment on it (`fwd comment <task id> "D12 is repealed; the cap still holds because ..."`), then dismiss the notification.

The whole grammar is in `docs/design/cli.md`, "Decisions".

## The page

```sh
fwd ui                   # opens http://127.0.0.1:<port>/?p=<project id> in the browser
fwd ui --no-open         # prints that URL instead
fwd --as impl-alice ui   # the same page, acting as impl-alice (&as=<its id>), who belongs to the project from then on
fwd ui --lan             # a URL a phone on the same Wi-Fi can open
```

The page is where a person reads and answers: Inbox (their notifications, worked from the keyboard, with an Ask box for questions about no particular task), Questions (the open questions asked of them, grouped by the priority each wants its answer at), Tasks (each with its labels after its title, and a filter by label; a task's panel adds and takes off its labels), Decisions (every ruling, by number), Graph and Scopes. Its header picks the acting principal, as `--as` does, and the project; to read another principal's inbox or questions without acting as them, a selector beside the Inbox's and the Questions page's headings shows one other principal's at a time (task jPQy9aQC, "Open the Questions and Inbox pages on your own, with a selector to look at another principal's"; `docs/DECISIONS.md`, D104, "The Questions and Inbox pages open on the viewer's own, and show one other principal's at a time only to look"). In a terminal the person uses `fwd inbox`, `fwd reply`, `fwd task approve <task id>` and `fwd task accept <task id>`. After `--lan` the daemon listens on the network until `fwd daemon start --loopback`, and until authentication exists every device on that network can read and write every graph.

## Backups

The data directory ("Where things live", below) holds the only copy of every project's graph on this machine. `fwd admin backup` copies it into a new dated folder while the daemon runs:

```sh
fwd admin backup                                         # into ~/Library/Application Support/fwd-backups/, keeping the newest 7
fwd admin backup --to ~/Dropbox/fwd-backups --keep 30    # any folder, a synced one included
```

Each backup is a folder named for when it was taken (`2026-09-28T130000-0700`) holding every project's databases, copied through SQLite so each holds what was committed at one moment, `projects.toml`, `identity.toml`, `daemon.toml`, each project's `checkouts.json`, and `backup.json`, which lists every file with its size and SHA-256. Every database copied must pass SQLite's integrity check, or the backup is not kept. The command prints where it wrote and each project with its files; then it removes the oldest backups of this data directory taken on this machine beyond `--keep`, and nothing else in the folder, so several machines can back up into one synced folder.

To take one every day at 13:00, add this line with `crontab -e`. It names `fwd` by where `cargo install` puts it, since cron's `PATH` leaves that folder out:

```sh
0 13 * * * "$HOME/.cargo/bin/fwd" admin backup --format text >> "$HOME/Library/Logs/fwd-backup.log" 2>&1
```

cron skips a run while the Mac sleeps; a launchd agent with `StartCalendarInterval` and the same command runs it when the Mac wakes.

To restore, first stop the agent sessions that use `fwd`, since their next command starts the daemon again; then stop the daemon and move the data directory aside. A restore writes only into an empty or new folder, so it never overwrites a graph, and the old directory stays until you delete it:

```sh
fwd daemon stop
mv ~/Library/Application\ Support/fwd ~/Library/Application\ Support/fwd.before-restore
fwd admin restore ~/Library/Application\ Support/fwd-backups/2026-09-28T130000-0700
```

The restore checks every file against `backup.json` before it writes anything, and each database it wrote with SQLite's integrity check; it refuses while a daemon serves the data directory. The next `fwd` command starts the daemon on the restored graphs.

To look at a backup without touching your graphs, restore it into a new folder and give it a daemon of its own. With only `FWD_DATA_DIR` set, `fwd` would reach your running daemon, which serves your graphs under the same project ids:

```sh
fwd admin restore <backup folder> --into /tmp/fwd-restored
export FWD_DATA_DIR=/tmp/fwd-restored FWD_RUNTIME_DIR=/tmp/fwd-restored.run FWD_DEV_RUNNER_DIR=/tmp/fwd-restored.dev-runner
fwd --project widgets task list    # starts a daemon on the restored graphs
fwd daemon stop                    # stops that daemon, not yours
```

## Where things live

| What | Where |
|---|---|
| Data directory | `~/Library/Application Support/fwd/` on macOS; `$XDG_DATA_HOME/fwd/`, usually `~/.local/share/fwd/`, on Linux. `FWD_DATA_DIR` overrides it. |
| In it | `projects.toml` (the index: each project and the upstreams it claims), `identity.toml` (your handle), `daemon.toml` (loopback or LAN, and the last port), `daemon.log`, and per project `projects/<project id>/project.sqlite` (the graph) with `checkouts.json` (the clones and worktrees the daemon has seen; `fwd checkouts` lists them). |
| Backups | Beside the data directory and named after it: `~/Library/Application Support/fwd-backups/` on macOS, one dated folder per `fwd admin backup` ("Backups", above). `--to` names another. |
| Runtime directory | `$TMPDIR/fwd-<uid>/` (`/tmp/fwd-<uid>/` when `TMPDIR` is unset). `FWD_RUNTIME_DIR` overrides it. It holds `daemon.ws` (the running daemon's port, pid, instance, start time and bind) and the lock files. |
| The repository | `.fwd/project.toml`, the marker naming the project. |
| Sutra's registry | `~/.dev-runner/`, the daemon's card on Daniel's dashboard. `FWD_DEV_RUNNER_DIR` overrides it. |

The daemon runs until `fwd daemon stop`; it has no idle exit yet. `fwd daemon status` reports on it and never starts one:

```json
{ "bind": "loopback", "instance_id": "cbd8afdf-...", "pid": 87500, "port": 62817, "started": 1790465340061,
  "project": "c8e250be-...", "projects": [ { "id": "c8e250be-...", "name": "widgets" } ] }
```

`project` is the current directory's project, and a project that failed to open carries an `error` in `projects`. With no daemon it prints `no daemon is answering` and exits 7.

To try something without touching the real graphs, point the three directories elsewhere first; the commands then start and use a daemon of their own, which `fwd daemon stop` in the same shell stops:

```sh
T=$(mktemp -d)
export FWD_DATA_DIR=$T/data FWD_RUNTIME_DIR=$T/run FWD_DEV_RUNNER_DIR=$T/dev-runner
```

## When something is off

- Run `fwd whoami` first. It shows the principal (the one bound to the `subject` the plugin stamped; otherwise, is `FWD_AS` what you meant?), the project, the checkout's branch and the scope; `"scope": null` means no active scope bound to this checkout, and no active scope, or several, naming its branch.
- `no project for <dir>: run fwd init in the repository, or pass --project <name|id>` means nothing on this machine claims the checkout's upstream.
- `could not write projects.toml in the data directory <dir>: this command was joining the checkout's upstream ...` means the checkout's marker names a project that this machine's index did not yet list for the checkout's upstream, and the command could not record it there; the cause beneath it says which file refused. Run one command in the checkout where it may write the data directory (a sandbox may forbid it); after that, commands there only read the index.
- `fwd daemon status` says whether the daemon runs and which projects opened. `fwd daemon logs` prints the log's path, so `tail -n 50 "$(fwd daemon logs)"` shows its end. `fwd daemon stop`, then any command, restarts the daemon.
- `the connection to the daemon on port <port> closed while this command ran ...; run the command again`, with exit 7, means the daemon stopped while the command waited on it (`fwd daemon stop`, the restart after installing a new build, `fwd daemon start --lan`). The command waited up to ten seconds for the daemon to come back on the same port, and stopped waiting at once when it came back on another. Run the command again, which joins the daemon wherever it listens now; a command that writes ends with this error only when the daemon went before it began to write, and with the one below once it had begun. Only `fwd watch` follows the daemon by itself ("Waiting for an answer").
- `the connection to the daemon on port <port> closed while this command ran, so what it wrote may not have reached the daemon; look (fwd task show, fwd message show) before writing it again`, with exit 7, comes from a command that writes, whose socket to the daemon closed at some moment while it ran, even when it went on to print what it made: a write committed while the socket was closed went to the command's own node alone, which forgot it when the command exited, while one committed before the close may have reached the daemon. What it printed is what it wrote to its own node. Look (`fwd task show`, `fwd message show`, or the id it printed) and write again only what is missing. `closed before this command began to write, and it wrote nothing` means just that: run it again. `fwd inbox pop` ends as a read does instead, since a mark that reached no daemon only brings the same notifications back at the next pop.
- Someone did not hear a comment: a triage agent that took a scope after a comment in it was written never receives it, while a question asked of no one waits on the Questions page, listed under its scope, until an agent comes to answer for that scope, and reaches it then; a triage agent that serves nothing hears nothing for its role, which each of its commands says; an implementer hears a comment on code only when a task assigned to it is anchored within 50 lines of it, or anywhere in the file when the comment or the task is on the whole file (`fwd link <task id> anchored_at <anchor id>`), or when the code is a step of a task it created or is assigned; a comment on a task reaches the task's assignee only; and `@handle` reaches its person only in a message posted since 2026-09-28.
- Someone is missing from a picker on the page: it offers only the principals that belong to the project (`fwd principal list --all` says why one does not); `fwd principal add <name>` makes an agent belong.
- In Claude Code with the plugin, a write refused because the agent has not named itself: `fwd principal add <name> --role <role>`, or, for a principal from before the plugin that no agent is bound to, `fwd --as <name> principal bind <name>`. A `--as` refused as naming someone other than your agent's principal: leave it off, since the command acts as you. A write refused as unstamped: the command held something besides `fwd`, `cd` and the filters (a `$`, a redirection to a file, another program), so run fwd on its own, or type `--as <your name>` on the command, which `FWD_AS` does not stand in for there.
- `--as <name>` is refused: no principal of the project has that name; the agent names itself first, `fwd principal add <name> --role <role>`. Or two or more principals of the project share the name: the refusal lists them by id, with their roles and scopes, and `--as <id>` (or `FWD_AS=<id>`; the first eight characters are enough) acts as the one meant. A write is refused for an agent with no role: `fwd principal role <name> <role>`.
- Any command takes an id's first six characters or more instead of the whole id (`fwd task show gA3lqGfs`), and `D12` for decision 12; a prefix that several entities share is refused with each of them listed.
- Ids are long base64 strings, and about one in 64 starts with `-`; commands take such ids as they are, and a message may start with `-` too. `fwd read` and `fwd dismiss` take several ids, so their flags go first: `fwd dismiss --format json <id> <id>`.
- A wrong value is refused without a list of the right ones: `scope status` takes planned, active, paused, blocked, done or abandoned; `fwd capabilities` lists the relation kinds.
- `--format text` is meant for people; agents read the JSON, which `fwd` prints whenever its output is piped. A command that makes something, a task, a message, a step or anything else, gives it under `record` (`record.id` is its id); this build also gives its fields beside `record`, as fwd printed them before, and the next build drops them.
