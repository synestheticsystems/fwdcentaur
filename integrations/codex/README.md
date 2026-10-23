# fwd, the Codex plugin

This plugin lets a Codex thread work in a fwdcentaur project's graph as an agent of its own, with no user to name and no command to remember each session. Codex tells fwd which thread makes each tool call, so what a thread writes is attributed to that thread's principal, and a subagent's to its own (`docs/DECISIONS.md`, D141, "The daemon serves a CLI-aligned MCP interface with automatic caller attachment"). It brings:

- **The fwd tools**: the fwd CLI's commands, served by the fwd daemon on this machine, named by their words (`fwd task start` is `task_start`). Their rules, refusals and wording are the CLI's. Every project tool takes the project's name, since a tool has no checkout to find it from.
- **The skills** for each role: `fwd:fwd-lead`, `fwd:fwd-triage` and `fwd:fwd-implementer`.

Nothing here authenticates anyone. The daemon answers only processes on this machine, and the caller fwd attaches is what the local Codex reports, which a program on this machine could imitate: it attributes work, it does not secure it.

## What is here

- `.agents/plugins/marketplace.json`: the marketplace, named `fwdcentaur`, that lists the plugin.
- `plugins/fwd/plugin.json`: the plugin's manifest, in the portable Agent Plugins form, naming it `fwd`.
- `plugins/fwd/mcp.json`: the plugin's one MCP server, `fwd`, which Codex starts as `fwd mcp stdio`. It declares no authentication, since there is none: Codex 0.159.2 drops a stdio server whose entry declares `extensions.com.openai.auth` at all, even `{"type": "none"}` (checked 2026-10-09).
- `plugins/fwd/skills/`: the three skills.

`fwd mcp stdio` (`crates/fwd/src/mcp_stdio.rs`) is how a Codex thread reaches the daemon. The daemon listens on whatever port it got at its last start, which a URL written into the plugin would not follow, so Codex starts the fwd binary instead. Before each message it finds the daemon as every fwd command does, the one the port file names answering as that instance, starting it when none runs, and passes the message to the daemon's `/mcp` unchanged. A daemon that restarts, on the same port or another, costs a thread nothing but a call in flight, and a call whose connection broke after it was sent is answered with an error saying it may have been made, never sent twice. The plugin installs no hooks.

## Installing

Once per machine, with Codex 0.159.2 (checked 2026-10-09).

1. Install fwd from the checkout, in a build that has `fwd mcp stdio`: `cargo install --path crates/fwd`. Codex starts `fwd` by name from the `PATH` it starts MCP servers with, which must include `~/.cargo/bin` (see "The relay's environment", below).
2. Add the marketplace and the plugin. The marketplace lives at `integrations/codex`, not at the repository's root, so Codex does not find it in the checkout on its own; add it once:

   ```sh
   codex plugin marketplace add /path/to/fwdcentaur/integrations/codex   # the checkout, not a worktree that may go away
   codex plugin add fwd@fwdcentaur
   ```

   In the ChatGPT desktop app, restart the app after adding the marketplace; the plugin is then also offered in the Plugins Directory under fwdcentaur. Installing copies the plugin into `~/.codex/plugins/cache/fwdcentaur/fwd/<version>/`, and Codex runs that copy.

Nothing else is needed. Codex asks before a fwd tool runs as your approval policy says, as it does for any MCP server: it asks you, and the call runs once you allow it (checked with an approval policy that asks, 2026-10-09).

Only a client that runs with no one to ask, a `codex exec` worker or a thread whose approval policy is `never`, needs the tools approved beforehand, since Codex refuses a call that needs approval where it may not ask. If you run such a client, you may approve fwd's tools for it in that client's configuration:

```toml
[plugins."fwd@fwdcentaur".mcp_servers.fwd]
default_tools_approval_mode = "approve"
```

That lets every fwd tool run without asking, the ones that write to the graph included. It is a choice for unattended clients, not part of installing.

A thread's sandbox does not hold back the tools: a thread with a read-only sandbox called them (checked 2026-10-09).

### The relay's environment

Of the variables in its own environment, Codex hands the relay `HOME`, `PATH` and `TMPDIR` (besides a few the shell sets, and its own `PLUGIN_ROOT` and `PLUGIN_DATA`); it filters out the rest, `FWD_DATA_DIR`, `FWD_RUNTIME_DIR`, `FWD_PROJECT` and `FWD_AS` among them, and a plugin's server has no setting to pass one on: `env_vars` under `[plugins."fwd@fwdcentaur".mcp_servers.fwd]` is not applied (checked 2026-10-09). So:

- The relay always uses fwd's default directories: the data folder under your home, and the runtime folder `$TMPDIR/fwd-<uid>`, where the daemon's port file and locks are. It reaches the daemon a terminal's fwd reaches only when both see the same `TMPDIR`; on the machine this was checked on there is one runtime folder, so they do, but not every way of starting Codex has been checked. If you run fwd with `FWD_DATA_DIR` or `FWD_RUNTIME_DIR` set, Codex's threads do not reach that daemon; the plugin offers no way to point them elsewhere, and none should be improvised by hand, since a relay that found another runtime folder over the same data folder would start a second daemon on the same databases.
- `fwd` is found on Codex's `PATH`. A desktop app started from the Dock may not have `~/.cargo/bin` on its `PATH`. After installing, check that the plugin's `fwd` server connects (in a thread, call fwd's `whoami`); if Codex cannot start it, put `fwd` where that `PATH` looks, a link in `/usr/local/bin` for instance (`ln -s ~/.cargo/bin/fwd /usr/local/bin/fwd`), and restart the app. Whether the ChatGPT desktop app's `PATH` includes `~/.cargo/bin` has not been checked.

## Checking it works

1. `codex mcp list` shows `fwd`, with command `fwd` and args `mcp stdio`, enabled. It reads the configuration only; it does not start the server.
2. In a new thread, ask Codex to call fwd's `whoami` for your project. That starts the relay and the daemon if need be. `subject` is `codex:<the session's root thread>/<this thread>`. `whoami` makes no one, so until the thread names itself or calls a tool that acts as it, `principal` is `null`.

### When the model cannot see some fwd tools

Codex 0.159.2 shows the model the tools of every installed plugin's MCP servers only within one budget that all of them share (about 60,000 bytes of tool definitions, counted in name order). A tool past it is not shown to the model and `tool_search` does not find it, though Codex still runs it when it is named exactly; servers you configure yourself under `[mcp_servers.<name>]` are outside that budget. fwd keeps its share small, about 32,000 bytes for its 75 tools, with no server instructions (Codex charges those to each tool), so with fwd alone every tool is shown. Many other plugins installed at once can still crowd some of fwd's tools out, those late in name order first (`task_*`, `unlink`, `whoami`). If a thread says a fwd tool does not exist, see which plugins are installed (`codex plugin list`). Removing plugins you do not use is one way out; another is to run fwd as a server you configure yourself, outside the budget (`docs/design/mcp.md`, "Transport"). Nothing in fwd works around the budget, and nothing here changes your configuration.

If the call says the running daemon predates the MCP endpoint, restart it (`fwd daemon stop`, then any fwd command) after installing the new fwd.

## Updating

After pulling a fwdcentaur that changes the plugin, run `codex plugin add fwd@fwdcentaur` again, which replaces the copy Codex runs, and start a new thread (or restart the desktop app). A change to `fwd mcp stdio` arrives with `cargo install`; a change to the tools also needs the daemon restarted (`fwd daemon stop`, then any fwd command), since the daemon serves them.

## Who a thread is

A thread's first call that acts as it, a write or a read of its own inbox or tasks (`prime`, `inbox`, `inbox_pop`, `task_next`, `task_list` of its own), gives it a principal of its own: an implementer named `codex-` and part of the thread's id, whether the thread is a top-level one or a subagent. It then selects another role with `principal_role` and takes a name with `principal_rename`, keeping one principal. Or it names itself first with `principal_add`, which the skills tell it to do. Each subagent is its own caller. A thread resumed later is still the same principal. No tool takes a user to act as. Discovery (`project_list`, `whoami`) makes no one.

A call Codex did not make for a thread, one without the thread and turn Codex attaches to a model's call, is refused rather than attributed to anyone.

### An existing principal

A lead from before the plugin that no thread is bound to, such as one another harness held, is bound to a Codex thread by you, from a terminal, before that thread calls anything but `whoami` or `project_list`: `fwd admin bind <its name> --subject codex:<session>/<thread>`, where `whoami` in that thread shows the subject. Any other call, a read of the thread's own inbox or tasks or a write, even one that is refused, makes the thread a `codex-…` principal of its own; retire that one first (`fwd principal retire <its id>`), and the bind then takes the thread. It binds only a lead at work that no one is bound to, to a subject no principal at work holds, and retires or replaces nothing itself.

## Not yet

- **Waking an idle thread.** A lead's notifications are meant to reach it through a watcher that sets itself up, with nothing to arm and no model turns while nothing arrives (D140, "A lead receives inbox notifications through an automatically established watcher"; task JpOUl0V6, "Local inbox delivery"). The ChatGPT desktop app offers no supported way for another program to start a turn in one of its threads, so for now a thread sees its notifications when it reads its inbox, and the skills tell agents not to poll for them. The daemon's MCP endpoint has a place for a delivery adapter to hear of each calling thread, so a working adapter needs nothing from the plugin unless it needs the host to announce a thread before its first call.
- **Triage on Sonnet.** The triage agent is a Sonnet agent (D129, "The triage agent is a Sonnet agent that annotates and relays"), and Codex's own subagents run OpenAI's models, so a Codex lead never spawns one as triage: it launches a persistent Claude agent on Sonnet where it has a way you authorized to do so, else asks you to start one (`integrations/claude-code/README.md`).

## Without the plugin

The CLI works in Codex as anywhere: `fwd` in a shell. Codex attaches no thread to a shell command, so there it acts as `--as` or `FWD_AS` names, else as you, the project's person: a `fwd whoami` run from a Codex thread's shell shows you (checked 2026-10-09), and a write from it is attributed to you. The skills therefore keep agents' writes to the tools (`docs/design/agents-and-inbox.md`, "An agent's identity").
