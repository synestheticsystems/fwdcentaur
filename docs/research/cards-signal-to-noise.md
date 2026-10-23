# Cards, recursion and signal: a design proposal

**Superseded the same night** (Daniel, 2026-09-26: relations render as lozenges with hover detail and click-to-navigate, and only messages hang beneath a card as rows; `docs/DECISIONS.md`). Kept as the record of the analysis; the principles of one figure on a ground, one line at rest and designing for a reference-happy writer carry over.

Proposed 2026-09-26 (night), for Daniel's review before code (task itq8VAXa). What it answers: what a card owes its reader at rest, how far the rows beneath it follow the graph by themselves, and how references are shown when there are many. The trigger was one card in the Koinon XMS inbox, the proposed decision D18: "some things in the inbox are becoming sufficiently noisy (possibly with valid things, but still, TMI) that i can't really understand them ... this is with the references collapsed. it's worse when expanded. Some of this is down to the formatting, some of it is down to the unbounded recursion (which we may need to find a goldilocks amount in some cases)." Manifold's rendering notes and the lessons mined from an earlier knowledge-graph UI of Daniel's (`manifold-outline-rendering.md`) are the prior art; the vocabulary of stretchtext and transclusion is Ted Nelson's.

## What the D18 card was

The decision itself is three lines: a title, a status, one line of the ruling. What made the card unreadable hung beneath it: three tasks cite D18, and each citing task drew as a full task row, with every one of its own citation chips (eight, sixteen and three, since the data-model task rests on sixteen rulings), a note that the same task also shows elsewhere on the page, and its own outline of comments, subtasks and rulings folded behind "21 more below". So the reader met about thirty chips and three collapsed outlines before the one comment that mattered, the triager's digest, and nothing said which of it was new. Two things went wrong at once: the outline recursed along links that are references, not conversation, and each row repeated its whole index of references as prose.

## The reader's question, per context

A card exists to answer one question fast. The question differs by where the reader is, and the resting shape of the card should follow it.

| Where | The reader's question | What must show at rest |
|---|---|---|
| Inbox | What happened, does it need me, what do I do? | Why it is here (one line per notification), the thing itself, what is new in its conversation, the actions |
| Questions | What is being asked, and is it answered? | The question, its answers and follow-ups, the state |
| A decision's panel | What does this ruling say, and what rests on it? | The record, then its references as one-line rows, then its comments |
| A task's panel | What is the state, and what has been said? | The fields, then the conversation, then references as one-line rows |
| Beneath another card (a task or ruling that appears as a child) | Do I need to go there? | One line, and a count of what waits beneath |

## Principles

1. **One figure, on a ground.** A card is one thing, the entity its notifications are about, drawn in full, with the reasons it is here above it. Everything else on the card is ground: one-line rows, counts, marks, expanders. When ground competes with the figure (chips as long as the title, notes as long as the content), the card fails.
2. **Conversation and reference are different links.** Replies, comments, forwards, and the task filed from a message are the conversation: they read in order, the way it happened, and the outline follows them by itself. Citations, subtasks, tasks discovered from a message, dependencies and assignments are references: an index, not a story. The outline never follows a reference by itself. Beneath a card, a reference shows as one line (a citing task, a subtask), and the reader opens it.
3. **Recursion follows the conversation, two levels deep.** Beneath a card's own row, conversation rows open by themselves for two levels (a question, its answers, the replies to those); below that, a count and a caret. A reference row opens nothing by itself: it shows its own summary line ("cites 16 rulings · 20 messages · 3 dependencies") and its outline waits for a click.
4. **What is new decides what is open.** A row that carries an unread notification for the reader, or arrived since the reader last had the card, opens by itself whatever its depth; rows the reader has already seen stay behind counts. The card's job is to show the delta, not the history.
5. **A repeat is a transclusion, not a copy.** An entity that appears twice on a page is one thing. It draws in full once, under the selected card or at its first appearance, and elsewhere as a one-line stub with a mark that leads there. Never two full copies, never two outlines.
6. **Chips are an index, capped, and never repeat the path.** A row's chips summarize its outbound references (its scope, the rulings it rests on). At most three show; the rest collapse to one count that expands in place. A chip for a fallen ruling (repealed or superseded) is a warning and stays visible ahead of the rest. A chip that points at an ancestor on the row's own path is dropped: a task beneath D18 does not say "cites D18".
7. **Every count is stretchtext.** A count ("4 tasks cite it") is a handle: it expands in place, and the reader chooses the depth. The page guesses no deeper than principle 3 allows.
8. **One line at rest.** A row's resting form is one line: who or what, its state, when, its chips; a second line of content only for a message, clipped with an expander. The card's own thing is the exception and shows its full text.
9. **The card says why it is here, in words.** Already built: the reasons head the card and name the thing, never a bare id. The reason line is the strongest figure after the thing itself.
10. **Design for a reference-happy writer.** Agents will cite everything a task touches. The primer can ask them to cite what a task rests on, but the page must stay readable when they do not: counts and caps, not hope.

## The defaults, by context and kind of link

| Context | Conversation links (replies, comments, forwards, task filed from a message) | Reference links (cites, cited by, subtasks, discovered from, dependencies, assignments) |
|---|---|---|
| Inbox card | Open two levels; new rows open at any depth | One line each with a summary count; closed |
| Questions card | Open two levels; new rows open at any depth | One line each; closed |
| Decision panel | Comments open two levels | One line each, grouped ("cited by", "cites", "decided in", "supersedes"), with re-justify marks; closed |
| Task panel | Open two levels | One line each; closed; dependencies and readiness stay in their own section |
| A row beneath any of these | Two levels below its own opening | One line, closed |

The current bound of six levels and fifty children becomes the bound on what the reader can open, not on what opens by itself.

## What the D18 card becomes

At rest: the reason line ("impl-xms proposed D18 (In the PoC, stations and handhelds are ephemeral Ankurah nodes …)"), the decision row with its one-line ruling, one summary line ("cited by 3 tasks (1 open) · decided in 1 task · 1 comment"), and the triager's digest open beneath it because it is new. Opening the citations shows three one-line rows: "Design the XMS core data model and define its Ankurah models · done · cites 15 rulings · 20 messages", and so on, none of them recursing until asked. The thirty chips become three counts; the three folded outlines become three carets; the "also under" notes become marks.

## What this asks of the code

The outline already declares, per model, which links it follows and keys every row by its appearance (`referents.rs`, `thread.rs`), so the change is a policy, not a rewrite: each link gets a class (conversation or reference) and each context an opening policy (how many conversation levels open; whether new rows open), the summary line is computed from listings the way the decision row's already is, and chips are filtered against the row's path. The interim rules built tonight (chips beyond three collapse, "also under" as a mark, a decision row closed behind a summary) are the first three of these applied by hand; the design makes them the rule for every model.

## Open for Daniel

- Whether "new" should open rows by itself (principle 4), or whether new rows should only be marked and counted. The lean: open, since the inbox is for the delta.
- Whether a reference ever recurses by itself anywhere. The lean: never; the panels show references one level deep, as one-line rows, and the reader opens them.
- The chip cap (three) and the summary line's wording.
- Whether the primer should tell agents to cite what a task rests on rather than everything it touches. The lean: yes, one sentence, and design for the worst case anyway.
