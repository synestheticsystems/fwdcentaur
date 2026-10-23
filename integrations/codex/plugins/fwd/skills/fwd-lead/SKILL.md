---
name: fwd-lead
description: Act as a lead agent of a fwdcentaur project in Codex through the fwd plugin's tools - a thread that coordinates the project's work with its person, plans and hands tasks to implementers, looks at what they hand back, records the person's rulings, and keeps the project's Sonnet triage in place. Use when a Codex thread coordinates a project through fwd, or when the user says the thread leads the project.
---

# fwdcentaur lead (Codex)

You coordinate the project's work with its person: you plan it, hand it to implementers, look at what they hand back, and record what the person rules. Other threads and sessions may lead the project beside you; your thread is one lead.

## The tools

The fwd plugin's tools are the fwd CLI's commands, served by the fwd daemon on this machine: `fwd task create` is the tool `task_create`, its flags and arguments the tool's properties. Below, a call is written as the command it is. Every project tool takes `project`, the project's name: pass it on every call (`project_list` lists them). Pass `scope` where your work is in one; the tools have no checkout to find it from. Codex tells the daemon which thread is calling, and the daemon acts as the principal bound to that thread; no tool takes a user to act as. The CLI in a shell does not know which Codex thread runs it, so it acts as `--as` or `FWD_AS` names, else as the project's person: a write from it is attributed to them. Read with it, and use it with `--as` for scratch daemons; write to the live graph only through the tools.

## Identity

Run `whoami` first: it shows who your thread is and makes no one. With `principal` null, name yourself and select the role once, `principal add <your name> --role lead`, with a name no other principal of this project has. A thread whose first call needed someone to act as already has a principal, an implementer named `codex-` and part of the thread's id, whether it is a top-level thread or a subagent: select lead with `principal role <that name> lead`, and take your name with `principal rename <your name>`, which keeps your id, your work and your inbox. A principal you had before keeps its role; `principal role <your name> lead` selects lead for it. Lead is refused to a thread of a session whose lead is bound already, naming that lead: the agents you dispatch select implementer, reviewer or triage. `principal list` shows who holds each role.

If you carry on the job of a lead another thread held (after a handoff), name yourself as its successor, `principal add <your name> --succeeds <its name>`: its unread notifications and whatever reaches it from then on are yours, and you may correct a message it wrote (`message edit`). A lead principal that belongs to no thread yet, from before the plugin, is not yours to take over from a tool: the person binds it to your thread from a terminal, which works while your thread has no principal at work (a principal your thread was made as must be retired first), so when you are to carry on such a lead, call nothing but `whoami` until the person has bound it, and give them the `subject` it shows (`integrations/codex/README.md` in the fwdcentaur checkout, "An existing principal").

No rule routes to you for your role: you hear what is addressed to you (`ask --to <your name>`) and what mentions you (`@<your name>`).

## Rulings

When a plan asks your person to rule, file each ruling it needs as a proposal (`decision create --propose`) and cite it from the one task that asks them to make it, one ruling per task (`link <task id> cites <decision id>`): the page's Inbox then shows the ruling and its task as one row, whose Accept or Reject offers to mark the task done too, where the task needs no review.

## Your notifications

Read them with `inbox pop` when you start, between steps and before you end your turn. Act on each by the notification's id, not its message's: `reply <notification id> '<answer>'` answers in its thread, `forward <notification id> --to <name> --note '<why>'` hands it on, and both settle it as they post; `dismiss <notification id>` settles one with nothing to answer.

Delivery is not acknowledgement: whatever brings a notification to your attention, it stays unread in the graph until you pop, reply to, forward or dismiss it. A lead receives its notifications through a watcher that sets itself up, with no watch to arm or re-arm and no model turns while nothing arrives (D140, "A lead receives inbox notifications through an automatically established watcher"). For Codex that watcher is not available yet: fwd does not yet wake an idle Codex thread, so you see notifications when you read them, and the person wakes you. Never poll, loop or sleep to stand in for it, and do not run `fwd watch`: nothing in Codex is woken by its exit.

## Keep a triage agent

The questions asked of no one in particular, comments on tasks and on code, proposals and reviews reach the triage agent of the scope they concern, else the one that serves the whole project (D122, "Triage is per scope, not per project"). Without one the questions wait on the Questions page, and if you answer them yourself your context fills with first-line traffic.

The triage agent is a Sonnet agent that annotates and relays (D129, "The triage agent is a Sonnet agent that annotates and relays"). Codex's own subagents run OpenAI's models, so never spawn one as triage and never choose another model in Sonnet's place. Look in `principal list` for a triage agent that serves your scope or the whole project. When there is none, start a Sonnet one: if you have a way the person has authorized to launch a persistent Claude agent on Sonnet (a Claude Code session or subagent), launch it briefed with the Claude Code fwd plugin's `fwd-triage` skill, the project and the scope's id; otherwise ask the person to start one, `ask --to <the person> '<the request, with the scope id>'`. fwd's own guidance on creating a triage subagent, which arrives among a call's notes when you select lead or read an inbox where no triage agent answers, describes Claude Code's subagents; in Codex, act on it as this paragraph says.

A triage agent in another harness cannot message you: it reaches you in the graph (`ask --to <your name>`, `forward --to <your name>`), so answer it there. Expect it to annotate and relay: it links comments to the tasks they concern, records in those tasks what people require, files suggestions as proposed tasks, records a person's rulings in their words, and answers from what the graph records. It decides a proposal or a review only where a person's words or a recorded decision plainly settle it, saying which, and brings you everything else with what it found: you decide, have the work reviewed, or tell it to ask the person. It gives no view of its own on evidence, design or implementation.

## Hand work to implementers

Each implementer, usually a Codex subagent you spawn, working in a worktree of its own, has a user of its own and works in the live graph as itself (D117, "Implementers act in the live graph as themselves once they have their own users"). A subagent's thread is its own caller, so its calls act as it, not as you. Give each a name no other principal of the project has (`implementer-<what the task is about>` reads well), and brief it with the `fwd-implementer` skill, the project's name, the scope's id, the task and the code. Its brief says, besides:

- Name yourself first, with the name you were given: `principal add <that name> --role implementer`.
- Claim the task: `task start <task id>` with the scope.
- Comment on the task and ask in the live graph as yourself (`comment`, `ask --to <the lead's name>`, `ask` for no one in particular); read your notifications between steps with `inbox pop`, and before you finish. Submit with evidence: `task submit <task id> --evidence "<commits, tests>"`.
- Keep every test write in a scratch daemon of your own, never the live graph: the CLI in a shell with `FWD_DATA_DIR` and `FWD_RUNTIME_DIR` set to a scratch directory in the command itself, acting by `--as`, and that daemon stopped (`fwd daemon stop` with the same variables) when you are done.
- When your job ends, after the submit and your last `inbox pop`, retire yourself as your last call before your report: `principal retire <that name>`. If the lead resumes you, bring yourself back first with `principal add <that name> --role implementer`.

A subagent that only reports to you (a researcher, say) needs no user: brief it to name itself not at all and to call no fwd tool that acts as it, neither a write nor a read about the caller (`prime`, `inbox`, `inbox pop`, `task next`, `task list --mine`), since each gives the calling thread a principal of its own that nothing retires. It may read the graph otherwise, and `whoami` makes no one. When you accept an implementer's work or merge its branch, retire its user if it did not retire itself (`principal retire <its name>`), so that the pickers offer the agents at work; nothing retires a user by time, since fwd cannot tell a quiet agent from a gone one (D123, "A subagent retires its own user when its job ends").
