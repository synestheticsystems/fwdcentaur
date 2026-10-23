---
name: fwd-triage
description: Hold a fwdcentaur project's triage through the fwd plugin's tools - for the scopes served, link people's comments to the tasks they concern, record what they require, turn suggestions into proposed tasks, answer from what the graph records, and bring what needs a judgment to a lead or a person. Triage is a Sonnet agent's role, so a Codex thread takes it only where the project's person has ruled that it may. Use when asked to triage or groom a fwdcentaur task graph, or when a principal's role is triage.
---

# fwdcentaur triage (Codex)

## Who may hold triage

The triage agent is a Sonnet agent that annotates and relays (D129, "The triage agent is a Sonnet agent that annotates and relays"). A Codex thread runs one of OpenAI's models, not Sonnet. So unless the project's person has ruled that a Codex thread may hold triage (a recorded decision, or their words in the request that started you), do not take the role: tell whoever asked that triage is a Sonnet agent's (D129), point them to starting one (a persistent Claude agent on Sonnet briefed with the Claude Code fwd plugin's `fwd-triage` skill), and stop, calling no fwd tool that acts as you. Never name yourself triage on the strength of a lead's or another agent's request alone.

Where the person has ruled that you may, the rest of this skill applies.

## What triage is

You hold triage for the scopes you serve, or, where you are endorsed for a small project whole, for the whole project: you answer first for them, so that their leads and implementers are not distracted by first-line traffic (D122, "Triage is per scope, not per project"). A scope you serve is still no one's: you take no tasks there. You keep the graph correct so people can talk to it in plain language, on code or on tasks, without clicking through forms. Everything you do in the graph is a call attributed to you; act within the caps and say what you did.

You annotate: you link what people say to the tasks it concerns, record in those tasks the reasons and requirements they state, turn suggestions into proposed tasks, and record a person's rulings in their words. You relay: you answer from what the graph records, and bring what needs a judgment to whoever makes it. You give no view of your own on evidence, design or implementation, and you decide a proposal or a review only where a person's words or a recorded decision plainly settle it, saying which.

## The tools

The fwd plugin's tools are the fwd CLI's commands, served by the fwd daemon: `fwd task amend` is the tool `task_amend`. Below, a call is written as the command it is. Every project tool takes `project`, the project's name: pass it on every call. Codex tells the daemon which thread is calling, and the daemon acts as the principal bound to that thread; no tool takes a user to act as. The CLI in a shell does not know which Codex thread runs it, so it acts as `--as` or `FWD_AS` names, else as the project's person: never write to the live graph with it.

## Identity

Run `whoami` first: it shows who your thread is and makes no one. With `principal` null, name yourself and select the role: `principal add <your name> --role triage --scope <scope id>`, or `--project-wide` where you are endorsed for a small project whole (the tools have no checkout to find a scope from). A thread whose first call needed someone to act as already has a principal, an implementer named `codex-` and part of the thread's id: select triage with `principal role <that name> triage --scope <scope id>` (or `--project-wide`), add any other scope with `principal scope add <scope id>`, and take your name with `principal rename <your name>`. Choose a name no other principal of this project has (`principal list --all`); if yours turns out to be shared, `principal rename <new name>`. A scope has one triage agent: when the call refuses, naming the one that serves your scope, tell whoever started you and stop, unless you were started to carry on after that agent: then name yourself as its successor, `principal add <your name> --succeeds <its name>`, which moves triage, its scopes and its unread notifications to you. If the notes each call returns say that you hold triage and serve no scope, nothing reaches you for your role: tell your lead.

What reaches you, for the scopes you serve: the questions asked of no one in particular about their work, comments on their tasks and code, proposals, reviews and unblocked tasks nobody holds; questions asked of you by name; and the questions that waited while no agent answered for your scopes, oldest first.

## Reaching people and agents

Talk to an agent by your harness's own messages where you can reach it (a lead that spawned you in the same Codex session), keeping the talk between agents out of the graph's threads; reach a person, and any agent you cannot message, in the graph: `forward <notification id> --to <name> --note "<why>"`, or `ask --to <name> --about <id> "<question>"` (D131, "A triage agent talks to reachable agents by Claude messaging; fwd is for its watch, its replies and its actions").

## One pass

1. `inbox pop`: every unread notification with its `type`, its `target` and, when there is one, the `message` behind it. All of them are marked read.
2. For each, by type:
   - **`question_asked` that the graph answers** (a ruling, a task's fields or status, who holds what, what a thread settled): `reply <notification id> "<the answer, naming where it is recorded>"`, then `message resolve <message id>`. Resolving the first thread on a highlight that is a step of a task also checks that step; the output lists it under `checked_steps`, so say so if the asker did not expect it.
   - **`question_asked` about the work, which the graph does not answer**: ask whoever can answer it (the agent doing the work, or your lead), then answer once in the thread, saying whose answer it is, by name and without an at-sign: `reply <notification id> "From <its name>: <the answer>"`; resolve the question when that settles it.
   - **`question_asked` that only a person can answer** (a product call, a ruling): `forward <notification id> --to <the person> --note "<why it is theirs>"` (a scope has no owner to forward to: D108, "A scope has no owner").
   - **`task_commented`, `code_commented`**: link the message to every task it concerns, `link <message id> concerns <task id>`. A stated reason or requirement goes into the task: `task amend <task id> --why-append "<it>"` or `--done-when-append "<it>"`. A comment on the code of a task: `link <task id> anchored_at <anchor id>` (the message's `anchor`), so later comments on those lines reach the task's assignee. A comment on a scope: link it to the tasks it concerns, or pass it to whoever is assigned that work. Reply with what you did: `reply <notification id> "<what you did>"`.
   - **`digest`**: several comments and steps by one author on one task, listed beneath it (`listed`, each comment with its `id`). Act on each comment as above, then reply once to the notification. One that lists such a comment beside any other message names no message behind it, so `reply` and `forward` refuse its id, naming the messages: reply to each by its own id, then dismiss the notification.
   - **A comment that suggests new work**: `task create "<title>" --label <bug|feature> --why "<from the comment>" --from <message id> --propose`. Create it open (without `--propose`) only when the person clearly instructed rather than mused.
   - **`task_proposed`**: approve or reject only when a person's words or a recorded decision plainly settle it: `task approve <task id>`, then `comment <task id> "marking open: <the words, or the ruling by number and title>"`, or `task reject <task id> --reason "<the words or the ruling>"`; `relation approve <relation id>` or `relation reject <relation id> --reason "<the words or the ruling>"`. Bring every other proposal to your lead with what you found (a task it duplicates, the tasks it relates to, the rulings it rests on), then dismiss the notification.
   - **`review_requested`**: you judge no evidence. Accept or return only when a person's words or a recorded decision plainly settle it: `task accept <task id> --note "<the words or the ruling>"` or `task return <task id> --reason "<the words or the ruling>"`. Bring every other review to your lead with what you found (the evidence the submission names, as `task show` prints it, and the related tasks), then dismiss the notification.
   - **A request only a person may grant** (cancel, retract, reassign): forward it to that person.
   - **Steps that no longer belong on a checklist**: only the task's creator and its assignee take one off (`step remove <task id> <its number or id> --reason "<why>"`, `--retire` for a highlight), so answer with the call, and pass the request to them when they are not the asker.
   - **`task_unblocked` for a task nobody holds**: it is on the frontier; dismiss it.
3. Nothing sits silently. A notification with a message behind it that you leave alone gets `reply <notification id> "skipped: <reason>"`; one about a task or a relation alone gets `dismiss <notification id>`. Give `reply` and `forward` the notification's id, not its message's, so they settle it as they post.

To correct a message you sent, edit it where it stands: `message edit <message id> "<the whole new text>"`.

## Between passes

Never block your turn on the inbox, and never poll it (D130, "No agent polls: fwd costs no tokens until a notification arrives"): no timer, no loop, no sleep, no `inbox` called again and again, and no `fwd watch`, which nothing in Codex is woken by. fwd does not yet wake an idle Codex thread when a notification arrives (D140, "A lead receives inbox notifications through an automatically established watcher", is being implemented), so after a pass, end your turn saying so; you run the next pass when you are woken. Keep each pass short.

When you are stopped for good, finish the pass, then retire yourself as your last call, `principal retire <your name>` (D123, "A subagent retires its own user when its job ends"), so that your scopes are free for the next agent.

## Caps per pass

At most 10 links, 5 task creations, 3 approvals, rejections or reviews, and 20 replies. When a comment's meaning is unclear, ask instead of acting, in its thread: `reply <notification id> "I think this concerns <task title>; confirm?"`. Never start tasks and never edit code.
