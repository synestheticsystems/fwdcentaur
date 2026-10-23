# The daemon's MCP endpoint (`/mcp`)

Task jDv1EY7K, "Local MCP: serve shared fwd operations from the daemon without authentication", in the Connections scope. The endpoint lets a Codex thread, or any MCP client, work in the task graph without a shell. It is the CLI by another door, not a second fwd API: each tool is a CLI command, parsed by the CLI's own parser and run by the CLI's own dispatcher, so the two cannot drift (D124: fwd documents itself through its CLI and MCP).

## Transport

- Streamable HTTP at `POST /mcp` on the daemon's own listener (`daemon.md`, "Runtime directory and listeners"), served by rmcp, the official Rust SDK. It is stateless: each POST is answered with JSON, so a daemon restart costs a client only the call in flight.
- The endpoint answers only a process on this machine that is not a web page, the rule that already covers the daemon's control routes: the peer must be on loopback and the request must carry no `Origin`. A LAN bind therefore does not expose it, and no page, whether the daemon's own, another site's or a DNS-rebinding page, can call it. The daemon's `Host` check applies as it does to every route.
- There is no login. Local access works under the same local trust as `fwd --as`.

Codex reaches it through the fwd plugin (`integrations/codex/README.md`), which starts `fwd mcp stdio`: a relay that posts each message from Codex's standard input to the daemon's `/mcp` byte for byte, so `_meta` arrives untouched, writes one answer line per request, finds the daemon as every CLI command does and starts it when none runs (`crates/fwd/src/mcp_stdio.rs`). The relay acts as no one; the daemon reads each call's caller. A client can instead name the endpoint itself, with the port `fwd daemon status` gives; the daemon reuses that port across restarts when it is free, but it is not fixed, and such a client cannot start the daemon.

```toml
[mcp_servers.fwd]
url = "http://127.0.0.1:<port>/mcp"
```

Codex asks the person before each fwd tool call, as its approval policy says. Preapproving them, `default_tools_approval_mode = "approve"` under the server, is an optional choice for threads that run unattended with `approval_policy = "never"`, where Codex otherwise refuses every MCP call; it approves every fwd tool, writes included, and is not part of setting fwd up.

## Who calls

No tool takes an actor, and no argument names whom to act as. The transport supplies the caller, and the operations take the caller as given (`crates/fwd/src/mcp.rs`, `Operations::call`). That way a hosted endpoint can supply an authenticated caller later without changing the operations.

- **Codex threads.** Every tool call a Codex model makes carries the calling thread's identity in `params._meta`: `threadId` and `sessionId`, plus `x-codex-turn-metadata`, whose `thread_id` and `session_id` must be the same two ids. `x-codex-turn-metadata` must be an object, the shape every model-made call had over stdio and over Streamable HTTP (`identity-report.md`, `http-identity-report.md`). A JSON string, the shape the app-server's direct `mcpServer/tool/call` forwards from its client, is refused. This is not a security boundary, since a forging process can send an object as easily, but no call a model makes takes the string shape. The caller's subject is `codex:<sessionId>/<threadId>` (`fwd_core::subject::codex`). The research behind this is in `/private/tmp/fwd-connections-active/identity-report.md`: a thread at the root of its session tree, a fork included, has its own id as its session, and a subagent shares its root's session. Values the model puts in its arguments never reach `_meta`.
- **Refused.** A call is refused, and nothing runs, when its `_meta` lacks the ids or the turn metadata, or when the two disagree. The refusal is a tool error the model reads. Codex's app-server method `mcpServer/tool/call` sends only `threadId`, so the endpoint refuses it (checked against Codex 0.159.2).
- **Not a security boundary.** A process running as the same user can send any metadata it likes. The check keeps the model, and accidental callers, from choosing whom they act as.

### The principal a thread acts as

- **When it is made.** A thread's principal is made on its first call that needs an actor: a write, or a read of the caller's own inbox or tasks (`inbox`, `inbox_pop`, `prime`, `task_next`, `task_list --mine`). Discovery (`project_list`) and `whoami` never make one, and neither do reads about the graph alone.
- **Name and role.** The principal is made as `fwd principal add` makes one, bound to the subject. Its name is `codex-` followed by the last 8 letters and digits of the thread id; that part of a Codex id is random, while the start is a timestamp that threads spawned together share. On a clash, 12, then 16, then all of them. Its role is implementer, whatever the thread's place in its session tree, since a top-level Codex thread is as often a worker or a reviewer as a lead (lead-codex's ruling, 2026-10-09). A thread selects another role with `principal_role`, or renames itself with `principal_rename`. An existing principal keeps its role.
- **Naming itself first.** A thread may call `principal_add` before it writes anything, with a name and role, or with `succeeds` to carry on another agent's job. It is then made as it says, bound to it, as a Claude Code agent names itself, and no principal is made for it first. `principal_succeed` from a thread with no principal is refused as the CLI refuses it, making nothing.
- **Concurrent first calls.** They make one principal, the automatic ones and several first `principal_add`s alike. The daemon makes principals under one lock and looks again before it makes one.
- **One thread, one principal.** A thread bound under one session that calls under another is refused, naming the principal, rather than made a second principal with its inbox split. Codex 0.159.2 keeps a thread's session across resumes; this guards a build or path that would not. No command moves a principal to another session: a person who retires the old principal lets the thread's next call make it anew under the new session, the old one keeping its work.
- **Resume.** A resumed thread keeps its ids, so it keeps its principal. A retired principal's writes are refused until `principal_add`, run as that principal, brings it back, as in the CLI.
- **Leads from before.** A lead that worked by `--as`, from before automatic attachment (lead-codex), is carried over to its thread by the person, in a terminal: `fwd admin bind <name> --subject codex:<session>/<thread>`. The thread's `whoami` gives the subject. Bind it before the thread's first call that writes or reads its own inbox or tasks, even one that is refused, since that call makes the thread a principal of its own. The bind is narrow: it takes only an agent that holds lead, is at work, was not succeeded and is bound to no thread, and only a Codex subject no principal at work is bound to. If the thread has a principal at work, the bind is refused, naming it, and nothing is retired or replaced: the person retires that one first (`fwd principal retire <its id>`) if it is not the thread's, and the bind then unbinds it as it binds the lead, its work staying attributed to it. A thread never takes over another principal itself: `principal_bind` is not a tool, and `fwd admin bind` refuses a command a Claude Code agent runs.
- **Sessions.** One-lead-per-session (D119) reads Codex sessions as it reads Claude Code sessions, keeping the two harnesses' sessions apart (`subject::same_session`).

## Project and scope

Every project tool takes `project`: a project's name or its id, as `project_list` gives them. Nothing binds a project to a transport session. The optional `scope` is the CLI's `--scope` typed on the command, read by the same function: a scope's id, or its first six characters or more (not its title, which `--scope` does not take either). The daemon has no checkout, so without `scope` a call works in no scope. `FWD_AS`, `FWD_PROJECT` and `FWD_SCOPE` in the daemon's environment mean nothing to a call.

## Tools

The tool `task_create` is `fwd task create`. In general, a command's words joined with `_` name its tool. A tool's properties are the command's arguments, under their long names (`--done-when` becomes `done_when`) or, for a positional argument, its name in the grammar. Required arguments, switches, lists, value enums, integer types and defaults (as the schema's `default`, such as `priority`'s `now` for `ask` and `next` for `comment`) are all read from the grammar (`crates/fwd/src/mcp/tools.rs`). Constraints a schema does not express, such as `requires` and `required_unless_present`, are enforced by the CLI's parser and come back as its refusals.

Descriptions are the CLI's help, abridged so the catalog stays compact: a tool's is its command's short help (the doc comment's first paragraph), and a property's its argument's, each cut to its leading whole sentences up to 240 and 180 characters, a single longer sentence to its leading clauses, never inside brackets. The `project` and `scope` properties carry no description of their own; `project_list`'s description says what they are once. The CLI's full help for any tool's command is one call away: the `help` tool returns `fwd <command> --help` as clap renders it. The catalog went from 73 KB (74 tools) to under 32 KB (75); a test keeps it under 36,000 bytes. Codex 0.159.2 shows the model the tools of plugin MCP servers, in name order, only within one budget of about 60,000 bytes that every installed plugin shares, and charges each tool its own definition plus its server's whole `initialize` `instructions`; directly configured servers are outside that budget (`/private/tmp/fwd-connections-active/catalog-limit-report.md`). fwd's 648 characters of instructions, charged per tool, hid all but 48 of its tools, and a tool past the budget is not shown and not found by `tool_search`. So the endpoint sends no instructions (`project_list` says what `project` and `scope` are, `whoami` how a thread is attributed and named, the plugin's skills the rest), and the compact catalog leaves room in the shared budget for the person's other plugins. The model-visible count through the installed plugin is the acceptance check.

A call's arguments become the command line again (`fwd --format json <words> --flag=value ... -- <positionals>`), and the CLI's parser reads it, so a value the CLI refuses is refused in the CLI's words. Every value goes after `=` or after `--`, so no value can be read as a flag. The command then runs through `crate::refuse_by_identity` and `crate::run_as_caller`, the steps the CLI takes after connecting (refusals, the command, belonging, the dormancy stamp), over the daemon's durable node. Its output is captured (`crate::Output::Captured`). A tool's result holds:

- `result`: the JSON the command prints with `--format json`; a list when it prints several values.
- `notes`: what the CLI would say on standard error, such as the lead's triage guidance or a question that waits.
- `error`: on a refusal, in place of `result`, the CLI's own error text. A command that failed after it made something (its dormancy stamp, or the notice after it) keeps what it printed under `result` beside the error, as the CLI prints the record before it fails, so the caller learns the id of what it was told not to make again.

Only read-only tools carry an annotation, `readOnlyHint: true`: the reads about the graph, `project_list`, `whoami` and `help`. Tools that write, and tools that read the caller's own inbox or tasks (`inbox`, `inbox_pop`, `prime`, `task_next`, `task_list`), which make the caller's principal on a thread's first call, carry none, so MCP's defaults apply (not read-only, possibly destructive): fwd claims nothing about them it cannot keep, and `task_cancel`, `principal_retire` or `decision_repeal` are not called harmless. A call runs to its end once it has started: rmcp runs each handler in a task of its own, which a dropped request does not abort.

Arguments of a parent command are its subcommand's tool's too, given after the parent's word (`inbox_pop` takes `for` and `unread` as `fwd inbox --for x pop` does). A positional value given without the one before it is refused, since the CLI would read it in the earlier one's place. A number as `priority`, which the CLI takes in this build only with a warning on standard error, is refused: the endpoint takes now, next or later. The grammar is parsed with no argument read from the environment, so `FWD_AS`, `FWD_PROJECT` and `FWD_SCOPE` in the daemon's environment mean nothing to a call, and the parsed command's words are checked against the tool's.

Calling an unknown tool is a protocol error. Every other failure is a tool error.

### Coverage

| Area | Tools | Left out, and why |
|---|---|---|
| Discovery | `project_list`, `whoami` | `project show`, `unclaim`, `rename`: the machine's project index, a person's upkeep |
| Principals | `principal_list`, `principal_add`, `principal_role`, `principal_rename`, `principal_retire`, `principal_succeed`, `principal_scope_add`, `principal_scope_remove`, `principal_scope_project_wide` | `principal bind`: taking over another principal is the person's step (`fwd admin bind`) |
| Tasks | `task_create`, `task_list`, `task_show`, `task_next`, `task_assign`, `task_unassign`, `task_approve`, `task_reject`, `task_start`, `task_submit`, `task_accept`, `task_return`, `task_done`, `task_cancel`, `task_reopen`, `task_rejustify`, `task_amend`, `task_hold`, `task_release`, `task_explain` | none |
| Steps | `step_add` (by id), `step_list`, `step_check`, `step_uncheck`, `step_remove` | `step add --path/--lines/--against/--side`: reads a file in a checkout |
| Relations | `link`, `unlink`, `relations`, `relation_approve`, `relation_reject`, `cite` | none |
| Scopes | `scope_create`, `scope_branches`, `scope_list`, `scope_show`, `scope_rename`, `scope_intention`, `scope_status` | `scope attach-dir`: binds a local checkout. `scope create` finds no branch or repository of its own, so it takes `branch` and `repo` |
| Messages | `comment`, `ask`, `reply`, `forward`, `message_list`, `message_show`, `message_resolve`, `message_unresolve`, `message_edit` | `--path/--lines/--against/--side` on `comment` and `ask`. A `[[path]]` file mention in text is refused, since no checkout reads the file |
| Decisions | `decision_create`, `decision_list`, `decision_show`, `decision_attach`, `decision_detach`, `decision_accept`, `decision_reject`, `decision_repeal`, `decision_supersede`, `decision_export` (`result.markdown`) | none |
| Inbox | `inbox`, `inbox_pop`, `read`, `dismiss`, `prime`, `graph` | `watch`: never ends by itself (inbox registration and wake delivery are another task's, below). `session stop-check`, `session stamp`: Claude Code's hooks |
| Lifecycle and files | none | `init`, `daemon *`, `ui`, `checkouts`, `anchor *`, `admin *`, `capabilities`: the daemon's lifecycle, the machine's data folder, local checkouts and files |

The texts the CLI writes for a person name CLI commands, such as a notification's `reason` ("reply with fwd reply L6irYTTF"). Their tools are named by the same words.

## Extension points

- `mcp::Hosting`: what the endpoint needs of the daemon (data folder, port, hosted projects, and a context on each one's node). The daemon implements it on its state. A hosted server would implement its own.
- `mcp::Caller`: the attested caller. `Caller::from_codex_meta` makes one at the local transport, and a hosted transport would make one from its authenticated session.
- `mcp::Seen`: called after every call that acted as a principal, with the project, the principal and the caller. This is where daemon-owned inbox registration and wake delivery go, which another task builds; the daemon passes `mcp::NoWake` for now.

## Tests

- `crates/fwd/tests/mcp.rs` drives rmcp's Streamable HTTP client against a scratch daemon. It covers initialize, list and discovery; missing and inconsistent metadata; the lead's and an implementer's work, with refusals compared to the CLI's own text, dependencies, the review rule, inbox list versus pop, and settling a notification by reply; concurrent first calls and concurrent first names; separate threads and resume; naming and succeeding with no principal made first; a thread under another session; the daemon's inherited environment; the narrow `fwd admin bind`; and the `Origin` refusal.
- The unit tests in `mcp/caller.rs` and `mcp/tools.rs` check the metadata contract, the generated schemas, and that arguments cannot become flags.
- `serve.rs` tests that the endpoint refuses a LAN peer and a page.
- Real Codex 0.159.2 was run over HTTP against a scratch daemon with a fake loopback model provider, in both direct-function and code mode (`/private/tmp/fwd-connections-active/mcp-e2e/`). Thread A, thread B, A's subagent, and A after a resume in a new app-server process were each attributed to their own principal. A model's `as` argument and a metadata-less `mcpServer/tool/call` were refused.
