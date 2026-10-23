# Roadmap

Draft for discussion, 2026-09-24. No dates; exit criteria instead. P unless marked.

## P0: the vertical slice, CLI only

Deliverables: `fwd-core`, `fwd`; the daemon hosting a durable Ankurah node per project over SQLite, the CLI as an ephemeral node over the WebSocket connector, the auto-spawn contract; Principal, Repo, Task, Relation, Scope, Message, Notification, CodeAnchor models; the rules as client pre-checks and a daemon validation service on Ankurah's `PermissiveAgent`; identity captured at `fwd init`; full EntityIds as identifiers; the routing service with the default rules; `fwd watch`; the Claude Code skills `fwd-implementer`, `fwd-triage` (until 2026-09-29 `fwd-supervisor`) and `fwd-lead` with hooks; anchors created from the CLI with resolution against blobs and the worktree (no editor yet); the daemon-served web UI (`fwd ui`: inbox, tasks, graph, scopes; `design/web-ui.md`).

Exit criteria: fwdcentaur's own backlog lives in fwdcentaur; one implementer session and one triage session run on it for a week; a human comment made with `fwd comment` reaches the implementer mid-task through the Stop hook and the PostToolBatch pop, and through the socket push on Claude Code; `fwd task next` returns anchors with correct current lines after three commits touching the file.

## P1: the extension

Deliverables: editor threads in files and both diff sides; gutter decorations; the graph panel with add, link, propose, comment, filter, focus, Back; the scope view with inbox; `fwd daemon ensure` used by the extension host; the webview as an ephemeral WASM node.

Exit criteria: a comment made in a diff view lands in the implementer's inbox and is linked to a task by the triage agent without the human touching the CLI; the graph panel handles a 500-task fixture with collapsed sub-tasks at interactive speed; orphaned anchors are visible and repositionable.

## P2: central hosting and multi-user (deferred; design when P0 and P1 have landed)

Deliverables: the team topology decision (server as the project's durable node with daemons ephemeral, or durable-to-durable replication); `fwdd --server` with Ankurah's JWT RBAC and the rule referee in `PolicyAgent`; `fwd login`; `fwd project publish` (local to hosted migration) and `join`; principals for teammates keyed by the GitHub ids captured since P0; routing by scope across machines.

Exit criteria: two laptops and one server share a project; comments from one person reach an agent on another machine; the behaviour when the server is unreachable is the chosen one and is reported honestly by the CLI.

## P3: triage driver

Deliverables: `fwd triage` (headless driver with caps, budgets, receipts, restart on context fill); Codex `turn/steer` delivery. GitHub Issues sync is skipped for now and reconsidered later (Q6); if built, it is a projection of milestone-level tasks, never a store.

Exit criteria: the triage agent runs unattended for a week with no action a human reverts more than once a day.

## P4: the business

Deliverables: hosted projects (the server, multi-tenant), governance (SSO, RBAC, audit, retention), a hosted triage agent billed per run with tokens at cost, licenses applied (AGPL server with an enterprise directory, MIT clients and protocol), public repository, docs site.

Exit criteria: a team outside Daniel's own uses a hosted project for a month and pays.

## Ankurah work implied (in priority order)

1. Whatever the team topology chosen at P2 requires: durable-to-durable replication, or nothing new if the server is simply the durable node.
2. Event export/import for project publish (P2).
3. Local-only collections or properties, or a documented pattern for application-side local state (P0 workaround exists).
4. Ref traversal in ankql (nice-to-have; the daemon does two-step queries).
5. RefSet for multi-target relations (later).
6. Multi-model membership for components (later; would let Task and Message become components).

## Dogfooding

fwdcentaur tracks itself from P0, and Ankurah's work items live in the same project so the graph crosses the two repositories from the start. The first triage agent is a subagent of Daniel's own Claude session, the lead, running the skill; the first implementer is the session that builds the extension.
