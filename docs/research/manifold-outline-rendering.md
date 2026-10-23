# The message outline: Manifold ideas

Research note, 2026-09-26. Sources, read and not run: an earlier knowledge-graph UI of Daniel's (TypeScript; a tree of view-model nodes over a live graph store, drawn by Preact components), Manifold's design packet (`ankurah/manifold` at `4c23574`), and the Ankurah revision fwd-web pins. At Daniel's request the earlier project is not named in this repository.

## What this is for

The page is about to draw each message as an outline: beneath it, every entity that refers to it (replies, tasks created from it, then their comments and subtasks), recursively, each level fed by its own live query (`docs/DECISIONS.md`, the 2026-09-26 entry on messages rendering their referents as an outline). That outline seeds a shared graph UI component library under Manifold, and a graph query language may later let Ankurah do the recursion. This note exists so the outline is built once, in a shape that survives both steps. The earlier UI drew live outlines of mixed graph items with a subscription per level; each idea below says what it did, what that solved or hit, and what to do in fwd-web now and in the library later. Manifold's own intentions follow separately.

## Manifold ideas from an earlier knowledge-graph UI

### Key every row by its appearance, never by its position

**Earlier UI.** View-model nodes sat between the data and the components. A child list built a node when its source reported an addition, reordered that node on a move and destroyed it on a removal, so a row kept its focus and state while it stayed. Chat rows were keyed by that node's identity, but outline rows by entity id plus index, so an insertion remounted every row below it; and each row's DOM id was the bare entity id, duplicated whenever an entity showed twice.

**Now.** Key each row by its appearance: the parent's key, the link that made it a child (`reply_to`, or the relation's id) and the child's entity id. Render each level with a keyed `<For>` and read entity fields inside `move ||` closures. This departs from the `docs/design/web-ui.md` idiom "Re-render a list whole in one closure rather than keying rows with `<For>`": an outline row owns a live query and may hold an open reply box, so a whole re-render of its parent's children would drop and reopen every descendant's query and close the boxes. The idiom's reason, that a keyed row would not redraw when its entity changes, holds only for rows that read their fields once when built, as the inbox's notification rows do; at the pinned Ankurah revision every generated view getter tracks the entity's own broadcast (`derive/src/model/view.rs`), so a keyed row reading fields inside closures redraws. My lean: keep the idiom for flat lists whose rows own nothing, and note in `web-ui.md` that the outline is keyed.

**Later.** The library takes row keys from the host, never from position or the bare entity id.

### Children point at their parent, and the link decides where and in what order they show

**Earlier UI.** A child held a link to its parent carrying a role and a fractional sequence number; a parent found its children by asking for incoming links with one role, so one entity could show in several views, each following its own roles. That was its whole "target ref" abstraction; "renderable" meant only content types drawn inline. Ordering was weak: outline siblings had no tie-break, and because an in-place sequence change did not reliably re-sort, reordering wrote a new link just as re-parenting did, so either move destroyed and rebuilt the item's node.

**Now.** fwdcentaur already has this shape: a reply holds `reply_to`, a comment holds `task`, and a task created from a message is linked by a `discovered_from` or `motivated_by` relation whose `to_id` is the message. Write down, as data, which kinds of link each kind of parent follows: under a message, its replies and the tasks linked to it that way; under a task, its root comments, subtasks and tasks discovered from it. Root comments are `task = ?` with no `reply_to`, because `ops::post_message` copies the parent's task onto a reply, which would otherwise show twice. A child reached through a relation takes two steps (the relation, then the task by id); show a placeholder until the task arrives. Order siblings by `created`, then id (ULIDs, so every client agrees), where the children are found, not in the view.

**Later.** That code becomes the trait "for what refers to an entity" named in the `DECISIONS.md` entry: per child, the appearance key, the link (a ref field, or a relation and its kind), the child's kind and id, and the order key; per node, whether its children are loading, complete, truncated or unavailable. Ankurah's draft navigation spec (`specs/unified-refs-edges-json`, whose inbound-navigation phase is pending) would let one predicate select relation-reached children, but it covers WHERE clauses only, so a tree-shaped result is a separate request, and it expects ref-like fields where fwdcentaur's relation endpoints are plain `EntityId`.

### Share one live query per entity and kind of link, and count them

**Earlier UI.** One object per entity held one live query over its incoming links, and each appearance filtered it on the client, so two appearances cost one subscription. Lifetimes followed a hand-maintained ownership graph and leaks were a standing worry: "leaking memory" TODOs, a diagnostics route counting active queries and live objects, and a final, unfinished commit that tears subscriptions down on garbage collection. For bulk traversal (an export), per-entity queries were "far too slow/expensive" and gave way to scans sorted by parent id and merged.

**Now.** Keep child queries in a page-wide cache keyed by entity and kind of link, counting holders: a row takes a handle on mount and releases it on unmount, and the last release drops the `LiveQuery`, which unsubscribes it at the daemon (`core/src/livequery/inner.rs`); a short grace lets re-expansion reuse it. Per-node queries also beat filtering the page-wide snapshots in `store.rs` once per node, since a change then wakes only the queries it matches. Count open outline queries behind a debug flag, and test that collapsing everything or switching the acting principal restores the baseline. One leak sits below fwd-web: the reactive_graph bridge (`signals/src/reactive_graph.rs`) keeps an entry for every Ankurah signal ever tracked and never removes it, so a query per row grows that map for the page's life; small now, worth a prune in ankurah-signals.

**Later.** The library owns no queries: children come from per-node queries now and one recursive query later, and for wide scopes from one kind of link fetched across the scope and grouped by parent, as the export learned.

### Keep focus, drafts and expansion outside the rows

**Earlier UI.** Focus was a pointer into the view-model tree and the chat's draft lived in its node, so a remount lost nothing. When a live update destroyed the focused node, focus fell back to the page root, although a comment names the nearest focusable ancestor as the ideal target. Before a write that would create an item, a pending focus (a matcher each node checked as it bound its element) was registered first, because the new node could bind during the write. Manifold's review of the same UI found no explicit exception keeping an active editor alive when its content was culled.

**Now.** Keep page-level signals keyed by appearance: the focused row, the expanded set, drafts (the inbox already keys drafts by notification) and a pending focus. When the focused key disappears, focus its next sibling, else its previous sibling, else its parent, never a look-alike row. After posting a reply, set the pending focus to the parent's key and the new message's id; `ops::post_message` returns the id only after the commit, when the row may already be mounted, so check mounted rows on setting it as well as on each mount, and expire it after a few seconds. A row with an open reply box or a draft stays mounted when its parent collapses.

**Later.** The library keeps this state host-owned and serializable, since Manifold counts expansion and reading position among what Back restores.

### Let the editor keep the arrow keys until it reaches an edge

**Earlier UI.** Keys went from `window` to the focused node, then through its behaviors and its ancestors', each answering stop, decline, native, continue or ignore. A text field kept Up and Down until its caret reached the first or last line; the outline then moved focus to the previous or next focusable visible node, carrying the caret's horizontal position and entry edge, so the caret kept its column. Escape went to the nearest focusable ancestor. Tests sent key events to the focused node and asserted on the tree, not the DOM.

**Now.** The rows are mostly read-only, so one keydown handler on the outline container, reading the focused-row signal, is enough: Up and Down follow the visible order, Left collapses or goes to the parent, Right expands or enters the first child, Enter opens the reply box, and Escape closes it and refocuses the row. In the textarea the arrows are its own, except Up on its first line and Down on its last, which leave for the neighbouring row. Mark the outline up as a WAI-ARIA tree (`treeitem`, `aria-level`, `aria-expanded`, one tabbable row), and compute the visible order in a plain Rust function so it is tested natively.

**Later.** Manifold wants the outline to be the complete accessible alternative to its geometric views, so the library ships keyboard and tree semantics first. It also asks for "a focused application contract, not a second general UI framework": the earlier UI had replaced the browser's event routing, focus and hit testing with its own, and the library should keep only the traversal order and the focus key.

### Stop at a cycle with a reference you can follow, and choose how duplicates show

**Earlier UI.** An outline item whose entity already appeared among its ancestors took an empty child list and looked like a leaf; a nested view making the same check drew a visible placeholder instead. An entity under two parents drew in full under both, and the registry of on-screen nodes kept only the first per entity id.

**Now.** `motivated_by` and `discovered_from` carry no acyclicity rule, and even a `subtask_of` cycle that arrives by sync is recorded rather than dropped (`docs/design/data-model.md`, the relation invariants and the cycle rule). Check a row's ancestor path for the child's id before opening its queries; at a cycle, draw one row that names the entity and, when activated, moves focus to its earlier appearance. Keep a page map from entity id to mounted appearance keys: mark an entity shown twice ("also under message 3") and leave a repeat collapsed while the entity is expanded elsewhere. Decide what a root is: if the roots are all unread messages, an unread reply shows as a root and again under its parent; listing thread roots (a reply's `thread`) and marking unread messages in place avoids that within a thread.

**Later.** The component owns cycle and duplicate handling, driven by the host's entity ids; a cut is always a reference, never an empty list.

### Bound the recursion, and never draw an unknown as empty

**Earlier UI.** The outline had no collapse, depth or count limit and no windowing; only cycles and visibility bounded it (a node loaded children only while visible, and cards went invisible after a delay but visible at once, against flicker). A row drew nothing until the property choosing its form had loaded, keeping "not loaded" apart from "none".

**Now.** Open a node's child queries only while it is expanded and mounted. Expand the roots and two levels by default, plus unread messages, and cap a node at about fifty children with "show more". A collapsed node shows its count if its query is open and a plain expander if not; an unanswered query shows "loading" and a failed one its error, never a leaf.

**Later.** The component draws loading, complete, truncated (with a count) and unavailable differently. When a project outgrows full drawing, window the flattened visible rows the keyboard already computes, keeping focused and drafting rows mounted.

### Choose each row by what its entity carries

**Earlier UI.** It had no renderer registry. An appearance property on the entity chose an item's form (bullet, checkbox, plain or highlight), the body showed an editor, a file icon or a favicon by content type, and a chat message added a tool-call viewer or a task-status spinner when its entity carried those properties. Mixed items shared one list without a type hierarchy, with kind decisions scattered across conditional slots.

**Now.** One `match` on the child's kind, in one place: a message row (author, age, content, status, Reply), a task row (title, status pill, readiness, and the label of the link that placed it: "discovered from", "motivated by", "subtask"), and a fallback row with the short id for an unknown kind, never skipped. Reuse the Tasks page's labels and pills.

**Later.** The renderer keys on the components an entity carries (the pinned Ankurah already records a set of model memberships per entity, `core/src/entity/entity.rs`), composes what each contributes, and gives missing, inaccessible and unknown kinds a visible row.

### Treat a live query's membership as a hint

**Earlier UI.** Its query layer carried workarounds for result sets that disagreed with their entities: a comment on snapshots "missing data" after a server read, locally removed ids kept "to not add back to the set", and a switch to ignore removals of records still active.

**Now.** fwdcentaur has met the same problem: a notification whose status changed "has been seen to linger" in its result set (`crates/fwd-web/src/app.rs`, the unread badge). Build rows from the entity's current state and hide one whose entity no longer matches the link that placed it. A new reply pushes later rows down, so keep CSS scroll anchoring and hold insertions while a pointer is pressed.

**Later.** Manifold's watch contract, below, gives the library a delivery shape with coalescing and reset.

## What Manifold's own notes already intend

Daniel's caveat applies ("it's early days for that yet"), and the packet labels most of this as proposals; `manifold-rendering.md` and `manifold-crossover.md` summarize it. What bears on the outline:

- **A chosen traversal, and the accessible view.** The rendering methodology (`outputs/12-rendering-methodology.md`, its table of inquiries) answers "What context can I explore from here?" with a "Query-grown outline/tree plus cross-relations", a traversal rather than the graph's hierarchy, so links outside the followed kinds (a task's dependencies) appear in a row as labelled references. The interaction contract (`outputs/03-interaction-and-projection.md`, navigation and accessibility) makes the outline "a complete alternative to geometric interaction", says "Bound live recursion and show cycles as references", and puts expansion state in what Back restores.
- **Identities and actions kept apart.** The same contract separates an item, a projection result and an "OccurrenceId: one contextual appearance"; the domain note (`outputs/02-domain-and-state.md`, archive, delete and undo) separates removing an appearance, unlinking a relation and archiving an item, and makes undo a compensating command that offers a diff when another writer touched the field. So dismissing a notification, resolving a message and retracting a relation stay distinct actions in the outline.
- **Live data as keyed diffs.** The architecture note (`outputs/04-architecture-and-trust.md`, the projection owner) proposes broad per-workspace subscriptions, application indexes and "deterministic full recomputation ... then keyed diffs"; watch (`outputs/06-cli-api.md`) sends an "ordered initial snapshot, keyed batches, status and reset"; the Ankurah memo (`outputs/07-ankurah-feedback.md`) keeps empty, unavailable, denied, stale and incomplete distinct, and names as an optional migration native composite results "behind the same source/result/occurrence model". The code that finds a node's children is that seam, and the decision log says not to wait for "the whole future graph/query system".
- **The toolkit keeps its ordinary behaviour.** The renderer note (`outputs/05-renderer-and-precedents.md`, component reuse) reuses the toolkit's controls, focus, text entry and keyboard conventions, and holds that "Component ownership is not graph membership or adjacency"; the scaling note (`outputs/17-scaling-and-dense-rendering.md`, lifecycle) gives live editors only to an active subset. Omitted children read "(+n)" when counted, "…" when not, and a distinct unknown otherwise (`outputs/16-bubble-sets-and-membership.md`).

## The five things to do first

1. In fwd-web, one natively testable module that answers what refers to an entity: the kinds of link each parent kind follows, as data; each child with its appearance key, link label, kind, id and (`created`, id) order; each node loading, complete, truncated or unavailable. The library extracts it as its trait.
2. A keyed `<For>` per level on the appearance key, with fields read inside closures, and a line in `docs/design/web-ui.md` limiting the whole-list idiom to flat lists.
3. Focus, expansion, drafts and pending focus in page signals keyed by appearance; one container keyboard handler over the computed visible order; ARIA tree markup.
4. Reference rows at cycles, marks on repeated appearances, and bounded depth and child count with explicit loading, count and "…" states.
5. A holder-counted cache of child queries, an open-query count with a test that collapsing or switching principals restores the baseline, and a request to ankurah-signals to prune bridge entries whose broadcast is gone.

## Pitfalls to avoid

- An index in a row key, or a DOM id from the bare entity id: rows below each insertion remount, and ids repeat.
- Re-rendering a parent's children whole: descendants' queries reopen and reply boxes close.
- An empty child list for a cycle, a collapsed node or an unanswered query: each reads as a leaf.
- Sorting without a total tie-break: equal keys swap between clients and reloads.
- Moving a child by removing and re-adding it: its row, focus and draft are rebuilt.
- Refocusing the page root, or a look-alike row, when the focused row disappears.
- Checking a pending focus only on mount, after a write whose row already arrived.
- One query per appearance, or query lifetimes nobody counts.
- Trusting result-set membership over the entity's own state.
- A reply shown twice, under its task and under the message it answers.
- Undo that replays a local closure over another writer's change.
- A recursive Leptos component that does not erase its view type (`.into_any()`) at the recursion.
