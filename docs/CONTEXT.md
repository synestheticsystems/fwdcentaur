# fwdcentaur: project context

Started 2026-09-24. Working name: **fwdcentaur** ("forward centaur": a human and a coding agent working as one unit, the way centaur chess teams did). Owner: Daniel Norman.

## What it is for

Humans and coding agents lose the thread between them. The reasons for a task evaporate between planning and implementation and across context compaction; a human's observation about a specific line of code has nowhere durable to live except a pull-request comment that goes "outdated" on the next push; and nobody can see which person or agent is working on what, with what intention. fwdcentaur exists to hold that shared thread: a fine-grained graph of the work (what is to be done, why, in what order), the code it touches, and the messages between the people and agents doing it.

## The pieces

- **A task graph**: tasks with dependencies, sub-tasks, motivations ("why"), provenance ("discovered from"), stored as finely as the work is actually divided.
- **Code anchors**: comments on specific lines or ranges of source, in the editor and in diffs, anchored so they survive the code changing underneath them and link into the graph. The graph therefore has code parts as well as task parts.
- **Projects and scopes**: a project is the task graph and its database; it spans one or more repositories, each identified by its upstream, and any number of local checkouts. A scope is an area of work, which anyone may work in and several people and agents share: its status, an articulated intention, the repository and branches its work happens on, the checkouts bound to it on each machine (local-only, not synced), and the tasks worked in it. It has no owner: who does a task is the task's assignee (D108, "A scope has no owner", 2026-09-29, which narrowed the vision's scopes per team member, D1).
- **Messages and inboxes**: everything a person or agent says is a durable message (a comment on code or on a task, a question, a proposal) with a status; a notification is created only when a specific party needs to know, and the inbox lists them. A human's comment on a line of code notifies the agent whose work it concerns and the project's triage agent, the agent that keeps the graph tidy.
- **A CLI intended for agents** (`fwd`): an ephemeral Ankurah node and the only interface agents use, so any agent harness with a shell can play either role. The daemon's MCP endpoint serves the same commands as tools for a harness that speaks MCP (`design/mcp.md`). The same binary, re-executed as `fwd daemon serve`, is the daemon that hosts the project's durable node.
- **A VS Code extension**: a graph panel (visualize, add, remove, propose, comment) and line comments in code and diff views that route to inboxes.
- **A web UI served by the daemon** (`fwd ui`): the inbox and the task graph in a browser, an ephemeral Ankurah node in WASM (Leptos) running the same rules and operations as the CLI.
- **Two agent roles**: the *implementing agent* works in a scope, takes tasks, and receives inbox notifications about them mid-session; the *triage* agent, one per scope (or one for a small project whole), which each agent selects for itself from a fixed set (lead, triage, implementer, reviewer), hears the questions asked of no one in particular about the work of the scopes it serves, new annotations and proposals, links and relinks tasks, amends descriptions, answers from what the graph records, and relays the rest to whoever can answer or decide it: a Sonnet agent that annotates and relays rather than judging the work (D129, "The triage agent is a Sonnet agent that annotates and relays", 2026-10-01).

## Foundations and relationships

- **Ankurah** (`github.com/ankurah/ankurah`, Rust, MIT/Apache-2.0), Daniel's local-first, event-sourced reactive database, is the storage and sync layer. Daniel owns it, so fwdcentaur can drive features into it rather than working around gaps. fwdcentaur tracks the `storage-engine-contract` line (0.10.0, unpublished) pinned by hash since 2026-09-25 (`DECISIONS.md`). State of the line: SQLite/Sled/Postgres/IndexedDB engines and, from the same day, a SQLite engine over Node's built-in `node:sqlite` for WASM nodes in Node.js, WebSocket durable server and clients, JWT RBAC with row-level scope filters, typed `Ref<T>` references, live queries, generated WASM/TypeScript bindings and React hooks. Node roles (Daniel): the daemon hosts the project's durable node; the CLI and the extension are ephemeral nodes over the WebSocket connector and require the durable node online. v1 is single-user and local-only on Ankurah's `PermissiveAgent`, with a GitHub user id captured from the start; hosting and multi-user come later. See `research/manifold-crossover.md` for the 0.9.3 characterization.
- **Ankurah Manifold** (`github.com/ankurah/manifold`, private), a design packet for a local knowledge environment on Ankurah. fwdcentaur may become a Manifold application. Its daemon lifecycle, CLI grammar, relation-as-entity rule, interior-pointing contract and task model are adopted here; its renderer decision and knowledge-item types are not. See `research/manifold-crossover.md` and `research/manifold-rendering.md`.
- **ankurah-ts** (`github.com/ankurah/ankurah-ts`), a transpiled TypeScript port of Ankurah targeting React Native. As of 2026-09-02 it is not usable as a runtime (the transpiler's emission step is in flight), so the VS Code extension cannot embed an Ankurah node in TypeScript yet; it talks to the local daemon instead.
- **sutra** (`github.com/synestheticsystems/sutra`), Daniel's dev-environment status dashboard, reads a well-known state folder of per-unit status files. Since 2026-09-26 the daemon keeps its card there, one row named `daemon` carrying its port and the number of projects open (`design/daemon.md`, "Status for sutra"); still not a dependency, since the daemon runs without a registry it can write.

## Business intent

Free to self-host; a business on top. The research in `research/business-models.md` points at: AGPL for the server with a proprietary enterprise directory, MIT/Apache for the CLI, extension and protocol, Ankurah staying MIT/Apache; revenue from a hosted team project with governance (SSO, RBAC, audit) and a hosted triage agent billed per run with tokens at cost.

## Documents

- `research/` holds the landscape notes (mainstream trackers, agent-native trackers, business models, agent inbox mechanics, Manifold crossover and rendering) and `research/findings.md`, the synthesis.
- `design-draft.md` is the design proposal for discussion.
- `DECISIONS.md` records what Daniel has established, what is proposed, and what is open, following the Manifold convention of separating requirements from proposals.
