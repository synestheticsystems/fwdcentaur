# fwd, the Claude Code plugin

This plugin gives each Claude Code agent on the machine a user of its own in fwdcentaur, so that what an agent writes in a project's graph is attributed to that agent and not to its lead or to you (`docs/design/agents-and-inbox.md`, "An agent's identity"; `docs/DECISIONS.md`, D112 and D118). Its one hook stamps each `fwd` command an agent runs with the ids Claude Code gives that agent, the session's and inside a subagent the subagent's own, and the command then acts as the principal the agent named for itself, with no `--as`. **The hook approves the commands it stamps, without a permission prompt.** Claude Code applies a hook's change to a command only when the hook also approves the command (`permissionDecision: "allow"`: code.claude.com/docs/en/hooks, "PreToolUse decision control"), which skips the prompt and, in auto mode, the classifier; the deny and ask rules in your settings still apply. So the hook stamps, and approves, only a command whose approval gives away nothing but fwd: one made of `fwd` invocations, `cd` and the filters `head`, `tail`, `grep`, `wc`, `jq`, `sort`, `uniq` and `cut` reading fwd's output on their standard input, joined by `|`, `&&`, `||` or `;`, with no `$` or backtick outside single quotes, no globbing, grouping or background, and no redirection but `2>&1` and to or from `/dev/null`. fwd's commands that act for the machine or its person rather than for an agent are never stamped: `fwd admin`, `fwd daemon`, `fwd init`, `fwd ui` and `fwd project`. A filter given a file, or an option that reads or writes one, is not stamped either, so that approving the command reads nothing fwd did not print. Any other command, one naming fwd among the rest included, is left alone and goes through the permission flow as it would without the plugin. So is a call that asks to run outside Claude Code's sandbox (the Bash tool's `dangerouslyDisableSandbox`), whatever its command: the hook approves a command for what it runs, and that approval never extends to a call the agent asked to elevate, so you see the request in the permission flow.

It also brings the skills for each role: `fwd:fwd-lead`, `fwd:fwd-triage` and `fwd:fwd-implementer`.

## What is here

- `.claude-plugin/plugin.json`: the manifest, which names the plugin `fwd`. The marketplace that lists it is `.claude-plugin/marketplace.json` at the root of the fwdcentaur checkout, named `fwdcentaur`.
- `hooks/hooks.json`: the `PreToolUse` hook on Bash, which runs `hooks/stamp`.
- `hooks/stamp`: hands the hook's JSON to `fwd session stamp`, which prints the JSON that stamps and approves a command it accepts and nothing for any other (`crates/fwd/src/claude_code.rs` holds what it accepts). It exits 0 whatever happens, printing nothing when `fwd` is not on the `PATH` or is too old to stamp, so the hook never blocks a command.
- `skills/`: the three skills.
- `settings.example.json`: the inbox hooks, for an agent that runs as a Claude Code session of its own (below).

A stamped command carries `FWD_CLAUDE_SESSION` (the session's id) and `FWD_CLAUDE_AGENT` (the subagent's id, or `main` for the session's own thread), exported before the command itself:

```sh
export FWD_CLAUDE_SESSION='56693b92-71ca-49ec-8ab4-aee1de36a4d3' FWD_CLAUDE_AGENT='a7b8a4211fe9f76b7'; fwd task show LK12MGks | head -40
```

fwd reads the pair as the command's subject, `claude-code:<session>/<agent>`, and acts as the principal of the project bound to it. Where a command carries a subject, `FWD_AS` is not read, and a `--as` naming any other principal is refused.

## Installing

Once per machine. `fwd` must be on the `PATH` Claude Code runs its hooks with (`~/.cargo/bin` after `cargo install --path crates/fwd`), in a build that has `fwd session stamp`.

```sh
claude plugin marketplace add /path/to/fwdcentaur   # the checkout, not a worktree that may go away
claude plugin install fwd@fwdcentaur               # user scope: every project on this machine
```

`claude plugin install` copies the plugin into Claude Code's plugin cache (`~/.claude/plugins/cache/fwdcentaur/fwd/<version>`), and sessions run that copy, not the checkout (Claude Code 2.1.269 to 2.1.284, checked 2026-09-30), so a change to the plugin reaches them only once the copy is refreshed ("Updating", below). Nothing is added to any project's repository. If the skills were copied into `~/.claude/skills/` before the plugin existed, remove those copies (`fwd-lead`, `fwd-triage`, `fwd-implementer`), since the plugin brings them.

## Checking it works

1. `claude plugin list` shows `fwd@fwdcentaur` enabled, and `claude plugin details fwd` lists the hook and the three skills. `scripts/refresh-plugin.sh --check`, from the checkout, says whether the copy sessions run matches the checkout ("Updating", below).
2. The hook alone, from the checkout, with no Claude Code involved: a command made only of fwd is stamped and approved, any other is left alone. Claude Code runs the call with the hook's `updatedInput` as its whole input, so the stamp gives back the call's other arguments as they came, such as `run_in_background` here, and changes only the command.
   ```sh
   echo '{"hook_event_name":"PreToolUse","session_id":"s1","agent_id":"a1","tool_name":"Bash","tool_input":{"command":"fwd watch --count 1","run_in_background":true}}' | integrations/claude-code/hooks/stamp
   # {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"export FWD_CLAUDE_SESSION='s1' FWD_CLAUDE_AGENT='a1'; fwd watch --count 1","run_in_background":true}}}
   echo '{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Bash","tool_input":{"command":"cargo test"}}' | integrations/claude-code/hooks/stamp
   # (nothing)
   ```
3. In a new session, ask Claude to run `fwd whoami`: `subject` is `claude-code:<the session's id>/main`, and a subagent's is its own agent id. Until the agent names itself, `principal` is `null`.

## Updating

Sessions run the copy of the plugin in Claude Code's plugin cache, not the checkout ("Installing", above), so after pulling a fwdcentaur that changes the plugin (its hook or its skills), refresh the copy, from the checkout:

```sh
scripts/refresh-plugin.sh           # claude plugin update fwd@fwdcentaur, then the copy compared with the checkout
scripts/refresh-plugin.sh --check   # the comparison alone, which changes nothing
```

The script runs `claude plugin update fwd@fwdcentaur`, then compares the copy Claude Code records for the plugin with `integrations/claude-code` in the checkout the marketplace was added from, file by file, leaving out the markers Claude Code writes into a copy (`.in_use`, `.orphaned_at`); it names each file that differs and exits 1 if one does. A new session loads the new copy as it starts, and a running session goes on with the copy it loaded until `/reload-plugins`.

The update makes a new copy only when the plugin's version has changed, and `.claude-plugin/plugin.json` names no version so that the version changes with every commit: with none named, Claude Code takes the commit the checkout is on as the version (code.claude.com/docs/en/plugins/loading, "Versions and updates", read 2026-09-30). Until 2026-09-30 the manifest named 0.1.0, so the version never changed: `claude plugin update` said the plugin was at the latest version and left the copy as it was first installed (task dcAEY3Wz). Leaving the version out costs three things, each accepted:

- Every update after a commit makes a new directory in the cache, named after the commit's first 12 characters, and Claude Code deletes the directory it replaced 14 days later.
- `claude plugin validate` warns that no version is specified.
- An edit to the plugin that is not committed moves no version, so the update leaves the copy as it was; the script's comparison then names the edited files, to be committed before it runs again.

The same page of Claude Code's docs ("In-place and copied plugins") says that a plugin at a relative path in a marketplace added from a local directory, as this one is, loads in place from the checkout, with no copy to refresh. The builds named under "Installing" made a copy and ran it, so the copy needs refreshing until a build that loads the plugin in place is installed.

## An agent's first commands

- An agent new to the project names itself and selects its role, which binds the principal to it: `fwd principal add <a name no other principal of the project has> --role lead|triage|implementer|reviewer`. A session has one lead, its own thread, and the subagents it dispatches select another role: lead is refused to a second agent of a session, naming the session's lead, while another session leads the project beside it. A triage agent serves a scope, the one its checkout finds or `--scope <id>` names, or with `--project-wide`, in a small project with small scopes, the whole project; with neither it is refused, naming the two ways out.
- A principal from before the plugin that no agent is bound to, such as `claude`, is taken over once, from the session that acts as it, by saying its name twice: `fwd --as claude principal bind claude`.
- An agent relaunched to carry on another's job names itself as the other's successor in one step, `fwd principal add <name> --succeeds <the other's name>`, which retires the other and moves its role (a lead's or a triage agent's included, with the scopes a triage agent served), its unread notifications and whatever reaches it from then on to the new agent.

Until an agent has named itself, its writes, and its reads of its own inbox and tasks, are refused with the command that names it. In a session one of whose agents is bound, a write in a command the hook left alone is refused too, since fwd cannot tell which of the session's agents ran it, unless `--as` is typed on it (below): run fwd on its own or piped into a filter, and without asking to leave the sandbox, or type `--as <your name>` on the command.

## Without the plugin

The plugin is optional (`docs/DECISIONS.md`, D120). An agent that does not use it, one that is not Claude Code or a Claude Code session on a machine without the plugin, acts as agents did before the plugin existed: it names itself with `fwd --as <name> principal add <name> --role <role>`, making a principal no agent is bound to, and acts as itself by typing `--as <name>` on each command, or by `FWD_AS=<name>` in its environment (`docs/design/agents-and-inbox.md`, "An agent's identity", "Three ways in").

Where the plugin is installed, a command the hook leaves alone carries no agent's stamp: a script an agent runs, a command with another program in it, a call that asks to leave the sandbox, one you type in a shell of your own that carries the session's id (`CLAUDE_CODE_SESSION_ID`), or one from an agent of another kind started from the session, which inherits that id. In a session one of whose agents is bound, such a command acts as the principal a `--as` typed on its command line names, `fwd --as <name> ...`, since no agent inherits what is typed on a command; with `FWD_AS` alone it reads as that principal but writes nothing, since every agent of the session inherits the session's environment; and with neither it acts as no one rather than as you, so that no agent reads your inbox as its own: its reads of the caller's own inbox and tasks are refused as its writes are, and its reads about the graph alone run (task JFASz_E2).

## The inbox hooks, for an agent that runs as a session of its own

The plugin's hook stamps Bash commands only. The hooks that put an agent's notifications into its context between its steps are commands Claude Code runs itself, which carry no stamp. Claude Code hands each of them, on standard input, the JSON the stamp reads too: the session's id (`session_id`) and, inside a subagent, the subagent's (`agent_id`). A command given `--format hook` reads it and acts for the principal bound to the agent whose hook fired, as that agent's stamped commands do, which wins over `FWD_AS`. While no principal is bound to it, a subagent's hook acts for no one and prints nothing, since a session's settings give its `FWD_AS` to its subagents too; the hook of the session's own thread acts for the principal `FWD_AS` names, as the hooks did before 2026-09-30, and for no one without it (`docs/design/agents-and-inbox.md`, "The hooks"). So a lead whose hooks ran by `FWD_AS` keeps getting its notifications when this build is installed, before its thread is bound. They belong in the checkout of a project whose agents use them rather than in the plugin, since a hook in every session on the machine would run fwd, and fail, after each batch of tool calls in every project with no fwd graph. A lead that runs as a session of its own, a pilot's lead say, or a triage agent that is not a subagent, sets them up so.

Which of them Claude Code runs inside a subagent, as its hooks reference says it (code.claude.com/docs/en/hooks, read 2026-09-30): `Stop` runs when the session's own thread finishes, a subagent's end being `SubagentStop`; `SessionStart` when a session starts, resumes, is cleared or is compacted, a subagent's start being `SubagentStart`; and `UserPromptSubmit` when you submit a prompt. The reference says the hooks of settings files and plugins run inside subagents and that tool events such as `PreToolUse` fire for a subagent's tool calls, but it does not name `PostToolBatch`, so it leaves open whether a subagent's batches of tool calls run that hook. Where they do, the hook puts the subagent's own notifications into its context if it named itself, and nothing if it has not. Either way a subagent reads its inbox with `fwd inbox pop` between its steps and before it finishes, since no `Stop` hook keeps it working while notifications are unread.

1. Give the agent a checkout of its own. An implementer's is a worktree on its own branch, where the scope it works in is found by that branch or by binding the checkout to it (`fwd scope attach-dir`); a triage agent needs no branch, and a detached worktree finds no scope, so it names the scope it serves as it names itself (`fwd principal add <name> --role triage --scope <id>`), or its checkout is bound to that scope first:
   ```sh
   git worktree add ../widgets-retry -b feat/retry     # an implementer
   git worktree add --detach ../widgets-triage         # the triage agent
   ```
2. In that checkout, put the `hooks` block of `settings.example.json` into `.claude/settings.local.json` (merge it into the file if it exists, and keep it out of git). The hooks find the principal the agent named for itself in the session (`fwd principal add <name> --role <role>`), or took over from before the plugin (`fwd --as <name> principal bind <name>`), by its binding to the agent and not by its name, so a rename changes nothing here. Until the session's own thread has a principal bound to it, its hooks act for the one `FWD_AS` names, which settings written before 2026-09-30 set in an `env` block beside the hooks, and print nothing without it; once one is bound, `FWD_AS` is not read and can go. Keep the hooks out of the shared `.claude/settings.json`, which is committed and reaches everyone who works in the repository, whether fwd is installed or not.
3. Start `claude` in that checkout. An implementer's first session binds the checkout to the scope its work belongs in, whoever made it, or makes a scope for work no scope covers (the skill says which), and later sessions there find it the same way. The triage agent runs its pass at the start and whenever its one background watch prints a notification, never on a timer or in a loop, and arms the watch again after each pass and after each `timeout` event (`fwd watch --count 1 --timeout 7100`, since the harness ends a background command within two hours), so that it costs no tokens between notifications but one short turn every two hours (`docs/DECISIONS.md`, D130); a lead usually runs it as a background subagent on Sonnet, as the lead's skill says.

Each hook is one `fwd` call:

| Hook event | Command | What happens |
|---|---|---|
| `SessionStart` | `fwd prime --format hook` | The scope, the ready tasks and the unread notifications go into the agent's context at startup, on resume, after `/clear` and after compaction (the entry has no matcher, so it runs for each). |
| `UserPromptSubmit` | `fwd inbox pop --format hook --hook-event UserPromptSubmit` | Notifications that arrived since the agent's last step go into its context and are marked read. It prints nothing when none arrived. |
| `PostToolBatch` | `fwd inbox pop --format hook --hook-event PostToolBatch` | The same after each batch of tool calls. Until 2026-09-29 it passed `--heartbeat` and recorded the scope's heartbeat, which went with the scope's owner (D108, "A scope has no owner"); the build after that took the flag and ignored it, and fwd refuses it now, saying to take it out, so a hook written before fails until `--heartbeat` is taken out of its command in `.claude/settings.local.json` (or `.claude/settings.json`). |
| `Stop` | `fwd session stop-check --format hook` | While unread notifications remain, the agent is kept working (`{"decision":"block",...}`); otherwise it may stop (`{}`). Claude Code lets it stop anyway after eight blocks in a row, and runs it for the session's own thread alone. |

A `PreCompact` hook would not help: Claude Code does not put its output into the context. The `SessionStart` entry already primes the session again after compaction.
